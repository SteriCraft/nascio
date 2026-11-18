//! Ore deposits (spec §4.6): a catalogue file describes the deposit types; the code only knows
//! the geological [`Context`]s and the body [`Shape`]s.
//!
//! Bodies are drawn per 2 km tile (2 km divides 100 km): each `(deposit type, tile)` has its own
//! RNG derived from the seed, so the result depends neither on the order of the work nor on the
//! number of threads, and adding a type to the catalogue does not move the others. A body is a
//! coherent shape (vein, stock, lens, layer, blanket, placer); its blocks replace only rock (or
//! sand and gravel for placers) when the chunks are generated ([`Body::ore_at`]).

use nascio_core::fmath::DetMath;
use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::geology::{CONTEXTS, Context};
use crate::rng::mix64;
use nascio_core::consts::WORLD_SIZE_F64;
use nascio_core::torus;

/// The default catalogue, compiled in (a copy of `config/deposits.toml`).
pub const DEFAULT_CATALOG: &str = include_str!("../../../config/deposits.toml");
/// Where the CLI and the viewer look for the catalogue.
pub const CATALOG_PATH: &str = "config/deposits.toml";
/// Side of a placement tile, m (divides the world).
pub const TILE_M: f64 = 2_000.0;
/// Tiles per axis.
pub const TILES: usize = 50;
/// Map-only types fit the bits of the `traps` layer (two bits each).
pub const MAX_TRAP_TYPES: usize = 4;

/// Body shapes known to the code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    /// Thin dipping slab (veins).
    Vein,
    /// Ellipsoid whose grade fades from the core (porphyry stocks).
    Stock,
    /// Small massive ellipsoid.
    Lens,
    /// Horizontal sheet with an irregular outline, in the sediments or the basement.
    Layer,
    /// Band parallel to the surface in the weathered zone.
    Blanket,
    /// Elongated band under a river bed or a beach, in sand and gravel.
    Placer,
    /// Map only (no block).
    Trap,
}

impl Shape {
    pub fn code(self) -> u8 {
        self as u8
    }

    fn from_code(c: u8) -> anyhow::Result<Self> {
        Ok(match c {
            0 => Shape::Vein,
            1 => Shape::Stock,
            2 => Shape::Lens,
            3 => Shape::Layer,
            4 => Shape::Blanket,
            5 => Shape::Placer,
            6 => Shape::Trap,
            _ => anyhow::bail!("unknown shape code {c}"),
        })
    }

    /// Whether the body follows the surface (its `y` is a depth below the ground).
    pub fn follows_surface(self) -> bool {
        matches!(self, Shape::Blanket | Shape::Placer)
    }
}

/// Rock a layer lies in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Host {
    #[default]
    Sediment,
    Basement,
}

fn one() -> f64 {
    1.0
}

fn default_density() -> f32 {
    3_000.0
}

/// One ore block of a deposit type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OreBlock {
    pub block: String,
    pub color: [u8; 3],
    #[serde(default = "one")]
    pub share: f64,
    #[serde(default = "default_density")]
    pub density: f32,
}

/// Block at the top of a layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cap {
    pub block: String,
    pub color: [u8; 3],
    /// Share of the layer thickness, from the top.
    pub fraction: f64,
    #[serde(default = "default_density")]
    pub density: f32,
}

/// A deposit type of the catalogue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DepositType {
    pub name: String,
    pub label: String,
    pub map_color: [u8; 3],
    #[serde(default)]
    pub blocks: Vec<OreBlock>,
    #[serde(default)]
    pub cap: Option<Cap>,
    pub contexts: BTreeMap<String, f64>,
    #[serde(default)]
    pub requires: Vec<String>,
    pub shape: Shape,
    #[serde(default)]
    pub host: Host,
    /// Rocks (block names) the body may replace, e.g. flint in limestone and chalk; empty:
    /// any rock its shape allows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_rock: Vec<String>,
    pub depth_m: [f64; 2],
    pub size_m: [f64; 2],
    pub thickness_m: [f64; 2],
    pub grade: f64,
    pub abundance: f64,
    /// Hottest rock the body may start in, °C (e.g. the oil window): the depth range of a body
    /// shrinks above that isotherm of its column (`[geotherm]`). None: no limit.
    #[serde(default)]
    pub max_temperature_c: Option<f64>,
    #[serde(default)]
    pub elements: Vec<String>,
}

