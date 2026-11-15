//! Distant terrain beyond the chunks (filled in part 2 of M2).

use bevy::prelude::*;

/// Figures shown in the HUD.
#[derive(Resource, Default, Clone, Debug)]
pub struct HorizonStats {
    pub levels: usize,
    pub triangles: usize,
    pub extent_m: f64,
    /// Levels waiting for their mesh.
    pub pending: usize,
}
