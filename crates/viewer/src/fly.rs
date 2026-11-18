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

use std::collections::{BTreeSet, HashMap};
use std::f64::consts::{FRAC_PI_2, TAU};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoAutoAabb;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use bevy_egui::{EguiContexts, egui};
use nascio_core::astro::AstroConfig;
use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS};
use nascio_core::coords::{BlockPos, ChunkPos, WorldPos};
use nascio_core::geo::{latitude, longitude, seam_distance};
use nascio_core::time::GameClock;
use nascio_core::torus::{delta, delta_chunks};
use nascio_voxel::mesh::MeshData;
use nascio_voxel::{ChunkMesh, ColumnSet, DeltaStore, Terrain, mesh_chunk};

use crate::remote::Remote;

use crate::curvature::{self, CurvedMaterial};
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
    /// Day of the year forced for the sky and the seasons; `None` follows real time.
    pub fixed_day: Option<f64>,
    /// Sun shadows near the camera.
    pub shadows: bool,
    /// Dim night light so the relief stays readable (dev viewer).
    pub night_floor: bool,
    pub horizon: bool,
    /// Display curvature radius `R_vis`, m (`None`: flat). Render only.
    pub curvature_radius_m: Option<f64>,
    /// Faintest star shown (visual magnitude).
    pub star_mag_limit: f64,
    /// Vertical sync of the window (`--no-vsync` turns it off at start).
    pub vsync: bool,
    /// Borderless fullscreen.
    pub fullscreen: bool,
    /// Weather cloud cover forced everywhere (`--clouds`, captures); `None` follows the climate.
    pub fixed_clouds: Option<f64>,
}

impl Default for FlySettings {
    fn default() -> Self {
        Self {
            view_distance_m: VIEW_MAX,
            fixed_hour: None,
            fixed_day: None,
            shadows: true,
            night_floor: true,
            horizon: true,
            curvature_radius_m: crate::curvature::RenderConfig::default().radius(),
            star_mag_limit: crate::stars::DEFAULT_LIMIT,
            vsync: true,
            fullscreen: false,
            fixed_clouds: None,
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
    /// The local world's seed (`None` connected: the server does not give it).
    pub seed: Option<u64>,
    /// Lapse rate of the world's climate, °C per m (snow line in the shader, HUD weather).
    pub lapse_rate_c_per_m: f64,
    /// Pressure profile and air above the ground (HUD weather).
    pub atmosphere: nascio_core::AtmosphereConfig,
    /// Underground temperature (HUD below ground, local world only).
    pub geotherm: nascio_core::GeothermConfig,
}

impl WorldInfo {
    /// Before the connection: default settings, replaced by the server's `WorldSky`.
    pub fn unknown() -> Self {
        Self {
            astro: AstroConfig::default(),
            clock: GameClock::default(),
            seed: None,
            lapse_rate_c_per_m: 0.0065,
            atmosphere: Default::default(),
            geotherm: Default::default(),
        }
    }

    /// The settings of the server's world.
    pub fn from_sky(w: &nascio_protocol::sky::SkyWorld) -> Self {
        Self {
            astro: w.astro,
            clock: w.clock,
            seed: None,
            lapse_rate_c_per_m: w.lapse_rate_c_per_m,
            atmosphere: w.atmosphere,
            geotherm: Default::default(),
        }
    }
}

/// Materials shared by every chunk.
#[derive(Resource)]
pub struct TerrainMaterials {
    pub opaque: Handle<CurvedMaterial>,
    pub water: Handle<CurvedMaterial>,
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

/// Worker threads shared by chunk and horizon generation.
#[derive(Resource, Clone)]
pub struct Workers {
    pub pool: Arc<rayon::ThreadPool>,
    pub threads: usize,
}

impl Workers {
    pub fn new() -> Self {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        // Leave room for Bevy's main, render and compute threads.
        let threads = (cores / 2).max(1);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("terrain-worker-{i}"))
            .build()
            .expect("terrain worker pool");
        Self {
            pool: Arc::new(pool),
            threads,
        }
    }
}

/// The world's modifications (god mode), shared with the workers. The store locks each
/// region on its own; readers get chunk snapshots, so an edit copies at most one chunk.
#[derive(Resource, Clone)]
pub struct SharedDeltas(pub Arc<DeltaStore>);

impl SharedDeltas {
    pub fn snapshot(&self) -> Arc<DeltaStore> {
        self.0.clone()
    }
}

struct ColumnResult {
    key: (i32, i32),
    /// Request number: only the latest request of a column is kept.
    request: u64,
    /// `None`: not meshable yet (network mode: a neighbouring column is missing).
    meshes: Option<Vec<(i32, ChunkMesh)>>,
}

enum Slot {
    Pending(u64),
    Ready {
        entities: Vec<Entity>,
        quads: usize,
        shown: bool,
        /// A new mesh is on its way (after an edit): replaces this one when it arrives.
        remesh: Option<u64>,
    },
}

/// Chunk column loader.
///
/// A ready column is *shown* only once the displayed horizon has a hole for it, and stays
/// until the horizon covers it again: chunks and horizon never overlap and never leave a gap.
#[derive(Resource)]
pub struct Streamer {
    tx: Sender<ColumnResult>,
    rx: Mutex<Receiver<ColumnResult>>,
    slots: HashMap<(i32, i32), Slot>,
    in_flight: usize,
    /// Column offsets within the view radius, nearest first.
    offsets: Vec<(i32, i32)>,
    radius: i32,
    /// Camera chunk column.
    center: (i32, i32),
    /// Bumped whenever the set of ready columns in range changes.
    pub version: u64,
    /// Desired columns not ready yet (0 when everything in range is meshed).
    pub missing: usize,
    pub quads: usize,
    pub ready_columns: usize,
    requests: u64,
    /// Columns to mesh again (edited blocks).
    remesh: Vec<(i32, i32)>,
}

impl Streamer {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx: Mutex::new(rx),
            slots: HashMap::new(),
            in_flight: 0,
            offsets: Vec::new(),
            radius: -1,
            center: (i32::MIN, i32::MIN),
            version: 0,
            missing: usize::MAX,
            quads: 0,
            ready_columns: 0,
            requests: 0,
            remesh: Vec::new(),
        }
    }

