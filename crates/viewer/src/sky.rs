//! Sky and lighting of the 3D mode (spec §2.5 – §2.8, M4).
//!
//! - [`SkyState`]: the instant (real time, or a fixed day and solar hour), the sun at the
//!   camera, the cloud cover at the camera, the Moon and the sidereal angle; computed once per
//!   frame and shared by every consumer.
//! - [`ClimateMaps`]: weather cloud cover, snow cover and temperature evaluated from the climate
//!   harmonics at the current date into a world-wide texture (red: clouds, green: snow, blue:
//!   temperature, alpha: mean height of the climate cell, for the snow line), sampled per pixel
//!   by the curved materials (`curvature.wesl`) and refreshed every few game hours.
//! - The sky dome (`sky.wesl`): gradient, sun and Moon discs, aurorae, overcast deck.
//! - The stars: one mesh of small quads in equatorial coordinates, rotated to the local sky
//!   every frame (§2.8), faded by daylight and clouds.
//! - The directional light only casts shadows near the camera (the curved materials light
//!   every pixel with its own sun); fog and clear colours follow the sky at the camera.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{DistanceFog, Material};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use nascio_core::astro::{
    AstroConfig, SunPosition, aurora_night_factor, ecliptic_to_equatorial, equatorial_direction,
    light_intensity, moon_illumination, sidereal_angle, sky_light, sun_ecliptic_longitude,
};
use nascio_core::climate::Harmonics;
use nascio_core::consts::WORLD_SIZE_F64;
use nascio_core::geo::{latitude, seam_distance};
use nascio_worldgen::world_io::World;

use crate::fly::{FlyCam, FlyCamera, FlySettings, Sun, WorldInfo};

const SHADER: &str = "embedded://nascio_viewer/sky.wesl";
/// Radius of the sky dome, m (inside the far plane, beyond the horizon).
const DOME_RADIUS: f32 = 54_000.0;
/// Radius of the star sphere, m.
const STAR_RADIUS: f32 = 50_000.0;
/// Climate texture refresh, game days.
const CLIMATE_REFRESH_DAYS: f64 = 0.05;
/// Altitude of the base of the aurora curtains, m.
const AURORA_ALTITUDE_M: f32 = 4_000.0;

/// The instant and the sky at the camera, once per frame.
#[derive(Resource, Clone, Debug, Default)]
pub struct SkyState {
    /// Continuous day of the year.
    pub day: f64,
    /// Sun hour angle at the camera, rad.
    pub hour_angle: f64,
    /// Declination δ(N), rad (before the seam weight).
    pub declination: f64,
    /// Sun at the camera.
    pub sun: Option<SunPosition>,
    /// Cloud cover at the camera, `max(c_min(s), weather)`.
    pub cloud: f64,
    /// Local sidereal angle at the camera, rad.
    pub sidereal: f64,
    /// Towards the Moon, world axes.
    pub moon_dir: [f64; 3],
    /// Illuminated fraction of the Moon.
    pub moon_lit: f64,
}

impl SkyState {
    fn sun(&self) -> SunPosition {
        self.sun.unwrap_or(SunPosition {
            elevation: -FRAC_PI_2,
            azimuth: 0.0,
            visible_elevation: -FRAC_PI_2,
        })
    }
}

/// Cloud and snow harmonics of the world, and the texture they are evaluated into.
#[derive(Resource)]
pub struct ClimateMaps {
    res: usize,
    cloud: Vec<Harmonics>,
    snow: Vec<Harmonics>,
    temperature: Vec<Harmonics>,
    /// Mean height of each climate cell (sea counted as 0 m, like the climate), as a byte.
    height: Vec<u8>,
    pub texture: Handle<Image>,
    last_day: Option<f64>,
}

fn harmonics(world: &World, name: &str) -> Option<Vec<Harmonics>> {
    let get = |s: &str| world.layer(&format!("{name}_{s}"));
    let (x0, a1, p1, a2, p2) = (get("x0")?, get("a1")?, get("p1")?, get("a2")?, get("p2")?);
    Some(
        (0..x0.width * x0.height)
            .map(|k| Harmonics {
                x0: x0.data.get_f64(k),
                a1: a1.data.get_f64(k),
                p1: p1.data.get_f64(k),
                a2: a2.data.get_f64(k),
                p2: p2.data.get_f64(k),
            })
            .collect(),
    )
}

