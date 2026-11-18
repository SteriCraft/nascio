//! Compressed chunk columns, built once and shared by every client.
//!
//! A column is `generate ⊕ delta` for every chunk of its range (the generator's range plus
//! the modified chunks), encoded with the protocol's codec v2 and compressed. Building runs
//! on blocking threads, bounded by a semaphore; concurrent requests for the same column wait
//! for the same build. An edit drops the column from the cache.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use nascio_core::consts::CHUNKS_PER_AXIS;
use nascio_core::coords::ChunkPos;
use nascio_protocol::chunk::Column;
use nascio_voxel::chunk::ChunkDeltas;
use nascio_voxel::{ColumnSet, DeltaStore, Terrain};
use tokio::sync::{OnceCell, Semaphore};

type Cell = Arc<OnceCell<Arc<[u8]>>>;

pub struct ColumnCache {
    terrain: Arc<Terrain>,
    deltas: Arc<DeltaStore>,
    /// Column, cell, last use.
    map: Mutex<BTreeMap<(i32, i32), (Cell, u64)>>,
    clock: AtomicU64,
    cap: usize,
    builders: Arc<Semaphore>,
    /// Columns built, bytes built (statistics).
    pub built: AtomicU64,
    pub built_bytes: AtomicU64,
}

/// Wraps column coordinates on the torus.
pub fn wrap_column(cx: i32, cz: i32) -> (i32, i32) {
    (
        cx.rem_euclid(CHUNKS_PER_AXIS),
        cz.rem_euclid(CHUNKS_PER_AXIS),
    )
}

impl ColumnCache {
    pub fn new(terrain: Arc<Terrain>, deltas: Arc<DeltaStore>, cap: usize, threads: usize) -> Self {
        Self {
            terrain,
            deltas,
            map: Mutex::new(BTreeMap::new()),
            clock: AtomicU64::new(0),
            cap: cap.max(16),
            builders: Arc::new(Semaphore::new(threads.max(1))),
            built: AtomicU64::new(0),
            built_bytes: AtomicU64::new(0),
        }
    }

    /// The compressed column `(cx, cz)`, built if needed.
    pub async fn get(self: &Arc<Self>, cx: i32, cz: i32) -> anyhow::Result<Arc<[u8]>> {
        let k = wrap_column(cx, cz);
        let cell = {
            let now = self.clock.fetch_add(1, Ordering::Relaxed);
            let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
            let cell = {
                let e = map.entry(k).or_insert_with(|| (Cell::default(), now));
                e.1 = now;
                e.0.clone()
            };
            if map.len() > self.cap + self.cap / 8 {
                // Drop the least recently used eighth.
                let mut uses: Vec<(u64, (i32, i32))> = map.iter().map(|(k, e)| (e.1, *k)).collect();
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
                let bytes = tokio::task::spawn_blocking(move || me.build(k.0, k.1))
                    .await
                    .context("column builder")??;
                Ok::<_, anyhow::Error>(Arc::<[u8]>::from(bytes))
            })
            .await?;
        Ok(bytes.clone())
    }

    /// Drops a column (it was edited).
    pub fn invalidate(&self, cx: i32, cz: i32) {
        let k = wrap_column(cx, cz);
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&k);
    }

    /// Builds a column: generation, deltas, codec v2, zstd.
    pub fn build(&self, cx: i32, cz: i32) -> anyhow::Result<Vec<u8>> {
        // The version is read before the chunks: an edit missing from them is numbered above.
        let version = self.deltas.region_lsn(cx, cz);
        let col = ColumnSet::new(&self.terrain, cx, cz);
        let range = col.chunk_range(self.terrain.config().underground_chunks);
        let touched = self.deltas.touched_ys(cx, cz);
        let lo = (*range.start()).min(touched.first().copied().unwrap_or(i32::MAX));
        let hi = (*range.end()).max(touched.last().copied().unwrap_or(i32::MIN));
        let chunks = (lo..=hi)
            .map(|cy| {
                let mut c = col.padded_chunk(&self.terrain, cy).inner();
                self.deltas.apply(ChunkPos::new(cx, cy, cz), &mut c);
                c.compact();
                c
            })
            .collect();
        let bytes = Column {
            cx,
            cz,
            cy_min: lo,
            version,
            chunks,
        }
        .compress()?;
        self.built.fetch_add(1, Ordering::Relaxed);
        self.built_bytes
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        Ok(bytes)
    }
}
