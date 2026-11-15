//! Final pass: classify the present-day plate boundaries, build trenches and arcs, smooth
//! orogens, and derive elevation, crust thickness and Moho depth.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use nascio_core::math::smoothstep;

use super::{CONTINENTAL, EventCounts, Hotspot, OCEANIC, Plate, Tectonics, flags};
use crate::grid::PeriodicGrid;
use crate::noise::Fbm;
use crate::rng::stage_u64;

/// Profile level above which a cell is flagged as trench or arc.
const FLAG_LEVEL: f64 = 0.6;
use crate::world_io::Layer;

/// Summary of a run.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub steps: u32,
    pub dt_ma: f64,
    pub plates: usize,
    pub hotspots: usize,
    pub initial_continental_fraction: f64,
    pub final_continental_fraction: f64,
    /// Final continental share is within `continental_fraction ± tolerance`.
    pub continental_fraction_ok: bool,
    /// Tectonic runs used to calibrate the initial continental share.
    pub calibration_runs: u32,
    pub events: EventCounts,
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "steps: {} (dt = {:.2} Ma), plates: {}, hotspots: {}",
            self.steps, self.dt_ma, self.plates, self.hotspots
        )?;
        writeln!(
            f,
            "continental fraction: {:.3} -> {:.3} ({}, {} calibration runs)",
            self.initial_continental_fraction,
            self.final_continental_fraction,
            if self.continental_fraction_ok {
                "ok"
            } else {
                "OUT OF TOLERANCE"
            },
            self.calibration_runs
        )?;
        let e = &self.events;
        write!(
            f,
            "events: ridge cells {}, rift cells {}, ocean-continent subductions {}, \
             ocean-ocean subductions {}, collisions {}, welds {}",
            e.ridge_cells,
            e.rift_cells,
            e.ocean_continent_subductions,
            e.ocean_ocean_subductions,
            e.collisions,
            e.welds
        )
    }
}

/// Output layers of the tectonic stage (spec §4.1).
#[derive(Clone, Debug)]
pub struct TectonicOutput {
    pub plate_id: PeriodicGrid<u8>,
    pub crust_type: PeriodicGrid<u8>,
    /// Crust thickness (surface to Moho), m.
    pub crust_thickness: PeriodicGrid<f32>,
    /// Crust age, Ma.
    pub crust_age: PeriodicGrid<f32>,
    /// Elevation relative to sea level, m.
    pub elevation: PeriodicGrid<f32>,
    /// Depth of the Moho below sea level, m (positive downwards).
    pub moho_depth: PeriodicGrid<f32>,
    pub history_flags: PeriodicGrid<u8>,
    /// Age of the hotspot edifice in Ma, −1 where none.
    pub hotspot_track: PeriodicGrid<f32>,
    pub plates: Vec<Plate>,
    pub hotspots: Vec<Hotspot>,
    pub report: Report,
}

impl TectonicOutput {
    /// Layers in the on-disk format.
    pub fn layers(&self) -> Vec<Layer> {
        vec![
            Layer::from_u8("plate_id", "id", &self.plate_id),
            Layer::from_u8("crust_type", "0=oceanic,1=continental", &self.crust_type),
            Layer::from_f32("crust_thickness", "m", &self.crust_thickness),
            Layer::from_f32("crust_age", "Ma", &self.crust_age),
            Layer::from_f32("elevation_tectonic", "m", &self.elevation),
            Layer::from_f32("moho_depth", "m below sea level", &self.moho_depth),
            Layer::from_u8("history_flags", "bits", &self.history_flags),
            Layer::from_f32("hotspot_track", "Ma", &self.hotspot_track),
        ]
    }
}

impl Tectonics {
    /// Ocean floor depth after `√age` subsidence.
    pub fn ocean_depth(&self, age_ma: f64) -> f64 {
        let t = &self.cfg.tectonics;
        let k = (age_ma.max(0.0) / t.abyssal_age_ma).sqrt().min(1.0);
        t.ridge_depth_m + (t.abyssal_depth_m - t.ridge_depth_m) * k
    }

