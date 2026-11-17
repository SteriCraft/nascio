//! Periodic arithmetic on the torus. Horizontal coordinates (x, z) have period [`WORLD_SIZE`].
//!
//! Never subtract two raw horizontal coordinates: use [`delta`] / [`delta_f64`].
//!
//! Convention: when two points are exactly half a world apart, the shortest offset is
//! `+WORLD_SIZE / 2` in **both** directions (`delta(a, b) == delta(b, a) == HALF_WORLD`).
//! This is the only case where `delta` is not antisymmetric.

use crate::consts::{HALF_WORLD, WORLD_SIZE, WORLD_SIZE_F64};
use crate::fmath::DetMath;

/// Wraps a coordinate into `[0, WORLD_SIZE)`.
#[inline]
pub fn wrap(v: i32) -> i32 {
    v.rem_euclid(WORLD_SIZE)
}

/// Wraps a coordinate into `[0, WORLD_SIZE)`.
#[inline]
pub fn wrap_f64(v: f64) -> f64 {
    let w = v.rem_euclid(WORLD_SIZE_F64);
    // rem_euclid may round up to exactly the period for tiny negative inputs.
    if w >= WORLD_SIZE_F64 { 0.0 } else { w }
}

/// Shortest signed offset from `a` to `b`, in `(-WORLD_SIZE/2, WORLD_SIZE/2]`.
///
/// Uses `i64` internally, so any pair of `i32` inputs is valid.
#[inline]
pub fn delta(a: i32, b: i32) -> i32 {
    let s = i64::from(WORLD_SIZE);
    let d = (i64::from(b) - i64::from(a)).rem_euclid(s);
    // d is in [0, S), so the result fits in i32.
    (if d > s / 2 { d - s } else { d }) as i32
}

/// Shortest signed offset from `a` to `b`, in `(-WORLD_SIZE/2, WORLD_SIZE/2]`.
#[inline]
pub fn delta_f64(a: f64, b: f64) -> f64 {
    let d = wrap_f64(b - a);
    if d > f64::from(HALF_WORLD) {
        d - WORLD_SIZE_F64
    } else {
        d
    }
}

/// Shortest offset from `a` to `b` on the 2D torus, as `(dx, dz)`.
#[inline]
pub fn delta2(a: (i32, i32), b: (i32, i32)) -> (i32, i32) {
    (delta(a.0, b.0), delta(a.1, b.1))
}

/// Shortest offset from `a` to `b` on the 2D torus, as `(dx, dz)`.
#[inline]
pub fn delta2_f64(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (delta_f64(a.0, b.0), delta_f64(a.1, b.1))
}

/// Squared shortest distance on the 2D torus (exact, in `i64`).
#[inline]
pub fn distance_sq(a: (i32, i32), b: (i32, i32)) -> i64 {
    let (dx, dz) = delta2(a, b);
    i64::from(dx) * i64::from(dx) + i64::from(dz) * i64::from(dz)
}

/// Shortest distance on the 2D torus.
#[inline]
pub fn distance(a: (i32, i32), b: (i32, i32)) -> f64 {
    (distance_sq(a, b) as f64).sqrt()
}

/// Shortest distance on the 2D torus.
#[inline]
pub fn distance_f64(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dz) = delta2_f64(a, b);
    dx.det_hypot(dz)
}

/// Periodic linear interpolation along the shortest path from `a` to `b`, wrapped.
#[inline]
pub fn lerp_f64(a: f64, b: f64, t: f64) -> f64 {
    wrap_f64(a + delta_f64(a, b) * t)
}

