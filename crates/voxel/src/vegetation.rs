//! Vegetation: plant species described in TOML (`[[voxel.species]]`), placed by biome
//! (`[[voxel.vegetation]]`). The world is cut into square cells (`plant_cell_m`); each cell
//! holds at most one plant, at a position, with a chance, a size and an orientation drawn from a
//! hash of the cell, so every chunk finds the same plants, across the seam included. A plant
//! may overhang the neighbouring chunks.
//!
//! A species names a shape (a model builder) and its parameters; the models are built once per
//! species and per height ([`SpeciesModels`]).

use nascio_world::rng::mix64;
use serde::{Deserialize, Serialize};

use crate::block::{BlockId, Blocks};

/// Plants of one biome (`[[voxel.vegetation]]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BiomePlants {
    /// Biome name, as in `nascio_world::biomes::NAMES`.
    pub biome: String,
    /// Share of the cells that hold a plant (0 to 1).
    pub density: f64,
    /// Species ([`Species::name`]), drawn evenly; unknown names are ignored.
    pub plants: Vec<String>,
}

pub fn default_vegetation() -> Vec<BiomePlants> {
    [
        ("lake", 0.04, "spruce"),
        ("tundra", 0.01, "spruce"),
        ("boreal forest", 0.33, "spruce"),
        ("temperate forest", 0.30, "spruce"),
        ("mediterranean", 0.09, "spruce"),
        ("steppe", 0.02, "spruce"),
        ("hot desert", 0.03, "cactus"),
        ("savanna", 0.04, "spruce"),
        ("tropical rainforest", 0.45, "spruce"),
        ("mangrove", 0.22, "spruce"),
        ("marsh", 0.06, "spruce"),
    ]
    .into_iter()
    .map(|(biome, density, plant)| BiomePlants {
        biome: biome.into(),
        density,
        plants: vec![plant.into()],
    })
    .collect()
}

/// Model builder of a species.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    /// Narrow cone of alternating branch layers over a bare trunk (spruce, fir).
    Conic,
    /// A bare column (cactus).
    Column,
}

/// Ground a species needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ground {
    /// Soil (not sand nor gravel), below the tree line: trees.
    Soil,
    /// Sand: desert plants.
    Sand,
}

/// A plant species (`[[voxel.species]]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Species {
    pub name: String,
    pub shape: Shape,
    pub ground: Ground,
    /// Height of a plant of size factor 1, blocks above the ground.
    pub height_m: f64,
    /// The size factor is drawn in `[1 − jitter, 1 + jitter]`; it scales height and crown.
    pub size_jitter: f64,
    /// Bare trunk under the crown, share of the height.
    pub trunk_share: f64,
    /// Crown radius at its widest, for size factor 1, m.
    pub crown_radius_m: f64,
    /// Conic crowns: the cone's apex sits this share of the height above the tip, which
    /// shortens the thin spike at the top (0: pointed cone).
    #[serde(default)]
    pub apex_share: f64,
    /// Side of the square trunk, blocks.
    pub trunk_width: i32,
    /// Trunk block, and crown block (none: no crown), named as in the registry.
    pub log: String,
    #[serde(default)]
    pub leaves: Option<String>,
}

pub fn default_species() -> Vec<Species> {
    vec![
        Species {
            name: "spruce".into(),
            shape: Shape::Conic,
            ground: Ground::Soil,
            height_m: 24.0,
            size_jitter: 0.3,
            trunk_share: 0.12,
            crown_radius_m: 3.0,
            apex_share: 0.15,
            trunk_width: 1,
            log: "spruce_log".into(),
            leaves: Some("spruce_leaves".into()),
        },
        Species {
            name: "cactus".into(),
            shape: Shape::Column,
            ground: Ground::Sand,
            height_m: 3.0,
            size_jitter: 0.3,
            trunk_share: 1.0,
            crown_radius_m: 0.0,
            apex_share: 0.0,
            trunk_width: 1,
            log: "cactus".into(),
            leaves: None,
        },
    ]
}

/// Role of a model block; where two plants overlap, the greater wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Part {
    Leaves,
    Log,
}

