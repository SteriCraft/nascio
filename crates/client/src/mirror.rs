//! The columns a client holds, kept up to date by the server's `Blocks` messages.
//!
//! Ordering rule (proposed spec §7.3): a column carries the number (`version`) of the last
//! journal record of its region when the server read it; a `Blocks` batch carries the number
//! of its last edit. A batch is applied to a column of lower version (values are absolute,
//! so edits already in the column are replayed harmlessly) and kept while its column is on
//! its way (columns and control messages travel on different streams).
//!
//! A batch below the chunks of a column (digging deeper than the server's range) cannot be
//! applied: what lies under the column is unknown, not air. The column is forgotten (its
//! cells become unknown, the batches wait) and asked again ([`Mirror::take_resends`]:
//! `ClientMsg::Resend`); the server's new column covers the edited chunk.

use std::collections::{BTreeMap, BTreeSet};

use nascio_chunk::block::AIR;
use nascio_chunk::{Chunk, PaddedChunk};
use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS};
use nascio_core::coords::{BlockPos, ChunkPos, LocalPos};
use nascio_protocol::chunk::Column;

/// Blocks of one chunk changed in a tick.
#[derive(Clone, Debug)]
struct Batch {
    cy: i32,
    lsn: u64,
    changes: Vec<(u16, u16)>,
}

/// Columns received, by `(cx, cz)` (wrapped).
#[derive(Default)]
pub struct Mirror {
    columns: BTreeMap<(i32, i32), Column>,
    waiting: BTreeMap<(i32, i32), Vec<Batch>>,
    /// Columns asked again and not received yet.
    asked: BTreeSet<(i32, i32)>,
    /// Columns to ask again (taken by the caller).
    resend: Vec<[i32; 2]>,
}

/// A batch newer than the column falls below its chunks.
fn below(c: &Column, b: &Batch) -> bool {
    b.lsn > c.version && b.cy < c.cy_min
}

fn key(cx: i32, cz: i32) -> (i32, i32) {
    (
        cx.rem_euclid(CHUNKS_PER_AXIS),
        cz.rem_euclid(CHUNKS_PER_AXIS),
    )
}

impl Mirror {
    /// A column arrived: replaces the one held and applies the batches newer than it.
    /// A column older than a batch below its chunks is not kept: the batches wait, and the
    /// column is asked again (unless it already is).
    pub fn column(&mut self, mut c: Column) {
        let k = key(c.cx, c.cz);
        if let Some(batches) = self.waiting.get(&k) {
            if batches.iter().any(|b| below(&c, b)) {
                self.columns.remove(&k);
                self.ask(k);
                return;
            }
            for b in self.waiting.remove(&k).unwrap_or_default() {
                apply(&mut c, &b);
            }
        }
        self.asked.remove(&k);
        self.columns.insert(k, c);
    }

    fn ask(&mut self, k: (i32, i32)) {
        if self.asked.insert(k) {
            self.resend.push([k.0, k.1]);
        }
    }

    /// Columns to ask the server again (`ClientMsg::Resend`), since the last call.
    pub fn take_resends(&mut self) -> Vec<[i32; 2]> {
        std::mem::take(&mut self.resend)
    }

    /// Blocks changed in chunk `chunk` (from a `ServerMsg::Blocks`).
    pub fn blocks(&mut self, chunk: [i32; 3], lsn: u64, changes: Vec<(u16, u16)>) {
        let k = key(chunk[0], chunk[2]);
        let b = Batch {
            cy: chunk[1],
            lsn,
            changes,
        };
        match self.columns.get_mut(&k) {
            Some(c) if below(c, &b) => {
                self.columns.remove(&k);
                self.waiting.entry(k).or_default().push(b);
                self.ask(k);
            }
            Some(c) => apply(c, &b),
            None => self.waiting.entry(k).or_default().push(b),
        }
    }

    /// The server dropped these columns from the client's range.
    pub fn forget(&mut self, columns: &[[i32; 2]]) {
        for c in columns {
            let k = key(c[0], c[1]);
            self.columns.remove(&k);
            self.waiting.remove(&k);
            self.asked.remove(&k);
        }
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    pub fn has(&self, cx: i32, cz: i32) -> bool {
        self.columns.contains_key(&key(cx, cz))
    }

    /// The column at `(cx, cz)`, if held.
    pub fn get(&self, cx: i32, cz: i32) -> Option<&Column> {
        self.columns.get(&key(cx, cz))
    }

    /// The chunk at `pos`, if its column is held and covers it.
    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        let c = self.columns.get(&key(pos.cx, pos.cz))?;
        let i = usize::try_from(pos.cy - c.cy_min).ok()?;
        c.chunks.get(i)
    }

    /// The block at `p` (session block id), if its column is held. Above the chunks of a
    /// column there is only air (the server's range covers the surface, water, plants and
    /// every modified chunk); below them the block is unknown.
    pub fn block(&self, p: BlockPos) -> Option<u16> {
        let c = p.chunk();
        let col = self.columns.get(&key(c.cx, c.cz))?;
        if c.cy >= col.cy_min + col.chunks.len() as i32 {
            return Some(AIR);
        }
        Some(self.chunk(c)?.get(p.local()))
    }
}

impl Mirror {
    /// Whether the column of `pos` and its eight neighbours are held: the chunk can then be
    /// meshed ([`padded_chunk`](Self::padded_chunk)).
    pub fn can_mesh(&self, cx: i32, cz: i32) -> bool {
        (-1..=1).all(|dz| (-1..=1).all(|dx| self.has(cx + dx, cz + dz)))
    }

