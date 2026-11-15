//! Pure, deterministic chunk generation (spec §5).
//!
//! Generation is column-based: the heights of a chunk column (plus a border) are computed once
//! in a [`ColumnSet`], and any chunk of that column — with its one-block border — is filled from
//! it. The same function fills the border, so neighbouring chunks always agree, across the seam
//! included.

use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS};
use nascio_core::coords::ChunkPos;

use crate::block::{AIR, BlockId, WATER};
use crate::chunk::{Chunk, ChunkDeltas, NP, PaddedChunk};
use crate::terrain::Terrain;

/// Columns stored around a chunk column: local `-2..=33` (one more than the padded border, for
/// the slope).
const NH: usize = NP + 2;

/// Surface data of one chunk column and its border.
#[derive(Clone, Debug)]
pub struct ColumnSet {
    pub cx: i32,
    pub cz: i32,
    /// Top solid block of each column, local `-1..=32`, `x` fastest.
    tops: Vec<i32>,
    /// Surface slope (m/m) of each column, local `-1..=32`.
    slopes: Vec<f32>,
    /// Water fills `top < y < water_top` (sea: 0; lakes: their surface), local `-1..=32`.
    water_tops: Vec<i32>,
}

impl ColumnSet {
    /// Computes the columns of chunk column `(cx, cz)` (wrapped).
    pub fn new(terrain: &Terrain, cx: i32, cz: i32) -> Self {
        let cx = cx.rem_euclid(CHUNKS_PER_AXIS);
        let cz = cz.rem_euclid(CHUNKS_PER_AXIS);
        let (ox, oz) = (cx * CHUNK_SIZE, cz * CHUNK_SIZE);
        let mut heights = vec![0.0f64; NH * NH];
        for lz in 0..NH {
            for lx in 0..NH {
                // Block columns are sampled at their centre.
                let x = f64::from(ox + lx as i32 - 2) + 0.5;
                let z = f64::from(oz + lz as i32 - 2) + 0.5;
                heights[lx + NH * lz] = terrain.height(x, z);
            }
        }
        let mut tops = vec![0; NP * NP];
        let mut slopes = vec![0.0; NP * NP];
        let mut water_tops = vec![0; NP * NP];
        for lz in 0..NP {
            for lx in 0..NP {
                let h = |dx: usize, dz: usize| heights[(lx + dx) + NH * (lz + dz)];
                tops[lx + NP * lz] = h(1, 1).floor() as i32;
                let gx = (h(2, 1) - h(0, 1)) * 0.5;
                let gz = (h(1, 2) - h(1, 0)) * 0.5;
                slopes[lx + NP * lz] = gx.hypot(gz) as f32;
                let x = f64::from(ox + lx as i32 - 1) + 0.5;
                let z = f64::from(oz + lz as i32 - 1) + 0.5;
                water_tops[lx + NP * lz] = terrain.water_top(x, z);
            }
        }
        Self {
            cx,
            cz,
            tops,
            slopes,
            water_tops,
        }
    }

    #[inline]
    fn k(x: i32, z: i32) -> usize {
        (x + 1) as usize + NP * (z + 1) as usize
    }

    /// Top solid block of local column `(x, z)`, each in `-1..=32`.
    #[inline]
    pub fn top(&self, x: i32, z: i32) -> i32 {
        self.tops[Self::k(x, z)]
    }

    /// Water fills local column `(x, z)` for `top < y < water_top`.
    #[inline]
    pub fn water_top(&self, x: i32, z: i32) -> i32 {
        self.water_tops[Self::k(x, z)]
    }

    /// Lowest and highest water top over the column and its border.
    pub fn water_range(&self) -> (i32, i32) {
        let lo = *self.water_tops.iter().min().expect("non-empty");
        let hi = *self.water_tops.iter().max().expect("non-empty");
        (lo, hi)
    }

    #[inline]
    pub fn slope(&self, x: i32, z: i32) -> f64 {
        f64::from(self.slopes[Self::k(x, z)])
    }

    /// Lowest and highest top over the column and its border.
    pub fn top_range(&self) -> (i32, i32) {
        let lo = *self.tops.iter().min().expect("non-empty");
        let hi = *self.tops.iter().max().expect("non-empty");
        (lo, hi)
    }

    /// Chunk layers `cy` worth generating: from `underground` chunks below the lowest surface
    /// up to the highest surface or water surface.
    pub fn chunk_range(&self, underground: i32) -> std::ops::RangeInclusive<i32> {
        let (lo, hi) = self.top_range();
        let bottom = lo.div_euclid(CHUNK_SIZE) - underground;
        let top = hi.max(self.water_range().1 - 1).div_euclid(CHUNK_SIZE);
        bottom..=top
    }

    /// Whether chunk `cy` (with its border) can have a visible face. A chunk that is all
    /// solid, all water or all air cannot: it needs no filling for meshing.
    pub fn may_have_faces(&self, cy: i32) -> bool {
        let (lo_top, hi_top) = self.top_range();
        let (lo, hi) = (cy * CHUNK_SIZE - 1, cy * CHUNK_SIZE + CHUNK_SIZE);
        let (lo_water, hi_water) = self.water_range();
        let all_solid = hi <= lo_top;
        let all_fluid_or_air = lo > hi_top && (hi < lo_water || lo >= hi_water);
        !(all_solid || all_fluid_or_air)
    }

