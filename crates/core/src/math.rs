//! Small scalar helpers.

/// Hermite smoothstep: 0 for `x <= edge0`, 1 for `x >= edge1`, `3t² − 2t³` in between (C¹).
///
/// Requires `edge0 < edge1`.
#[inline]
pub fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    debug_assert!(edge0 < edge1);
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoothstep_edges_and_midpoint() {
        assert_eq!(smoothstep(0.0, 1.0, -5.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 0.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 1.0), 1.0);
        assert_eq!(smoothstep(0.0, 1.0, 7.0), 1.0);
        assert!((smoothstep(1000.0, 2000.0, 1500.0) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn smoothstep_is_monotonic() {
        let mut prev = 0.0;
        for i in 0..=1000 {
            let v = smoothstep(0.0, 3000.0, f64::from(i) * 3.0);
            assert!(v >= prev);
            prev = v;
        }
    }
}
