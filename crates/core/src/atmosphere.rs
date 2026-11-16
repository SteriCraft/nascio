//! Atmosphere: pressure against altitude and water vapour saturation (spec §4.3).
//!
//! Nascio's atmosphere follows the pressure profile of Earth's standard atmosphere (US Standard
//! Atmosphere 1976), compressed vertically so that the Kármán line (100 km on Earth) sits at
//! `karman_line_m` (50 km by default): a Nascio altitude `h` reads Earth's profile at
//! `h · 100 km / karman_line_m`. The pressure is forced to zero at the Kármán line (a tiny linear
//! correction, below 0.001 hPa) and stays zero above it.
//!
//! The temperature lapse rate is **not** compressed (−6.5 °C/km, see the climate): only the
//! pressure follows the compressed profile. Above the ground the air cools with the lapse rate
//! down to `tropopause_c` (then stays there), keeps the mixing ratio of the surface air, and the
//! diurnal cycle fades over the boundary layer ([`AtmosphereConfig::air`]).

use serde::{Deserialize, Serialize};

/// Earth's Kármán line, m.
const EARTH_KARMAN_M: f64 = 100_000.0;

/// US Standard Atmosphere 1976: `[geometric altitude km, pressure hPa]`, ascending altitudes.
const EARTH_PRESSURE: [[f64; 2]; 41] = [
    [0.0, 1013.25],
    [1.0, 898.76],
    [2.0, 795.01],
    [3.0, 701.21],
    [4.0, 616.60],
    [5.0, 540.48],
    [6.0, 472.17],
    [7.0, 411.05],
    [8.0, 356.51],
    [9.0, 308.00],
    [10.0, 264.99],
    [11.0, 226.99],
    [12.0, 193.99],
    [13.0, 165.79],
    [14.0, 141.70],
    [15.0, 121.11],
    [16.0, 103.52],
    [17.0, 88.497],
    [18.0, 75.652],
    [19.0, 64.674],
    [20.0, 55.293],
    [22.0, 40.475],
    [24.0, 29.717],
    [26.0, 21.883],
    [28.0, 16.161],
    [30.0, 11.970],
    [32.0, 8.8906],
    [35.0, 5.7459],
    [40.0, 2.8714],
    [45.0, 1.4910],
    [50.0, 0.79779],
    [55.0, 0.42525],
    [60.0, 0.21958],
    [65.0, 0.10929],
    [70.0, 0.052209],
    [75.0, 0.023881],
    [80.0, 0.010524],
    [85.0, 0.0044568],
    [90.0, 0.0018359],
    [95.0, 0.00076],
    [100.0, 0.00032011],
];

/// Atmosphere parameters (`[atmosphere]` in TOML).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AtmosphereConfig {
    /// Pressure at sea level, hPa.
    pub sea_level_hpa: f64,
    /// Altitude of the Kármán line, where the pressure reaches zero, m.
    pub karman_line_m: f64,
    /// The air never cools below this temperature with altitude (tropopause), °C.
    pub tropopause_c: f64,
    /// Depth over which the diurnal temperature cycle fades above the ground, m.
    pub boundary_layer_m: f64,
}

impl Default for AtmosphereConfig {
    fn default() -> Self {
        Self {
            sea_level_hpa: 1013.25,
            karman_line_m: 50_000.0,
            tropopause_c: -56.5,
            boundary_layer_m: 1_000.0,
        }
    }
}

impl AtmosphereConfig {
    /// Sanity checks.
    pub fn validate(&self) -> Result<(), String> {
        if !(self.sea_level_hpa > 0.0 && self.karman_line_m > 0.0 && self.boundary_layer_m > 0.0) {
            return Err("sea_level_hpa, karman_line_m and boundary_layer_m must be > 0".into());
        }
        Ok(())
    }

    /// Air pressure at altitude `h` (m, may be negative), hPa.
    pub fn pressure_hpa(&self, h: f64) -> f64 {
        let earth_km = h * EARTH_KARMAN_M / self.karman_line_m / 1_000.0;
        if earth_km >= 100.0 {
            return 0.0;
        }
        let top = EARTH_PRESSURE[EARTH_PRESSURE.len() - 1][1];
        let p = earth_pressure(earth_km) - top * earth_km.max(0.0) / 100.0;
        p * self.sea_level_hpa / EARTH_PRESSURE[0][1]
    }
}

/// Air at one point: temperature, relative humidity and pressure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Air {
    pub temperature_c: f64,
    /// Relative humidity, 0–1.
    pub humidity: f64,
    pub pressure_hpa: f64,
}

/// Surface climate at one place and instant, input of [`AtmosphereConfig::air`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceClimate {
    /// Daily mean air temperature at the ground, °C.
    pub temperature_c: f64,
    /// Mixing ratio of the surface air, g/kg.
    pub mixing_ratio: f64,
    /// Daily temperature range at the ground, °C.
    pub diurnal_range_c: f64,
    /// Height of the ground (or of the water surface), m.
    pub ground_m: f64,
}

impl AtmosphereConfig {
    /// Air at altitude `h` above a place with surface climate `s`, at local solar day fraction
    /// `local_frac`, for a lapse rate `lapse` (°C per m).
    pub fn air(&self, s: &SurfaceClimate, h: f64, local_frac: f64, lapse: f64) -> Air {
        let above = (h - s.ground_m).max(0.0);
        let fade = (1.0 - above / self.boundary_layer_m).clamp(0.0, 1.0);
        let diurnal = crate::climate::diurnal_offset(s.diurnal_range_c, local_frac) * fade;
        let t = (s.temperature_c + diurnal - lapse * (h - s.ground_m)).max(self.tropopause_c);
        let p = self.pressure_hpa(h);
        Air {
            temperature_c: t,
            humidity: relative_humidity(s.mixing_ratio, t, p),
            pressure_hpa: p,
        }
    }
}

