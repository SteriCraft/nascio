//! Climate sanity (spec §4.3): continuity across the seam, plausible zonal means, seasons that
//! fade out at the seam, harmonics continuous over the year.

use nascio_core::climate::Harmonics;
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

#[test]
fn climate_is_continuous_across_the_seam_and_zonally_plausible() {
    let g = generate_full(&WorldConfig::new(42, Scale::Fifth)).expect("generation");
    let t = &g.climate.temperature;
    let res = t.x0.width();
    let row_mean = |j: usize, f: &dyn Fn(usize) -> f64| {
        (0..res).map(|i| f(j * res + i)).sum::<f64>() / res as f64
    };
    let x0 = |k: usize| f64::from(t.x0.data()[k]);
    let a1 = |k: usize| f64::from(t.a1.data()[k]);
    // Rows 0 and res−1 sit at the same seam distance on both sides of the seam: no systematic
    // jump, and per-column jumps (land against sea) no larger than between two ordinary rows.
    let jump = |ja: usize, jb: usize| -> (f64, f64) {
        let d: Vec<f64> = (0..res)
            .map(|i| x0(ja * res + i) - x0(jb * res + i))
            .collect();
        (
            d.iter().sum::<f64>() / res as f64,
            d.iter().fold(0.0f64, |m, v| m.max(v.abs())),
        )
    };
    let (seam_mean, seam_max) = jump(0, res - 1);
    let (_, ordinary_max) = (1..res / 2 - 1)
        .map(|j| jump(j, j + 1))
        .fold((0.0, 0.0f64), |a, b| (0.0, a.1.max(b.1)));
    assert!(seam_mean.abs() < 2.0, "systematic seam jump {seam_mean}");
    assert!(
        seam_max <= ordinary_max + 1.0,
        "seam {seam_max} vs ordinary {ordinary_max}"
    );
    // Seasons vanish at the seam, not at the mid-latitudes.
    assert!(row_mean(0, &a1) < 0.3 * row_mean(res / 4, &a1));
    // Zonal means: warm equator, cold seam.
    let eq = 0.5 * (row_mean(res / 2 - 1, &x0) + row_mean(res / 2, &x0));
    let seam = 0.5 * (row_mean(0, &x0) + row_mean(res - 1, &x0));
    assert!((20.0..32.0).contains(&eq), "equator {eq}");
    assert!((-28.0..-10.0).contains(&seam), "seam {seam}");
    // Polar band: heavy precipitation and permanent snow on land.
    let p0 = row_mean(0, &|k| f64::from(g.climate.precipitation.x0.data()[k]));
    assert!(p0 >= 1_000.0, "polar precipitation {p0}");
    // Continuity over the year of an arbitrary cell.
    let h: Harmonics = t.cell(res * res / 3);
    assert!((h.eval(0.0) - h.eval(365.0)).abs() < 1e-9);
}
