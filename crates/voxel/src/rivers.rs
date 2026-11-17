//! River water in the chunks (M6): the rivers of the final hydrology (32 m grid) as a smoothed
//! polyline along `flow_direction`, with a channel carved to the river depth and water up to a
//! surface that never rises downstream.
//!
//! - Water from `min_width_m` on; downstream of a water cell every river cell keeps at least
//!   that width, down to its lake or the sea.
//! - The trace joins the cells of the river, moved towards the middle of their upstream (widest)
//!   and downstream neighbours (Laplacian smoothing, `smooth_passes` times).
//! - The surface of a cell starts at its ground height, is capped by its upstream cells
//!   (never rises downstream) and is at least the level of the lake or sea it reaches; it is
//!   interpolated along the segments and rounded to whole blocks (steps on steep slopes).
//! - Inside the channel the ground is lowered under the water (parabolic profile, at least one
//!   block of water); on the banks (`bank_m` wide) it is raised to the water surface so that
//!   the water never hangs above lower ground.

use nascio_core::torus::{delta_f64, wrap_f64};
use nascio_worldgen::erosion::flow::{D8, NO_DIR};
use nascio_worldgen::grid::PeriodicGrid;

/// Parameters of the river water.
#[derive(Clone, Copy, Debug)]
pub struct RiverParams {
    pub min_width_m: f64,
    pub bank_m: f64,
    pub smooth_passes: usize,
}

/// One segment of the trace, from `a` to `a + d`.
#[derive(Clone, Copy, Debug)]
struct Seg {
    ax: f64,
    az: f64,
    dx: f32,
    dz: f32,
    /// Half width, depth (m) and water surface (m) at both ends.
    half: [f32; 2],
    depth: [f32; 2],
    level: [f32; 2],
}

/// The river at one column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiverHit {
    /// In the channel: water fills `bed < y < water_top`.
    Channel { water_top: i32, bed: i32 },
    /// On a bank: the ground reaches at least `top`, level with the water.
    Bank { top: i32 },
}

/// The river traces of a world, with a spatial index on the elevation grid.
#[derive(Clone, Debug)]
pub struct Rivers {
    segs: Vec<Seg>,
    /// `(cell, segment)` pairs, sorted: the segments that may reach each cell.
    index: Vec<(u32, u32)>,
    res: usize,
    cell: f64,
    bank: f64,
}

