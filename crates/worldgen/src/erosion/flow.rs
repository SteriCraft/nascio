//! Flow routing on a periodic grid: sea mask, priority-flood, D8 receivers, stack order,
//! accumulation and basins.
//!
//! Everything is deterministic: the flood uses a heap keyed by (elevation, cell index), the
//! receivers are a parallel gather with a fixed neighbour order, and the stack and the
//! accumulation are sequential.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

use rayon::prelude::*;

pub use nascio_world::erosion::flow::{D8, NO_DIR};

/// Shape of a square periodic grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub res: usize,
    pub cell: f64,
}

impl Shape {
    #[inline]
    pub fn n(self) -> usize {
        self.res * self.res
    }

    /// Index of the neighbour of `k` in direction `d` (wrapped).
    #[inline]
    pub fn neighbour(self, k: usize, d: usize) -> usize {
        let r = self.res as i64;
        let (i, j) = ((k % self.res) as i64, (k / self.res) as i64);
        let (di, dj) = D8[d];
        ((j + dj).rem_euclid(r) * r + (i + di).rem_euclid(r)) as usize
    }

    /// Distance to the neighbour in direction `d`.
    #[inline]
    pub fn step(self, d: usize) -> f64 {
        if d % 2 == 1 {
            self.cell * std::f64::consts::SQRT_2
        } else {
            self.cell
        }
    }
}

/// Order-preserving map of an `f64` to `u64` (total order, negatives included).
#[inline]
fn key(v: f64) -> u64 {
    let b = v.to_bits();
    if b >> 63 == 1 { !b } else { b | (1 << 63) }
}

/// Sea: 8-connected regions below sea level of at least `min_area_m2`. Smaller regions below
/// sea level are depressions (lakes or filled). If nothing qualifies, the lowest cell (lowest
/// index on ties) is the only base level, so every grid can drain.
pub fn sea_mask(h: &[f64], shape: Shape, min_area_m2: f64) -> Vec<bool> {
    let n = shape.n();
    let min_cells = (min_area_m2 / (shape.cell * shape.cell)).ceil().max(1.0) as usize;
    let mut comp = vec![u32::MAX; n];
    let mut sea = vec![false; n];
    let mut members = Vec::new();
    let mut next = 0u32;
    for start in 0..n {
        if comp[start] != u32::MAX || h[start] >= 0.0 {
            continue;
        }
        members.clear();
        comp[start] = next;
        members.push(start);
        let mut q = 0;
        while q < members.len() {
            let k = members[q];
            q += 1;
            for d in 0..8 {
                let m = shape.neighbour(k, d);
                if comp[m] == u32::MAX && h[m] < 0.0 {
                    comp[m] = next;
                    members.push(m);
                }
            }
        }
        if members.len() >= min_cells {
            for &k in &members {
                sea[k] = true;
            }
        }
        next += 1;
    }
    if !sea.iter().any(|&s| s) {
        let lowest = (0..n)
            .min_by(|&a, &b| h[a].total_cmp(&h[b]).then(a.cmp(&b)))
            .expect("non-empty grid");
        sea[lowest] = true;
    }
    sea
}

/// Priority-flood (Barnes et al. 2014) from the sea: every depression is raised to its spill
/// level. With `epsilon`, filled cells also get a strictly increasing elevation along the flood
/// (`next_up`), so every land cell has a strictly lower neighbour and drains to the sea.
pub fn priority_flood(h: &[f64], sea: &[bool], shape: Shape, epsilon: bool) -> Vec<f64> {
    let n = shape.n();
    let mut filled = h.to_vec();
    let mut closed = sea.to_vec();
    let mut heap = BinaryHeap::new();
    let mut pit = VecDeque::new();
    // Seeds: sea cells next to land (interior sea cells never reach land first).
    for k in 0..n {
        if sea[k] && (0..8).any(|d| !sea[shape.neighbour(k, d)]) {
            heap.push(Reverse((key(filled[k]), k as u32)));
        }
    }
    loop {
        let c = if let Some(c) = pit.pop_front() {
            c
        } else if let Some(Reverse((_, c))) = heap.pop() {
            c as usize
        } else {
            break;
        };
        let hc = filled[c];
        for d in 0..8 {
            let m = shape.neighbour(c, d);
            if closed[m] {
                continue;
            }
            closed[m] = true;
            let floor = if epsilon { hc.next_up() } else { hc };
            if filled[m] <= floor {
                filled[m] = floor;
                pit.push_back(m);
            } else {
                heap.push(Reverse((key(filled[m]), m as u32)));
            }
        }
    }
    filled
}

