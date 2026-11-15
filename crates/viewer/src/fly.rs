//! Free-flight 3D mode (spec §7): camera, chunk streaming, lighting and HUD.
//!
//! **Camera-relative rendering.** The camera position is a `WorldPos` (f64, wrapped). Bevy only
//! sees a *render origin*: an integer block position near the camera, re-centred every
//! [`REBASE_DISTANCE`] m. Every anchored entity (chunks, horizon) is placed at the torus
//! `delta` from that origin, so coordinates given to the GPU stay small and the seam is
//! handled by the same `delta` as everything else.
//!
//! **Streaming.** Chunk columns within the view distance are generated and meshed on a
//! dedicated rayon pool, nearest first, then turned into Bevy meshes on the main thread.

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use bevy::asset::RenderAssetUsages;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::DistanceFog;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use bevy_egui::{EguiContexts, egui};
use nascio_core::astro::AstroConfig;
use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS};
use nascio_core::coords::{BlockPos, WorldPos};
use nascio_core::geo::{latitude, longitude, seam_distance};
use nascio_core::time::GameClock;
use nascio_core::torus::delta;
use nascio_voxel::mesh::MeshData;
use nascio_voxel::{ChunkMesh, ColumnSet, Terrain, mesh_chunk};

use crate::perf::FrameStats;

/// Distance from the render origin beyond which it is moved to the camera.
pub const REBASE_DISTANCE: f64 = 1_024.0;
/// Speed range of the camera, m/s.
pub const SPEED_MIN: f64 = 5.0;
pub const SPEED_MAX: f64 = 2_000.0;
/// View distance range, m.
pub const VIEW_MIN: f32 = 128.0;
pub const VIEW_MAX: f32 = 1_024.0;
/// Camera never goes closer to the ground than this.
const GROUND_CLEARANCE: f64 = 1.7;

/// User-facing settings of the 3D mode.
#[derive(Resource, Clone, Debug)]
pub struct FlySettings {
    pub view_distance_m: f32,
    /// Local solar hour forced for lighting; `None` follows real time (spec §2.3).
    pub fixed_hour: Option<f64>,
    pub horizon: bool,
}

impl Default for FlySettings {
    fn default() -> Self {
        Self {
            view_distance_m: 512.0,
            fixed_hour: None,
            horizon: true,
        }
    }
}

/// The free camera.
#[derive(Resource, Clone, Debug)]
pub struct FlyCam {
    pub pos: WorldPos,
    /// Heading from north (−z) towards east (+x), radians.
    pub yaw: f64,
    /// Up positive, radians.
    pub pitch: f64,
    /// m/s.
    pub speed: f64,
    /// Set by scripted motion (benchmark): user input is ignored.
    pub scripted: bool,
}

impl FlyCam {
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(-self.yaw as f32) * Quat::from_rotation_x(self.pitch as f32)
    }

    pub fn forward(&self) -> [f64; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [sy * cp, sp, -cy * cp]
    }
}

/// Integer world position Bevy's coordinates are relative to.
#[derive(Resource, Clone, Copy, Debug)]
pub struct RenderOrigin(pub BlockPos);

impl RenderOrigin {
    /// Bevy translation of an integer world position.
    pub fn translation(&self, x: i32, y: i32, z: i32) -> Vec3 {
        Vec3::new(
            delta(self.0.x, x) as f32,
            (y - self.0.y) as f32,
            delta(self.0.z, z) as f32,
        )
    }
}

