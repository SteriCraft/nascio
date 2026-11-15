//! Calibration of an elevation distribution on target hypsometric bands, by rank.
//!
//! The mapping is monotone: the order of the cells is kept (ridges stay the shallowest ocean,
//! trenches the deepest, orogens the highest land), only the values follow the target
//! distribution. Ties are broken by cell index, so the result is deterministic.

/// Elevation at quantile `q ∈ [0, 1]` of piecewise-uniform bands `[low, high, weight]`.
pub fn inverse_cdf(bands: &[[f64; 3]], q: f64) -> f64 {
    let total: f64 = bands.iter().map(|b| b[2]).sum();
    let mut target = q.clamp(0.0, 1.0) * total;
    for b in bands {
        if target <= b[2] {
            return b[0] + (b[1] - b[0]) * (target / b[2]);
        }
        target -= b[2];
    }
    bands.last().map_or(0.0, |b| b[1])
}

/// Replaces `values[k]` for every `k` in `cells` by the band value at its rank.
pub fn match_ranks(values: &mut [f64], cells: &[usize], bands: &[[f64; 3]]) {
    let mut order: Vec<usize> = cells.to_vec();
    order.sort_by(|&a, &b| values[a].total_cmp(&values[b]).then(a.cmp(&b)));
    let n = order.len() as f64;
    for (rank, &k) in order.iter().enumerate() {
        values[k] = inverse_cdf(bands, (rank as f64 + 0.5) / n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BANDS: [[f64; 3]; 2] = [[0.0, 100.0, 3.0], [100.0, 1_000.0, 1.0]];

    #[test]
    fn inverse_cdf_hits_band_edges() {
        assert_eq!(inverse_cdf(&BANDS, 0.0), 0.0);
        assert_eq!(inverse_cdf(&BANDS, 0.75), 100.0);
        assert_eq!(inverse_cdf(&BANDS, 1.0), 1_000.0);
        assert!((inverse_cdf(&BANDS, 0.375) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn matching_is_monotone_and_follows_bands() {
        let mut v: Vec<f64> = (0..1_000).map(|i| f64::from((i * 7_919) % 1_000)).collect();
        let original = v.clone();
        let cells: Vec<usize> = (0..1_000).collect();
        match_ranks(&mut v, &cells, &BANDS);
        for a in 0..1_000 {
            for b in 0..1_000 {
                if original[a] < original[b] {
                    assert!(v[a] <= v[b]);
                }
            }
        }
        let below_100 = v.iter().filter(|&&x| x < 100.0).count();
        assert_eq!(below_100, 750);
        assert!(v.iter().all(|&x| (0.0..=1_000.0).contains(&x)));
    }

    #[test]
    fn only_selected_cells_change() {
        let mut v = vec![5.0, -3.0, 7.0];
        match_ranks(&mut v, &[0, 2], &BANDS);
        assert_eq!(v[1], -3.0);
        assert!(v[0] < v[2]);
    }
}
