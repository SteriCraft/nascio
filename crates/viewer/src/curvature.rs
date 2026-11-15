//! Display curvature (render only): the flat torus is drawn as if it were a sphere of radius
//! `R_vis`, by lowering every vertex by `d² / (2 R_vis)`, with `d` the horizontal distance to the
//! camera. Physics and generation stay flat.
//!
//! - Chunks, water and horizon share one material extension whose vertex shader applies the
//!   drop ([`Curvature`], `curvature.wesl`).
//! - Frustum culling still uses the flat bounds: chunk bounds are extended downwards by the
//!   largest drop they can get ([`CHUNK_AABB_DROP`]), horizon levels are never culled.
//! - A point at height `H` is visible from height `h` up to `√(2Rh) + √(2RH)` (tangent to the
//!   parabola): the horizon only draws the levels within that range and the fog ends there.

use std::f64::consts::TAU;

use bevy::asset::embedded_asset;
use bevy::camera::primitives::{Aabb, MeshAabb};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use nascio_core::consts::WORLD_SIZE_F64;
use serde::Deserialize;

use crate::fly::FlySettings;

/// Radius of the sphere with the same circumference as the torus, m (≈ 15 915 m).
pub const REAL_RADIUS_M: f64 = WORLD_SIZE_F64 / TAU;
/// Range of the radius slider, m.
pub const RADIUS_MIN_M: f64 = 5_000.0;
pub const RADIUS_MAX_M: f64 = 500_000.0;
/// Largest horizontal distance between the camera and a vertex of a shown chunk, m (maximum
/// view distance, plus a chunk diagonal and the camera's offset within its column).
const CHUNK_REACH_M: f64 = 1_200.0;
/// Extra depth given to chunk bounds so lowered chunks are not culled, m.
pub const CHUNK_AABB_DROP: f32 = drop_at(RADIUS_MIN_M, CHUNK_REACH_M) as f32;

const SHADER: &str = "embedded://nascio_viewer/curvature.wesl";

/// Rendering parameters of the viewer (`--render-config`, `[render]` section).
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RenderConfig {
    /// Display radius `R_vis`, m; 0 draws the world flat.
    pub curvature_radius_m: f64,
}

impl Default for RenderConfig {
    fn default() -> Self {
        // Provisional: to be fixed after the author's trial (docs/decisions.md).
        Self {
            curvature_radius_m: REAL_RADIUS_M,
        }
    }
}

impl RenderConfig {
    pub fn from_toml(s: &str) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            render: RenderConfig,
        }
        Ok(toml::from_str::<File>(s)?.render)
    }

    /// The radius as a setting: `None` when flat, clamped to the slider range otherwise.
    pub fn radius(&self) -> Option<f64> {
        (self.curvature_radius_m > 0.0)
            .then(|| self.curvature_radius_m.clamp(RADIUS_MIN_M, RADIUS_MAX_M))
    }
}

/// Drop of a point at horizontal distance `d` from the camera, m.
pub const fn drop_at(radius: f64, d: f64) -> f64 {
    d * d / (2.0 * radius)
}

/// Farthest horizontal distance at which a point at height `target` can be seen from height
/// `eye` over a sea at 0 m (both clamped to the sea level).
pub fn visible_range(radius: f64, eye: f64, target: f64) -> f64 {
    (2.0 * radius * eye.max(0.0)).sqrt() + (2.0 * radius * target.max(0.0)).sqrt()
}

/// The vertex shader's coefficient `k = 1 / (2 R)`.
fn coefficient(radius: Option<f64>) -> f32 {
    radius.map_or(0.0, |r| (0.5 / r) as f32)
}

pub type CurvedMaterial = ExtendedMaterial<StandardMaterial, Curvature>;

/// Material extension lowering the vertices.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct Curvature {
    /// x: `1 / (2 R)` (0 when flat).
    #[uniform(100)]
    pub params: Vec4,
}

impl MaterialExtension for Curvature {
    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    // The prepass and shadow passes would use the flat default vertex shader: off (neither is
    // used by the viewer today).
    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }
}

/// A curved material for `base`, with the current radius.
pub fn material(base: StandardMaterial, radius: Option<f64>) -> CurvedMaterial {
    CurvedMaterial {
        base,
        extension: Curvature {
            params: Vec4::new(coefficient(radius), 0.0, 0.0, 0.0),
        },
    }
}

/// Bounds of a chunk mesh, extended downwards by the largest drop.
pub fn chunk_aabb(mesh: &Mesh) -> Option<Aabb> {
    let aabb = mesh.get_aabb()?;
    let (min, max) = (Vec3::from(aabb.min()), Vec3::from(aabb.max()));
    Some(Aabb::from_min_max(min - Vec3::Y * CHUNK_AABB_DROP, max))
}

pub struct CurvaturePlugin;

impl Plugin for CurvaturePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "curvature.wesl");
        app.add_plugins(MaterialPlugin::<CurvedMaterial>::default())
            .add_systems(Update, sync_curvature);
    }
}

/// Pushes the radius chosen in the UI to every curved material.
fn sync_curvature(
    settings: Res<FlySettings>,
    mut last: Local<Option<f32>>,
    mut materials: ResMut<Assets<CurvedMaterial>>,
) {
    let k = coefficient(settings.curvature_radius_m);
    if *last == Some(k) {
        return;
    }
    *last = Some(k);
    for (_, m) in materials.iter_mut() {
        if m.extension.params.x != k {
            m.extension.params.x = k;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_radius_matches_circumference() {
        assert!((REAL_RADIUS_M - 15_915.494).abs() < 1e-2);
    }

    #[test]
    fn range_is_tangent_to_parabola() {
        // From height h, the sea is seen up to √(2Rh), where the drop equals h.
        let (r, h) = (REAL_RADIUS_M, 100.0);
        let d = visible_range(r, h, 0.0);
        assert!((drop_at(r, d) - h).abs() < 1e-9);
        // Below the sea level, nothing extra is seen.
        assert_eq!(visible_range(r, -5.0, -10.0), 0.0);
    }

    #[test]
    fn chunk_margin_covers_minimum_radius() {
        let need = drop_at(
            RADIUS_MIN_M,
            f64::from(crate::fly::VIEW_MAX) + 32.0 * 2f64.sqrt() + 32.0,
        );
        assert!(f64::from(CHUNK_AABB_DROP) >= need);
    }

    #[test]
    fn config_parses_and_flat_is_none() {
        let c = RenderConfig::from_toml("[render]\ncurvature_radius_m = 0.0\n").unwrap();
        assert_eq!(c.radius(), None);
        let c = RenderConfig::from_toml("").unwrap();
        assert_eq!(c.radius(), Some(REAL_RADIUS_M));
    }
}
