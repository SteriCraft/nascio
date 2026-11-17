//! Detail noise shared by the fine grid and the chunks, and the ruggedness that scales it.

use nascio_core::math::smoothstep;

use super::ErosionConfig;
use crate::grid::PeriodicGrid;
use crate::noise::{PeriodicNoise, divisor_at_most};
use crate::rng::{mix64, stage_u64};
use crate::tectonics::flags;

/// Periodic detail noise shared by the fine grid and the chunks.
#[derive(Clone, Debug)]
pub struct Detail {
    cfg: ErosionConfig,
    /// `(noise, amplitude)` from the coarsest octave.
    octaves: Vec<(PeriodicNoise, f64)>,
    norm: f64,
}

impl Detail {
    pub fn new(seed: u64, cfg: &ErosionConfig) -> Self {
        let base = stage_u64(seed, "erosion/detail");
        let mut octaves = Vec::new();
        let mut spacing = divisor_at_most(cfg.detail_base_spacing_m);
        let mut amp = 1.0;
        while f64::from(spacing) >= cfg.detail_finest_m {
            let k = octaves.len() as u64;
            octaves.push((PeriodicNoise::new(mix64(base ^ k), spacing), amp));
            let next = divisor_at_most(f64::from(spacing) / 2.0);
            if next == spacing {
                break;
            }
            spacing = next;
            amp *= cfg.detail_gain;
        }
        let norm = octaves.iter().map(|(_, a)| a).sum::<f64>().max(1e-9);
        Self {
            cfg: cfg.clone(),
            octaves,
            norm,
        }
    }

    /// Spacings of the octaves, coarsest first.
    pub fn spacings(&self) -> impl Iterator<Item = f64> + '_ {
        self.octaves.iter().map(|(n, _)| n.spacing())
    }

    /// Detail at `(x, z)` from the octaves whose spacing lies in `[min_spacing, max_spacing)`,
    /// for a base elevation `e` (sets land, coast or sea floor) and a ruggedness `r ∈ [0, 1]`.
    pub fn value(&self, x: f64, z: f64, e: f64, r: f64, min_spacing: f64, max_spacing: f64) -> f64 {
        let c = &self.cfg;
        let r = r.clamp(0.0, 1.0);
        let land = c.detail_plain_m + (c.detail_mountain_m - c.detail_plain_m) * r;
        let w = smoothstep(-c.coast_band_m, c.coast_band_m, e);
        let damp =
            c.coast_damping + (1.0 - c.coast_damping) * smoothstep(0.0, c.coast_band_m, e.abs());
        let amplitude = (c.detail_seabed_m + (land - c.detail_seabed_m) * w) * damp;
        let (mut smooth, mut ridged) = (0.0, 0.0);
        for (n, a) in &self.octaves {
            let s = n.spacing();
            if s >= max_spacing {
                continue;
            }
            if s < min_spacing {
                break;
            }
            let v = n.sample(x, z);
            smooth += a * v;
            ridged += a * (1.0 - 2.0 * v.abs());
        }
        let k = c.ridged_share * r * w;
        amplitude * ((1.0 - k) * smooth + k * ridged) / self.norm
    }
}

/// Smallest octave spacing baked into a fine grid of cell `cell_m`; finer octaves are left to
/// the chunks.
pub fn baked_min_spacing(cell_m: f64) -> f64 {
    2.0 * cell_m
}

/// Ruggedness per tectonic cell, 0 (flat) to 1 (most rugged): elevation, slope and orogeny,
/// then smoothed. `history` returns the `history_flags` bits of a cell.
pub fn ruggedness(
    elevation: &PeriodicGrid<f32>,
    history: impl Fn(i64, i64) -> u8,
    c: &ErosionConfig,
) -> PeriodicGrid<f32> {
    let (w, h) = (elevation.width(), elevation.height());
    let cell = elevation.cell_size();
    let raw = PeriodicGrid::from_fn(w, h, cell, |i, j| {
        let (i, j) = (i as i64, j as i64);
        let e = f64::from(*elevation.get(i, j));
        if e <= 0.0 {
            return 0.0f32;
        }
        let mut grade = 0.0f64;
        for (di, dj) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            grade = grade.max((f64::from(*elevation.get(i + di, j + dj)) - e).abs() / cell);
        }
        let mut r = smoothstep(150.0, c.mountain_elevation_m, e).max(smoothstep(
            0.0,
            c.mountain_slope,
            grade,
        ));
        if history(i, j) & flags::OROGENY != 0 {
            r = r.max(c.orogeny_ruggedness);
        }
        r as f32
    });
    box3(&box3(&raw))
}

/// Periodic 3×3 mean.
fn box3(g: &PeriodicGrid<f32>) -> PeriodicGrid<f32> {
    PeriodicGrid::from_fn(g.width(), g.height(), g.cell_size(), |i, j| {
        let (i, j) = (i as i64, j as i64);
        let mut s = 0.0;
        for dj in -1..=1 {
            for di in -1..=1 {
                s += g.get(i + di, j + dj);
            }
        }
        s / 9.0
    })
}