impl ClimateMaps {
    /// Reads the harmonics (a world without climate gets clear skies and no snow).
    pub fn new(world: &World, images: &mut Assets<Image>) -> Self {
        let res = world.layer("cloud_cover_x0").map_or(1, |l| l.width);
        let cloud = harmonics(world, "cloud_cover").unwrap_or_else(|| vec![Harmonics::default()]);
        let snow = harmonics(world, "snow_fraction").unwrap_or_else(|| vec![Harmonics::default()]);
        let temperature = harmonics(world, "temperature").unwrap_or_default();
        let height = cell_heights(world, res);
        let mut image = Image::new_fill(
            Extent3d {
                width: res as u32,
                height: res as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::default(),
        );
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            ..default()
        });
        Self {
            res,
            cloud,
            snow,
            temperature,
            height,
            texture: images.add(image),
            last_day: None,
        }
    }

    /// Weather cloud cover at a world position (nearest cell).
    pub fn cloud_at(&self, x: f64, z: f64, day: f64) -> f64 {
        let cell = WORLD_SIZE_F64 / self.res as f64;
        let i = ((nascio_core::torus::wrap_f64(x) / cell) as usize).min(self.res - 1);
        let j = ((nascio_core::torus::wrap_f64(z) / cell) as usize).min(self.res - 1);
        self.cloud
            .get(j * self.res + i)
            .map_or(0.0, |h| h.eval(day).clamp(0.0, 1.0))
    }

    fn bytes(&self, day: f64) -> Vec<u8> {
        let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let mut out = Vec::with_capacity(self.res * self.res * 4);
        for k in 0..self.res * self.res {
            let c = self.cloud.get(k).map_or(0.0, |h| h.eval(day));
            let s = self.snow.get(k).map_or(0.0, |h| h.eval(day));
            // Without temperatures (older worlds), +50 °C: no snow added with height.
            let t = self.temperature.get(k).map_or(50.0, |h| h.eval(day));
            let a = self.height.get(k).copied().unwrap_or(0);
            out.extend_from_slice(&[byte(c), byte(s), byte((t + 50.0) / 100.0), a]);
        }
        out
    }
}

/// Height scale of the climate texture's alpha, m (matches `CLIMATE_HEIGHT` in the shader).
const CLIMATE_HEIGHT_M: f64 = 3_500.0;