impl DepositType {
    /// `(name, colour, density)` of every block it uses (ore blocks, then the cap).
    pub fn block_defs(&self) -> Vec<(&str, [u8; 3], f32)> {
        let mut v: Vec<_> = self
            .blocks
            .iter()
            .map(|b| (b.block.as_str(), b.color, b.density))
            .collect();
        if let Some(c) = &self.cap {
            v.push((c.block.as_str(), c.color, c.density));
        }
        v
    }

    fn validate(&self) -> anyhow::Result<()> {
        let n = &self.name;
        for c in self
            .contexts
            .keys()
            .map(String::as_str)
            .chain(self.requires.iter().flat_map(|r| r.split('|')))
        {
            anyhow::ensure!(
                Context::from_name(c.trim()).is_some(),
                "deposit {n}: unknown context {c}"
            );
        }
        let ordered = |r: [f64; 2]| r[0] >= 0.0 && r[0] <= r[1];
        anyhow::ensure!(
            ordered(self.depth_m) && ordered(self.size_m) && ordered(self.thickness_m),
            "deposit {n}: depth_m, size_m and thickness_m must be increasing ranges >= 0"
        );
        anyhow::ensure!(
            self.size_m[0] > 0.0 && self.size_m[1] <= 4_000.0 && self.thickness_m[1] > 0.0,
            "deposit {n}: sizes must lie in (0, 4000] m and thicknesses be > 0"
        );
        anyhow::ensure!(
            self.max_temperature_c.is_none_or(|t| t > 0.0),
            "deposit {n}: max_temperature_c must be > 0"
        );
        anyhow::ensure!(
            self.grade > 0.0 && self.grade <= 1.0 && self.abundance >= 0.0,
            "deposit {n}: grade must lie in (0, 1] and abundance be >= 0"
        );
        if self.shape == Shape::Trap {
            anyhow::ensure!(
                self.blocks.is_empty() && self.cap.is_none(),
                "deposit {n}: a trap has no block"
            );
        } else {
            anyhow::ensure!(!self.blocks.is_empty(), "deposit {n}: no block");
            anyhow::ensure!(
                self.blocks
                    .iter()
                    .all(|b| b.share > 0.0 && !b.block.is_empty()),
                "deposit {n}: block shares must be > 0"
            );
        }
        anyhow::ensure!(
            self.host_rock.iter().all(|r| !r.trim().is_empty()),
            "deposit {n}: empty host_rock name"
        );
        if let Some(c) = &self.cap {
            anyhow::ensure!(
                self.shape == Shape::Layer && c.fraction > 0.0 && c.fraction < 1.0,
                "deposit {n}: a cap needs a layer and a fraction in (0, 1)"
            );
        }
        Ok(())
    }

    /// Weight of this type for the context weights `w` of a cell: `Σ weight × context`, times
    /// each requirement (`"a|b"`: the larger of `a` and `b`).
    pub fn weight(&self, w: &[f32; CONTEXTS]) -> f64 {
        let get =
            |name: &str| Context::from_name(name.trim()).map_or(0.0, |c| f64::from(w[c.index()]));
        let sum: f64 = self.contexts.iter().map(|(c, k)| k * get(c)).sum();
        let req: f64 = self
            .requires
            .iter()
            .map(|r| r.split('|').map(get).fold(0.0, f64::max))
            .product();
        (sum * req).max(0.0)
    }
}

/// The deposit catalogue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    #[serde(rename = "deposit", default)]
    pub deposits: Vec<DepositType>,
    /// Source text, copied into the world.
    #[serde(skip)]
    pub text: String,
}

