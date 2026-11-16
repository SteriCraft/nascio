//! Tectonic uplift during erosion (spec §4.2, M3 review).
//!
//! The uplift rate comes from the present-day plate velocities of the tectonic stage: along
//! every convergent boundary (closing speed above the transform threshold), the continental
//! side(s) rise at `uplift_max_m_yr × closing / fastest plate speed`. The profile is smeared over
//! the orogen width by the same periodic box blur as the collision orogens, then rescaled so that
//! a straight boundary keeps its peak rate (independent of the resolution). Welded plates share
//! one velocity; their suture keeps rising at `suture_uplift_share` of the rate given by their
//! initial (driving) velocities, like the convergence that goes on after a continental
//! collision (India and Asia).
//!
//! The plates of M1 move 40 km in 150 Ma (0.27 mm/yr at most), a hundred times slower than on
//! Earth, while erosion runs on Earth rates: a physical shortening rate would barely register
//! (2 m in 1 Ma). The rate is therefore normalised by the fastest plate and calibrated so that
//! uplift roughly balances erosion in active ranges.

use crate::config::WorldConfig;
use crate::grid::PeriodicGrid;
use crate::tectonics::{CONTINENTAL, TectonicOutput, box_blur};

/// Uplift rate on the tectonic grid, m/yr.
pub fn uplift_rate(cfg: &WorldConfig, tect: &TectonicOutput) -> PeriodicGrid<f32> {
    let g = &tect.plate_id;
    let (w, h) = (g.width(), g.height());
    let cell = g.cell_size();
    let t = &cfg.tectonics;
    let steps = tect.report.steps.max(1);
    let max_speed = t.total_displacement_m / f64::from(steps);
    let thr = t.boundary_threshold * max_speed;
    let crust = tect.crust_type.data();
    let mut seed = vec![0f32; w * h];
    if cfg.erosion.uplift_max_m_yr <= 0.0 || max_speed <= 0.0 {
        return PeriodicGrid::from_vec(w, h, cell, seed);
    }
    for idx in 0..w * h {
        let (i, j) = g.coords(idx);
        let a = g.data()[idx];
        for (di, dj) in [(1i64, 0i64), (0, 1)] {
            let bidx = g.index(i as i64 + di, j as i64 + dj);
            let b = g.data()[bidx];
            if a == b {
                continue;
            }
            let (pa, pb) = (&tect.plates[a as usize], &tect.plates[b as usize]);
            // Speed of a towards b along the normal a → b, m per step.
            let normal = |va: (f64, f64), vb: (f64, f64)| {
                (va.0 - vb.0) * di as f64 + (va.1 - vb.1) * dj as f64
            };
            // Welded plates share exactly the same velocity.
            let welded = pa.velocity == pb.velocity;
            let closing = if !welded {
                normal(pa.velocity, pb.velocity)
            } else if crust[idx] == CONTINENTAL && crust[bidx] == CONTINENTAL {
                cfg.erosion.suture_uplift_share * normal(pa.drive, pb.drive)
            } else {
                0.0
            };
            if closing <= thr {
                continue;
            }
            let rate = (cfg.erosion.uplift_max_m_yr * (closing / max_speed).min(1.0)) as f32;
            for c in [idx, bidx] {
                if crust[c] == CONTINENTAL {
                    seed[c] = seed[c].max(rate);
                }
            }
        }
    }
    let oro_w = t.orogeny_width_m.max(t.min_feature_cells * cell);
    let radius = ((oro_w / (2.0 * cell)).round() as usize).max(1);
    for _ in 0..3 {
        box_blur(&mut seed, w, h, radius);
    }
    let peak = line_peak(radius) as f32;
    for v in &mut seed {
        *v /= peak;
    }
    PeriodicGrid::from_vec(w, h, cell, seed)
}

/// Peak of a one-cell-wide straight line after three box blurs of radius `r` (1-D kernel at 0).
fn line_peak(r: usize) -> f64 {
    let n = 2 * r + 1;
    let mut k = vec![1.0];
    for _ in 0..3 {
        let mut next = vec![0.0; k.len() + n - 1];
        for (i, a) in k.iter().enumerate() {
            for o in &mut next[i..i + n] {
                *o += a / n as f64;
            }
        }
        k = next;
    }
    k[k.len() / 2]
}

/// Uplift rate per fine cell (m/yr), bilinear from the tectonic grid.
pub fn on_fine(rate: &PeriodicGrid<f32>, res: usize, cell: f64) -> Vec<f64> {
    let mut v = vec![0.0; res * res];
    for (k, o) in v.iter_mut().enumerate() {
        let (i, j) = (k % res, k / res);
        let (x, z) = ((i as f64 + 0.5) * cell, (j as f64 + 0.5) * cell);
        *o = rate.sample_bilinear(x, z).max(0.0);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_peak_matches_the_blur() {
        let (w, h, r) = (40, 40, 3);
        let mut v = vec![0f32; w * h];
        for j in 0..h {
            v[j * w + 5] = 1.0; // a straight line, near the seam
        }
        for _ in 0..3 {
            box_blur(&mut v, w, h, r);
        }
        let peak = v[7 * w + 5];
        assert!((f64::from(peak) - line_peak(r)).abs() < 1e-5);
    }
}
