//! Horizon: nested height-mesh levels beyond the chunks (spec §7).
//!
//! Level `L` is a grid of `cells × cells` quads of `base_spacing · 2^L` metres. All levels share
//! one centre, snapped to twice the coarsest spacing, so each level's hole is exactly the square
//! covered by the level inside it, and all levels are rebuilt and swapped together. Level 0 also
//! has a hole for every chunk column that is meshed and within the view distance; the streamer
//! shows a column only once the displayed level 0 has a hole for it, and keeps it until the
//! horizon covers it again. Skirts hide the cracks between levels and along the chunk boundary.
//!
//! Heights come from the same [`Terrain`] as the chunks, keeping only the noise octaves that the
//! level's spacing can represent. Under water (sea or lake) the mesh is flattened to the water
//! surface and coloured by depth (opaque distant water).

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;
use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS, WORLD_SIZE_F64};
use nascio_core::math::smoothstep;
use nascio_core::torus::wrap_f64;
use nascio_voxel::Terrain;
use nascio_voxel::mesh::MeshData;

use crate::curvature::{CurvedMaterial, visible_range};
use crate::fly::{Anchor, FlyCam, FlySettings, SharedTerrain, Streamer, Workers, to_bevy_mesh};

/// Horizon layout.
#[derive(Clone, Debug)]
pub struct HorizonConfig {
    /// Vertex spacing of level 0, m (a divisor of the chunk size).
    pub base_spacing_m: f64,
    /// Quads per side of each level (even).
    pub cells: usize,
    pub levels: usize,
    /// Minimum delay between two rebuilds of level 0's hole, s.
    pub mask_interval_s: f64,
}

impl Default for HorizonConfig {
    fn default() -> Self {
        Self {
            base_spacing_m: 16.0,
            cells: 192,
            levels: 5,
            mask_interval_s: 0.1,
        }
    }
}

impl HorizonConfig {
    pub fn spacing(&self, level: usize) -> f64 {
        self.base_spacing_m * f64::from(1u32 << level)
    }

    pub fn half_extent(&self, level: usize) -> f64 {
        self.cells as f64 / 2.0 * self.spacing(level)
    }

    /// Distance covered around the centre.
    pub fn extent(&self) -> f64 {
        self.half_extent(self.levels - 1)
    }

    /// Grid the shared centre snaps to.
    fn snap(&self) -> f64 {
        2.0 * self.spacing(self.levels - 1)
    }

    /// Chunk columns along one side of level 0.
    fn l0_columns(&self) -> i32 {
        (2.0 * self.half_extent(0) / f64::from(CHUNK_SIZE)) as i32
    }
}

/// Figures shown in the HUD.
#[derive(Resource, Default, Clone, Debug)]
pub struct HorizonStats {
    pub levels: usize,
    pub triangles: usize,
    pub extent_m: f64,
    /// Levels drawn (the others lie beyond the curvature's visible range).
    pub shown_levels: usize,
    /// Visible range with the display curvature, capped by the extent, m.
    pub range_m: f64,
    /// Rebuilds waiting (0 when the displayed horizon is up to date).
    pub pending: usize,
}

/// Heights of a level's vertices, with a one-vertex margin for normals.
struct Heights {
    side: usize,
    data: Vec<f32>,
    /// Water surface per vertex (sea level or a lake's surface).
    water: Vec<f32>,
    /// Highest vertex, m.
    max: f32,
}

impl Heights {
    #[inline]
    fn at(&self, i: i64, j: i64) -> f32 {
        self.data[(i + 1) as usize + self.side * (j + 1) as usize]
    }

    #[inline]
    fn water(&self, i: i64, j: i64) -> f32 {
        self.water[(i + 1) as usize + self.side * (j + 1) as usize]
    }

    /// Drawn surface: the ground, or the water above it.
    #[inline]
    fn surf(&self, i: i64, j: i64) -> f32 {
        self.at(i, j).max(self.water(i, j))
    }
}

enum Kind {
    Full(u64),
    Mask,
}

