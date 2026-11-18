//! The columns a client holds, kept up to date by the server's `Blocks` messages.
//!
//! Ordering rule (proposed spec §7.3): a column carries the number (`version`) of the last
//! journal record of its region when the server read it; a `Blocks` batch carries the number
//! of its last edit. A batch is applied to a column of lower version (values are absolute,
//! so edits already in the column are replayed harmlessly) and kept while its column is on
//! its way (columns and control messages travel on different streams).

use std::collections::BTreeMap;

use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS};
use nascio_core::coords::{BlockPos, ChunkPos, LocalPos};
use nascio_protocol::chunk::Column;
use nascio_voxel::Chunk;
use nascio_voxel::block::AIR;

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
}

fn key(cx: i32, cz: i32) -> (i32, i32) {
    (
        cx.rem_euclid(CHUNKS_PER_AXIS),
        cz.rem_euclid(CHUNKS_PER_AXIS),
    )
}

impl Mirror {
    /// A column arrived: replaces the one held and applies the batches newer than it.
    pub fn column(&mut self, mut c: Column) {
        let k = key(c.cx, c.cz);
        if let Some(batches) = self.waiting.remove(&k) {
            for b in batches {
                apply(&mut c, &b);
            }
        }
        self.columns.insert(k, c);
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

/// Applies a batch to a column of lower version; chunks outside the column are added as air.
fn apply(c: &mut Column, b: &Batch) {
    if b.lsn <= c.version {
        return;
    }
    // Grow the column to the batch's chunk (the server sends the full column again later).
    while b.cy < c.cy_min {
        c.chunks.insert(0, Chunk::filled(0));
        c.cy_min -= 1;
    }
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
}
