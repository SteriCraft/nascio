//! The client's horizon tiles ([`nascio_chunk::horizon`]): asked to their source, kept by key
//! until the horizon moves away from them.
//!
//! In local flight they are computed here from the terrain (development); connected, the
//! server sends them.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use bevy::prelude::Resource;
use nascio_chunk::horizon::{HorizonTile, TileKey};
use nascio_voxel::Terrain;

use crate::fly::Workers;

/// Where the tiles come from.
pub enum TileSource {
    /// Computed from the local world (`--fly`, benchmarks, captures).
    Local(Arc<Terrain>),
}

#[derive(Resource)]
pub struct TileStore {
    source: TileSource,
    /// Looked up by key only: no iteration order reaches a result.
    have: HashMap<TileKey, Arc<HorizonTile>>,
    asked: HashSet<TileKey>,
    tx: Sender<HorizonTile>,
    rx: Mutex<Receiver<HorizonTile>>,
}

impl TileStore {
    pub fn new(source: TileSource) -> Self {
        let (tx, rx) = channel();
        Self {
            source,
            have: HashMap::new(),
            asked: HashSet::new(),
            tx,
            rx: Mutex::new(rx),
        }
    }

    /// Takes the tiles that arrived.
    pub fn poll(&mut self) {
        let rx = self.rx.lock().unwrap_or_else(|e| e.into_inner());
        for t in rx.try_iter() {
            self.asked.remove(&t.key);
            self.have.insert(t.key, Arc::new(t));
        }
    }

    /// Asks the tiles of `keys` not here yet; true once they all are.
    pub fn ensure(&mut self, keys: &BTreeSet<TileKey>, workers: &Workers) -> bool {
        let mut all = true;
        for &key in keys {
            if self.have.contains_key(&key) {
                continue;
            }
            all = false;
            if !self.asked.insert(key) {
                continue;
            }
            match &self.source {
                TileSource::Local(terrain) => {
                    let terrain = terrain.clone();
                    let tx = self.tx.clone();
                    workers.pool.spawn(move || {
                        let _ = tx.send(nascio_voxel::horizon::horizon_tile(&terrain, key));
                    });
                }
            }
        }
        all
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
