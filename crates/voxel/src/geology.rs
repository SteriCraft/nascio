//! Rock column of the chunks (spec §4.6): soil, weathering, sedimentary cover, basement by
//! geological setting, lower crust, Moho, then magma; and the ore bodies of the deposit
//! catalogue.
//!
//! Everything is a pure function of the world position (wrapped), so neighbouring chunks agree
//! and the column is continuous across the seam. Setting boundaries are warped by a periodic
//! noise so that they do not follow the 160 m cells.

use anyhow::Context;
use nascio_core::torus;
use nascio_world::deposits::{Body, BodyIndex, Catalog, Host, Shape, block_hash};
use nascio_world::geology::setting;
use nascio_world::grid::PeriodicGrid;
use nascio_world::noise::Fbm;
use nascio_world::rng::{mix64, stage_u64};
use nascio_world::world_io::{Layer, World};

use crate::block::{BlockId, Blocks};

/// What a block of the column is made of, for the hosts of the ore bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Grass and dirt.
    Soil,
    /// Sand and gravel.
    Loose,
    /// Regolith and laterite.
    Weathered,
    Sediment,
    Basement,
}

/// Rock noise of one block column (wrapped block centre), each value sampled on first use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RockNoise {
    x: f64,
    z: f64,
    sediment: Option<(f64, f64)>,
    pluton: Option<bool>,
    pluton_far: Option<f64>,
}

impl RockNoise {
    pub fn new(x: f64, z: f64) -> Self {
        Self {
            x,
            z,
            sediment: None,
            pluton: None,
            pluton_far: None,
        }
    }
}

/// Geology of one column, sampled once per column.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColumnGeo {
    /// Setting ([`setting`]) at the warped position.
    pub setting: u8,
    /// Oceanic crust under the column.
    pub oceanic: bool,
    pub sediment: f32,
    pub weathering: f32,
    pub laterite: bool,
    /// On a river bed (gravel).
    pub river: bool,
    /// Biome code ([`biomes::code`]) at the warped position.
    pub biome: u8,
    /// Surface climate (set by [`crate::Terrain::column_geo`]).
    pub climate: crate::climate::ColumnClimate,
    pub arid: bool,
}

/// Ore blocks of one deposit type.
#[derive(Clone, Debug)]
struct Ore {
    shape: Shape,
    host: Host,
    /// `(block, cumulative share)`.
    blocks: Vec<(BlockId, f64)>,
    /// `(block, share of the thickness from the top)`.
    cap: Option<(BlockId, f64)>,
    /// Rocks the body may replace (`host_rock`); empty: any rock of its class.
    host_rock: Vec<BlockId>,
}

/// The bodies of the types placed `above` another first, the rest after, each in its order:
/// where a body overlaps its parent (an oxidised cap on the top of its porphyry), the one on
/// top mineralises the block first.
fn on_top_first(bodies: Vec<Body>, cat: &Catalog) -> Vec<Body> {
    let on_top = |b: &Body| {
        cat.deposits
            .get(usize::from(b.kind))
            .is_some_and(|d| d.above.is_some())
    };
    let (mut first, rest): (Vec<Body>, Vec<Body>) = bodies.into_iter().partition(on_top);
    first.extend(rest);
    first
}

/// Geological data of a world for the chunks.
#[derive(Clone, Debug)]
pub struct Geology {
    setting: PeriodicGrid<u8>,
    crust: PeriodicGrid<u8>,
    sediment: PeriodicGrid<f32>,
    weathering: PeriodicGrid<f32>,
    laterite: PeriodicGrid<f32>,
    precipitation: PeriodicGrid<f32>,
    rivers: PeriodicGrid<u8>,
    biome: PeriodicGrid<u8>,
    warp_x: Fbm,
    warp_z: Fbm,
    /// Finer warp of the biome boundaries only (the biome grid is 160 m).
    biome_warp_x: Fbm,
    biome_warp_z: Fbm,
    pluton: Fbm,
    strata: Fbm,
    facies: Fbm,
    strata_seed: u64,
    bodies: BodyIndex,
    ores: Vec<Ore>,
}