/// An entity placed at an integer world position (kept relative to the render origin).
#[derive(Component, Clone, Copy, Debug)]
pub struct Anchor {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// The terrain shared with worker threads.
#[derive(Resource, Clone)]
pub struct SharedTerrain(pub Arc<Terrain>);

/// World data needed by the HUD and lighting.
#[derive(Resource, Clone)]
pub struct WorldInfo {
    pub astro: AstroConfig,
    pub clock: GameClock,
    pub seed: u64,
}

/// Materials shared by every chunk.
#[derive(Resource)]
pub struct TerrainMaterials {
    pub opaque: Handle<StandardMaterial>,
    pub water: Handle<StandardMaterial>,
}

#[derive(Component)]
pub struct FlyCamera;

#[derive(Component)]
pub struct Sun;

/// Whether the mouse is over the UI (input goes to egui, not to the camera).
#[derive(Resource, Default)]
pub struct UiCapture(pub bool);

// ---------------------------------------------------------------------------------------------
// Streaming

struct ColumnResult {
    key: (i32, i32),
    meshes: Vec<(i32, ChunkMesh)>,
}

enum Slot {
    Pending,
    Ready { entities: Vec<Entity>, quads: usize },
}

/// Chunk column loader.
#[derive(Resource)]
pub struct Streamer {
    pool: rayon::ThreadPool,
    threads: usize,
    tx: Sender<ColumnResult>,
    rx: Mutex<Receiver<ColumnResult>>,
    slots: HashMap<(i32, i32), Slot>,
    in_flight: usize,
    /// Column offsets within the view radius, nearest first.
    offsets: Vec<(i32, i32)>,
    radius: i32,
    /// Desired columns not ready yet (0 when everything in range is meshed).
    pub missing: usize,
    pub quads: usize,
    pub ready_columns: usize,
}

impl Streamer {
    pub fn new() -> Self {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        let threads = (cores / 2).max(1);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("chunk-worker-{i}"))
            .build()
            .expect("chunk worker pool");
        let (tx, rx) = channel();
        Self {
            pool,
            threads,
            tx,
            rx: Mutex::new(rx),
            slots: HashMap::new(),
            in_flight: 0,
            offsets: Vec::new(),
            radius: -1,
            missing: usize::MAX,
            quads: 0,
            ready_columns: 0,
        }
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    fn set_radius(&mut self, radius: i32) {
        if radius == self.radius {
            return;
        }
        self.radius = radius;
        let r2 = radius * radius;
        let mut v: Vec<(i32, i32)> = (-radius..=radius)
            .flat_map(|dz| (-radius..=radius).map(move |dx| (dx, dz)))
            .filter(|(dx, dz)| dx * dx + dz * dz <= r2)
            .collect();
        v.sort_by_key(|&(dx, dz)| (dx * dx + dz * dz, dz, dx));
        self.offsets = v;
    }
}

fn chunk_delta(a: i32, b: i32) -> i32 {
    let d = (b - a).rem_euclid(CHUNKS_PER_AXIS);
    if d > CHUNKS_PER_AXIS / 2 {
        d - CHUNKS_PER_AXIS
    } else {
        d
    }
}

fn to_bevy_mesh(m: MeshData) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, m.colors)
    .with_inserted_indices(Indices::U32(m.indices))
}