/// Mean height of each `res²` climate cell over the fine `elevation` layer, sea counted as 0 m
/// (the climate's own cell height), encoded on a byte over [`CLIMATE_HEIGHT_M`].
fn cell_heights(world: &World, res: usize) -> Vec<u8> {
    let Some(e) = world.layer("elevation") else {
        return vec![0; res * res];
    };
    let ratio = (e.width / res).max(1);
    let norm = 1.0 / (ratio * ratio) as f64;
    (0..res * res)
        .map(|k| {
            let (i, j) = (k % res, k / res);
            let mut sum = 0.0;
            for fj in j * ratio..(j + 1) * ratio {
                for fi in i * ratio..(i + 1) * ratio {
                    sum += e
                        .data
                        .get_f64((fj % e.height) * e.width + fi % e.width)
                        .max(0.0);
                }
            }
            ((sum * norm / CLIMATE_HEIGHT_M).clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect()
}

/// Instant shown by the 3D mode: real time, or the fixed day of year and local solar hour of
/// the settings. Returns `(day of year, days since the epoch, hour angle at x)`.
pub fn instant(info: &WorldInfo, settings: &FlySettings, x: f64) -> (f64, f64, f64) {
    let now = info.clock.now();
    let lon_frac = nascio_core::torus::wrap_f64(x) / WORLD_SIZE_F64;
    // Day fraction at the reference longitude (λ = 0).
    let frac0 = match settings.fixed_hour {
        Some(h) => (h / 24.0 - lon_frac).rem_euclid(1.0),
        None => now.day_frac(),
    };
    let day = settings
        .fixed_day
        .unwrap_or_else(|| now.day_of_year())
        .floor()
        + frac0;
    let t_days = now.year() as f64 * 365.0 + day;
    let local = (frac0 + lon_frac).rem_euclid(1.0);
    (day, t_days, TAU * local - PI)
}

/// Computes [`SkyState`] and refreshes the climate texture.
pub fn update_sky_state(
    info: Res<WorldInfo>,
    settings: Res<FlySettings>,
    cam: Res<FlyCam>,
    mut sky: ResMut<SkyState>,
    mut maps: ResMut<ClimateMaps>,
    mut images: ResMut<Assets<Image>>,
) {
    let astro: AstroConfig = info.astro;
    let (day, t_days, h) = instant(&info, &settings, cam.pos.x);
    let z = cam.pos.z;
    let sun = astro.sun_at(day, h, z);
    let (ra, dec) = astro.moon_equatorial(t_days, day);
    let sidereal = sidereal_angle(h, day);
    let phi = latitude(z);
    let moon_dir = equatorial_direction(phi, sidereal, ra, dec);
    let (rs, ds) = ecliptic_to_equatorial(sun_ecliptic_longitude(day), 0.0);
    let a = [dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin()];
    let b = [ds.cos() * rs.cos(), ds.cos() * rs.sin(), ds.sin()];
    let elongation = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2])
        .clamp(-1.0, 1.0)
        .acos();
    *sky = SkyState {
        day,
        hour_angle: h,
        declination: nascio_core::astro::declination(day),
        sun: Some(sun),
        cloud: astro.cloud_cover(seam_distance(z), maps.cloud_at(cam.pos.x, z, day)),
        sidereal,
        moon_dir,
        moon_lit: moon_illumination(elongation),
    };
    let stale = maps.last_day.is_none_or(|d| {
        let dd = (day - d).abs();
        dd.min(365.0 - dd) > CLIMATE_REFRESH_DAYS
    });
    if stale {
        let bytes = maps.bytes(day);
        if let Some(mut img) = images.get_mut(&maps.texture) {
            img.data = Some(bytes);
        }
        maps.last_day = Some(day);
    }
}

/// Colour of the sky near the horizon at the camera (fog and clear colour), linear RGB.
pub fn horizon_color(e_vis: f64, cloud: f64) -> [f32; 3] {
    let s =
        |a: f64, b: f64, x: f64| nascio_core::math::smoothstep(a.to_radians(), b.to_radians(), x);
    let night_k = s(-18.0, -4.0, e_vis);
    let day_k = s(-2.0, 15.0, e_vis);
    let mix = |a: [f64; 3], b: [f64; 3], t: f64| {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    let hor = mix(
        mix([0.006, 0.008, 0.018], [0.3, 0.25, 0.3], night_k),
        [0.62, 0.74, 0.9],
        day_k,
    );
    let grey_k = 0.012 + 0.988 * sky_light(e_vis);
    let grey = [0.5 * grey_k, 0.52 * grey_k, 0.56 * grey_k];
    let c = mix(hor, grey, cloud.clamp(0.0, 1.0));
    [c[0] as f32, c[1] as f32, c[2] as f32]
}

/// Sun light (shadows only), fog and clear colours.
pub fn update_light_and_fog(
    sky: Res<SkyState>,
    settings: Res<FlySettings>,
    mut sun: Query<(&mut Transform, &mut DirectionalLight), With<Sun>>,
    mut fog: Query<&mut DistanceFog>,
    mut clear: ResMut<ClearColor>,
) {
    let p = sky.sun();
    let d = p.direction();
    let dir = Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32);
    if let Ok((mut t, mut light)) = sun.single_mut() {
        if light.shadow_maps_enabled != settings.shadows {
            light.shadow_maps_enabled = settings.shadows;
        }
        // Never exactly vertical (looking_to needs a non-parallel up).
        *t = Transform::default().looking_to(-dir, Vec3::new(0.0, 1.0, 1e-3).normalize());
        light.illuminance =
            (12_000.0 * light_intensity(p.visible_elevation) * (1.0 - 0.8 * sky.cloud)) as f32;
    }
    let c = horizon_color(p.visible_elevation, sky.cloud);
    let color = Color::linear_rgb(c[0], c[1], c[2]);
    clear.0 = color;
    if let Ok(mut f) = fog.single_mut() {
        f.color = color;
    }
}

// ---------------------------------------------------------------------------------------------
// Dome

#[derive(ShaderType, Clone, Copy, Debug, Default, Reflect)]
pub struct SkyUniform {
    pub sun: Vec4,
    pub moon: Vec4,
    pub sun_geo: Vec4,
    pub cam: Vec4,
    pub time: Vec4,
    pub seam: Vec4,
    pub aurora: Vec4,
    /// xyz: towards the north celestial pole.
    pub pole: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct SkyMaterial {
    #[uniform(0)]
    pub u: SkyUniform,
    /// Moon albedo map (NASA SVS CGI Moon Kit, see `ASSETS.md`).
    #[texture(1)]
    #[sampler(2)]
    pub moon_map: Option<Handle<Image>>,
}

/// Moon map shipped with the viewer.
const MOON_MAP: &str = "assets/moon/lroc_color_512.png";

/// Loads the Moon map (next to the working directory, the executable or the crate sources);
/// `None` (a plain disc) if it cannot be read.
fn load_moon_map(images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let candidates = [
        std::path::PathBuf::from(MOON_MAP),
        std::path::Path::new("crates/viewer").join(MOON_MAP),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(MOON_MAP),
    ];
    let bytes = candidates.iter().find_map(|p| std::fs::read(p).ok());
    let Some(bytes) = bytes else {
        warn!("moon map {MOON_MAP} not found: plain Moon");
        return None;
    };
    match Image::from_buffer(
        &bytes,
        bevy::image::ImageType::Extension("png"),
        bevy::image::CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::default(),
    ) {
        Ok(image) => Some(images.add(image)),
        Err(e) => {
            warn!("moon map {MOON_MAP}: {e}");
            None
        }
    }
}

impl Material for SkyMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }
}

