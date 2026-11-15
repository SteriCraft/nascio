//! Sun position, seam twilight band and daily insolation (spec §2.4 – §2.6).
//!
//! Conventions (see `docs/decisions.md`):
//! - angles are in radians unless a name ends in `_deg`;
//! - azimuth `A` is measured from north (−z) towards east (+x), in `[0, 2π)`;
//! - the sun direction in world space (x east, y up, z south) is
//!   `(cos e · sin A, sin e, −cos e · cos A)`.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use serde::{Deserialize, Serialize};

use crate::consts::{DAYS_PER_YEAR, OBLIQUITY_DEG, SOLAR_CONSTANT};
use crate::geo::{abs_latitude_from_seam, latitude, seam_distance};
use crate::math::smoothstep;
use crate::time::GameTime;

/// Margin by which the sun is kept below the horizon in the seam band: 4°.
pub const SEAM_SUN_MARGIN_DEG: f64 = 4.0;
/// Direct light fades in between `e_vis = 0°` and this elevation.
pub const LIGHT_FADE_DEG: f64 = 3.0;

/// Obliquity ε in radians.
#[inline]
pub fn obliquity() -> f64 {
    OBLIQUITY_DEG.to_radians()
}

/// Tunable astronomy parameters (stored in `world.toml`). Distances are in metres from the seam.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AstroConfig {
    /// Seasons fade to zero below this seam distance (`δ_eff = δ · smoothstep(0, s_fade, s)`).
    pub s_fade: f64,
    /// Below this seam distance the sun is always at least 4° under the horizon.
    pub s_dark: f64,
    /// Above this seam distance the visible sun equals the geometric sun.
    pub s_light: f64,
}

impl Default for AstroConfig {
    fn default() -> Self {
        Self {
            s_fade: 3_000.0,
            s_dark: 1_000.0,
            s_light: 2_000.0,
        }
    }
}

/// Solar declination δ(N) = −ε · cos(2π (N + 10) / 365), N the continuous day of year.
#[inline]
pub fn declination(n: f64) -> f64 {
    -obliquity() * (TAU * (n + 10.0) / DAYS_PER_YEAR as f64).cos()
}

impl AstroConfig {
    /// Season weight `w(s) = smoothstep(0, S_FADE, s)`.
    #[inline]
    pub fn season_weight(&self, s: f64) -> f64 {
        smoothstep(0.0, self.s_fade, s)
    }

    /// Effective declination `δ_eff = δ(N) · w(s)`.
    #[inline]
    pub fn effective_declination(&self, n: f64, s: f64) -> f64 {
        declination(n) * self.season_weight(s)
    }

    /// Twilight-band ratio `r(s) = 1 − smoothstep(S_DARK, S_LIGHT, s)`.
    #[inline]
    pub fn darkness_ratio(&self, s: f64) -> f64 {
        1.0 - smoothstep(self.s_dark, self.s_light, s)
    }

    /// Upper bound of the noon elevation: `E_max(s) = 90° − |φ(s)| + ε · w(s)`.
    #[inline]
    pub fn max_noon_elevation(&self, s: f64) -> f64 {
        FRAC_PI_2 - abs_latitude_from_seam(s) + obliquity() * self.season_weight(s)
    }

    /// Visible elevation `e_vis = e − r(s) · (E_max(s) + 4°)` (rendering only).
    #[inline]
    pub fn visible_elevation(&self, e: f64, s: f64) -> f64 {
        let r = self.darkness_ratio(s);
        if r == 0.0 {
            e
        } else {
            e - r * (self.max_noon_elevation(s) + SEAM_SUN_MARGIN_DEG.to_radians())
        }
    }

    /// Sun position at continuous day of year `n`, hour angle `h` and world z.
    pub fn sun_at(&self, n: f64, h: f64, z: f64) -> SunPosition {
        let s = seam_distance(z);
        let phi = latitude(z);
        let dec = self.effective_declination(n, s);
        let elevation = elevation(phi, dec, h);
        SunPosition {
            elevation,
            azimuth: azimuth(phi, dec, h),
            visible_elevation: self.visible_elevation(elevation, s),
        }
    }

