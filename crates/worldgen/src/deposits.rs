//! Ore deposits (spec §4.6): a catalogue file describes the deposit types; the code only knows
//! the geological [`Context`]s and the body [`Shape`]s.
//!
//! Bodies are drawn per 2 km tile (2 km divides 100 km): each `(deposit type, tile)` has its own
//! RNG derived from the seed, so the result depends neither on the order of the work nor on the
//! number of threads, and adding a type to the catalogue does not move the others. A body is a
//! coherent shape (vein, stock, lens, layer, blanket, placer); its blocks replace only rock (or
//! sand and gravel for placers) when the chunks are generated ([`Body::ore_at`]).

use std::collections::BTreeMap;
use std::path::Path;

use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::erosion::ErosionOutput;
use crate::erosion::flow::{D8, NO_DIR};
use crate::geology::{CONTEXTS, Context, GeologyOutput};
use crate::grid::PeriodicGrid;
use crate::rng::{mix64, stage_rng};
use crate::world_io::Layer;
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
    pub depth_m: [f64; 2],
    pub size_m: [f64; 2],
    pub thickness_m: [f64; 2],
    pub grade: f64,
    pub abundance: f64,
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
        for c in self.contexts.keys().chain(&self.requires) {
            anyhow::ensure!(
                Context::from_name(c).is_some(),
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
        if let Some(c) = &self.cap {
            anyhow::ensure!(
                self.shape == Shape::Layer && c.fraction > 0.0 && c.fraction < 1.0,
                "deposit {n}: a cap needs a layer and a fraction in (0, 1)"
            );
        }
        Ok(())
    }

    /// Weight of this type for the context weights `w` of a cell.
    pub fn weight(&self, w: &[f32; CONTEXTS]) -> f64 {
        let get = |name: &str| Context::from_name(name).map_or(0.0, |c| f64::from(w[c.index()]));
        let sum: f64 = self.contexts.iter().map(|(c, k)| k * get(c)).sum();
        let req: f64 = self.requires.iter().map(|c| get(c)).product();
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
            Shape::Vein => f64::from(self.b) * f64::from(self.dip).sin() + c,
            Shape::Layer => c * 1.6,
            _ => c,
        };
        (self.y - h, self.y + h)
    }

    /// Irregular outline of sheets: radius factor at angle `t`.
    fn outline(&self, t: f64) -> f64 {
        let s = f64::from(self.seed);
        1.0 + 0.22 * (3.0 * t + s * 0.001).sin() + 0.12 * (5.0 * t + s * 0.017).sin()
    }

    /// Ore at world position `(x, y, z)` of a column whose top solid block is `top`: the
    /// probability that the block is mineralised, and its height in the body from −1 (bottom)
    /// to 1 (top) for sheets (0 otherwise). `None` outside.
    pub fn ore_at(&self, x: f64, y: f64, z: f64, top: f64) -> Option<(f64, f64)> {
        let dx = torus::delta_f64(self.x, x);
        let dz = torus::delta_f64(self.z, z);
        let (sy, cy) = f64::from(self.yaw).sin_cos();
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
                let (sd, cd) = f64::from(self.dip).sin_cos();
                let dy = y - self.y;
                let across = v * sd + dy * cd;
                let along = v * cd - dy * sd;
                let wobble = 0.75 + 0.25 * (u / 9.0 + f64::from(self.seed % 97)).sin();
                let inside =
                    across.abs() < c * wobble && (u / a).powi(2) + (along / b).powi(2) < 1.0;
                inside.then_some((g, 0.0))
            }
            Shape::Layer | Shape::Blanket | Shape::Placer => {
                let r = ((u / a).powi(2) + (v / b).powi(2)).sqrt();
                if r >= 1.45 || r >= self.outline(v.atan2(u)) {
                    return None;
                }
                let dy = if self.shape == Shape::Layer {
                    // Gentle undulation of the bedding.
                    let wave = c * 0.6 * ((u / 170.0 + f64::from(self.seed % 61)).sin());
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

/// Everything the placement reads.
struct Inputs<'a> {
    geo: &'a GeologyOutput,
    ero: &'a ErosionOutput,
    res: usize,
    cell: f64,
}

impl Inputs<'_> {
    fn cell_index(&self, x: f64, z: f64) -> usize {
        let i = ((torus::wrap_f64(x) / self.cell) as usize).min(self.res - 1);
        let j = ((torus::wrap_f64(z) / self.cell) as usize).min(self.res - 1);
        j * self.res + i
    }

    fn weights(&self, x: f64, z: f64) -> &[f32; CONTEXTS] {
        &self.geo.weights[self.cell_index(x, z)]
    }

    fn surface(&self, x: f64, z: f64) -> f64 {
        self.ero.elevation.sample_bilinear(x, z)
    }
}

