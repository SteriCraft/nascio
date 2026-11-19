//! Display curvature (render only): the flat torus is drawn as if it were a sphere of radius
//! `R_vis`, by lowering every vertex by `d² / (2 R_vis)`, with `d` the horizontal distance to the
//! camera. Physics and generation stay flat.
//!
//! - Chunks, water and horizon share one material extension whose vertex shader applies the
//!   drop ([`Curvature`], `curvature.wesl`), measured from the camera position given in the
//!   uniform; the prepass/shadow vertex shader (`curvature_prepass.wesl`) applies the same drop,
//!   so shadows fall on the displayed geometry.
//! - The same extension lights every pixel with its own sun (M4): see `curvature.wesl`.
//! - Frustum culling still uses the flat bounds: chunk bounds are extended downwards by the
//!   largest drop they can get ([`CHUNK_AABB_DROP`]), horizon levels are never culled.
//! - A point at height `H` is visible from height `h` up to `√(2Rh) + √(2RH)` (tangent to the
//!   parabola): the horizon only draws the levels within that range and the fog ends there.

use bevy::asset::RenderAssetUsages;
use bevy::asset::embedded_asset;
use bevy::camera::primitives::{Aabb, MeshAabb};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;
use serde::Deserialize;

use crate::fly::FlySettings;

/// Range of the radius accepted from `--curvature-radius` and `--render-config`, m.
pub const RADIUS_MIN_M: f64 = 5_000.0;
pub const RADIUS_MAX_M: f64 = 500_000.0;
/// Largest horizontal distance between the camera and a vertex of a shown chunk, m (maximum
/// view distance, plus a chunk diagonal and the camera's offset within its column).
const CHUNK_REACH_M: f64 = 1_200.0;
/// Extra depth given to chunk bounds so lowered chunks are not culled, m.
pub const CHUNK_AABB_DROP: f32 = drop_at(RADIUS_MIN_M, CHUNK_REACH_M) as f32;

const SHADER: &str = "embedded://nascio_viewer/curvature.wesl";
const PREPASS_SHADER: &str = "embedded://nascio_viewer/curvature_prepass.wesl";
/// Snow cover gained per °C of cooling with height above the climate cell's mean (5 °C, about
/// 770 m, from bare ground to full cover).
const SNOW_PER_DEGREE: f32 = 0.2;
/// Texture alpha below which foliage is cut out (alpha-mask material; `curvature.wesl` and
/// `curvature_prepass.wesl` have the same constant). The texture mips keep the share of texels
/// above it (`crate::textures`).
pub const FOLIAGE_CUTOFF: f32 = 0.5;
/// Default display radius `R_vis`, m (decided after the author's trial, M4).
pub const DEFAULT_RADIUS_M: f64 = 200_000.0;

/// Rendering parameters of the viewer (`--render-config`, `[render]` section).
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RenderConfig {
    /// Display radius `R_vis`, m; 0 draws the world flat.
    pub curvature_radius_m: f64,
    /// Columns within this many chunks of the camera column draw cut-out foliage; farther
    /// ones draw it opaque (the cut-out inner faces about double a forest's quads). Negative:
    /// no cut-out foliage. Overrides the client's "Distance de transparence" for the session.
    pub foliage_cutout_chunks: i32,
}

/// Default of [`RenderConfig::foliage_cutout_chunks`] (author's choice).
pub const DEFAULT_FOLIAGE_CUTOUT_CHUNKS: i32 = 4;

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            curvature_radius_m: DEFAULT_RADIUS_M,
            foliage_cutout_chunks: DEFAULT_FOLIAGE_CUTOUT_CHUNKS,
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

/// Per-frame data of the curved materials (layout of `Curved` in the shaders). It lives in a
/// shared storage buffer: rewriting it every frame leaves the materials untouched (changing a
/// material re-prepares every chunk that uses it, which cost 8 ms per frame).
#[derive(Clone, Copy, Debug, Default)]
pub struct CurvedUniform {
    /// x: `1 / (2 R)` (0 when flat); y, z, w: camera position in render space.
    pub drop: Vec4,
    /// x, y: camera world x and z (m, wrapped); z: hour angle at the camera; w: δ(N).
    pub sun: Vec4,
    /// `s_fade, s_dark, s_light, s_clear`, m.
    pub seam: Vec4,
    /// x: shadows on; y: direct light scale; z: sky light scale; w: night floor.
    pub light: Vec4,
    /// x: camera world height (m); y: lapse rate (°C/m); z: snow cover per °C; w: unused.
    pub snow: Vec4,
    /// xyz: towards the visible sun at the camera; w: e_vis at the camera (rad).
    pub air: Vec4,
    /// x: cloud cover at the camera; y, z: horizontal distances where the relief starts and
    /// ends fading into the sky (m); w: unused.
    pub haze: Vec4,
}

