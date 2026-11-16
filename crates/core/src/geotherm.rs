//! Temperature of the rock below the surface (spec §4.6).
//!
//! From the rock surface (temperature `T_s`) to the Moho (`moho_c`, 1 200 °C) the temperature
//! follows an exponential curve of the relative depth `u = d / D` (`D`: crust thickness):
//!
//! `T(d) = T_s + (moho_c − T_s) · (e^(k·u) − 1) / (e^k − 1)`
//!
//! Gentle near the surface and steep near the Moho: with `k` = 2.7, about 50 °C at 500 m under
//! an ordinary continent (`D` = 3 500 m, `T_s` ≈ 10 °C), about 250 °C halfway down, and about
//! 1 °C per metre just above the Moho. `T_s` is the mean annual air temperature on land; under
//! water it goes from the surface water to the bottom water over `deep_water_m`.

use serde::{Deserialize, Serialize};

/// Sea water freezes at about −1.8 °C: the sea floor is never colder.
const SEA_WATER_FREEZING_C: f64 = -1.8;

/// Tunable parameters of the underground temperature (`[geotherm]` in TOML).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeothermConfig {
    /// Temperature at the Moho (top of the magma), °C.
    pub moho_c: f64,
    /// Steepness `k` of the curve (0: linear).
    pub steepness: f64,
    /// Temperature of the bottom water of deep seas, °C.
    pub deep_water_c: f64,
    /// Below this water depth, the sea floor has the bottom-water temperature, m.
    pub deep_water_m: f64,
}

impl Default for GeothermConfig {
    fn default() -> Self {
        Self {
            moho_c: 1_200.0,
            steepness: 2.7,
            deep_water_c: 2.0,
            deep_water_m: 200.0,
        }
    }
}

impl GeothermConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(self.moho_c > 100.0 && self.steepness >= 0.0 && self.deep_water_m > 0.0) {
            return Err("moho_c must be > 100, steepness >= 0 and deep_water_m > 0".into());
        }
        Ok(())
    }

    /// Share of the way from the surface temperature to the Moho temperature at relative depth
    /// `u` (clamped to `[0, 1]`).
    pub fn shape(&self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        let k = self.steepness;
        if k < 1e-6 {
            u
        } else {
            (k * u).exp_m1() / k.exp_m1()
        }
    }

    /// Rock temperature (°C) at `depth` m below the rock surface, for a crust `crust` m thick
    /// and a surface temperature `surface_c`. Above the surface: `surface_c`; below the Moho:
    /// `moho_c` (magma).
    pub fn temperature(&self, surface_c: f64, depth: f64, crust: f64) -> f64 {
        let u = if crust > 0.0 { depth / crust } else { 1.0 };
        surface_c + (self.moho_c - surface_c) * self.shape(u)
    }

    /// Depth (m below the rock surface) where the rock reaches `t` °C; 0 if the surface is
    /// already that warm, `crust` if only the magma is.
    pub fn depth_of(&self, surface_c: f64, crust: f64, t: f64) -> f64 {
        let f = ((t - surface_c) / (self.moho_c - surface_c)).clamp(0.0, 1.0);
        let k = self.steepness;
        let u = if k < 1e-6 {
            f
        } else {
            (f * k.exp_m1()).ln_1p() / k
        };
        u * crust
    }

    /// Temperature of the rock surface: the mean annual air temperature `air_mean_c` on land;
    /// under `water_depth` m of water, from the surface water to the bottom water.
    pub fn surface_c(&self, air_mean_c: f64, water_depth: f64) -> f64 {
        if water_depth <= 0.0 {
            return air_mean_c;
        }
        let top = air_mean_c.max(SEA_WATER_FREEZING_C);
        let a = (water_depth / self.deep_water_m).clamp(0.0, 1.0);
        top + (self.deep_water_c - top) * a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gentle_near_the_surface_steep_near_the_moho() {
        let g = GeothermConfig::default();
        // Ordinary continent: about 50 °C at 500 m.
        let t500 = g.temperature(10.0, 500.0, 3_500.0);
        assert!((t500 - 50.0).abs() < 2.0, "{t500}");
        assert!(g.temperature(10.0, 0.0, 3_500.0) == 10.0);
        assert!((g.temperature(10.0, 3_500.0, 3_500.0) - 1_200.0).abs() < 1e-9);
        // Gradients: below 0.1 °C/m at the top, about 1 °C/m at the bottom.
        let top = g.temperature(10.0, 10.0, 3_500.0) - 10.0;
        let bottom = 1_200.0 - g.temperature(10.0, 3_490.0, 3_500.0);
        assert!(top < 1.0 && (8.0..12.0).contains(&bottom), "{top} {bottom}");
        // Monotonic, and the inverse matches.
        let mut last = f64::NEG_INFINITY;
        for k in 0..=70 {
            let d = f64::from(k) * 50.0;
            let t = g.temperature(10.0, d, 3_500.0);
            assert!(t > last);
            last = t;
            assert!((g.depth_of(10.0, 3_500.0, t) - d).abs() < 1e-6);
        }
        // Thin oceanic crust: hotter at the same depth; thick orogen: cooler.
        assert!(g.temperature(2.0, 300.0, 700.0) > g.temperature(10.0, 300.0, 3_500.0));
        assert!(g.temperature(0.0, 500.0, 8_000.0) < t500);
    }

    #[test]
    fn sea_floor_has_bottom_water() {
        let g = GeothermConfig::default();
        assert_eq!(g.surface_c(14.0, 0.0), 14.0);
        assert_eq!(g.surface_c(25.0, 1_000.0), 2.0);
        assert!((g.surface_c(-20.0, 50.0) - (-1.8 + 3.8 * 0.25)).abs() < 1e-9);
        let lin = GeothermConfig {
            steepness: 0.0,
            ..g
        };
        assert!((lin.temperature(0.0, 350.0, 700.0) - 600.0).abs() < 1e-9);
        assert!(g.validate().is_ok());
    }
}
