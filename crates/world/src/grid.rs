//! Periodic 2D grids covering the whole torus.
//!
//! Cell `(i, j)` covers `[i·c, (i+1)·c) × [j·c, (j+1)·c)` with `c` the cell size; its value is
//! attached to the cell centre. Indices wrap on both axes.

use nascio_core::consts::WORLD_SIZE_F64;
use nascio_core::torus::wrap_f64;

/// A periodic grid of `width × height` cells, row-major (`z` rows of `x` cells).
#[derive(Clone, Debug, PartialEq)]
pub struct PeriodicGrid<T> {
    width: usize,
    height: usize,
    cell: f64,
    data: Vec<T>,
}

impl<T: Clone> PeriodicGrid<T> {
    /// Grid filled with `fill`. `width · cell_m` and `height · cell_m` must equal `WORLD_SIZE`.
    pub fn new(width: usize, height: usize, cell_m: f64, fill: T) -> Self {
        Self::check(width, height, cell_m);
        Self {
            width,
            height,
            cell: cell_m,
            data: vec![fill; width * height],
        }
    }
}

impl<T> PeriodicGrid<T> {
    fn check(width: usize, height: usize, cell_m: f64) {
        assert!(width > 0 && height > 0);
        assert!(
            (width as f64 * cell_m - WORLD_SIZE_F64).abs() < 1e-6
                && (height as f64 * cell_m - WORLD_SIZE_F64).abs() < 1e-6,
            "grid {width}×{height} of {cell_m} m does not cover the torus"
        );
    }

    /// Grid whose cell `(i, j)` holds `f(i, j)`.
    pub fn from_fn(
        width: usize,
        height: usize,
        cell_m: f64,
        mut f: impl FnMut(usize, usize) -> T,
    ) -> Self {
        Self::check(width, height, cell_m);
        let mut data = Vec::with_capacity(width * height);
        for j in 0..height {
            for i in 0..width {
                data.push(f(i, j));
            }
        }
        Self {
            width,
            height,
            cell: cell_m,
            data,
        }
    }