fn grid_u8(l: &Layer) -> PeriodicGrid<u8> {
    let data = (0..l.width * l.height)
        .map(|k| l.data.get_f64(k) as u8)
        .collect();
    PeriodicGrid::from_vec(l.width, l.height, l.cell_m, data)
}

fn grid_f32(l: &Layer) -> PeriodicGrid<f32> {
    let data = (0..l.width * l.height)
        .map(|k| l.data.get_f64(k) as f32)
        .collect();
    PeriodicGrid::from_vec(l.width, l.height, l.cell_m, data)
}

/// Precipitation below which the sediments hold gypsum, mm/yr.
const ARID_MM: f64 = 400.0;
/// Amplitude of the warp of setting boundaries, m.
const WARP_M: f64 = 120.0;
/// Amplitude of the finer warp of biome boundaries, m.
const BIOME_WARP_M: f64 = 60.0;
/// Mean thickness of a sedimentary bed, m.
const BED_M: f64 = 7.0;

impl Geology {
    /// Geology of a generated world. Every layer is required (worlds made before M5 are
    /// refused).
    pub fn from_world(world: &World, blocks: &Blocks) -> anyhow::Result<Self> {
        let set = world
            .layer("geo_context")
            .context("world has no geo_context layer (made before M5?)")?;
        let crust = world
            .layer("crust_type")
            .context("world has no crust_type layer (made before M5?)")?;
        let sed = world
            .layer("sediment_thickness")
            .context("world has no sediment_thickness layer (made before M5?)")?;
        let wea = world
            .layer("weathering_depth")
            .context("world has no weathering_depth layer (made before M5?)")?;
        let lat = world
            .layer("laterite")
            .context("world has no laterite layer (made before M5?)")?;
        let cat = world
            .catalog()?
            .context("world has no deposit catalogue (made before M5?)")?;
        let mut ores = Vec::new();
        {
            for d in &cat.deposits {
                let id = |name: &str| {
                    blocks
                        .id(name)
                        .ok_or_else(|| anyhow::anyhow!("block {name} missing from the registry"))
                };
                let total: f64 = d.blocks.iter().map(|b| b.share).sum();
                let mut acc = 0.0;
                let mut list = Vec::new();
                for b in &d.blocks {
                    acc += b.share / total.max(1e-12);
                    list.push((id(&b.block)?, acc));
                }
                let cap = match &d.cap {
                    Some(c) => Some((id(&c.block)?, c.fraction)),
                    None => None,
                };
                let host_rock = d
                    .host_rock
                    .iter()
                    .map(|n| id(n))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                ores.push(Ore {
                    shape: d.shape,
                    host: d.host,
                    blocks: list,
                    cap,
                    host_rock,
                });
            }
        }
        let seed = world.config.seed;
        let s = |label: &str| stage_u64(seed, label);
        Ok(Self {
            setting: grid_u8(set),
            crust: grid_u8(crust),
            sediment: grid_f32(sed),
            weathering: grid_f32(wea),
            laterite: grid_f32(lat),
            precipitation: grid_f32(
                world
                    .layer("precipitation_x0")
                    .context("world has no precipitation_x0 layer (made before M5?)")?,
            ),
            rivers: grid_u8(
                world
                    .layer("river_order")
                    .context("world has no river_order layer (made before M5?)")?,
            ),
            biome: grid_u8(
                world
                    .layer("biome")
                    .context("world has no biome layer (made before M5?)")?,
            ),
            warp_x: Fbm::new(s("column-warp-x"), 800.0, 3, 0.5),
            warp_z: Fbm::new(s("column-warp-z"), 800.0, 3, 0.5),
            biome_warp_x: Fbm::new(s("biome-warp-x"), 250.0, 2, 0.5),
            biome_warp_z: Fbm::new(s("biome-warp-z"), 250.0, 2, 0.5),
            pluton: Fbm::new(s("column-pluton"), 1_600.0, 2, 0.5),
            strata: Fbm::new(s("column-strata"), 2_000.0, 2, 0.5),
            facies: Fbm::new(s("column-facies"), 4_000.0, 2, 0.5),
            strata_seed: s("column-beds"),
            bodies: BodyIndex::new(on_top_first(world.bodies()?, &cat)),
            ores,
        })
    }