    /// Forgets every column (back to the menu): their entities go, the loader starts over.
    pub fn reset(&mut self, commands: &mut Commands) {
        for slot in self.slots.values() {
            if let Slot::Ready { entities, .. } = slot {
                for &e in entities {
                    commands.entity(e).despawn();
                }
            }
        }
        *self = Self::new();
    }

    /// Meshes these columns again, if loaded or loading.
    pub fn request_remesh(&mut self, keys: impl IntoIterator<Item = (i32, i32)>) {
        self.remesh.extend(keys);
    }

    fn set_radius(&mut self, radius: i32) {
        if radius == self.radius {
            return;
        }
        self.radius = radius;
        self.version += 1;
        let r2 = radius * radius;
        let mut v: Vec<(i32, i32)> = (-radius..=radius)
            .flat_map(|dz| (-radius..=radius).map(move |dx| (dx, dz)))
            .filter(|(dx, dz)| dx * dx + dz * dz <= r2)
            .collect();
        v.sort_by_key(|&(dx, dz)| (dx * dx + dz * dz, dz, dx));
        self.offsets = v;
    }

    /// Whether column `key` lies within `radius` columns of the camera column.
    fn within(&self, key: (i32, i32), radius: i32) -> bool {
        let (dx, dz) = (
            delta_chunks(self.center.0, key.0),
            delta_chunks(self.center.1, key.1),
        );
        dx * dx + dz * dz <= radius * radius
    }

    /// Whether column `key` is meshed and within the view distance: the horizon leaves a hole
    /// for exactly these columns.
    pub fn wants_hole(&self, key: (i32, i32)) -> bool {
        matches!(self.slots.get(&key), Some(Slot::Ready { .. })) && self.within(key, self.radius)
    }
}

/// Where the columns come from.
#[derive(Clone)]
enum ColumnSource {
    /// Generated here, with the local deltas (local flight).
    Local(Arc<Terrain>, Arc<DeltaStore>, Arc<nascio_chunk::Blocks>),
    /// Sent by the server.
    Remote(Remote),
}

