//! Reproducible benchmark: a scripted camera path, one CSV line per frame and a summary.
//!
//! ```text
//! nascio-viewer out/m2/world-42 --bench bench/default.toml --bench-out out/bench.csv
//! ```
//!
//! The path is a list of waypoints (`x`, `z`, height above ground `agl` or absolute `y`, leg
//! `speed` in m/s, optional `yaw`/`pitch` in degrees and `shot` file). Consecutive waypoints are
//! joined along the shortest way on the torus, so a path can cross the seams. The camera first
//! waits at the first waypoint until everything in range is loaded, then follows the path in
//! real time while every frame is recorded.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;
use bevy::render::view::window::screenshot::{Screenshot, save_to_disk};
use bevy::window::PrimaryWindow;
use nascio_core::coords::WorldPos;
use nascio_core::torus::delta_f64;
use serde::Deserialize;

use crate::fly::{FlyCam, SharedTerrain, Streamer};
use crate::horizon::HorizonStats;
use crate::perf::FrameStats;

/// A benchmark file.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchFile {
    /// Chunk view distance, m (default: the maximum).
    pub view_distance_m: Option<f32>,
    /// Fixed local solar hour (default 10).
    pub hour: Option<f64>,
    /// Frames everything must stay loaded before the run starts.
    #[serde(default = "default_settle")]
    pub settle_frames: u32,
    /// Start anyway after this long, s.
    #[serde(default = "default_max_load")]
    pub max_load_s: f64,
    pub waypoint: Vec<Waypoint>,
}

fn default_settle() -> u32 {
    30
}

fn default_max_load() -> f64 {
    120.0
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waypoint {
    pub x: f64,
    pub z: f64,
    /// Height above the ground (or above sea level over water).
    pub agl: Option<f64>,
    /// Absolute height (used when `agl` is absent).
    pub y: Option<f64>,
    /// Speed of the leg reaching this waypoint, m/s.
    pub speed: Option<f64>,
    /// Heading, degrees from north towards east (default: along the leg).
    pub yaw: Option<f64>,
    /// Pitch, degrees (default −10).
    pub pitch: Option<f64>,
    /// Screenshot taken when the waypoint is reached.
    pub shot: Option<String>,
}

impl BenchFile {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let f: Self = toml::from_str(&std::fs::read_to_string(path)?)?;
        anyhow::ensure!(f.waypoint.len() >= 2, "a path needs at least two waypoints");
        Ok(f)
    }
}

/// Unwrapped path with leg timings.
struct Path {
    /// Unwrapped `(x, z)` of each waypoint.
    points: Vec<(f64, f64)>,
    /// Time at which each waypoint is reached.
    times: Vec<f64>,
}

impl Path {
    fn new(w: &[Waypoint]) -> Self {
        let mut points = vec![(w[0].x, w[0].z)];
        let mut times = vec![0.0];
        for k in 1..w.len() {
            let (px, pz) = points[k - 1];
            let dx = delta_f64(w[k - 1].x, w[k].x);
            let dz = delta_f64(w[k - 1].z, w[k].z);
            points.push((px + dx, pz + dz));
            let speed = w[k].speed.unwrap_or(40.0).max(0.1);
            times.push(times[k - 1] + dx.hypot(dz) / speed);
        }
        Self { points, times }
    }

    fn duration(&self) -> f64 {
        *self.times.last().expect("non-empty")
    }

    /// Leg index and progress at time `t`.
    fn locate(&self, t: f64) -> (usize, f64) {
        let k = self
            .times
            .partition_point(|&s| s <= t)
            .clamp(1, self.times.len() - 1);
        let span = (self.times[k] - self.times[k - 1]).max(1e-9);
        (k, ((t - self.times[k - 1]) / span).clamp(0.0, 1.0))
    }
}

#[derive(Clone, Copy, Debug)]
struct Row {
    t: f64,
    frame_ms: f64,
    cpu_ms: f64,
    render_cpu_ms: f64,
    gpu_ms: Option<f64>,
    pos: WorldPos,
    ready: usize,
    missing: usize,
    quads: usize,
    horizon_tris: usize,
}

enum Phase {
    Loading { settled: u32, waited: f64 },
    Running { t: f64 },
    Done { frames: u32 },
}

#[derive(Resource)]
pub struct Bench {
    file: BenchFile,
    path: Path,
    out: PathBuf,
    phase: Phase,
    rows: Vec<Row>,
    shots: BTreeSet<usize>,
}

impl Bench {
    pub fn new(file: BenchFile, out: PathBuf) -> Self {
        let path = Path::new(&file.waypoint);
        Self {
            file,
            path,
            out,
            phase: Phase::Loading {
                settled: 0,
                waited: 0.0,
            },
            rows: Vec::new(),
            shots: BTreeSet::new(),
        }
    }

