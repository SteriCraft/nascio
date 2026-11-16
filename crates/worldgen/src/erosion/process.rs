//! Erosion processes on the fine grid (spec §4.2): fluvial incision (stream power), nonlinear
//! hillslope diffusion and coastal sediment fans.
//!
//! Each step routes the flow on the current surface (priority-flood with epsilon, D8,
//! discharge), then:
//! - **stream power** `∂h/∂t = −K · A^m · S` (n = 1), solved implicitly in stack order (Braun &
//!   Willett 2013): unconditionally stable. The sea is a base level at 0 m; lakes (cells raised
//!   by the flood) are not eroded and pass their receiver's level on as the lake surface;
//! - **hillslope diffusion** with the nonlinear flux of Roering et al. (1999),
//!   `q = −D·∇h / (1 − (|∇h|/Sc)²)` (factor capped), explicit with stable sub-steps. Each cell
//!   gathers its exchanges with its four neighbours, so mass is conserved and the parallel pass
//!   is deterministic. Towards the sea the neighbour height is sea level, and the material
//!   leaving the land joins the sediment of that sea cell;
//! - **coastal sediments**: the volume eroded upstream of each river mouth (and shed by the
//!   coast) builds a fan under the sea, never above `deposit_top_m`, with a slope of
//!   `deposit_slope` away from the mouth.
//!
//! The sea never changes extent: land is never lowered below sea level and fans stay below it.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rayon::prelude::*;

use super::ErosionConfig;
use super::flow::{self, Shape};

/// Totals over the whole run, m³.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ErosionStats {
    pub steps: u32,
    /// Added to the land by tectonic uplift.
    pub uplift_m3: f64,
    /// Largest total uplift of a cell, m.
    pub max_uplift_m: f64,
    /// Removed from the land by rivers.
    pub fluvial_m3: f64,
    /// Moved by hillslope diffusion from the land into the sea.
    pub coastal_m3: f64,
    /// Deposited under the sea.
    pub deposited_m3: f64,
    /// Sediment that found no room (a sea filled up to `deposit_top_m`).
    pub lost_m3: f64,
    /// Diffusion sub-steps per step.
    pub diffusion_substeps: u32,
}

/// Runs the erosion steps on `h` (fine grid, m), with `rain` the rain volume per cell (m³/yr)
/// and `uplift` the tectonic uplift rate per cell (m/yr, applied to the land only).
pub fn erode(
    h: &mut [f64],
    rain: &[f64],
    uplift: &[f64],
    shape: Shape,
    sea: &[bool],
    cfg: &ErosionConfig,
) -> ErosionStats {
    let steps = cfg.steps();
    let mut stats = ErosionStats {
        steps,
        ..ErosionStats::default()
    };
    let area = shape.cell * shape.cell;
    // Rain normalised by 1 m/yr: an area in m².
    let substeps = diffusion_substeps(shape.cell, cfg);
    stats.diffusion_substeps = substeps;
    let mut sediment = vec![0.0f64; shape.n()];
    for _ in 0..steps {
        // Tectonic uplift of the land (the sea keeps its extent).
        for k in 0..shape.n() {
            if !sea[k] && uplift[k] > 0.0 {
                h[k] += uplift[k] * cfg.dt_yr;
                stats.uplift_m3 += uplift[k] * cfg.dt_yr * area;
            }
        }
        let filled = flow::priority_flood(h, sea, shape, true);
        let (rec, dir) = flow::receivers(&filled, sea, shape);
        let order = flow::stack(&rec);
        let discharge = flow::accumulate(&order, &rec, rain);
        // Stream power, receivers first.
        let mut eroded = vec![0.0f64; shape.n()];
        for &k in &order {
            let k = k as usize;
            let r = rec[k] as usize;
            if r == k || filled[k] > h[k] {
                continue;
            }
            let hr = if sea[r] {
                0.0
            } else if filled[r] > h[r] {
                filled[r]
            } else {
                h[r]
            };
            if hr >= h[k] {
                continue;
            }
            let f = cfg.dt_yr * cfg.k_fluvial * discharge[k].powf(cfg.m_exponent)
                / shape.step(dir[k] as usize);
            let new = (h[k] + f * hr) / (1.0 + f);
            eroded[k] = (h[k] - new) * area;
            h[k] = new;
        }
        // Sediment flux down to the mouths (lakes pass it on).
        let flux = flow::accumulate(&order, &rec, &eroded);
        for k in 0..shape.n() {
            if sea[k] && rec[k] as usize == k {
                sediment[k] += flux[k];
            }
        }
        stats.fluvial_m3 += eroded.iter().sum::<f64>();
        // Hillslope diffusion.
        let dt = cfg.dt_yr / f64::from(substeps);
        for _ in 0..substeps {
            let shed = diffuse(h, sea, shape, cfg, dt);
            for (s, d) in sediment.iter_mut().zip(&shed) {
                *s += d;
            }
            stats.coastal_m3 += shed.iter().sum::<f64>();
        }
        let delivered: f64 = sediment.iter().sum();
        let deposited = deposit(h, sea, shape, cfg, &mut sediment);
        stats.deposited_m3 += deposited;
        stats.lost_m3 += delivered - deposited;
    }
    stats.max_uplift_m =
        uplift.iter().fold(0.0f64, |m, &u| m.max(u)) * cfg.dt_yr * f64::from(steps);
    stats
}

