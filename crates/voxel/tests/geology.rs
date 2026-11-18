//! Rock column and ore bodies of a generated world (spec §4.6).

use std::sync::OnceLock;

use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS};
use nascio_core::coords::{ChunkPos, LocalPos};
use nascio_voxel::geology::Class;
use nascio_voxel::{ColumnSet, Terrain, VoxelConfig, generate_chunk};
use nascio_worldgen::blocks::BUILTIN;
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::deposits::Shape;
use nascio_worldgen::generate::generate_full;

fn terrain() -> &'static Terrain {
    static T: OnceLock<Terrain> = OnceLock::new();
    T.get_or_init(|| {
        let g = generate_full(&WorldConfig::new(42, Scale::Fifth)).expect("generation");
        Terrain::from_world(&g.world, VoxelConfig::default()).expect("terrain")
    })
}

fn rank(c: Class) -> u8 {
    match c {
        Class::Soil | Class::Loose => 0,
        Class::Weathered => 1,
        Class::Sediment => 2,
        Class::Basement => 3,
    }
}

/// From the top down: soil, weathering, sediments, basement, then magma below the Moho; the
/// column supplies limestone, chalk, clay and quartz sand somewhere.
#[test]
fn column_layers_in_order() {
    let t = terrain();
    let ids = t.blocks().ids;
    let mut seen = vec![0usize; t.blocks().len()];
    for cx in (0..CHUNKS_PER_AXIS).step_by(173) {
        for cz in (0..CHUNKS_PER_AXIS).step_by(211) {
            let col = ColumnSet::new(t, cx, cz);
            let (top, moho) = (col.top(9, 21), col.moho(9, 21));
            let (wx, wz) = (cx * CHUNK_SIZE + 9, cz * CHUNK_SIZE + 21);
            let mut last = 0;
            let mut y = top;
            while y >= moho {
                let (b, class) =
                    t.rock_block(col.geo(9, 21), wx, wz, top, col.slope(9, 21), moho, top - y);
                assert!(
                    rank(class) >= last,
                    "({cx},{cz}) y {y}: {class:?} after rank {last}"
                );
                last = rank(class);
                seen[usize::from(b)] += 1;
                // Coarse steps below the first 600 m keep the test fast.
                y -= if top - y < 600 { 1 } else { 7 };
            }
            assert_eq!(col.block(t, 9, moho - 1, 21), ids.magma);
        }
    }
    for (name, id) in [
        ("limestone", ids.limestone),
        ("clay", ids.clay),
        ("sand", ids.sand),
        ("granite", ids.granite),
        ("basalt", ids.basalt),
        ("gabbro", ids.gabbro),
        ("granulite", ids.granulite),
        ("chalk", ids.chalk.expect("generator 20 has chalk")),
    ] {
        assert!(seen[usize::from(id)] > 0, "no {name}");
    }
}

/// Ore blocks appear inside the bodies and only replace rock (or sand and gravel).
#[test]
fn ore_bodies_replace_rock_only() {
    let t = terrain();
    let g = t.geology();
    let builtin = BUILTIN.len() as u16;
    let mut found = [false; 4];
    for shape in [Shape::Stock, Shape::Layer, Shape::Vein, Shape::Lens] {
        let mut ore_blocks = 0;
        for b in g.bodies().iter().filter(|b| b.shape == shape).take(6) {
            let (cx, cz) = (
                (b.x / f64::from(CHUNK_SIZE)) as i32,
                (b.z / f64::from(CHUNK_SIZE)) as i32,
            );
            let col = ColumnSet::new(t, cx, cz);
            let cy = (b.y / f64::from(CHUNK_SIZE)).floor() as i32;
            for dy in -1..=1 {
                let c = generate_chunk(t, ChunkPos::new(cx, cy + dy, cz));
                for z in 0..32u8 {
                    for x in 0..32u8 {
                        for y in 0..32u8 {
                            let id = c.get(LocalPos { x, y, z });
                            if id < builtin {
                                continue;
                            }
                            ore_blocks += 1;
                            let (lx, lz) = (i32::from(x), i32::from(z));
                            let wy = (cy + dy) * CHUNK_SIZE + i32::from(y);
                            let top = col.top(lx, lz);
                            assert!(wy <= top && wy >= col.moho(lx, lz));
                            let (_, class) = t.rock_block(
                                col.geo(lx, lz),
                                cx * CHUNK_SIZE + lx,
                                cz * CHUNK_SIZE + lz,
                                top,
                                col.slope(lx, lz),
                                col.moho(lx, lz),
                                top - wy,
                            );
                            assert_ne!(class, Class::Soil, "{shape:?} ore in the soil");
                        }
                    }
                }
            }
        }
        let k = [Shape::Stock, Shape::Layer, Shape::Vein, Shape::Lens]
            .iter()
            .position(|&s| s == shape)
            .unwrap();
        found[k] = ore_blocks > 0;
    }
    assert!(found.iter().all(|&f| f), "ore found per shape: {found:?}");
}

