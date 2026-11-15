//! Final hydrography on the eroded fine grid (spec §4.2, §4.4 first draft): lakes, flow
//! directions, discharge, basins and the river network (Strahler order).
//!
//! An exact priority-flood (no epsilon) gives every depression its spill level. Depressions at
//! least `lake_min_depth_m` deep and `lake_min_area_m2` wide become lakes; the others are filled
//! (dry flats). Flow is then routed on the epsilon-filled surface, through the lakes.

use super::ErosionConfig;
use super::flow::{self, NO_DIR, Shape};

/// Hydrography of a fine grid.
pub struct Hydrography {
    /// Ground elevation (lake beds included; small depressions filled), m.
    pub elevation: Vec<f64>,
    /// Water depth of lakes, m (0 elsewhere).
    pub lake_depth: Vec<f64>,
    pub sea: Vec<bool>,
    /// D8 direction code, [`NO_DIR`] on the sea.
    pub dir: Vec<u8>,
    /// Discharge, m³/yr.
    pub discharge: Vec<f64>,
    /// Basin: 0 on the sea, then numbered by outlet (a sea cell or a lake) in index order.
    pub basin: Vec<u32>,
    /// Strahler order of the rivers, 0 off rivers.
    pub river_order: Vec<u8>,
    pub lakes: usize,
}

/// Lakes and filled depressions: returns `(ground, lake_depth, lake_id)` where `lake_id` is
/// the smallest cell index of the lake, or `u32::MAX`.
pub fn lakes(
    h: &[f64],
    sea: &[bool],
    shape: Shape,
    cfg: &ErosionConfig,
) -> (Vec<f64>, Vec<f64>, Vec<u32>) {
    let n = shape.n();
    let level = flow::priority_flood(h, sea, shape, false);
    let mut ground = h.to_vec();
    let mut depth = vec![0.0; n];
    let mut lake = vec![u32::MAX; n];
    let mut seen = vec![false; n];
    let mut members = Vec::new();
    let min_cells = (cfg.lake_min_area_m2 / (shape.cell * shape.cell)).ceil() as usize;
    for start in 0..n {
        if seen[start] || level[start] <= h[start] {
            continue;
        }
        // One depression: 8-connected flooded cells sharing the same water level.
        members.clear();
        members.push(start);
        seen[start] = true;
        let mut q = 0;
        let mut deepest = 0.0f64;
        while q < members.len() {
            let k = members[q];
            q += 1;
            deepest = deepest.max(level[k] - h[k]);
            for d in 0..8 {
                let m = shape.neighbour(k, d);
                if !seen[m] && level[m] > h[m] && level[m] == level[start] {
                    seen[m] = true;
                    members.push(m);
                }
            }
        }
        let is_lake = deepest >= cfg.lake_min_depth_m && members.len() >= min_cells;
        for &k in &members {
            if is_lake {
                depth[k] = level[k] - h[k];
                lake[k] = start as u32;
            } else {
                ground[k] = level[k];
            }
        }
    }
    (ground, depth, lake)
}

/// Strahler order on the receiver tree, for cells whose discharge reaches `threshold`.
pub fn strahler(order: &[u32], rec: &[u32], river: &[bool]) -> Vec<u8> {
    let n = rec.len();
    let mut out = vec![0u8; n];
    let mut best = vec![0u8; n];
    let mut count = vec![0u8; n];
    for &k in order.iter().rev() {
        let k = k as usize;
        if !river[k] {
            continue;
        }
        let o = match (best[k], count[k]) {
            (0, _) => 1,
            (b, c) if c >= 2 => b.saturating_add(1),
            (b, _) => b,
        };
        out[k] = o;
        let r = rec[k] as usize;
        if r != k {
            if o > best[r] {
                best[r] = o;
                count[r] = 1;
            } else if o == best[r] {
                count[r] = count[r].saturating_add(1);
            }
        }
    }
    out
}

