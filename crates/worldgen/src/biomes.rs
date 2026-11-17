//! Biomes on the fine grid (spec §4.5): simplified Köppen classification from the climate
//! harmonics, with special cases (ice caps and glaciers, mangroves, marshes).
//!
//! The temperature of each fine cell is the bilinear climate temperature corrected by the lapse
//! rate for the difference between the cell's height and the mean height of the climate cell,
//! so summits get their glaciers. Months are the twelve evaluations at `N = 365·(m + 0.5)/12`.

use nascio_core::geo::latitude;
use rayon::prelude::*;

use crate::climate::{ClimateOutput, HarmonicGrid};
use crate::config::WorldConfig;
use crate::erosion::ErosionOutput;
use crate::grid::PeriodicGrid;
use crate::world_io::Layer;

pub use nascio_world::biomes::{BiomeConfig, NAMES, code};

/// Monthly climate of a place.
#[derive(Clone, Copy, Debug)]
pub struct Months {
    /// Mean temperature, °C.
    pub t: [f64; 12],
    /// Precipitation, mm per month.
    pub p: [f64; 12],
    /// Smallest snow cover over the year.
    pub min_snow: f64,
    /// Latitude, degrees (sets the summer half of the year).
    pub lat: f64,
}

/// Köppen-like class of a land climate.
pub fn classify(m: &Months, c: &BiomeConfig) -> u8 {
    let t_max = m.t.iter().cloned().fold(f64::MIN, f64::max);
    let t_min = m.t.iter().cloned().fold(f64::MAX, f64::min);
    let t_ann = m.t.iter().sum::<f64>() / 12.0;
    let p_ann: f64 = m.p.iter().sum();
    if m.min_snow >= c.permanent_snow || t_max < c.ice_max_c {
        return code::ICE;
    }
    if t_max < c.tundra_max_c {
        return code::TUNDRA;
    }
    // Summer half: April–September in the north, October–March in the south.
    let summer = |month: usize| (3..9).contains(&month) == (m.lat >= 0.0);
    let p_summer: f64 = (0..12).filter(|&k| summer(k)).map(|k| m.p[k]).sum();
    let share = if p_ann > 0.0 { p_summer / p_ann } else { 0.5 };
    let thr = 20.0 * t_ann
        + if share >= 0.7 {
            280.0
        } else if share >= 0.3 {
            140.0
        } else {
            0.0
        };
    if p_ann < thr.max(0.0) {
        let hot = t_ann >= 18.0;
        return if p_ann < 0.5 * thr {
            if hot {
                code::HOT_DESERT
            } else {
                code::COLD_DESERT
            }
        } else {
            code::STEPPE
        };
    }
    if t_min >= c.tropical_min_c {
        let driest = m.p.iter().cloned().fold(f64::MAX, f64::min);
        return if driest >= c.rainforest_driest_mm {
            code::RAINFOREST
        } else {
            code::SAVANNA
        };
    }
    if t_min > c.temperate_min_c {
        let summer_dry = (0..12)
            .filter(|&k| summer(k))
            .map(|k| m.p[k])
            .fold(f64::MAX, f64::min);
        let winter_wet = (0..12)
            .filter(|&k| !summer(k))
            .map(|k| m.p[k])
            .fold(0.0, f64::max);
        return if summer_dry < 40.0 && summer_dry < winter_wet / 3.0 {
            code::MEDITERRANEAN
        } else {
            code::TEMPERATE_FOREST
        };
    }
    let warm = m.t.iter().filter(|&&t| t >= 10.0).count();
    if warm < c.boreal_warm_months {
        code::BOREAL_FOREST
    } else {
        code::TEMPERATE_FOREST
    }
}

fn sample(h: &HarmonicGrid, x: f64, z: f64) -> nascio_core::climate::Harmonics {
    nascio_core::climate::Harmonics {
        x0: h.x0.sample_bilinear(x, z),
        a1: h.a1.sample_bilinear(x, z),
        // Phases do not interpolate across their wrap: take the nearest cell.
        p1: f64::from(*nearest(&h.p1, x, z)),
        a2: h.a2.sample_bilinear(x, z),
        p2: f64::from(*nearest(&h.p2, x, z)),
    }
}

fn nearest<T>(g: &PeriodicGrid<T>, x: f64, z: f64) -> &T {
    let (i, j) = g.cell_of(x, z);
    g.get(i as i64, j as i64)
}