struct LevelResult {
    kind: Kind,
    level: usize,
    center: (i32, i32),
    mesh: MeshData,
    heights: Arc<Heights>,
    hole: Arc<HashSet<(i32, i32)>>,
}

#[derive(Resource)]
pub struct Horizon {
    cfg: HorizonConfig,
    tx: Sender<LevelResult>,
    rx: Mutex<Receiver<LevelResult>>,
    entities: Vec<Option<Entity>>,
    triangles: Vec<usize>,
    /// Highest point of each displayed level, m.
    level_max: Vec<f32>,
    /// Displayed centre and level-0 hole.
    center: Option<(i32, i32)>,
    hole: Arc<HashSet<(i32, i32)>>,
    /// Centre being built, its generation and finished levels.
    target: Option<(i32, i32)>,
    generation: u64,
    pending: Vec<Option<LevelResult>>,
    l0_heights: Option<((i32, i32), Arc<Heights>)>,
    mask_in_flight: bool,
    mask_version: u64,
    last_mask: f64,
}

impl Horizon {
    pub fn new(cfg: HorizonConfig) -> Self {
        let (tx, rx) = channel();
        let levels = cfg.levels;
        Self {
            cfg,
            tx,
            rx: Mutex::new(rx),
            entities: vec![None; levels],
            triangles: vec![0; levels],
            level_max: vec![0.0; levels],
            center: None,
            hole: Arc::default(),
            target: None,
            generation: 0,
            pending: Vec::new(),
            l0_heights: None,
            mask_in_flight: false,
            mask_version: u64::MAX,
            last_mask: f64::NEG_INFINITY,
        }
    }

    /// Whether a horizon is on screen.
    pub fn is_displayed(&self) -> bool {
        self.center.is_some()
    }

    /// Whether the displayed horizon leaves chunk column `key` to the chunks.
    pub fn has_hole(&self, key: (i32, i32)) -> bool {
        self.hole.contains(&key)
    }

