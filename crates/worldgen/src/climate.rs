//! Climate on the 160 m grid (spec §4.3, M4).
//!
//! A typical year is simulated over `steps` evenly spaced days, then every quantity is projected
//! on its annual harmonics `(X0, A1, p1, A2, p2)` ([`nascio_core::climate::Harmonics`]).
//!
//! - **Temperature**: equilibrium temperature from the daily insolation Q (with `δ_eff`, so the
//!   seasons vanish at the seam): the annual mean follows a calibrated profile of the annual
//!   mean insolation (relative to the equator's), which stands in for the heat transport and the
//!   ice albedo, and the seasonal part is `k·(Q(N) − Q̄)`; minus the lapse rate, minus the albedo
//!   cooling of the polar band. Thermal inertia is a first-order relaxation
//!   towards `T_eq` whose time constant gives a phase lag of about one month on land and 1.5 to
//!   2 months at sea (with a damped seasonal swing, `sea_amplitude`), blended by
//!   continentality (distance to the coast). The periodic steady
//!   state of that relaxation is computed exactly, harmonic by harmonic (DFT of the samples). Sea
//!   surfaces never cool below the freezing point of sea water. Ocean currents add warm and cold
//!   anomalies.
//! - **Winds**: three cells per hemisphere defined in latitude φ, shifted with `δ_eff` (the ITCZ
//!   follows the sun): trade winds, westerlies, polar easterlies. The wind dies out at the seam
//!   (polar anticyclone: both sides diverge).
//! - **Ocean currents** (schematic, constant over the year): warm poleward boundary currents on
//!   the western edges of the basins, cold equatorward ones (upwelling) on their eastern edges in
//!   the subtropics, zonal drift with the winds elsewhere.
//! - **Moisture and precipitation**: the air carries water vapour as a mixing ratio `w` (g/kg),
//!   advected along the wind (semi-Lagrangian gather, deterministic). Over the sea it recharges
//!   towards `sea_humidity` times the saturation mixing ratio `w_s(T, P)` at the local air
//!   temperature and pressure ([`nascio_core::atmosphere`]); over land it slowly dries. Wherever
//!   `w > w_s(T, P)` the excess condenses and rains out: air climbing a slope cools with the
//!   lapse rate and meets a lower pressure (windward rain, rain shadow beyond), and air carried
//!   over colder land does the same (coastal winter snow). Over the sea the air only cools from
//!   below (cold currents): the excess condenses into fog and is removed, without rain. The large-scale rain is the zonal
//!   profile shifted with the ITCZ (wet ITCZ, dry belts near 30°), scaled by the moisture of the
//!   air relative to marine air at the cell's sea-level temperature. Polar band: precipitation
//!   forced up to `polar_precip_mm·c_min(s)`. The surface mixing ratio is exported (`humidity`):
//!   with the temperature and the pressure it gives the relative humidity anywhere.
//! - **Clouds** from the precipitation, never below `c_min(s)` (§2.7).
//! - **Snow cover**: snowpack balance (snowfall below freezing, degree-day melt) run until it
//!   repeats from one year to the next; permanent snow in the polar band (§4.3).
//! - **Diurnal range** (static): larger inland and in dry climates; the cycle itself is evaluated
//!   at run time (`nascio_core::climate::diurnal_offset`).

use nascio_core::fmath::DetMath;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::f64::consts::TAU;

use nascio_core::AstroConfig;
use nascio_core::atmosphere::saturation_mixing_ratio;
use nascio_core::climate::Harmonics;
use nascio_core::geo::{latitude, seam_distance};
use nascio_core::math::smoothstep;
use rayon::prelude::*;

use crate::config::WorldConfig;
use crate::erosion::precip::{easterly_weight, zonal};
use crate::grid::PeriodicGrid;
use crate::world_io::Layer;

const YEAR: f64 = 365.0;
const SECONDS_PER_YEAR: f64 = YEAR * 86_400.0;

pub use nascio_world::climate::ClimateConfig;

/// The five harmonic grids of one quantity.
#[derive(Clone, Debug)]
pub struct HarmonicGrid {
    pub x0: PeriodicGrid<f32>,
    pub a1: PeriodicGrid<f32>,
    pub p1: PeriodicGrid<f32>,
    pub a2: PeriodicGrid<f32>,
    pub p2: PeriodicGrid<f32>,
}