    /// Camera at time `t` along the path.
    fn camera_at(&self, t: f64, terrain: &nascio_voxel::Terrain, cam: &mut FlyCam) {
        let w = &self.file.waypoint;
        let (k, s) = self.path.locate(t);
        let (a, b) = (self.path.points[k - 1], self.path.points[k]);
        let x = a.0 + (b.0 - a.0) * s;
        let z = a.1 + (b.1 - a.1) * s;
        let ground = terrain.height(x, z).max(0.0);
        let height = |wp: &Waypoint| match (wp.agl, wp.y) {
            (Some(agl), _) => (true, agl),
            (None, Some(y)) => (false, y),
            (None, None) => (true, 60.0),
        };
        let (ha, hb) = (height(&w[k - 1]), height(&w[k]));
        let lerp = |p: f64, q: f64| p + (q - p) * s;
        let y = match (ha, hb) {
            ((true, p), (true, q)) => ground + lerp(p, q),
            ((false, p), (false, q)) => lerp(p, q),
            ((_, p), (_, q)) => lerp(p, q).max(ground + 2.0),
        };
        let heading = (b.0 - a.0).atan2(-(b.1 - a.1));
        let yaw = match (w[k - 1].yaw, w[k].yaw) {
            (Some(p), Some(q)) => lerp(p, q).to_radians(),
            (_, Some(q)) => q.to_radians(),
            _ => heading,
        };
        let pitch = lerp(w[k - 1].pitch.unwrap_or(-10.0), w[k].pitch.unwrap_or(-10.0));
        cam.pos = WorldPos::new(x, y.max(ground + 2.0), z);
        cam.yaw = yaw;
        cam.pitch = pitch.to_radians();
        cam.speed = w[k].speed.unwrap_or(40.0);
        cam.scripted = true;
    }
}

/// Moves the camera along the path (before streaming).
pub fn drive(
    mut bench: ResMut<Bench>,
    mut cam: ResMut<FlyCam>,
    terrain: Res<SharedTerrain>,
    streamer: Res<Streamer>,
    horizon: Res<HorizonStats>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs_f64();
    let b = &mut *bench;
    match &mut b.phase {
        Phase::Loading { settled, waited } => {
            *waited += dt;
            let loaded = streamer.missing == 0 && horizon.pending == 0;
            *settled = if loaded { *settled + 1 } else { 0 };
            if *settled >= b.file.settle_frames || *waited >= b.file.max_load_s {
                info!(
                    "bench: loaded in {:.1} s, running {:.1} s of path",
                    waited,
                    b.path.duration()
                );
                b.phase = Phase::Running { t: 0.0 };
            }
            b.camera_at(0.0, &terrain.0, &mut cam);
        }
        Phase::Running { t } => {
            *t += dt;
            let t = *t;
            b.camera_at(t.min(b.path.duration()), &terrain.0, &mut cam);
            for (k, &tk) in b.path.times.iter().enumerate() {
                if tk <= t
                    && b.shots.insert(k)
                    && let Some(shot) = &b.file.waypoint[k].shot
                {
                    commands
                        .spawn(Screenshot::primary_window())
                        .observe(save_to_disk(PathBuf::from(shot)));
                }
            }
        }
        Phase::Done { .. } => {}
    }
}