/// Generates and meshes column `key` (with its deltas) on the worker pool; in network mode,
/// meshes it from the columns the server sent instead (no generation).
fn spawn_column(
    workers: &Workers,
    source: ColumnSource,
    tx: Sender<ColumnResult>,
    key: (i32, i32),
    request: u64,
) {
    workers.pool.spawn(move || {
        let (terrain, deltas, blocks) = match source {
            ColumnSource::Remote(remote) => {
                let meshes = remote.mesh_column(key);
                let _ = tx.send(ColumnResult {
                    key,
                    request,
                    meshes,
                });
                return;
            }
            ColumnSource::Local(t, d, b) => (t, d, b),
        };
        let underground = terrain.config().underground_chunks;
        let col = ColumnSet::new(&terrain, key.0, key.1);
        let mut tint = terrain.chunk_tint(key.0, key.1);
        tint.water_depth = col.water_depths();
        let mut ys: BTreeSet<i32> = col
            .chunk_range(underground)
            .filter(|&cy| col.may_have_faces(cy))
            .collect();
        ys.extend(deltas.touched_ys(key.0, key.1));
        let meshes = ys
            .into_iter()
            .map(|cy| {
                let mut p = col.padded_chunk(&terrain, cy);
                deltas.apply_padded(ChunkPos::new(key.0, cy, key.1), &mut p);
                (cy, mesh_chunk(&p, &blocks, &tint))
            })
            .filter(|(_, m)| !m.is_empty())
            .collect();
        let _ = tx.send(ColumnResult {
            key,
            request,
            meshes: Some(meshes),
        });
    });
}

/// Spawns the Bevy entities of a meshed column; returns them and their quad count.
fn spawn_meshes(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &TerrainMaterials,
    origin: &RenderOrigin,
    key: (i32, i32),
    column: Vec<(i32, ChunkMesh)>,
    shown: bool,
) -> (Vec<Entity>, usize) {
    let mut entities = Vec::new();
    let mut quads = 0;
    for (cy, m) in column {
        let anchor = Anchor {
            x: key.0 * CHUNK_SIZE,
            y: cy * CHUNK_SIZE,
            z: key.1 * CHUNK_SIZE,
        };
        let t = Transform::from_translation(origin.translation(anchor.x, anchor.y, anchor.z));
        quads += m.opaque.quad_count() + m.water.quad_count();
        for (data, material, water) in [
            (m.opaque, &materials.opaque, false),
            (m.water, &materials.water, true),
        ] {
            if data.is_empty() {
                continue;
            }
            let mesh = to_bevy_mesh(data);
            // Bounds deep enough for the display curvature (no recomputation by Bevy).
            let aabb = curvature::chunk_aabb(&mesh).unwrap_or_default();
            let h = meshes.add(mesh);
            let mut e = commands.spawn((
                Mesh3d(h),
                MeshMaterial3d(material.clone()),
                t,
                anchor,
                aabb,
                NoAutoAabb,
                if shown {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                },
            ));
            if water {
                e.insert(bevy::light::NotShadowCaster);
            }
            entities.push(e.id());
        }
    }
    (entities, quads)
}

pub fn to_bevy_mesh(m: MeshData) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, m.colors)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, m.material);
    // Chunks carry their texture coordinates (UV_1); the horizon has none.
    if !m.uv.is_empty() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, m.uv);
    }
    mesh.insert_indices(Indices::U32(m.indices));
    mesh
}