/// A model block: offset from the trunk's first block (`dy = 0` on the block above the
/// ground; the trunk covers `0..trunk_width` in `dx` and `dz`) and role.
pub type ModelBlock = (i32, i32, i32, Part);

/// Largest accepted trunk width, blocks.
pub const MAX_TRUNK_WIDTH: i32 = 3;

/// The models of one species, built once, and its resolved blocks.
#[derive(Clone, Debug)]
pub struct SpeciesModels {
    pub name: String,
    pub ground: Ground,
    pub log: BlockId,
    pub leaves: Option<BlockId>,
    pub trunk_width: i32,
    target: f64,
    jitter: f64,
    /// Height of `models[0]`.
    min_height: i32,
    models: Vec<Vec<ModelBlock>>,
}

impl SpeciesModels {
    /// Resolves and builds a species; `None` if a block is missing from the registry or a
    /// parameter is out of range.
    pub fn new(s: &Species, blocks: &Blocks) -> Option<Self> {
        let log = blocks.id(&s.log)?;
        let leaves = match &s.leaves {
            Some(n) => Some(blocks.id(n)?),
            None => None,
        };
        let ok = s.height_m >= 1.0
            && s.height_m <= 200.0
            && (0.0..=0.9).contains(&s.size_jitter)
            && (0.0..=1.0).contains(&s.trunk_share)
            && (0.0..=30.0).contains(&s.crown_radius_m)
            && (0.0..=1.0).contains(&s.apex_share)
            && (1..=MAX_TRUNK_WIDTH).contains(&s.trunk_width);
        if !ok {
            return None;
        }
        let h = |f: f64| ((s.height_m * f).round() as i32).max(1);
        let (lo, hi) = (h(1.0 - s.size_jitter), h(1.0 + s.size_jitter));
        let seed = s
            .name
            .bytes()
            .fold(0x5eed_u64, |a, b| mix64(a ^ u64::from(b)));
        let models = (lo..=hi)
            .map(|height| build(s, height, leaves.is_some(), seed))
            .collect();
        Some(Self {
            name: s.name.clone(),
            ground: s.ground,
            log,
            leaves,
            trunk_width: s.trunk_width,
            target: s.height_m,
            jitter: s.size_jitter,
            min_height: lo,
            models,
        })
    }

    /// Whether the species is a tree (on soil, below the tree line).
    pub fn is_tree(&self) -> bool {
        self.ground == Ground::Soil
    }

    /// Height of a plant whose size draw is `size` in `[0, 1)`.
    pub fn height(&self, size: f64) -> i32 {
        let f = 1.0 - self.jitter + 2.0 * self.jitter * size;
        let max = self.min_height + self.models.len() as i32 - 1;
        ((self.target * f).round() as i32).clamp(self.min_height, max)
    }

    /// Model of a plant of this height (unrotated).
    pub fn model(&self, height: i32) -> &[ModelBlock] {
        let k = (height - self.min_height).clamp(0, self.models.len() as i32 - 1);
        &self.models[k as usize]
    }

    /// Block of a part.
    pub fn block(&self, part: Part) -> BlockId {
        match part {
            Part::Log => self.log,
            Part::Leaves => self.leaves.unwrap_or(self.log),
        }
    }

    /// Block seen from above (crown, else trunk).
    pub fn crown_block(&self) -> BlockId {
        self.leaves.unwrap_or(self.log)
    }

    /// Columns covered seen from above by the model of the target height.
    pub fn footprint(&self) -> usize {
        let mut cols: Vec<(i32, i32)> = self
            .model(self.target.round() as i32)
            .iter()
            .map(|q| (q.0, q.2))
            .collect();
        cols.sort_unstable();
        cols.dedup();
        cols.len()
    }

    /// Largest horizontal distance of a block from the trunk's first column, any height and
    /// orientation, blocks.
    pub fn reach(&self) -> i32 {
        let w = self.trunk_width;
        self.models
            .iter()
            .flatten()
            .flat_map(|&(dx, _, dz, _)| (0..4).map(move |r| rotate(dx, dz, r, w)))
            .map(|(dx, dz)| dx.abs().max(dz.abs()))
            .max()
            .unwrap_or(0)
    }
}

