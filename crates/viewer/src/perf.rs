//! Per-frame CPU and GPU timings (on-screen counter and benchmark).
//!
//! - `frame_ms`: wall-clock time between frames (what the player feels).
//! - `cpu_ms`: time spent in the main schedule (simulation, streaming, UI), measured from the
//!   first to the last system set of the frame.
//! - `render_cpu_ms`: CPU time encoding render passes, from Bevy's render diagnostics.
//! - `gpu_ms`: GPU time of the frame's passes, from timestamp queries (Vulkan and DX12 only);
//!   `None` when the adapter cannot time.
//!
//! Render diagnostics come back a few frames late: they are attributed to the frame where they
//! arrive, which is fine for averages and percentiles.

use std::time::Instant;

use bevy::diagnostic::{DiagnosticPath, DiagnosticsStore};
use bevy::prelude::*;

/// Number of frames averaged for the on-screen counter.
const DISPLAY_WINDOW: usize = 60;

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameSample {
    pub frame_ms: f64,
    pub cpu_ms: f64,
    pub render_cpu_ms: f64,
    pub gpu_ms: Option<f64>,
}

#[derive(Resource, Default)]
pub struct FrameStats {
    pub last: FrameSample,
    history: Vec<FrameSample>,
    begin: Option<Instant>,
    frames: u64,
}

impl FrameStats {
    /// Mean of the last frames, for display.
    pub fn average(&self) -> FrameSample {
        let n = self.history.len().max(1) as f64;
        let gpu: Vec<f64> = self.history.iter().filter_map(|s| s.gpu_ms).collect();
        FrameSample {
            frame_ms: self.history.iter().map(|s| s.frame_ms).sum::<f64>() / n,
            cpu_ms: self.history.iter().map(|s| s.cpu_ms).sum::<f64>() / n,
            render_cpu_ms: self.history.iter().map(|s| s.render_cpu_ms).sum::<f64>() / n,
            gpu_ms: (!gpu.is_empty()).then(|| gpu.iter().sum::<f64>() / gpu.len() as f64),
        }
    }
}

pub struct PerfPlugin;

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameStats>()
            .add_plugins((
                bevy::diagnostic::FrameTimeDiagnosticsPlugin::default(),
                bevy::render::diagnostic::RenderDiagnosticsPlugin,
            ))
            .add_systems(First, begin_frame)
            .add_systems(Last, end_frame);
    }
}

fn begin_frame(mut stats: ResMut<FrameStats>) {
    stats.begin = Some(Instant::now());
}

fn end_frame(mut stats: ResMut<FrameStats>, time: Res<Time<Real>>, store: Res<DiagnosticsStore>) {
    let cpu_ms = stats
        .begin
        .map_or(0.0, |b| b.elapsed().as_secs_f64() * 1_000.0);
    let sample = FrameSample {
        frame_ms: time.delta_secs_f64() * 1_000.0,
        cpu_ms,
        render_cpu_ms: top_level_sum(&store, "elapsed_cpu").unwrap_or(0.0),
        gpu_ms: top_level_sum(&store, "elapsed_gpu"),
    };
    stats.frames += 1;
    if std::env::var_os("NASCIO_DUMP_DIAG").is_some() && stats.frames.is_multiple_of(300) {
        for d in store.iter() {
            if let Some(m) = d.measurement() {
                eprintln!("diag {} = {:.3}", d.path(), m.value);
            }
        }
    }
    stats.last = sample;
    stats.history.push(sample);
    if stats.history.len() > DISPLAY_WINDOW {
        stats.history.remove(0);
    }
}

/// Sum of the outermost render spans ending in `field` (nested spans are already included in
/// their parent).
fn top_level_sum(store: &DiagnosticsStore, field: &str) -> Option<f64> {
    let spans: Vec<(Vec<&str>, f64)> = store
        .iter()
        .filter_map(|d| {
            let path: &DiagnosticPath = d.path();
            let parts: Vec<&str> = path.components().collect();
            (parts.first() == Some(&"render") && parts.last() == Some(&field))
                .then(|| {
                    d.measurement()
                        .map(|m| (parts[1..parts.len() - 1].to_vec(), m.value))
                })
                .flatten()
        })
        .collect();
    if spans.is_empty() {
        return None;
    }
    let total = spans
        .iter()
        .filter(|(p, _)| {
            !spans
                .iter()
                .any(|(q, _)| q.len() < p.len() && p.starts_with(q.as_slice()))
        })
        .map(|(_, v)| v)
        .sum();
    Some(total)
}