impl Catalog {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let mut c: Self = toml::from_str(text)?;
        c.text = text.to_string();
        for (i, d) in c.deposits.iter().enumerate() {
            d.validate()?;
            anyhow::ensure!(
                c.deposits[..i].iter().all(|o| o.name != d.name),
                "duplicate deposit {}",
                d.name
            );
        }
        anyhow::ensure!(c.deposits.len() < 127, "at most 126 deposit types");
        anyhow::ensure!(
            c.deposits.iter().filter(|d| d.shape == Shape::Trap).count() <= MAX_TRAP_TYPES,
            "at most {MAX_TRAP_TYPES} map-only (trap) types"
        );
        Ok(c)
    }

    /// The compiled-in catalogue.
    pub fn default_catalog() -> Self {
        Self::parse(DEFAULT_CATALOG).expect("valid default catalogue")
    }

    /// `path` if given, else [`CATALOG_PATH`] if it exists, else the compiled-in catalogue.
    pub fn load_or_default(path: Option<&Path>) -> anyhow::Result<Self> {
        let path = path.map(Path::to_path_buf).or_else(|| {
            let p = Path::new(CATALOG_PATH);
            p.exists().then(|| p.to_path_buf())
        });
        match path {
            Some(p) => {
                let text = std::fs::read_to_string(&p)
                    .map_err(|e| anyhow::anyhow!("reading {}: {e}", p.display()))?;
                Self::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e:#}", p.display()))
            }
            None => Ok(Self::default_catalog()),
        }
    }

    /// blake3 of the source text, hex.
    pub fn hash(&self) -> String {
        blake3::hash(self.text.as_bytes()).to_hex().to_string()
    }
}

/// One deposit body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// Index of its type in the catalogue.
    pub kind: u16,
    pub shape: Shape,
    /// Centre, m (`x`, `z` wrapped). `y`: absolute height of the centre, or for shapes that
    /// follow the surface, depth of the centre below the ground.
    pub x: f64,
    pub z: f64,
    pub y: f64,
    /// Half length (along `yaw`), half width, half thickness, m. For a vein `b` is the half
    /// extent down the dip.
    pub a: f32,
    pub b: f32,
    pub c: f32,
    /// Strike from +x towards +z, radians; dip below the horizontal, radians (veins).
    pub yaw: f32,
    pub dip: f32,
    /// Share of mineralised blocks at the core.
    pub grade: f32,
    pub seed: u32,
}

/// Size of a serialised body.
const BODY_BYTES: usize = 2 + 1 + 3 * 8 + 6 * 4 + 4;
const BODIES_MAGIC: &[u8; 4] = b"NDEP";

/// Mineralised volume of a body, m³.
pub fn ore_volume(b: &Body) -> f64 {
    let (a, bb, c) = (f64::from(b.a), f64::from(b.b), f64::from(b.c));
    let g = f64::from(b.grade);
    let pi = std::f64::consts::PI;
    match b.shape {
        Shape::Stock => 4.0 / 3.0 * pi * a * bb * c * g * 0.4,
        Shape::Lens => 4.0 / 3.0 * pi * a * bb * c * g,
        Shape::Vein | Shape::Layer | Shape::Blanket | Shape::Placer => pi * a * bb * 2.0 * c * g,
        Shape::Trap => 0.0,
    }
}

impl Body {
    /// Horizontal radius of a disc holding the body, m.
    pub fn radius(&self) -> f64 {
        let r = f64::from(self.a.max(self.b));
        match self.shape {
            Shape::Layer | Shape::Blanket | Shape::Placer => r * 1.45,
            _ => r + f64::from(self.c),
        }
    }

    /// Lowest and highest absolute heights of a body that does not follow the surface.
    pub fn y_range(&self) -> (f64, f64) {
        let c = f64::from(self.c);
        let h = match self.shape {
            Shape::Vein => f64::from(self.b) * f64::from(self.dip).det_sin() + c,
            Shape::Layer => c * 1.6,
            _ => c,
        };
        (self.y - h, self.y + h)
    }

    /// Irregular outline of sheets: radius factor at angle `t`.
    fn outline(&self, t: f64) -> f64 {
        let s = f64::from(self.seed);
        1.0 + 0.22 * (3.0 * t + s * 0.001).det_sin() + 0.12 * (5.0 * t + s * 0.017).det_sin()
    }