    /// Biome code at `(x, z)`, read at the warped position like the settings.
    pub fn biome(&self, x: f64, z: f64) -> u8 {
        let wx = x + WARP_M * self.warp_x.sample(x, z);
        let wz = z + WARP_M * self.warp_z.sample(x, z);
        self.biome_warped(x, z, wx, wz)
    }

    /// [`Self::biome`] at `(x, z)` whose settings warp lands at `(wx, wz)`.
    fn biome_warped(&self, x: f64, z: f64, wx: f64, wz: f64) -> u8 {
        let b = &self.biome;
        let wx = wx + BIOME_WARP_M * self.biome_warp_x.sample(x, z);
        let wz = wz + BIOME_WARP_M * self.biome_warp_z.sample(x, z);
        let (i, j) = b.cell_of(wx, wz);
        *b.get(i as i64, j as i64)
    }

    /// Geology of the column at `(x, z)` (block centre).
    pub fn column(&self, x: f64, z: f64) -> ColumnGeo {
        self.column_with(x, z, true)
    }

    /// [`Self::column`] without its biome (left at the default), for callers that read the
    /// biome elsewhere.
    pub fn column_without_biome(&self, x: f64, z: f64) -> ColumnGeo {
        self.column_with(x, z, false)
    }

    fn column_with(&self, x: f64, z: f64, biome: bool) -> ColumnGeo {
        let wx = x + WARP_M * self.warp_x.sample(x, z);
        let wz = z + WARP_M * self.warp_z.sample(x, z);
        let near = |g: &PeriodicGrid<u8>, x: f64, z: f64| {
            let (i, j) = g.cell_of(x, z);
            *g.get(i as i64, j as i64)
        };
        let set = near(&self.setting, wx, wz);
        let oceanic = near(&self.crust, wx, wz) == 0;
        ColumnGeo {
            setting: set,
            oceanic,
            sediment: self.sediment.sample_bilinear(x, z).max(0.0) as f32,
            weathering: self.weathering.sample_bilinear(x, z).max(0.0) as f32,
            laterite: self.laterite.sample_bilinear(x, z) > 0.5,
            river: near(&self.rivers, x, z) > 0,
            biome: if biome {
                self.biome_warped(x, z, wx, wz)
            } else {
                0
            },
            climate: Default::default(),
            arid: self.precipitation.sample_bilinear(x, z) < ARID_MM,
        }
    }

    /// Bodies whose disc may reach the square `[x, x + size) × [z, z + size)`.
    pub fn bodies_near(&self, x: f64, z: f64, size: f64) -> Vec<Body> {
        self.bodies.near(x, z, size).into_iter().copied().collect()
    }

    pub fn bodies(&self) -> &[Body] {
        &self.bodies.bodies
    }

    /// Strata and facies noise of the column of `n`, sampled once.
    fn sediment_noise(&self, n: &mut RockNoise) -> (f64, f64) {
        let (x, z) = (n.x, n.z);
        *n.sediment
            .get_or_insert_with(|| (self.strata.sample(x, z), self.facies.sample(x, z)))
    }

    /// Whether the column of `n` is in a pluton, sampled once.
    fn in_pluton(&self, n: &mut RockNoise) -> bool {
        let (x, z) = (n.x, n.z);
        *n.pluton
            .get_or_insert_with(|| self.pluton.sample(x, z) > 0.25)
    }

