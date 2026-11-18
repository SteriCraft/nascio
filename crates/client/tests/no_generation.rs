//! Invariant 5: the client never generates terrain. The compiler guarantees it as long as
//! `nascio-client` (and what it builds on) does not depend on a generation crate; this test
//! reads the dependency tree and fails the day one of them appears.

use std::process::Command;

/// Crates that hold generation code.
const GENERATION: [&str; 3] = ["nascio-world", "nascio-worldgen", "nascio-voxel"];

/// Names of the packages `package` depends on (normal dependencies, transitively).
fn dependencies(package: &str) -> Vec<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "tree",
            "--offline",
            "-e",
            "normal",
            "--prefix",
            "none",
            "-p",
        ])
        .arg(package)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("runs cargo tree");
    assert!(
        out.status.success(),
        "cargo tree -p {package}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut names: Vec<String> = String::from_utf8(out.stdout)
        .expect("utf-8")
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    names.sort();
    names.dedup();
    assert!(
        names.iter().any(|n| n == package),
        "{package} missing from its own tree"
    );
    names
}

#[test]
fn client_side_crates_do_not_depend_on_generation() {
    for package in ["nascio-client", "nascio-protocol", "nascio-chunk"] {
        let deps = dependencies(package);
        for g in GENERATION {
            assert!(
                !deps.iter().any(|d| d == g),
                "{package} depends on the generation crate {g}"
            );
        }
        assert!(
            deps.iter().any(|d| d == "nascio-chunk"),
            "{package} should build on nascio-chunk"
        );
    }
}

#[test]
fn chunk_crate_stays_minimal() {
    // No compression (the network codec lives in nascio-protocol) and no noise library.
    let deps = dependencies("nascio-chunk");
    for d in &deps {
        assert!(
            !d.contains("zstd") && !d.contains("noise"),
            "nascio-chunk depends on {d}"
        );
    }
}