/// Steepest-descent (D8) receiver and direction code of every cell on a surface where every
/// non-sea cell has a lower neighbour (an epsilon-filled surface). Sea cells are their own
/// receiver (`NO_DIR`). Ties go to the first direction of [`D8`].
pub fn receivers(surface: &[f64], sea: &[bool], shape: Shape) -> (Vec<u32>, Vec<u8>) {
    let n = shape.n();
    let mut rec = vec![0u32; n];
    let mut dir = vec![NO_DIR; n];
    rec.par_iter_mut()
        .zip(dir.par_iter_mut())
        .enumerate()
        .for_each(|(k, (r, dk))| {
            *r = k as u32;
            if sea[k] {
                return;
            }
            let mut best = 0.0;
            for d in 0..8 {
                let m = shape.neighbour(k, d);
                let s = (surface[k] - surface[m]) / shape.step(d);
                if s > best {
                    best = s;
                    *r = m as u32;
                    *dk = d as u8;
                }
            }
        });
    (rec, dir)
}

/// Stack order (Braun & Willett 2013): base levels first, every cell after its receiver.
/// Cells that cannot reach a base level (cycles) are left out, so `len() < n` reveals them.
pub fn stack(rec: &[u32]) -> Vec<u32> {
    let n = rec.len();
    // Donor lists in compressed form.
    let mut count = vec![0u32; n + 1];
    for (k, &r) in rec.iter().enumerate() {
        if r as usize != k {
            count[r as usize + 1] += 1;
        }
    }
    for k in 0..n {
        count[k + 1] += count[k];
    }
    let mut fill = count.clone();
    let mut donors = vec![0u32; count[n] as usize];
    for (k, &r) in rec.iter().enumerate() {
        if r as usize != k {
            donors[fill[r as usize] as usize] = k as u32;
            fill[r as usize] += 1;
        }
    }
    let mut order = Vec::with_capacity(n);
    let mut todo = Vec::new();
    for (base, &r) in rec.iter().enumerate() {
        if r as usize != base {
            continue;
        }
        todo.push(base as u32);
        while let Some(k) = todo.pop() {
            order.push(k);
            let k = k as usize;
            todo.extend_from_slice(&donors[count[k] as usize..count[k + 1] as usize]);
        }
    }
    order
}

/// Accumulates `weight` down the receiver tree: every cell gets its own weight plus that of all
/// its upstream cells.
pub fn accumulate(order: &[u32], rec: &[u32], weight: &[f64]) -> Vec<f64> {
    let mut acc = weight.to_vec();
    for &k in order.iter().rev() {
        let r = rec[k as usize] as usize;
        if r != k as usize {
            acc[r] += acc[k as usize];
        }
    }
    acc
}