/// Stable number of explicit diffusion sub-steps per erosion step.
fn diffusion_substeps(cell: f64, cfg: &ErosionConfig) -> u32 {
    let d = cfg.hillslope_diffusivity * cfg.max_diffusion_factor;
    if d <= 0.0 {
        return 0;
    }
    let dt_max = 0.9 * cell * cell / (4.0 * d);
    (cfg.dt_yr / dt_max).ceil().max(1.0) as u32
}

/// Roering factor `1 / (1 − (S/Sc)²)`, capped.
#[inline]
fn roering(s: f64, cfg: &ErosionConfig) -> f64 {
    let x = s / cfg.critical_slope;
    let denom = 1.0 - x * x;
    if denom <= 1.0 / cfg.max_diffusion_factor {
        cfg.max_diffusion_factor
    } else {
        1.0 / denom
    }
}

/// One explicit diffusion sub-step on the land; returns the volume each sea cell receives
/// from the land (m³).
fn diffuse(h: &mut [f64], sea: &[bool], shape: Shape, cfg: &ErosionConfig, dt: f64) -> Vec<f64> {
    let res = shape.res;
    let dx = shape.cell;
    let k = cfg.hillslope_diffusivity * dt / (dx * dx);
    // Exchange (m) of cell `a` with neighbour `b`: positive when `a` gains.
    let exchange = |ha: f64, hb: f64| {
        let s = (hb - ha).abs() / dx;
        k * roering(s, cfg) * (hb - ha)
    };
    let cur = h.to_vec();
    let at = |i: usize, j: usize, di: isize, dj: isize| {
        let ii = (i as isize + di).rem_euclid(res as isize) as usize;
        let jj = (j as isize + dj).rem_euclid(res as isize) as usize;
        jj * res + ii
    };
    const N4: [(isize, isize); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    let mut shed = vec![0.0f64; shape.n()];
    h.par_chunks_mut(res)
        .zip(shed.par_chunks_mut(res))
        .enumerate()
        .for_each(|(j, (row, shed_row))| {
            for i in 0..res {
                let a = j * res + i;
                if sea[a] {
                    // Gains from the land around it, at sea level.
                    let mut gain = 0.0;
                    for (di, dj) in N4 {
                        let b = at(i, j, di, dj);
                        if !sea[b] && cur[b] > 0.0 {
                            gain += -exchange(cur[b], 0.0);
                        }
                    }
                    shed_row[i] = gain.max(0.0) * dx * dx;
                    continue;
                }
                let mut dh = 0.0;
                for (di, dj) in N4 {
                    let b = at(i, j, di, dj);
                    if sea[b] {
                        if cur[a] > 0.0 {
                            dh += exchange(cur[a], 0.0).min(0.0);
                        }
                    } else {
                        dh += exchange(cur[a], cur[b]);
                    }
                }
                row[i] = cur[a] + dh;
            }
        });
    shed
}

/// Builds the sediment fans: every sea cell holding sediment (in index order) spreads it over
/// the sea around it, raising cells up to `deposit_top − deposit_slope · distance`, nearest
/// first. Sediment the cone cannot hold is then spread flat up to `deposit_top` (a shallow sea
/// silting up). Returns the deposited volume (m³); `sediment` is emptied, and what even a flat
/// fill cannot hold (a full sea) is lost.
fn deposit(
    h: &mut [f64],
    sea: &[bool],
    shape: Shape,
    cfg: &ErosionConfig,
    sediment: &mut [f64],
) -> f64 {
    let mut fans = Fans {
        stamp: vec![u32::MAX; shape.n()],
        dist: vec![0.0; shape.n()],
        heap: BinaryHeap::new(),
        pass: 0,
    };
    let mut total = 0.0;
    for (mouth, s) in sediment.iter_mut().enumerate() {
        if *s <= 0.0 {
            continue;
        }
        let volume = std::mem::take(s);
        let left = fans.spread(
            h,
            sea,
            shape,
            mouth,
            volume,
            cfg.deposit_top_m,
            cfg.deposit_slope,
        );
        let left = if left > 0.0 {
            fans.spread(h, sea, shape, mouth, left, cfg.deposit_top_m, 0.0)
        } else {
            left
        };
        total += volume - left;
    }
    total
}

/// Scratch space of [`deposit`]: Dijkstra over the sea, reset by stamping.
struct Fans {
    stamp: Vec<u32>,
    dist: Vec<f64>,
    heap: BinaryHeap<Reverse<(u64, u32)>>,
    pass: u32,
}

impl Fans {
    /// Spreads `volume` from `mouth` under the profile `top − slope · distance`; returns what
    /// is left.
    #[allow(clippy::too_many_arguments)]
    fn spread(
        &mut self,
        h: &mut [f64],
        sea: &[bool],
        shape: Shape,
        mouth: usize,
        mut volume: f64,
        top: f64,
        slope: f64,
    ) -> f64 {
        let area = shape.cell * shape.cell;
        let id = self.pass;
        self.pass = self.pass.wrapping_add(1);
        self.heap.clear();
        self.stamp[mouth] = id;
        self.dist[mouth] = 0.0;
        self.heap.push(Reverse((0u64, mouth as u32)));
        while let Some(Reverse((bits, c))) = self.heap.pop() {
            let c = c as usize;
            let d = f64::from_bits(bits);
            if d > self.dist[c] {
                continue;
            }
            let target = top - slope * d;
            if h[c] < target {
                let add = (target - h[c]).min(volume / area);
                h[c] += add;
                volume -= add * area;
                if volume <= 0.0 {
                    return 0.0;
                }
            }
            for dd in 0..8 {
                let m = shape.neighbour(c, dd);
                if !sea[m] {
                    continue;
                }
                let nd = d + shape.step(dd);
                if self.stamp[m] != id || nd < self.dist[m] {
                    self.stamp[m] = id;
                    self.dist[m] = nd;
                    self.heap.push(Reverse((nd.to_bits(), m as u32)));
                }
            }
        }
        volume
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(res: usize) -> Shape {
        Shape {
            res,
            cell: 100_000.0 / res as f64,
        }
    }

    #[test]
    fn diffusion_conserves_mass_and_flattens() {
        let s = shape(32);
        let cfg = ErosionConfig {
            hillslope_diffusivity: 1_000.0,
            ..ErosionConfig::default()
        };
        // All land, a periodic bump crossing the seam.
        let mut h: Vec<f64> = (0..s.n())
            .map(|k| {
                let i = (k % 32) as f64;
                100.0 + 900.0 * (std::f64::consts::TAU * i / 32.0).cos().max(0.0)
            })
            .collect();
        let sea = vec![false; s.n()];
        let before: f64 = h.iter().sum();
        let range = |h: &[f64]| {
            h.iter().cloned().fold(f64::MIN, f64::max) - h.iter().cloned().fold(f64::MAX, f64::min)
        };
        let r0 = range(&h);
        let n = diffusion_substeps(s.cell, &cfg);
        for _ in 0..20 * n {
            let shed = diffuse(&mut h, &sea, s, &cfg, cfg.dt_yr / f64::from(n));
            assert!(shed.iter().all(|&v| v == 0.0));
        }
        let after: f64 = h.iter().sum();
        assert!(
            (after - before).abs() < 1e-6 * before,
            "{before} -> {after}"
        );
        assert!(range(&h) < 0.5 * r0);
    }

    #[test]
    fn coast_sheds_into_the_sea_with_mass_balance() {
        let s = shape(32);
        let cfg = ErosionConfig {
            hillslope_diffusivity: 50.0,
            ..ErosionConfig::default()
        };
        let mut h: Vec<f64> = (0..s.n())
            .map(|k| if k % 32 < 16 { 300.0 } else { -500.0 })
            .collect();
        let sea: Vec<bool> = h.iter().map(|&v| v < 0.0).collect();
        let land0: f64 = (0..s.n()).filter(|&k| !sea[k]).map(|k| h[k]).sum();
        let shed = diffuse(&mut h, &sea, s, &cfg, 1_000.0);
        let land1: f64 = (0..s.n()).filter(|&k| !sea[k]).map(|k| h[k]).sum();
        let lost = (land0 - land1) * s.cell * s.cell;
        let got: f64 = shed.iter().sum();
        assert!(
            lost > 0.0 && (lost - got).abs() < 1e-6 * lost,
            "{lost} vs {got}"
        );
        // The sea itself is unchanged by diffusion.
        assert!((0..s.n()).filter(|&k| sea[k]).all(|k| h[k] == -500.0));
    }

    #[test]
    fn fans_conserve_volume_and_stay_below_the_top() {
        let s = shape(64);
        let cfg = ErosionConfig::default();
        let mut h: Vec<f64> = (0..s.n())
            .map(|k| if k % 64 < 10 { 50.0 } else { -40.0 })
            .collect();
        let sea: Vec<bool> = h.iter().map(|&v| v < 0.0).collect();
        let before: f64 = (0..s.n()).filter(|&k| sea[k]).map(|k| h[k]).sum();
        let mut sed = vec![0.0; s.n()];
        let mouth = 30 * 64 + 10;
        let v = 2.0e9;
        sed[mouth] = v;
        let got = deposit(&mut h, &sea, s, &cfg, &mut sed);
        assert!((got - v).abs() < 1e-6 * v, "{got} vs {v}");
        assert!(sed.iter().all(|&x| x == 0.0));
        let after: f64 = (0..s.n()).filter(|&k| sea[k]).map(|k| h[k]).sum();
        assert!(((after - before) * s.cell * s.cell - v).abs() < 1.0);
        assert!(
            h.iter()
                .zip(&sea)
                .all(|(&x, &is_sea)| !is_sea || x <= cfg.deposit_top_m + 1e-9)
        );
        assert_eq!(h[mouth], cfg.deposit_top_m);
        // A small fan slopes away from the mouth.
        let mut h2: Vec<f64> = (0..s.n())
            .map(|k| if k % 64 < 10 { 50.0 } else { -400.0 })
            .collect();
        sed[mouth] = 1.0e9;
        assert!((deposit(&mut h2, &sea, s, &cfg, &mut sed) - 1.0e9).abs() < 1.0);
        assert!(h2[30 * 64 + 12] < h2[mouth] && h2[mouth] == cfg.deposit_top_m);
    }

    #[test]
    fn stream_power_carves_towards_the_sea_and_never_below_it() {
        // A tilted plane towards a sea column: incision is fastest downstream.
        let s = shape(64);
        let cfg = ErosionConfig {
            duration_yr: 2.0e5,
            k_fluvial: 1e-5,
            hillslope_diffusivity: 0.0,
            ..ErosionConfig::default()
        };
        let mut h: Vec<f64> = (0..s.n())
            .map(|k| {
                let i = k % 64;
                if i == 0 {
                    -100.0
                } else {
                    20.0 * i as f64 + 5.0 * ((k / 64) % 7) as f64
                }
            })
            .collect();
        let h0 = h.clone();
        let sea: Vec<bool> = h.iter().map(|&v| v < 0.0).collect();
        let rain = vec![s.cell * s.cell; s.n()];
        let stats = erode(&mut h, &rain, &vec![0.0; s.n()], s, &sea, &cfg);
        assert!(stats.fluvial_m3 > 0.0);
        assert!(h.iter().zip(&sea).all(|(&v, &is_sea)| is_sea || v >= 0.0));
        assert!(h.iter().zip(&h0).all(|(a, b)| a <= b || *b < 0.0));
    }
}
