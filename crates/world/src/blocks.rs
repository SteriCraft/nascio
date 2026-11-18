//! Block registry of a world (spec §5). The types live in `nascio-chunk` (the client reads
//! them without the generation crates); the ore blocks of the deposit catalogue are added here.

pub use nascio_chunk::block::{BUILTIN, BlockDef, BlockRegistry, Matter};

use crate::deposits::Catalog;

/// The column blocks, then every ore block of `catalog` (first definition of a name wins).
pub fn registry_with_catalog(catalog: &Catalog) -> anyhow::Result<BlockRegistry> {
    let mut r = BlockRegistry::builtin();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_blocks_follow_the_column() {
        let cat = Catalog::default_catalog();
        let r = registry_with_catalog(&cat).unwrap();
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