impl HarmonicGrid {
    fn from_cells(res: usize, cell: f64, h: &[Harmonics]) -> Self {
        let g = |f: fn(&Harmonics) -> f64| {
            PeriodicGrid::from_vec(res, res, cell, h.iter().map(|x| f(x) as f32).collect())
        };
        Self {
            x0: g(|h| h.x0),
            a1: g(|h| h.a1),
            p1: g(|h| h.p1),
            a2: g(|h| h.a2),
            p2: g(|h| h.p2),
        }
    }

    /// Harmonics of cell `k`.
    pub fn cell(&self, k: usize) -> Harmonics {
        Harmonics {
            x0: f64::from(self.x0.data()[k]),
            a1: f64::from(self.a1.data()[k]),
            p1: f64::from(self.p1.data()[k]),
            a2: f64::from(self.a2.data()[k]),
            p2: f64::from(self.p2.data()[k]),
        }
    }

    fn layers(&self, name: &str, unit: &str) -> Vec<Layer> {
        vec![
            Layer::from_f32(&format!("{name}_x0"), unit, &self.x0),
            Layer::from_f32(&format!("{name}_a1"), unit, &self.a1),
            Layer::from_f32(&format!("{name}_p1"), "day", &self.p1),
            Layer::from_f32(&format!("{name}_a2"), unit, &self.a2),
            Layer::from_f32(&format!("{name}_p2"), "day", &self.p2),
        ]
    }
}

/// Names of the harmonic quantities, with their units.
pub const HARMONIC_QUANTITIES: [(&str, &str); 7] = [
    ("temperature", "C"),
    ("precipitation", "mm/yr"),
    ("humidity", "g/kg"),
    ("wind_u", "m/s"),
    ("wind_v", "m/s"),
    ("snow_fraction", "0-1"),
    ("cloud_cover", "0-1"),
];

/// Output of the climate stage.
#[derive(Clone, Debug)]
pub struct ClimateOutput {
    pub temperature: HarmonicGrid,
    pub precipitation: HarmonicGrid,
    /// Water vapour mixing ratio of the surface air, g/kg.
    pub humidity: HarmonicGrid,
    /// Wind towards +x (east), m/s.
    pub wind_u: HarmonicGrid,
    /// Wind towards +z (south), m/s.
    pub wind_v: HarmonicGrid,
    /// Share of the ground covered by snow.
    pub snow_fraction: HarmonicGrid,
    /// Weather cloud cover (already at least `c_min(s)`).
    pub cloud_cover: HarmonicGrid,
    /// Daily temperature range, °C.
    pub diurnal_range: PeriodicGrid<f32>,
    /// Surface ocean current towards +x, m/s (0 on land).
    pub ocean_current_u: PeriodicGrid<f32>,
    /// Surface ocean current towards +z, m/s.
    pub ocean_current_v: PeriodicGrid<f32>,
    /// Share of each cell covered by the sea.
    pub sea_fraction: PeriodicGrid<f32>,
}

impl ClimateOutput {
    pub fn layers(&self) -> Vec<Layer> {
        let mut v = Vec::new();
        for (name, unit) in HARMONIC_QUANTITIES {
            v.extend(self.quantity(name).layers(name, unit));
        }
        v.push(Layer::from_f32("diurnal_range", "C", &self.diurnal_range));
        v.push(Layer::from_f32(
            "ocean_current_u",
            "m/s",
            &self.ocean_current_u,
        ));
        v.push(Layer::from_f32(
            "ocean_current_v",
            "m/s",
            &self.ocean_current_v,
        ));
        v
    }

    /// Harmonics of a quantity by name (see [`HARMONIC_QUANTITIES`]).
    pub fn quantity(&self, name: &str) -> &HarmonicGrid {
        match name {
            "temperature" => &self.temperature,
            "precipitation" => &self.precipitation,
            "humidity" => &self.humidity,
            "wind_u" => &self.wind_u,
            "wind_v" => &self.wind_v,
            "snow_fraction" => &self.snow_fraction,
            "cloud_cover" => &self.cloud_cover,
            _ => panic!("unknown climate quantity {name}"),
        }
    }
}

/// Static description of the surface on the climate grid.
struct Surface {
    res: usize,
    cell: f64,
    /// Share of sea per cell.
    sea: Vec<f64>,
    /// Mean surface height above sea level (sea and lakes at their surface, ≥ 0), m.
    height: Vec<f64>,
    /// Distance to the sea, m (0 on sea cells).
    coast: Vec<f64>,
}