/// Biome per fine cell.
pub fn biomes(cfg: &WorldConfig, ero: &ErosionOutput, clim: &ClimateOutput) -> PeriodicGrid<u8> {
    let c = &cfg.biomes;
    let lapse = cfg.climate.lapse_rate_c_per_m;
    let elev = &ero.elevation;
    let (res, cell) = (elev.width(), elev.cell_size());
    // Mean land height of each climate cell, at the fine resolution (same downsampling as the
    // climate), to correct the temperature for the local relief.
    let cres = clim.temperature.x0.width();
    let ratio = res / cres;
    let mut mean_h = vec![0.0f64; cres * cres];
    for (k, m) in mean_h.iter_mut().enumerate() {
        let (ci, cj) = (k % cres, k / cres);
        let mut s = 0.0;
        for fj in cj * ratio..(cj + 1) * ratio {
            for fi in ci * ratio..(ci + 1) * ratio {
                let f = fj * res + fi;
                if ero.basin_id.data()[f] != 0 {
                    s += f64::from(elev.data()[f]).max(0.0);
                }
            }
        }
        *m = s / (ratio * ratio) as f64;
    }
    let mean_h = PeriodicGrid::from_vec(cres, cres, clim.temperature.x0.cell_size(), mean_h);
    let months: Vec<f64> = (0..12).map(|m| 365.0 * (m as f64 + 0.5) / 12.0).collect();
    let mut out = vec![0u8; res * res];
    out.par_chunks_mut(res).enumerate().for_each(|(j, row)| {
        let z = (j as f64 + 0.5) * cell;
        let lat = latitude(z).to_degrees();
        for (i, o) in row.iter_mut().enumerate() {
            let k = j * res + i;
            if ero.basin_id.data()[k] == 0 {
                *o = code::OCEAN;
                continue;
            }
            if ero.lake_depth.data()[k] > 0.0 {
                *o = code::LAKE;
                continue;
            }
            let x = (i as f64 + 0.5) * cell;
            let h = f64::from(elev.data()[k]);
            let dt = -lapse * (h.max(0.0) - mean_h.sample_bilinear(x, z));
            let th = sample(&clim.temperature, x, z);
            let ph = sample(&clim.precipitation, x, z);
            let sh = sample(&clim.snow_fraction, x, z);
            let mut m = Months {
                t: [0.0; 12],
                p: [0.0; 12],
                min_snow: 1.0,
                lat,
            };
            for (mi, &n) in months.iter().enumerate() {
                m.t[mi] = th.eval(n) + dt;
                m.p[mi] = ph.eval(n).max(0.0) / 12.0;
                m.min_snow = m.min_snow.min(sh.eval(n));
            }
            let mut b = classify(&m, c);
            let t_min = m.t.iter().cloned().fold(f64::MAX, f64::min);
            let near = |pred: &dyn Fn(usize) -> bool| {
                (-1i64..=1).any(|dj| {
                    (-1i64..=1).any(|di| {
                        let ii = (i as i64 + di).rem_euclid(res as i64) as usize;
                        let jj = (j as i64 + dj).rem_euclid(res as i64) as usize;
                        pred(jj * res + ii)
                    })
                })
            };
            let arid = matches!(b, code::HOT_DESERT | code::COLD_DESERT | code::STEPPE);
            if t_min >= c.tropical_min_c
                && h <= c.mangrove_max_m
                && !arid
                && near(&|f| ero.basin_id.data()[f] == 0)
            {
                b = code::MANGROVE;
            } else if !arid
                && !matches!(b, code::ICE | code::TUNDRA)
                && m.p.iter().sum::<f64>() >= c.marsh_min_mm
                && slope(elev, i, j) <= c.marsh_max_slope
                && near(&|f| {
                    ero.lake_depth.data()[f] > 0.0
                        || ero.river_order.data()[f] >= c.marsh_river_order
                })
            {
                b = code::MARSH;
            }
            *o = b;
        }
    });
    PeriodicGrid::from_vec(res, res, cell, out)
}

/// Largest slope to the four neighbours, m/m.
fn slope(g: &PeriodicGrid<f32>, i: usize, j: usize) -> f64 {
    let h = f64::from(*g.get(i as i64, j as i64));
    [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)]
        .iter()
        .map(|&(di, dj)| (f64::from(*g.get(i as i64 + di, j as i64 + dj)) - h).abs())
        .fold(0.0, f64::max)
        / g.cell_size()
}

/// The `biome` layer.
pub fn layer(b: &PeriodicGrid<u8>) -> Layer {
    Layer::from_u8("biome", "code (see biomes::NAMES)", b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn months(t: [f64; 12], p: [f64; 12]) -> Months {
        Months {
            t,
            p,
            min_snow: 0.0,
            lat: 45.0,
        }
    }

    #[test]
    fn classic_climates() {
        let c = BiomeConfig::default();
        // Equatorial: hot and wet all year.
        assert_eq!(
            classify(&months([27.0; 12], [200.0; 12]), &c),
            code::RAINFOREST
        );
        // Sahara-like: hot, almost no rain.
        assert_eq!(
            classify(&months([28.0; 12], [2.0; 12]), &c),
            code::HOT_DESERT
        );
        // Siberia-like: cold winters, short summer.
        let t = [
            -30.0, -28.0, -18.0, -5.0, 5.0, 12.0, 16.0, 13.0, 5.0, -6.0, -20.0, -28.0,
        ];
        assert_eq!(classify(&months(t, [40.0; 12]), &c), code::BOREAL_FOREST);
        // Western Europe.
        let t = [
            4.0, 5.0, 8.0, 11.0, 14.0, 17.0, 19.0, 19.0, 16.0, 12.0, 8.0, 5.0,
        ];
        assert_eq!(classify(&months(t, [70.0; 12]), &c), code::TEMPERATE_FOREST);
        // Mediterranean: dry summer.
        let p = [
            90.0, 80.0, 70.0, 50.0, 30.0, 10.0, 5.0, 8.0, 30.0, 70.0, 90.0, 100.0,
        ];
        assert_eq!(classify(&months(t, p), &c), code::MEDITERRANEAN);
        // Tundra and ice.
        let cold = [
            -25.0, -25.0, -20.0, -12.0, -3.0, 4.0, 7.0, 5.0, 0.0, -8.0, -18.0, -22.0,
        ];
        assert_eq!(classify(&months(cold, [30.0; 12]), &c), code::TUNDRA);
        assert_eq!(classify(&months([-20.0; 12], [30.0; 12]), &c), code::ICE);
        let mut m = months(t, [70.0; 12]);
        m.min_snow = 1.0;
        assert_eq!(classify(&m, &c), code::ICE);
    }
}
