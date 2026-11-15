//! Hydrology of generated worlds: every flow path ends at the sea, basins are consistent along
//! flow paths, across the seams included (CLAUDE.md, invariant 1).

use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::erosion::flow::{D8, NO_DIR};
use nascio_worldgen::generate::generate;
use nascio_worldgen::world_io::World;

fn layer_u(world: &World, name: &str) -> Vec<u32> {
    let l = world
        .layer(name)
        .unwrap_or_else(|| panic!("no {name} layer"));
    (0..l.width * l.height)
        .map(|k| l.data.get_f64(k) as u32)
        .collect()
}

fn check_world(seed: u64, scale: Scale) {
    let (world, _) = generate(&WorldConfig::new(seed, scale)).expect("generation");
    let dir = layer_u(&world, "flow_direction");
    let basin = layer_u(&world, "basin_id");
    let res = world.layer("flow_direction").expect("dir").width;
    let r = res as i64;
    let rec = |k: usize| -> usize {
        let d = dir[k];
        if d == u32::from(NO_DIR) {
            return k;
        }
        let (di, dj) = D8[d as usize];
        let (i, j) = ((k % res) as i64, (k / res) as i64);
        ((j + dj).rem_euclid(r) * r + (i + di).rem_euclid(r)) as usize
    };
    let (mut cross_x, mut cross_z) = (0, 0);
    for k in 0..res * res {
        let is_sea = basin[k] == 0;
        assert_eq!(
            is_sea,
            dir[k] == u32::from(NO_DIR),
            "seed {seed}: cell {k} is sea xor has no receiver (orphan sink)"
        );
        if is_sea {
            continue;
        }
        let next = rec(k);
        if basin[next] != 0 {
            assert_eq!(
                basin[k], basin[next],
                "seed {seed}: basin changes along flow"
            );
        }
        let (i, j, ni, nj) = (k % res, k / res, next % res, next / res);
        cross_x += usize::from(i.abs_diff(ni) > 1);
        cross_z += usize::from(j.abs_diff(nj) > 1);
    }
    // Every path reaches the sea: walk from every land cell (no cycles, bounded length).
    for start in 0..res * res {
        let mut k = start;
        let mut steps = 0;
        while basin[k] != 0 {
            k = rec(k);
            steps += 1;
            assert!(steps <= res * res, "seed {seed}: cycle from cell {start}");
        }
    }
    println!("seed {seed}: {cross_x} flow links across x = 0, {cross_z} across z = 0");
    assert!(
        cross_x + cross_z > 0,
        "seed {seed}: no flow across a seam, the check above proves nothing"
    );
}

#[test]
fn flow_paths_reach_the_sea_twenty_fifth() {
    for seed in [42, 7, 1234, 2026] {
        check_world(seed, Scale::TwentyFifth);
    }
}

#[test]
fn flow_paths_reach_the_sea_fifth() {
    for seed in [42, 7] {
        check_world(seed, Scale::Fifth);
    }
}