impl Surface {
    /// Downsamples the fine grid: `elevation` (m) and `is_sea` per fine cell.
    fn new(res: usize, cell: f64, elevation: &PeriodicGrid<f32>, is_sea: &[bool]) -> Self {
        let fine = elevation.width();
        let ratio = fine / res;
        let norm = 1.0 / (ratio * ratio) as f64;
        let mut sea = vec![0.0; res * res];
        let mut height = vec![0.0; res * res];
        sea.par_iter_mut()
            .zip(height.par_iter_mut())
            .enumerate()
            .for_each(|(k, (s, h))| {
                let (i, j) = (k % res, k / res);
                for fj in j * ratio..(j + 1) * ratio {
                    for fi in i * ratio..(i + 1) * ratio {
                        let f = fj * fine + fi;
                        if is_sea[f] {
                            *s += norm;
                        } else {
                            *h += norm * f64::from(elevation.data()[f]).max(0.0);
                        }
                    }
                }
            });
        let seeds: Vec<bool> = sea.iter().map(|&s| s >= 0.5).collect();
        let coast = distance_from(&seeds, res, cell);
        Self {
            res,
            cell,
            sea,
            height,
            coast,
        }
    }

    fn is_sea(&self, k: usize) -> bool {
        self.sea[k] >= 0.5
    }

    /// Maritime share: 1 at sea, decaying inland.
    fn maritime(&self, k: usize, decay: f64) -> f64 {
        let s = self.sea[k];
        s + (1.0 - s) * (-self.coast[k] / decay).det_exp()
    }
}

/// Periodic 8-neighbour Dijkstra distance (m) from the `seeds` cells.
fn distance_from(seeds: &[bool], res: usize, cell: f64) -> Vec<f64> {
    let n = res * res;
    let mut dist = vec![f64::INFINITY; n];
    let mut heap = BinaryHeap::new();
    for (k, &s) in seeds.iter().enumerate() {
        if s {
            dist[k] = 0.0;
            heap.push(Reverse((0u64, k)));
        }
    }
    let diag = cell * std::f64::consts::SQRT_2;
    while let Some(Reverse((bits, k))) = heap.pop() {
        let d = f64::from_bits(bits);
        if d > dist[k] {
            continue;
        }
        let (i, j) = ((k % res) as i64, (k / res) as i64);
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
            let ii = (i + di).rem_euclid(res as i64) as usize;
            let jj = (j + dj).rem_euclid(res as i64) as usize;
            let m = jj * res + ii;
            let nd = d + if di != 0 && dj != 0 { diag } else { cell };
            if nd < dist[m] {
                dist[m] = nd;
                heap.push(Reverse((nd.to_bits(), m)));
            }
        }
    }
    // A world without sea: everything is far inland.
    dist.into_iter()
        .map(|d| if d.is_finite() { d } else { 1e9 })
        .collect()
}

/// Periodic bilinear sample of a row-major `res²` field at world (x, z), cell centres at
/// `(i + 0.5)·cell`.
#[inline]
fn bilinear(f: &[f64], res: usize, cell: f64, x: f64, z: f64) -> f64 {
    let u = x / cell - 0.5;
    let v = z / cell - 0.5;
    let (i0, j0) = (u.floor(), v.floor());
    let (tx, tz) = (u - i0, v - j0);
    let r = res as i64;
    let i0 = (i0 as i64).rem_euclid(r) as usize;
    let j0 = (j0 as i64).rem_euclid(r) as usize;
    let i1 = (i0 + 1) % res;
    let j1 = (j0 + 1) % res;
    let a = f[j0 * res + i0] * (1.0 - tx) + f[j0 * res + i1] * tx;
    let b = f[j1 * res + i0] * (1.0 - tx) + f[j1 * res + i1] * tx;
    a * (1.0 - tz) + b * tz
}

/// Signed latitude (degrees) relative to the ITCZ, which follows `δ_eff`.
fn shifted_latitude(phi_deg: f64, dec_eff: f64, shift: f64) -> f64 {
    (phi_deg - shift * dec_eff.to_degrees()).clamp(-90.0, 90.0)
}

