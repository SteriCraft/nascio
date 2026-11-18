//! The blocks the simulation moves the players against: decoded columns, owned by the
//! simulation thread.
//!
//! A column is decoded from the very bytes [`ColumnCache`] sends to the clients, and read
//! like the client's `Mirror` reads it: air above its chunks, unknown below them. The
//! server's simulation and the client's prediction thus see the same blocks, which the
//! bit-exact reconciliation needs.
//!
//! A missing column is built and decoded by a tokio task and handed back through a channel;
//! the simulation never waits for it (the physics sees an unknown cell meanwhile). The edits
//! of a tick are applied in place, like `Mirror::blocks`; a column on its way when one of its
//! chunks is edited is asked again on arrival.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use nascio_chunk::{BlockId, Chunk};
use nascio_core::consts::CHUNK_SIZE;
use nascio_core::coords::{BlockPos, LocalPos};
use nascio_protocol::chunk::Column;

use crate::columns::{ColumnCache, wrap_column};

type Key = (i32, i32);

struct Held {
    col: Column,
    /// Tick of the last use.
    used: u64,
}

pub struct WorldView {
    held: BTreeMap<Key, Held>,
    /// Columns asked and not back yet; `true`: edited meanwhile (ask again on arrival).
    pending: BTreeMap<Key, bool>,
    cache: Arc<ColumnCache>,
    rt: tokio::runtime::Handle,
    max_id: BlockId,
    cap: usize,
    clock: u64,
    tx: Sender<(Key, anyhow::Result<Column>)>,
    rx: Receiver<(Key, anyhow::Result<Column>)>,
}

impl WorldView {
    /// At most about `cap` columns held (the least recently used beyond are dropped).
    pub fn new(
        cache: Arc<ColumnCache>,
        rt: tokio::runtime::Handle,
        blocks: usize,
        cap: usize,
    ) -> Self {
        let (tx, rx) = channel();
        Self {
            held: BTreeMap::new(),
            pending: BTreeMap::new(),
            cache,
            rt,
            max_id: blocks.saturating_sub(1).min(usize::from(BlockId::MAX)) as BlockId,
            cap: cap.max(16),
            clock: 0,
            tx,
            rx,
        }
    }

    /// Start of a tick: takes in the columns that arrived, drops the oldest beyond the cap.
    pub fn begin_tick(&mut self) {
        self.clock += 1;
        while let Ok((k, col)) = self.rx.try_recv() {
            let stale = self.pending.remove(&k).unwrap_or(true);
            match col {
                Ok(col) if !stale => {
                    self.held.insert(
                        k,
                        Held {
                            col,
                            used: self.clock,
                        },
                    );
                }
                Ok(_) => self.request(k),
                Err(e) => crate::say_err!("column {k:?} for the simulation: {e:#}"),
            }
        }
        if self.held.len() > self.cap + self.cap / 8 {
            // Never a column used during the last tick.
            let mut uses: Vec<(u64, Key)> = self
                .held
                .iter()
                .filter(|(_, h)| h.used + 1 < self.clock)
                .map(|(k, h)| (h.used, *k))
                .collect();
            uses.sort_unstable();
            let over = self.held.len() - self.cap;
            for (_, k) in uses.into_iter().take(over) {
                self.held.remove(&k);
            }
        }
    }

    /// Asks the column `(cx, cz)` if neither held nor on its way; marks it used.
    pub fn want(&mut self, cx: i32, cz: i32) {
        let k = wrap_column(cx, cz);
        if let Some(h) = self.held.get_mut(&k) {
            h.used = self.clock;
        } else if !self.pending.contains_key(&k) {
            self.request(k);
        }
    }

    /// Asks every column within `margin` metres of `(x, z)` (horizontally).
    pub fn want_around(&mut self, x: f64, z: f64, margin: f64) {
        let n = f64::from(CHUNK_SIZE);
        let (x0, x1) = (
            ((x - margin) / n).floor() as i32,
            ((x + margin) / n).floor() as i32,
        );
        let (z0, z1) = (
            ((z - margin) / n).floor() as i32,
            ((z + margin) / n).floor() as i32,
        );
        for cz in z0..=z1 {
            for cx in x0..=x1 {
                self.want(cx, cz);
            }
        }
    }