#[derive(Component)]
pub struct SkyDome;

#[derive(Component)]
pub struct StarField;

#[derive(Resource)]
pub struct StarMaterial(pub Handle<StandardMaterial>);

/// A sphere seen from inside (triangles wound inwards).
fn dome_mesh(radius: f32) -> Mesh {
    let (rings, segments) = (32u32, 64u32);
    let mut pos = Vec::new();
    let mut normals = Vec::new();
    for r in 0..=rings {
        let theta = PI as f32 * r as f32 / rings as f32;
        for s in 0..=segments {
            let p = TAU as f32 * s as f32 / segments as f32;
            let v = Vec3::new(theta.sin() * p.cos(), theta.cos(), theta.sin() * p.sin());
            pos.push((v * radius).to_array());
            normals.push((-v).to_array());
        }
    }
    let mut idx = Vec::new();
    let w = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let (a, b, c, d) = (
                r * w + s,
                r * w + s + 1,
                (r + 1) * w + s,
                (r + 1) * w + s + 1,
            );
            // Both windings: seen from inside whatever the culling convention.
            idx.extend_from_slice(&[a, b, c, b, d, c, a, c, b, b, c, d]);
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_indices(Indices::U32(idx))
}

/// The stars of `catalogue` down to magnitude `limit` as small quads facing the centre, in
/// equatorial axes (x: α = 0, y: α = 6 h, z: north celestial pole); brightness from the
/// magnitude and tint from the B−V index in the vertex colour.
fn star_mesh(catalogue: &[crate::stars::Star], limit: f64) -> Mesh {
    let mut pos = Vec::new();
    let mut colors = Vec::new();
    let mut idx = Vec::new();
    for s in catalogue.iter().filter(|s| s.vmag <= limit) {
        let n = Vec3::new(
            (s.dec.cos() * s.ra.cos()) as f32,
            (s.dec.cos() * s.ra.sin()) as f32,
            s.dec.sin() as f32,
        );
        let t = if n.z.abs() < 0.99 { Vec3::Z } else { Vec3::X };
        let u = n.cross(t).normalize();
        let v = n.cross(u);
        let mag = s.vmag as f32;
        let size = (55.0 + 45.0 * (2.5 - mag).max(0.0)) * 0.5;
        // Perceived brightness: flux relative to a first-magnitude star, softened.
        let b = 10f32.powf(-0.4 * (mag - 1.0)).powf(0.6).clamp(0.015, 1.8);
        let tint = crate::stars::color(s.bv);
        let base = pos.len() as u32;
        for (du, dv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            pos.push((n * STAR_RADIUS + (u * du + v * dv) * size).to_array());
            colors.push([b * tint[0], b * tint[1], b * tint[2], 1.0]);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(idx))
}

/// The star catalogue and the mesh built for the current magnitude limit.
#[derive(Resource)]
pub struct StarCatalogue {
    stars: Vec<crate::stars::Star>,
    mesh: Handle<Mesh>,
    limit: f64,
}

/// Rebuilds the star mesh when the magnitude limit slider moves.
pub fn update_star_limit(
    settings: Res<FlySettings>,
    mut catalogue: ResMut<StarCatalogue>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let limit = settings.star_mag_limit;
    if (catalogue.limit - limit).abs() < 1e-6 {
        return;
    }
    catalogue.limit = limit;
    if let Some(mut m) = meshes.get_mut(&catalogue.mesh) {
        *m = star_mesh(&catalogue.stars, limit);
    }
}

pub fn setup_sky(
    mut commands: Commands,
    settings: Res<FlySettings>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sky_materials: ResMut<Assets<SkyMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let moon_map = load_moon_map(&mut images);
    commands.spawn((
        Mesh3d(meshes.add(dome_mesh(DOME_RADIUS))),
        MeshMaterial3d(sky_materials.add(SkyMaterial {
            moon_map,
            ..default()
        })),
        Transform::default(),
        NoFrustumCulling,
        NotShadowCaster,
        NotShadowReceiver,
        SkyDome,
    ));
    let stars = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 1.0, 1.0, 0.0),
        unlit: true,
        alpha_mode: AlphaMode::Add,
        fog_enabled: false,
        cull_mode: None,
        ..default()
    });
    commands.insert_resource(StarMaterial(stars.clone()));
    let catalogue = crate::stars::load();
    let limit = settings.star_mag_limit;
    let mesh = meshes.add(star_mesh(&catalogue, limit));
    commands.insert_resource(StarCatalogue {
        stars: catalogue,
        mesh: mesh.clone(),
        limit,
    });
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(stars),
        Transform::default(),
        NoFrustumCulling,
        NotShadowCaster,
        NotShadowReceiver,
        StarField,
    ));
}

