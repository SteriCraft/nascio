//! Provisional precipitation for erosion (spec §4.2): the climate of M4 replaces it.
//!
//! On the tectonic grid, in mm/yr:
//! - a zonal profile in latitude φ (wet ITCZ, dry belts near 30°, mid-latitude maximum, dry
//!   polar latitudes);
//! - an orographic effect along the zonal winds of each belt (trade winds and polar winds from
//!   the east, westerlies from the west): every row is walked along the wind with a moisture
//!   reservoir that the sea refills, that slowly dries over land and that climbs empty out. The
//!   windward side gets wetter, the lee side dries (rain shadow);
//! - the polar band (spec §2.7): `P = max(P, polar_precip · c_min(s))`.
//!
//! Winds are purely zonal, so rows are independent (parallel and deterministic). Each row is
//! walked twice around the torus so the moisture state wraps consistently.

use nascio_core::fmath::DetMath;
use nascio_core::geo::{latitude, seam_distance};
use nascio_core::math::smoothstep;
use rayon::prelude::*;

use super::ErosionConfig;
use crate::config::WorldConfig;
use crate::grid::PeriodicGrid;

/// Zonal precipitation at `|φ|` in degrees, linear between the table points.
pub fn zonal(table: &[[f64; 2]], abs_lat_deg: f64) -> f64 {
    let a = abs_lat_deg.clamp(table[0][0], table[table.len() - 1][0]);
    for w in table.windows(2) {
        if a <= w[1][0] {
            let t = (a - w[0][0]) / (w[1][0] - w[0][0]);
            return w[0][1] + (w[1][1] - w[0][1]) * t;
        }
    }
    table[table.len() - 1][1]
}

/// Weight of the easterly winds at `|φ|` in degrees (1 = from the east, 0 = from the west):
/// easterlies below 30° and above 60°, westerlies between, blended over `±blend`.
pub fn easterly_weight(abs_lat_deg: f64, blend: f64) -> f64 {
    let b = blend.max(1e-6);
    let west = smoothstep(30.0 - b, 30.0 + b, abs_lat_deg)
        * (1.0 - smoothstep(60.0 - b, 60.0 + b, abs_lat_deg));
    1.0 - west
}

/// Precipitation along one row for a wind blowing towards `+x` (`dir = 1`) or `−x`
/// (`dir = −1`). `elev` is the row's elevation, `pz` the zonal precipitation of the row.
fn walk_row(elev: &[f64], dx: f64, pz: f64, dir: i64, c: &ErosionConfig) -> Vec<f64> {
    let w = elev.len() as i64;
    let mut out = vec![0.0; elev.len()];
    let recharge = 1.0 - (-dx / c.ocean_recharge_m).det_exp();
    let decay = (-dx / c.land_decay_m).det_exp();
    let scale = pz / 1_000.0;
    let mut q = 1.0f64;
    let mut prev = elev[0].max(0.0);
    for step in 0..2 * w {
        let i = (step * dir).rem_euclid(w) as usize;
        let h = elev[i];
        if h <= 0.0 {
            q += (1.0 - q) * recharge;
        } else {
            q *= decay;
        }
        let top = h.max(0.0);
        let climb = (top - prev).max(0.0);
        prev = top;
        let released = q * (1.0 - (-climb / c.orographic_height_m).det_exp());
        q -= released;
        if step >= w {
            out[i] = pz * (c.dry_floor + (1.0 - c.dry_floor) * q)
                + scale * c.orographic_flux * released / dx;
        }
    }
    out
}

/// Mean of each `ratio × ratio` block of the fine grid.
fn downsample(fine: &PeriodicGrid<f64>, res: usize, cell: f64) -> Vec<f64> {
    let ratio = fine.width() / res;
    let norm = 1.0 / (ratio * ratio) as f64;
    (0..res * res)
        .map(|k| {
            let (i, j) = (k % res, k / res);
            let mut s = 0.0;
            for fj in j * ratio..(j + 1) * ratio {
                for fi in i * ratio..(i + 1) * ratio {
                    s += fine.data()[fj * fine.width() + fi];
                }
            }
            debug_assert!((cell * res as f64 - 100_000.0).abs() < 1e-6);
            s * norm
        })
        .collect()
}