/// Receives finished columns and requests missing ones, nearest first.
#[allow(clippy::too_many_arguments)]
pub fn stream_chunks(
    mut commands: Commands,
    mut streamer: ResMut<Streamer>,
    workers: Res<Workers>,
    terrain: Option<Res<SharedTerrain>>,
    deltas: Option<Res<SharedDeltas>>,
    settings: Res<FlySettings>,
    cam: Res<FlyCam>,
    origin: Res<RenderOrigin>,
    materials: Res<TerrainMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    remote: Option<Res<Remote>>,
    render: Option<Res<crate::textures::RenderBlocks>>,
) {
    let remote = remote.map(|r| r.clone());
    // Connected: the server's columns; else the local world (none: nothing to show yet).
    let source = match (&remote, &terrain, &deltas) {
        (Some(r), _, _) => ColumnSource::Remote(r.clone()),
        (None, Some(t), Some(d)) => match &render {
            Some(b) => ColumnSource::Local(t.0.clone(), d.snapshot(), b.0.clone()),
            None => return,
        },
        _ => return,
    };
    let s = &mut *streamer;
    s.set_radius((settings.view_distance_m / CHUNK_SIZE as f32).ceil() as i32);
    let cam_block = cam.pos.block();
    let center = (
        cam_block.x.div_euclid(CHUNK_SIZE),
        cam_block.z.div_euclid(CHUNK_SIZE),
    );
    if center != s.center {
        s.center = center;
        s.version += 1;
    }
    let keep = s.radius + 2;

    // 1. Finished columns (spawned hidden: the horizon decides when they appear); new meshes
    // of edited columns replace the old ones.
    let results: Vec<ColumnResult> = s.rx.lock().expect("channel").try_iter().collect();
    for r in results {
        s.in_flight -= 1;
        let in_range = s.within(r.key, keep);
        let Some(column_meshes) = r.meshes else {
            // Not meshable yet: a pending column is asked again later; a shown one keeps its
            // mesh until the next change.
            match s.slots.get_mut(&r.key) {
                Some(Slot::Pending(q)) if *q == r.request => {
                    s.slots.remove(&r.key);
                }
                Some(Slot::Ready { remesh, .. }) if *remesh == Some(r.request) => *remesh = None,
                _ => {}
            }
            continue;
        };
        match s.slots.get_mut(&r.key) {
            Some(Slot::Pending(q)) if *q == r.request => {
                if !in_range {
                    s.slots.remove(&r.key);
                    continue;
                }
                let (entities, quads) = spawn_meshes(
                    &mut commands,
                    &mut meshes,
                    &materials,
                    &origin,
                    r.key,
                    column_meshes,
                    false,
                );
                s.quads += quads;
                s.version += 1;
                s.slots.insert(
                    r.key,
                    Slot::Ready {
                        entities,
                        quads,
                        shown: false,
                        remesh: None,
                    },
                );
            }
            Some(Slot::Ready {
                entities,
                quads,
                shown,
                remesh,
            }) if *remesh == Some(r.request) => {
                for &e in entities.iter() {
                    commands.entity(e).despawn();
                }
                let (new, n) = spawn_meshes(
                    &mut commands,
                    &mut meshes,
                    &materials,
                    &origin,
                    r.key,
                    column_meshes,
                    *shown,
                );
                s.quads = s.quads - *quads + n;
                *entities = new;
                *quads = n;
                *remesh = None;
            }
            _ => {}
        }
    }

    // Edited columns: request a new mesh (a pending request is superseded).
    for key in std::mem::take(&mut s.remesh) {
        s.requests += 1;
        let request = s.requests;
        match s.slots.get_mut(&key) {
            Some(Slot::Pending(q)) => *q = request,
            Some(Slot::Ready { remesh, .. }) => *remesh = Some(request),
            None => continue,
        }
        s.in_flight += 1;
        spawn_column(&workers, source.clone(), s.tx.clone(), key, request);
    }

    // 2. Forget pending columns that left the range (their results will be discarded).
    let center_now = s.center;
    s.slots.retain(|&k, slot| {
        if !matches!(slot, Slot::Pending(_)) {
            return true;
        }
        let (dx, dz) = (
            delta_chunks(center_now.0, k.0),
            delta_chunks(center_now.1, k.1),
        );
        dx * dx + dz * dz <= keep * keep
    });

    // 3. Request missing columns, nearest first.
    let max_in_flight = workers.threads * 2;
    let mut missing = 0;
    let mut ready = 0;
    for i in 0..s.offsets.len() {
        let (dx, dz) = s.offsets[i];
        let key = (
            (center.0 + dx).rem_euclid(CHUNKS_PER_AXIS),
            (center.1 + dz).rem_euclid(CHUNKS_PER_AXIS),
        );
        match s.slots.get(&key) {
            Some(Slot::Ready { .. }) => ready += 1,
            Some(Slot::Pending(_)) => missing += 1,
            None => {
                missing += 1;
                if remote.as_ref().is_some_and(|r| !r.can_mesh(key)) {
                    continue;
                }
                if s.in_flight < max_in_flight {
                    s.in_flight += 1;
                    s.requests += 1;
                    let request = s.requests;
                    s.slots.insert(key, Slot::Pending(request));
                    spawn_column(&workers, source.clone(), s.tx.clone(), key, request);
                }
            }
        }
    }
    s.missing = missing;
    s.ready_columns = ready;
}

