//! Periodic gradient noise on the torus (spec §5).
//!
//! The lattice spacing must divide `WORLD_SIZE`, and lattice nodes are hashed modulo the
//! number of nodes per period, so every noise function loops exactly every 100 000 m.

use nascio_core::consts::{WORLD_SIZE, WORLD_SIZE_F64};
use nascio_core::fmath::DetMath;
use nascio_core::torus::wrap_f64;

use crate::rng::mix64;

/// All divisors of `WORLD_SIZE` (2⁵·5⁵), ascending (computed once).
pub fn world_divisors() -> &'static [u32] {
    static DIVISORS: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
    DIVISORS.get_or_init(|| {
        (1..=WORLD_SIZE as u32)
            .filter(|v| (WORLD_SIZE as u32).is_multiple_of(*v))
            .collect()
    })
}

/// Largest divisor of `WORLD_SIZE` not above `max`, or 1.
pub fn divisor_at_most(max: f64) -> u32 {
    world_divisors()
        .iter()
        .rev()
        .find(|&&d| f64::from(d) <= max)
        .copied()
        .unwrap_or(1)
}

/// The node hash picks one of `2^GRADIENT_BITS` unit gradients at evenly spaced angles.
const GRADIENT_BITS: u32 = 8;

/// The shared gradient table (computed once with the deterministic `sin`/`cos`).
fn gradients() -> &'static [(f64, f64); 1 << GRADIENT_BITS] {
    static TABLE: std::sync::OnceLock<[(f64, f64); 1 << GRADIENT_BITS]> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|k| {
            let angle = k as f64 / f64::from(1u32 << GRADIENT_BITS) * std::f64::consts::TAU;
            (angle.det_cos(), angle.det_sin())
        })
    })
}

/// One octave of periodic 2D gradient (Perlin-style) noise, roughly in `[-1, 1]`.
#[derive(Clone, Debug)]
pub struct PeriodicNoise {
    seed: u64,
    spacing: f64,
    nodes: i64,
    gradients: &'static [(f64, f64); 1 << GRADIENT_BITS],
}

impl PeriodicNoise {
    /// `spacing_m` must divide `WORLD_SIZE`.
    pub fn new(seed: u64, spacing_m: u32) -> Self {
        assert!(
            spacing_m > 0 && (WORLD_SIZE as u32).is_multiple_of(spacing_m),
            "noise spacing {spacing_m} must divide {WORLD_SIZE}"
        );
        Self {
            seed,
            spacing: f64::from(spacing_m),
            nodes: i64::from(WORLD_SIZE as u32 / spacing_m),
            gradients: gradients(),
        }
    }

    /// Lattice spacing in metres.
    pub fn spacing(&self) -> f64 {
        self.spacing
    }

    #[inline]
    fn gradient(&self, ix: i64, iz: i64) -> (f64, f64) {
        let ix = ix.rem_euclid(self.nodes) as u64;
        let iz = iz.rem_euclid(self.nodes) as u64;
        let h = mix64(self.seed ^ mix64(ix.wrapping_mul(0x8CB9_2BA7_2F3D_8DD7) ^ iz));
        self.gradients[(h >> (64 - GRADIENT_BITS)) as usize]
    }

    /// Noise value at world position `(x, z)` (metres, any value: wrapped internally).
    pub fn sample(&self, x: f64, z: f64) -> f64 {
        let gx = wrap_f64(x) / self.spacing;
        let gz = wrap_f64(z) / self.spacing;
        let (x0, z0) = (gx.floor(), gz.floor());
        let (fx, fz) = (gx - x0, gz - z0);
        let (ix, iz) = (x0 as i64, z0 as i64);
        let dot = |cx: i64, cz: i64, dx: f64, dz: f64| {
            let (gx, gz) = self.gradient(ix + cx, iz + cz);
            gx * dx + gz * dz
        };
        let n00 = dot(0, 0, fx, fz);
        let n10 = dot(1, 0, fx - 1.0, fz);
        let n01 = dot(0, 1, fx, fz - 1.0);
        let n11 = dot(1, 1, fx - 1.0, fz - 1.0);
        let (u, v) = (fade(fx), fade(fz));
        let a = n00 + u * (n10 - n00);
        let b = n01 + u * (n11 - n01);
        // Unit gradients give |n| ≤ √2/2; rescale to about [-1, 1].
        std::f64::consts::SQRT_2 * (a + v * (b - a))
    }
}

