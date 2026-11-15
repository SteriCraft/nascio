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