/// Base level reached by every cell (its own index for a base level).
pub fn outlets(order: &[u32], rec: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = (0..rec.len() as u32).collect();
    for &k in order {
        out[k as usize] = out[rec[k as usize] as usize];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(res: usize) -> Shape {
        Shape {
            res,
            cell: 100_000.0 / res as f64,
        }
    }

    #[test]
    fn key_preserves_order() {
        let v = [-3_000.5, -1.0, -0.0, 0.0, 1e-9, 2.5, 3_000.0];
        for w in v.windows(2) {
            assert!(key(w[0]) <= key(w[1]), "{w:?}");
        }
    }

    #[test]
    fn flood_fills_a_bowl_to_its_spill_level_with_ties() {
        // 25×25 torus: sea on column 0, land rising to 50 m, and a flat-bottomed bowl at 10 m
        // whose rim (20 m) has one notch at 15 m towards the sea.
        let s = shape(25);
        let mut h = vec![50.0; s.n()];
        for j in 0..25 {
            h[j * 25] = -100.0;
        }
        for j in 8..=16 {
            for i in 8..=16 {
                let rim = i == 8 || i == 16 || j == 8 || j == 16;
                h[j * 25 + i] = if rim { 20.0 } else { 10.0 };
            }
        }
        h[12 * 25 + 8] = 15.0; // notch, west side
        for i in 1..8 {
            h[12 * 25 + i] = 15.0 - 0.5 * (8 - i) as f64; // channel down to the sea
        }
        let sea = sea_mask(&h, s, 1.0);
        assert!(sea[0] && !sea[12 * 25 + 12]);
        let f = priority_flood(&h, &sea, s, false);
        for j in 9..=15 {
            for i in 9..=15 {
                assert_eq!(f[j * 25 + i], 15.0, "({i}, {j})");
            }
        }
        // Without depressions outside the bowl, the rest is unchanged.
        assert_eq!(f[3 * 25 + 3], 50.0);
        // With epsilon, everything drains; twice the same result.
        let fe = priority_flood(&h, &sea, s, true);
        assert_eq!(fe, priority_flood(&h, &sea, s, true));
        let (rec, _) = receivers(&fe, &sea, s);
        let order = stack(&rec);
        assert_eq!(order.len(), s.n());
        for k in 0..s.n() {
            assert!(sea[k] || rec[k] as usize != k, "orphan sink at {k}");
            assert!(sea[k] || fe[rec[k] as usize] < fe[k]);
        }
        // The bowl drains through the notch.
        let out = outlets(&order, &rec);
        assert_eq!(out[12 * 25 + 12], (12 * 25) as u32);
    }

    #[test]
    fn small_regions_below_sea_level_are_not_sea() {
        let s = shape(25);
        let mut h = vec![10.0; s.n()];
        h[5] = -5.0;
        h[300..400].fill(-50.0);
        let sea = sea_mask(&h, s, 50.0 * s.cell * s.cell);
        assert!(!sea[5] && sea[350]);
        // No region at all: the lowest cell becomes the base level.
        let sea = sea_mask(&vec![1.0; s.n()], s, 1.0);
        assert_eq!(sea.iter().filter(|&&b| b).count(), 1);
        assert!(sea[0]);
    }

    /// A valley crossing both seams drains continuously across them.
    #[test]
    fn valley_drains_across_both_seams() {
        let s = shape(125);
        let r = 125i64;
        // Sea on columns 40..45. Land rises away from it (shortest way round the torus), and a
        // valley follows row 0, centred on the z seam.
        let h: Vec<f64> = (0..s.n())
            .map(|k| {
                let (i, j) = ((k % 125) as i64, (k / 125) as i64);
                if (40..45).contains(&i) {
                    return -200.0;
                }
                let dx = ((i - 42).rem_euclid(r)).min((42 - i).rem_euclid(r)) as f64;
                let dz = (j.rem_euclid(r)).min((-j).rem_euclid(r)) as f64;
                5.0 + 2.0 * dx + 3.0 * dz * dz
            })
            .collect();
        let sea = sea_mask(&h, s, 1.0);
        let f = priority_flood(&h, &sea, s, true);
        let (rec, _) = receivers(&f, &sea, s);
        let order = stack(&rec);
        assert_eq!(order.len(), s.n(), "every cell reaches the sea");
        let idx = |i: i64, j: i64| (j.rem_euclid(r) * r + i.rem_euclid(r)) as usize;
        // Valley sides drain into the valley floor across the z seam.
        for i in [0, 10, 100, 120] {
            assert_eq!(rec[idx(i, -1)] as usize / 125, 0, "row 124 at {i}");
            assert_eq!(rec[idx(i, 1)] as usize / 125, 0, "row 1 at {i}");
        }
        // Along the floor, x = 120 drains east across the x seam (47 cells to the sea, against
        // 78 going west).
        let mut k = idx(120, 0);
        let mut crossed = false;
        while !sea[k] {
            let next = rec[k] as usize;
            crossed |= k % 125 == 124 && next.is_multiple_of(125);
            k = next;
        }
        assert!(crossed, "the valley must drain across x = 0");
        // Both sides of the z seam belong to the same basin.
        let out = outlets(&order, &rec);
        for i in [10, 100, 120] {
            assert_eq!(out[idx(i, -1)], out[idx(i, 1)]);
        }
        // Accumulation conserves the total.
        let acc = accumulate(&order, &rec, &vec![1.0; s.n()]);
        let at_sea: f64 = (0..s.n())
            .filter(|&k| rec[k] as usize == k)
            .map(|k| acc[k])
            .sum();
        assert_eq!(at_sea, s.n() as f64);
    }
}
