//! Determinism: the same seed and config always give the same world, whatever the number of
//! threads (CLAUDE.md, invariant 3).

use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate;

fn hash_with_threads(cfg: &WorldConfig, threads: usize) -> String {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("thread pool")
        .install(|| generate(cfg).expect("generation").0.hash())
}

fn check(scale: Scale) {
    let cfg = WorldConfig::new(42, scale);
    let a = hash_with_threads(&cfg, 1);
    let b = hash_with_threads(&cfg, 1);
    let c = hash_with_threads(&cfg, 8);
    assert_eq!(a, b, "two runs differ");
    assert_eq!(a, c, "the thread count changes the world");

    let other = hash_with_threads(&WorldConfig::new(43, scale), 4);
    assert_ne!(a, other, "different seeds give the same world");
}

#[test]
fn same_seed_same_hash_twenty_fifth() {
    check(Scale::TwentyFifth);
}

#[test]
#[ignore = "slower; run with --ignored"]
fn same_seed_same_hash_fifth() {
    check(Scale::Fifth);
}

#[test]
fn continental_fraction_stays_in_tolerance() {
    for seed in [1, 2, 3, 42] {
        for scale in [Scale::TwentyFifth, Scale::Fifth] {
            let (_, out) = generate(&WorldConfig::new(seed, scale)).expect("generation");
            assert!(
                out.report.continental_fraction_ok,
                "seed {seed} at 1/{}: {}",
                scale.divisor(),
                out.report
            );
        }
    }
}
