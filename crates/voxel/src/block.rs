//! Block registry (spec §5). Block ids are `u16`; colours are provisional until textures exist.

/// A block id.
pub type BlockId = u16;

pub const AIR: BlockId = 0;
pub const WATER: BlockId = 1;
pub const STONE: BlockId = 2;
pub const DIRT: BlockId = 3;
pub const GRASS: BlockId = 4;
pub const SAND: BlockId = 5;

/// How a block takes part in rendering and physics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matter {
    /// Nothing: faces next to it are visible.
    Empty,
    /// Opaque solid.
    Solid,
    /// Translucent fluid: hides nothing, but its surface is drawn.
    Fluid,
}

/// Definition of one block type.
#[derive(Clone, Copy, Debug)]
pub struct BlockDef {
    pub name: &'static str,
    /// Provisional sRGB colour.
    pub color: [u8; 3],
    pub matter: Matter,
    /// Density in kg/m³ (0 for air), for later physics.
    pub density: f32,
}

/// The registry, indexed by [`BlockId`].
pub const BLOCKS: [BlockDef; 6] = [
    BlockDef {
        name: "air",
        color: [0, 0, 0],
        matter: Matter::Empty,
        density: 0.0,
    },
    BlockDef {
        name: "water",
        color: [38, 92, 150],
        matter: Matter::Fluid,
        density: 1_000.0,
    },
    BlockDef {
        name: "stone",
        color: [122, 118, 112],
        matter: Matter::Solid,
        density: 2_700.0,
    },
    BlockDef {
        name: "dirt",
        color: [118, 85, 58],
        matter: Matter::Solid,
        density: 1_500.0,
    },
    BlockDef {
        name: "grass",
        color: [88, 132, 58],
        matter: Matter::Solid,
        density: 1_400.0,
    },
    BlockDef {
        name: "sand",
        color: [212, 196, 146],
        matter: Matter::Solid,
        density: 1_600.0,
    },
];

/// Definition of a block id. Unknown ids panic: the registry is closed.
#[inline]
pub fn def(id: BlockId) -> &'static BlockDef {
    &BLOCKS[usize::from(id)]
}

/// Block id by name.
pub fn by_name(name: &str) -> Option<BlockId> {
    BLOCKS
        .iter()
        .position(|b| b.name == name)
        .map(|i| i as BlockId)
}

#[inline]
pub fn is_solid(id: BlockId) -> bool {
    def(id).matter == Matter::Solid
}

/// Linear-light RGB of a block colour (for vertex colours).
pub fn linear_color(id: BlockId) -> [f32; 3] {
    def(id).color.map(|c| srgb_to_linear(f32::from(c) / 255.0))
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
    fn ids_match_registry() {
        for (name, id) in [
            ("air", AIR),
            ("water", WATER),
            ("stone", STONE),
            ("dirt", DIRT),
            ("grass", GRASS),
            ("sand", SAND),
        ] {
            assert_eq!(by_name(name), Some(id));
        }
        assert!(is_solid(STONE) && !is_solid(WATER) && !is_solid(AIR));
    }
}