impl CurvedUniform {
    fn floats(&self) -> Vec<f32> {
        [
            self.drop, self.sun, self.seam, self.light, self.snow, self.air, self.haze,
        ]
        .iter()
        .flat_map(|v| v.to_array())
        .collect()
    }
}

/// The shared per-frame buffer of the curved materials.
#[derive(Resource)]
pub struct CurvedFrame(pub Handle<ShaderBuffer>);

impl CurvedFrame {
    pub fn new(buffers: &mut Assets<ShaderBuffer>) -> Self {
        let data = CurvedUniform::default().floats();
        Self(buffers.add(ShaderBuffer::new(data, RenderAssetUsages::default())))
    }
}

/// Material extension: display curvature and per-pixel sun.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct Curvature {
    /// [`CurvedUniform`], shared by every curved material.
    #[storage(100, read_only)]
    pub frame: Handle<ShaderBuffer>,
    /// Weather clouds (red), snow cover (green), temperature (blue) and cell height (alpha)
    /// over the whole world (see [`crate::sky::ClimateMaps`]).
    #[texture(101)]
    #[sampler(102)]
    pub climate: Handle<Image>,
    /// Colour of the world's surface (see [`crate::sky::GroundAlbedo`]).
    #[texture(103)]
    #[sampler(104)]
    pub ground: Handle<Image>,
    /// Block textures, one layer per image (see [`crate::textures`]), sampled at the chunk
    /// vertices' UV_1 when their material channel names a layer.
    #[texture(105, dimension = "2d_array")]
    #[sampler(106)]
    pub blocks: Handle<Image>,
}

impl MaterialExtension for Curvature {
    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn prepass_vertex_shader() -> ShaderRef {
        PREPASS_SHADER.into()
    }

    /// Cuts the foliage out in the shadow pass (run only for the alpha-mask material, or when
    /// depth clipping is emulated).
    fn prepass_fragment_shader() -> ShaderRef {
        PREPASS_SHADER.into()
    }

    fn enable_prepass() -> bool {
        true
    }

    fn enable_shadows() -> bool {
        true
    }
}

