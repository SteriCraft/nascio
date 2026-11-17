//! Block registry of a world (spec §5): block names → `u16` ids, assigned at generation (the
//! blocks of the rock column first, in [`BUILTIN`] order, then the catalogue's ore blocks) and
//! stored with the world as `blocks.toml`. Code refers to blocks by name.

use serde::{Deserialize, Serialize};

use crate::deposits::Catalog;

/// How a block takes part in rendering and physics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Matter {
    /// Nothing: faces next to it are visible.
    Empty,
    /// Opaque solid.
    Solid,
    /// Translucent fluid: hides nothing, but its surface is drawn.
    Fluid,
    /// Thin opaque layer at the bottom of its cell (snow cover): hides nothing.
    Layer,
}

/// Definition of one block type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockDef {
    pub id: u16,
    pub name: String,
    /// Provisional sRGB colour.
    pub color: [u8; 3],
    pub matter: Matter,
    /// Density in kg/m³ (0 for air), for later physics.
    pub density: f32,
}

/// Blocks of the rock column (spec §4.6): `(name, colour, matter, density)`. `air` must stay
/// first (id 0: empty chunks).
pub const BUILTIN: [(&str, [u8; 3], Matter, f32); 31] = [
    ("air", [0, 0, 0], Matter::Empty, 0.0),
    ("water", [38, 92, 150], Matter::Fluid, 1_000.0),
    // Mantle magma under the Moho: infinite and deadly (spec §3).
    ("magma", [255, 96, 24], Matter::Solid, 3_000.0),
    ("grass", [88, 132, 58], Matter::Solid, 1_400.0),
    ("dirt", [118, 85, 58], Matter::Solid, 1_500.0),
    // Quartz sand: beaches, hot deserts, shallow sea floors, river beds.
    ("sand", [212, 196, 146], Matter::Solid, 1_600.0),
    ("gravel", [138, 130, 120], Matter::Solid, 1_800.0),
    ("regolith", [146, 118, 88], Matter::Solid, 1_900.0),
    ("laterite", [170, 74, 42], Matter::Solid, 2_000.0),
    ("sandstone", [198, 166, 116], Matter::Solid, 2_300.0),
    ("clay", [132, 122, 108], Matter::Solid, 2_200.0),
    ("limestone", [196, 192, 174], Matter::Solid, 2_600.0),
    ("gypsum", [228, 224, 214], Matter::Solid, 2_300.0),
    ("granite", [182, 152, 142], Matter::Solid, 2_700.0),
    ("gneiss", [142, 134, 130], Matter::Solid, 2_750.0),
    ("schist", [104, 114, 110], Matter::Solid, 2_800.0),
    ("basalt", [62, 62, 66], Matter::Solid, 2_900.0),
    ("gabbro", [50, 60, 54], Matter::Solid, 3_000.0),
    ("andesite", [112, 106, 102], Matter::Solid, 2_650.0),
    ("granulite", [122, 98, 94], Matter::Solid, 2_900.0),
    ("peridotite", [88, 112, 72], Matter::Solid, 3_300.0),
    // Surface soils by biome (M6, `[[voxel.soils]]`). Grass is one block everywhere: its
    // colour follows the climate (chunk and horizon tint).
    ("moss", [112, 122, 82], Matter::Solid, 1_200.0),
    ("podzol", [92, 70, 52], Matter::Solid, 1_400.0),
    ("red_soil", [150, 72, 46], Matter::Solid, 1_600.0),
    ("mud", [84, 74, 60], Matter::Solid, 1_700.0),
    // Permanent ice (glaciers, sea and lake ice) and snow (M6).
    ("ice", [168, 204, 236], Matter::Solid, 917.0),
    ("snow", [244, 247, 251], Matter::Solid, 400.0),
    // Thin snow cover on top of a block, in the air cell above it (drawn 1/8 m thick).
    ("snow_layer", [244, 247, 251], Matter::Layer, 200.0),
    // Provisional vegetation (M6): one spruce model, cacti in the hot desert. Leaves are opaque.
    ("spruce_log", [88, 64, 44], Matter::Solid, 600.0),
    ("spruce_leaves", [42, 96, 50], Matter::Solid, 200.0),
    ("cactus", [74, 124, 60], Matter::Solid, 900.0),
];

/// The registry, indexed by id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockRegistry {
    #[serde(rename = "block")]
    pub blocks: Vec<BlockDef>,
}

impl BlockRegistry {
    /// The blocks of the rock column only.
    pub fn builtin() -> Self {
        let blocks = BUILTIN
            .iter()
            .enumerate()
            .map(|(i, &(name, color, matter, density))| BlockDef {
                id: i as u16,
                name: name.to_string(),
                color,
                matter,
                density,
            })
            .collect();
        Self { blocks }
    }

    /// The column blocks, then every ore block of `catalog` (first definition of a name wins).
    pub fn with_catalog(catalog: &Catalog) -> anyhow::Result<Self> {
        let mut r = Self::builtin();
        for d in &catalog.deposits {
            for (name, color, density) in d.block_defs() {
                if let Some(id) = r.id(name) {
                    anyhow::ensure!(
                        usize::from(id) >= BUILTIN.len(),
                        "deposit {}: block {name} is a rock-column block",
                        d.name
                    );
                    continue;
                }
                let id = u16::try_from(r.blocks.len())?;
                r.blocks.push(BlockDef {
                    id,
                    name: name.to_string(),
                    color,
                    matter: Matter::Solid,
                    density,
                });
            }
        }
        Ok(r)
    }

    /// Id of a block name.
    pub fn id(&self, name: &str) -> Option<u16> {
        self.blocks
            .iter()
            .position(|b| b.name == name)
            .map(|i| i as u16)
    }

    pub fn to_toml(&self) -> anyhow::Result<String> {
        Ok(toml::to_string(self)?)
    }

    pub fn from_toml(s: &str) -> anyhow::Result<Self> {
        let r: Self = toml::from_str(s)?;
        r.validate()?;
        Ok(r)
    }

    /// Ids are `0..n` in order, names unique, `air` is id 0 and empty.
    pub fn validate(&self) -> anyhow::Result<()> {
        for (i, b) in self.blocks.iter().enumerate() {
            anyhow::ensure!(
                usize::from(b.id) == i,
                "block {} has id {} at {i}",
                b.name,
                b.id
            );
            anyhow::ensure!(
                self.blocks[..i].iter().all(|o| o.name != b.name),
                "duplicate block {}",
                b.name
            );
        }
        anyhow::ensure!(
            self.blocks
                .first()
                .is_some_and(|b| b.name == "air" && b.matter == Matter::Empty),
            "block 0 must be air"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_blocks_follow_the_column() {
        let cat = Catalog::default_catalog();
        let r = BlockRegistry::with_catalog(&cat).unwrap();
        r.validate().unwrap();
        assert_eq!(r.id("air"), Some(0));
        assert_eq!(r.id("granite"), BlockRegistry::builtin().id("granite"));
        let cu = r.id("chalcopyrite").unwrap();
        assert!(usize::from(cu) >= BUILTIN.len());
        assert_eq!(BlockRegistry::from_toml(&r.to_toml().unwrap()).unwrap(), r);
        // Limestone, clay and quartz sand come with the column.
        for name in ["limestone", "clay", "sand"] {
            assert!(BlockRegistry::builtin().id(name).is_some());
        }
    }
}