    /// Chunk columns of level 0 that are meshed and in range.
    fn compute_hole(&self, center: (i32, i32), streamer: &Streamer) -> HashSet<(i32, i32)> {
        let n = self.cfg.l0_columns();
        let first = (
            (f64::from(center.0) - self.cfg.half_extent(0)) as i32 / CHUNK_SIZE,
            (f64::from(center.1) - self.cfg.half_extent(0)) as i32 / CHUNK_SIZE,
        );
        let mut set = HashSet::new();
        for dz in 0..n {
            for dx in 0..n {
                let key = (
                    (first.0 + dx).rem_euclid(CHUNKS_PER_AXIS),
                    (first.1 + dz).rem_euclid(CHUNKS_PER_AXIS),
                );
                if streamer.wants_hole(key) {
                    set.insert(key);
                }
            }
        }
        set
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_job(
        &self,
        workers: &Workers,
        terrain: &Arc<Terrain>,
        kind: Kind,
        level: usize,
        center: (i32, i32),
        hole: Arc<HashSet<(i32, i32)>>,
        heights: Option<Arc<Heights>>,
    ) {
        let terrain = terrain.clone();
        let cfg = self.cfg.clone();
        let tx = self.tx.clone();
        workers.pool.spawn(move || {
            let heights =
                heights.unwrap_or_else(|| Arc::new(level_heights(&terrain, &cfg, level, center)));
            let mesh = build_level(&terrain, &cfg, level, center, &heights, &hole);
            let _ = tx.send(LevelResult {
                kind,
                level,
                center,
                mesh,
                heights,
                hole,
            });
        });
    }
}

/// Snapped centre for a camera position, wrapped into `[0, WORLD_SIZE)`.
fn snapped_center(cfg: &HorizonConfig, x: f64, z: f64) -> (i32, i32) {
    let snap = cfg.snap();
    let f = |v: f64| wrap_f64((v / snap).round() * snap) as i32;
    (f(x), f(z))
}

fn level_heights(
    terrain: &Terrain,
    cfg: &HorizonConfig,
    level: usize,
    center: (i32, i32),
) -> Heights {
    let n = cfg.cells as i64;
    let s = cfg.spacing(level);
    let half = cfg.half_extent(level);
    let side = cfg.cells + 3;
    let mut data = Vec::with_capacity(side * side);
    let mut water = Vec::with_capacity(side * side);
    for j in -1..=n + 1 {
        for i in -1..=n + 1 {
            let x = f64::from(center.0) - half + i as f64 * s;
            let z = f64::from(center.1) - half + j as f64 * s;
            // Block tops sit on average half a metre above the continuous surface.
            data.push((terrain.height_lod(x, z, 0.5 * s) + 0.5) as f32);
            water.push(terrain.water_top(x, z) as f32);
        }
    }
    let max = data
        .iter()
        .zip(&water)
        .map(|(d, w)| d.max(*w))
        .fold(f32::NEG_INFINITY, f32::max);
    Heights {
        side,
        data,
        water,
        max,
    }
}

/// Mesh of one level, vertices relative to the shared centre.
fn build_level(
    terrain: &Terrain,
    cfg: &HorizonConfig,
    level: usize,
    center: (i32, i32),
    h: &Heights,
    hole: &HashSet<(i32, i32)>,
) -> MeshData {
    let n = cfg.cells as i64;
    let s = cfg.spacing(level);
    let half = cfg.half_extent(level);
    let inner = half / 2.0;
    let emitted = |i: i64, j: i64| -> bool {
        if i < 0 || j < 0 || i >= n || j >= n {
            return false;
        }
        let (rx, rz) = (-half + i as f64 * s, -half + j as f64 * s);
        if level > 0 && rx >= -inner && rx + s <= inner && rz >= -inner && rz + s <= inner {
            return false;
        }
        if level == 0 {
            let cx = wrap_f64(f64::from(center.0) + rx + 0.5 * s);
            let cz = wrap_f64(f64::from(center.1) + rz + 0.5 * s);
            let key = (
                (cx / f64::from(CHUNK_SIZE)) as i32 % CHUNKS_PER_AXIS,
                (cz / f64::from(CHUNK_SIZE)) as i32 % CHUNKS_PER_AXIS,
            );
            return !hole.contains(&key);
        }
        true
    };

    let blocks = terrain.blocks();
    let water = blocks.linear(blocks.ids.water);
    let mut m = MeshData::default();
    let side = (n + 1) as usize;
    let mut normals = Vec::with_capacity(side * side);
    for j in 0..=n {
        for i in 0..=n {
            let raw = h.at(i, j);
            let dx = f64::from(h.surf(i + 1, j) - h.surf(i - 1, j)) / (2.0 * s);
            let dz = f64::from(h.surf(i, j + 1) - h.surf(i, j - 1)) / (2.0 * s);
            let len = (dx * dx + 1.0 + dz * dz).sqrt();
            let normal = [(-dx / len) as f32, (1.0 / len) as f32, (-dz / len) as f32];
            let wl = h.water(i, j);
            let color = if raw < wl {
                let k = 1.15 - 0.75 * smoothstep(0.0, 400.0, f64::from(wl - raw)) as f32;
                // Alpha < 1 tags water for the shader (the material is opaque).
                [water[0] * k, water[1] * k, water[2] * k, 0.5]
            } else {
                let rdx = f64::from(h.at(i + 1, j) - h.at(i - 1, j)) / (2.0 * s);
                let rdz = f64::from(h.at(i, j + 1) - h.at(i, j - 1)) / (2.0 * s);
                // Beaches are too thin to show at horizon scale.
                let top = (raw.floor() as i32).max(terrain.config().beach_high_m + 1);
                let wx = (f64::from(center.0) - half + i as f64 * s).floor() as i32;
                let wz = (f64::from(center.1) - half + j as f64 * s).floor() as i32;
                let block = terrain.top_block(
                    nascio_core::torus::wrap(wx),
                    nascio_core::torus::wrap(wz),
                    top,
                    rdx.hypot(rdz),
                );
                let c = blocks.linear(block);
                [c[0], c[1], c[2], 1.0]
            };
            m.positions.push([
                (-half + i as f64 * s) as f32,
                h.surf(i, j),
                (-half + j as f64 * s) as f32,
            ]);
            m.normals.push(normal);
            normals.push(normal);
            m.colors.push(color);
        }
    }
    let v = |i: i64, j: i64| (i + j * (n + 1)) as u32;
    let skirt = (4.0 * s + 20.0) as f32;
    for j in 0..n {
        for i in 0..n {
            if !emitted(i, j) {
                continue;
            }
            let (a, b, c, d) = (v(i, j), v(i + 1, j), v(i, j + 1), v(i + 1, j + 1));
            m.indices.extend_from_slice(&[a, c, b, b, c, d]);
            // Skirts along edges with no neighbour quad.
            for (ni, nj, e0, e1) in [
                (i - 1, j, a, c),
                (i + 1, j, b, d),
                (i, j - 1, a, b),
                (i, j + 1, c, d),
            ] {
                // Water is flat: no crack to hide, and a skirt would show through the
                // translucent chunk water next to the hole.
                let wet = |e: u32| m.colors[e as usize][3] < 1.0;
                if emitted(ni, nj) || (wet(e0) && wet(e1)) {
                    continue;
                }
                let base = m.positions.len() as u32;
                for e in [e0, e1] {
                    let p = m.positions[e as usize];
                    let col = m.colors[e as usize];
                    m.positions.push([p[0], p[1] - skirt, p[2]]);
                    m.normals.push(normals[e as usize]);
                    m.colors
                        .push([col[0] * 0.7, col[1] * 0.7, col[2] * 0.7, col[3]]);
                }
                m.indices
                    .extend_from_slice(&[e0, e1, base + 1, e0, base + 1, base]);
            }
        }
    }
    m
}

/// Material of the horizon: double-sided so skirts show from either side.
#[derive(Resource)]
pub struct HorizonMaterial(pub Handle<CurvedMaterial>);

/// Requests rebuilds, applies finished levels, publishes stats.
#[allow(clippy::too_many_arguments)]
pub fn update_horizon(
    mut commands: Commands,
    mut horizon: ResMut<Horizon>,
    mut stats: ResMut<HorizonStats>,
    streamer: Res<Streamer>,
    workers: Res<Workers>,
    terrain: Res<SharedTerrain>,
    settings: Res<FlySettings>,
    cam: Res<FlyCam>,
    origin: Res<crate::fly::RenderOrigin>,
    material: Res<HorizonMaterial>,
    time: Res<Time<Real>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut vis: Query<&mut Visibility>,
) {
    let h = &mut *horizon;
    let now = time.elapsed_secs_f64();

    // 1. Results.
    let results: Vec<LevelResult> = h.rx.lock().expect("channel").try_iter().collect();
    let mut apply = Vec::new();
    for r in results {
        match r.kind {
            Kind::Full(g) if g == h.generation && Some(r.center) == h.target => {
                let level = r.level;
                h.pending[level] = Some(r);
            }
            Kind::Mask => {
                h.mask_in_flight = false;
                if h.target.is_none() && Some(r.center) == h.center {
                    apply.push(r);
                }
            }
            Kind::Full(_) => {}
        }
    }
    if h.target.is_some() && h.pending.iter().all(Option::is_some) {
        apply.extend(h.pending.drain(..).flatten());
        h.center = h.target.take();
    }
    for r in apply {
        if r.level == 0 {
            h.hole = r.hole.clone();
            h.l0_heights = Some((r.center, r.heights.clone()));
        }
        h.triangles[r.level] = r.mesh.indices.len() / 3;
        h.level_max[r.level] = r.heights.max;
        let anchor = Anchor {
            x: r.center.0,
            y: 0,
            z: r.center.1,
        };
        let bundle = (
            Mesh3d(meshes.add(to_bevy_mesh(r.mesh))),
            anchor,
            Transform::from_translation(origin.translation(anchor.x, 0, anchor.z)),
        );
        match h.entities[r.level] {
            Some(e) => {
                commands.entity(e).insert(bundle);
            }
            None => {
                let e = commands
                    .spawn((
                        bundle,
                        MeshMaterial3d(material.0.clone()),
                        Visibility::Inherited,
                        // Lowered by the display curvature: the flat bounds are wrong, and the
                        // levels surround the camera anyway.
                        NoFrustumCulling,
                        // Far beyond the shadow cascades; the chunks cast the near shadows.
                        bevy::light::NotShadowCaster,
                    ))
                    .id();
                h.entities[r.level] = Some(e);
            }
        }
    }

    // 2. Visibility: with the display curvature, a level whose nearest point lies beyond the
    // visible range of the highest displayed point is entirely below the horizon.
    let range = match settings.curvature_radius_m {
        Some(r) => {
            let top = h.level_max.iter().copied().fold(0.0, f32::max);
            visible_range(r, cam.pos.y, f64::from(top))
        }
        None => f64::INFINITY,
    };
    let mut shown_levels = 0;
    for (level, e) in h.entities.iter().enumerate() {
        let near = if level == 0 {
            0.0
        } else {
            // The camera is at most half a snap step from the centre (per axis).
            h.cfg.half_extent(level - 1) - h.cfg.snap()
        };
        let show = settings.horizon && near <= range;
        let target_vis = if show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if let Some(e) = e {
            shown_levels += usize::from(show);
            if let Ok(mut v) = vis.get_mut(*e)
                && *v != target_vis
            {
                *v = target_vis;
            }
        }
    }

    // 3. Requests.
    if settings.horizon {
        let desired = snapped_center(&h.cfg, cam.pos.x, cam.pos.z);
        let building = h.target.or(h.center);
        if building != Some(desired) {
            h.generation += 1;
            h.target = Some(desired);
            h.pending = (0..h.cfg.levels).map(|_| None).collect();
            let hole = Arc::new(h.compute_hole(desired, &streamer));
            h.mask_version = streamer.version;
            h.last_mask = now;
            for level in 0..h.cfg.levels {
                h.spawn_job(
                    &workers,
                    &terrain.0,
                    Kind::Full(h.generation),
                    level,
                    desired,
                    hole.clone(),
                    None,
                );
            }
        } else if h.target.is_none()
            && !h.mask_in_flight
            && streamer.version != h.mask_version
            && now - h.last_mask >= h.cfg.mask_interval_s
            && let (Some(center), Some((hc, heights))) = (h.center, h.l0_heights.clone())
            && hc == center
        {
            let hole = h.compute_hole(center, &streamer);
            h.mask_version = streamer.version;
            h.last_mask = now;
            if *h.hole != hole {
                h.mask_in_flight = true;
                h.spawn_job(
                    &workers,
                    &terrain.0,
                    Kind::Mask,
                    0,
                    center,
                    Arc::new(hole),
                    Some(heights),
                );
            }
        }
    }

    stats.levels = if h.center.is_some() { h.cfg.levels } else { 0 };
    stats.triangles = h.triangles.iter().sum();
    stats.extent_m = if settings.horizon && h.center.is_some() {
        h.cfg.extent()
    } else {
        0.0
    };
    stats.shown_levels = if h.center.is_some() { shown_levels } else { 0 };
    stats.range_m = range.min(stats.extent_m);
    stats.pending = usize::from(h.target.is_some())
        + usize::from(h.mask_in_flight)
        + usize::from(settings.horizon && streamer.version != h.mask_version);
}

/// The level-0 hole must contain every chunk the view distance can show.
pub fn check_layout(cfg: &HorizonConfig) {
    let reach = cfg.half_extent(0) - cfg.snap() / 2.0 - f64::from(CHUNK_SIZE);
    assert!(
        reach >= f64::from(crate::fly::VIEW_MAX),
        "horizon level 0 ({reach} m) must cover the maximum view distance"
    );
    assert!(
        2.0 * cfg.extent() < WORLD_SIZE_F64,
        "horizon wraps around the torus"
    );
}
