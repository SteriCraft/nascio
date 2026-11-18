//! Block registry of a world (spec §5) as the generator uses it: the block table of
//! `nascio-chunk` (what meshing and the client read) plus the ids of the blocks the generator
//! places. Block ids are `u16`, assigned at generation and stored with the world
//! (`blocks.toml`); code refers to blocks by name.

pub use nascio_chunk::block::{
    AIR, BUILTIN, BlockDef, BlockId, BlockRegistry, Matter, srgb_to_linear,
};

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

/// The block table with the ids of the blocks the generator places. Dereferences to the
/// table of `nascio-chunk` (meshing takes `&nascio_chunk::Blocks`).
#[derive(Clone, Debug)]
pub struct Blocks {
    table: nascio_chunk::Blocks,
    pub ids: ColumnIds,
}

impl std::ops::Deref for Blocks {
    type Target = nascio_chunk::Blocks;

    fn deref(&self) -> &nascio_chunk::Blocks {
        &self.table
    }
}

impl Blocks {
    pub fn new(registry: BlockRegistry) -> anyhow::Result<Self> {
        let table = nascio_chunk::Blocks::new(registry)?;
        let id = |name: &str| {
            table
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
            ice: table.id("ice"),
            snow: table.id("snow"),
            snow_layer: table.id("snow_layer"),
            spruce_log: table.id("spruce_log"),
            spruce_leaves: table.id("spruce_leaves"),
            cactus: table.id("cactus"),
        };
        Ok(Self { table, ids })
    }

    /// The rock-column blocks only.
    pub fn builtin() -> Self {
        Self::new(BlockRegistry::builtin()).expect("valid built-in registry")
    }

    /// The table alone (what the client receives).
    pub fn table(&self) -> &nascio_chunk::Blocks {
        &self.table
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
        let full = Blocks::new(nascio_world::blocks::registry_with_catalog(&cat).unwrap()).unwrap();
        assert_eq!(full.ids, b.ids);
        assert!(full.len() > b.len());
    }
}