/// Receives finished columns, requests missing ones, drops far ones.
#[allow(clippy::too_many_arguments)]
pub fn stream_chunks(
    mut commands: Commands,
    mut streamer: ResMut<Streamer>,
    terrain: Res<SharedTerrain>,
    settings: Res<FlySettings>,
    cam: Res<FlyCam>,
    origin: Res<RenderOrigin>,
    materials: Res<TerrainMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let s = &mut *streamer;
    s.set_radius((settings.view_distance_m / CHUNK_SIZE as f32).ceil() as i32);
    let cam_block = cam.pos.block();
    let (ccx, ccz) = (
        cam_block.x.div_euclid(CHUNK_SIZE),
        cam_block.z.div_euclid(CHUNK_SIZE),
    );
    let keep = s.radius + 2;
    let in_range = |key: (i32, i32), r: i32| {
        let (dx, dz) = (chunk_delta(ccx, key.0), chunk_delta(ccz, key.1));
        dx * dx + dz * dz <= r * r
    };

    // 1. Finished columns.
    let results: Vec<ColumnResult> = s.rx.lock().expect("channel").try_iter().collect();
    for r in results {
        s.in_flight -= 1;
        if !matches!(s.slots.get(&r.key), Some(Slot::Pending)) || !in_range(r.key, keep) {
            s.slots.remove(&r.key);
            continue;
        }
        let mut entities = Vec::new();
        let mut quads = 0;
        for (cy, m) in r.meshes {
            let anchor = Anchor {
                x: r.key.0 * CHUNK_SIZE,
                y: cy * CHUNK_SIZE,
                z: r.key.1 * CHUNK_SIZE,
            };
            let t = Transform::from_translation(origin.translation(anchor.x, anchor.y, anchor.z));
            quads += m.opaque.quad_count() + m.water.quad_count();
            if !m.opaque.is_empty() {
                let h = meshes.add(to_bevy_mesh(m.opaque));
                entities.push(
                    commands
                        .spawn((
                            Mesh3d(h),
                            MeshMaterial3d(materials.opaque.clone()),
                            t,
                            anchor,
                        ))
                        .id(),
                );
            }
            if !m.water.is_empty() {
                let h = meshes.add(to_bevy_mesh(m.water));
                entities.push(
                    commands
                        .spawn((
                            Mesh3d(h),
                            MeshMaterial3d(materials.water.clone()),
                            t,
                            anchor,
                        ))
                        .id(),
                );
            }
        }
        s.quads += quads;
        s.slots.insert(r.key, Slot::Ready { entities, quads });
    }

    // 2. Drop columns out of range.
    let far: Vec<(i32, i32)> = s
        .slots
        .keys()
        .copied()
        .filter(|&k| !in_range(k, keep))
        .collect();
    for k in far {
        if let Some(Slot::Ready { entities, quads }) = s.slots.remove(&k) {
            s.quads -= quads;
            for e in entities {
                commands.entity(e).despawn();
            }
        }
        // A pending column is simply forgotten: its result will be discarded.
    }

    // 3. Request missing columns, nearest first.
    let max_in_flight = s.threads * 2;
    let underground = terrain.0.config().underground_chunks;
    let mut missing = 0;
    let mut ready = 0;
    for i in 0..s.offsets.len() {
        let (dx, dz) = s.offsets[i];
        let key = (
            (ccx + dx).rem_euclid(CHUNKS_PER_AXIS),
            (ccz + dz).rem_euclid(CHUNKS_PER_AXIS),
        );
        match s.slots.get(&key) {
            Some(Slot::Ready { .. }) => ready += 1,
            Some(Slot::Pending) => missing += 1,
            None => {
                missing += 1;
                if s.in_flight < max_in_flight {
                    s.in_flight += 1;
                    s.slots.insert(key, Slot::Pending);
                    let terrain = terrain.0.clone();
                    let tx = s.tx.clone();
                    s.pool.spawn(move || {
                        let col = ColumnSet::new(&terrain, key.0, key.1);
                        let meshes = col
                            .chunk_range(underground)
                            .map(|cy| (cy, mesh_chunk(&col.padded_chunk(&terrain, cy))))
                            .filter(|(_, m)| !m.is_empty())
                            .collect();
                        let _ = tx.send(ColumnResult { key, meshes });
                    });
                }
            }
        }
    }
    s.missing = missing;
    s.ready_columns = ready;
}

// ---------------------------------------------------------------------------------------------
// Camera

/// Mouse look (right button held), WASD/ZQSD + Space/Shift, wheel for speed.
#[allow(clippy::too_many_arguments)]
pub fn fly_input(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    ui: Res<UiCapture>,
    terrain: Res<SharedTerrain>,
    mut cam: ResMut<FlyCam>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if cam.scripted {
        return;
    }
    let looking = buttons.pressed(MouseButton::Right) && !ui.0;
    if let Ok(mut c) = cursor.single_mut() {
        let (grab, visible) = if looking {
            (CursorGrabMode::Locked, false)
        } else {
            (CursorGrabMode::None, true)
        };
        if c.grab_mode != grab {
            c.grab_mode = grab;
            c.visible = visible;
        }
    }
    if looking {
        let d = motion.delta;
        cam.yaw = (cam.yaw + f64::from(d.x) * 0.0025).rem_euclid(TAU);
        cam.pitch =
            (cam.pitch - f64::from(d.y) * 0.0025).clamp(-FRAC_PI_2 + 0.01, FRAC_PI_2 - 0.01);
    }
    if !ui.0 && scroll.delta.y != 0.0 {
        let notches = match scroll.unit {
            MouseScrollUnit::Line => f64::from(scroll.delta.y),
            MouseScrollUnit::Pixel => f64::from(scroll.delta.y) / 40.0,
        };
        cam.speed = (cam.speed * 1.25f64.powf(notches)).clamp(SPEED_MIN, SPEED_MAX);
    }

    // Physical keys: KeyW is Z on an AZERTY keyboard.
    let axis = |pos: KeyCode, neg: KeyCode| {
        f64::from(u8::from(keys.pressed(pos))) - f64::from(u8::from(keys.pressed(neg)))
    };
    let fwd = axis(KeyCode::KeyW, KeyCode::KeyS);
    let right = axis(KeyCode::KeyD, KeyCode::KeyA);
    let up = axis(KeyCode::Space, KeyCode::ShiftLeft);
    if fwd == 0.0 && right == 0.0 && up == 0.0 {
        return;
    }
    let f = cam.forward();
    let (sy, cy) = cam.yaw.sin_cos();
    let r = [cy, 0.0, sy];
    let mut v = [
        f[0] * fwd + r[0] * right,
        f[1] * fwd + up,
        f[2] * fwd + r[2] * right,
    ];
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let step = cam.speed * time.delta_secs_f64() / len;
    v = v.map(|c| c * step);
    let mut p = cam.pos.translate(v);
    let ground = terrain.0.height(p.x, p.z).max(0.0);
    p.y = p.y.max(ground + GROUND_CLEARANCE);
    cam.pos = p;
}