/// Shows the columns the horizon has a hole for (or, without horizon, those in range) and
/// despawns far columns once the horizon covers them.
pub fn column_visibility(
    mut commands: Commands,
    mut streamer: ResMut<Streamer>,
    settings: Res<FlySettings>,
    horizon: Res<crate::horizon::Horizon>,
    mut vis: Query<&mut Visibility, With<Anchor>>,
) {
    let s = &mut *streamer;
    let use_horizon = settings.horizon && horizon.is_displayed();
    let (radius, keep, center) = (s.radius, s.radius + 2, s.center);
    let within = |k: (i32, i32), r: i32| {
        let (dx, dz) = (delta_chunks(center.0, k.0), delta_chunks(center.1, k.1));
        dx * dx + dz * dz <= r * r
    };
    let mut removed = 0;
    s.slots.retain(|&key, slot| {
        let Slot::Ready {
            entities,
            quads,
            shown,
            ..
        } = slot
        else {
            return true;
        };
        let holed = use_horizon && horizon.has_hole(key);
        let want = if use_horizon {
            holed
        } else {
            within(key, radius)
        };
        if !within(key, keep) && !holed {
            removed += *quads;
            for &e in entities.iter() {
                commands.entity(e).despawn();
            }
            return false;
        }
        if want != *shown {
            *shown = want;
            for &e in entities.iter() {
                if let Ok(mut v) = vis.get_mut(e) {
                    *v = if want {
                        Visibility::Inherited
                    } else {
                        Visibility::Hidden
                    };
                }
            }
        }
        true
    });
    if removed > 0 {
        s.quads -= removed;
        s.version += 1;
    }
}

// ---------------------------------------------------------------------------------------------
// Camera

