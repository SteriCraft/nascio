//! Maximum slope of the sea floor (pre-M5 review).
//!
//! The floor of the sea is raised wherever it drops faster than `max_slope` away from the
//! coast, a shallower sea cell or a sediment fan: `h(x) ≥ max_y (h(y) − max_slope · |x − y|)`.
//! Land counts as sea level (its height never props the floor up above 0), so mountain chains
//! that plunge into the sea continue under it at most at `max_slope`, and the vertical fronts
//! of the sediment fans become aprons. Only cells of `raisable` change, they only rise, and
//! they stay below sea level.
//!
//! The upper envelope is computed with forward and backward chamfer sweeps over the periodic
//! grid (8 neighbours, octagonal distance), repeated until nothing changes: sequential and
//! deterministic, and continuous across the seams because neighbours wrap.

/// Raises the cells of `raisable` so that no slope towards them exceeds `max_slope`. `h` is a
/// row-major `w × hgt` periodic grid of cell size `cell` (m). Returns the added volume (m³).
pub fn limit_slope(
    h: &mut [f64],
    w: usize,
    hgt: usize,
    cell: f64,
    max_slope: f64,
    raisable: &[bool],
) -> f64 {
    assert_eq!(h.len(), w * hgt);
    assert_eq!(raisable.len(), h.len());
    if max_slope <= 0.0 || w == 0 || hgt == 0 {
        return 0.0;
    }
    let mut v: Vec<f64> = h
        .iter()
        .zip(raisable)
        .map(|(&e, &r)| if r { e } else { e.min(0.0) })
        .collect();
    let straight = max_slope * cell;
    let diagonal = max_slope * cell * std::f64::consts::SQRT_2;
    // Neighbours already visited by a forward sweep (row-major order); the backward sweep
    // uses the opposite offsets.
    let forward: [(isize, isize, f64); 4] = [
        (-1, 0, straight),
        (-1, -1, diagonal),
        (0, -1, straight),
        (1, -1, diagonal),
    ];
    let wrap = |i: usize, d: isize, n: usize| (i as isize + d).rem_euclid(n as isize) as usize;
    // Each pair of sweeps carries a constraint across the whole grid in its directions; the
    // wrap needs a few more. The cap only guards against pathological inputs.
    for _ in 0..64 {
        let mut changed = false;
        for backward in [false, true] {
            let sign = if backward { -1 } else { 1 };
            for jj in 0..hgt {
                let j = if backward { hgt - 1 - jj } else { jj };
                for ii in 0..w {
                    let i = if backward { w - 1 - ii } else { ii };
                    let k = j * w + i;
                    if !raisable[k] {
                        continue;
                    }
                    let mut best = v[k];
                    for &(di, dj, drop) in &forward {
                        let n = wrap(j, sign * dj, hgt) * w + wrap(i, sign * di, w);
                        let cand = v[n] - drop;
                        if cand > best {
                            best = cand;
                        }
                    }
                    if best > v[k] + 1e-9 {
                        v[k] = best;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    let area = cell * cell;
    let mut added = 0.0;
    for (k, e) in h.iter_mut().enumerate() {
        if raisable[k] && v[k] > *e {
            added += (v[k] - *e) * area;
            *e = v[k];
        }
    }
    added
}

/// Coastal cone: land cells (`!sea`) are lowered to at most `max_slope × distance to the sea`
/// (chamfer distance, periodic), so a chain cannot plunge into the sea as a wall. Only the
/// distance to the coast counts: slopes between land cells inland are untouched, and summits
/// far enough from the sea keep their height. Returns the removed volume (m³).
pub fn limit_coastal_land(
    h: &mut [f64],
    w: usize,
    hgt: usize,
    cell: f64,
    max_slope: f64,
    sea: &[bool],
) -> f64 {
    assert_eq!(sea.len(), h.len());
    if max_slope <= 0.0 {
        return 0.0;
    }
    // Distance cone from the sea: sea cells at 0, land raised to −max_slope · distance.
    let land: Vec<bool> = sea.iter().map(|&s| !s).collect();
    let mut cone: Vec<f64> = sea.iter().map(|&s| if s { 0.0 } else { -1e12 }).collect();
    limit_slope(&mut cone, w, hgt, cell, max_slope, &land);
    let area = cell * cell;
    let mut removed = 0.0;
    for (k, e) in h.iter_mut().enumerate() {
        let top = -cone[k];
        if land[k] && *e > top {
            removed += (*e - top) * area;
            *e = top;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn max_slope(h: &[f64], w: usize, hgt: usize, cell: f64, mask: &[bool]) -> f64 {
        let mut m: f64 = 0.0;
        for j in 0..hgt {
            for i in 0..w {
                let k = j * w + i;
                if !mask[k] {
                    continue;
                }
                for (di, dj) in [(1isize, 0isize), (0, 1), (-1, 0), (0, -1)] {
                    let n = (j as isize + dj).rem_euclid(hgt as isize) as usize * w
                        + (i as isize + di).rem_euclid(w as isize) as usize;
                    let src = if mask[n] { h[n] } else { h[n].min(0.0) };
                    m = m.max((src - h[k]) / cell);
                }
            }
        }
        m
    }

    /// A cliff: land (and a plateau at −1 m) next to a −3 000 m abyss, across the x seam.
    #[test]
    fn cliffs_become_slopes_across_the_seam() {
        let (w, hgt, cell) = (1_000, 8, 32.0);
        let mut h = vec![0.0; w * hgt];
        for j in 0..hgt {
            for i in 0..w {
                // Land on columns 990..1000 and 0..5 (crossing the seam), a −1 m fan on
                // columns 5..10, the abyss elsewhere.
                h[j * w + i] = match i {
                    990.. | 0..5 => 2_500.0,
                    5..10 => -1.0,
                    _ => -3_000.0,
                };
            }
        }
        let mask: Vec<bool> = h.iter().map(|&e| e < 0.0).collect();
        let before = h.clone();
        let added = limit_slope(&mut h, w, hgt, cell, 0.25, &mask);
        assert!(added > 0.0);
        assert!(max_slope(&h, w, hgt, cell, &mask) <= 0.25 + 1e-9);
        for k in 0..h.len() {
            assert!(h[k] >= before[k], "cells only rise");
            assert_eq!(mask[k], h[k] < 0.0, "the sea keeps its extent");
        }
        // Exact profiles on both sides of the land block, which straddles the x seam: 8 m per
        // 32 m cell down from the coast (west) and from the −1 m fan (east).
        let row = &h[..w];
        for d in 0..80 {
            let west = row[989 - d];
            let east = row[10 + d];
            assert!(
                (west - (-8.0 * (d + 1) as f64).max(-3_000.0)).abs() < 1e-6,
                "{d}: {west}"
            );
            assert!((east - (-1.0 - 8.0 * (d + 1) as f64).max(-3_000.0)).abs() < 1e-6);
        }
        // Far from the coast the abyss is untouched.
        assert_eq!(row[500], -3_000.0);
    }

    /// A 3 000 m plateau straddling the x seam, sea elsewhere: its edges become a 0.5 cone
    /// from both coasts, its middle keeps its height.
    #[test]
    fn coastal_cone_lowers_walls_only_near_the_sea() {
        let (w, hgt, cell) = (1_000, 4, 32.0);
        let mut h: Vec<f64> = (0..w * hgt)
            .map(|k| match k % w {
                800.. | 0..200 => 3_000.0,
                _ => -500.0,
            })
            .collect();
        let sea: Vec<bool> = h.iter().map(|&e| e < 0.0).collect();
        let removed = limit_coastal_land(&mut h, w, hgt, cell, 0.5, &sea);
        assert!(removed > 0.0);
        let row = &h[..w];
        for d in 0..150 {
            // d + 1 cells from the coast on both sides of the seam: 16 m per cell.
            let top = (16.0 * (d + 1) as f64).min(3_000.0);
            assert!(
                (row[199 - d] - top).abs() < 1e-6,
                "east {d}: {}",
                row[199 - d]
            );
            assert!(
                (row[800 + d] - top).abs() < 1e-6,
                "west {d}: {}",
                row[800 + d]
            );
        }
        assert_eq!(
            row[0], 3_000.0,
            "the middle, 200 cells inland, is untouched"
        );
        assert!(
            row[200..800].iter().all(|&e| e == -500.0),
            "the sea is untouched"
        );
    }

    #[test]
    fn gentle_floors_are_untouched() {
        let (w, hgt, cell) = (64, 64, 160.0);
        let h0: Vec<f64> = (0..w * hgt)
            .map(|k| -1_000.0 - 10.0 * ((k % w) as f64 * 0.1).sin())
            .collect();
        let mut h = h0.clone();
        let mask = vec![true; w * hgt];
        assert_eq!(limit_slope(&mut h, w, hgt, cell, 0.25, &mask), 0.0);
        assert_eq!(h, h0);
    }
}