/// Provisional precipitation on the tectonic grid, mm/yr.
pub fn precipitation(cfg: &WorldConfig, fine: &PeriodicGrid<f64>) -> PeriodicGrid<f32> {
    let c = &cfg.erosion;
    let res = cfg.scale.tectonic_res();
    let cell = cfg.scale.tectonic_cell_m();
    let elev = downsample(fine, res, cell);
    let mut p = vec![0f64; res * res];
    p.par_chunks_mut(res).enumerate().for_each(|(j, row)| {
        let z = (j as f64 + 0.5) * cell;
        let lat = latitude(z).to_degrees().abs();
        let pz = zonal(&c.zonal_precip, lat);
        let e_row = &elev[j * res..(j + 1) * res];
        let we = easterly_weight(lat, c.wind_blend_deg);
        let from_east = walk_row(e_row, cell, pz, -1, c);
        let from_west = walk_row(e_row, cell, pz, 1, c);
        let polar = c.polar_precip_mm * cfg.astro.min_cloud_cover(seam_distance(z));
        for (i, o) in row.iter_mut().enumerate() {
            *o = (we * from_east[i] + (1.0 - we) * from_west[i]).max(polar);
        }
    });
    // Light periodic smoothing (two 3×3 means): the walk is one cell sharp.
    let g = PeriodicGrid::from_vec(res, res, cell, p);
    let blur = |g: &PeriodicGrid<f64>| {
        PeriodicGrid::from_fn(res, res, cell, |i, j| {
            let (i, j) = (i as i64, j as i64);
            let mut s = 0.0;
            for dj in -1..=1 {
                for di in -1..=1 {
                    s += g.get(i + di, j + dj);
                }
            }
            s / 9.0
        })
    };
    blur(&blur(&g)).map(|&v| v as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zonal_profile_interpolates() {
        let c = ErosionConfig::default();
        assert_eq!(zonal(&c.zonal_precip, 0.0), 2_200.0);
        assert_eq!(zonal(&c.zonal_precip, 28.0), 350.0);
        assert!(zonal(&c.zonal_precip, 50.0) > zonal(&c.zonal_precip, 30.0));
        assert_eq!(zonal(&c.zonal_precip, 95.0), 300.0);
    }

    #[test]
    fn wind_belts() {
        assert_eq!(easterly_weight(10.0, 5.0), 1.0);
        assert_eq!(easterly_weight(45.0, 5.0), 0.0);
        assert_eq!(easterly_weight(80.0, 5.0), 1.0);
        assert!((easterly_weight(30.0, 5.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn windward_wet_lee_dry() {
        let c = ErosionConfig::default();
        // Sea, then a 2 000 m range in the middle of a 40-cell continent, then sea.
        let n = 125;
        let elev: Vec<f64> = (0..n)
            .map(|i| match i {
                40..=79 => 200.0 + 1_800.0 * (-(((i as f64) - 60.0) / 5.0).powi(2)).det_exp(),
                _ => -2_000.0,
            })
            .collect();
        let p = walk_row(&elev, 800.0, 1_000.0, 1, &c);
        // Wind towards +x: windward slope (i < 60) wetter than the lee (i > 60).
        assert!(p[57] > 2.0 * p[63], "windward {} lee {}", p[57], p[63]);
        assert!(p[57] > p[20], "windward {} sea {}", p[57], p[20]);
        assert!(p[75] < p[20]);
        // Periodic: the row wraps without a jump at its end.
        assert!((p[0] - p[n - 1]).abs() < 1.0);
    }
}