/// Mouse look (right button held; captured for good in god mode, and connected while alive),
/// WASD/ZQSD + Space/Shift, wheel for speed. Connected, the keys move the player instead
/// ([`crate::player::walk`]) and the wheel sets its flight speed.
#[allow(clippy::too_many_arguments)]
pub fn fly_input(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    ui: Res<UiCapture>,
    terrain: Option<Res<SharedTerrain>>,
    deltas: Option<Res<SharedDeltas>>,
    remote: Option<Res<Remote>>,
    mut god: ResMut<crate::god::God>,
    mut cam: ResMut<FlyCam>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    chat: Res<crate::chat::ChatBox>,
    pause: Res<crate::pause::Pause>,
    life: Res<crate::player::Life>,
) {
    if cam.scripted {
        return;
    }
    // God mode captures the mouse for good (cross at the centre, right click places), and so
    // does playing connected; the in-game menu, the chat and death free it.
    let playing = remote.is_some() && life.playing() && !chat.open;
    let looking =
        !pause.open && (god.on || playing || (buttons.pressed(MouseButton::Right) && !ui.0));
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
    if !ui.0 && !god.on && !pause.open && scroll.delta.y != 0.0 {
        let notches = match scroll.unit {
            MouseScrollUnit::Line => f64::from(scroll.delta.y),
            MouseScrollUnit::Pixel => f64::from(scroll.delta.y) / 40.0,
        };
        cam.speed = (cam.speed * 1.25f64.powf(notches)).clamp(SPEED_MIN, SPEED_MAX);
    }

    if chat.open || pause.open || remote.is_some() {
        // The keyboard writes in the chat, or the menu is open; connected, it moves the player.
        return;
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
    // Floor: the generated ground, or the first solid block below the camera where blocks
    // were modified (dug pits, placed blocks). Walls and ceilings do not stop the camera.
    let (cx, cz) = (
        (p.x.floor() as i32).div_euclid(CHUNK_SIZE),
        (p.z.floor() as i32).div_euclid(CHUNK_SIZE),
    );
    // From the higher of the old and new heights: a fast fall cannot start inside a block.
    let from = p.y.max(cam.pos.y);
    let ground = match (&remote, &terrain, &deltas) {
        // Connected: the blocks the server sent (no floor where nothing came yet).
        (Some(r), _, _) => r.floor_below(p.x, from, p.z).unwrap_or(f64::NEG_INFINITY),
        (None, Some(terrain), Some(deltas)) => {
            let store = deltas.snapshot();
            if store.column_modified(cx, cz) {
                god.floor_below(&terrain.0, &store, p.x, from, p.z)
                    .unwrap_or(f64::NEG_INFINITY)
            } else {
                terrain.0.height(p.x, p.z)
            }
        }
        _ => f64::NEG_INFINITY,
    };
    p.y = p.y.max(ground.max(0.0) + GROUND_CLEARANCE);
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

// ---------------------------------------------------------------------------------------------
// HUD

#[derive(Resource)]
pub struct HudState {
    /// The debug panel is shown (F3).
    pub shown: bool,
    pub teleport: (String, String),
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            shown: false,
            teleport: ("50000".into(), "50000".into()),
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
    workers: Res<Workers>,
    horizon: Option<Res<crate::horizon::HorizonStats>>,
    stats: Res<FrameStats>,
    terrain: Option<Res<SharedTerrain>>,
    remote: Option<Res<Remote>>,
    info: Res<WorldInfo>,
    sky: Res<crate::sky::SkyState>,
    climate: Option<Res<crate::sky::ClimateMaps>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let p = cam.pos;
    let avg = stats.average();
    // Ground under the camera: the local terrain, or the blocks the server sent.
    let surface = match (&remote, &terrain) {
        (Some(r), _) => r
            .surface(p.x, p.z)
            .map(|(y, b)| (f64::from(y + 1), r.blocks.def(b).name.clone())),
        (None, Some(t)) => Some(local_surface(&t.0, p.x, p.z)),
        _ => None,
    };
    if ctx.input(|i| i.key_pressed(egui::Key::F3)) {
        hud.shown = !hud.shown;
    }
    let mut shown = hud.shown;
    egui::Window::new("Nascio — 3D (F3)")
        .default_pos([10.0, 10.0])
        .resizable(false)
        .open(&mut shown)
        .show(ctx, |ui| {
            ui.monospace(format!("x {:>9.1}  y {:>7.1}  z {:>9.1}", p.x, p.y, p.z));
            ui.monospace(format!(
                "φ {:+7.2}°  λ {:6.2}°  s {:6.0} m",
                latitude(p.z).to_degrees(),
                longitude(p.x).to_degrees(),
                seam_distance(p.z)
            ));
            match &surface {
                Some((h, block)) => {
                    ui.monospace(format!(
                        "ground {h:7.1} m   heading {:5.1}°",
                        cam.yaw.to_degrees()
                    ));
                    ui.monospace(format!("speed {:7.1} m/s (wheel)", cam.speed));
                    ui.monospace(format!("ground block {block}"));
                }
                None => {
                    ui.monospace(format!(
                        "ground   (not loaded)   heading {:5.1}°",
                        cam.yaw.to_degrees()
                    ));
                    ui.monospace(format!("speed {:7.1} m/s (wheel)", cam.speed));
                }
            }
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
                workers.threads
            ));
            if let Some(h) = &horizon {
                ui.monospace(format!(
                    "horizon {}/{} levels, {} k triangles, {:.1} / {:.0} km",
                    h.shown_levels,
                    h.levels,
                    h.triangles / 1_000,
                    h.range_m / 1_000.0,
                    h.extent_m / 1_000.0
                ));
            }
            ui.separator();
            let mut fixed = settings.fixed_hour.is_some();
            ui.horizontal(|ui| {
                ui.checkbox(&mut fixed, "fixed solar hour");
                let mut hour = settings.fixed_hour.unwrap_or(10.0);
                if fixed {
                    ui.add(egui::Slider::new(&mut hour, 0.0..=24.0));
                }
                settings.fixed_hour = fixed.then_some(hour);
            });
            let mut fixed_day = settings.fixed_day.is_some();
            ui.horizontal(|ui| {
                ui.checkbox(&mut fixed_day, "fixed day");
                let mut day = settings.fixed_day.unwrap_or(172.0);
                if fixed_day {
                    ui.add(egui::Slider::new(&mut day, 0.0..=364.0));
                }
                settings.fixed_day = fixed_day.then_some(day);
            });
            ui.checkbox(&mut settings.night_floor, "night floor");
            if let Some(p) = sky.sun {
                ui.monospace(format!(
                    "day {:6.2}  sun e {:+5.1}  e_vis {:+5.1} deg",
                    sky.day,
                    p.elevation.to_degrees(),
                    p.visible_elevation.to_degrees()
                ));
            }
            ui.monospace(format!(
                "clouds {:3.0} %  moon {:3.0} % lit",
                100.0 * sky.cloud,
                100.0 * sky.moon_lit
            ));
            // Local flight only: the server refuses a jump (speed check).
            ui.add_enabled_ui(terrain.is_some(), |ui| ui.horizontal(|ui| {
                ui.label("x");
                ui.add(egui::TextEdit::singleline(&mut hud.teleport.0).desired_width(60.0));
                ui.label("z");
                ui.add(egui::TextEdit::singleline(&mut hud.teleport.1).desired_width(60.0));
                if ui.button("Teleport").clicked()
                    && let (Ok(x), Ok(z)) = (
                        hud.teleport.0.trim().parse::<f64>(),
                        hud.teleport.1.trim().parse::<f64>(),
                    )
                    && let Some(t) = &terrain
                {
                    let ground = t.0.height(x, z).max(0.0);
                    cam.pos = WorldPos::new(x, ground + 60.0, z);
                }
            }));
            ui.small(if remote.is_some() {
                "ZQSD: walk · Z twice: run · Space: jump · Shift: crouch · Space twice: fly (administrators) · G: edit · F3: this panel · Esc: menu".to_owned()
            } else {
                format!(
                    "Right drag: look · WASD/ZQSD: move · Space/Shift: up/down · F3: this panel · Esc: menu{}",
                    info.seed.map_or(String::new(), |s| format!(" · seed {s}"))
                )
            });
        });
    hud.shown = shown;
    let ground = surface.as_ref().map_or(0.0, |s| s.0.max(0.0));
    let under = match (&remote, &terrain) {
        (Some(r), _) => remote_underground(r, p),
        (None, Some(t)) => underground(&t.0, &info, climate.as_deref(), p),
        _ => None,
    };
    clock(
        ctx,
        &info,
        &settings,
        &cam,
        climate.as_deref(),
        ground,
        under,
    );
    capture.0 = ctx.egui_wants_pointer_input() || ctx.is_pointer_over_egui();
    Ok(())
}

