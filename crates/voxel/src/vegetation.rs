//! Provisional vegetation (M6): one fixed spruce model whatever the biome, 3-block cacti in the
//! hot desert. The world is cut into square cells (`plant_cell_m`); each cell holds at most one
//! plant, at a position and with a chance drawn from a hash of the cell, so every chunk finds
//! the same plants, across the seam included. A plant may overhang the neighbouring chunks.

use nascio_world::rng::mix64;
use serde::{Deserialize, Serialize};

/// Plants of one biome (`[[voxel.vegetation]]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BiomePlants {
    /// Biome name, as in `nascio_world::biomes::NAMES`.
    pub biome: String,
    /// Share of the cells that hold a plant (0 to 1).
    pub density: f64,
    /// Plant models ([`PlantKind::name`]), drawn evenly; unknown names are ignored.
    pub plants: Vec<String>,
}

pub fn default_vegetation() -> Vec<BiomePlants> {
    [
        ("lake", 0.05, "spruce"),
        ("tundra", 0.01, "spruce"),
        ("boreal forest", 0.45, "spruce"),
        ("temperate forest", 0.40, "spruce"),
        ("mediterranean", 0.12, "spruce"),
        ("steppe", 0.02, "spruce"),
        ("hot desert", 0.03, "cactus"),
        ("savanna", 0.05, "spruce"),
        ("tropical rainforest", 0.60, "spruce"),
        ("mangrove", 0.30, "spruce"),
        ("marsh", 0.08, "spruce"),
    ]
    .into_iter()
    .map(|(biome, density, plant)| BiomePlants {
        biome: biome.into(),
        density,
        plants: vec![plant.into()],
    })
    .collect()
}

/// A plant model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlantKind {
    Spruce,
    Cactus,
}

impl PlantKind {
    pub const ALL: [PlantKind; 2] = [PlantKind::Spruce, PlantKind::Cactus];

    pub fn name(self) -> &'static str {
        match self {
            PlantKind::Spruce => "spruce",
            PlantKind::Cactus => "cactus",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// Whether the plant is a tree (above the tree line only, on soil).
    pub fn is_tree(self) -> bool {
        self == PlantKind::Spruce
    }

    /// Blocks of the model: `(dx, dy, dz, part)`, `dy = 0` on the block above the ground.
    pub fn parts(self) -> Vec<(i32, i32, i32, Part)> {
        let mut v = Vec::new();
        match self {
            PlantKind::Spruce => {
                for dy in 0..=6 {
                    v.push((0, dy, 0, Part::Log));
                }
                // Leaf layers: (dy, radius, corners kept).
                for (dy, r, corners) in [
                    (2i32, 2i32, false),
                    (3, 1, true),
                    (4, 2, false),
                    (5, 1, true),
                    (6, 1, false),
                    (7, 0, true),
                    (8, 0, true),
                ] {
                    for dz in -r..=r {
                        for dx in -r..=r {
                            let corner = dx.abs() == r && dz.abs() == r && r > 0;
                            if ((dx, dz) != (0, 0) || dy > 6) && (corners || !corner) {
                                v.push((dx, dy, dz, Part::Leaves));
                            }
                        }
                    }
                }
            }
            PlantKind::Cactus => {
                for dy in 0..3 {
                    v.push((0, dy, 0, Part::Cactus));
                }
            }
        }
        v
    }
}

/// A plant: model and trunk position (`x`, `z` in the caller's frame, unwrapped), `base` the
/// first block above the ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plant {
    pub x: i32,
    pub z: i32,
    pub base: i32,
    pub kind: PlantKind,
}

/// Horizontal reach of the widest model from its trunk, blocks.
pub const MAX_RADIUS: i32 = 2;

/// A block of a plant model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Part {
    Leaves,
    Log,
    Cactus,
}

/// The draw of one vegetation cell `(ci, cj)` (wrapped): offset of the plant in the cell,
/// a uniform number in `[0, 1)` for the density and one to pick the model.
pub fn cell_draw(salt: u64, ci: i64, cj: i64, cell: i32) -> (i32, i32, f64, f64) {
    let h = mix64(salt ^ mix64(((ci as u64) << 32) ^ (cj as u64 & 0xFFFF_FFFF)));
    let c = cell as u64;
    let ox = (h % c) as i32;
    let oz = ((h >> 16) % c) as i32;
    let u = ((h >> 32) & 0xFF_FFFF) as f64 / f64::from(1 << 24);
    let h2 = mix64(h);
    let pick = (h2 >> 11) as f64 / (1u64 << 53) as f64;
    (ox, oz, u, pick)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spruce_has_one_trunk_and_no_leaf_on_it() {
        let p = PlantKind::Spruce.parts();
        let logs = p.iter().filter(|q| q.3 == Part::Log).count();
        assert_eq!(logs, 7);
        for &(dx, dy, dz, part) in &p {
            assert!(dx.abs() <= MAX_RADIUS && dz.abs() <= MAX_RADIUS);
            if part == Part::Leaves && (dx, dz) == (0, 0) {
                assert!(dy > 6);
            }
        }
        let mut keys: Vec<_> = p.iter().map(|q| (q.0, q.1, q.2)).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), p.len());
    }

    #[test]
    fn draws_are_deterministic_and_inside_the_cell() {
        for k in 0..1000 {
            let (ox, oz, u, pick) = cell_draw(7, k, -k, 5);
            assert!((0..5).contains(&ox) && (0..5).contains(&oz));
            assert!((0.0..1.0).contains(&u) && (0.0..1.0).contains(&pick));
            assert_eq!(cell_draw(7, k, -k, 5), (ox, oz, u, pick));
        }
    }
}
