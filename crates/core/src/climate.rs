//! Annual harmonics of the climate (spec §4.3) and the diurnal cycle, evaluated at run time.
//!
//! Each climate quantity is stored per cell as `(X0, A1, p1, A2, p2)`:
//! `X(N) = X0 + A1·cos(2π(N − p1)/365) + A2·cos(4π(N − p2)/365)`, with N the continuous day of
//! the year and the phases in days. The evaluation is continuous in N by construction, also
//! across the new year.

use std::f64::consts::TAU;

use crate::consts::DAYS_PER_YEAR;

const YEAR: f64 = DAYS_PER_YEAR as f64;

/// Annual harmonics of one quantity at one place.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Harmonics {
    pub x0: f64,
    pub a1: f64,
    /// Day of the first harmonic's maximum, in `[0, 365)`.
    pub p1: f64,
    pub a2: f64,
    /// Day of the second harmonic's first maximum, in `[0, 182.5)`.
    pub p2: f64,
}

impl Harmonics {
    /// Value at continuous day of year `n`.
    #[inline]
    pub fn eval(&self, n: f64) -> f64 {
        self.x0
            + self.a1 * (TAU * (n - self.p1) / YEAR).cos()
            + self.a2 * (2.0 * TAU * (n - self.p2) / YEAR).cos()
    }

    /// Projection of `samples`, taken at `N_k = 365·k/n` (`n ≥ 5`), on the mean and the first two
    /// annual harmonics (least squares, exact for evenly spaced samples).
    pub fn project(samples: &[f64]) -> Self {
        let n = samples.len();
        assert!(n >= 5, "at least 5 samples per year");
        let nf = n as f64;
        let mut x0 = 0.0;
        let (mut c1, mut s1, mut c2, mut s2) = (0.0, 0.0, 0.0, 0.0);
        for (k, &x) in samples.iter().enumerate() {
            let w = TAU * k as f64 / nf;
            x0 += x;
            c1 += x * w.cos();
            s1 += x * w.sin();
            c2 += x * (2.0 * w).cos();
            s2 += x * (2.0 * w).sin();
        }
        let f = 2.0 / nf;
        let (c1, s1, c2, s2) = (c1 * f, s1 * f, c2 * f, s2 * f);
        // a·cos(w) + b·sin(w) = A·cos(w − θ), θ = atan2(b, a); w − θ = 2π(N − p)/365.
        let p1 = (s1.atan2(c1) / TAU * YEAR).rem_euclid(YEAR);
        let p2 = (s2.atan2(c2) / (2.0 * TAU) * YEAR).rem_euclid(YEAR / 2.0);
        Self {
            x0: x0 / nf,
            a1: c1.hypot(s1),
            p1,
            a2: c2.hypot(s2),
            p2,
        }
    }
}

/// Local solar time of the daily temperature maximum, as a fraction of the day (14:30).
pub const DIURNAL_PEAK_FRAC: f64 = 14.5 / 24.0;

/// Diurnal temperature offset for a daily range `range` (°C, maximum − minimum) at local solar
/// day fraction `local_frac`: a cosine peaking at 14:30 local solar time.
#[inline]
pub fn diurnal_offset(range: f64, local_frac: f64) -> f64 {
    0.5 * range * (TAU * (local_frac - DIURNAL_PEAK_FRAC)).cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn projection_recovers_two_harmonics() {
        let h = Harmonics {
            x0: 12.0,
            a1: 9.0,
            p1: 200.0,
            a2: 2.0,
            p2: 40.0,
        };
        for n in [24usize, 48] {
            let s: Vec<f64> = (0..n).map(|k| h.eval(YEAR * k as f64 / n as f64)).collect();
            let p = Harmonics::project(&s);
            assert!((p.x0 - h.x0).abs() < 1e-9);
            assert!((p.a1 - h.a1).abs() < 1e-9 && (p.p1 - h.p1).abs() < 1e-6);
            assert!((p.a2 - h.a2).abs() < 1e-9 && (p.p2 - h.p2).abs() < 1e-6);
        }
    }

    #[test]
    fn diurnal_peaks_mid_afternoon() {
        assert!((diurnal_offset(10.0, DIURNAL_PEAK_FRAC) - 5.0).abs() < 1e-12);
        assert!(diurnal_offset(10.0, DIURNAL_PEAK_FRAC + 0.5) < -4.99);
        // Continuous across midnight.
        assert!((diurnal_offset(8.0, 0.0) - diurnal_offset(8.0, 1.0)).abs() < 1e-12);
    }

    proptest! {
        #[test]
        fn continuous_across_new_year(x0 in -30.0f64..30.0, a1 in 0.0f64..20.0, p1 in 0.0f64..365.0,
                                      a2 in 0.0f64..5.0, p2 in 0.0f64..182.5) {
            let h = Harmonics { x0, a1, p1, a2, p2 };
            prop_assert!((h.eval(0.0) - h.eval(YEAR)).abs() < 1e-9);
            prop_assert!((h.eval(YEAR - 1e-6) - h.eval(0.0)).abs() < 1e-3);
        }
    }
}
