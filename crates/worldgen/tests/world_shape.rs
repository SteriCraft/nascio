//! Shape of generated worlds: elevation bounds (spec §3) and Earth-like hypsometry.

use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate;

#[test]
fn elevations_follow_spec_targets() {
    for seed in [1, 2, 42] {
        let (world, out) = generate(&WorldConfig::new(seed, Scale::Fifth)).expect("generation");
        let e = world.layer("elevation_tectonic").expect("elevation");
        let n = e.width * e.height;
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for i in 0..n {
            let v = e.data.get_f64(i);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        // Pre-M5 review: a shallower ocean, trenches at −2 500 m at most.
        assert!(lo >= -2_500.0 - 1e-6, "seed {seed}: min {lo}");
        assert!(hi <= 3_000.0 + 1e-6, "seed {seed}: max {hi}");
        let r = &out.report;
        // 35 % continental crust: about 29 % land (as on Earth), a few % shelves.
        assert!((0.25..=0.33).contains(&r.land_fraction), "seed {seed}: {r}");
        assert!(
            (0.02..=0.09).contains(&r.shelf_fraction),
            "seed {seed}: {r}"
        );
    }
}

#[test]
fn hotspot_islands_emerge_on_every_seed() {
    for seed in [42, 7, 1234, 2026] {
        let (world, _) = generate(&WorldConfig::new(seed, Scale::Fifth)).expect("generation");
        let e = world.layer("elevation_tectonic").expect("elevation");
        let crust = world.layer("crust_type").expect("crust");
        let flags = world.layer("history_flags").expect("flags");
        let islands = (0..e.width * e.height)
            .filter(|&i| {
                crust.data.get_f64(i) == 0.0
                    && (flags.data.get_f64(i) as u8) & nascio_worldgen::tectonics::flags::HOTSPOT
                        != 0
                    && e.data.get_f64(i) > 0.0
            })
            .count();
        println!("seed {seed}: {islands} emerged hotspot cells");
        assert!(
            islands >= 3,
            "seed {seed}: only {islands} emerged hotspot cells"
        );
    }
}

#[test]
fn hotspot_islands_survive_erosion() {
    for seed in [42, 7, 1234, 2026] {
        let (world, _) = generate(&WorldConfig::new(seed, Scale::Fifth)).expect("generation");
        let fine = world.layer("elevation").expect("fine elevation");
        let crust = world.layer("crust_type").expect("crust");
        let flags = world.layer("history_flags").expect("flags");
        let ratio = fine.width / crust.width;
        let islands = (0..fine.width * fine.height)
            .filter(|&k| {
                let (i, j) = (k % fine.width, k / fine.width);
                let t = (j / ratio) * crust.width + i / ratio;
                crust.data.get_f64(t) == 0.0
                    && (flags.data.get_f64(t) as u8) & nascio_worldgen::tectonics::flags::HOTSPOT
                        != 0
                    && fine.data.get_f64(k) > 0.0
            })
            .count();
        println!("seed {seed}: {islands} emerged hotspot cells after erosion");
        assert!(islands >= 3, "seed {seed}: only {islands} cells");
    }
}

/// The sea floor never drops faster than `max_seafloor_slope` away from the coast or a
/// shallower sea cell, on the tectonic and the eroded grids, across the seams included
/// (neighbours wrap).
#[test]
fn sea_floor_slope_is_bounded() {
    let cfg = WorldConfig::new(42, Scale::Fifth);
    let max = cfg.tectonics.max_seafloor_slope;
    let (world, _) = generate(&cfg).expect("generation");
    for name in ["elevation_tectonic", "elevation"] {
        let e = world.layer(name).expect("layer");
        let (w, h) = (e.width, e.height);
        let cell = e.cell_m;
        let at = |i: usize, j: usize| e.data.get_f64((j % h) * w + i % w);
        let mut worst: f64 = 0.0;
        for j in 0..h {
            for i in 0..w {
                let v = at(i, j);
                if v >= 0.0 {
                    continue;
                }
                for (ni, nj) in [(i + 1, j), (i + w - 1, j), (i, j + 1), (i, j + h - 1)] {
                    worst = worst.max((at(ni, nj).min(0.0) - v) / cell);
                }
            }
        }
        println!("{name}: steepest sea-floor step {worst:.4}");
        assert!(worst <= max + 1e-3, "{name}: slope {worst} > {max}");
    }
}
