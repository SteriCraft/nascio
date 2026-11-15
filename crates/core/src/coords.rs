//! Block, chunk and continuous world coordinates.
//!
//! Horizontal components are always kept wrapped into `[0, WORLD_SIZE)` (blocks) or
//! `[0, CHUNKS_PER_AXIS)` (chunks). `y` is vertical, unbounded and non-periodic.

use serde::{Deserialize, Serialize};

use crate::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS};
use crate::torus::{delta, delta_f64, wrap, wrap_f64};

/// Integer position of a block. `x` and `z` are wrapped on construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    /// Builds a block position, wrapping `x` and `z`.
    #[inline]
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self {
            x: wrap(x),
            y,
            z: wrap(z),
        }
    }

    /// Position moved by `(dx, dy, dz)`, wrapped.
    #[inline]
    pub fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        // i64 avoids overflow; the wrapped result always fits in i32.
        let x = (i64::from(self.x) + i64::from(dx)).rem_euclid(i64::from(crate::WORLD_SIZE));
        let z = (i64::from(self.z) + i64::from(dz)).rem_euclid(i64::from(crate::WORLD_SIZE));
        Self {
            x: x as i32,
            y: self.y + dy,
            z: z as i32,
        }
    }

    /// Shortest offset from `self` to `other` (periodic in x and z).
    #[inline]
    pub fn delta_to(self, other: BlockPos) -> (i32, i32, i32) {
        (
            delta(self.x, other.x),
            other.y - self.y,
            delta(self.z, other.z),
        )
    }

    /// Chunk containing this block.
    #[inline]
    pub fn chunk(self) -> ChunkPos {
        ChunkPos::new(
            self.x.div_euclid(CHUNK_SIZE),
            self.y.div_euclid(CHUNK_SIZE),
            self.z.div_euclid(CHUNK_SIZE),
        )
    }

    /// Position of this block inside its chunk.
    #[inline]
    pub fn local(self) -> LocalPos {
        LocalPos {
            x: self.x.rem_euclid(CHUNK_SIZE) as u8,
            y: self.y.rem_euclid(CHUNK_SIZE) as u8,
            z: self.z.rem_euclid(CHUNK_SIZE) as u8,
        }
    }

    /// Rebuilds a block position from its chunk and local position.
    #[inline]
    pub fn from_chunk_local(chunk: ChunkPos, local: LocalPos) -> Self {
        let o = chunk.origin();
        Self::new(
            o.x + i32::from(local.x),
            o.y + i32::from(local.y),
            o.z + i32::from(local.z),
        )
    }
}

/// Position of a cubic chunk of [`CHUNK_SIZE`]³ blocks. `cx` and `cz` are wrapped on construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChunkPos {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
}

impl ChunkPos {
    /// Builds a chunk position, wrapping `cx` and `cz` into `[0, CHUNKS_PER_AXIS)`.
    #[inline]
    pub fn new(cx: i32, cy: i32, cz: i32) -> Self {
        Self {
            cx: cx.rem_euclid(CHUNKS_PER_AXIS),
            cy,
            cz: cz.rem_euclid(CHUNKS_PER_AXIS),
        }
    }

    /// Chunk moved by `(dx, dy, dz)` chunks, wrapped.
    #[inline]
    pub fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self::new(self.cx + dx, self.cy + dy, self.cz + dz)
    }

    /// Shortest offset in chunks from `self` to `other` (periodic in x and z).
    #[inline]
    pub fn delta_to(self, other: ChunkPos) -> (i32, i32, i32) {
        let s = CHUNKS_PER_AXIS;
        let d = |a: i32, b: i32| {
            let d = (b - a).rem_euclid(s);
            if d > s / 2 { d - s } else { d }
        };
        (
            d(self.cx, other.cx),
            other.cy - self.cy,
            d(self.cz, other.cz),
        )
    }

    /// Block at the minimum corner of the chunk.
    #[inline]
    pub fn origin(self) -> BlockPos {
        BlockPos::new(
            self.cx * CHUNK_SIZE,
            self.cy * CHUNK_SIZE,
            self.cz * CHUNK_SIZE,
        )
    }
}

/// Position of a block inside a chunk, each component in `0..CHUNK_SIZE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LocalPos {
    pub x: u8,
    pub y: u8,
    pub z: u8,
}

impl LocalPos {
    /// Linear index in a chunk array, `x` fastest then `z` then `y`.
    #[inline]
    pub fn index(self) -> usize {
        let n = CHUNK_SIZE as usize;
        usize::from(self.x) + n * (usize::from(self.z) + n * usize::from(self.y))
    }
}

