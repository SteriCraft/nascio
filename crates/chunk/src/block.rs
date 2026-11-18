//! Block table of a world (spec §5): block names → `u16` ids, assigned at generation (the
//! blocks of the rock column first, in [`BUILTIN`] order, then the catalogue's ore blocks) and
//! stored with the world as `blocks.toml`. The server sends it to the client at connection
//! (`ServerHello`). Code refers to blocks by name. Colours are provisional until textures
//! exist.

use serde::{Deserialize, Serialize};

/// A block id.
pub type BlockId = u16;

/// Air is always id 0 (validated by the registry): empty chunks are all zeros.
pub const AIR: BlockId = 0;

/// Blocks drawn as foliage (their sides keep their green under the seasonal snow).
pub const FOLIAGE: [&str; 1] = ["spruce_leaves"];

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
pub const BUILTIN: [(&str, [u8; 3], Matter, f32); 32] = [
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
    // Chalk beds of marine and platform sediments (host of flint).
    ("chalk", [236, 232, 220], Matter::Solid, 2_000.0),
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

/// Colour multiplying a textured face (vertex colour).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FaceTint {
    /// White: the texture as is.
    None,
    /// The chunk's climate grass tint (`ChunkTint::at`) times the factor when the block is the
    /// chunk's tinted block (`tint.grass`), white otherwise. The factor brings a grey texture's
    /// mean back to the tint (1 / its mean linear luminance).
    Climate(f32),
    /// A fixed linear RGB colour.
    Fixed([f32; 3]),
}

/// Texture of one face: layer in the block texture array and its tint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceTexture {
    pub layer: u16,
    pub tint: FaceTint,
}

/// Textures of a block's faces: top (+y), sides (±x, ±z), bottom (−y). `None`: the face keeps
/// the block colour.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BlockTextures {
    pub top: Option<FaceTexture>,
    pub side: Option<FaceTexture>,
    pub bottom: Option<FaceTexture>,
}

/// The block table with what meshing needs per id (matter, linear colour, foliage, textures).
#[derive(Clone, Debug)]
pub struct Blocks {
    registry: BlockRegistry,
    matter: Vec<Matter>,
    linear: Vec<[f32; 3]>,
    foliage: Vec<bool>,
    textures: Vec<BlockTextures>,
}

impl Blocks {
    pub fn new(registry: BlockRegistry) -> anyhow::Result<Self> {
        registry.validate()?;
        let matter = registry.blocks.iter().map(|b| b.matter).collect();
        let linear = registry
            .blocks
            .iter()
            .map(|b| b.color.map(|c| srgb_to_linear(f32::from(c) / 255.0)))
            .collect();
        let foliage = registry
            .blocks
            .iter()
            .map(|b| FOLIAGE.contains(&b.name.as_str()))
            .collect();
        let textures = vec![BlockTextures::default(); registry.blocks.len()];
        Ok(Self {
            registry,
            matter,
            linear,
            foliage,
            textures,
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
        self.foliage[usize::from(id)]
    }

    /// Sets the face textures of every block (one entry per id).
    pub fn set_textures(&mut self, textures: Vec<BlockTextures>) -> anyhow::Result<()> {
        anyhow::ensure!(
            textures.len() == self.len(),
            "{} textures for {} blocks",
            textures.len(),
            self.len()
        );
        self.textures = textures;
        Ok(())
    }

    /// Face textures of a block.
    #[inline]
    pub fn textures(&self, id: BlockId) -> &BlockTextures {
        &self.textures[usize::from(id)]
    }

    /// Texture of the face of `id` whose normal is `sign` along `normal_axis`: top for +y,
    /// bottom for −y, side otherwise.
    #[inline]
    pub fn face_texture(&self, id: BlockId, normal_axis: usize, sign: i32) -> Option<FaceTexture> {
        let t = self.textures(id);
        match (normal_axis, sign > 0) {
            (1, true) => t.top,
            (1, false) => t.bottom,
            _ => t.side,
        }
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
    fn builtin_table_is_valid_and_round_trips() {
        let r = BlockRegistry::builtin();
        r.validate().unwrap();
        assert_eq!(r.id("air"), Some(AIR));
        assert_eq!(BlockRegistry::from_toml(&r.to_toml().unwrap()).unwrap(), r);
        let b = Blocks::new(r).unwrap();
        let (granite, water) = (b.id("granite").unwrap(), b.id("water").unwrap());
        assert!(b.is_solid(granite) && !b.is_solid(water) && !b.is_solid(AIR));
        assert_eq!(b.matter(water), Matter::Fluid);
        assert!(b.is_foliage(b.id("spruce_leaves").unwrap()));
        assert!(!b.is_foliage(granite));
    }

    #[test]
    fn face_textures_are_set_and_picked_per_face() {
        let mut b = Blocks::builtin();
        assert!(b.set_textures(vec![BlockTextures::default(); 3]).is_err());
        let granite = b.id("granite").unwrap();
        let tex = |layer| {
            Some(FaceTexture {
                layer,
                tint: FaceTint::None,
            })
        };
        let mut t = vec![BlockTextures::default(); b.len()];
        t[usize::from(granite)] = BlockTextures {
            top: tex(1),
            side: tex(2),
            bottom: tex(3),
        };
        b.set_textures(t).unwrap();
        assert_eq!(b.face_texture(granite, 1, 1), tex(1));
        assert_eq!(b.face_texture(granite, 1, -1), tex(3));
        for (d, s) in [(0, 1), (0, -1), (2, 1), (2, -1)] {
            assert_eq!(b.face_texture(granite, d, s), tex(2));
        }
        assert_eq!(b.face_texture(AIR, 1, 1), None);
    }

    #[test]
    fn invalid_tables_are_refused() {
        let mut r = BlockRegistry::builtin();
        r.blocks.swap(0, 1);
        assert!(Blocks::new(r).is_err(), "air must be id 0");
        let mut r = BlockRegistry::builtin();
        r.blocks[2].name = "water".into();
        assert!(Blocks::new(r).is_err(), "duplicate name");
        let mut r = BlockRegistry::builtin();
        r.blocks[3].id = 9;
        assert!(Blocks::new(r).is_err(), "ids in order");
    }
}