    /// Far pluton noise of the column of `n` (granite of the upper crust), sampled once.
    fn pluton_far(&self, n: &mut RockNoise) -> f64 {
        let (x, z) = (n.x, n.z);
        *n.pluton_far
            .get_or_insert_with(|| self.pluton.sample(x + 50_000.0, z))
    }

    /// Sedimentary bed at height `y` of the column of `n`.
    pub fn sediment_block(
        &self,
        ids: &crate::block::ColumnIds,
        col: &ColumnGeo,
        n: &mut RockNoise,
        y: i32,
    ) -> BlockId {
        let (strata, facies) = self.sediment_noise(n);
        let yy = f64::from(y) + 8.0 * strata;
        let bed = (yy / BED_M).floor() as i64;
        let h = (mix64(self.strata_seed ^ bed as u64) >> 11) as f64 / (1u64 << 53) as f64;
        // Lateral facies changes: the same bed shifts lithology across a basin.
        let h = (h + 0.3 * (facies + 1.0)).fract();
        let marine = setting::oceanic(col.setting) || col.setting == setting::MARINE_SHELF;
        // Chalk: pelagic carbonate of shelves, raised on the platforms (older worlds: limestone).
        let chalk = ids.chalk.unwrap_or(ids.limestone);
        let table: &[(BlockId, f64)] = if marine {
            &[
                (ids.limestone, 0.3),
                (chalk, 0.45),
                (ids.clay, 0.9),
                (ids.sandstone, 1.0),
            ]
        } else if col.arid {
            &[
                (ids.sandstone, 0.35),
                (ids.clay, 0.65),
                (ids.limestone, 0.8),
                (ids.gypsum, 1.0),
            ]
        } else if col.setting == setting::PLATFORM {
            &[
                (ids.sandstone, 0.35),
                (ids.clay, 0.65),
                (ids.limestone, 0.85),
                (chalk, 1.0),
            ]
        } else {
            &[(ids.sandstone, 0.4), (ids.clay, 0.75), (ids.limestone, 1.0)]
        };
        table
            .iter()
            .find(|(_, c)| h < *c)
            .map_or(table[table.len() - 1].0, |(b, _)| *b)
    }

    /// Basement block at relative depth `u` (0 at the surface, 1 at the Moho) of the column
    /// of `n`.
    pub fn basement_block(
        &self,
        ids: &crate::block::ColumnIds,
        col: &ColumnGeo,
        n: &mut RockNoise,
        u: f64,
    ) -> BlockId {
        let lower = u > 2.0 / 3.0;
        let continental = |n: &mut RockNoise, granite_bias: f64| {
            if lower {
                ids.granulite
            } else if self.in_pluton(n) || self.pluton_far(n) > 0.6 - granite_bias {
                ids.granite
            } else {
                ids.gneiss
            }
        };
        match col.setting {
            setting::OCEAN | setting::RIDGE | setting::TRENCH => {
                if u < 0.3 {
                    ids.basalt
                } else {
                    ids.gabbro
                }
            }
            setting::OPHIOLITE => {
                if u < 0.15 {
                    ids.basalt
                } else if u < 0.4 {
                    ids.gabbro
                } else {
                    ids.peridotite
                }
            }
            setting::HOTSPOT => {
                if u < 0.45 {
                    ids.basalt
                } else if col.oceanic {
                    ids.gabbro
                } else {
                    continental(n, 0.0)
                }
            }
            setting::ARC => {
                if u < 0.3 {
                    ids.andesite
                } else if lower {
                    if col.oceanic {
                        ids.gabbro
                    } else {
                        ids.granulite
                    }
                } else {
                    ids.granite
                }
            }
            setting::OROGEN => {
                if lower {
                    ids.granulite
                } else if u > 0.15 && self.in_pluton(n) {
                    ids.granite
                } else if u < 0.35 {
                    ids.schist
                } else {
                    ids.gneiss
                }
            }
            setting::RIFT if u < 0.08 => ids.basalt,
            setting::CRATON => continental(n, 0.0),
            _ if col.oceanic => {
                if u < 0.3 {
                    ids.basalt
                } else {
                    ids.gabbro
                }
            }
            _ => continental(n, 0.2),
        }
    }