/// Bog iron (`replaces_soil`) lies right under the surface block, in the soil, and never takes
/// the surface block itself.
#[test]
fn bog_iron_replaces_the_soil_under_the_surface_block() {
    let t = terrain();
    let blocks = t.blocks();
    let limonite = (0..blocks.len() as u16)
        .find(|&id| blocks.def(id).name == "limonite")
        .expect("limonite in the registry");
    let (mut in_soil, mut at_surface) = (0, 0);
    for b in t
        .geology()
        .bodies()
        .iter()
        .filter(|b| b.shape == Shape::Blanket)
        .take(60)
    {
        let (cx, cz) = (
            (b.x / f64::from(CHUNK_SIZE)) as i32,
            (b.z / f64::from(CHUNK_SIZE)) as i32,
        );
        let col = ColumnSet::new(t, cx, cz);
        let top = col.top(16, 16);
        let cy = top.div_euclid(CHUNK_SIZE);
        for dy in -1..=0 {
            let c = generate_chunk(t, ChunkPos::new(cx, cy + dy, cz));
            for z in 0..32u8 {
                for x in 0..32u8 {
                    let (lx, lz) = (i32::from(x), i32::from(z));
                    let top = col.top(lx, lz);
                    for y in 0..32u8 {
                        let wy = (cy + dy) * CHUNK_SIZE + i32::from(y);
                        if c.get(LocalPos { x, y, z }) != limonite {
                            continue;
                        }
                        if wy == top {
                            at_surface += 1;
                        }
                        let (_, class) = t.rock_block(
                            col.geo(lx, lz),
                            cx * CHUNK_SIZE + lx,
                            cz * CHUNK_SIZE + lz,
                            top,
                            col.slope(lx, lz),
                            col.moho(lx, lz),
                            top - wy,
                        );
                        in_soil += usize::from(class == Class::Soil);
                    }
                }
            }
        }
    }
    assert_eq!(at_surface, 0, "limonite as the surface block");
    assert!(in_soil > 0, "no limonite in the soil");
}

/// The border of a chunk equals its neighbour's blocks across both seams, rock and ore.
#[test]
fn geology_continuous_across_the_seams() {
    let t = terrain();
    let last = CHUNKS_PER_AXIS - 1;
    for (a, b, axis_x) in
        [((last, 1_200), (0, 1_200)), ((900, last), (900, 0))].map(|(a, b)| (a, b, a.0 == last))
    {
        let ca = ColumnSet::new(t, a.0, a.1);
        let cb = ColumnSet::new(t, b.0, b.1);
        for cy in ca.chunk_range(4) {
            let pa = ca.padded_chunk(t, cy);
            let pb = cb.padded_chunk(t, cy);
            for u in 0..CHUNK_SIZE {
                for y in 0..CHUNK_SIZE {
                    let (in_b, border_a) = if axis_x {
                        (pb.get(0, y, u), pa.get(CHUNK_SIZE, y, u))
                    } else {
                        (pb.get(u, y, 0), pa.get(u, y, CHUNK_SIZE))
                    };
                    assert_eq!(in_b, border_a, "{a:?}->{b:?} cy {cy} u {u} y {y}");
                }
            }
        }
    }
}
