//! A type placed `above` another (spec §4.6): its bodies sit on bodies of the other type, and
//! adding it moves nothing else.

use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::deposits::{Catalog, DEFAULT_CATALOG};
use nascio_worldgen::generate::generate_full_with;
use nascio_worldgen::progress::Progress;

const CAP: &str = r#"
[[deposit]]
name = "test_cap"
label = "Test cap"
map_color = [1, 2, 3]
blocks = [{ block = "test_cap_ore", color = [1, 2, 3] }]
contexts = { arc = 1.0, orogen = 1.0, rift = 1.0, craton = 1.0, platform = 1.0, basin = 1.0 }
shape = "lens"
depth_m = [0, 20]
size_m = [20, 40]
thickness_m = [2, 4]
grade = 0.5
above = "porphyry_copper"
probability = 1.0
"#;

#[test]
fn bodies_above_sit_on_their_parents() {
    let cfg = WorldConfig::new(42, Scale::TwentyFifth);
    let base = generate_full_with(&cfg, &Catalog::default_catalog(), &Progress::new()).unwrap();
    let with = Catalog::parse(&format!("{DEFAULT_CATALOG}\n{CAP}")).unwrap();
    let kinds = with.deposits.len();
    let (pk, ck) = (
        with.deposits
            .iter()
            .position(|d| d.name == "porphyry_copper")
            .unwrap() as u16,
        (kinds - 1) as u16,
    );
    let g = generate_full_with(&cfg, &with, &Progress::new()).unwrap();

    // The other types do not move.
    let others: Vec<_> = g
        .deposits
        .bodies
        .iter()
        .filter(|b| b.kind != ck)
        .copied()
        .collect();
    assert_eq!(others, base.deposits.bodies);

    let parents: Vec<_> = others.iter().filter(|b| b.kind == pk).collect();
    let caps: Vec<_> = g.deposits.bodies.iter().filter(|b| b.kind == ck).collect();
    assert!(!parents.is_empty(), "no porphyry at this seed");
    assert!(!caps.is_empty() && caps.len() <= parents.len());
    for c in &caps {
        assert!(
            parents.iter().any(|p| p.x == c.x && p.z == c.z),
            "cap at ({}, {}) on no porphyry",
            c.x,
            c.z
        );
    }

    // Deterministic.
    let again = generate_full_with(&cfg, &with, &Progress::new()).unwrap();
    assert_eq!(again.deposits.bodies, g.deposits.bodies);
}