/// Wind (u towards +x, v towards +z), m/s, at signed shifted latitude `lat` (degrees) and seam
/// distance `s`.
fn wind(c: &ClimateConfig, lat: f64, s: f64) -> (f64, f64) {
    let a = lat.abs();
    let east = easterly_weight(a, c.wind_blend_deg);
    let b = c.wind_blend_deg.max(1e-6);
    let polar = smoothstep(60.0 - b, 60.0 + b, a);
    let speed_east = c.trade_wind_m_s * (1.0 - polar) + c.polar_wind_m_s * polar;
    // Zonal: easterlies blow towards −x, westerlies towards +x.
    let u = (1.0 - east) * c.westerly_m_s - east * speed_east;
    // Meridional: easterlies blow towards the equator, westerlies towards the pole. Poleward is
    // −z in the north (φ > 0) and +z in the south. Zero at the ITCZ (convergence).
    let poleward = 0.3 * (1.0 - east) * c.westerly_m_s - 0.4 * east * speed_east;
    let side = lat.signum() * smoothstep(0.0, 5.0, a);
    let v = -side * poleward;
    let calm = smoothstep(0.0, c.seam_calm_m, s);
    (u * calm, v * calm)
}

/// Gaussian-free bump: 1 inside `[lo, hi]` degrees, fading over 5° outside.
fn band(a: f64, lo: f64, hi: f64) -> f64 {
    smoothstep(lo - 5.0, lo, a) * (1.0 - smoothstep(hi, hi + 5.0, a))
}

/// Ocean currents: (anomaly °C per cell, u, v) — static.
fn currents(c: &ClimateConfig, surf: &Surface) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let (res, cell) = (surf.res, surf.cell);
    let reach = (4.0 * c.current_width_m / cell).ceil() as usize;
    let mut anomaly = vec![0.0; res * res];
    let mut u = vec![0.0; res * res];
    let mut v = vec![0.0; res * res];
    anomaly
        .par_chunks_mut(res)
        .zip(u.par_chunks_mut(res))
        .zip(v.par_chunks_mut(res))
        .enumerate()
        .for_each(|(j, ((an, ur), vr))| {
            let z = (j as f64 + 0.5) * cell;
            let phi = latitude(z).to_degrees();
            let a = phi.abs();
            let s = seam_distance(z);
            let (wu, _) = wind(c, phi, s);
            let poleward_z = -phi.signum();
            for i in 0..res {
                let k = j * res + i;
                if !surf.is_sea(k) {
                    continue;
                }
                // Distance to land towards the west and towards the east.
                let walk = |dir: i64| {
                    (1..=reach)
                        .find(|&step| {
                            let ii = (i as i64 + dir * step as i64).rem_euclid(res as i64);
                            !surf.is_sea(j * res + ii as usize)
                        })
                        .map_or(f64::INFINITY, |step| (step as f64 - 0.5) * cell)
                };
                let (dw, de) = (walk(-1), walk(1));
                let west = (-dw / c.current_width_m).det_exp();
                let east = (-de / c.current_width_m).det_exp();
                // Land to the west: western boundary of the basin, warm and poleward.
                let warm = west * band(a, 20.0, 50.0);
                // Land to the east: eastern boundary, cold upwelling, equatorward.
                let cold = east * band(a, 15.0, 35.0);
                an[i] = c.current_warm_c * warm - c.current_cold_c * cold;
                vr[i] = c.current_speed_m_s * poleward_z * (warm - cold);
                ur[i] = 0.1 * wu * (1.0 - warm.max(cold));
            }
        });
    (anomaly, u, v)
}

/// Spreads the sea anomaly onto the nearby land: mean of the sea anomalies within `radius`
/// cells (periodic box), times the maritime share.
fn spread_to_land(anomaly: &[f64], surf: &Surface, radius: usize, decay: f64) -> Vec<f64> {
    let res = surf.res;
    let w: Vec<f32> = (0..res * res)
        .map(|k| if surf.is_sea(k) { 1.0 } else { 0.0 })
        .collect();
    let mut num: Vec<f32> = anomaly.iter().map(|&a| a as f32).collect();
    let mut den = w.clone();
    crate::tectonics::box_blur(&mut num, res, res, radius);
    crate::tectonics::box_blur(&mut den, res, res, radius);
    (0..res * res)
        .map(|k| {
            if surf.is_sea(k) {
                anomaly[k]
            } else if den[k] > 1e-6 {
                f64::from(num[k] / den[k]) * surf.maritime(k, decay)
            } else {
                0.0
            }
        })
        .collect()
}

