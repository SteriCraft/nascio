//! Tectonic elevation → fine grid (spec §4.2): continuous rift grabens, periodic bicubic
//! interpolation and periodic detail noise.
//!
//! The detail noise is a fractal sum whose octaves run from `detail_base_spacing_m` down to
//! `detail_finest_m`. The fine grid bakes the octaves at least two cells wide (so erosion can
//! carve them); chunk generation adds the finer ones with the same amplitudes ([`Detail`]).

use nascio_core::math::smoothstep;
use nascio_core::torus::delta_f64;
use rayon::prelude::*;

use super::ErosionConfig;
use crate::config::WorldConfig;
use crate::grid::PeriodicGrid;
use crate::noise::{PeriodicNoise, divisor_at_most};
use crate::rng::{mix64, stage_u64};
use crate::tectonics::{CONTINENTAL, TectonicOutput, flags};

pub use nascio_world::erosion::upsample::{Detail, baked_min_spacing, ruggedness};

/// Rift axis on the tectonic grid. Continental rifts leave inland water holes along a plate
/// boundary (spec §4.1), often fossil: the plates may have been welded since, so present-day
/// velocities do not show them. The axis follows the plate boundary (landward cells next to
/// another plate, orogens excluded) up to `link_m / 2` away from a hole, which joins holes
/// up to `link_m` apart. The result is closed by a 3×3 square so diagonal steps join.
pub fn rift_mask(tect: &TectonicOutput, link_m: f64) -> PeriodicGrid<bool> {
    let g = &tect.plate_id;
    let (w, h, c) = (g.width(), g.height(), g.cell_size());
    let n = w * h;
    let water = |k: usize| tect.inland_water.data()[k] != 0;
    let landward = |k: usize| tect.crust_type.data()[k] == CONTINENTAL || water(k);
    let boundary: Vec<bool> = (0..n)
        .map(|k| {
            let (i, j) = g.coords(k);
            landward(k)
                && tect.history_flags.data()[k] & flags::OROGENY == 0
                && [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .any(|&(di, dj)| {
                        let m = g.index(i as i64 + di, j as i64 + dj);
                        g.data()[m] != g.data()[k] && landward(m)
                    })
        })
        .collect();
    // Breadth-first search from the holes along boundary cells (8-connected), in index order.
    let max_steps = ((0.5 * link_m / c).round() as u32).max(1);
    let mut dist = vec![u32::MAX; n];
    let mut queue = std::collections::VecDeque::new();
    for k in (0..n).filter(|&k| water(k)) {
        dist[k] = 0;
        queue.push_back(k);
    }
    while let Some(k) = queue.pop_front() {
        if dist[k] >= max_steps {
            continue;
        }
        let (i, j) = g.coords(k);
        for dj in -1..=1i64 {
            for di in -1..=1i64 {
                let m = g.index(i as i64 + di, j as i64 + dj);
                if dist[m] == u32::MAX && boundary[m] {
                    dist[m] = dist[k] + 1;
                    queue.push_back(m);
                }
            }
        }
    }
    let base = PeriodicGrid::from_vec(w, h, c, dist.iter().map(|&d| d != u32::MAX).collect());
    close3(&base)
}

/// Morphological closing by a 3×3 square (periodic). Never removes a cell of the input.
fn close3(base: &PeriodicGrid<bool>) -> PeriodicGrid<bool> {
    let (w, h, c) = (base.width(), base.height(), base.cell_size());
    let morph = |g: &PeriodicGrid<bool>, any: bool| {
        PeriodicGrid::from_fn(w, h, c, |i, j| {
            let (i, j) = (i as i64, j as i64);
            let mut hits = (-1..=1).flat_map(|dj| (-1..=1).map(move |di| (di, dj)));
            if any {
                hits.any(|(di, dj)| *g.get(i + di, j + dj))
            } else {
                hits.all(|(di, dj)| *g.get(i + di, j + dj))
            }
        })
    };
    let closed = morph(&morph(base, true), false);
    PeriodicGrid::from_fn(w, h, c, |i, j| {
        *closed.get(i as i64, j as i64) || *base.get(i as i64, j as i64)
    })
}

/// Replaces masked cells by the mean of their unmasked (or already filled) 8-neighbours, ring
/// by ring from the edge of the mask. Deterministic (Jacobi rings).
fn inpaint(values: &PeriodicGrid<f32>, mask: &PeriodicGrid<bool>) -> PeriodicGrid<f32> {
    let mut v = values.clone();
    let mut known: Vec<bool> = mask.data().iter().map(|m| !m).collect();
    if known.iter().all(|&k| !k) {
        return v;
    }
    loop {
        let mut updates = Vec::new();
        for idx in 0..known.len() {
            if known[idx] {
                continue;
            }
            let (i, j) = v.coords(idx);
            let (mut s, mut n) = (0.0f64, 0u32);
            for dj in -1..=1i64 {
                for di in -1..=1i64 {
                    let k = v.index(i as i64 + di, j as i64 + dj);
                    if known[k] {
                        s += f64::from(v.data()[k]);
                        n += 1;
                    }
                }
            }
            if n > 0 {
                updates.push((idx, (s / f64::from(n)) as f32));
            }
        }
        if updates.is_empty() {
            return v;
        }
        for (idx, val) in updates {
            v.data_mut()[idx] = val;
            known[idx] = true;
        }
    }
}

/// Graben depth (m, ≥ 0) at each fine cell: `rift_depth · (1 − smoothstep(0, rift_width, d))`
/// with `d` the distance from the fine cell centre to the nearest rift cell (a square of the
/// tectonic grid), measured on the torus.
fn graben(mask: &PeriodicGrid<bool>, fine_res: usize, cfg: &ErosionConfig) -> Vec<f32> {
    let tc = mask.cell_size();
    let fc = tc * mask.width() as f64 / fine_res as f64;
    let reach = (cfg.rift_width_m / tc).ceil() as i64 + 1;
    let cells: Vec<usize> = (0..mask.data().len()).filter(|&k| mask.data()[k]).collect();
    let mut out = vec![0f32; fine_res * fine_res];
    if cells.is_empty() || cfg.rift_depth_m <= 0.0 {
        return out;
    }
    // Tectonic cells within `reach` of the rift: only their fine cells need the distance.
    let mut near = vec![false; mask.data().len()];
    for &k in &cells {
        let (i, j) = mask.coords(k);
        for dj in -reach..=reach {
            for di in -reach..=reach {
                near[mask.index(i as i64 + di, j as i64 + dj)] = true;
            }
        }
    }
    let ratio = fine_res / mask.width();
    out.par_chunks_mut(fine_res)
        .enumerate()
        .for_each(|(fj, row)| {
            let z = (fj as f64 + 0.5) * fc;
            let tj = (fj / ratio) as i64;
            for (fi, o) in row.iter_mut().enumerate() {
                let ti = (fi / ratio) as i64;
                if !near[mask.index(ti, tj)] {
                    continue;
                }
                let x = (fi as f64 + 0.5) * fc;
                let mut d2 = f64::INFINITY;
                for dj in -reach..=reach {
                    for di in -reach..=reach {
                        if !*mask.get(ti + di, tj + dj) {
                            continue;
                        }
                        let cx = ((ti + di) as f64 + 0.5) * tc;
                        let cz = ((tj + dj) as f64 + 0.5) * tc;
                        let ex = (delta_f64(cx, x).abs() - 0.5 * tc).max(0.0);
                        let ez = (delta_f64(cz, z).abs() - 0.5 * tc).max(0.0);
                        d2 = d2.min(ex * ex + ez * ez);
                    }
                }
                let d = d2.sqrt();
                *o = (cfg.rift_depth_m * (1.0 - smoothstep(0.0, cfg.rift_width_m, d))) as f32;
            }
        });
    out
}

/// Periodic domain warp of the tectonic sampling position (M4): the tectonic elevation is
/// calibrated cell by cell, so its coastlines and steps follow the 160 m lattice and a plain
/// bicubic keeps them as stairs. Shifting the sampling point by two periodic noises of about
/// one cell turns the stairs into natural, wiggly outlines.
struct Warp {
    nx: [PeriodicNoise; 2],
    nz: [PeriodicNoise; 2],
    amplitude: f64,
}

impl Warp {
    fn new(seed: u64, tectonic_cell: f64, cells: f64) -> Self {
        let base = stage_u64(seed, "erosion/coast-warp");
        let coarse = divisor_at_most(2.5 * tectonic_cell);
        let fine = divisor_at_most(1.0 * tectonic_cell);
        Self {
            nx: [
                PeriodicNoise::new(mix64(base), coarse),
                PeriodicNoise::new(mix64(base ^ 1), fine),
            ],
            nz: [
                PeriodicNoise::new(mix64(base ^ 2), coarse),
                PeriodicNoise::new(mix64(base ^ 3), fine),
            ],
            amplitude: cells * tectonic_cell,
        }
    }

    fn apply(&self, x: f64, z: f64) -> (f64, f64) {
        if self.amplitude <= 0.0 {
            return (x, z);
        }
        let n = |p: &[PeriodicNoise; 2]| p[0].sample(x, z) + 0.5 * p[1].sample(x, z);
        (
            x + self.amplitude * n(&self.nx),
            z + self.amplitude * n(&self.nz),
        )
    }
}

/// Fine-grid elevation before erosion, in metres (f64 for the erosion arithmetic).
pub fn upsample(cfg: &WorldConfig, tect: &TectonicOutput) -> PeriodicGrid<f64> {
    let e = &cfg.erosion;
    let res = cfg.scale.fine_res();
    let fc = cfg.scale.fine_cell_m();
    let mask = rift_mask(tect, e.rift_link_m);
    let base = inpaint(&tect.elevation, &mask);
    let rug = ruggedness(&tect.elevation, |i, j| *tect.history_flags.get(i, j), e);
    let graben = graben(&mask, res, e);
    let detail = Detail::new(cfg.seed, e);
    let min_spacing = baked_min_spacing(fc);
    let warp = Warp::new(cfg.seed, base.cell_size(), e.coast_warp_cells);
    let mut data = vec![0f64; res * res];
    data.par_chunks_mut(res).enumerate().for_each(|(j, row)| {
        let z = (j as f64 + 0.5) * fc;
        for (i, o) in row.iter_mut().enumerate() {
            let x = (i as f64 + 0.5) * fc;
            let (wx, wz) = warp.apply(x, z);
            let b = base.sample_bicubic(wx, wz);
            let r = rug.sample_bilinear(x, z);
            let d = detail.value(x, z, b, r, min_spacing, f64::INFINITY);
            *o = b + d - f64::from(graben[j * res + i]);
        }
    });
    PeriodicGrid::from_vec(res, res, fc, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_fills_one_cell_gaps_across_the_seam() {
        let n = 25;
        // Every other cell of row 5 (n is odd: cells 24 and 0 touch across the seam).
        let g = PeriodicGrid::from_fn(n, n, 4_000.0, |i, j| j == 5 && i % 2 == 0);
        let m = close3(&g);
        for k in 0..n as i64 {
            assert!(*m.get(k, 5), "gap at {k}");
        }
        assert!(!*m.get(3, 4) && !*m.get(3, 6));
    }

    #[test]
    fn inpaint_fills_holes_from_their_rim() {
        let n = 25;
        let v = PeriodicGrid::from_fn(n, n, 4_000.0, |i, _| if i == 5 { -80.0 } else { 300.0 });
        let m = PeriodicGrid::from_fn(n, n, 4_000.0, |i, _| i == 5);
        let out = inpaint(&v, &m);
        assert!(out.data().iter().all(|&x| x == 300.0));
    }

    #[test]
    fn graben_is_deepest_on_the_axis_and_wraps() {
        let n = 25;
        // Rift along column 0: it must deepen both sides of the x seam.
        let m = PeriodicGrid::from_fn(n, n, 4_000.0, |i, _| i == 0);
        let cfg = ErosionConfig {
            rift_width_m: 6_000.0,
            ..ErosionConfig::default()
        };
        let res = 125;
        let g = graben(&m, res, &cfg);
        let at = |i: usize| g[7 * res + i];
        assert_eq!(at(2), 250.0);
        assert!(at(res - 1) > 200.0 && (at(res - 1) - at(5)).abs() < 1e-3);
        assert!(at(5) > at(8) && at(8) > 0.0);
        assert_eq!(at(60), 0.0);
    }

    #[test]
    fn detail_octave_split_adds_up() {
        let cfg = ErosionConfig::default();
        let d = Detail::new(3, &cfg);
        for k in 0..50 {
            let (x, z) = (f64::from(k) * 731.3, f64::from(k) * 211.7);
            let all = d.value(x, z, 500.0, 0.7, 0.0, f64::INFINITY);
            let coarse = d.value(x, z, 500.0, 0.7, 64.0, f64::INFINITY);
            let fine = d.value(x, z, 500.0, 0.7, 0.0, 64.0);
            assert!((all - coarse - fine).abs() < 1e-9);
        }
    }
}
