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
//!
//! The player's own edits show at once: they are kept over the columns ([`PendingEdits`])
//! until the server confirms or refuses them, and every read ([`Mirror::block`], meshing,
//! the predicted physics) sees them.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use nascio_chunk::block::AIR;
use nascio_chunk::{Chunk, PaddedChunk};
use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS};
use nascio_core::coords::{BlockPos, ChunkPos, LocalPos};
use nascio_protocol::chunk::Column;

use crate::edits::{PendingEdit, PendingEdits};

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
    /// The player's edits not answered yet, shown over the columns.
    pending: PendingEdits,
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

    /// Blocks changed in chunk `chunk` (from a `ServerMsg::Blocks`). Returns the cells whose
    /// shown block changed (a confirmed prediction changes nothing). A batch newer than its
    /// column also answers the predicted edits of its cells ([`PendingEdits::confirm`]).
    pub fn blocks(&mut self, chunk: [i32; 3], lsn: u64, changes: Vec<(u16, u16)>) -> Vec<BlockPos> {
        let k = key(chunk[0], chunk[2]);
        let origin = ChunkPos::new(chunk[0], chunk[1], chunk[2]);
        let cells: Vec<(BlockPos, u16)> = changes
            .iter()
            .map(|&(i, id)| (BlockPos::from_chunk_local(origin, local_of(i)), id))
            .collect();
        let before: Vec<Option<u16>> = cells.iter().map(|&(p, _)| self.block(p)).collect();
        let fresh = self.columns.get(&k).is_none_or(|c| lsn > c.version);
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
        if fresh {
            for &(p, id) in &cells {
                self.pending.confirm(p, id);
            }
        }
        cells
            .iter()
            .zip(before)
            .filter(|&(&(p, _), b)| self.block(p) != b)
            .map(|(&(p, _), _)| p)
            .collect()
    }

    /// The player put `block` at `pos`: shown at once, until the server answers. False (and
    /// nothing kept) if the cell is not known or already holds that block.
    pub fn predict(&mut self, pos: BlockPos, block: u16, now: Instant) -> bool {
        match self.block(pos) {
            Some(original) if original != block => {
                self.pending.push(
                    pos,
                    PendingEdit {
                        block,
                        original,
                        at: now,
                    },
                );
                true
            }
            _ => false,
        }
    }

    /// The server refused the edit at `pos` (`ServerMsg::EditRefused`): its oldest predicted
    /// edit goes. True if the shown block changed.
    pub fn refused(&mut self, pos: BlockPos) -> bool {
        let before = self.block(pos);
        self.pending.refuse(pos);
        self.block(pos) != before
    }

    /// Predicted edits left unanswered for `timeout` go; returns the cells whose shown block
    /// changed.
    pub fn expire(&mut self, now: Instant, timeout: Duration) -> Vec<BlockPos> {
        let shown: Vec<(BlockPos, Option<u16>)> = self
            .pending
            .iter()
            .map(|(p, _)| (p, self.block(p)))
            .collect();
        let gone = self.pending.expire(now, timeout);
        shown
            .into_iter()
            .filter(|(p, b)| gone.contains(p) && self.block(*p) != *b)
            .map(|(p, _)| p)
            .collect()
    }

    /// The predicted edits not answered yet.
    pub fn pending(&self) -> &PendingEdits {
        &self.pending
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

    /// The block at `p` (session block id), if its column is held, with the player's
    /// predicted edits. Above the chunks of a column there is only air (the server's range
    /// covers the surface, water, plants and every modified chunk); below them the block is
    /// unknown.
    pub fn block(&self, p: BlockPos) -> Option<u16> {
        let held = self.server_block(p)?;
        Some(self.pending.get(p).unwrap_or(held))
    }

    /// The block at `p` as the server sent it (no prediction).
    pub fn server_block(&self, p: BlockPos) -> Option<u16> {
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
        // The predicted edits, over the server's blocks.
        let o = pos.origin();
        for (q, id) in self.pending.iter() {
            let (dx, dy, dz) = o.delta_to(q);
            if [dx, dy, dz].iter().all(|d| (-1..=n).contains(d)) {
                p.set(dx, dy, dz, id);
            }
        }
        Some(p)
    }
}

/// Columns whose mesh shows the cell `p`: its own, plus the neighbour in x and/or z when the
/// cell lies on that border (the mesh of a column reads a one-block border, corners
/// included: a corner cell is also in the diagonal column). Keys wrapped on the torus.
pub fn columns_for_cell(p: BlockPos) -> Vec<(i32, i32)> {
    let n = CHUNK_SIZE;
    let (cx, cz) = (p.x.div_euclid(n), p.z.div_euclid(n));
    let side = |l: i32| match l {
        0 => Some(-1),
        l if l == n - 1 => Some(1),
        _ => None,
    };
    let (sx, sz) = (side(p.x.rem_euclid(n)), side(p.z.rem_euclid(n)));
    let mut v = Vec::with_capacity(4);
    for dx in [Some(0), sx].into_iter().flatten() {
        for dz in [Some(0), sz].into_iter().flatten() {
            v.push(key(cx + dx, cz + dz));
        }
    }
    v
}