/// Exact periodic steady state of `dT/dt = (T_eq − T)/τ` for samples of `T_eq` over one year.
fn relax(teq: &[f64], tau_days: f64) -> Vec<f64> {
    let n = teq.len();
    let nf = n as f64;
    // DFT, filter each harmonic by 1 / (1 + i·m·ω·τ), inverse DFT.
    let mut out = vec![0.0; n];
    let mean = teq.iter().sum::<f64>() / nf;
    out.iter_mut().for_each(|o| *o = mean);
    for m in 1..=n / 2 {
        let (mut a, mut b) = (0.0, 0.0);
        for (k, &x) in teq.iter().enumerate() {
            let w = TAU * (m * k) as f64 / nf;
            a += x * w.det_cos();
            b += x * w.det_sin();
        }
        let f = if 2 * m == n { 1.0 / nf } else { 2.0 / nf };
        let (a, b) = (a * f, b * f);
        // X·e^{-iw}… with H = 1/(1 + iθ): amplitude 1/√(1+θ²), lag atan θ.
        let theta = TAU * m as f64 / YEAR * tau_days;
        let gain = 1.0 / (1.0 + theta * theta).sqrt();
        let lag = theta.det_atan();
        for (k, o) in out.iter_mut().enumerate() {
            let w = TAU * (m * k) as f64 / nf - lag;
            *o += gain * (a * w.det_cos() + b * w.det_sin());
        }
    }
    out
}

/// Relaxation time giving a phase lag of `lag_days` on the annual harmonic.
fn tau_for_lag(lag_days: f64) -> f64 {
    (TAU * lag_days / YEAR).det_tan() / (TAU / YEAR)
}