fn uniform(rng: &mut ChaCha8Rng, r: [f64; 2]) -> f64 {
    r[0] + (r[1] - r[0]) * rng.random::<f64>()
}

/// Poisson draw (Knuth; `lambda` stays small per tile).
fn poisson(rng: &mut ChaCha8Rng, lambda: f64) -> usize {
    if lambda <= 0.0 {
        return 0;
    }
    let l = (-lambda.min(50.0)).exp();
    let mut k = 0;
    let mut p = 1.0;
    loop {
        p *= rng.random::<f64>();
        if p <= l || k >= 200 {
            return k;
        }
        k += 1;
    }
}

/// Moves a placer onto the nearest river cell (river placers) or beach cell (coastal ones)
/// within ~160 m; returns the new centre and the direction of the band.
fn snap_placer(inp: &Inputs, x: f64, z: f64, river: bool) -> Option<(f64, f64, f64)> {
    let e = &inp.ero.elevation;
    let fcell = e.cell_size();
    let (ci, cj) = e.cell_of(x, z);
    let reach = (160.0 / fcell).ceil().max(1.0) as i64;
    let mut best: Option<(f64, i64, i64)> = None;
    for dj in -reach..=reach {
        for di in -reach..=reach {
            let (i, j) = (ci as i64 + di, cj as i64 + dj);
            let k = e.index(i, j);
            let score = if river {
                let s = f64::from(inp.geo.river_source.data()[k]);
                if s <= 0.05 {
                    continue;
                }
                s
            } else {
                let h = f64::from(e.data()[k]);
                let land = *inp.ero.basin_id.get(i, j) != 0;
                if !land || !(0.0..4.0).contains(&h) {
                    continue;
                }
                1.0 / (1.0 + h)
            };
            let d = ((di * di + dj * dj) as f64).sqrt();
            let s = score / (1.0 + 0.2 * d);
            if best.is_none_or(|b| s > b.0) {
                best = Some((s, i, j));
            }
        }
    }
    let (_, i, j) = best?;
    let (px, pz) = (
        (i.rem_euclid(e.width() as i64) as f64 + 0.5) * fcell,
        (j.rem_euclid(e.width() as i64) as f64 + 0.5) * fcell,
    );
    let yaw = if river {
        let d = inp.ero.flow_direction.data()[e.index(i, j)];
        if d == NO_DIR {
            0.0
        } else {
            let (di, dj) = D8[usize::from(d)];
            (dj as f64).atan2(di as f64)
        }
    } else {
        // Along the coast: across the slope.
        let gx = e.sample_bilinear(px + fcell, pz) - e.sample_bilinear(px - fcell, pz);
        let gz = e.sample_bilinear(px, pz + fcell) - e.sample_bilinear(px, pz - fcell);
        gz.atan2(gx) + std::f64::consts::FRAC_PI_2
    };
    Some((px, pz, yaw))
}

