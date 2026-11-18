//! What the server sends for the far view (spec §7, §8): horizon tiles, built on demand and
//! shared by every client, and `WorldSky` (settings, reduced climate, far ground), built once at
//! start.
//!
//! Both show the generated world: the players' edits do not reach the horizon (known limit,
//! `decisions.md`). Neither carries geology nor the seed (`crates/protocol/tests/no_geology.rs`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, ensure};
use nascio_chunk::horizon::TileKey;
use nascio_core::climate::Harmonics;
use nascio_protocol::sky::{
    CLIMATE_RES, Field, GROUND_RES, GroundColors, HARMONIC_QUANTITIES, ReducedClimate, SkyWorld,
    WorldSky, components,
};
use nascio_voxel::Terrain;
use nascio_worldgen::world_io::{Layer, World};
use tokio::sync::{OnceCell, Semaphore};

type Cell = Arc<OnceCell<Arc<[u8]>>>;

/// Encoded horizon tiles, least recently used dropped beyond the cap.
pub struct TileCache {
    terrain: Arc<Terrain>,
    /// Tile, cell, last use.
    map: Mutex<BTreeMap<TileKey, (Cell, u64)>>,
    clock: AtomicU64,
    cap: usize,
    builders: Arc<Semaphore>,
    /// Tiles built, bytes built (statistics).
    pub built: AtomicU64,
    pub built_bytes: AtomicU64,
}

impl TileCache {
    pub fn new(terrain: Arc<Terrain>, cap: usize, threads: usize) -> Self {
        Self {
            terrain,
            map: Mutex::new(BTreeMap::new()),
            clock: AtomicU64::new(0),
            cap: cap.max(64),
            builders: Arc::new(Semaphore::new(threads.max(1))),
            built: AtomicU64::new(0),
            built_bytes: AtomicU64::new(0),
        }
    }

    /// The compressed tile, built if needed.
    pub async fn get(self: &Arc<Self>, key: TileKey) -> anyhow::Result<Arc<[u8]>> {
        let cell = {
            let now = self.clock.fetch_add(1, Ordering::Relaxed);
            let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
            let cell = {
                let e = map.entry(key).or_insert_with(|| (Cell::default(), now));
                e.1 = now;
                e.0.clone()
            };
            if map.len() > self.cap + self.cap / 8 {
                let mut uses: Vec<(u64, TileKey)> = map.iter().map(|(k, e)| (e.1, *k)).collect();
                uses.sort_unstable();
                for (_, k) in uses.into_iter().take(map.len() - self.cap) {
                    map.remove(&k);
                }
            }
            cell
        };
        let bytes = cell
            .get_or_try_init(|| async {
                let _permit = self.builders.acquire().await?;
                let me = self.clone();
                let bytes = tokio::task::spawn_blocking(move || me.build(key))
                    .await
                    .context("tile builder")??;
                Ok::<_, anyhow::Error>(Arc::<[u8]>::from(bytes))
            })
            .await?;
        Ok(bytes.clone())
    }

    pub fn build(&self, key: TileKey) -> anyhow::Result<Vec<u8>> {
        let tile = nascio_voxel::horizon::horizon_tile(&self.terrain, key);
        let bytes = nascio_protocol::horizon::encode_tile(&tile)?;
        self.built.fetch_add(1, Ordering::Relaxed);
        self.built_bytes
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        Ok(bytes)
    }
}

/// Values of a layer averaged (or sampled, if its side is not a multiple) on `res²` cells.
fn reduce(layer: &Layer, res: usize, f: impl Fn(usize) -> f64) -> Vec<f64> {
    let (w, h) = (layer.width, layer.height);
    let mut out = Vec::with_capacity(res * res);
    for j in 0..res {
        for i in 0..res {
            if w % res == 0 && h % res == 0 {
                let (rx, rz) = (w / res, h / res);
                let mut sum = 0.0;
                for fj in j * rz..(j + 1) * rz {
                    for fi in i * rx..(i + 1) * rx {
                        sum += f(fj * w + fi);
                    }
                }
                out.push(sum / (rx * rz) as f64);
            } else {
                let fi = ((i as f64 + 0.5) * w as f64 / res as f64) as usize;
                let fj = ((j as f64 + 0.5) * h as f64 / res as f64) as usize;
                out.push(f(fj.min(h - 1) * w + fi.min(w - 1)));
            }
        }
    }
    out
}

fn layer<'a>(world: &'a World, name: &str) -> anyhow::Result<&'a Layer> {
    world
        .layer(name)
        .with_context(|| format!("the world has no {name} layer (generated before M5?)"))
}

/// The reduced climate: the [`nascio_protocol::sky::FIELDS`] on [`CLIMATE_RES`]² cells.
pub fn reduced_climate(world: &World) -> anyhow::Result<ReducedClimate> {
    let mut fields = Vec::new();
    for q in HARMONIC_QUANTITIES {
        let parts = ["x0", "a1", "p1", "a2", "p2"]
            .map(|s| layer(world, &format!("{q}_{s}")))
            .into_iter()
            .collect::<anyhow::Result<Vec<_>>>()?;
        let w = parts[0].width;
        ensure!(
            parts
                .iter()
                .all(|l| l.width == w && l.height == parts[0].height),
            "{q}: harmonics of different sizes"
        );
        let harmonic = |k: usize| Harmonics {
            x0: parts[0].data.get_f64(k),
            a1: parts[1].data.get_f64(k),
            p1: parts[2].data.get_f64(k),
            a2: parts[3].data.get_f64(k),
            p2: parts[4].data.get_f64(k),
        };
        // Averaged as cartesian components: the phases wrap, their components do not.
        for c in 0..5 {
            let v = reduce(parts[0], CLIMATE_RES, |k| components(&harmonic(k))[c]);
            fields.push(Field::new(&v));
        }
    }
    let diurnal = layer(world, "diurnal_range")?;
    fields.push(Field::new(&reduce(diurnal, CLIMATE_RES, |k| {
        diurnal.data.get_f64(k)
    })));
    // Mean height of the cell, the sea counted as 0 m (the climate's own cell height).
    let e = layer(world, "elevation")?;
    fields.push(Field::new(&reduce(e, CLIMATE_RES, |k| {
        e.data.get_f64(k).max(0.0)
    })));
    Ok(ReducedClimate { fields })
}

/// `WorldSky` of a world, compressed.
pub fn world_sky(world: &World, terrain: &Terrain) -> anyhow::Result<Vec<u8>> {
    let c = &world.config;
    WorldSky {
        world: SkyWorld {
            clock: c.clock,
            astro: c.astro,
            lapse_rate_c_per_m: c.climate.lapse_rate_c_per_m,
            atmosphere: c.atmosphere,
        },
        climate: reduced_climate(world)?,
        ground: GroundColors {
            rgb: nascio_voxel::horizon::ground_colors(terrain, GROUND_RES),
        },
    }
    .compress()
}