/// Applies the vsync and fullscreen settings to the window.
pub fn apply_window(
    settings: Res<FlySettings>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    let want = if settings.vsync {
        bevy::window::PresentMode::AutoVsync
    } else {
        bevy::window::PresentMode::AutoNoVsync
    };
    let mode = if settings.fullscreen {
        bevy::window::WindowMode::BorderlessFullscreen(bevy::window::MonitorSelection::Primary)
    } else {
        bevy::window::WindowMode::Windowed
    };
    for mut w in &mut windows {
        if w.present_mode != want {
            w.present_mode = want;
        }
        if w.mode != mode {
            w.mode = mode;
        }
    }
}

/// `hh:mm` of a day fraction.
fn hhmm(frac: f64) -> String {
    let minutes = (frac.rem_euclid(1.0) * 1_440.0).floor() as u32 % 1_440;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Ground height under `(x, z)` and its top block, from the local terrain.
fn local_surface(t: &Terrain, x: f64, z: f64) -> (f64, String) {
    let top = t.height(x, z).floor() as i32;
    let slope = (t.height(x + 2.0, z) - t.height(x - 2.0, z))
        .hypot(t.height(x, z + 2.0) - t.height(x, z - 2.0))
        / 4.0;
    let block = t.top_block(
        nascio_core::torus::wrap(x.floor() as i32),
        nascio_core::torus::wrap(z.floor() as i32),
        top,
        slope,
    );
    (t.height(x, z), t.blocks().def(block).name.clone())
}

/// Below the ground, connected: depth under the highest solid block and the block at the
/// camera, from the blocks the server sent (no geology: no rock temperature).
fn remote_underground(r: &Remote, p: WorldPos) -> Option<(f64, Option<f64>, String, bool)> {
    let (top, _) = r.surface(p.x, p.z)?;
    let top = f64::from(top + 1);
    if p.y >= top {
        return None;
    }
    let at = nascio_core::coords::BlockPos::new(
        p.x.floor() as i32,
        p.y.floor() as i32,
        p.z.floor() as i32,
    );
    let name = r
        .block(at)
        .map_or_else(|| "?".to_owned(), |b| r.blocks.def(b).name.clone());
    Some(((top - p.y).floor(), None, name, false))
}

/// Top-right clock: local solar time at the camera and time of the origin meridian (λ = 0,
/// the game's reference time, spec §2.3), with the game date. Follows the fixed hour and day.
/// Below the time: air temperature, relative humidity and pressure at the camera (§4.3).
/// Below the ground: `(depth m, rock temperature °C, block name, in magma)`.
fn underground(
    terrain: &Terrain,
    info: &WorldInfo,
    climate: Option<&crate::sky::ClimateMaps>,
    p: WorldPos,
) -> Option<(f64, Option<f64>, String, bool)> {
    let top = terrain.height(p.x, p.z).floor();
    if p.y >= top + 1.0 {
        return None;
    }
    let top_i = top as i32;
    let moho = terrain.moho_y(p.x, p.z, top_i);
    let depth = top - p.y.floor();
    let y = p.y.floor() as i32;
    // Mean annual surface temperature: the four quarter days cancel both harmonics.
    let air = climate
        .and_then(|c| {
            let v: Option<Vec<f64>> = [0.0, 91.25, 182.5, 273.75]
                .iter()
                .map(|&d| {
                    c.surface_at(p.x, p.z, d, top.max(0.0), info.lapse_rate_c_per_m)
                        .map(|s| s.temperature_c)
                })
                .collect();
            v.map(|v| v.iter().sum::<f64>() / 4.0)
        })
        .unwrap_or(10.0);
    let water = (terrain.water_level(p.x, p.z) - top).max(0.0);
    let ts = info.geotherm.surface_c(air, water);
    let magma = y < moho;
    let t = if magma {
        info.geotherm.moho_c
    } else {
        info.geotherm
            .temperature(ts, depth, f64::from((top_i - moho).max(1)))
    };
    let blocks = terrain.blocks();
    let (bx, bz) = (
        nascio_core::torus::wrap(p.x.floor() as i32),
        nascio_core::torus::wrap(p.z.floor() as i32),
    );
    let col = terrain.column_geo(p.x, p.z);
    let block = if magma {
        blocks.ids.magma
    } else {
        terrain
            .rock_block(&col, bx, bz, top_i, 0.0, moho, depth as i32)
            .0
    };
    Some((depth, Some(t), blocks.def(block).name.clone(), magma))
}

#[allow(clippy::too_many_arguments)]
fn clock(
    ctx: &egui::Context,
    info: &WorldInfo,
    settings: &FlySettings,
    cam: &FlyCam,
    climate: Option<&crate::sky::ClimateMaps>,
    ground: f64,
    under: Option<(f64, Option<f64>, String, bool)>,
) {
    let (day, t_days, h) = crate::sky::instant(info, settings, cam.pos.x);
    let local = (h + std::f64::consts::PI) / TAU;
    let origin = day.rem_euclid(1.0);
    let year = (t_days / 365.0).floor();
    let p = cam.pos;
    let air = climate
        .and_then(|c| c.surface_at(p.x, p.z, day, ground, info.lapse_rate_c_per_m))
        .map(|s| info.atmosphere.air(&s, p.y, local, info.lapse_rate_c_per_m));
    egui::Area::new(egui::Id::new("clock"))
        .anchor(egui::Align2::RIGHT_TOP, [-10.0, 10.0])
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.monospace(format!(
                    "local  {}  (λ {:5.1}°)",
                    hhmm(local),
                    longitude(cam.pos.x).to_degrees()
                ));
                ui.monospace(format!("origin {}  (λ   0.0°)", hhmm(origin)));
                ui.monospace(format!("year {year:.0} · day {:.0}", day.floor()));
                ui.separator();
                if let Some((depth, t, block, magma)) = &under {
                    ui.monospace(format!("underground {depth:6.0} m ({block})"));
                    if let Some(t) = t {
                        ui.monospace(format!("rock        {t:6.0} °C"));
                    }
                    if *magma {
                        ui.colored_label(egui::Color32::from_rgb(255, 90, 40), "MAGMA: DEADLY");
                    } else if t.is_some_and(|t| t > 60.0) {
                        ui.colored_label(egui::Color32::from_rgb(255, 160, 60), "hot rock");
                    }
                    return;
                }
                match air {
                    Some(a) => {
                        ui.monospace(format!("temperature {:6.1} °C", a.temperature_c));
                        ui.monospace(format!("humidity    {:6.0} %", a.humidity * 100.0));
                        ui.monospace(format!("pressure    {:6.1} hPa", a.pressure_hpa));
                    }
                    None => {
                        ui.monospace(format!(
                            "pressure    {:6.1} hPa",
                            info.atmosphere.pressure_hpa(p.y)
                        ));
                    }
                }
            });
        });
}

/// Entering the 3D mode: every column is re-evaluated by [`column_visibility`].
pub fn reset_visibility(mut streamer: ResMut<Streamer>) {
    for slot in streamer.slots.values_mut() {
        if let Slot::Ready { shown, .. } = slot {
            *shown = false;
        }
    }
}