    /// Block at local column `(x, z)` (in `-1..=32`) and world height `y`.
    #[inline]
    pub fn block(&self, terrain: &Terrain, x: i32, y: i32, z: i32) -> BlockId {
        let top = self.top(x, z);
        if y > top {
            if y < self.water_top(x, z) { WATER } else { AIR }
        } else {
            terrain.column_block(top, self.slope(x, z), top - y)
        }
    }

    /// Chunk `cy` of this column with its one-block border.
    pub fn padded_chunk(&self, terrain: &Terrain, cy: i32) -> PaddedChunk {
        let mut blocks = Box::new([AIR; NP * NP * NP]);
        let oy = cy * CHUNK_SIZE;
        for lz in -1..=CHUNK_SIZE {
            for lx in -1..=CHUNK_SIZE {
                for ly in -1..=CHUNK_SIZE {
                    blocks[PaddedChunk::index(lx, ly, lz)] = self.block(terrain, lx, oy + ly, lz);
                }
            }
        }
        PaddedChunk { blocks }
    }
}

/// Generated content of a chunk: pure and deterministic.
pub fn generate_chunk(terrain: &Terrain, pos: ChunkPos) -> Chunk {
    ColumnSet::new(terrain, pos.cx, pos.cz)
        .padded_chunk(terrain, pos.cy)
        .inner()
}

/// `generate(pos) ⊕ delta(pos)`: the chunk as players see it.
pub fn chunk_at(terrain: &Terrain, deltas: &impl ChunkDeltas, pos: ChunkPos) -> Chunk {
    let mut c = generate_chunk(terrain, pos);
    deltas.apply(pos, &mut c);
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{GRASS, SAND, STONE};
    use crate::chunk::NoDeltas;
    use crate::terrain::tests::test_terrain;
    use nascio_core::coords::LocalPos;

    #[test]
    fn deterministic() {
        let t = test_terrain();
        let col = ColumnSet::new(&t, 17, 3_000);
        for cy in col.chunk_range(2) {
            let p = ChunkPos::new(17, cy, 3_000);
            assert!(generate_chunk(&t, p) == generate_chunk(&t, p));
        }
    }

    /// The border of a chunk equals the matching blocks of its neighbour, across both seams.
    #[test]
    fn borders_agree_across_seams() {
        let t = test_terrain();
        let last = CHUNKS_PER_AXIS - 1;
        for (a, b, axis_x) in [
            ((last, 40), (0, 40), true),
            ((40, last), (40, 0), false),
            ((100, 100), (101, 100), true),
        ] {
            let ca = ColumnSet::new(&t, a.0, a.1);
            let cb = ColumnSet::new(&t, b.0, b.1);
            for cy in ca.chunk_range(1) {
                let pa = ca.padded_chunk(&t, cy);
                let pb = cb.padded_chunk(&t, cy);
                for u in 0..CHUNK_SIZE {
                    for y in 0..CHUNK_SIZE {
                        let (in_b, border_a) = if axis_x {
                            (pb.get(0, y, u), pa.get(CHUNK_SIZE, y, u))
                        } else {
                            (pb.get(u, y, 0), pa.get(u, y, CHUNK_SIZE))
                        };
                        assert_eq!(in_b, border_a, "{a:?}->{b:?} cy {cy} u {u} y {y}");
                    }
                }
            }
        }
    }

    #[test]
    fn trivial_chunks_have_no_faces() {
        let t = test_terrain();
        for cx in (0..CHUNKS_PER_AXIS).step_by(131) {
            let col = ColumnSet::new(&t, cx, 2_000);
            for cy in col.chunk_range(3) {
                if !col.may_have_faces(cy) {
                    assert!(crate::mesh::mesh_chunk(&col.padded_chunk(&t, cy)).is_empty());
                }
            }
        }
    }

    #[test]
    fn simple_column() {
        let t = test_terrain();
        let mut seen = [false; 6];
        for cx in (0..CHUNKS_PER_AXIS).step_by(97) {
            let col = ColumnSet::new(&t, cx, 1_000);
            for cy in col.chunk_range(1) {
                let c = generate_chunk(&t, ChunkPos::new(cx, cy, 1_000));
                for y in 0..32u8 {
                    let p = LocalPos { x: 5, y, z: 9 };
                    let wy = cy * CHUNK_SIZE + i32::from(y);
                    let top = col.top(5, 9);
                    let b = c.get(p);
                    seen[usize::from(b)] = true;
                    if wy > top {
                        assert_eq!(b, if wy < col.water_top(5, 9) { WATER } else { AIR });
                    } else if wy < top - 10 {
                        assert_eq!(b, STONE);
                    }
                }
            }
        }
        assert!(seen[usize::from(GRASS)] && seen[usize::from(SAND)] && seen[usize::from(WATER)]);
        let p = ChunkPos::new(5, 0, 5);
        assert!(chunk_at(&t, &NoDeltas, p) == generate_chunk(&t, p));
    }
}