/// Local position of index `i` of a chunk (`x`, then `z`, then `y`).
fn local_of(i: u16) -> LocalPos {
    let (i, n) = (i32::from(i), CHUNK_SIZE);
    LocalPos {
        x: (i % n) as u8,
        z: ((i / n) % n) as u8,
        y: (i / (n * n)) as u8,
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
        chunk.set(local_of(i), id);
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

    fn col_at(cx: i32, cz: i32, version: u64) -> Column {
        Column {
            cz,
            ..col(cx, version)
        }
    }

    /// A mirror holding column (0, 0) at version 5, a cell in it and its chunk index.
    fn held() -> (Mirror, BlockPos, [i32; 3], u16) {
        let mut m = Mirror::default();
        m.column(col(0, 5));
        let p = BlockPos::new(3, 3, 5);
        let c = p.chunk();
        (m, p, [c.cx, c.cy, c.cz], p.local().index() as u16)
    }

    #[test]
    fn a_confirmed_prediction_changes_nothing_on_screen() {
        let (mut m, p, chunk, idx) = held();
        let t0 = Instant::now();
        assert!(m.predict(p, 0, t0));
        assert_eq!(m.block(p), Some(0));
        assert_eq!(m.server_block(p), Some(1));
        assert!(m.blocks(chunk, 6, vec![(idx, 0)]).is_empty());
        assert!(m.pending().is_empty());
        assert_eq!(m.block(p), Some(0));
    }

    #[test]
    fn the_server_may_answer_another_block() {
        let (mut m, p, chunk, idx) = held();
        assert!(m.predict(p, 2, Instant::now()));
        let changed = m.blocks(chunk, 6, vec![(idx, 3)]);
        assert!(changed.contains(&p));
        assert_eq!(m.block(p), Some(3));
        assert!(m.pending().is_empty());
    }

    #[test]
    fn a_refused_prediction_restores_the_block() {
        let (mut m, p, _, _) = held();
        assert!(m.predict(p, 0, Instant::now()));
        assert!(m.refused(p));
        assert_eq!(m.block(p), Some(1));
        assert!(m.pending().is_empty());
        assert!(!m.refused(p));
    }

    #[test]
    fn an_unanswered_prediction_expires() {
        let (mut m, p, _, _) = held();
        let t0 = Instant::now();
        let s = Duration::from_secs(1);
        assert!(m.predict(p, 0, t0));
        assert!(m.expire(t0 + Duration::from_millis(999), s).is_empty());
        assert_eq!(m.block(p), Some(0));
        assert_eq!(m.pending().len(), 1);
        assert_eq!(m.expire(t0 + s, s), vec![p]);
        assert_eq!(m.block(p), Some(1));
    }

    #[test]
    fn chained_edits_on_one_cell_do_not_flicker() {
        let (mut m, p, chunk, idx) = held();
        let t0 = Instant::now();
        assert!(m.predict(p, 0, t0));
        assert!(m.predict(p, 2, t0));
        assert_eq!(m.block(p), Some(2));
        assert!(m.blocks(chunk, 6, vec![(idx, 0)]).is_empty());
        assert_eq!(m.block(p), Some(2));
        assert!(m.blocks(chunk, 7, vec![(idx, 2)]).is_empty());
        assert!(m.pending().is_empty());
        assert_eq!(m.block(p), Some(2));
    }

    #[test]
    fn predictions_cross_the_seam() {
        let mut m = Mirror::default();
        for cz in -1..=1 {
            for cx in -1..=1 {
                m.column(col_at(cx, cz, 5));
            }
        }
        let p = BlockPos::new(-1, 3, 5);
        assert!(m.predict(p, 0, Instant::now()));
        assert_eq!(m.block(BlockPos::new(99_999, 3, 5)), Some(0));
        let padded = m.padded_chunk(ChunkPos::new(0, 0, 0)).expect("padded");
        assert_eq!(padded.get(-1, 3, 5), 0);
        assert_eq!(padded.get(-1, 3, 6), 1, "the neighbour cell is untouched");
        let idx = p.local().index() as u16;
        assert!(m.blocks([-1, 0, 0], 6, vec![(idx, 0)]).is_empty());
        assert!(m.pending().is_empty());
    }

    #[test]
    fn nothing_is_predicted_where_nothing_is_known_or_changed() {
        let (mut m, p, _, _) = held();
        let t0 = Instant::now();
        let far = BlockPos::new(500, 3, 5);
        assert!(!m.predict(far, 0, t0));
        assert!(m.pending().is_empty());
        assert!(!m.predict(p, 1, t0), "already that block");
        assert!(m.pending().is_empty());
    }

    #[test]
    fn a_stale_batch_does_not_confirm() {
        let (mut m, p, chunk, idx) = held();
        assert!(m.predict(p, 0, Instant::now()));
        m.blocks(chunk, 5, vec![(idx, 0)]);
        assert_eq!(m.pending().len(), 1);
    }

    #[test]
    fn columns_for_cell_covers_borders_and_seam() {
        let set = |x, z| -> BTreeSet<(i32, i32)> {
            columns_for_cell(BlockPos::new(x, 0, z))
                .into_iter()
                .collect()
        };
        let want = |v: &[(i32, i32)]| -> BTreeSet<(i32, i32)> { v.iter().copied().collect() };
        let n = CHUNKS_PER_AXIS - 1;
        assert_eq!(set(40, 40), want(&[(1, 1)]));
        assert_eq!(set(32, 40), want(&[(1, 1), (0, 1)]));
        assert_eq!(set(63, 40), want(&[(1, 1), (2, 1)]));
        assert_eq!(set(40, 63), want(&[(1, 1), (1, 2)]));
        assert_eq!(set(32, 32), want(&[(1, 1), (0, 1), (1, 0), (0, 0)]));
        assert_eq!(set(0, 40), want(&[(0, 1), (n, 1)]));
        assert_eq!(set(99_999, 0), want(&[(n, 0), (0, 0), (n, n), (0, n)]));
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