    /// Grid built from raw row-major data.
    pub fn from_vec(width: usize, height: usize, cell_m: f64, data: Vec<T>) -> Self {
        Self::check(width, height, cell_m);
        assert_eq!(data.len(), width * height);
        Self {
            width,
            height,
            cell: cell_m,
            data,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Cell size in metres.
    pub fn cell_size(&self) -> f64 {
        self.cell
    }

    pub fn data(&self) -> &[T] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [T] {
        &mut self.data
    }

    /// Row-major index of a wrapped cell.
    #[inline]
    pub fn index(&self, i: i64, j: i64) -> usize {
        let i = i.rem_euclid(self.width as i64) as usize;
        let j = j.rem_euclid(self.height as i64) as usize;
        j * self.width + i
    }

    /// `(i, j)` of a row-major index.
    #[inline]
    pub fn coords(&self, idx: usize) -> (usize, usize) {
        (idx % self.width, idx / self.width)
    }

    /// Value of a wrapped cell.
    #[inline]
    pub fn get(&self, i: i64, j: i64) -> &T {
        &self.data[self.index(i, j)]
    }

    /// Sets a wrapped cell.
    #[inline]
    pub fn set(&mut self, i: i64, j: i64, v: T) {
        let k = self.index(i, j);
        self.data[k] = v;
    }

    /// World coordinates `(x, z)` of a cell centre.
    #[inline]
    pub fn cell_center(&self, i: usize, j: usize) -> (f64, f64) {
        ((i as f64 + 0.5) * self.cell, (j as f64 + 0.5) * self.cell)
    }

    /// Cell containing a world position.
    #[inline]
    pub fn cell_of(&self, x: f64, z: f64) -> (usize, usize) {
        let i = (wrap_f64(x) / self.cell).floor() as usize;
        let j = (wrap_f64(z) / self.cell).floor() as usize;
        (i.min(self.width - 1), j.min(self.height - 1))
    }

    /// Same grid shape with mapped values.
    pub fn map<U>(&self, f: impl FnMut(&T) -> U) -> PeriodicGrid<U> {
        PeriodicGrid {
            width: self.width,
            height: self.height,
            cell: self.cell,
            data: self.data.iter().map(f).collect(),
        }
    }

    /// Continuous grid coordinates of a world position, relative to cell centres.
    #[inline]
    fn grid_pos(&self, x: f64, z: f64) -> (i64, i64, f64, f64) {
        let gx = wrap_f64(x) / self.cell - 0.5;
        let gz = wrap_f64(z) / self.cell - 0.5;
        let (fx, fz) = (gx.floor(), gz.floor());
        (fx as i64, fz as i64, gx - fx, gz - fz)
    }
}

impl<T: Copy + Into<f64>> PeriodicGrid<T> {
    /// Periodic bilinear interpolation between cell centres.
    pub fn sample_bilinear(&self, x: f64, z: f64) -> f64 {
        let (i, j, fx, fz) = self.grid_pos(x, z);
        let v = |di: i64, dj: i64| (*self.get(i + di, j + dj)).into();
        let a = v(0, 0) + fx * (v(1, 0) - v(0, 0));
        let b = v(0, 1) + fx * (v(1, 1) - v(0, 1));
        a + fz * (b - a)
    }

    /// Periodic bicubic (Catmull-Rom) interpolation between cell centres.
    pub fn sample_bicubic(&self, x: f64, z: f64) -> f64 {
        let (i, j, fx, fz) = self.grid_pos(x, z);
        let row = |dj: i64| {
            let p = |di: i64| (*self.get(i + di, j + dj)).into();
            catmull_rom(p(-1), p(0), p(1), p(2), fx)
        };
        catmull_rom(row(-1), row(0), row(1), row(2), fz)
    }

    /// Bicubic made of monotone cubic Hermite passes ([`monotone_cubic`]): Catmull-Rom where
    /// the nodes are smooth and monotone, no overshoot next to steep steps (no spurious islets
    /// in front of a cliff, no pits at its foot), within the range of the four surrounding cell
    /// centres, continuous and without plateaus. It replaces a Catmull-Rom clamped to those
    /// four nodes, whose bounds changed across every node line: wherever the bicubic overshot
    /// there, the surface stepped along the line (metres high on the 1/5 world at
    /// (87 280, 480..555)), and the clamp left flat creases inside the cells.
    pub fn sample_bicubic_monotone(&self, x: f64, z: f64) -> f64 {
        let (i, j, fx, fz) = self.grid_pos(x, z);
        let row = |dj: i64| {
            let p = |di: i64| (*self.get(i + di, j + dj)).into();
            monotone_cubic(p(-1), p(0), p(1), p(2), fx)
        };
        monotone_cubic(row(-1), row(0), row(1), row(2), fz)
    }
}

#[inline]
fn catmull_rom(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

/// Cubic Hermite between `p1` and `p2` (`t` in [0, 1]) with Catmull-Rom tangents limited
/// for monotonicity: zero at a local extremum of the nodes, at most three times the smaller
/// adjacent difference otherwise (Fritsch–Carlson box), so the curve never leaves [p1, p2].
/// The tangent at a node depends only on that node and its two neighbours: both intervals
/// share it, and the curve is C¹ across nodes.
#[inline]
fn monotone_cubic(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    let tangent = |l: f64, r: f64| {
        if l * r <= 0.0 {
            0.0
        } else {
            let m = 0.5 * (l + r);
            let cap = 3.0 * l.abs().min(r.abs());
            m.clamp(-cap, cap)
        }
    };
    let d = p2 - p1;
    let (m1, m2) = (tangent(p1 - p0, d), tangent(d, p3 - p2));
    let t2 = t * t;
    let t3 = t2 * t;
    let v = (2.0 * t3 - 3.0 * t2 + 1.0) * p1
        + (t3 - 2.0 * t2 + t) * m1
        + (-2.0 * t3 + 3.0 * t2) * p2
        + (t3 - t2) * m2;
    // Rounding only: the limited tangents keep the cubic within [p1, p2].
    v.clamp(p1.min(p2), p1.max(p2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ramp() -> PeriodicGrid<f32> {
        PeriodicGrid::from_fn(25, 25, 4_000.0, |i, j| (i * 100 + j) as f32)
    }

    #[test]
    fn wrapping_access() {
        let g = ramp();
        assert_eq!(*g.get(-1, 0), 2_400.0);
        assert_eq!(*g.get(25, 26), 1.0);
        assert_eq!(g.cell_of(-1.0, 100_000.5), (24, 0));
        assert_eq!(g.coords(g.index(3, 7)), (3, 7));
    }

    #[test]
    #[should_panic]
    fn rejects_grid_not_covering_torus() {
        let _ = PeriodicGrid::new(10, 10, 160.0, 0u8);
    }

    #[test]
    fn interpolation_hits_cell_centres() {
        let g = ramp();
        let (x, z) = g.cell_center(7, 3);
        assert!((g.sample_bilinear(x, z) - 703.0).abs() < 1e-9);
        assert!((g.sample_bicubic(x, z) - 703.0).abs() < 1e-9);
    }

    #[test]
    fn bilinear_across_seam_blends_last_and_first() {
        let g = ramp();
        // x = 0 lies halfway between the centres of columns 24 and 0.
        let v = g.sample_bilinear(0.0, g.cell_center(0, 5).1);
        assert!((v - (2_405.0 + 5.0) / 2.0).abs() < 1e-9);
    }

    #[test]
    fn monotone_bicubic_does_not_overshoot_a_step() {
        let g = PeriodicGrid::from_fn(
            25,
            25,
            4_000.0,
            |i, _| if i < 12 { -1.0f32 } else { -1_500.0 },
        );
        let z = 10_000.0;
        let (mut raw_max, mut monotone_max) = (f64::MIN, f64::MIN);
        for k in 0..200 {
            let x = 40_000.0 + f64::from(k) * 50.0;
            raw_max = raw_max.max(g.sample_bicubic(x, z));
            monotone_max = monotone_max.max(g.sample_bicubic_monotone(x, z));
        }
        assert!(raw_max > -1.0, "the plain bicubic overshoots: {raw_max}");
        assert!(monotone_max <= -1.0);
    }

    proptest! {
        /// No step across node lines (x or z), on rough grids where the bicubic overshoots;
        /// the result stays within the four surrounding nodes.
        #[test]
        fn monotone_bicubic_is_continuous_and_bounded(
            seed in 0u64..1_000,
            node in 0i64..25,
            t in 0f64..1.0,
            eps in 1e-7f64..1e-4,
        ) {
            let g = PeriodicGrid::from_fn(25, 25, 4_000.0, |i, j| {
                let h = (i as u64 * 7_919 + j as u64 * 104_729 + seed * 6_151) % 1_009;
                (h as f32 - 500.0) * if h.is_multiple_of(3) { 3.0 } else { 0.2 }
            });
            let line = (node as f64 + 0.5) * 4_000.0;
            let across = (t * 100_000.0).max(1.0);
            for (a, b) in [
                ((line - eps, across), (line + eps, across)),
                ((across, line - eps), (across, line + eps)),
            ] {
                let (va, vb) = (g.sample_bicubic_monotone(a.0, a.1), g.sample_bicubic_monotone(b.0, b.1));
                prop_assert!((va - vb).abs() < 1e-2, "{a:?} {va} vs {b:?} {vb}");
                let (i, j, _, _) = g.grid_pos(a.0, a.1);
                let c = [g.get(i, j), g.get(i + 1, j), g.get(i, j + 1), g.get(i + 1, j + 1)].map(|v| f64::from(*v));
                let (lo, hi) = (c.iter().copied().fold(f64::MAX, f64::min), c.iter().copied().fold(f64::MIN, f64::max));
                prop_assert!((lo..=hi).contains(&va));
            }
        }

        #[test]
        fn bicubic_continuous_across_seam(t in 0f64..100_000.0, eps in 1e-6f64..1e-3) {
            let g = PeriodicGrid::from_fn(25, 25, 4_000.0, |i, j| ((i * 7919 + j * 104_729) % 97) as f32);
            let a = g.sample_bicubic(100_000.0 - eps, t);
            let b = g.sample_bicubic(eps, t);
            prop_assert!((a - b).abs() < 1e-2);
            let a = g.sample_bicubic(t, 100_000.0 - eps);
            let b = g.sample_bicubic(t, eps);
            prop_assert!((a - b).abs() < 1e-2);
        }
    }
}