/// Draws the bodies of type `kind` in tile `t`.
fn tile_bodies(seed: u64, kind: usize, d: &DepositType, t: usize, inp: &Inputs) -> Vec<Body> {
    if d.abundance <= 0.0 {
        return Vec::new();
    }
    let (ti, tj) = (t % TILES, t / TILES);
    let (x0, z0) = (ti as f64 * TILE_M, tj as f64 * TILE_M);
    // Mean and largest weight over an 8×8 sampling of the tile.
    let (mut mean, mut max) = (0.0f64, 0.0f64);
    for sj in 0..8 {
        for si in 0..8 {
            let x = x0 + (si as f64 + 0.5) * TILE_M / 8.0;
            let z = z0 + (sj as f64 + 0.5) * TILE_M / 8.0;
            let w = d.weight(inp.weights(x, z));
            mean += w / 64.0;
            max = max.max(w);
        }
    }
    if mean <= 0.0 {
        return Vec::new();
    }
    let mut rng = stage_rng(seed, &format!("deposits/{}/{t}", d.name));
    let area_km2 = TILE_M * TILE_M / 1e6;
    let count = poisson(&mut rng, d.abundance * area_km2 / 100.0 * mean);
    let mut out = Vec::new();
    for _ in 0..count {
        // Position: rejection sampling on the weight inside the tile.
        let mut pos = None;
        for _ in 0..16 {
            let x = x0 + rng.random::<f64>() * TILE_M;
            let z = z0 + rng.random::<f64>() * TILE_M;
            if rng.random::<f64>() * max < d.weight(inp.weights(x, z)) {
                pos = Some((x, z));
                break;
            }
        }
        let size = uniform(&mut rng, d.size_m);
        let thick = uniform(&mut rng, d.thickness_m);
        let depth = uniform(&mut rng, d.depth_m);
        let aspect = 0.45 + 0.55 * rng.random::<f64>();
        let mut yaw = rng.random::<f64>() * std::f64::consts::TAU;
        let dip = (45.0 + 40.0 * rng.random::<f64>()).to_radians();
        let grade = (d.grade * (0.75 + 0.5 * rng.random::<f64>())).min(1.0);
        let body_seed = rng.random::<u32>();
        let Some((mut x, mut z)) = pos else {
            continue;
        };
        let k = inp.cell_index(x, z);
        let sediment = f64::from(inp.geo.sediment.data()[k]);
        let weathering = f64::from(inp.geo.weathering.data()[k]);
        let (a, b, c) = match d.shape {
            Shape::Vein => (size / 2.0, size * (0.2 + 0.25 * aspect), thick / 2.0),
            Shape::Stock | Shape::Lens => (size / 2.0, size / 2.0 * aspect, thick / 2.0),
            Shape::Layer | Shape::Blanket | Shape::Trap => {
                (size / 2.0, size / 2.0 * aspect, thick / 2.0)
            }
            Shape::Placer => (size / 2.0, size / 2.0 * (0.12 + 0.1 * aspect), thick / 2.0),
        };
        let surface = inp.surface(x, z);
        let y = match d.shape {
            Shape::Layer if d.host == Host::Sediment => {
                // Inside the sedimentary cover.
                let room = sediment - 2.0 * c;
                if room <= d.depth_m[0] {
                    continue;
                }
                surface
                    - (c + d.depth_m[0]
                        + rng.random::<f64>()
                            * (room - d.depth_m[0]).min(d.depth_m[1] - d.depth_m[0]))
            }
            Shape::Layer => surface - sediment - depth - c,
            Shape::Blanket => {
                if weathering < 2.0 * c.min(1.0) {
                    continue;
                }
                depth.min((weathering - c).max(c))
            }
            Shape::Placer => {
                let river = d.contexts.contains_key("river_source")
                    || d.requires.iter().any(|r| r == "river_source");
                let Some((px, pz, py)) = snap_placer(inp, x, z, river) else {
                    continue;
                };
                (x, z, yaw) = (px, pz, py);
                depth.max(c)
            }
            _ => surface - depth,
        };
        out.push(Body {
            kind: kind as u16,
            shape: d.shape,
            x: torus::wrap_f64(x),
            z: torus::wrap_f64(z),
            y,
            a: a as f32,
            b: b as f32,
            c: c as f32,
            yaw: yaw as f32,
            dip: dip as f32,
            grade: grade as f32,
            seed: body_seed,
        });
    }
    out
}

/// Summary of the placement.
#[derive(Clone, Debug, Default)]
pub struct DepositsReport {
    /// Per type: `(name, bodies, mineralised volume m³, mean depth of the centre m)`.
    pub per_type: Vec<(String, usize, f64, f64)>,
}

impl std::fmt::Display for DepositsReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "deposits:")?;
        for (name, n, v, d) in &self.per_type {
            writeln!(
                f,
                "  {name:<16} {n:>5} bodies, {:>9.3} Mm3 ore, mean depth {d:>4.0} m",
                v / 1e6
            )?;
        }
        Ok(())
    }
}

/// Output of the deposit stage.
pub struct DepositsOutput {
    pub bodies: Vec<Body>,
    /// Dominant deposit (type index + 1) per tectonic cell, 0 = none.
    pub dominant: PeriodicGrid<u8>,
    /// Map-only types: `2k + 1` province, `2k + 2` trap (k: rank among the map-only types).
    pub traps: PeriodicGrid<u8>,
    pub report: DepositsReport,
}