    /// The first body of `bodies` that contains block `(x, y, z)` (wrapped), a `rock` of class
    /// `class`, in a column whose top is `top`: `(body, ore of its type, mineralisation
    /// probability, height in the body)`.
    #[allow(clippy::too_many_arguments)]
    fn body_at<'a>(
        &'a self,
        bodies: &'a [Body],
        class: Class,
        rock: BlockId,
        x: i32,
        y: i32,
        z: i32,
        top: i32,
    ) -> impl Iterator<Item = (&'a Body, &'a Ore, f64, f64)> + 'a {
        let (fx, fy, fz) = (f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5);
        bodies.iter().filter_map(move |b| {
            if class == Class::Soil {
                return None;
            }
            let ore = self.ores.get(usize::from(b.kind))?;
            let host_ok = match ore.shape {
                Shape::Vein | Shape::Stock | Shape::Lens => {
                    matches!(class, Class::Weathered | Class::Sediment | Class::Basement)
                }
                Shape::Layer => match ore.host {
                    Host::Sediment => class == Class::Sediment,
                    Host::Basement => class == Class::Basement,
                },
                Shape::Blanket => class == Class::Weathered,
                Shape::Placer => matches!(class, Class::Loose | Class::Weathered | Class::Sediment),
                Shape::Trap => false,
            };
            if !host_ok || !(ore.host_rock.is_empty() || ore.host_rock.contains(&rock)) {
                return None;
            }
            let (p, rel) = if b.shape.follows_surface() {
                b.ore_at(fx, f64::from(y), fz, f64::from(top))?
            } else {
                let (lo, hi) = b.y_range();
                if fy < lo || fy > hi {
                    return None;
                }
                b.ore_at(fx, fy, fz, f64::from(top))?
            };
            Some((b, ore, p, rel))
        })
    }

    /// Deposit type of the first body containing the block, mineralised or not (sections).
    #[allow(clippy::too_many_arguments)]
    pub fn ore_kind(
        &self,
        bodies: &[Body],
        class: Class,
        rock: BlockId,
        x: i32,
        y: i32,
        z: i32,
        top: i32,
    ) -> Option<u16> {
        self.body_at(bodies, class, rock, x, y, z, top)
            .next()
            .map(|(b, ..)| b.kind)
    }

    /// Ore replacing `rock`, of class `class`, at block `(x, y, z)` (wrapped) of a column
    /// whose top is `top`, if a body of `bodies` mineralises it.
    #[allow(clippy::too_many_arguments)]
    pub fn ore(
        &self,
        bodies: &[Body],
        class: Class,
        rock: BlockId,
        x: i32,
        y: i32,
        z: i32,
        top: i32,
    ) -> Option<BlockId> {
        for (b, ore, p, rel) in self.body_at(bodies, class, rock, x, y, z, top) {
            if block_hash(b.seed, x, y, z) >= p {
                continue;
            }
            if let Some((cap, f)) = ore.cap
                && rel > 1.0 - 2.0 * f
            {
                return Some(cap);
            }
            let h = block_hash(b.seed ^ 0x9E37_79B9, x, y, z);
            return ore
                .blocks
                .iter()
                .find(|(_, c)| h < *c)
                .or(ore.blocks.last())
                .map(|(id, _)| *id);
        }
        None
    }
}

/// Wrapped integer block coordinate (hashes must agree on both sides of the seam).
#[inline]
pub fn wrap(v: i32) -> i32 {
    torus::wrap(v)
}