/// Moves the render origin when the camera is far from it, and places everything.
pub fn place_camera(
    cam: Res<FlyCam>,
    mut origin: ResMut<RenderOrigin>,
    mut camera: Query<&mut Transform, With<FlyCamera>>,
    mut anchored: Query<(&Anchor, &mut Transform), Without<FlyCamera>>,
) {
    let o = WorldPos::new(
        f64::from(origin.0.x),
        f64::from(origin.0.y),
        f64::from(origin.0.z),
    );
    let rel = cam.pos.relative_to(o);
    if f64::from(rel[0]).hypot(f64::from(rel[2])) > REBASE_DISTANCE || origin.is_changed() {
        let b = cam.pos.block();
        origin.0 = BlockPos::new(
            b.x.div_euclid(CHUNK_SIZE) * CHUNK_SIZE,
            0,
            b.z.div_euclid(CHUNK_SIZE) * CHUNK_SIZE,
        );
        for (a, mut t) in &mut anchored {
            t.translation = origin.translation(a.x, a.y, a.z);
        }
    }
    let o = WorldPos::new(
        f64::from(origin.0.x),
        f64::from(origin.0.y),
        f64::from(origin.0.z),
    );
    if let Ok(mut t) = camera.single_mut() {
        t.translation = Vec3::from_array(cam.pos.relative_to(o));
        t.rotation = cam.rotation();
    }
}

// ---------------------------------------------------------------------------------------------
// Lighting

/// Sun direction from the astronomy of §2.4 at the camera, sky and fog colours.
pub fn update_sun(
    info: Res<WorldInfo>,
    settings: Res<FlySettings>,
    cam: Res<FlyCam>,
    mut sun: Query<(&mut Transform, &mut DirectionalLight), With<Sun>>,
    mut fog: Query<&mut DistanceFog>,
    mut clear: ResMut<ClearColor>,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    let now = info.clock.now();
    let n = now.day_of_year();
    let h = match settings.fixed_hour {
        Some(hour) => TAU * (hour / 24.0) - PI,
        None => now.hour_angle(cam.pos.x),
    };
    let p = info.astro.sun_at(n, h, cam.pos.z);
    let d = p.direction();
    let dir = Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32);
    let day = (p.visible_elevation.to_degrees() / 6.0 + 0.5).clamp(0.0, 1.0) as f32;
    if let Ok((mut t, mut light)) = sun.single_mut() {
        *t = Transform::default().looking_to(-dir, Vec3::Y);
        light.illuminance = 12_000.0 * day;
    }
    // Dev viewer: night keeps a twilight floor so the relief stays readable.
    let sky = Color::srgb(0.10 + 0.35 * day, 0.14 + 0.50 * day, 0.24 + 0.66 * day);
    clear.0 = sky;
    ambient.brightness = 350.0 + 500.0 * day;
    if let Ok(mut f) = fog.single_mut() {
        f.color = sky;
    }
}

// ---------------------------------------------------------------------------------------------
// HUD

