//! Block registry of a world (spec §5). Block ids are `u16`, assigned at generation and stored
//! with the world (`blocks.toml`, see [`nascio_world::blocks`]); code refers to blocks by
//! name. Colours are provisional until textures exist.

// Rendering only (colours, previews): platform maths is fine here.
#![allow(clippy::disallowed_methods)]

pub use nascio_world::blocks::{BlockDef, BlockRegistry, Matter};

/// A block id.
pub type BlockId = u16;

/// Air is always id 0 (validated by the registry): empty chunks are all zeros.
pub const AIR: BlockId = 0;

/// Ids of the rock-column blocks the generator places (resolved by name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnIds {
    pub water: BlockId,
    pub magma: BlockId,
    pub grass: BlockId,
    pub dirt: BlockId,
    pub sand: BlockId,
    pub gravel: BlockId,
    pub regolith: BlockId,
    pub laterite: BlockId,
    pub sandstone: BlockId,
    pub clay: BlockId,
    pub limestone: BlockId,
    pub gypsum: BlockId,
    pub granite: BlockId,
    pub gneiss: BlockId,
    pub schist: BlockId,
    pub basalt: BlockId,
    pub gabbro: BlockId,
    pub andesite: BlockId,
    pub granulite: BlockId,
    pub peridotite: BlockId,
    /// Surface blocks of M6, `None` in older registries.
    pub ice: Option<BlockId>,
    pub snow: Option<BlockId>,
    pub snow_layer: Option<BlockId>,
    /// Vegetation blocks of M6, `None` in older registries.
    pub spruce_log: Option<BlockId>,
    pub spruce_leaves: Option<BlockId>,
    pub cactus: Option<BlockId>,
}

/// The registry with what meshing needs per id (matter, linear colour).
#[derive(Clone, Debug)]
pub struct Blocks {
    registry: BlockRegistry,
    matter: Vec<Matter>,
    linear: Vec<[f32; 3]>,
    pub ids: ColumnIds,
}

impl Blocks {
    pub fn new(registry: BlockRegistry) -> anyhow::Result<Self> {
        registry.validate()?;
        let id = |name: &str| {
            registry
                .id(name)
                .ok_or_else(|| anyhow::anyhow!("block registry has no {name}"))
        };
        let ids = ColumnIds {
            water: id("water")?,
            magma: id("magma")?,
            grass: id("grass")?,
            dirt: id("dirt")?,
            sand: id("sand")?,
            gravel: id("gravel")?,
            regolith: id("regolith")?,
            laterite: id("laterite")?,
            sandstone: id("sandstone")?,
            clay: id("clay")?,
            limestone: id("limestone")?,
            gypsum: id("gypsum")?,
            granite: id("granite")?,
            gneiss: id("gneiss")?,
            schist: id("schist")?,
            basalt: id("basalt")?,
            gabbro: id("gabbro")?,
            andesite: id("andesite")?,
            granulite: id("granulite")?,
            peridotite: id("peridotite")?,
            ice: registry.id("ice"),
            snow: registry.id("snow"),
            snow_layer: registry.id("snow_layer"),
            spruce_log: registry.id("spruce_log"),
            spruce_leaves: registry.id("spruce_leaves"),
            cactus: registry.id("cactus"),
        };
        let matter = registry.blocks.iter().map(|b| b.matter).collect();
        let linear = registry
            .blocks
            .iter()
            .map(|b| b.color.map(|c| srgb_to_linear(f32::from(c) / 255.0)))
            .collect();
        Ok(Self {
            registry,
            matter,
            linear,
            ids,
        })
    }

    /// The rock-column blocks only.
    pub fn builtin() -> Self {
        Self::new(BlockRegistry::builtin()).expect("valid built-in registry")
    }

    pub fn registry(&self) -> &BlockRegistry {
        &self.registry
    }

    pub fn len(&self) -> usize {
        self.matter.len()
    }

    pub fn is_empty(&self) -> bool {
        self.matter.is_empty()
    }

    /// Definition of an id (unknown ids panic: the registry is closed).
    pub fn def(&self, id: BlockId) -> &BlockDef {
        &self.registry.blocks[usize::from(id)]
    }

    /// Id of a block name.
    pub fn id(&self, name: &str) -> Option<BlockId> {
        self.registry.id(name)
    }

    #[inline]
    pub fn matter(&self, id: BlockId) -> Matter {
        self.matter[usize::from(id)]
    }

    #[inline]
    pub fn is_solid(&self, id: BlockId) -> bool {
        self.matter(id) == Matter::Solid
    }

    /// Whether a block is foliage (tagged for the shader: its sides stay green under the
    /// seasonal snow).
    #[inline]
    pub fn is_foliage(&self, id: BlockId) -> bool {
        self.ids.spruce_leaves == Some(id)
    }

    /// Linear-light RGB of a block colour (for vertex colours).
    #[inline]
    pub fn linear(&self, id: BlockId) -> [f32; 3] {
        self.linear[usize::from(id)]
    }
}

/// sRGB transfer function, inverse.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_blocks_resolve_by_name() {
        let b = Blocks::builtin();
        assert_eq!(b.id("air"), Some(AIR));
        assert_eq!(b.def(b.ids.granite).name, "granite");
        assert!(b.is_solid(b.ids.granite) && !b.is_solid(b.ids.water) && !b.is_solid(AIR));
        assert_eq!(b.matter(b.ids.water), Matter::Fluid);
        let cat = nascio_world::deposits::Catalog::default_catalog();
        let full = Blocks::new(BlockRegistry::with_catalog(&cat).unwrap()).unwrap();
        assert_eq!(full.ids, b.ids);
        assert!(full.len() > b.len());
    }
}