/// Runs the climate stage. `elevation` is the fine grid (m), `is_sea` its sea mask.
pub fn run(
    cfg: &WorldConfig,
    elevation: &PeriodicGrid<f32>,
    is_sea: &[bool],
    progress: &crate::progress::Progress,
) -> ClimateOutput {
    let c = &cfg.climate;
    let astro: AstroConfig = cfg.astro;
    let res = cfg.scale.tectonic_res();
    let cell = cfg.scale.tectonic_cell_m();
    let n = res * res;
    let steps = c.steps as usize;
    let days: Vec<f64> = (0..steps).map(|k| YEAR * k as f64 / steps as f64).collect();
    let surf = Surface::new(res, cell, elevation, is_sea);

    // --- Temperature ---
    let q_eq = days
        .iter()
        .map(|&d| astro.insolation(d, 50_000.0))
        .sum::<f64>()
        / steps as f64;
    let (mut an_sea, mut cur_u, mut cur_v) = currents(c, &surf);
    // The boundary currents are found row by row: smooth them across rows.
    for f in [&mut an_sea, &mut cur_u, &mut cur_v] {
        let mut v: Vec<f32> = f.iter().map(|&x| x as f32).collect();
        for _ in 0..3 {
            crate::tectonics::box_blur(&mut v, res, res, 2);
        }
        for (k, x) in f.iter_mut().enumerate() {
            *x = if surf.is_sea(k) { f64::from(v[k]) } else { 0.0 };
        }
    }
    let reach = ((c.coast_decay_m / cell).round() as usize).max(1);
    let anomaly = spread_to_land(&an_sea, &surf, reach, c.coast_decay_m);
    let (tau_land, tau_sea) = (tau_for_lag(c.lag_land_days), tau_for_lag(c.lag_sea_days));
    // Equilibrium temperature per row and day at sea level (it depends on z only).
    let row_teq: Vec<Vec<f64>> = (0..res)
        .into_par_iter()
        .map(|j| {
            let z = (j as f64 + 0.5) * cell;
            let q: Vec<f64> = days.iter().map(|&d| astro.insolation(d, z)).collect();
            let mean = q.iter().sum::<f64>() / steps as f64;
            let t_mean = zonal(&c.mean_profile, mean / q_eq);
            q.iter()
                .map(|&qd| t_mean + c.seasonal_c_per_w * (qd - mean))
                .collect()
        })
        .collect();
    // (surface temperature, temperature of the land surface: no sea-ice floor, for the snow).
    let temps: Vec<(Vec<f64>, Vec<f64>)> = (0..n)
        .into_par_iter()
        .map(|k| {
            let j = k / res;
            let z = (j as f64 + 0.5) * cell;
            let polar = astro.min_cloud_cover(seam_distance(z));
            let base =
                -c.lapse_rate_c_per_m * surf.height[k] - c.polar_cooling_c * polar + anomaly[k];
            let teq: Vec<f64> = row_teq[j].iter().map(|&t| t + base).collect();
            // Maritime influence: the sea's temperature (lagging, with a damped seasonal
            // swing) blended into the land's by the continentality.
            let m = surf.maritime(k, c.coast_decay_m);
            let on_land = relax(&teq, tau_land);
            let mut on_sea = relax(&teq, tau_sea);
            let mean = on_sea.iter().sum::<f64>() / on_sea.len() as f64;
            on_sea
                .iter_mut()
                .for_each(|v| *v = mean + c.sea_amplitude * (*v - mean));
            let land: Vec<f64> = on_land
                .iter()
                .zip(&on_sea)
                .map(|(l, s)| m * s + (1.0 - m) * l)
                .collect();
            let mut t = land.clone();
            let sea = surf.sea[k];
            if sea > 0.0 {
                for v in &mut t {
                    *v = sea * v.max(c.sea_ice_c) + (1.0 - sea) * *v;
                }
            }
            (t, land)
        })
        .collect();
    let (temp, temp_land): (Vec<Vec<f64>>, Vec<Vec<f64>>) = temps.into_iter().unzip();

    // --- Winds, moisture, precipitation ---
    let mut wind_u = vec![vec![0.0; steps]; n];
    let mut wind_v = vec![vec![0.0; steps]; n];
    let mut precip = vec![vec![0.0; steps]; n];
    let mut humidity = vec![vec![0.0; steps]; n];
    let sea_level = cfg.atmosphere.pressure_hpa(0.0);
    let pressure: Vec<f64> = surf
        .height
        .iter()
        .map(|&h| cfg.atmosphere.pressure_hpa(h))
        .collect();
    let recharge = |dist: f64| 1.0 - (-dist / c.ocean_recharge_m).det_exp();
    for (step, &day) in days.iter().enumerate() {
        progress.set_range(0.1, 0.85, step, steps, 0.0);
        progress.detail(format!("day sample {}/{steps}", step + 1));
        // Per cell: wind, upstream offset, wind speed.
        let per_cell: Vec<(f64, f64, f64, f64, f64)> = (0..n)
            .into_par_iter()
            .map(|k| {
                let z = ((k / res) as f64 + 0.5) * cell;
                let s = seam_distance(z);
                let dec = astro.effective_declination(day, s);
                let lat = shifted_latitude(latitude(z).to_degrees(), dec, c.itcz_shift);
                let (u, v) = wind(c, lat, s);
                let speed = u.det_hypot(v);
                let len = c.advection_step_m * speed / c.advection_ref_wind_m_s;
                let (ox, oz) = if speed > 1e-9 {
                    (-u / speed * len, -v / speed * len)
                } else {
                    (0.0, 0.0)
                };
                (u, v, ox, oz, speed)
            })
            .collect();
        // Saturation mixing ratio at the cell (air temperature, pressure at the surface), and
        // that of marine air at the cell's temperature brought back to sea level.
        let sat: Vec<(f64, f64)> = (0..n)
            .into_par_iter()
            .map(|k| {
                let t = temp_land[k][step];
                let ws = saturation_mixing_ratio(t, pressure[k]);
                let t0 = t + c.lapse_rate_c_per_m * surf.height[k];
                let marine = c.sea_humidity * saturation_mixing_ratio(t0, sea_level);
                (ws, marine)
            })
            .collect();
        // The air is advected relative to the marine moisture of its row (latitude): moving
        // poleward alone does not condense, the zonal profile already holds that rain. Only
        // local departures (relief, land/sea contrast, currents) make it condense.
        let row_ref: Vec<f64> = (0..res)
            .map(|j| {
                let r = &sat[j * res..(j + 1) * res];
                (r.iter().map(|s| s.1).sum::<f64>() / res as f64).max(1e-6)
            })
            .collect();
        let mut w: Vec<f64> = (0..n)
            .map(|k| c.sea_humidity * sat[k].0 / row_ref[k / res])
            .collect();
        let mut condensed = vec![0.0f64; n];
        for _ in 0..c.advection_iterations {
            let next: Vec<(f64, f64)> = (0..n)
                .into_par_iter()
                .map(|k| {
                    let (i, j) = (k % res, k / res);
                    let (x, z) = ((i as f64 + 0.5) * cell, (j as f64 + 0.5) * cell);
                    let (_, _, ox, oz, _) = per_cell[k];
                    let len = ox.det_hypot(oz).max(1.0);
                    let wk = bilinear(&w, res, cell, x + ox, z + oz) * row_ref[j];
                    let ws = sat[k].0;
                    let sea = surf.sea[k];
                    // Sea: recharge towards `sea_humidity · w_s`; land: slow drying.
                    let target = c.sea_humidity * ws;
                    let sea_w = if wk < target {
                        wk + (target - wk) * recharge(len.max(cell))
                    } else {
                        wk
                    };
                    let land_w = wk * (-len / c.land_decay_m).det_exp();
                    let wk = sea * sea_w + (1.0 - sea) * land_w;
                    // Above saturation, the excess condenses and rains out.
                    let excess = (wk - ws).max(0.0);
                    ((wk - excess) / row_ref[j], excess / len)
                })
                .collect();
            for (k, (wk, cond)) in next.into_iter().enumerate() {
                w[k] = wk;
                condensed[k] = cond;
            }
        }
        for k in 0..n {
            let j = k / res;
            let z = (j as f64 + 0.5) * cell;
            let s = seam_distance(z);
            let dec = astro.effective_declination(day, s);
            let lat = shifted_latitude(latitude(z).to_degrees(), dec, c.itcz_shift);
            let pz = zonal(&c.zonal_precip, lat.abs());
            let (u, v, _, _, speed) = per_cell[k];
            let wk = w[k] * row_ref[j];
            let moist = if sat[k].1 > 0.0 {
                (wk / sat[k].1).min(1.0)
            } else {
                1.0
            };
            // Condensation: column mass × wind × condensed g/kg per metre → kg/m²/s → mm/yr.
            // Over the sea the air only cools from below (cold currents): fog, not rain.
            let rain = c.rain_column_kg_m2 * speed * condensed[k] / 1_000.0
                * SECONDS_PER_YEAR
                * (1.0 - surf.sea[k]);
            let p = pz * (c.dry_floor + (1.0 - c.dry_floor) * moist) + rain;
            let polar = c.polar_precip_mm * astro.min_cloud_cover(s);
            wind_u[k][step] = u;
            wind_v[k][step] = v;
            precip[k][step] = p.max(polar);
            humidity[k][step] = wk;
        }
    }

    // --- Clouds, snow, diurnal range ---
    let dt = YEAR / steps as f64;
    let derived: Vec<(Vec<f64>, Vec<f64>, f64)> = (0..n)
        .into_par_iter()
        .map(|k| {
            let z = ((k / res) as f64 + 0.5) * cell;
            let cmin = astro.min_cloud_cover(seam_distance(z));
            let clouds: Vec<f64> = precip[k]
                .iter()
                .map(|&p| (p / c.cloud_full_mm).clamp(c.cloud_min, 1.0).max(cmin))
                .collect();
            // Snowpack balance, repeated until one year matches the previous one. Computed for
            // a land surface everywhere, sea cells included: the renderer samples the cover
            // bilinearly on coastal land and must not see the 160 m cells of the coastline.
            let t = &temp_land[k];
            let mut swe = 0.0f64;
            let mut cover = vec![0.0; steps];
            for _year in 0..6 {
                for step in 0..steps {
                    let snow = 1.0 - smoothstep(c.snow_below_c, c.rain_above_c, t[step]);
                    swe += precip[k][step] / YEAR * dt * snow;
                    swe -= c.melt_mm_per_degree_day * t[step].max(0.0) * dt;
                    swe = swe.clamp(0.0, 1.0e5);
                    cover[step] = (swe / c.snow_full_mm).min(1.0);
                }
            }
            // Permanent snow in the polar band (§4.3).
            cover.iter_mut().for_each(|v| *v = v.max(cmin));
            let p_mean = precip[k].iter().sum::<f64>() / steps as f64;
            let land = 1.0 - surf.maritime(k, c.coast_decay_m);
            let humid = (p_mean / 2_000.0).clamp(0.0, 1.0);
            let land_range = c.diurnal_dry_c + (c.diurnal_wet_c - c.diurnal_dry_c) * humid;
            let range = c.diurnal_sea_c + (land_range - c.diurnal_sea_c) * land;
            (clouds, cover, range)
        })
        .collect();

    let project = |v: &[Vec<f64>]| -> Vec<Harmonics> {
        v.par_iter().map(|s| Harmonics::project(s)).collect()
    };
    let grid = |v: Vec<f64>| {
        PeriodicGrid::from_vec(res, res, cell, v.into_iter().map(|x| x as f32).collect())
    };
    let clouds: Vec<Vec<f64>> = derived.iter().map(|d| d.0.clone()).collect();
    let snow: Vec<Vec<f64>> = derived.iter().map(|d| d.1.clone()).collect();
    ClimateOutput {
        temperature: HarmonicGrid::from_cells(res, cell, &project(&temp)),
        precipitation: HarmonicGrid::from_cells(res, cell, &project(&precip)),
        humidity: HarmonicGrid::from_cells(res, cell, &project(&humidity)),
        wind_u: HarmonicGrid::from_cells(res, cell, &project(&wind_u)),
        wind_v: HarmonicGrid::from_cells(res, cell, &project(&wind_v)),
        snow_fraction: HarmonicGrid::from_cells(res, cell, &project(&snow)),
        cloud_cover: HarmonicGrid::from_cells(res, cell, &project(&clouds)),
        diurnal_range: grid(derived.iter().map(|d| d.2).collect()),
        ocean_current_u: grid(cur_u),
        ocean_current_v: grid(cur_v),
        sea_fraction: grid(surf.sea.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaxation_lags_and_damps() {
        let n = 48;
        let teq: Vec<f64> = (0..n)
            .map(|k| 10.0 + 20.0 * (TAU * k as f64 / n as f64).det_cos())
            .collect();
        let tau = tau_for_lag(30.0);
        let t = relax(&teq, tau);
        let h = Harmonics::project(&t);
        assert!((h.x0 - 10.0).abs() < 1e-9);
        assert!((h.p1 - 30.0).abs() < 1e-6, "lag {}", h.p1);
        assert!(h.a1 < 20.0 && h.a1 > 15.0, "amplitude {}", h.a1);
        // Sea: longer lag, smaller amplitude.
        let hs = Harmonics::project(&relax(&teq, tau_for_lag(52.0)));
        assert!(hs.a1 < h.a1 && (hs.p1 - 52.0).abs() < 1e-6);
    }

    #[test]
    fn winds_by_belt_and_calm_at_the_seam() {
        let c = ClimateConfig::default();
        let far = 20_000.0;
        // Trade winds from the east, towards the equator (north: +z).
        let (u, v) = wind(&c, 15.0, far);
        assert!(u < 0.0 && v > 0.0);
        // Westerlies, poleward (north: −z).
        let (u, v) = wind(&c, 45.0, far);
        assert!(u > 0.0 && v < 0.0);
        // Southern hemisphere mirrors the meridional component.
        let (u, v) = wind(&c, -45.0, far);
        assert!(u > 0.0 && v > 0.0);
        // Polar easterlies; calm at the seam.
        assert!(wind(&c, 80.0, far).0 < 0.0);
        assert_eq!(wind(&c, 89.0, 0.0), (0.0, 0.0));
    }

    #[test]
    fn distance_wraps_around_the_seam() {
        let res = 20;
        let mut seeds = vec![false; res * res];
        seeds[0] = true;
        let d = distance_from(&seeds, res, 100.0);
        assert_eq!(d[res - 1], 100.0);
        assert_eq!(d[(res - 1) * res], 100.0);
        assert!((d[(res - 1) * res + res - 1] - 100.0 * std::f64::consts::SQRT_2).abs() < 1e-9);
    }

    #[test]
    fn bilinear_is_periodic() {
        let res = 10;
        let f: Vec<f64> = (0..res * res).map(|k| (k % res) as f64).collect();
        let a = bilinear(&f, res, 10.0, 0.0, 5.0);
        let b = bilinear(&f, res, 10.0, 100.0, 5.0);
        assert!((a - b).abs() < 1e-12);
        assert!((a - 4.5).abs() < 1e-12);
    }
}
