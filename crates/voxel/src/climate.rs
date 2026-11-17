//! Climate of the surface for the chunks (M6): annual mean and warmest-month temperatures and
//! annual precipitation, from the climate harmonics (160 m grid).
//!
//! Temperatures are stored reduced to sea level with the lapse rate, from the mean land height
//! of each climate cell (as the biomes do), so that the temperature at any block height `y` is
//! `t_sl − lapse · max(y, 0)`: the fine relief gets its own snow line.

use nascio_core::climate::Harmonics;
use nascio_worldgen::grid::PeriodicGrid;
use nascio_worldgen::world_io::{Layer, World};

/// Samples of the year used for the warmest month.
const YEAR_SAMPLES: usize = 24;

/// Surface climate of a world.
#[derive(Clone, Debug)]
pub struct SurfaceClimate {
    /// Lapse rate, °C per m (positive: colder upwards).
    pub lapse: f64,
    t_mean_sl: PeriodicGrid<f32>,
    t_warm_sl: PeriodicGrid<f32>,
    precip: PeriodicGrid<f32>,
}

/// Climate of one column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnClimate {
    /// Annual mean temperature reduced to sea level, °C.
    pub t_mean_sl: f32,
    /// Warmest-month temperature reduced to sea level, °C.
    pub t_warm_sl: f32,
    /// Annual precipitation, mm.
    pub precip: f32,
}

impl Default for ColumnClimate {
    /// A temperate climate, for worlds without climate.
    fn default() -> Self {
        Self {
            t_mean_sl: 12.0,
            t_warm_sl: 20.0,
            precip: 900.0,
        }
    }
}

fn grid(l: &Layer) -> PeriodicGrid<f32> {
    let data = (0..l.width * l.height)
        .map(|k| l.data.get_f64(k) as f32)
        .collect();
    PeriodicGrid::from_vec(l.width, l.height, l.cell_m, data)
}

impl SurfaceClimate {
    /// Climate of a generated world (`None` before M4), with `elevation` its finest relief.
    pub fn from_world(world: &World, elevation: &PeriodicGrid<f32>) -> Option<Self> {
        let names = ["x0", "a1", "p1", "a2", "p2"];
        let t: Vec<PeriodicGrid<f32>> = names
            .iter()
            .map(|n| world.layer(&format!("temperature_{n}")).map(grid))
            .collect::<Option<_>>()?;
        let precip = grid(world.layer("precipitation_x0")?);
        let lapse = world.config.climate.lapse_rate_c_per_m;
        let (w, h, cell) = (t[0].width(), t[0].height(), t[0].cell_size());
        // Mean land height of each climate cell, from the finest relief.
        let ratio = ((cell / elevation.cell_size()).round() as usize).max(1);
        let mean_h = PeriodicGrid::from_fn(w, h, cell, |i, j| {
            let mut s = 0.0f64;
            for fj in 0..ratio {
                for fi in 0..ratio {
                    let e = elevation.get((i * ratio + fi) as i64, (j * ratio + fj) as i64);
                    s += f64::from(*e).max(0.0);
                }
            }
            (s / (ratio * ratio) as f64) as f32
        });
        let harm = |i: usize, j: usize| {
            let g = |k: usize| f64::from(*t[k].get(i as i64, j as i64));
            Harmonics {
                x0: g(0),
                a1: g(1),
                p1: g(2),
                a2: g(3),
                p2: g(4),
            }
        };
        let lift = |i: usize, j: usize| lapse * f64::from(*mean_h.get(i as i64, j as i64));
        let t_mean_sl =
            PeriodicGrid::from_fn(w, h, cell, |i, j| (harm(i, j).x0 + lift(i, j)) as f32);
        let t_warm_sl = PeriodicGrid::from_fn(w, h, cell, |i, j| {
            let hm = harm(i, j);
            let warm = (0..YEAR_SAMPLES)
                .map(|k| hm.eval(365.0 * (k as f64 + 0.5) / YEAR_SAMPLES as f64))
                .fold(f64::NEG_INFINITY, f64::max);
            (warm + lift(i, j)) as f32
        });
        Some(Self {
            lapse,
            t_mean_sl,
            t_warm_sl,
            precip,
        })
    }

    /// Climate at world position `(x, z)` (bilinear, periodic).
    pub fn column(&self, x: f64, z: f64) -> ColumnClimate {
        ColumnClimate {
            t_mean_sl: self.t_mean_sl.sample_bilinear(x, z) as f32,
            t_warm_sl: self.t_warm_sl.sample_bilinear(x, z) as f32,
            precip: self.precip.sample_bilinear(x, z).max(0.0) as f32,
        }
    }
}