#[derive(Resource)]
pub struct HudState {
    pub teleport: (String, String),
    pub map_request: bool,
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            teleport: ("50000".into(), "50000".into()),
            map_request: false,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn hud(
    mut contexts: EguiContexts,
    mut cam: ResMut<FlyCam>,
    mut settings: ResMut<FlySettings>,
    mut hud: ResMut<HudState>,
    mut capture: ResMut<UiCapture>,
    streamer: Res<Streamer>,
    horizon: Option<Res<crate::horizon::HorizonStats>>,
    stats: Res<FrameStats>,
    terrain: Res<SharedTerrain>,
    info: Res<WorldInfo>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let p = cam.pos;
    let avg = stats.average();
    egui::Window::new("Nascio — 3D")
        .default_pos([10.0, 10.0])
        .resizable(false)
        .show(ctx, |ui| {
            ui.monospace(format!("x {:>9.1}  y {:>7.1}  z {:>9.1}", p.x, p.y, p.z));
            ui.monospace(format!(
                "φ {:+7.2}°  λ {:6.2}°  s {:6.0} m",
                latitude(p.z).to_degrees(),
                longitude(p.x).to_degrees(),
                seam_distance(p.z)
            ));
            ui.monospace(format!(
                "ground {:7.1} m   heading {:5.1}°",
                terrain.0.height(p.x, p.z),
                cam.yaw.to_degrees()
            ));
            ui.monospace(format!("speed {:7.1} m/s (wheel)", cam.speed));
            ui.separator();
            ui.monospace(format!(
                "frame {:6.2} ms  ({:5.1} i/s)",
                avg.frame_ms,
                1_000.0 / avg.frame_ms.max(1e-3)
            ));
            ui.monospace(format!(
                "CPU   {:6.2} ms  render CPU {:5.2} ms",
                avg.cpu_ms, avg.render_cpu_ms
            ));
            match avg.gpu_ms {
                Some(g) => ui.monospace(format!("GPU   {g:6.2} ms (timed passes)")),
                None => ui.monospace("GPU      n/a (no timestamp queries)"),
            };
            ui.monospace(format!(
                "columns {} ready, {} missing · {} k quads · {} workers",
                streamer.ready_columns,
                streamer.missing,
                streamer.quads / 1_000,
                streamer.threads()
            ));
            if let Some(h) = &horizon {
                ui.monospace(format!(
                    "horizon {} levels, {} k triangles, {:.0} km",
                    h.levels,
                    h.triangles / 1_000,
                    h.extent_m / 1_000.0
                ));
            }
            ui.separator();
            ui.add(
                egui::Slider::new(&mut settings.view_distance_m, VIEW_MIN..=VIEW_MAX)
                    .text("view distance (m)")
                    .step_by(32.0),
            );
            ui.checkbox(&mut settings.horizon, "horizon");
            let mut fixed = settings.fixed_hour.is_some();
            ui.horizontal(|ui| {
                ui.checkbox(&mut fixed, "fixed solar hour");
                let mut hour = settings.fixed_hour.unwrap_or(10.0);
                if fixed {
                    ui.add(egui::Slider::new(&mut hour, 0.0..=24.0));
                }
                settings.fixed_hour = fixed.then_some(hour);
            });
            ui.horizontal(|ui| {
                ui.label("x");
                ui.add(egui::TextEdit::singleline(&mut hud.teleport.0).desired_width(60.0));
                ui.label("z");
                ui.add(egui::TextEdit::singleline(&mut hud.teleport.1).desired_width(60.0));
                if ui.button("Teleport").clicked()
                    && let (Ok(x), Ok(z)) = (
                        hud.teleport.0.trim().parse::<f64>(),
                        hud.teleport.1.trim().parse::<f64>(),
                    )
                {
                    let ground = terrain.0.height(x, z).max(0.0);
                    cam.pos = WorldPos::new(x, ground + 60.0, z);
                }
            });
            if ui.button("Back to map").clicked() {
                hud.map_request = true;
            }
            ui.small(format!(
                "Right drag: look · WASD/ZQSD: move · Space/Shift: up/down · seed {}",
                info.seed
            ));
        });
    capture.0 = ctx.egui_wants_pointer_input() || ctx.is_pointer_over_egui();
    Ok(())
}
