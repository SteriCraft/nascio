//! Latitude and longitude on the torus (spec §2.1, §2.2).
//!
//! `z = 0` (≡ 100 000) is the pole line (« the seam »), `z = 50 000` is the equator.
//! Latitudes follow a sphere's area distribution: `φ = asin(d)`, `d = (50 000 − z) / 50 000`.

use crate::fmath::DetMath;
use std::f64::consts::TAU;

use crate::consts::{HALF_WORLD, WORLD_SIZE_F64};
use crate::torus::wrap_f64;

const HALF: f64 = HALF_WORLD as f64;

/// `d(z) = (50 000 − z) / 50 000 ∈ (−1, 1]`, with z wrapped first. Equals `sin φ`.
#[inline]
pub fn sin_latitude(z: f64) -> f64 {
    ((HALF - wrap_f64(z)) / HALF).clamp(-1.0, 1.0)
}

/// Latitude φ in radians, positive in the northern hemisphere (z < 50 000).
#[inline]
pub fn latitude(z: f64) -> f64 {
    sin_latitude(z).det_asin()
}

/// Distance to the seam `s = min(z, 100 000 − z) ∈ [0, 50 000]`.
#[inline]
pub fn seam_distance(z: f64) -> f64 {
    let z = wrap_f64(z);
    z.min(WORLD_SIZE_F64 - z)
}

/// `|φ|` in radians as a function of the seam distance: `asin(1 − s / 50 000)`.
#[inline]
pub fn abs_latitude_from_seam(s: f64) -> f64 {
    (1.0 - s / HALF).clamp(-1.0, 1.0).det_asin()
}

/// Longitude λ in radians, in `[0, 2π)`.
#[inline]
pub fn longitude(x: f64) -> f64 {
    TAU * wrap_f64(x) / WORLD_SIZE_F64
}

/// z of the northern-hemisphere point at seam distance `s` (`z = s`).
#[inline]
pub fn z_north(s: f64) -> f64 {
    wrap_f64(s)
}

/// z of the southern-hemisphere point at seam distance `s` (`z = 100 000 − s`).
#[inline]
pub fn z_south(s: f64) -> f64 {
    wrap_f64(WORLD_SIZE_F64 - s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landmarks() {
        assert!((latitude(50_000.0)).abs() < 1e-12);
        assert!((latitude(0.0).to_degrees() - 90.0).abs() < 1e-9);
        assert!((latitude(99_999.999).to_degrees() + 90.0).abs() < 0.03);
        assert!(latitude(25_000.0) > 0.0 && latitude(75_000.0) < 0.0);
        // Polar circle at s ≈ 4 125 m.
        assert!((abs_latitude_from_seam(4_125.0).to_degrees() - 66.56).abs() < 0.01);
        // Tropics ≈ 19 890 m from the equator.
        assert!((latitude(50_000.0 - 19_890.0).to_degrees() - 23.44).abs() < 0.01);
        // S_FADE = 3 000 m ⇒ φ ≈ 70°.
        assert!((abs_latitude_from_seam(3_000.0).to_degrees() - 70.05).abs() < 0.01);
    }

    #[test]
    fn seam_distance_symmetric() {
        assert_eq!(seam_distance(0.0), 0.0);
        assert_eq!(seam_distance(100_000.0), 0.0);
        assert_eq!(seam_distance(1_000.0), 1_000.0);
        assert_eq!(seam_distance(99_000.0), 1_000.0);
        assert_eq!(seam_distance(-1_000.0), 1_000.0);
        assert_eq!(seam_distance(50_000.0), 50_000.0);
        for z in [123.0, 4_000.0, 37_000.0, 61_000.0, 99_876.0] {
            let s = seam_distance(z);
            assert!((abs_latitude_from_seam(s) - latitude(z).abs()).abs() < 1e-12);
        }
    }

    #[test]
    fn tropics_cover_forty_percent() {
        let tropics = (0..100_000)
            .filter(|&z| latitude(f64::from(z) + 0.5).to_degrees().abs() < 23.44)
            .count();
        let share = tropics as f64 / 100_000.0;
        assert!((share - 0.398).abs() < 0.002, "{share}");
    }

    #[test]
    fn longitude_hour_spacing() {
        assert_eq!(longitude(0.0), 0.0);
        assert!((longitude(100_000.0 / 24.0).to_degrees() - 15.0).abs() < 1e-9);
        assert_eq!(longitude(100_000.0), 0.0);
    }
}