    /// Whether every column within `margin` metres of `(x, z)` is held.
    pub fn has_around(&self, x: f64, z: f64, margin: f64) -> bool {
        let n = f64::from(CHUNK_SIZE);
        let cols = |a: f64| ((a - margin) / n).floor() as i32..=((a + margin) / n).floor() as i32;
        cols(z).all(|cz| cols(x).all(|cx| self.held.contains_key(&wrap_column(cx, cz))))
    }

    fn request(&mut self, k: Key) {
        self.pending.insert(k, false);
        let (cache, tx, max_id) = (self.cache.clone(), self.tx.clone(), self.max_id);
        self.rt.spawn(async move {
            let col = match cache.get(k.0, k.1).await {
                Ok(bytes) => {
                    tokio::task::spawn_blocking(move || Column::decompress(&bytes, max_id))
                        .await
                        .map_err(anyhow::Error::from)
                        .and_then(|c| c)
                }
                Err(e) => Err(e),
            };
            let _ = tx.send((k, col));
        });
    }

    /// Blocks changed in chunk `(cx, cy, cz)`, edit number `lsn`: applied to the column held
    /// (like the client's mirror); a column on its way is asked again when it arrives. Below a
    /// column's chunks the column is dropped and asked again (the server's rebuilt column
    /// covers the edited chunk; the client's mirror would wrongly add air there).
    pub fn blocks(&mut self, chunk: [i32; 3], lsn: u64, changes: &[(u16, u16)]) {
        let k = wrap_column(chunk[0], chunk[2]);
        if let Some(p) = self.pending.get_mut(&k) {
            *p = true;
            return;
        }
        let Some(h) = self.held.get_mut(&k) else {
            return;
        };
        let c = &mut h.col;
        if lsn <= c.version {
            return;
        }
        if chunk[1] < c.cy_min {
            self.held.remove(&k);
            self.request(k);
            return;
        }
        while chunk[1] >= c.cy_min + c.chunks.len() as i32 {
            c.chunks.push(Chunk::filled(0));
        }
        let ch = &mut c.chunks[(chunk[1] - c.cy_min) as usize];
        let n = CHUNK_SIZE;
        for &(i, id) in changes {
            let i = i32::from(i);
            ch.set(
                LocalPos {
                    x: (i % n) as u8,
                    z: ((i / n) % n) as u8,
                    y: (i / (n * n)) as u8,
                },
                id,
            );
        }
        ch.compact();
        c.version = lsn;
    }

    /// The block at a cell (`x`, `z` wrapped or not): air above a column's chunks, `None`
    /// below them or when the column is not held.
    pub fn block(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        let p = BlockPos::new(x, y, z);
        let c = p.chunk();
        let col = &self.held.get(&wrap_column(c.cx, c.cz))?.col;
        let i = c.cy - col.cy_min;
        if i < 0 {
            return None;
        }
        Some(match col.chunks.get(i as usize) {
            Some(ch) => ch.get(p.local()),
            None => nascio_chunk::block::AIR,
        })
    }

    /// Columns held, and columns on their way.
    pub fn len(&self) -> (usize, usize) {
        (self.held.len(), self.pending.len())
    }

    /// Bytes of the decoded chunks held.
    pub fn heap_bytes(&self) -> usize {
        self.held
            .values()
            .map(|h| h.col.chunks.iter().map(Chunk::heap_bytes).sum::<usize>())
            .sum()
    }

    /// Columns held (tests).
    pub fn held(&self) -> BTreeSet<Key> {
        self.held.keys().copied().collect()
    }
}

impl nascio_physics::BlockSource for WorldView {
    fn block(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        WorldView::block(self, x, y, z)
    }
}