/// Records the frame (after the frame stats) and finishes the run.
#[allow(clippy::too_many_arguments)]
pub fn record(
    mut bench: ResMut<Bench>,
    stats: Res<FrameStats>,
    cam: Res<FlyCam>,
    streamer: Res<Streamer>,
    horizon: Res<HorizonStats>,
    window: Query<&Window, With<PrimaryWindow>>,
    adapter: Option<Res<RenderAdapterInfo>>,
    mut exit: MessageWriter<AppExit>,
) {
    let b = &mut *bench;
    match &mut b.phase {
        Phase::Running { t } => {
            let s = stats.last;
            let t = *t;
            b.rows.push(Row {
                t,
                frame_ms: s.frame_ms,
                cpu_ms: s.cpu_ms,
                render_cpu_ms: s.render_cpu_ms,
                gpu_ms: s.gpu_ms,
                pos: cam.pos,
                ready: streamer.ready_columns,
                missing: streamer.missing,
                quads: streamer.quads,
                horizon_tris: horizon.triangles,
            });
            if t >= b.path.duration() {
                let res = window
                    .single()
                    .map(|w| (w.physical_width(), w.physical_height()))
                    .unwrap_or((0, 0));
                let gpu_name = adapter.map_or_else(
                    || "unknown adapter".to_string(),
                    |a| format!("{} ({:?}, {})", a.name, a.backend, a.driver_info),
                );
                match write_results(&b.out, &b.rows, res, &gpu_name) {
                    Ok(summary) => println!("{summary}"),
                    Err(e) => eprintln!("bench: cannot write results: {e:#}"),
                }
                b.phase = Phase::Done { frames: 0 };
            }
        }
        Phase::Done { frames } => {
            // Leave time for pending screenshots to be written.
            *frames += 1;
            if *frames > 10 {
                exit.write(AppExit::Success);
            }
        }
        Phase::Loading { .. } => {}
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let k = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[k]
}

fn write_results(
    out: &std::path::Path,
    rows: &[Row],
    res: (u32, u32),
    gpu_name: &str,
) -> anyhow::Result<String> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut csv = String::from(
        "frame,t_s,frame_ms,cpu_ms,render_cpu_ms,gpu_ms,x,y,z,columns_ready,columns_missing,chunk_quads,horizon_triangles\n",
    );
    for (i, r) in rows.iter().enumerate() {
        let gpu = r.gpu_ms.map_or(String::new(), |g| format!("{g:.3}"));
        writeln!(
            csv,
            "{i},{:.4},{:.3},{:.3},{:.3},{gpu},{:.1},{:.1},{:.1},{},{},{},{}",
            r.t,
            r.frame_ms,
            r.cpu_ms,
            r.render_cpu_ms,
            r.pos.x,
            r.pos.y,
            r.pos.z,
            r.ready,
            r.missing,
            r.quads,
            r.horizon_tris
        )?;
    }
    std::fs::write(out, csv)?;

    let sorted = |f: &dyn Fn(&Row) -> Option<f64>| {
        let mut v: Vec<f64> = rows.iter().filter_map(f).collect();
        v.sort_by(f64::total_cmp);
        v
    };
    let frame = sorted(&|r| Some(r.frame_ms));
    let cpu = sorted(&|r| Some(r.cpu_ms));
    let gpu = sorted(&|r| r.gpu_ms);
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let over = frame.iter().filter(|&&f| f > 1_000.0 / 60.0).count();
    let worst1: Vec<f64> = frame[frame.len() - (frame.len() / 100).max(1)..].to_vec();
    let mut s = String::new();
    writeln!(s, "Nascio benchmark — {}", out.display())?;
    writeln!(s, "adapter: {gpu_name}")?;
    writeln!(s, "resolution: {}x{}", res.0, res.1)?;
    writeln!(
        s,
        "frames: {}  duration: {:.1} s",
        rows.len(),
        rows.last().map_or(0.0, |r| r.t)
    )?;
    writeln!(
        s,
        "frame ms: mean {:.2}  p50 {:.2}  p95 {:.2}  p99 {:.2}  max {:.2}",
        mean(&frame),
        percentile(&frame, 0.5),
        percentile(&frame, 0.95),
        percentile(&frame, 0.99),
        frame.last().copied().unwrap_or(f64::NAN)
    )?;
    writeln!(
        s,
        "fps: mean {:.1}  1% low {:.1}  frames over 16.7 ms: {} ({:.2} %)",
        1_000.0 / mean(&frame),
        1_000.0 / mean(&worst1),
        over,
        100.0 * over as f64 / frame.len().max(1) as f64
    )?;
    writeln!(
        s,
        "cpu ms: mean {:.2}  p95 {:.2}  p99 {:.2}",
        mean(&cpu),
        percentile(&cpu, 0.95),
        percentile(&cpu, 0.99)
    )?;
    if gpu.is_empty() {
        writeln!(s, "gpu ms: n/a (no timestamp queries on this adapter)")?;
    } else {
        writeln!(
            s,
            "gpu ms (timed passes): mean {:.2}  p95 {:.2}  p99 {:.2}",
            mean(&gpu),
            percentile(&gpu, 0.95),
            percentile(&gpu, 0.99)
        )?;
    }
    let verdict = if percentile(&frame, 0.95) <= 1_000.0 / 60.0 {
        "PASS: p95 frame time within 16.7 ms (60 i/s)"
    } else {
        "FAIL: p95 frame time above 16.7 ms (60 i/s)"
    };
    writeln!(s, "{verdict}")?;
    std::fs::write(out.with_extension("summary.txt"), &s)?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wp(x: f64, z: f64, speed: f64) -> Waypoint {
        Waypoint {
            x,
            z,
            agl: Some(50.0),
            y: None,
            speed: Some(speed),
            yaw: None,
            pitch: None,
            shot: None,
        }
    }

    #[test]
    fn path_crosses_the_seam_the_short_way() {
        let p = Path::new(&[wp(99_900.0, 10.0, 10.0), wp(100.0, 10.0, 10.0)]);
        assert!((p.points[1].0 - 100_100.0).abs() < 1e-9);
        assert!((p.duration() - 20.0).abs() < 1e-9);
        assert_eq!(p.locate(10.0), (1, 0.5));
    }

    #[test]
    fn default_file_parses() {
        let f: BenchFile = toml::from_str(include_str!("../../../bench/default.toml"))
            .expect("bench/default.toml");
        assert!(f.waypoint.len() >= 2);
    }
}