impl Rivers {
    /// Builds the traces from the hydrology layers (all on the elevation grid). `ground` is the
    /// eroded elevation; `lake_level` the lake surface per cell (`-∞` off lakes).
    pub fn new(
        dir: &PeriodicGrid<u8>,
        width: &PeriodicGrid<f32>,
        depth: &PeriodicGrid<f32>,
        ground: &PeriodicGrid<f32>,
        lake_level: Option<&PeriodicGrid<f32>>,
        p: RiverParams,
    ) -> Self {
        let res = dir.width();
        let cell = dir.cell_size();
        let n = res * res;
        let d = dir.data();
        let lake = |k: usize| lake_level.map_or(f32::NEG_INFINITY, |l| l.data()[k]);
        let step = |k: usize, code: u8| {
            let (i, j) = ((k % res) as i64, (k / res) as i64);
            let (di, dj) = D8[usize::from(code)];
            dir.index(i + di, j + dj)
        };
        let down = |k: usize| (d[k] != NO_DIR).then(|| step(k, d[k]));
        // Land river cells (not lake, not sea).
        let land = |k: usize| d[k] != NO_DIR && !lake(k).is_finite();
        let min_w = p.min_width_m as f32;
        let mut water = vec![false; n];
        for k in 0..n {
            if !(land(k) && width.data()[k] >= min_w) || water[k] {
                continue;
            }
            // Downstream of a water cell, the river keeps water down to its lake or the sea.
            let mut cur = k;
            loop {
                water[cur] = true;
                match down(cur) {
                    Some(m) if land(m) && !water[m] => cur = m,
                    _ => break,
                }
            }
        }
        let wdown = |k: usize| down(k).filter(|&m| water[m]);
        // Topological order, upstream first (Kahn, FIFO from the sources in index order).
        let mut indeg = vec![0u8; n];
        for k in (0..n).filter(|&k| water[k]) {
            if let Some(m) = wdown(k) {
                indeg[m] += 1;
            }
        }
        let mut order: Vec<usize> = (0..n).filter(|&k| water[k] && indeg[k] == 0).collect();
        let mut head = 0;
        while head < order.len() {
            if let Some(m) = wdown(order[head]) {
                indeg[m] -= 1;
                if indeg[m] == 0 {
                    order.push(m);
                }
            }
            head += 1;
        }
        // Surface: ground, capped from upstream, at least the level of the receiving water.
        let mut level: Vec<f32> = ground.data().to_vec();
        for &k in &order {
            if let Some(m) = wdown(k) {
                level[m] = level[m].min(level[k]);
            }
        }
        let mut end = vec![0.0f32; n];
        for &k in order.iter().rev() {
            end[k] = match down(k) {
                Some(m) if water[m] => end[m],
                Some(m) if lake(m).is_finite() => lake(m),
                _ => 0.0,
            };
            level[k] = level[k].max(end[k]);
        }
        let w = |k: usize| width.data()[k].max(min_w);
        // Main upstream cell (widest, first in index order on ties).
        let mut up = vec![u32::MAX; n];
        for k in (0..n).filter(|&k| water[k]) {
            if let Some(m) = wdown(k)
                && (up[m] == u32::MAX || w(k) > w(up[m] as usize))
            {
                up[m] = k as u32;
            }
        }
        // Trace: offsets from the cell centres, smoothed.
        let offset_to = |k: usize, m: usize| {
            let (ki, kj) = ((k % res) as f64, (k / res) as f64);
            let (mi, mj) = ((m % res) as f64, (m / res) as f64);
            let r = res as f64;
            let dw = |a: f64, b: f64| {
                let v = (b - a).rem_euclid(r);
                if v > r / 2.0 { v - r } else { v }
            };
            (dw(ki, mi) * cell, dw(kj, mj) * cell)
        };
        let mut off = vec![(0.0f64, 0.0f64); n];
        for _ in 0..p.smooth_passes {
            let prev = off.clone();
            for &k in &order {
                let (Some(m), u) = (wdown(k), up[k]) else {
                    continue;
                };
                if u == u32::MAX {
                    continue;
                }
                let u = u as usize;
                let (ux, uz) = offset_to(k, u);
                let (mx, mz) = offset_to(k, m);
                off[k] = (
                    0.5 * prev[k].0 + 0.25 * (ux + prev[u].0) + 0.25 * (mx + prev[m].0),
                    0.5 * prev[k].1 + 0.25 * (uz + prev[u].1) + 0.25 * (mz + prev[m].1),
                );
            }
        }
        let depth_of = |k: usize| depth.data()[k].max(0.0);
        let mut segs = Vec::new();
        for &k in &order {
            let Some(m) = down(k) else { continue };
            let (cx, cz) = dir.cell_center(k % res, k / res);
            let (ax, az) = (wrap_f64(cx + off[k].0), wrap_f64(cz + off[k].1));
            let (mx, mz) = offset_to(k, m);
            let (bx, bz) = (mx + off[m].0 - off[k].0, mz + off[m].1 - off[k].1);
            let (hb, db, lb) = if water[m] {
                (w(m), depth_of(m), level[m])
            } else {
                (w(k), depth_of(k), end[k])
            };
            segs.push(Seg {
                ax,
                az,
                dx: bx as f32,
                dz: bz as f32,
                half: [0.5 * w(k), 0.5 * hb],
                depth: [depth_of(k), db],
                level: [level[k], lb],
            });
        }
        let mut index = Vec::new();
        for (s, g) in segs.iter().enumerate() {
            let r = f64::from(g.half[0].max(g.half[1])) + p.bank_m + 1.0;
            let (x0, x1) = (
                g.ax + f64::from(g.dx.min(0.0)),
                g.ax + f64::from(g.dx.max(0.0)),
            );
            let (z0, z1) = (
                g.az + f64::from(g.dz.min(0.0)),
                g.az + f64::from(g.dz.max(0.0)),
            );
            let ci = |v: f64| (v / cell).floor() as i64;
            for j in ci(z0 - r)..=ci(z1 + r) {
                for i in ci(x0 - r)..=ci(x1 + r) {
                    index.push((dir.index(i, j) as u32, s as u32));
                }
            }
        }
        index.sort_unstable();
        index.dedup();
        Self {
            segs,
            index,
            res,
            cell,
            bank: p.bank_m,
        }
    }