/// Periodic linear interpolation in 2D along the shortest path, wrapped.
#[inline]
pub fn lerp2_f64(a: (f64, f64), b: (f64, f64), t: f64) -> (f64, f64) {
    (lerp_f64(a.0, b.0, t), lerp_f64(a.1, b.1, t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const S: i32 = WORLD_SIZE;

    #[test]
    fn wrap_basic() {
        assert_eq!(wrap(0), 0);
        assert_eq!(wrap(S), 0);
        assert_eq!(wrap(-1), S - 1);
        assert_eq!(wrap(S + 5), 5);
        assert!((0..S).contains(&wrap(i32::MIN)));
        assert_eq!(wrap_f64(-0.5), 99_999.5);
        assert_eq!(wrap_f64(-1e-20), 0.0);
        assert_eq!(wrap_f64(100_000.25), 0.25);
    }

    #[test]
    fn delta_across_seam() {
        assert_eq!(delta(99_999, 0), 1);
        assert_eq!(delta(0, 99_999), -1);
        assert_eq!(delta(99_990, 10), 20);
        assert_eq!(delta(10, 99_990), -20);
        assert_eq!(delta_f64(99_999.5, 0.5), 1.0);
        assert_eq!(delta_f64(0.5, 99_999.5), -1.0);
    }

    #[test]
    fn delta_half_world_tie_is_positive_both_ways() {
        assert_eq!(delta(0, HALF_WORLD), HALF_WORLD);
        assert_eq!(delta(HALF_WORLD, 0), HALF_WORLD);
        assert_eq!(delta_f64(0.0, 50_000.0), 50_000.0);
        assert_eq!(delta_f64(50_000.0, 0.0), 50_000.0);
    }

    #[test]
    fn delta_extreme_inputs() {
        assert_eq!(
            delta(i32::MIN, i32::MAX),
            delta(wrap(i32::MIN), wrap(i32::MAX))
        );
    }

    #[test]
    fn distance_across_corner() {
        // Opposite corners of the map are neighbours on the torus.
        assert_eq!(distance_sq((99_999, 99_999), (0, 0)), 2);
        assert!((distance_f64((99_999.0, 0.0), (0.0, 99_999.0)) - 2f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn lerp_takes_short_path() {
        assert_eq!(lerp_f64(99_990.0, 10.0, 0.5), 0.0);
        assert_eq!(lerp_f64(99_990.0, 10.0, 0.25), 99_995.0);
        assert_eq!(
            lerp2_f64((99_990.0, 5.0), (10.0, 99_995.0), 0.5),
            (0.0, 0.0)
        );
    }

    proptest! {
        #[test]
        fn delta_reaches_b(a in any::<i32>(), b in any::<i32>()) {
            prop_assert_eq!(wrap(wrap(a) + delta(a, b)), wrap(b));
        }

        #[test]
        fn delta_in_range(a in any::<i32>(), b in any::<i32>()) {
            let d = delta(a, b);
            prop_assert!(d > -HALF_WORLD && d <= HALF_WORLD);
        }

        #[test]
        fn delta_translation_invariant(a in -S..2 * S, b in -S..2 * S, k in -20i32..20) {
            prop_assert_eq!(delta(a + k * S, b), delta(a, b));
            prop_assert_eq!(delta(a, b - k * S), delta(a, b));
            prop_assert_eq!(delta(a + k, b + k), delta(a, b));
        }

        #[test]
        fn delta_antisymmetric_except_tie(a in any::<i32>(), b in any::<i32>()) {
            let d = delta(a, b);
            if d == HALF_WORLD {
                prop_assert_eq!(delta(b, a), HALF_WORLD);
            } else {
                prop_assert_eq!(delta(b, a), -d);
            }
        }

        #[test]
        fn delta_f64_matches_i32(a in -3 * S..3 * S, b in -3 * S..3 * S) {
            prop_assert_eq!(delta_f64(f64::from(a), f64::from(b)), f64::from(delta(a, b)));
            prop_assert_eq!(wrap_f64(f64::from(a)), f64::from(wrap(a)));
        }

        #[test]
        fn delta_f64_reaches_b(a in -1e6f64..1e6, b in -1e6f64..1e6) {
            let d = delta_f64(a, b);
            prop_assert!(d > -50_000.0 && d <= 50_000.0);
            let reached = delta_f64(wrap_f64(a + d), b);
            prop_assert!(reached.abs() < 1e-6);
        }

        #[test]
        fn wrap_f64_in_range(v in -1e9f64..1e9) {
            let w = wrap_f64(v);
            prop_assert!((0.0..WORLD_SIZE_F64).contains(&w));
        }

        #[test]
        fn distance_symmetric_and_bounded(
            ax in any::<i32>(), az in any::<i32>(), bx in any::<i32>(), bz in any::<i32>()
        ) {
            let (a, b) = ((ax, az), (bx, bz));
            prop_assert_eq!(distance_sq(a, b), distance_sq(b, a));
            prop_assert!(distance(a, b) <= f64::from(HALF_WORLD) * 2f64.sqrt() + 1e-9);
        }

        #[test]
        fn distance_seam_small(x in 0i32..S, off in 1i32..100) {
            // Points on either side of the seam (z = 0) are close.
            prop_assert_eq!(distance_sq((x, S - off), (x, off)), i64::from(2 * off).pow(2));
        }
    }
}
