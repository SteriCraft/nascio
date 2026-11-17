//! Pure, deterministic chunk generation (spec §5).
//!
//! Generation is column-based: the heights of a chunk column (plus a border) are computed once
//! in a [`ColumnSet`], and any chunk of that column — with its one-block border — is filled from
//! it. The same function fills the border, so neighbouring chunks always agree, across the seam
//! included.

use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS};
use nascio_core::coords::ChunkPos;
use nascio_core::fmath::DetMath;

use crate::block::{AIR, BlockId};
use crate::chunk::{Chunk, ChunkDeltas, NP, PaddedChunk};
use crate::geology::{ColumnGeo, wrap};
use crate::terrain::{RockColumn, Terrain};
use crate::vegetation::{MAX_RADIUS, Part, PlantKind};
use nascio_worldgen::deposits::Body;

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
    /// First magma block (the Moho) of each column, local `-1..=32`.
    mohos: Vec<i32>,
    /// Geology of each column, local `-1..=32`.
    geos: Vec<ColumnGeo>,
    /// Ore bodies that may reach the column.
    bodies: Vec<Body>,
    /// Plant blocks `(y, part)` above the ground of each column, local `-1..=32`.
    plants: Vec<Vec<(i32, Part)>>,
    /// Highest plant block (`i32::MIN`: none).
    plant_top: i32,
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
        let mut mohos = vec![0; NP * NP];
        let mut geos = vec![ColumnGeo::default(); NP * NP];
        for lz in 0..NP {
            for lx in 0..NP {
                let h = |dx: usize, dz: usize| heights[(lx + dx) + NH * (lz + dz)];
                tops[lx + NP * lz] = h(1, 1).floor() as i32;
                let gx = (h(2, 1) - h(0, 1)) * 0.5;
                let gz = (h(1, 2) - h(1, 0)) * 0.5;
                slopes[lx + NP * lz] = gx.det_hypot(gz) as f32;
                let x = f64::from(ox + lx as i32 - 1) + 0.5;
                let z = f64::from(oz + lz as i32 - 1) + 0.5;
                water_tops[lx + NP * lz] = terrain.water_top(x, z);
                mohos[lx + NP * lz] = terrain.moho_y(x, z, tops[lx + NP * lz]);
                geos[lx + NP * lz] = terrain.column_geo(x, z);
            }
        }
        let bodies = terrain.geology().map_or_else(Vec::new, |g| {
            g.bodies_near(
                f64::from(ox - 2),
                f64::from(oz - 2),
                f64::from(CHUNK_SIZE + 4),
            )
        });
        // Plants of the column and its border, overhanging ones included. Where two overlap, the
        // higher part wins (cactus, then log, then leaves).
        let mut plants = vec![Vec::new(); NP * NP];
        let mut plant_top = i32::MIN;
        let shapes = PlantKind::ALL.map(PlantKind::parts);
        let r = MAX_RADIUS;
        let found = terrain.plants_in(
            ox - 1 - r,
            oz - 1 - r,
            ox + CHUNK_SIZE + r,
            oz + CHUNK_SIZE + r,
        );
        for p in found {
            let shape = &shapes[PlantKind::ALL
                .iter()
                .position(|k| *k == p.kind)
                .unwrap_or(0)];
            for &(dx, dy, dz, part) in shape {
                let (lx, lz) = (p.x + dx - ox, p.z + dz - oz);
                if !(-1..=CHUNK_SIZE).contains(&lx) || !(-1..=CHUNK_SIZE).contains(&lz) {
                    continue;
                }
                let k = Self::k(lx, lz);
                let y = p.base + dy;
                if y <= tops[k] {
                    continue;
                }
                let cell: &mut Vec<(i32, Part)> = &mut plants[k];
                match cell.iter_mut().find(|e| e.0 == y) {
                    Some(e) => e.1 = e.1.max(part),
                    None => cell.push((y, part)),
                }
                plant_top = plant_top.max(y);
            }
        }
        Self {
            plants,
            plant_top,
            cx,
            cz,
            tops,
            slopes,
            water_tops,
            mohos,
            geos,
            bodies,
        }
    }

    /// First magma block of local column `(x, z)`.
    #[inline]
    pub fn moho(&self, x: i32, z: i32) -> i32 {
        self.mohos[Self::k(x, z)]
    }

    /// Chunk layers of the whole crust: from one chunk of magma below the deepest Moho up to
    /// the highest surface or water surface (without Moho data: `underground` chunks).
    pub fn crust_chunk_range(&self, underground: i32) -> std::ops::RangeInclusive<i32> {
        let lowest = *self.mohos.iter().min().expect("non-empty");
        if lowest == i32::MIN {
            return self.chunk_range(underground);
        }
        (lowest.div_euclid(CHUNK_SIZE) - 1)..=*self.chunk_range(0).end()
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

    /// Water depth (blocks, at most 255) of the 32² local columns, `x + 32 z`, for meshing.
    pub fn water_depths(&self) -> [u8; 1024] {
        std::array::from_fn(|k| {
            let (x, z) = ((k % 32) as i32, (k / 32) as i32);
            (self.water_top(x, z) - 1 - self.top(x, z)).clamp(0, 255) as u8
        })
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
        // `hi + 1`: the snow cover sits above the top block.
        let top = (hi + 1)
            .max(self.water_range().1 - 1)
            .max(self.plant_top)
            .div_euclid(CHUNK_SIZE);
        bottom..=top
    }

    /// Whether chunk `cy` (with its border) can have a visible face. A chunk that is all
    /// solid, all water or all air cannot: it needs no filling for meshing.
    pub fn may_have_faces(&self, cy: i32) -> bool {
        let (lo_top, hi_top) = self.top_range();
        let (lo, hi) = (cy * CHUNK_SIZE - 1, cy * CHUNK_SIZE + CHUNK_SIZE);
        let (lo_water, hi_water) = self.water_range();
        let all_solid = hi <= lo_top;
        let all_fluid_or_air =
            lo > (hi_top + 1).max(self.plant_top) && (hi < lo_water || lo >= hi_water);
        !(all_solid || all_fluid_or_air)
    }

    /// Geology of local column `(x, z)`.
    #[inline]
    pub fn geo(&self, x: i32, z: i32) -> &ColumnGeo {
        &self.geos[Self::k(x, z)]
    }

    /// Ore bodies that may reach this column.
    pub fn bodies(&self) -> &[Body] {
        &self.bodies
    }

    /// Block at local column `(x, z)` (in `-1..=32`) and world height `y`.
    #[inline]
    pub fn block(&self, terrain: &Terrain, x: i32, y: i32, z: i32) -> BlockId {
        self.block_with(terrain, x, y, z, &mut None)
    }

    /// [`Self::block`], reusing `rock`, the rock data of column `(x, z)` (computed on first
    /// need; `None` when not yet computed).
    #[inline]
    fn block_with(
        &self,
        terrain: &Terrain,
        x: i32,
        y: i32,
        z: i32,
        rock: &mut Option<RockColumn>,
    ) -> BlockId {
        let top = self.top(x, z);
        let ids = &terrain.blocks().ids;
        if y > top {
            let water_top = self.water_top(x, z);
            if y < water_top {
                return terrain.water_block(self.geo(x, z), y, water_top);
            }
            if y <= self.plant_top
                && let Some(&(_, part)) = self.plants[Self::k(x, z)].iter().find(|e| e.0 == y)
                && let Some(b) = terrain.part_block(part)
            {
                return b;
            }
            if y == top + 1
                && let Some(l) = terrain.snow_layer(self.geo(x, z), top, self.slope(x, z))
            {
                return l;
            }
            return AIR;
        }
        let moho = self.moho(x, z);
        if y < moho {
            return ids.magma;
        }
        let (wx, wz) = (
            wrap(self.cx * CHUNK_SIZE + x),
            wrap(self.cz * CHUNK_SIZE + z),
        );
        let geo = self.geo(x, z);
        let rc =
            rock.get_or_insert_with(|| terrain.rock_column(geo, wx, wz, top, self.slope(x, z)));
        let (b, class) = terrain.rock_at(rc, geo, top, moho, top - y);
        if !self.bodies.is_empty()
            && let Some(g) = terrain.geology()
            && let Some(ore) = g.ore(&self.bodies, class, wx, y, wz, top)
        {
            return ore;
        }
        b
    }

    /// Chunk `cy` of this column with its one-block border.
    pub fn padded_chunk(&self, terrain: &Terrain, cy: i32) -> PaddedChunk {
        let mut blocks = Box::new([AIR; NP * NP * NP]);
        let oy = cy * CHUNK_SIZE;
        for lz in -1..=CHUNK_SIZE {
            for lx in -1..=CHUNK_SIZE {
                let mut rock = None;
                for ly in -1..=CHUNK_SIZE {
                    blocks[PaddedChunk::index(lx, ly, lz)] =
                        self.block_with(terrain, lx, oy + ly, lz, &mut rock);
                }
            }
        }
        PaddedChunk { blocks }
    }
}