    /// Sun position at a game instant and world position (x, z).
    pub fn sun(&self, time: GameTime, x: f64, z: f64) -> SunPosition {
        self.sun_at(time.day_of_year(), time.hour_angle(x), z)
    }

    /// Daily mean insolation Q (W/m²) at continuous day of year `n` and world z.
    pub fn insolation(&self, n: f64, z: f64) -> f64 {
        daily_insolation(latitude(z), self.effective_declination(n, seam_distance(z)))
    }

    /// Aurora intensity in `[0, 1]`: `(1 − smoothstep(S_DARK, S_LIGHT + 1000, s)) · night`.
    #[inline]
    pub fn aurora_intensity(&self, s: f64, night_factor: f64) -> f64 {
        (1.0 - smoothstep(self.s_dark, self.s_light + 1_000.0, s)) * night_factor.clamp(0.0, 1.0)
    }
}

/// Position of the sun in the local sky.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunPosition {
    /// Geometric elevation `e` (physics, climate).
    pub elevation: f64,
    /// Azimuth from north (−z) towards east (+x), in `[0, 2π)`.
    pub azimuth: f64,
    /// Visible elevation `e_vis` (rendering). Equal to `elevation` away from the seam.
    pub visible_elevation: f64,
}

impl SunPosition {
    /// Unit vector towards the visible sun, in world axes (x east, y up, z south).
    pub fn direction(&self) -> [f64; 3] {
        direction(self.visible_elevation, self.azimuth)
    }

    /// Directional light intensity in `[0, 1]` (0 when the visible sun is below the horizon).
    pub fn light_intensity(&self) -> f64 {
        light_intensity(self.visible_elevation)
    }
}

/// Geometric elevation: `sin e = sin φ sin δ + cos φ cos δ cos H`.
#[inline]
pub fn elevation(phi: f64, dec: f64, h: f64) -> f64 {
    (phi.sin() * dec.sin() + phi.cos() * dec.cos() * h.cos())
        .clamp(-1.0, 1.0)
        .asin()
}

/// Azimuth from north towards east, in `[0, 2π)`. Morning (`H < 0`) gives an eastern azimuth.
#[inline]
pub fn azimuth(phi: f64, dec: f64, h: f64) -> f64 {
    let y = -dec.cos() * h.sin();
    let x = dec.sin() * phi.cos() - dec.cos() * h.cos() * phi.sin();
    let a = y.atan2(x).rem_euclid(TAU);
    if a >= TAU { 0.0 } else { a }
}

/// World-space unit vector for an elevation and azimuth (x east, y up, z south).
#[inline]
pub fn direction(elevation: f64, azimuth: f64) -> [f64; 3] {
    let c = elevation.cos();
    [c * azimuth.sin(), elevation.sin(), -c * azimuth.cos()]
}

/// Directional light intensity: 0 for `e_vis ≤ 0`, smooth fade up to 1 at +3°.
#[inline]
pub fn light_intensity(visible_elevation: f64) -> f64 {
    smoothstep(0.0, LIGHT_FADE_DEG.to_radians(), visible_elevation)
}

