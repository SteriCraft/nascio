//! Initial state: warped periodic Voronoi plates, continental masses, hotspots.

use std::f64::consts::TAU;

use nascio_core::consts::WORLD_SIZE_F64;
use nascio_core::torus::{distance_f64, wrap_f64};
use rand::RngExt;
use rayon::prelude::*;

use super::{CONTINENTAL, Cell, EventCounts, Hotspot, OCEANIC, Plate, Tectonics};
use crate::config::WorldConfig;
use crate::grid::PeriodicGrid;
use crate::noise::Fbm;
use crate::rng::{stage_rng, stage_u64};

impl Tectonics {
    /// Builds the initial state from a validated config.
    pub fn new(cfg: &WorldConfig) -> Self {
        let t = &cfg.tectonics;
        let seed = cfg.seed;
        let res = cfg.scale.tectonic_res();
        let cell_m = cfg.scale.tectonic_cell_m();
        // One cell per step at most for the fastest plate.
        let steps = (t.total_displacement_m / cell_m).ceil().max(1.0) as u32;
        let dt_ma = t.duration_ma / f64::from(steps);
        let max_speed = t.total_displacement_m / f64::from(steps);

        let plates = make_plates(cfg, max_speed);
        let hotspots = make_hotspots(cfg);
        let np = plates.len();

        // Plate ownership: nearest seed after a domain warp, on the torus.
        let warp_x = Fbm::new(
            stage_u64(seed, "tectonics/warp-x"),
            t.plate_warp_period_m,
            4,
            0.5,
        );
        let warp_z = Fbm::new(
            stage_u64(seed, "tectonics/warp-z"),
            t.plate_warp_period_m,
            4,
            0.5,
        );
        let continents = Fbm::new(
            stage_u64(seed, "tectonics/continents"),
            t.continent_noise_period_m,
            6,
            0.5,
        );
        let ages = Fbm::new(stage_u64(seed, "tectonics/ages"), 25_000.0, 3, 0.5);

        let n = res * res;
        let probe = PeriodicGrid::new(res, res, cell_m, 0u8);
        let fields: Vec<(u8, f64, f64)> = (0..n)
            .into_par_iter()
            .map(|idx| {
                let (i, j) = probe.coords(idx);
                let (x, z) = probe.cell_center(i, j);
                let q = (
                    wrap_f64(x + t.plate_warp_amplitude_m * warp_x.sample(x, z)),
                    wrap_f64(z + t.plate_warp_amplitude_m * warp_z.sample(x, z)),
                );
                let mut best = (f64::INFINITY, 0u8);
                for p in &plates {
                    let d = distance_f64(q, p.seed);
                    if d < best.0 {
                        best = (d, p.id);
                    }
                }
                (
                    best.1,
                    continents.sample(x, z),
                    ages.sample(x, z) * 0.5 + 0.5,
                )
            })
            .collect();

        // Exact continental share: threshold at the quantile of the continent noise.
        let mut sorted: Vec<f64> = fields.iter().map(|f| f.1).collect();
        sorted.sort_by(f64::total_cmp);
        let k = ((1.0 - t.continental_fraction) * n as f64).round() as usize;
        let threshold = sorted[k.min(n - 1)];

        let cells: Vec<Cell> = fields
            .iter()
            .map(|&(plate, c, a)| {
                let continental = c >= threshold && t.continental_fraction > 0.0;
                Cell {
                    plate,
                    crust: if continental { CONTINENTAL } else { OCEANIC },
                    age: if continental {
                        (1_000.0 + 1_500.0 * a) as f32
                    } else {
                        (t.abyssal_age_ma * (0.4 + 0.8 * a)) as f32
                    },
                    orogeny: 0.0,
                    hotspot: 0.0,
                    hotspot_age: -1.0,
                    flags: 0,
                }
            })
            .collect();

        let mut s = Self {
            cfg: cfg.clone(),
            res,
            cell_m,
            steps,
            dt_ma,
            step_index: 0,
            plates,
            hotspots,
            grid: PeriodicGrid::from_vec(res, res, cell_m, cells),
            events: EventCounts::default(),
            initial_continental_fraction: 0.0,
            contact: vec![0.0; np * np],
            group: (0..np as u8).collect(),
        };
        s.initial_continental_fraction = s.continental_fraction();
        s
    }
}

fn make_plates(cfg: &WorldConfig, max_speed: f64) -> Vec<Plate> {
    let t = &cfg.tectonics;
    let mut rng = stage_rng(cfg.seed, "tectonics/plates");
    let n = rng.random_range(t.plate_count[0]..=t.plate_count[1]) as usize;
    // Loose Poisson-disc sampling so plates have comparable sizes.
    let min_dist = 0.6 * WORLD_SIZE_F64 / (n as f64).sqrt();
    let mut seeds: Vec<(f64, f64)> = Vec::with_capacity(n);
    while seeds.len() < n {
        let mut candidate = (0.0, 0.0);
        for _ in 0..500 {
            candidate = (
                rng.random::<f64>() * WORLD_SIZE_F64,
                rng.random::<f64>() * WORLD_SIZE_F64,
            );
            if seeds
                .iter()
                .all(|s| distance_f64(*s, candidate) >= min_dist)
            {
                break;
            }
        }
        seeds.push(candidate);
    }
    seeds
        .into_iter()
        .enumerate()
        .map(|(k, seed)| {
            let angle = rng.random::<f64>() * TAU;
            let frac = rng.random_range(t.plate_speed_fraction[0]..=t.plate_speed_fraction[1]);
            let speed = frac * max_speed;
            Plate {
                id: k as u8,
                seed,
                velocity: (speed * angle.cos(), speed * angle.sin()),
            }
        })
        .collect()
}

fn make_hotspots(cfg: &WorldConfig) -> Vec<Hotspot> {
    let t = &cfg.tectonics;
    let mut rng = stage_rng(cfg.seed, "tectonics/hotspots");
    let n = rng.random_range(t.hotspot_count[0]..=t.hotspot_count[1]);
    (0..n)
        .map(|_| Hotspot {
            pos: (
                rng.random::<f64>() * WORLD_SIZE_F64,
                rng.random::<f64>() * WORLD_SIZE_F64,
            ),
            travel_m: rng.random::<f64>() * t.hotspot_island_spacing_m,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Scale;

    #[test]
    fn initial_state() {
        let cfg = WorldConfig::new(42, Scale::Fifth);
        let t = Tectonics::new(&cfg);
        assert_eq!(t.res, 125);
        assert_eq!(t.steps, 50);
        assert!((8..=12).contains(&t.plates.len()));
        assert!((2..=5).contains(&t.hotspots.len()));
        assert!((t.initial_continental_fraction - 0.30).abs() < 0.01);
        // Every plate owns cells.
        for p in &t.plates {
            assert!(
                t.grid.data().iter().any(|c| c.plate == p.id),
                "plate {}",
                p.id
            );
        }
        assert!(t.max_speed() <= t.cell_m + 1e-9);
    }

    #[test]
    fn steps_scale_with_resolution() {
        for (scale, steps) in [
            (Scale::TwentyFifth, 10),
            (Scale::Fifth, 50),
            (Scale::Full, 250),
        ] {
            let cfg = WorldConfig::new(1, scale);
            let t = &cfg.tectonics;
            assert_eq!(
                (t.total_displacement_m / scale.tectonic_cell_m()).ceil() as u32,
                steps
            );
        }
    }
}