    /// Number of trace segments.
    pub fn len(&self) -> usize {
        self.segs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    /// The river at world position `(x, z)` (column centre), if any.
    pub fn at(&self, x: f64, z: f64) -> Option<RiverHit> {
        let i = (wrap_f64(x) / self.cell).floor() as usize % self.res;
        let j = (wrap_f64(z) / self.cell).floor() as usize % self.res;
        let key = (j * self.res + i) as u32;
        let lo = self.index.partition_point(|e| e.0 < key);
        let mut best: Option<(f64, &Seg, f64, f64)> = None;
        for &(c, s) in &self.index[lo..] {
            if c != key {
                break;
            }
            let g = &self.segs[s as usize];
            let (px, pz) = (delta_f64(g.ax, x), delta_f64(g.az, z));
            let (dx, dz) = (f64::from(g.dx), f64::from(g.dz));
            let len2 = dx * dx + dz * dz;
            let t = if len2 > 0.0 {
                ((px * dx + pz * dz) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let dist = (px - t * dx).hypot(pz - t * dz);
            let lerp = |v: [f32; 2]| f64::from(v[0]) + (f64::from(v[1]) - f64::from(v[0])) * t;
            let half = lerp(g.half);
            let e = dist - half;
            if best.is_none_or(|b| e < b.0) {
                best = Some((e, g, t, dist / half.max(1e-3)));
            }
        }
        let (e, g, t, frac) = best?;
        if e > self.bank {
            return None;
        }
        let lerp = |v: [f32; 2]| f64::from(v[0]) + (f64::from(v[1]) - f64::from(v[0])) * t;
        let water_top = lerp(g.level).round() as i32;
        if e > 0.0 {
            return Some(RiverHit::Bank { top: water_top - 1 });
        }
        let blocks = (lerp(g.depth) * (1.0 - frac * frac)).round().max(1.0) as i32;
        Some(RiverHit::Channel {
            water_top,
            bed: water_top - 1 - blocks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nascio_core::consts::WORLD_SIZE_F64;

    /// A straight river along +x on row 0 of a 32 m grid (crossing the x seam), falling 1 m per
    /// cell from x = 2000 m to the sea, with a pit that must not raise the surface downstream.
    fn straight() -> Rivers {
        let res = (WORLD_SIZE_F64 / 32.0) as usize;
        let cell = 32.0;
        let len = 70usize;
        let start = res - 40;
        let on = |i: usize| (i + res - start) % res < len;
        let dir = PeriodicGrid::from_fn(
            res,
            res,
            cell,
            |i, j| {
                if j == 0 && on(i) { 0 } else { NO_DIR }
            },
        );
        let width = PeriodicGrid::from_fn(
            res,
            res,
            cell,
            |i, j| {
                if j == 0 && on(i) { 4.0 } else { 0.0 }
            },
        );
        let depth = width.map(|&w| w / 2.0);
        let ground = PeriodicGrid::from_fn(res, res, cell, |i, j| {
            let s = (i + res - start) % res;
            if j == 0 && on(i) {
                if s == 30 { 10.0 } else { 80.0 - s as f32 }
            } else {
                -5.0
            }
        });
        Rivers::new(
            &dir,
            &width,
            &depth,
            &ground,
            None,
            RiverParams {
                min_width_m: 1.5,
                bank_m: 2.0,
                smooth_passes: 2,
            },
        )
    }

    #[test]
    fn surface_never_rises_downstream_and_crosses_the_seam() {
        let r = straight();
        assert!(!r.is_empty());
        let z = 16.5;
        let mut prev = i32::MAX;
        let x0 = WORLD_SIZE_F64 - 40.0 * 32.0;
        for k in 0..(69 * 32) {
            let x = wrap_f64(x0 + 16.5 + f64::from(k));
            match r.at(x, z) {
                Some(RiverHit::Channel { water_top, bed }) => {
                    assert!(water_top <= prev, "rises at x = {x}");
                    assert!(bed < water_top - 1);
                    prev = water_top;
                }
                other => panic!("no channel at x = {x}: {other:?}"),
            }
        }
        // After the pit (cell 30, 10 m) the surface stays at or below 10 m.
        assert!(prev <= 10);
    }

    #[test]
    fn banks_then_nothing_away_from_the_river() {
        let r = straight();
        let x = 500.5;
        assert!(matches!(r.at(x, 16.5), Some(RiverHit::Channel { .. })));
        assert!(matches!(r.at(x, 16.5 + 3.0), Some(RiverHit::Bank { .. })));
        assert_eq!(r.at(x, 16.5 + 10.0), None);
    }
}