impl DepositsOutput {
    pub fn layers(&self) -> Vec<Layer> {
        vec![
            Layer::from_u8(
                "deposits",
                "deposit type + 1 (deposits.toml order), 0=none",
                &self.dominant,
            ),
            Layer::from_u8(
                "traps",
                "2k+1 province, 2k+2 trap of the k-th map-only type",
                &self.traps,
            ),
        ]
    }
}

/// Places the deposit bodies of `catalog` and builds the map layers.
pub fn run(
    seed: u64,
    catalog: &Catalog,
    geo: &GeologyOutput,
    ero: &ErosionOutput,
    progress: &crate::progress::Progress,
) -> DepositsOutput {
    let res = geo.setting.width();
    let cell = geo.setting.cell_size();
    let inp = Inputs {
        geo,
        ero,
        res,
        cell,
    };
    let mut bodies = Vec::new();
    let nt = catalog.deposits.len().max(1);
    for (kind, d) in catalog.deposits.iter().enumerate() {
        let per_tile: Vec<Vec<Body>> = (0..TILES * TILES)
            .into_par_iter()
            .map(|t| tile_bodies(seed, kind, d, t, &inp))
            .collect();
        bodies.extend(per_tile.into_iter().flatten());
        progress.set_range(0.0, 0.8, kind, nt, 1.0);
    }

    // Map: dominant mineralised volume per cell over the footprints.
    let n = res * res;
    let mut score = vec![BTreeMap::<u16, f64>::new(); n];
    let mut traps = vec![0u8; n];
    let trap_rank: Vec<Option<u8>> = {
        let mut r = 0u8;
        catalog
            .deposits
            .iter()
            .map(|d| {
                (d.shape == Shape::Trap).then(|| {
                    r += 1;
                    r - 1
                })
            })
            .collect()
    };
    for (k, t) in traps.iter_mut().enumerate() {
        let w = &geo.weights[k];
        for (d, rank) in catalog.deposits.iter().zip(&trap_rank) {
            if let Some(r) = rank
                && *t == 0
                && d.weight(w) >= 0.3
            {
                *t = 2 * r + 1;
            }
        }
    }
    for b in &bodies {
        let r = b.radius() * 0.7;
        let reach = (r / cell).ceil() as i64;
        let (ci, cj) = ((b.x / cell) as i64, (b.z / cell) as i64);
        let vol = ore_volume(b);
        for dj in -reach..=reach {
            for di in -reach..=reach {
                let (cx, cz) = (
                    (ci + di) as f64 * cell + cell / 2.0,
                    (cj + dj) as f64 * cell + cell / 2.0,
                );
                if torus::distance_f64((b.x, b.z), (cx, cz)) > r.max(cell * 0.5) {
                    continue;
                }
                let k = geo.setting.index(ci + di, cj + dj);
                if b.shape == Shape::Trap {
                    if let Some(rk) = trap_rank[usize::from(b.kind)] {
                        traps[k] = 2 * rk + 2;
                    }
                } else {
                    *score[k].entry(b.kind).or_default() += vol;
                }
            }
        }
    }
    let dominant: Vec<u8> = score
        .iter()
        .map(|s| {
            s.iter()
                .fold(None, |best: Option<(u16, f64)>, (&k, &v)| match best {
                    Some((_, bv)) if bv >= v => best,
                    _ => Some((k, v)),
                })
                .map_or(0, |(k, _)| (k + 1) as u8)
        })
        .collect();

    let mut report = DepositsReport::default();
    for (kind, d) in catalog.deposits.iter().enumerate() {
        let of: Vec<&Body> = bodies
            .iter()
            .filter(|b| usize::from(b.kind) == kind)
            .collect();
        let vol: f64 = of.iter().map(|b| ore_volume(b)).sum();
        let depth = if of.is_empty() {
            0.0
        } else {
            of.iter()
                .map(|b| {
                    if b.shape.follows_surface() {
                        b.y
                    } else {
                        inp.surface(b.x, b.z) - b.y
                    }
                })
                .sum::<f64>()
                / of.len() as f64
        };
        report.per_type.push((d.name.clone(), of.len(), vol, depth));
    }
    progress.set(1.0);
    let grid = |v: Vec<u8>| PeriodicGrid::from_vec(res, res, cell, v);
    DepositsOutput {
        bodies,
        dominant: grid(dominant),
        traps: grid(traps),
        report,
    }
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

    #[test]
    fn default_catalogue_is_valid_and_hashed() {
        let c = Catalog::default_catalog();
        assert!(c.deposits.len() >= 20);
        assert_eq!(c.hash().len(), 64);
        for name in [
            "lead_zinc",
            "laterite_nickel",
            "platinum",
            "manganese",
            "phosphate",
            "rock_salt",
            "ilmenite",
        ] {
            assert!(c.deposits.iter().any(|d| d.name == name), "{name}");
        }
        let pz = c.deposits.iter().find(|d| d.name == "lead_zinc").unwrap();
        assert!(pz.elements.iter().any(|e| e == "Ag"));
        let salt = c.deposits.iter().find(|d| d.name == "rock_salt").unwrap();
        assert_eq!(salt.cap.as_ref().unwrap().block, "sylvite");
        assert!(
            c.deposits
                .iter()
                .filter(|d| d.shape == Shape::Trap)
                .all(|d| d.blocks.is_empty())
        );
    }

    #[test]
    fn rejects_unknown_context_and_trap_blocks() {
        let base = r#"[[deposit]]
name = "x"
label = "X"
map_color = [1, 2, 3]
contexts = { nowhere = 1.0 }
shape = "lens"
blocks = [{ block = "xo", color = [1, 1, 1] }]
depth_m = [0, 10]
size_m = [10, 20]
thickness_m = [1, 2]
grade = 0.5
abundance = 1
"#;
        assert!(Catalog::parse(base).is_err());
        assert!(Catalog::parse(&base.replace("nowhere", "arc")).is_ok());
        assert!(
            Catalog::parse(
                &base
                    .replace("nowhere", "arc")
                    .replace("\"lens\"", "\"trap\"")
            )
            .is_err()
        );
    }

    fn body(shape: Shape, x: f64) -> Body {
        Body {
            kind: 0,
            shape,
            x,
            z: 500.0,
            y: -100.0,
            a: 60.0,
            b: 40.0,
            c: 20.0,
            yaw: 0.3,
            dip: 1.0,
            grade: 0.5,
            seed: 7,
        }
    }

    #[test]
    fn bodies_round_trip_and_cross_the_seam() {
        let bodies = vec![body(Shape::Stock, 10.0), body(Shape::Layer, 99_990.0)];
        let back = bodies_from_bytes(&bodies_to_bytes(&bodies)).unwrap();
        assert_eq!(back, bodies);
        // A body near x = 0 is found from both sides of the seam, with the same ore.
        let idx = BodyIndex::new(bodies);
        assert_eq!(idx.near(99_968.0, 480.0, 32.0).len(), 2);
        assert_eq!(idx.near(0.0, 480.0, 32.0).len(), 2);
        let s = &idx.bodies[0];
        assert!(s.ore_at(10.0, -100.0, 500.0, 0.0).is_some());
        assert_eq!(
            s.ore_at(-20.0, -100.0, 500.0, 0.0),
            s.ore_at(99_980.0, -100.0, 500.0, 0.0)
        );
        assert!(s.ore_at(10.0, -200.0, 500.0, 0.0).is_none());
        // Stock grade fades from the core.
        let core = s.ore_at(10.0, -100.0, 500.0, 0.0).unwrap().0;
        let rim = s.ore_at(10.0, -117.0, 500.0, 0.0).unwrap().0;
        assert!(core > rim);
    }

    #[test]
    fn blocks_by_share_and_cap() {
        let c = Catalog::default_catalog();
        let pc = c
            .deposits
            .iter()
            .find(|d| d.name == "porphyry_copper")
            .unwrap();
        assert_eq!(pick_block(pc, 0.9, 0.1), "chalcopyrite");
        assert_eq!(pick_block(pc, 0.9, 0.95), "molybdenite");
        let salt = c.deposits.iter().find(|d| d.name == "rock_salt").unwrap();
        assert_eq!(pick_block(salt, 0.8, 0.5), "sylvite");
        assert_eq!(pick_block(salt, 0.2, 0.5), "rock_salt");
        let h = block_hash(3, -5, 7, 99_999);
        assert!((0.0..1.0).contains(&h) && h == block_hash(3, -5, 7, 99_999));
    }
}