    /// Ore at world position `(x, y, z)` of a column whose top solid block is `top`: the
    /// probability that the block is mineralised, and its height in the body from −1 (bottom)
    /// to 1 (top) for sheets (0 otherwise). `None` outside.
    pub fn ore_at(&self, x: f64, y: f64, z: f64, top: f64) -> Option<(f64, f64)> {
        let dx = torus::delta_f64(self.x, x);
        let dz = torus::delta_f64(self.z, z);
        let (sy, cy) = f64::from(self.yaw).det_sin_cos();
        let u = dx * cy + dz * sy;
        let v = -dx * sy + dz * cy;
        let (a, b, c) = (f64::from(self.a), f64::from(self.b), f64::from(self.c));
        let g = f64::from(self.grade);
        match self.shape {
            Shape::Stock | Shape::Lens => {
                let dy = y - self.y;
                let r2 = (u / a).powi(2) + (v / b).powi(2) + (dy / c).powi(2);
                (r2 < 1.0).then(|| {
                    let k = if self.shape == Shape::Stock {
                        1.0 - r2
                    } else {
                        1.0
                    };
                    (g * k, 0.0)
                })
            }
            Shape::Vein => {
                let (sd, cd) = f64::from(self.dip).det_sin_cos();
                let dy = y - self.y;
                let across = v * sd + dy * cd;
                let along = v * cd - dy * sd;
                let wobble = 0.75 + 0.25 * (u / 9.0 + f64::from(self.seed % 97)).det_sin();
                let inside =
                    across.abs() < c * wobble && (u / a).powi(2) + (along / b).powi(2) < 1.0;
                inside.then_some((g, 0.0))
            }
            Shape::Layer | Shape::Blanket | Shape::Placer => {
                let r = ((u / a).powi(2) + (v / b).powi(2)).sqrt();
                if r >= 1.45 || r >= self.outline(v.det_atan2(u)) {
                    return None;
                }
                let dy = if self.shape == Shape::Layer {
                    // Gentle undulation of the bedding.
                    let wave = c * 0.6 * ((u / 170.0 + f64::from(self.seed % 61)).det_sin());
                    y - (self.y + wave)
                } else {
                    // Depth below the ground, measured downwards.
                    (top - y) - self.y
                };
                if dy.abs() >= c {
                    return None;
                }
                // Height in the body, −1 at the bottom, 1 at the top (`dy` grows downwards for
                // the surface shapes).
                let rel = if self.shape == Shape::Layer {
                    dy / c
                } else {
                    -dy / c
                };
                Some((g, rel))
            }
            Shape::Trap => None,
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.kind.to_le_bytes());
        out.push(self.shape.code());
        for v in [self.x, self.z, self.y] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in [self.a, self.b, self.c, self.yaw, self.dip, self.grade] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.seed.to_le_bytes());
    }

    fn read(b: &[u8]) -> anyhow::Result<Self> {
        let f64_at = |o: usize| f64::from_le_bytes(b[o..o + 8].try_into().expect("8 bytes"));
        let f32_at = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"));
        Ok(Self {
            kind: u16::from_le_bytes([b[0], b[1]]),
            shape: Shape::from_code(b[2])?,
            x: f64_at(3),
            z: f64_at(11),
            y: f64_at(19),
            a: f32_at(27),
            b: f32_at(31),
            c: f32_at(35),
            yaw: f32_at(39),
            dip: f32_at(43),
            grade: f32_at(47),
            seed: u32::from_le_bytes(b[51..55].try_into()?),
        })
    }
}

/// Serialises bodies (`deposits.bin`).
pub fn bodies_to_bytes(bodies: &[Body]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + bodies.len() * BODY_BYTES);
    out.extend_from_slice(BODIES_MAGIC);
    out.extend_from_slice(&(bodies.len() as u32).to_le_bytes());
    for b in bodies {
        b.write(&mut out);
    }
    out
}

/// Parses `deposits.bin`.
pub fn bodies_from_bytes(bytes: &[u8]) -> anyhow::Result<Vec<Body>> {
    anyhow::ensure!(
        bytes.len() >= 8 && &bytes[..4] == BODIES_MAGIC,
        "not a deposits file"
    );
    let n = u32::from_le_bytes(bytes[4..8].try_into()?) as usize;
    anyhow::ensure!(bytes.len() == 8 + n * BODY_BYTES, "truncated deposits file");
    bytes[8..]
        .as_chunks::<BODY_BYTES>()
        .0
        .iter()
        .map(|b| Body::read(b))
        .collect()
}

/// Spatial index of bodies: for each tile, the bodies whose disc reaches it.
#[derive(Clone, Debug, Default)]
pub struct BodyIndex {
    pub bodies: Vec<Body>,
    tiles: Vec<Vec<u32>>,
}