/// Earth's standard pressure (hPa) at `km`, log-linear between the table rows; below sea level
/// the first layer's scale height is extended.
fn earth_pressure(km: f64) -> f64 {
    let t = &EARTH_PRESSURE;
    let i = t.partition_point(|r| r[0] <= km).clamp(1, t.len() - 1);
    let (a, b) = (t[i - 1], t[i]);
    let f = (km - a[0]) / (b[0] - a[0]);
    (a[1].ln() + f * (b[1].ln() - a[1].ln())).exp()
}

/// Saturation vapour pressure (hPa) at `t_c` °C: Magnus formulas over water above 0 °C and over
/// ice below.
#[inline]
pub fn saturation_vapour_hpa(t_c: f64) -> f64 {
    if t_c >= 0.0 {
        6.112 * (17.62 * t_c / (243.12 + t_c)).exp()
    } else {
        6.112 * (22.46 * t_c / (272.62 + t_c)).exp()
    }
}

/// Mixing ratio of water vapour at saturation (g of water per kg of dry air) at `t_c` °C and
/// pressure `p_hpa`. The vapour pressure is capped at half the air pressure, so the value stays
/// finite in the thinnest air.
#[inline]
pub fn saturation_mixing_ratio(t_c: f64, p_hpa: f64) -> f64 {
    let e = saturation_vapour_hpa(t_c).min(0.5 * p_hpa);
    if p_hpa <= 0.0 {
        return 0.0;
    }
    622.0 * e / (p_hpa - e)
}

/// Relative humidity (0–1) of air holding `w` g/kg at `t_c` °C and `p_hpa`.
#[inline]
pub fn relative_humidity(w: f64, t_c: f64, p_hpa: f64) -> f64 {
    let ws = saturation_mixing_ratio(t_c, p_hpa);
    if ws <= 0.0 {
        return 0.0;
    }
    (w / ws).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_earth_profile() {
        let a = AtmosphereConfig::default();
        let near = |h: f64, p: f64, tol: f64| {
            let got = a.pressure_hpa(h);
            assert!((got - p).abs() < tol, "{h} m: {got} hPa, expected {p}");
        };
        near(0.0, 1013.25, 1e-9);
        near(1_000.0, 795.0, 0.5);
        near(1_500.0, 701.2, 0.5);
        near(2_000.0, 616.6, 0.5);
        near(3_000.0, 472.2, 0.5);
        near(25_000.0, 0.798, 0.01);
        assert_eq!(a.pressure_hpa(50_000.0), 0.0);
        assert_eq!(a.pressure_hpa(80_000.0), 0.0);
        assert!(a.pressure_hpa(49_999.0) >= 0.0);
        assert!(a.pressure_hpa(-500.0) > 1013.25);
        // Monotonic.
        let mut last = f64::MAX;
        for k in 0..=1_000 {
            let p = a.pressure_hpa(k as f64 * 50.0);
            assert!(p <= last, "{}", k * 50);
            last = p;
        }
    }

    #[test]
    fn air_cools_and_saturates_with_altitude() {
        let a = AtmosphereConfig::default();
        let s = SurfaceClimate {
            temperature_c: 15.0,
            mixing_ratio: 7.0,
            diurnal_range_c: 10.0,
            ground_m: 200.0,
        };
        let noon = a.air(&s, 200.0, 0.5, 0.0065);
        let night = a.air(&s, 200.0, 0.0, 0.0065);
        let peak = a.air(&s, 200.0, crate::climate::DIURNAL_PEAK_FRAC, 0.0065);
        assert!((peak.temperature_c - 20.0).abs() < 1e-9);
        assert!(night.temperature_c < noon.temperature_c);
        assert!(night.humidity > noon.humidity, "cool nights are more humid");
        // 1 km up: lapse rate, no diurnal cycle left, lower pressure, higher humidity.
        let up = a.air(&s, 1_200.0, crate::climate::DIURNAL_PEAK_FRAC, 0.0065);
        assert!((up.temperature_c - (15.0 - 6.5)).abs() < 1e-9);
        assert!(up.pressure_hpa < peak.pressure_hpa && up.humidity > peak.humidity);
        // Very high: tropopause floor, saturated (w capped), zero pressure past the Kármán line.
        let top = a.air(&s, 30_000.0, 0.5, 0.0065);
        assert_eq!(top.temperature_c, a.tropopause_c);
        assert_eq!(a.air(&s, 60_000.0, 0.5, 0.0065).pressure_hpa, 0.0);
    }

    #[test]
    fn saturation_follows_clausius_clapeyron() {
        assert!((saturation_vapour_hpa(0.0) - 6.112).abs() < 1e-9);
        assert!((saturation_vapour_hpa(20.0) - 23.4).abs() < 0.2);
        // Continuous at the water/ice switch.
        assert!((saturation_vapour_hpa(-1e-9) - saturation_vapour_hpa(0.0)).abs() < 1e-6);
        let w = saturation_mixing_ratio(15.0, 1013.25);
        assert!((w - 10.7).abs() < 0.2, "{w}");
        // Thinner air holds more vapour per kg at the same temperature.
        assert!(saturation_mixing_ratio(15.0, 600.0) > w);
        assert_eq!(saturation_mixing_ratio(15.0, 0.0), 0.0);
        assert!((relative_humidity(w / 2.0, 15.0, 1013.25) - 0.5).abs() < 1e-12);
    }
}