/// A curved material for `base`, reading `frame` and sampling `climate`, `ground` and the
/// block texture array `blocks`.
pub fn material(
    base: StandardMaterial,
    frame: Handle<ShaderBuffer>,
    climate: Handle<Image>,
    ground: Handle<Image>,
    blocks: Handle<Image>,
) -> CurvedMaterial {
    CurvedMaterial {
        base,
        extension: Curvature {
            frame,
            climate,
            ground,
            blocks,
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
        // Shared modules imported by curvature.wesl and sky.wesl (audit R2).
        bevy::shader::load_shader_library!(app, "atmosphere.wesl");
        bevy::shader::load_shader_library!(app, "sun.wesl");
        embedded_asset!(app, "curvature.wesl");
        embedded_asset!(app, "curvature_prepass.wesl");
        app.add_plugins(MaterialPlugin::<CurvedMaterial>::default());
    }
}

/// Horizontal distance at which the relief has faded into the sky: the end of the horizon (or
/// of the chunks without it), m.
pub fn relief_end(settings: &FlySettings, horizon_extent_m: f64) -> f32 {
    if settings.horizon && horizon_extent_m > 0.0 {
        horizon_extent_m as f32 * 0.95
    } else {
        settings.view_distance_m
    }
}

/// "Ombres alpha": the foliage material is an alpha mask (its shadow pass runs
/// `curvature_prepass.wesl`, which cuts the shadows out) or opaque (solid shadows, no fragment
/// shader in the shadow pass). The main pass cuts the foliage out in both cases
/// (`curvature.wesl`). Changing the material re-prepares the chunks once.
pub fn sync_foliage_shadows(
    settings: Res<FlySettings>,
    terrain: Option<Res<crate::fly::TerrainMaterials>>,
    mut materials: ResMut<Assets<CurvedMaterial>>,
) {
    let Some(terrain) = terrain else {
        return;
    };
    if !settings.is_changed() && !terrain.is_added() {
        return;
    }
    let mode = if settings.alpha_shadows {
        AlphaMode::Mask(FOLIAGE_CUTOFF)
    } else {
        AlphaMode::Opaque
    };
    if materials
        .get(&terrain.foliage)
        .is_some_and(|m| m.base.alpha_mode != mode)
        && let Some(mut m) = materials.get_mut(&terrain.foliage)
    {
        m.base.alpha_mode = mode;
    }
}

/// Pushes the radius, the camera and the sky to the curved materials' buffer, once per frame.
#[allow(clippy::too_many_arguments)]
pub fn sync_curved(
    settings: Res<FlySettings>,
    cam: Res<crate::fly::FlyCam>,
    camera: Query<&Transform, With<crate::fly::FlyCamera>>,
    sky: Res<crate::sky::SkyState>,
    info: Res<crate::fly::WorldInfo>,
    horizon: Res<crate::horizon::HorizonStats>,
    frame: Res<CurvedFrame>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
) {
    let Ok(t) = camera.single() else {
        return;
    };
    let a = info.astro;
    let p = sky.sun();
    let d = p.direction();
    let end = relief_end(&settings, horizon.extent_m);
    let u = CurvedUniform {
        drop: Vec4::new(
            coefficient(settings.curvature_radius_m),
            t.translation.x,
            t.translation.y,
            t.translation.z,
        ),
        sun: Vec4::new(
            nascio_core::torus::wrap_f64(cam.pos.x) as f32,
            nascio_core::torus::wrap_f64(cam.pos.z) as f32,
            sky.hour_angle as f32,
            sky.declination as f32,
        ),
        seam: Vec4::new(
            a.s_fade as f32,
            a.s_dark as f32,
            a.s_light as f32,
            a.s_clear as f32,
        ),
        light: Vec4::new(
            if settings.shadows { 1.0 } else { 0.0 },
            1.0,
            0.45,
            // Dev viewer: a night floor keeps the relief readable.
            if settings.night_floor { 1.0 } else { 0.0 },
        ),
        snow: Vec4::new(
            cam.pos.y as f32,
            info.lapse_rate_c_per_m as f32,
            SNOW_PER_DEGREE,
            0.0,
        ),
        air: Vec4::new(
            d[0] as f32,
            d[1] as f32,
            d[2] as f32,
            p.visible_elevation as f32,
        ),
        haze: Vec4::new(
            sky.cloud as f32,
            end * 0.8,
            end,
            settings.fixed_clouds.map_or(-1.0, |c| c as f32),
        ),
    };
    if let Some(mut b) = buffers.get_mut(&frame.0) {
        *b = ShaderBuffer::new(u.floats(), RenderAssetUsages::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nascio_core::consts::WORLD_SIZE_F64;
    use std::f64::consts::TAU;

    /// Radius of the sphere with the same circumference as the torus, m (≈ 15 915 m).
    const REAL_RADIUS_M: f64 = WORLD_SIZE_F64 / TAU;

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
        assert_eq!(c.radius(), Some(DEFAULT_RADIUS_M));
    }

    /// The material kinds of the shader are the mesher's (audit F4).
    #[test]
    fn shader_material_kinds_match_the_mesher() {
        use nascio_voxel::mesh::{
            MATERIAL_FOLIAGE, MATERIAL_FOLIAGE_CUT, MATERIAL_GROUND, MATERIAL_WATER,
        };
        let src = include_str!("curvature.wesl");
        for (name, v) in [
            ("GROUND", MATERIAL_GROUND),
            ("WATER", MATERIAL_WATER),
            ("FOLIAGE", MATERIAL_FOLIAGE),
            ("FOLIAGE_CUT", MATERIAL_FOLIAGE_CUT),
        ] {
            let line = format!("const MATERIAL_{name}: f32 = {v:.1};");
            assert!(src.contains(&line), "missing {line}");
        }
        // No alpha tag left.
        assert!(!src.contains("0.99"));
        // The foliage cutout and kind are the same in both passes.
        let prepass = include_str!("curvature_prepass.wesl");
        for s in [src, prepass] {
            let cutoff = format!("const FOLIAGE_CUTOFF: f32 = {FOLIAGE_CUTOFF:.1};");
            assert!(s.contains(&cutoff), "missing {cutoff}");
            let kind = format!("const MATERIAL_FOLIAGE_CUT: f32 = {MATERIAL_FOLIAGE_CUT:.1};");
            assert!(s.contains(&kind), "missing {kind}");
        }
    }
}
