//! The client's horizon tiles ([`nascio_chunk::horizon`]): asked to their source, kept by key
//! until the horizon moves away from them.
//!
//! In local flight they are computed here from the terrain (development). Connected, they come
//! from the disk cache of the server's world, else from the server (asked in batches, asked
//! again if one has not come after [`ASK_AGAIN_S`]), and go to the disk cache on arrival
//! ([`crate::remote`]).

use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use bevy::prelude::Resource;
use nascio_chunk::horizon::{HorizonTile, TileKey};
use nascio_protocol::horizon::{MAX_TILES_PER_REQUEST, decode_tile};
use nascio_protocol::messages::ClientMsg;
use nascio_voxel::Terrain;

use crate::fly::Workers;
use crate::remote::{Remote, tile_file};

/// A tile asked to the server and not come after this long is asked again, s.
const ASK_AGAIN_S: f64 = 10.0;

/// Where the tiles come from.
pub enum TileSource {
    /// Computed from the local world (`--fly`, benchmarks, captures).
    Local(Arc<Terrain>),
    /// Sent by the server (and its disk cache).
    Remote(Remote),
}

#[derive(Resource)]
pub struct TileStore {
    source: TileSource,
    /// Looked up by key only: no iteration order reaches a result.
    have: HashMap<TileKey, Arc<HorizonTile>>,
    /// Tiles asked, and when.
    asked: HashMap<TileKey, f64>,
    tx: Sender<HorizonTile>,
    rx: Mutex<Receiver<HorizonTile>>,
}

impl TileStore {
    pub fn new(source: TileSource) -> Self {
        let (tx, rx) = channel();
        Self {
            source,
            have: HashMap::new(),
            asked: HashMap::new(),
            tx,
            rx: Mutex::new(rx),
        }
    }

    fn keep(&mut self, t: HorizonTile) {
        self.asked.remove(&t.key);
        self.have.insert(t.key, Arc::new(t));
    }

    /// Takes the tiles that arrived.
    pub fn poll(&mut self) {
        let mut came: Vec<HorizonTile> = self
            .rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_iter()
            .collect();
        if let TileSource::Remote(r) = &self.source {
            came.append(&mut r.tiles.lock().unwrap_or_else(|e| e.into_inner()));
        }
        for t in came {
            self.keep(t);
        }
    }

    /// Asks the tiles of `keys` not here yet (`now`: seconds); true once they all are.
    pub fn ensure(&mut self, keys: &BTreeSet<TileKey>, workers: &Workers, now: f64) -> bool {
        let mut ask = Vec::new();
        let mut from_disk = Vec::new();
        for &key in keys {
            if self.have.contains_key(&key) {
                continue;
            }
            if let Some(&at) = self.asked.get(&key)
                && (matches!(self.source, TileSource::Local(_)) || now - at < ASK_AGAIN_S)
            {
                continue;
            }
            match &self.source {
                TileSource::Local(terrain) => {
                    self.asked.insert(key, now);
                    let terrain = terrain.clone();
                    let tx = self.tx.clone();
                    workers.pool.spawn(move || {
                        let _ = tx.send(nascio_voxel::horizon::horizon_tile(&terrain, key));
                    });
                }
                TileSource::Remote(r) => {
                    let cached = r.cache.as_deref().and_then(|d| {
                        let bytes = std::fs::read(tile_file(d, key)).ok()?;
                        decode_tile(&bytes).ok().filter(|t| t.key == key)
                    });
                    match cached {
                        Some(t) => from_disk.push(t),
                        None => {
                            self.asked.insert(key, now);
                            ask.push(key);
                        }
                    }
                }
            }
        }
        let all = ask.is_empty() && from_disk.len() + self.count(keys) == keys.len();
        for t in from_disk {
            self.keep(t);
        }
        if let TileSource::Remote(r) = &self.source {
            for batch in ask.chunks(MAX_TILES_PER_REQUEST) {
                r.send(ClientMsg::Tiles {
                    keys: batch.to_vec(),
                });
            }
        }
        all
    }

    fn count(&self, keys: &BTreeSet<TileKey>) -> usize {
        keys.iter().filter(|k| self.have.contains_key(k)).count()
    }

    pub fn get(&self, key: &TileKey) -> Option<&Arc<HorizonTile>> {
        self.have.get(key)
    }

    /// Forgets the tiles outside `keys` once more than twice as many are kept.
    pub fn trim(&mut self, keys: &BTreeSet<TileKey>) {
        if self.have.len() > 2 * keys.len() {
            self.have.retain(|k, _| keys.contains(k));
        }
    }
}