/// Continuous world position in metres. `x` and `z` are wrapped on construction.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldPos {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl WorldPos {
    /// Builds a world position, wrapping `x` and `z`.
    #[inline]
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self {
            x: wrap_f64(x),
            y,
            z: wrap_f64(z),
        }
    }

    /// World position of a block's minimum corner plus a local offset.
    #[inline]
    pub fn from_block(block: BlockPos, offset: [f32; 3]) -> Self {
        Self::new(
            f64::from(block.x) + f64::from(offset[0]),
            f64::from(block.y) + f64::from(offset[1]),
            f64::from(block.z) + f64::from(offset[2]),
        )
    }

    /// Block containing this position.
    #[inline]
    pub fn block(self) -> BlockPos {
        BlockPos::new(
            self.x.floor() as i32,
            self.y.floor() as i32,
            self.z.floor() as i32,
        )
    }

    /// Position moved by a vector, wrapped.
    #[inline]
    pub fn translate(self, d: [f64; 3]) -> Self {
        Self::new(self.x + d[0], self.y + d[1], self.z + d[2])
    }

    /// Shortest offset from `origin` to `self`, as `f32` for camera-relative rendering.
    #[inline]
    pub fn relative_to(self, origin: WorldPos) -> [f32; 3] {
        [
            delta_f64(origin.x, self.x) as f32,
            (self.y - origin.y) as f32,
            delta_f64(origin.z, self.z) as f32,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::WORLD_SIZE;
    use proptest::prelude::*;

    #[test]
    fn negative_y_chunking() {
        let b = BlockPos::new(5, -1, 7);
        assert_eq!(b.chunk(), ChunkPos::new(0, -1, 0));
        assert_eq!(b.local(), LocalPos { x: 5, y: 31, z: 7 });
        let b = BlockPos::new(0, -32, 0);
        assert_eq!(b.chunk().cy, -1);
        assert_eq!(b.local().y, 0);
    }

    #[test]
    fn chunk_wraps_at_3125() {
        assert_eq!(CHUNKS_PER_AXIS, 3125);
        assert_eq!(ChunkPos::new(3125, 0, -1), ChunkPos::new(0, 0, 3124));
        assert_eq!(BlockPos::new(99_999, 0, 0).chunk().cx, 3124);
        assert_eq!(BlockPos::new(100_000, 0, 0).chunk().cx, 0);
        assert_eq!(ChunkPos::new(3124, 0, 0).offset(1, 0, 0).cx, 0);
        assert_eq!(
            ChunkPos::new(3124, 0, 0).delta_to(ChunkPos::new(0, 0, 3124)),
            (1, 0, -1)
        );
    }

    #[test]
    fn block_offset_across_seam() {
        let b = BlockPos::new(99_998, 10, 1);
        assert_eq!(b.offset(3, -20, -2), BlockPos::new(1, -10, 99_999));
        assert_eq!(b.delta_to(BlockPos::new(1, 10, 99_999)), (3, 0, -2));
    }

    #[test]
    fn local_index_is_dense() {
        let mut seen = vec![false; 32 * 32 * 32];
        for y in 0..32 {
            for z in 0..32 {
                for x in 0..32 {
                    let i = LocalPos { x, y, z }.index();
                    assert!(!seen[i]);
                    seen[i] = true;
                }
            }
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn camera_relative_across_seam_is_small() {
        let camera = WorldPos::new(99_999.5, 64.0, 0.25);
        let p = WorldPos::new(0.5, 70.0, 99_999.25);
        assert_eq!(p.relative_to(camera), [1.0, 6.0, -1.0]);
        let q = WorldPos::from_block(BlockPos::new(-1, 0, -1), [0.5, 0.0, 0.5]);
        assert_eq!(q, WorldPos::new(99_999.5, 0.0, 99_999.5));
    }

    proptest! {
        #[test]
        fn block_chunk_local_roundtrip(x in any::<i32>(), y in -1_000_000i32..1_000_000, z in any::<i32>()) {
            let b = BlockPos::new(x, y, z);
            prop_assert!((0..WORLD_SIZE).contains(&b.x) && (0..WORLD_SIZE).contains(&b.z));
            let c = b.chunk();
            prop_assert!((0..CHUNKS_PER_AXIS).contains(&c.cx) && (0..CHUNKS_PER_AXIS).contains(&c.cz));
            prop_assert_eq!(BlockPos::from_chunk_local(c, b.local()), b);
        }

        #[test]
        fn relative_position_bounded(
            ax in -1e6f64..1e6, az in -1e6f64..1e6, bx in -1e6f64..1e6, bz in -1e6f64..1e6
        ) {
            let r = WorldPos::new(bx, 0.0, bz).relative_to(WorldPos::new(ax, 0.0, az));
            prop_assert!(r[0].abs() <= 50_000.0 && r[2].abs() <= 50_000.0);
        }

        #[test]
        fn block_offset_matches_new(
            x in 0i32..WORLD_SIZE, z in 0i32..WORLD_SIZE, dx in -300_000i32..300_000, dz in -300_000i32..300_000
        ) {
            prop_assert_eq!(BlockPos::new(x, 0, z).offset(dx, 0, dz), BlockPos::new(x + dx, 0, z + dz));
        }
    }
}