/// Final hydrography of `h` with the rain volume per cell (m³/yr).
pub fn hydrography(
    h: &[f64],
    rain: &[f64],
    shape: Shape,
    sea_min_area_m2: f64,
    cfg: &ErosionConfig,
) -> Hydrography {
    let n = shape.n();
    let sea = flow::sea_mask(h, shape, sea_min_area_m2);
    let (ground, lake_depth, lake) = lakes(h, &sea, shape, cfg);
    // Route on the water surface (lakes flat), epsilon-filled so every cell drains.
    let surface: Vec<f64> = (0..n).map(|k| ground[k] + lake_depth[k]).collect();
    let filled = flow::priority_flood(&surface, &sea, shape, true);
    let (rec, dir) = flow::receivers(&filled, &sea, shape);
    let order = flow::stack(&rec);
    debug_assert_eq!(order.len(), n, "every cell drains to the sea");
    let discharge = flow::accumulate(&order, &rec, rain);
    // Outlet of each cell: the first sea cell or lake down its path.
    let mut outlet: Vec<u32> = (0..n as u32).collect();
    for &k in &order {
        let k = k as usize;
        outlet[k] = if lake[k] != u32::MAX {
            lake[k]
        } else if sea[k] {
            k as u32
        } else {
            outlet[rec[k] as usize]
        };
    }
    let mut used = vec![false; n];
    for k in 0..n {
        if !sea[k] {
            used[outlet[k] as usize] = true;
        }
    }
    let mut id = vec![0u32; n];
    let mut next = 1u32;
    for (k, u) in used.iter().enumerate() {
        if *u {
            id[k] = next;
            next += 1;
        }
    }
    let basin = (0..n)
        .map(|k| if sea[k] { 0 } else { id[outlet[k] as usize] })
        .collect();
    let threshold = cfg.river_threshold_m3s * super::SECONDS_PER_YEAR;
    let river: Vec<bool> = (0..n)
        .map(|k| !sea[k] && discharge[k] >= threshold)
        .collect();
    let river_order = strahler(&order, &rec, &river);
    let lakes = (0..n).filter(|&k| lake[k] == k as u32).count();
    debug_assert!(dir.iter().zip(&sea).all(|(&d, &s)| s == (d == NO_DIR)));
    Hydrography {
        elevation: ground,
        lake_depth,
        sea,
        dir,
        discharge,
        basin,
        river_order,
        lakes,
    }
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

    /// Sea on column 0; land at 100 m with two pits: a deep wide one (lake) and a shallow one
    /// (filled).
    fn pits() -> (Shape, Vec<f64>, ErosionConfig) {
        let s = shape(50);
        let mut h: Vec<f64> = (0..s.n())
            .map(|k| {
                if k % 50 == 0 {
                    -100.0
                } else {
                    100.0 + (k % 50) as f64
                }
            })
            .collect();
        for j in 10..15 {
            for i in 10..15 {
                h[j * 50 + i] = 80.0;
            }
        }
        h[40 * 50 + 30] = 128.0;
        let cfg = ErosionConfig {
            lake_min_depth_m: 2.0,
            lake_min_area_m2: 10.0 * s.cell * s.cell,
            ..ErosionConfig::default()
        };
        (s, h, cfg)
    }

    #[test]
    fn deep_wide_pits_are_lakes_others_are_filled() {
        let (s, h, cfg) = pits();
        let sea = flow::sea_mask(&h, s, 1.0);
        let (ground, depth, lake) = lakes(&h, &sea, s, &cfg);
        // The 5×5 pit at 80 m spills at its lowest rim cell: 100 + 9 = 109 m.
        assert_eq!(depth[12 * 50 + 12], 29.0);
        assert_eq!(lake[12 * 50 + 12], (10 * 50 + 10) as u32);
        assert_eq!(ground[12 * 50 + 12], 80.0);
        // The one-cell pit is filled to its spill level (128 m against 129 m: one metre).
        assert_eq!(depth[40 * 50 + 30], 0.0);
        assert_eq!(ground[40 * 50 + 30], 129.0);
        assert_eq!(lake[40 * 50 + 30], u32::MAX);
    }

    #[test]
    fn rivers_flow_through_lakes_to_the_sea() {
        let (s, h, cfg) = pits();
        let rain = vec![s.cell * s.cell; s.n()];
        let hy = hydrography(&h, &rain, s, 1.0, &cfg);
        assert_eq!(hy.lakes, 1);
        let lake_basin = hy.basin[12 * 50 + 12];
        assert!(lake_basin != 0);
        // The lake rim drains into the lake: same basin.
        assert_eq!(hy.basin[12 * 50 + 16], lake_basin);
        // Every land cell drains (direction set), the sea has none.
        for k in 0..s.n() {
            assert_eq!(hy.sea[k], hy.dir[k] == NO_DIR);
        }
        // The lake outflow carries at least the lake's catchment.
        let max_q = hy.discharge.iter().cloned().fold(0.0, f64::max);
        assert!(max_q >= 25.0 * s.cell * s.cell);
    }

    #[test]
    fn strahler_orders() {
        // A binary tree: 4 sources → 2 order-1 confluences → 1 order-2 confluence.
        //   0 ← 1 ← {2 ← {3, 4}, 5 ← {6, 7}}
        let rec = vec![0u32, 0, 1, 2, 2, 1, 5, 5];
        let order = flow::stack(&rec);
        let river = vec![true; 8];
        let o = strahler(&order, &rec, &river);
        assert_eq!(&o[3..5], &[1, 1]);
        assert_eq!(o[2], 2);
        assert_eq!(o[5], 2);
        assert_eq!(o[1], 3);
        assert_eq!(o[0], 3);
        // A tributary of lower order does not raise the order.
        let rec = vec![0u32, 0, 1, 2, 2, 1];
        let o = strahler(&flow::stack(&rec), &rec, &[true; 6]);
        assert_eq!(o[1], 2);
    }
}