    /// Computes the output layers from the current state.
    pub fn finish(&self) -> TectonicOutput {
        let t = &self.cfg.tectonics;
        let g = &self.grid;
        let (w, h) = (g.width(), g.height());
        let n = w * h;
        let thr = t.boundary_threshold * self.max_speed();

        // 1. Present-day convergent boundaries → trench and arc seeds.
        let mut trench_seeds = Vec::new();
        let mut arc_seeds = Vec::new();
        for idx in 0..n {
            let (i, j) = g.coords(idx);
            let a = g.data()[idx];
            for (di, dj) in [(1i64, 0i64), (0, 1)] {
                let bidx = g.index(i as i64 + di, j as i64 + dj);
                let b = g.data()[bidx];
                if a.plate == b.plate {
                    continue;
                }
                let va = self.plates[a.plate as usize].velocity;
                let vb = self.plates[b.plate as usize].velocity;
                // Speed of a towards b along the normal a → b.
                let closing = (va.0 - vb.0) * di as f64 + (va.1 - vb.1) * dj as f64;
                if closing <= thr || (a.crust == CONTINENTAL && b.crust == CONTINENTAL) {
                    continue;
                }
                let a_overrides = match (a.crust, b.crust) {
                    (CONTINENTAL, OCEANIC) => true,
                    (OCEANIC, CONTINENTAL) => false,
                    // Ocean–ocean: the older plate sinks; ties go to the lower plate id.
                    _ => (a.age, a.plate) < (b.age, b.plate),
                };
                let (over, under) = if a_overrides {
                    ((idx, a.plate), (bidx, b.plate))
                } else {
                    ((bidx, b.plate), (idx, a.plate))
                };
                trench_seeds.push(under);
                arc_seeds.push(over);
            }
        }
        let trench_w = self.feature(t.trench_width_m);
        let arc_w = self.feature(t.arc_width_m);
        let arc_d = t.arc_distance_m.max(trench_w + self.cell_m);
        let d_trench = self.plate_distance(&trench_seeds, 3.0 * trench_w);
        let d_arc = self.plate_distance(&arc_seeds, arc_d + 3.0 * arc_w);

        // 2. Orogens: smooth the collision uplift potential over the orogen width.
        let oro_w = self.feature(t.orogeny_width_m);
        let mut oro: Vec<f32> = g
            .data()
            .iter()
            .map(|c| {
                if c.crust == CONTINENTAL {
                    c.orogeny
                } else {
                    0.0
                }
            })
            .collect();
        let radius = ((oro_w / (2.0 * self.cell_m)).round() as usize).max(1);
        for _ in 0..3 {
            box_blur(&mut oro, w, h, radius);
        }

        // 3. Per-cell elevation and derived layers.
        let relief = t.max_elevation_m - t.continent_base_elevation_m;
        let ridge_age = (0.1 * t.abyssal_age_ma).max(1.5 * self.dt_ma);
        let arc_noise = Fbm::new(
            stage_u64(self.cfg.seed, "tectonics/island-arcs"),
            (2.5 * self.cell_m).max(4_000.0),
            2,
            0.5,
        );
        let mut elevation = Vec::with_capacity(n);
        let mut thickness = Vec::with_capacity(n);
        let mut moho = Vec::with_capacity(n);
        let mut history = Vec::with_capacity(n);
        let mut track = Vec::with_capacity(n);
        for (idx, c) in g.data().iter().enumerate() {
            let mut f = c.flags & (flags::RIFT | flags::OPHIOLITE);
            let g_trench = gauss(f64::from(d_trench[idx]) / trench_w);
            let g_arc = gauss((f64::from(d_arc[idx]) - arc_d) / arc_w);
            let edifice = self.edifice_height(c);
            if edifice > 0.0 {
                f |= flags::HOTSPOT;
            }
            let e = if c.crust == OCEANIC {
                let base = self.ocean_depth(f64::from(c.age));
                if f64::from(c.age) < ridge_age {
                    f |= flags::RIDGE;
                }
                let mut e = base + (t.trench_depth_m - base).min(0.0) * g_trench;
                if g_trench > FLAG_LEVEL {
                    f |= flags::TRENCH;
                }
                // Island arc: a submerged ridge with discrete volcanic islands where the
                // along-arc noise is high.
                let (x, z) = g.cell_center(idx % w, idx / w);
                let v = smoothstep(0.55, 0.8, 0.5 + 0.5 * arc_noise.sample(x, z));
                let target =
                    t.island_arc_ridge_m + (t.island_arc_peak_m - t.island_arc_ridge_m) * v;
                if target > e {
                    e += (target - e) * g_arc;
                }
                if g_arc > FLAG_LEVEL {
                    f |= flags::ARC;
                }
                e + edifice
            } else {
                let o = f64::from(oro[idx]);
                if o > 0.25 * relief {
                    f |= flags::OROGENY;
                }
                if g_arc > FLAG_LEVEL {
                    f |= flags::ARC;
                }
                let plateau = t.hotspot_plateau_m * edifice / t.hotspot_edifice_height_m;
                (t.continent_base_elevation_m
                    + relief * (o / relief).tanh()
                    + t.arc_uplift_m * g_arc
                    + plateau)
                    .min(t.max_elevation_m)
            };
            let thick = if c.crust == OCEANIC {
                // Edifices and arcs are crust too.
                t.moho_ocean_m + 0.5 * (e - self.ocean_depth(f64::from(c.age))).max(0.0)
            } else {
                let k = ((e - t.continent_base_elevation_m) / relief).clamp(0.0, 1.0);
                t.moho_continent_m + (t.moho_orogen_max_m - t.moho_continent_m) * k
            };
            elevation.push(e as f32);
            thickness.push(thick as f32);
            moho.push((thick - e) as f32);
            history.push(f);
            track.push(if c.hotspot > 0.0 { c.hotspot_age } else { -1.0 });
        }

        let final_cf = self.continental_fraction();
        let report = Report {
            steps: self.steps,
            dt_ma: self.dt_ma,
            plates: self.plates.len(),
            hotspots: self.hotspots.len(),
            initial_continental_fraction: self.initial_continental_fraction,
            final_continental_fraction: final_cf,
            continental_fraction_ok: (final_cf - t.continental_fraction).abs()
                <= t.continental_fraction_tolerance,
            calibration_runs: 1,
            events: self.events,
        };
        let cm = self.cell_m;
        TectonicOutput {
            plate_id: g.map(|c| c.plate),
            crust_type: g.map(|c| c.crust),
            crust_thickness: PeriodicGrid::from_vec(w, h, cm, thickness),
            crust_age: g.map(|c| c.age),
            elevation: PeriodicGrid::from_vec(w, h, cm, elevation),
            moho_depth: PeriodicGrid::from_vec(w, h, cm, moho),
            history_flags: PeriodicGrid::from_vec(w, h, cm, history),
            hotspot_track: PeriodicGrid::from_vec(w, h, cm, track),
            plates: self.plates.clone(),
            hotspots: self.hotspots.clone(),
            report,
        }
    }

