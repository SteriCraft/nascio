//! Horizon tiles: the same vertex across the seam, consistent surfaces, determinism.

use std::sync::OnceLock;

use nascio_chunk::horizon::{Surface, TILE, TileKey, quantize_height, spacing};
use nascio_core::consts::WORLD_SIZE_F64;
use nascio_voxel::horizon::horizon_tile;
use nascio_voxel::{Terrain, VoxelConfig};
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

fn terrain() -> &'static Terrain {
    static T: OnceLock<Terrain> = OnceLock::new();
    T.get_or_init(|| {
        let g = generate_full(&WorldConfig::new(42, Scale::Fifth)).expect("generation");
        Terrain::from_world(&g.world, VoxelConfig::default()).expect("terrain")
    })
}

fn vertex(level: u8, vi: i64, vj: i64) -> nascio_chunk::horizon::HorizonVertex {
    let (key, k) = TileKey::containing(level, vi, vj);
    horizon_tile(terrain(), key).vertices[k]
}

/// Levels 0 and 1 wrap on their lattice: a vertex one world further is the same vertex,
/// though it lies in another tile (the tiles do not divide the period).
#[test]
fn seam_vertices_match() {
    for level in [0u8, 1] {
        let period = (WORLD_SIZE_F64 / spacing(level)) as i64;
        for (vi, vj) in [(-1, 5), (period - 1, 17), (3, -2), (-40, -40)] {
            assert_eq!(
                vertex(level, vi, vj),
                vertex(level, vi + period, vj + period),
                "level {level} vertex ({vi}, {vj})"
            );
        }
    }
}

#[test]
fn surfaces_are_consistent_and_deterministic() {
    let t = terrain();
    for key in [
        TileKey {
            level: 0,
            i: 40,
            j: 90,
        },
        TileKey {
            level: 3,
            i: -1,
            j: 7,
        },
        TileKey {
            level: 4,
            i: 3,
            j: 3,
        },
    ] {
        let tile = horizon_tile(t, key);
        assert_eq!(tile, horizon_tile(t, key), "{key:?} not deterministic");
        assert_eq!(tile.vertices.len(), TILE * TILE);
        let s = spacing(key.level);
        let (fi, fj) = key.first_vertex();
        for (k, v) in tile.vertices.iter().enumerate() {
            let x = (fi + (k % TILE) as i64) as f64 * s;
            let z = (fj + (k / TILE) as i64) as f64 * s;
            let (wx, wz) = (
                nascio_core::torus::wrap_f64(x),
                nascio_core::torus::wrap_f64(z),
            );
            let base = quantize_height(t.height_lod(wx, wz, 0.5 * s) + 0.5);
            assert_eq!(v.water, t.water_level(wx, wz).round() as f32);
            assert_eq!(v.height, base);
            match v.surface {
                Surface::Ground => assert!(v.height >= v.water),
                Surface::Water | Surface::Ice => {
                    assert!(v.height < v.water);
                    if v.surface == Surface::Ice {
                        assert_eq!(v.color, [0; 3]);
                    }
                }
            }
        }
    }
}
