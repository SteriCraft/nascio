//! Horizon tiles (spec §7): the far relief, as the server sends it and the client meshes it.
//!
//! Level `L` of the horizon samples the surface on the lattice of spacing
//! `BASE_SPACING_M · 2^L`, vertex `(I, J)` lying at `x = I · spacing`, `z = J · spacing`
//! (unwrapped: the data is taken at the wrapped position). A tile holds `TILE × TILE` vertices,
//! from `(i · TILE, j · TILE)`. Every vertex carries the drawn surface only: height, water
//! level, its nature (ground, water, ice) and the ground's colour. No geology, no block id: the
//! relief seen from far and nothing else.
//!
//! Tiles ignore the players' edits: the horizon shows the generated world (known limit).

use nascio_core::fmath::DetMath;
use serde::{Deserialize, Serialize};

/// Vertices per side of a tile.
pub const TILE: usize = 32;
/// Vertex spacing of level 0, m.
pub const BASE_SPACING_M: f64 = 16.0;
/// Levels a tile may belong to (the client uses 5).
pub const MAX_LEVELS: u8 = 8;
/// Heights are kept in steps of 1/`HEIGHT_STEPS` m.
pub const HEIGHT_STEPS: f64 = 16.0;

/// Vertex spacing of a level, m.
pub fn spacing(level: u8) -> f64 {
    BASE_SPACING_M * f64::from(1u32 << level)
}

/// A tile: its level and its index along x and z (unwrapped, may be negative).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TileKey {
    pub level: u8,
    pub i: i32,
    pub j: i32,
}

impl TileKey {
    /// The tile holding lattice vertex `(vi, vj)` of `level`, and the vertex's index in it.
    pub fn containing(level: u8, vi: i64, vj: i64) -> (TileKey, usize) {
        let t = TILE as i64;
        let key = TileKey {
            level,
            i: vi.div_euclid(t) as i32,
            j: vj.div_euclid(t) as i32,
        };
        let index = vi.rem_euclid(t) as usize + TILE * vj.rem_euclid(t) as usize;
        (key, index)
    }

    /// Lattice index of the tile's first vertex, along x and z.
    pub fn first_vertex(&self) -> (i64, i64) {
        (
            i64::from(self.i) * TILE as i64,
            i64::from(self.j) * TILE as i64,
        )
    }
}

/// What the horizon draws at a vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Surface {
    Ground = 0,
    /// Open water, coloured by its depth.
    Water = 1,
    /// Frozen water.
    Ice = 2,
}

impl Surface {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Ground),
            1 => Some(Self::Water),
            2 => Some(Self::Ice),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizonVertex {
    /// Top of the ground, m, a multiple of 1/[`HEIGHT_STEPS`].
    pub height: f32,
    /// Surface of the sea or of a lake, whole metres.
    pub water: f32,
    pub surface: Surface,
    /// Colour of the ground, sRGB (zero on water and ice: the client colours them).
    pub color: [u8; 3],
}

impl HorizonVertex {
    /// Drawn surface: the ground, or the water above it.
    pub fn top(&self) -> f32 {
        self.height.max(self.water)
    }
}

/// A tile's vertices, row-major in z (`TILE²`).
#[derive(Clone, Debug, PartialEq)]
pub struct HorizonTile {
    pub key: TileKey,
    pub vertices: Vec<HorizonVertex>,
}

/// A height rounded to the steps kept in a tile.
pub fn quantize_height(h: f64) -> f32 {
    ((h * HEIGHT_STEPS).round() / HEIGHT_STEPS) as f32
}

/// Linear colour of distant water `depth` m deep over a bottom of linear colour `bed`: the
/// chunks' translucent water ([`crate::mesh::water_alpha`], [`crate::mesh::water_shade`]) laid
/// over its bottom, so that the horizon meets the chunks without a step.
pub fn water_color(water: [f32; 3], bed: [f32; 3], depth: f64) -> [f32; 3] {
    let alpha = crate::mesh::water_alpha(depth.round().clamp(1.0, 255.0) as u8);
    let k = crate::mesh::water_shade(alpha);
    [0, 1, 2].map(|i| water[i] * k * alpha + bed[i] * (1.0 - alpha))
}

/// Linear colour component to an sRGB byte.
pub fn linear_to_srgb8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let v = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.det_powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containing_is_floor_division() {
        let (k, n) = TileKey::containing(2, -1, 33);
        assert_eq!((k.i, k.j), (-1, 1));
        assert_eq!(n, TILE - 1 + TILE);
        let (fx, fz) = k.first_vertex();
        assert_eq!((fx, fz), (-(TILE as i64), TILE as i64));
    }

    #[test]
    fn heights_keep_sixteenths() {
        assert_eq!(quantize_height(12.53), 12.5);
        assert_eq!(quantize_height(-0.03), 0.0);
        assert_eq!(f64::from(quantize_height(100.0625)), 100.0625);
    }
}
