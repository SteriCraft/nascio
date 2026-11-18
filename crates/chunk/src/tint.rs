//! Grass tint and water depth of a chunk column, computed by the generator and sent with the
//! column (meshing reads them).

use crate::block::BlockId;

/// Grass tint of the four corners of a chunk column, for meshing: bilinear inside the chunk,
/// so neighbouring chunks share their edge colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChunkTint {
    /// The tinted block (grass), `None`: no tint.
    pub grass: Option<BlockId>,
    /// Linear RGB at local `(0, 0)`, `(32, 0)`, `(0, 32)`, `(32, 32)`.
    pub corners: [[f32; 3]; 4],
    /// Water depth (blocks) of each local column, `x + 32 z`; 255: deep or unknown.
    pub water_depth: [u8; 1024],
}

impl ChunkTint {
    pub const NONE: Self = Self {
        grass: None,
        corners: [[1.0; 3]; 4],
        water_depth: [u8::MAX; 1024],
    };

    /// Colour at local horizontal position `(x, z)` in `[0, 32]`.
    pub fn at(&self, x: f32, z: f32) -> [f32; 3] {
        let (u, v) = (x / 32.0, z / 32.0);
        let c = &self.corners;
        std::array::from_fn(|k| {
            let a = c[0][k] + (c[1][k] - c[0][k]) * u;
            let b = c[2][k] + (c[3][k] - c[2][k]) * u;
            a + (b - a) * v
        })
    }
}