/// Bounded cache of [`ColumnSet`]s (a column with its plants and ore bodies weighs some
/// 100 KB): at most `capacity` columns, the least recently used one dropped first.
#[derive(Debug)]
pub struct ColumnCache {
    capacity: usize,
    tick: u64,
    columns: std::collections::BTreeMap<(i32, i32), (u64, std::sync::Arc<ColumnSet>)>,
}

impl ColumnCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            tick: 0,
            columns: std::collections::BTreeMap::new(),
        }
    }

    /// Column `(cx, cz)` (wrapped), computed if missing.
    pub fn get(&mut self, terrain: &Terrain, cx: i32, cz: i32) -> std::sync::Arc<ColumnSet> {
        let key = (
            cx.rem_euclid(CHUNKS_PER_AXIS),
            cz.rem_euclid(CHUNKS_PER_AXIS),
        );
        self.tick += 1;
        if let Some(e) = self.columns.get_mut(&key) {
            e.0 = self.tick;
            return e.1.clone();
        }
        if self.columns.len() >= self.capacity
            && let Some(old) = self.columns.iter().min_by_key(|e| e.1.0).map(|e| *e.0)
        {
            self.columns.remove(&old);
        }
        let col = std::sync::Arc::new(ColumnSet::new(terrain, key.0, key.1));
        self.columns.insert(key, (self.tick, col.clone()));
        col
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
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
    use crate::chunk::NoDeltas;
    use crate::terrain::tests::test_terrain;
    use nascio_core::coords::LocalPos;

    #[test]
    fn column_cache_is_bounded_and_wraps() {
        let t = test_terrain();
        let mut c = ColumnCache::new(3);
        let a = c.get(&t, -1, 5);
        assert!(std::sync::Arc::ptr_eq(
            &a,
            &c.get(&t, CHUNKS_PER_AXIS - 1, 5)
        ));
        for k in 0..5 {
            c.get(&t, k, 0);
            assert!(c.len() <= 3);
        }
        // The oldest column was dropped; a recent one is still shared.
        let b = c.get(&t, 4, 0);
        assert!(std::sync::Arc::ptr_eq(&b, &c.get(&t, 4, 0)));
        assert!(!std::sync::Arc::ptr_eq(&a, &c.get(&t, -1, 5)));
    }

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
                    assert!(
                        crate::mesh::mesh_chunk(
                            &col.padded_chunk(&t, cy),
                            t.blocks(),
                            &crate::ChunkTint::NONE
                        )
                        .is_empty()
                    );
                }
            }
        }
    }

    /// Chunks go down to the Moho: rock above it, magma below, with no visible face in
    /// between (both opaque), and the same Moho on both sides of the seams.
    #[test]
    fn crust_down_to_the_moho_then_magma() {
        let t = test_terrain();
        let ids = t.blocks().ids;
        let last = CHUNKS_PER_AXIS - 1;
        for (cx, cz) in [(0, 0), (last, 900), (1_500, last), (2_000, 1_200)] {
            let col = ColumnSet::new(&t, cx, cz);
            let range = col.crust_chunk_range(2);
            let (mut stone, mut magma) = (false, false);
            for cy in range.clone() {
                let c = generate_chunk(&t, ChunkPos::new(cx, cy, cz));
                for y in 0..32u8 {
                    let wy = cy * CHUNK_SIZE + i32::from(y);
                    let b = c.get(LocalPos { x: 7, y, z: 7 });
                    if wy < col.moho(7, 7) {
                        assert_eq!(b, ids.magma, "({cx},{cz}) y {wy}");
                        magma = true;
                    } else if wy <= col.top(7, 7) - 10 {
                        assert!(
                            t.blocks().is_solid(b) && b != ids.magma,
                            "({cx},{cz}) y {wy}"
                        );
                        stone = true;
                    }
                }
                if !col.may_have_faces(cy) {
                    assert!(
                        crate::mesh::mesh_chunk(
                            &col.padded_chunk(&t, cy),
                            t.blocks(),
                            &crate::ChunkTint::NONE
                        )
                        .is_empty()
                    );
                }
            }
            assert!(stone && magma, "({cx},{cz}) range {range:?}");
            assert!(col.moho(7, 7) <= col.top(7, 7) - t.config().min_crust_m);
            assert!(col.moho(7, 7) < -600);
        }
        // Continuity of the Moho across the x seam.
        let a = ColumnSet::new(&t, last, 300);
        let b = ColumnSet::new(&t, 0, 300);
        assert_eq!(a.moho(CHUNK_SIZE, 5), b.moho(0, 5));
    }

    #[test]
    fn plants_stand_on_the_ground() {
        let t = test_terrain();
        let log = t.blocks().ids.spruce_log.expect("vegetation blocks");
        let mut n = 0;
        for cz in (0..CHUNKS_PER_AXIS).step_by(97) {
            for cx in (0..CHUNKS_PER_AXIS).step_by(89) {
                let (ox, oz) = (cx * CHUNK_SIZE, cz * CHUNK_SIZE);
                let col = ColumnSet::new(&t, cx, cz);
                let plants = t.plants_in(ox, oz, ox + 31, oz + 31);
                assert_eq!(plants, t.plants_in(ox, oz, ox + 31, oz + 31));
                for p in plants {
                    let (lx, lz) = (p.x - ox, p.z - oz);
                    assert_eq!(p.base, col.top(lx, lz) + 1);
                    assert!(col.water_top(lx, lz) <= p.base);
                    assert_eq!(col.block(&t, lx, p.base, lz), log);
                    assert_eq!(col.block(&t, lx, p.base + 6, lz), log);
                    n += 1;
                }
            }
        }
        assert!(n > 50, "{n} trees");
    }

    #[test]
    fn simple_column() {
        let t = test_terrain();
        let ids = t.blocks().ids;
        let mut seen = vec![false; t.blocks().len()];
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
                    let plant = [ids.spruce_log, ids.spruce_leaves, ids.cactus].contains(&Some(b));
                    if wy > top && !plant {
                        assert_eq!(
                            b,
                            if wy < col.water_top(5, 9) {
                                ids.water
                            } else {
                                AIR
                            }
                        );
                    } else if wy < top - 10 {
                        assert!(t.blocks().is_solid(b));
                    }
                }
            }
        }
        assert!(seen[usize::from(ids.grass)] && seen[usize::from(ids.sand)]);
        assert!(seen[usize::from(ids.water)]);
        let p = ChunkPos::new(5, 0, 5);
        assert!(chunk_at(&t, &NoDeltas, p) == generate_chunk(&t, p));
    }
}