    /// Distance in metres from `seeds` (cell, plate), propagating only through cells of the
    /// seed's plate, up to `max_m`. Seeds sit on a boundary, half a cell from their centre.
    /// Unreached cells get `f32::INFINITY`. Dijkstra on 8 neighbours, ties by cell index.
    fn plate_distance(&self, seeds: &[(usize, u8)], max_m: f64) -> Vec<f32> {
        let g = &self.grid;
        let n = g.data().len();
        let mut dist = vec![f64::INFINITY; n];
        let mut heap = BinaryHeap::new();
        for &(idx, plate) in seeds {
            let d0 = 0.5 * self.cell_m;
            if d0 < dist[idx] && g.data()[idx].plate == plate {
                dist[idx] = d0;
                heap.push(Reverse((d0.to_bits(), idx)));
            }
        }
        let diag = self.cell_m * std::f64::consts::SQRT_2;
        while let Some(Reverse((bits, idx))) = heap.pop() {
            let d = f64::from_bits(bits);
            if d > dist[idx] {
                continue;
            }
            let (i, j) = g.coords(idx);
            let plate = g.data()[idx].plate;
            for (di, dj) in [
                (-1i64, -1i64),
                (0, -1),
                (1, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ] {
                let nidx = g.index(i as i64 + di, j as i64 + dj);
                if g.data()[nidx].plate != plate {
                    continue;
                }
                let nd = d + if di != 0 && dj != 0 {
                    diag
                } else {
                    self.cell_m
                };
                if nd <= max_m && nd < dist[nidx] {
                    dist[nidx] = nd;
                    heap.push(Reverse((nd.to_bits(), nidx)));
                }
            }
        }
        dist.into_iter().map(|d| d as f32).collect()
    }
}

#[inline]
fn gauss(x: f64) -> f64 {
    if x.is_finite() { (-x * x).exp() } else { 0.0 }
}

/// Periodic box blur of radius `r` cells, horizontal then vertical.
fn box_blur(v: &mut [f32], w: usize, h: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f64;
    let mut tmp = vec![0f32; v.len()];
    for j in 0..h {
        let row = &v[j * w..(j + 1) * w];
        let mut s: f64 = (0..=2 * r)
            .map(|k| f64::from(row[(k + w * 4 - r) % w]))
            .sum();
        for i in 0..w {
            tmp[j * w + i] = (s * norm) as f32;
            s += f64::from(row[(i + r + 1) % w]) - f64::from(row[(i + w * 4 - r) % w]);
        }
    }
    for i in 0..w {
        let mut s: f64 = (0..=2 * r)
            .map(|k| f64::from(tmp[((k + h * 4 - r) % h) * w + i]))
            .sum();
        for j in 0..h {
            v[j * w + i] = (s * norm) as f32;
            s += f64::from(tmp[((j + r + 1) % h) * w + i])
                - f64::from(tmp[((j + h * 4 - r) % h) * w + i]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_blur_is_periodic_and_mass_preserving() {
        let (w, h) = (25, 25);
        let mut v = vec![0f32; w * h];
        v[0] = 625.0; // corner cell: the blur must wrap on both axes
        box_blur(&mut v, w, h, 2);
        let total: f32 = v.iter().sum();
        assert!((total - 625.0).abs() < 1e-3);
        assert!((v[24 * w + 24] - 25.0).abs() < 1e-4);
        assert!((v[2 * w + 2] - 25.0).abs() < 1e-4);
        assert_eq!(v[3 * w], 0.0);
    }
}