impl BodyIndex {
    pub fn new(bodies: Vec<Body>) -> Self {
        let mut tiles = vec![Vec::new(); TILES * TILES];
        for (i, b) in bodies.iter().enumerate() {
            if b.shape == Shape::Trap {
                continue;
            }
            let r = b.radius();
            let lo_i = ((b.x - r) / TILE_M).floor() as i64;
            let hi_i = ((b.x + r) / TILE_M).floor() as i64;
            let lo_j = ((b.z - r) / TILE_M).floor() as i64;
            let hi_j = ((b.z + r) / TILE_M).floor() as i64;
            for tj in lo_j..=hi_j {
                for ti in lo_i..=hi_i {
                    let t = tj.rem_euclid(TILES as i64) as usize * TILES
                        + ti.rem_euclid(TILES as i64) as usize;
                    if tiles[t].last() != Some(&(i as u32)) {
                        tiles[t].push(i as u32);
                    }
                }
            }
        }
        Self { bodies, tiles }
    }

    /// Bodies whose disc may reach the square `[x, x + size) × [z, z + size)` (any position).
    pub fn near(&self, x: f64, z: f64, size: f64) -> Vec<&Body> {
        let mut ids: Vec<u32> = Vec::new();
        let lo_i = (x / TILE_M).floor() as i64;
        let hi_i = ((x + size) / TILE_M).floor() as i64;
        let lo_j = (z / TILE_M).floor() as i64;
        let hi_j = ((z + size) / TILE_M).floor() as i64;
        for tj in lo_j..=hi_j {
            for ti in lo_i..=hi_i {
                let t = tj.rem_euclid(TILES as i64) as usize * TILES
                    + ti.rem_euclid(TILES as i64) as usize;
                ids.extend(&self.tiles[t]);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        let (cx, cz) = (x + size / 2.0, z + size / 2.0);
        ids.into_iter()
            .map(|i| &self.bodies[i as usize])
            .filter(|b| {
                torus::distance_f64((b.x, b.z), (cx, cz))
                    <= b.radius() + size * std::f64::consts::FRAC_1_SQRT_2
            })
            .collect()
    }
}

/// Deterministic hash of a block and a body, in `[0, 1)`.
#[inline]
pub fn block_hash(seed: u32, x: i32, y: i32, z: i32) -> f64 {
    let h = mix64(
        u64::from(seed)
            ^ mix64((x as u32 as u64) | ((z as u32 as u64) << 32))
            ^ mix64(y as u32 as u64 ^ 0x5555),
    );
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Picks the ore block of a mineralised block at height `rel` in its body (−1..1): the cap
/// block in the top `fraction` of a layer, else by share (`h` in `[0, 1)`).
pub fn pick_block(d: &DepositType, rel: f64, h: f64) -> &str {
    if let Some(c) = &d.cap
        && rel > 1.0 - 2.0 * c.fraction
    {
        return &c.block;
    }
    let total: f64 = d.blocks.iter().map(|b| b.share).sum();
    let mut acc = 0.0;
    for b in &d.blocks {
        acc += b.share / total;
        if h < acc {
            return &b.block;
        }
    }
    &d.blocks.last().expect("validated").block
}

/// Keeps the world size in scope for the tile size check.
const _: () = assert!((WORLD_SIZE_F64 as usize).is_multiple_of(TILE_M as usize));

#[cfg(test)]
mod tests {
    use super::*;

    fn flint(extra: &str) -> String {
        format!(
            r#"[[deposit]]
name = "flint"
label = "Flint"
map_color = [1, 2, 3]
blocks = [{{ block = "flint", color = [1, 2, 3] }}]
contexts = {{ platform = 1.0 }}
shape = "lens"
depth_m = [0, 60]
size_m = [8, 30]
thickness_m = [1, 2]
grade = 0.3
abundance = 500
{extra}"#
        )
    }

    #[test]
    fn host_rock_is_optional_and_checked() {
        let c = Catalog::parse(&flint(r#"host_rock = ["limestone", "chalk"]"#)).unwrap();
        assert_eq!(c.deposits[0].host_rock, ["limestone", "chalk"]);
        let plain = Catalog::parse(&flint("")).unwrap();
        assert!(plain.deposits[0].host_rock.is_empty());
        // Absent from a written catalogue when empty.
        assert!(!toml::to_string(&plain).unwrap().contains("host_rock"));
        assert!(Catalog::parse(&flint(r#"host_rock = [" "]"#)).is_err());
    }
}