/// Daily mean top-of-atmosphere insolation (W/m²) at latitude φ and declination δ:
/// `Q = (S0/π)(h0 sin φ sin δ + cos φ cos δ sin h0)`, `h0 = acos(clamp(−tan φ tan δ, −1, 1))`.
pub fn daily_insolation(phi: f64, dec: f64) -> f64 {
    let phi = phi.clamp(-FRAC_PI_2, FRAC_PI_2);
    let x = -phi.tan() * dec.tan();
    // tan(±π/2) is finite in f64, but guard anyway: 0 · ∞ would be NaN.
    let x = if x.is_nan() { 0.0 } else { x.clamp(-1.0, 1.0) };
    let h0 = x.acos();
    let q = SOLAR_CONSTANT / PI * (h0 * phi.sin() * dec.sin() + phi.cos() * dec.cos() * h0.sin());
    q.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::WORLD_SIZE_F64;
    use crate::geo::{z_north, z_south};
    use proptest::prelude::*;

    const JUNE_SOLSTICE: f64 = 172.5;
    const DECEMBER_SOLSTICE: f64 = 355.0;
    const MARCH_EQUINOX: f64 = 81.25;
    const SEPTEMBER_EQUINOX: f64 = 263.75;

    fn deg(r: f64) -> f64 {
        r.to_degrees()
    }

    fn cfg() -> AstroConfig {
        AstroConfig::default()
    }

    #[test]
    fn declination_landmarks() {
        assert!((deg(declination(JUNE_SOLSTICE)) - 23.44).abs() < 1e-9);
        assert!((deg(declination(DECEMBER_SOLSTICE)) + 23.44).abs() < 1e-9);
        assert!(deg(declination(MARCH_EQUINOX)).abs() < 1e-9);
        assert!(deg(declination(SEPTEMBER_EQUINOX)).abs() < 1e-9);
        // Northern spring between March and June: declination rises.
        assert!(declination(120.0) > 0.0);
    }

    #[test]
    fn equator_equinox_noon_is_zenith() {
        // N = 81.25 ⇒ day_frac = 0.25 at x = 0, so local noon is at x = 25 000.
        let t = GameTime::from_year_day(0, MARCH_EQUINOX);
        assert!(t.hour_angle(25_000.0).abs() < 1e-9);
        let sun = cfg().sun(t, 25_000.0, 50_000.0);
        assert!((deg(sun.elevation) - 90.0).abs() < 1e-4);
        assert_eq!(sun.elevation, sun.visible_elevation);
    }

    #[test]
    fn noon_sun_is_south_in_north_and_north_in_south() {
        let c = cfg();
        let north = c.sun_at(SEPTEMBER_EQUINOX, 0.0, 30_000.0);
        let south = c.sun_at(SEPTEMBER_EQUINOX, 0.0, 70_000.0);
        assert!((north.azimuth - PI).abs() < 1e-9);
        assert!(south.azimuth < 1e-9 || south.azimuth > TAU - 1e-9);
        // South is +z in world space.
        assert!(north.direction()[2] > 0.0 && south.direction()[2] < 0.0);
    }

    #[test]
    fn sun_rises_in_the_east_in_both_hemispheres() {
        let c = cfg();
        for z in [10_000.0, 30_000.0, 50_000.0, 70_000.0, 90_000.0] {
            for n in [0.0, 81.25, JUNE_SOLSTICE, 263.75, DECEMBER_SOLSTICE] {
                // Scan the morning for the rising crossing of the horizon.
                let mut prev = c.sun_at(n, -PI, z);
                let mut found = false;
                for i in 1..=2_000 {
                    let h = -PI + PI * f64::from(i) / 2_000.0;
                    let cur = c.sun_at(n, h, z);
                    if prev.elevation < 0.0 && cur.elevation >= 0.0 {
                        let d = cur.direction();
                        assert!(d[0] > 0.0, "z={z} n={n}: sunrise not east, dir={d:?}");
                        found = true;
                        break;
                    }
                    prev = cur;
                }
                assert!(found, "z={z} n={n}: no sunrise");
            }
        }
    }

    #[test]
    fn sun_is_on_the_horizon_on_the_seam() {
        let c = cfg();
        for i in 0..1_000 {
            let n = 365.0 * f64::from(i) / 1_000.0;
            let h = -PI + TAU * f64::from(i * 7 % 1_000) / 1_000.0;
            for z in [0.0, WORLD_SIZE_F64] {
                let sun = c.sun_at(n, h, z);
                assert!(sun.elevation.abs() < 1e-9);
                assert!((deg(sun.visible_elevation) + 4.0).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn polar_day_and_night_near_polar_circle() {
        let c = cfg();
        let s = 3_600.0; // inside the polar circle (4 125 m), beyond S_FADE.
        let min_north = (0..=360)
            .map(|i| {
                c.sun_at(JUNE_SOLSTICE, -PI + TAU * f64::from(i) / 360.0, z_north(s))
                    .elevation
            })
            .fold(f64::INFINITY, f64::min);
        let max_south = (0..=360)
            .map(|i| {
                c.sun_at(JUNE_SOLSTICE, -PI + TAU * f64::from(i) / 360.0, z_south(s))
                    .elevation
            })
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(min_north > 0.0, "no polar day: {}", deg(min_north));
        assert!(max_south < 0.0, "no polar night: {}", deg(max_south));
    }

    #[test]
    fn noon_elevation_bounded_by_e_max() {
        let c = cfg();
        for si in 0..=500 {
            let s = 50_000.0 * f64::from(si) / 500.0;
            for ni in 0..73 {
                let n = 5.0 * f64::from(ni);
                for z in [z_north(s), z_south(s)] {
                    let e = c.sun_at(n, 0.0, z).elevation;
                    assert!(e <= c.max_noon_elevation(s) + 1e-12, "s={s} n={n}");
                }
            }
        }
    }

    #[test]
    fn e_vis_continuous_in_s() {
        // Fine sampling: a discontinuity would show as a jump independent of the step.
        let c = cfg();
        for &(n, h) in &[
            (0.0, 0.0),
            (JUNE_SOLSTICE, 0.0),
            (DECEMBER_SOLSTICE, 1.0),
            (100.0, -2.5),
            (200.0, PI - 0.01),
        ] {
            for hemisphere in [z_north as fn(f64) -> f64, z_south] {
                let step = 0.01;
                let mut prev = c.sun_at(n, h, hemisphere(0.0));
                for i in 1..=500_000 {
                    let s = step * f64::from(i);
                    let cur = c.sun_at(n, h, hemisphere(s));
                    let jump_e = (cur.elevation - prev.elevation).abs();
                    let jump_v = (cur.visible_elevation - prev.visible_elevation).abs();
                    assert!(deg(jump_e) < 0.1 && deg(jump_v) < 0.1, "n={n} h={h} s={s}");
                    prev = cur;
                }
            }
        }
    }

    #[test]
    fn e_vis_continuous_in_t() {
        // 0.1 s steps over a full day, at several latitudes, plus the year boundary.
        let c = cfg();
        for z in [0.0, 500.0, 1_500.0, 2_500.0, 30_000.0, 50_000.0, 98_500.0] {
            for start in [0.0, 364.0 * 3_600.0, 100.25 * 3_600.0] {
                let mut prev = c.sun(GameTime::from_seconds(start), 12_345.0, z);
                for i in 1..=36_000 {
                    let t = GameTime::from_seconds(start + 0.1 * f64::from(i));
                    let cur = c.sun(t, 12_345.0, z);
                    assert!(deg((cur.visible_elevation - prev.visible_elevation).abs()) < 0.02);
                    assert!(deg((cur.elevation - prev.elevation).abs()) < 0.02);
                    prev = cur;
                }
            }
        }
    }

    #[test]
    fn insolation_annual_mean_peaks_at_equator() {
        let c = cfg();
        let annual_mean = |z: f64| {
            (0..365)
                .map(|n| c.insolation(f64::from(n) + 0.5, z))
                .sum::<f64>()
                / 365.0
        };
        let equator = annual_mean(50_000.0);
        for zi in 0..200 {
            let z = 500.0 * f64::from(zi);
            if (z - 50_000.0).abs() > 1.0 {
                assert!(annual_mean(z) < equator, "z={z}");
            }
        }
        assert!(equator > 400.0 && equator < 440.0, "{equator}");
    }

    #[test]
    fn insolation_stronger_in_summer_hemisphere() {
        let c = cfg();
        for s in [5_000.0, 10_000.0, 20_000.0, 35_000.0, 45_000.0] {
            let (north, south) = (z_north(s), z_south(s));
            assert!(
                c.insolation(JUNE_SOLSTICE, north) > c.insolation(JUNE_SOLSTICE, south),
                "s={s}"
            );
            assert!(
                c.insolation(DECEMBER_SOLSTICE, south) > c.insolation(DECEMBER_SOLSTICE, north),
                "s={s}"
            );
        }
    }

    #[test]
    fn insolation_finite_and_near_zero_on_seam() {
        let c = cfg();
        for ni in 0..365 {
            let n = f64::from(ni) + 0.5;
            for z in [0.0, 1e-9, 0.01, WORLD_SIZE_F64 - 0.01, WORLD_SIZE_F64] {
                let q = c.insolation(n, z);
                assert!(q.is_finite() && q < 5.0, "n={n} z={z} q={q}");
            }
            for zi in 0..1_000 {
                assert!(c.insolation(n, 100.0 * f64::from(zi)).is_finite());
            }
        }
        // Raw function at the exact pole with any declination.
        for d in [-0.4, 0.0, 0.4] {
            assert!(daily_insolation(FRAC_PI_2, d).is_finite());
            assert!(daily_insolation(-FRAC_PI_2, d).is_finite());
        }
    }

    #[test]
    fn light_and_aurora() {
        let c = cfg();
        assert_eq!(light_intensity(0.0), 0.0);
        assert_eq!(light_intensity(-0.1), 0.0);
        assert_eq!(light_intensity(3f64.to_radians()), 1.0);
        assert_eq!(c.aurora_intensity(0.0, 1.0), 1.0);
        assert_eq!(c.aurora_intensity(1_000.0, 1.0), 1.0);
        assert_eq!(c.aurora_intensity(3_000.0, 1.0), 0.0);
        assert_eq!(c.aurora_intensity(500.0, 0.0), 0.0);
    }

    #[test]
    fn config_has_serde_defaults() {
        // Missing fields fall back to defaults.
        let c: AstroConfig =
            serde::Deserialize::deserialize(serde::de::value::MapDeserializer::<
                _,
                serde::de::value::Error,
            >::new(std::iter::once(("s_dark", 900.0))))
            .unwrap();
        assert_eq!(c.s_dark, 900.0);
        assert_eq!(c.s_fade, 3_000.0);
    }

    proptest! {
        #[test]
        fn seam_band_is_dark(s in 0.0f64..=1_000.0, n in 0.0f64..365.0, h in -PI..PI, north in any::<bool>()) {
            let z = if north { z_north(s) } else { z_south(s) };
            let sun = cfg().sun_at(n, h, z);
            prop_assert!(deg(sun.visible_elevation) <= -4.0 + 1e-9);
            prop_assert_eq!(sun.light_intensity(), 0.0);
        }

        #[test]
        fn outside_band_visible_equals_geometric(s in 2_000.0f64..=50_000.0, n in 0.0f64..365.0, h in -PI..PI, north in any::<bool>()) {
            let z = if north { z_north(s) } else { z_south(s) };
            let sun = cfg().sun_at(n, h, z);
            prop_assert_eq!(sun.visible_elevation, sun.elevation);
        }

        #[test]
        fn continuous_across_seam(eps in 0.0f64..1.0, n in 0.0f64..365.0, h in -PI..PI) {
            let c = cfg();
            let north = c.sun_at(n, h, z_north(eps));
            let south = c.sun_at(n, h, z_south(eps));
            let seam = c.sun_at(n, h, 0.0);
            // Both sides converge to the seam value; e ~ sqrt(s) near the seam.
            prop_assert!(deg((north.elevation - south.elevation).abs()) < 1e-3);
            prop_assert!(deg((north.visible_elevation - south.visible_elevation).abs()) < 1e-3);
            prop_assert!(deg((north.elevation - seam.elevation).abs()) < 0.5);
            prop_assert!(deg((north.visible_elevation - seam.visible_elevation).abs()) < 1.0);
        }

        #[test]
        fn direction_is_unit(n in 0.0f64..365.0, h in -PI..PI, z in 0.0f64..100_000.0) {
            let d = cfg().sun_at(n, h, z).direction();
            prop_assert!(((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) - 1.0).abs() < 1e-12);
        }
    }
}