/// `(dx, dz)` turned by `rot` quarter turns about the trunk's axis (a trunk `w` wide covers
/// `0..w`, and keeps covering it).
pub fn rotate(dx: i32, dz: i32, rot: u8, w: i32) -> (i32, i32) {
    // Doubled coordinates centred on the trunk's axis.
    let (mut x, mut z) = (2 * dx - (w - 1), 2 * dz - (w - 1));
    for _ in 0..rot % 4 {
        (x, z) = (-z, x);
    }
    ((x + w - 1) / 2, (z + w - 1) / 2)
}

/// Uniform number in `[0, 1)` from a hash.
fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// The model of `s` at `height`.
fn build(s: &Species, height: i32, has_crown: bool, seed: u64) -> Vec<ModelBlock> {
    let w = s.trunk_width;
    let mut v = Vec::new();
    match s.shape {
        Shape::Column => {
            for dy in 0..height {
                for dz in 0..w {
                    for dx in 0..w {
                        v.push((dx, dy, dz, Part::Log));
                    }
                }
            }
        }
        Shape::Conic => {
            let scale = f64::from(height) / s.height_m;
            let radius = s.crown_radius_m * scale;
            // Trunk up to two blocks under the tip; the crown from `base` to the tip.
            let trunk_top = (height - 3).max(0);
            let base = ((s.trunk_share * f64::from(height)).round() as i32).clamp(1, height - 1);
            for dy in 0..=trunk_top {
                for dz in 0..w {
                    for dx in 0..w {
                        v.push((dx, dy, dz, Part::Log));
                    }
                }
            }
            if !has_crown {
                return v;
            }
            let c = f64::from(w - 1) / 2.0;
            let span = f64::from((height - 1 - base).max(1));
            let apex = s.apex_share * f64::from(height);
            let reach = radius.ceil() as i32 + 1;
            for dy in base..height {
                let from_top = height - 1 - dy;
                // Cone, with every other layer slightly pulled in (branch whorls); below the
                // tip every layer keeps at least a ring around the trunk, which stays hidden.
                let t = (f64::from(from_top) + apex) / (span + apex);
                let r = radius * t - if from_top % 2 == 1 { 0.6 } else { 0.0 };
                let r = if from_top > 1 { r.max(1.0) } else { r.max(0.0) };
                let r2 = (r + 0.45).powi(2);
                for dz in -reach..w + reach {
                    for dx in -reach..w + reach {
                        let trunk = (0..w).contains(&dx) && (0..w).contains(&dz);
                        if trunk && dy <= trunk_top {
                            continue;
                        }
                        let d2 = (f64::from(dx) - c).powi(2) + (f64::from(dz) - c).powi(2);
                        if d2 > r2 {
                            continue;
                        }
                        // Ragged edge: some outer blocks missing, fixed per model.
                        let edge = d2.sqrt() > r - 0.6;
                        let k = mix64(
                            seed ^ mix64(
                                (u64::from(height as u32) << 40)
                                    ^ (u64::from(dy as u32) << 20)
                                    ^ (u64::from((dx + 64) as u32) << 8)
                                    ^ u64::from((dz + 64) as u32),
                            ),
                        );
                        if edge && !trunk && d2 > 2.0 && unit(k) < 0.15 {
                            continue;
                        }
                        v.push((dx, dy, dz, Part::Leaves));
                    }
                }
            }
        }
    }
    v
}

/// A plant: species (index in the terrain's table), trunk position (`x`, `z` in the caller's
/// frame, unwrapped: the trunk covers `x..x + trunk_width`), `base` the first block above the
/// ground, height and orientation (quarter turns).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plant {
    pub x: i32,
    pub z: i32,
    pub base: i32,
    pub species: u16,
    pub height: i32,
    pub rot: u8,
}

/// The draw of one vegetation cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellDraw {
    /// Offset of the trunk in the cell.
    pub ox: i32,
    pub oz: i32,
    /// Uniform numbers in `[0, 1)`: density, species pick, size.
    pub u: f64,
    pub pick: f64,
    pub size: f64,
    /// Orientation, quarter turns.
    pub rot: u8,
}

