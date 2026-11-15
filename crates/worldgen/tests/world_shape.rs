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
        assert!(lo >= -3_500.0 - 1e-6, "seed {seed}: min {lo}");
        assert!(hi <= 3_000.0 + 1e-6, "seed {seed}: max {hi}");
        let r = &out.report;
        // 30 % continental crust: about a quarter of the surface is land, a few % shelves.
        assert!((0.20..=0.30).contains(&r.land_fraction), "seed {seed}: {r}");
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
