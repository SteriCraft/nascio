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

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use nascio_chunk::horizon::spacing;
    use nascio_client::Auth;
    use nascio_server::config::SecretLogin;
    use nascio_server::{RunningServer, ServerConfig};
    use nascio_worldgen::config::{Scale, WorldConfig};
    use nascio_worldgen::generate::generate_full;

    use super::*;

    /// Asks `keys` until they are all here (or panics after 60 s).
    fn fill(store: &mut TileStore, keys: &BTreeSet<TileKey>, workers: &Workers) {
        let start = Instant::now();
        loop {
            store.poll();
            if store.ensure(keys, workers, start.elapsed().as_secs_f64()) {
                return;
            }
            assert!(start.elapsed() < Duration::from_secs(60), "tiles timed out");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A second connection to the same world reads `WorldSky` and the tiles it already got
    /// from the disk cache: the server sends neither again, and no tile is even asked.
    #[test]
    fn a_second_connection_reads_the_disk_cache() {
        let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
            .expect("world")
            .world;
        let base = std::env::temp_dir().join(format!("nascio-client-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _rt = rt.enter();
        let cfg = ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            chunk_radius_m: 96,
            client_budget_bytes_per_s: 4 << 20,
            generation_threads: 4,
            open_registration: true,
            secret_login: SecretLogin::All,
            spawn_xz: Some([50_000.0, 50_000.0]),
            discovery_port: 0,
            ..ServerConfig::default()
        };
        let server = RunningServer::start(&world, cfg, &base.join("server")).unwrap();
        let shared = server.shared.clone();
        let root = base.join("client-cache");
        let auth = || Auth::Secret("test-secret".into());
        let workers = Workers::new();

        // First connection: the sky and the tiles come from the server.
        let a = Remote::start_in(
            server.addr,
            server.cert_der.clone(),
            "player_a",
            auth(),
            root.clone(),
        )
        .unwrap();
        assert_eq!(shared.sent_skies.load(Ordering::Relaxed), 1);
        let dir = a.cache.clone().expect("a cache folder");
        assert!(dir.starts_with(&root));
        let (x, z) = (a.spawn[0], a.spawn[2]);
        // The tiles holding the spawn on the first three levels and their east neighbours.
        let keys: BTreeSet<TileKey> = (0..3u8)
            .flat_map(|level| {
                let s = spacing(level);
                let (k, _) =
                    TileKey::containing(level, (x / s).floor() as i64, (z / s).floor() as i64);
                [k, TileKey { i: k.i + 1, ..k }]
            })
            .collect();
        let mut store = TileStore::new(TileSource::Remote(a.clone()));
        fill(&mut store, &keys, &workers);
        let tiles = shared.sent_tiles.load(Ordering::Relaxed);
        assert_eq!(tiles, keys.len() as u64, "each tile sent once");
        for &k in &keys {
            assert!(tile_file(&dir, k).is_file(), "tile {k:?} not cached");
        }

        // Second connection: everything from the disk.
        let b = Remote::start_in(
            server.addr,
            server.cert_der.clone(),
            "player_b",
            auth(),
            root.clone(),
        )
        .unwrap();
        assert_eq!(b.cache.as_deref(), Some(dir.as_path()));
        assert_eq!(
            b.sky.climate, a.sky.climate,
            "the cached sky is the server's"
        );
        let mut store = TileStore::new(TileSource::Remote(b.clone()));
        assert!(
            store.ensure(&keys, &workers, 0.0),
            "every tile read from the disk at once"
        );
        for &k in &keys {
            assert!(store.get(&k).is_some());
        }
        // Give a stray request time to be served.
        std::thread::sleep(Duration::from_secs(2));
        assert_eq!(
            shared.sent_skies.load(Ordering::Relaxed),
            1,
            "sky sent again"
        );
        assert_eq!(
            shared.sent_tiles.load(Ordering::Relaxed),
            tiles,
            "tiles sent again"
        );

        // Leaving: the server drops both players at once (not after its idle timeout).
        assert_eq!(shared.list().len(), 2);
        a.disconnect();
        b.disconnect();
        let left = Instant::now();
        while !shared.list().is_empty() {
            assert!(
                left.elapsed() < Duration::from_secs(5),
                "players still listed"
            );
            std::thread::sleep(Duration::from_millis(50));
        }

        server.shutdown().unwrap();
        drop(_rt);
        let _ = std::fs::remove_dir_all(&base);
    }
}