#[inline]
fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Fractal sum of periodic octaves. Octave spacings are divisors of `WORLD_SIZE`, each about
/// half the previous one.
#[derive(Clone, Debug)]
pub struct Fbm {
    octaves: Vec<(PeriodicNoise, f64)>,
    norm: f64,
}

impl Fbm {
    /// `base_spacing_m` is snapped down to a divisor of `WORLD_SIZE`; `gain` is the amplitude
    /// ratio between consecutive octaves.
    pub fn new(seed: u64, base_spacing_m: f64, octaves: u32, gain: f64) -> Self {
        let mut spacing = divisor_at_most(base_spacing_m.min(WORLD_SIZE_F64));
        let mut amp = 1.0;
        let mut list = Vec::new();
        for o in 0..octaves.max(1) {
            list.push((PeriodicNoise::new(mix64(seed ^ u64::from(o)), spacing), amp));
            let next = divisor_at_most(f64::from(spacing) / 2.0);
            if next == spacing {
                break;
            }
            spacing = next;
            amp *= gain;
        }
        let norm = list.iter().map(|(_, a)| a).sum::<f64>();
        Self {
            octaves: list,
            norm,
        }
    }

    /// Value roughly in `[-1, 1]`.
    pub fn sample(&self, x: f64, z: f64) -> f64 {
        self.octaves
            .iter()
            .map(|(n, a)| a * n.sample(x, z))
            .sum::<f64>()
            / self.norm
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn divisors() {
        let d = world_divisors();
        assert_eq!(d.len(), 36);
        assert!(d.contains(&160) && d.contains(&3125) && d.contains(&625));
        assert_eq!(divisor_at_most(1_562.5), 1_250);
        assert_eq!(divisor_at_most(20_000.0), 20_000);
    }

    #[test]
    #[should_panic]
    fn rejects_non_dividing_spacing() {
        let _ = PeriodicNoise::new(1, 300);
    }

    #[test]
    fn bounded_and_varied() {
        let n = PeriodicNoise::new(7, 2_000);
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for i in 0..20_000 {
            let v = n.sample(f64::from(i) * 37.3, f64::from(i) * 11.9);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        assert!(lo >= -1.0001 && hi <= 1.0001);
        assert!(lo < -0.5 && hi > 0.5);
    }

    #[test]
    fn seeds_differ() {
        let a = PeriodicNoise::new(1, 5_000);
        let b = PeriodicNoise::new(2, 5_000);
        assert!((0..100).any(|i| {
            let x = f64::from(i) * 1_234.5;
            (a.sample(x, 777.0) - b.sample(x, 777.0)).abs() > 1e-3
        }));
    }

    #[test]
    fn continuous_across_seam() {
        let f = Fbm::new(9, 25_000.0, 6, 0.5);
        for i in 0..200 {
            let t = f64::from(i) * 500.0;
            assert!((f.sample(99_999.999, t) - f.sample(0.001, t)).abs() < 1e-3);
            assert!((f.sample(t, 99_999.999) - f.sample(t, 0.001)).abs() < 1e-3);
        }
    }

    proptest! {
        #[test]
        fn periodic(x in -2e5f64..2e5, z in -2e5f64..2e5, kx in -3i32..3, kz in -3i32..3) {
            let n = Fbm::new(3, 50_000.0, 7, 0.5);
            let a = n.sample(x, z);
            let b = n.sample(x + f64::from(kx) * WORLD_SIZE_F64, z + f64::from(kz) * WORLD_SIZE_F64);
            prop_assert!((a - b).abs() < 1e-9);
        }
    }
}