type CameraQuery<'w, 's> =
    Query<'w, 's, &'static Transform, (With<FlyCamera>, Without<SkyDome>, Without<StarField>)>;
type DomeQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static MeshMaterial3d<SkyMaterial>),
    (With<SkyDome>, Without<StarField>),
>;
type StarQuery<'w, 's> = Query<'w, 's, &'static mut Transform, (With<StarField>, Without<SkyDome>)>;

/// Places the dome and the stars on the camera and pushes the sky uniforms.
#[allow(clippy::too_many_arguments)]
pub fn update_sky(
    sky: Res<SkyState>,
    info: Res<WorldInfo>,
    settings: Res<FlySettings>,
    cam: Res<FlyCam>,
    time: Res<Time>,
    camera: CameraQuery,
    mut dome: DomeQuery,
    mut stars: StarQuery,
    star_mat: Option<Res<StarMaterial>>,
    mut sky_materials: ResMut<Assets<SkyMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok(cam_t) = camera.single() else {
        return;
    };
    let astro = info.astro;
    let p = sky.sun();
    let v = |d: [f64; 3]| Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32);
    let sun_geo = nascio_core::astro::direction(p.elevation, p.azimuth);
    let k = settings.curvature_radius_m.map_or(0.0, |r| 0.5 / r) as f32;
    if let Ok((mut t, m)) = dome.single_mut() {
        t.translation = cam_t.translation;
        if let Some(mut mat) = sky_materials.get_mut(&m.0) {
            mat.u = SkyUniform {
                sun: v(p.direction()).extend(p.visible_elevation as f32),
                moon: v(sky.moon_dir).extend(sky.moon_lit as f32),
                sun_geo: v(sun_geo).extend(sky.cloud as f32),
                cam: Vec4::new(
                    nascio_core::torus::wrap_f64(cam.pos.x) as f32,
                    nascio_core::torus::wrap_f64(cam.pos.z) as f32,
                    cam.pos.y as f32,
                    k,
                ),
                time: Vec4::new(
                    sky.hour_angle as f32,
                    sky.declination as f32,
                    time.elapsed_secs_wrapped(),
                    1.0,
                ),
                seam: Vec4::new(
                    astro.s_fade as f32,
                    astro.s_dark as f32,
                    astro.s_light as f32,
                    astro.s_clear as f32,
                ),
                aurora: Vec4::new(
                    astro.s_aurora as f32,
                    astro.s_aurora_out as f32,
                    AURORA_ALTITUDE_M,
                    0.0,
                ),
                pole: v(equatorial_direction(
                    latitude(cam.pos.z),
                    sky.sidereal,
                    0.0,
                    FRAC_PI_2,
                ))
                .extend(0.0),
            };
        }
    }
    // Stars: equatorial axes → local sky (columns are the images of the equatorial basis).
    let phi = latitude(cam.pos.z);
    let col = |ra: f64, dec: f64| v(equatorial_direction(phi, sky.sidereal, ra, dec));
    let rot = Mat3::from_cols(col(0.0, 0.0), col(FRAC_PI_2, 0.0), col(0.0, FRAC_PI_2));
    if let Ok(mut t) = stars.single_mut() {
        t.translation = cam_t.translation;
        t.rotation = Quat::from_mat3(&rot);
    }
    if let Some(sm) = star_mat
        && let Some(mut m) = materials.get_mut(&sm.0)
    {
        // Visible at night (§2.8), never through the clouds (weight 1 − c).
        let night = aurora_night_factor(p.visible_elevation);
        let vis = (night * (1.0 - sky.cloud)) as f32;
        if (m.base_color.alpha() - vis).abs() > 1e-3 {
            m.base_color = Color::srgba(1.0, 1.0, 1.0, vis);
        }
    }
}

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "sky.wesl");
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default())
            .init_resource::<SkyState>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizon_colour_is_continuous_and_dark_at_night() {
        let n = horizon_color((-30f64).to_radians(), 0.0);
        let d = horizon_color(30f64.to_radians(), 0.0);
        assert!(n.iter().all(|&c| c < 0.05) && d[2] > 0.8);
        // Overcast night stays dark (§2.7: black night under the deck).
        assert!(
            horizon_color((-30f64).to_radians(), 1.0)
                .iter()
                .all(|&c| c < 0.01)
        );
        let mut prev = horizon_color((-30f64).to_radians(), 0.5);
        for k in 1..=600 {
            let c = horizon_color((-30.0 + 0.1 * f64::from(k)).to_radians(), 0.5);
            for i in 0..3 {
                assert!((c[i] - prev[i]).abs() < 0.02);
            }
            prev = c;
        }
    }

    #[test]
    fn star_rotation_is_orthonormal() {
        for &(phi, sid) in &[(0.3, 1.0), (-1.2, 4.0), (1.5, -2.0)] {
            let col = |ra: f64, dec: f64| {
                let d = equatorial_direction(phi, sid, ra, dec);
                Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32)
            };
            let m = Mat3::from_cols(col(0.0, 0.0), col(FRAC_PI_2, 0.0), col(0.0, FRAC_PI_2));
            assert!((m.determinant() - 1.0).abs() < 1e-4, "{}", m.determinant());
            // The celestial pole sits at elevation |φ| above the horizon.
            assert!((m.z_axis.y - (phi as f32).sin()).abs() < 1e-4);
        }
    }
}