    /// Chunk `pos` with its one-block border, for meshing; `None` until its column and the
    /// eight neighbouring ones are held. Above a column there is air; below its chunks (never
    /// seen by a player: rock under the generator's range) each local column repeats its
    /// lowest block, which hides the faces exactly as the solid rock would.
    pub fn padded_chunk(&self, pos: ChunkPos) -> Option<PaddedChunk> {
        if !self.can_mesh(pos.cx, pos.cz) {
            return None;
        }
        let mut p = PaddedChunk::filled(AIR);
        let n = CHUNK_SIZE;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let col = self.get(pos.cx + dx, pos.cz + dz)?;
                for dy in -1..=1 {
                    let cy = pos.cy + dy;
                    let (ox, oy, oz) = (dx * n, dy * n, dz * n);
                    // Local range of this neighbour that falls in the padded chunk.
                    let r = |d: i32| match d {
                        -1 => (n - 1)..n,
                        0 => 0..n,
                        _ => 0..1,
                    };
                    let top = col.cy_min + col.chunks.len() as i32;
                    if cy >= top {
                        continue; // air, already there
                    }
                    for ly in r(dy) {
                        for lz in r(dz) {
                            for lx in r(dx) {
                                let id = if cy < col.cy_min {
                                    col.chunks[0].get(LocalPos {
                                        x: lx as u8,
                                        y: 0,
                                        z: lz as u8,
                                    })
                                } else {
                                    col.chunks[(cy - col.cy_min) as usize].get(LocalPos {
                                        x: lx as u8,
                                        y: ly as u8,
                                        z: lz as u8,
                                    })
                                };
                                p.set(lx + ox, ly + oy, lz + oz, id);
                            }
                        }
                    }
                }
            }
        }
        Some(p)
    }
}

/// Applies a batch to a column of lower version; chunks above the column are added as air
/// (a batch below it never comes here: see [`below`]).
fn apply(c: &mut Column, b: &Batch) {
    if b.lsn <= c.version {
        return;
    }
    debug_assert!(b.cy >= c.cy_min, "a batch below its column");
    while b.cy >= c.cy_min + c.chunks.len() as i32 {
        c.chunks.push(Chunk::filled(0));
    }
    let chunk = &mut c.chunks[(b.cy - c.cy_min) as usize];
    for &(i, id) in &b.changes {
        let i = i32::from(i);
        let n = CHUNK_SIZE;
        let p = LocalPos {
            x: (i % n) as u8,
            z: ((i / n) % n) as u8,
            y: (i / (n * n)) as u8,
        };
        chunk.set(p, id);
    }
    chunk.compact();
    c.version = b.lsn;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(cx: i32, version: u64) -> Column {
        Column {
            cx,
            cz: 0,
            cy_min: 0,
            version,
            chunks: vec![Chunk::filled(1)],
            tint: nascio_chunk::ChunkTint::NONE,
        }
    }

    #[test]
    fn batches_follow_the_version_rule() {
        let mut m = Mirror::default();
        let p = BlockPos::new(99_999, 3, 5);
        let idx = p.local().index() as u16;
        let c = p.chunk();
        // A batch before its column: kept, then applied (newer than the column).
        m.blocks([c.cx, c.cy, c.cz], 7, vec![(idx, 2)]);
        m.column(col(CHUNKS_PER_AXIS - 1, 5));
        assert_eq!(m.block(p), Some(2));
        // Older than the column: ignored. Newer: applied.
        m.blocks([c.cx, c.cy, c.cz], 7, vec![(idx, 3)]);
        assert_eq!(m.block(p), Some(2));
        m.blocks([c.cx - CHUNKS_PER_AXIS, c.cy, c.cz], 8, vec![(idx, 0)]);
        assert_eq!(m.block(p), Some(0));
        assert_eq!(m.get(-1, 0).unwrap().version, 8);
        // A batch already in a column that arrives later is replayed harmlessly.
        m.blocks([0, 0, 0], 4, vec![(0, 9)]);
        m.column(col(0, 4));
        assert_eq!(m.block(BlockPos::new(0, 0, 0)), Some(1));
        m.forget(&[[CHUNKS_PER_AXIS - 1, 0]]);
        assert!(!m.has(-1, 0));
    }

    #[test]
    fn an_edit_below_a_column_forgets_it_and_asks_it_again() {
        let mut m = Mirror::default();
        m.column(col(0, 5));
        assert_eq!(m.block(BlockPos::new(3, 40, 3)), Some(AIR), "air above");
        // Dug below the column (chunk −1): its cells are unknown, the column asked again.
        m.blocks([0, -1, 0], 6, vec![(0, 0)]);
        assert!(!m.has(0, 0));
        assert_eq!(m.block(BlockPos::new(3, 40, 3)), None);
        assert_eq!(m.take_resends(), vec![[0, 0]]);
        // Another batch waits; the column already on its way (older) is not kept, and not
        // asked twice.
        m.blocks([0, 0, 0], 7, vec![(1, 2)]);
        m.column(col(0, 5));
        assert!(!m.has(0, 0));
        assert!(m.take_resends().is_empty());
        // The server's new column covers the edited chunk.
        let mut fresh = col(0, 6);
        fresh.cy_min = -1;
        fresh.chunks.insert(0, Chunk::filled(0));
        m.column(fresh);
        assert!(m.has(0, 0));
        assert_eq!(m.block(BlockPos::new(0, -32, 0)), Some(0));
        assert_eq!(
            m.block(BlockPos::new(1, 0, 0)),
            Some(2),
            "the later batch applied"
        );
        // An edit at or above the column's chunks is applied as before.
        m.blocks([0, 3, 0], 8, vec![(0, 2)]);
        assert!(m.has(0, 0) && m.take_resends().is_empty());
        assert_eq!(m.block(BlockPos::new(0, 96, 0)), Some(2));
    }
}