/// The draw of vegetation cell `(ci, cj)` (wrapped), trunks at most `trunk` wide.
///
/// A trunk lies in `[0, cell - 2]` of its cell, so two trunks of neighbouring cells are at
/// least one block apart, diagonals and the seam included. `cell` is at least `trunk + 2`.
pub fn cell_draw(salt: u64, ci: i64, cj: i64, cell: i32, trunk: i32) -> CellDraw {
    let h = mix64(salt ^ mix64(((ci as u64) << 32) ^ (cj as u64 & 0xFFFF_FFFF)));
    let c = (cell - trunk) as u64;
    let h2 = mix64(h);
    let h3 = mix64(h2);
    CellDraw {
        ox: (h % c) as i32,
        oz: ((h >> 16) % c) as i32,
        u: ((h >> 32) & 0xFF_FFFF) as f64 / f64::from(1 << 24),
        pick: unit(h2),
        size: unit(h3),
        rot: (h3 & 3) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spruce() -> SpeciesModels {
        SpeciesModels::new(&default_species()[0], &Blocks::builtin()).expect("spruce")
    }

    #[test]
    fn spruce_models_have_one_trunk_and_no_leaf_on_it() {
        let s = spruce();
        assert!(s.is_tree());
        for h in 17..=31 {
            let m = s.model(h);
            let logs = m.iter().filter(|q| q.3 == Part::Log).count() as i32;
            assert_eq!(logs, h - 2, "height {h}");
            assert_eq!(m.iter().map(|q| q.1).max(), Some(h - 1));
            let mut keys: Vec<_> = m.iter().map(|q| (q.0, q.1, q.2)).collect();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), m.len(), "height {h}: two blocks in one place");
        }
        assert!((3..=6).contains(&s.reach()), "reach {}", s.reach());
        assert!(s.footprint() > 20);
    }

    #[test]
    fn heights_stay_within_the_jitter() {
        for sp in default_species() {
            let m = SpeciesModels::new(&sp, &Blocks::builtin()).expect("species");
            let (lo, hi) = (
                sp.height_m * (1.0 - sp.size_jitter) - 0.5,
                sp.height_m * (1.0 + sp.size_jitter) + 0.5,
            );
            let mut seen = std::collections::BTreeSet::new();
            for k in 0..2000 {
                let d = cell_draw(3, k, 7 * k, 5, 1);
                let h = m.height(d.size);
                assert!((lo..=hi).contains(&f64::from(h)), "{}: {h}", sp.name);
                assert_eq!(m.model(h).iter().map(|q| q.1).max(), Some(h - 1));
                seen.insert(h);
            }
            assert!(seen.len() >= 2, "{}: one height only", sp.name);
        }
    }

    #[test]
    fn rotation_keeps_the_trunk_in_place() {
        for w in 1..=MAX_TRUNK_WIDTH {
            for dz in 0..w {
                for dx in 0..w {
                    for r in 0..4 {
                        let (x, z) = rotate(dx, dz, r, w);
                        assert!((0..w).contains(&x) && (0..w).contains(&z));
                    }
                }
            }
            assert_eq!(rotate(5, -2, 4, w), (5, -2));
            let (x, z) = rotate(3, 0, 1, w);
            assert_eq!(rotate(x, z, 3, w), (3, 0));
        }
    }

    #[test]
    fn missing_blocks_drop_the_species() {
        let mut s = default_species()[0].clone();
        s.log = "no such block".into();
        assert!(SpeciesModels::new(&s, &Blocks::builtin()).is_none());
    }

    #[test]
    fn draws_are_deterministic_and_inside_the_cell() {
        for k in 0..1000 {
            let d = cell_draw(7, k, -k, 5, 1);
            assert!((0..4).contains(&d.ox) && (0..4).contains(&d.oz));
            assert!((0.0..1.0).contains(&d.u) && (0.0..1.0).contains(&d.pick));
            assert!((0.0..1.0).contains(&d.size) && d.rot < 4);
            assert_eq!(cell_draw(7, k, -k, 5, 1), d);
        }
    }
}
