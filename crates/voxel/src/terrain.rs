//! Surface height function shared by the chunks and the horizon (spec §5).
//!
//! `height = bicubic(elevation) + detail` (bicubic clamped to the surrounding cells, so steep
//! steps such as the edge of a sediment fan do not overshoot into islets). `elevation` is the eroded 32 m grid of M3 (one cell
//! per chunk at full resolution). The detail noise is the one of `nascio-worldgen`
//! ([`Detail`]): the fine grid already contains the octaves at least two cells wide, the chunks
//! add only the finer ones, with the amplitude of the context (ruggedness, coast, sea floor).
//! Every layer of a complete world is required: worlds made before M5 are refused.
//!
//! Water: the sea fills every column up to `y = 0`; a lake fills the columns of its cells up
//! to its surface.

use anyhow::{Context, ensure};
use nascio_core::fmath::DetMath;
use nascio_world::biomes;
use nascio_world::erosion::upsample::{Detail, baked_min_spacing, ruggedness};
use nascio_world::grid::PeriodicGrid;
use nascio_world::world_io::{Layer, World};

use crate::block::{BlockId, Blocks, srgb_to_linear};
use crate::climate::{ColumnClimate, SurfaceClimate};
use crate::geology::{Class, ColumnGeo, Geology, RockNoise};
use crate::rivers::{Gauge, RiverHit, RiverParams, Rivers};
use crate::vegetation::{BiomePlants, Part, Plant, PlantKind, cell_draw, default_vegetation};
pub use nascio_chunk::ChunkTint;
use nascio_core::consts::WORLD_SIZE;
use nascio_world::erosion::flow::NO_DIR;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// The ice biome (permanent snow, or a warmest month below its threshold) is looked up only
/// where the warmest month is below the glacier threshold plus this margin, °C.
const ICE_BIOME_MARGIN_C: f64 = 8.0;

/// Tunable parameters of chunk generation (`[voxel]` in TOML). The relief itself (detail noise
/// amplitudes) is configured by `[erosion]` in `world.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VoxelConfig {
    /// Above this elevation the surface is bare stone.
    pub grass_max_elevation_m: f64,
    /// Above this slope (m/m) the surface is bare stone.
    pub rock_slope: f64,
    /// Dirt thickness under grass.
    pub dirt_depth_m: i32,
    /// Surfaces between these heights are beaches (sand).
    pub beach_low_m: i32,
    pub beach_high_m: i32,
    /// Sand thickness on beaches, shallow sea floors and hot deserts.
    pub sand_depth_m: i32,
    /// Sea floors deeper than this have no sand (bare sediment), m.
    pub shelf_sand_m: i32,
    /// Gravel thickness on river beds.
    pub gravel_depth_m: i32,
    /// Rivers carry water from this width on, m.
    pub river_min_width_m: f64,
    /// Banks levelled with the river surface on each side of the channel, m.
    pub river_bank_m: f64,
    /// Smoothing passes of the river trace.
    pub river_smooth_passes: usize,
    /// River gauge: `game` (width and depth by Strahler order) or `physical` (world layers).
    pub river_gauge: Gauge,
    /// Game gauge: rivers carry water from this Strahler order of the drainage network on.
    pub river_min_order: u8,
    /// Game gauge: channel width by Strahler order (`river_min_order`, then +1…; the last
    /// for higher orders), m.
    pub river_order_width_m: Vec<f64>,
    /// Game gauge: depth at the channel centre by Strahler order, m.
    pub river_order_depth_m: Vec<f64>,
    /// Meander wavelength, in channel widths.
    pub river_meander_wavelength: f64,
    /// Meanders: largest heading deviation of the sine-generated curve, degrees.
    pub river_meander_angle_deg: f64,
    /// Meander amplitude at most this many channel widths.
    pub river_meander_max_amplitude: f64,
    /// Full meanders below this river gradient, none above `river_meander_steep` (m/m).
    pub river_meander_flat: f64,
    pub river_meander_steep: f64,
    /// Beyond the banks the ground stays within this slope of the water level (m/m)…
    pub river_valley_slope: f64,
    /// …up to this height above or below it, m.
    pub river_valley_height_m: f64,
    /// Weathered rock thins out on block slopes between these values (m/m).
    pub weathering_slope: [f64; 2],
    /// Weathered rock thins out between these surface heights, m…
    pub weathering_alt_m: [f64; 2],
    /// …down to this fraction of its thickness.
    pub weathering_alt_keep: f64,
    /// Sea and lake ice: the freezing threshold varies by up to this much, °C (noise)…
    pub water_ice_dither_c: f64,
    /// …over this wavelength, m.
    pub water_ice_dither_m: f64,
    /// Chunks generated below the lowest surface of a column.
    pub underground_chunks: i32,
    /// The crust keeps at least this thickness of stone above the magma, m.
    pub min_crust_m: i32,
    /// Glaciers where the warmest month at the surface is below this (and on the ice biome), °C.
    pub glacier_warm_c: f64,
    /// A glacier is snow-covered (accumulation) where its warmest month is below this, bare
    /// ice above, °C.
    pub glacier_snow_c: f64,
    /// Snow thickness on the accumulation zone of glaciers, m.
    pub glacier_snow_m: i32,
    /// Ice thickness on flat ground and at the bare-rock slope (`rock_slope`), m.
    pub ice_max_m: f64,
    pub ice_min_m: f64,
    /// Thin snow cover (`snow_layer`) off the glaciers where the warmest month is below this, °C.
    pub snow_layer_warm_c: f64,
    /// Sea and lake water carries permanent ice where its warmest month is below this, °C.
    pub water_ice_warm_c: f64,
    /// Thickness of sea and lake ice, m.
    pub water_ice_m: i32,
    /// Grass colours (sRGB) of a wet, a dry and a cold climate, blended by the climate.
    pub grass_wet: [u8; 3],
    pub grass_dry: [u8; 3],
    pub grass_cold: [u8; 3],
    /// Annual precipitation of fully wet and fully dry grass, mm.
    pub grass_wet_mm: f64,
    pub grass_dry_mm: f64,
    /// Annual mean temperature (at the surface) of fully warm and fully cold grass, °C.
    pub grass_warm_c: f64,
    pub grass_cold_c: f64,
    /// Soil of each biome (`[[voxel.soils]]`); biomes not listed use `temperate forest`.
    pub soils: Vec<BiomeSoil>,
    /// Trees grow where the warmest month at the ground is at least this (tree line), °C.
    pub tree_line_warm_c: f64,
    /// Plants grow on slopes up to this, m/m.
    pub plant_max_slope: f64,
    /// Side of the vegetation cells (at most one plant each), m; divides `WORLD_SIZE`, at
    /// least 3 (one block between trunks).
    pub plant_cell_m: i32,
    /// Plants of each biome (`[[voxel.vegetation]]`); biomes not listed have none.
    pub vegetation: Vec<BiomePlants>,
    /// Far ground (horizon tiles, far colours): the expected cover seen from above is
    /// multiplied by this (seen from afar, at a grazing angle, crowns hide more ground).
    pub far_canopy_gain: f64,
    /// Far ground: brightness of the crowns' colour (self-shadowing of the foliage).
    pub far_canopy_shade: f64,
    /// Far ground: horizon tiles rise by this share of the mean plant height × cover.
    pub far_canopy_relief: f64,
    /// Each column reads its biome this far (at most, triangular draw per axis) from its
    /// centre: biome boundaries become a mixed fringe (soils, plant density), m.
    pub biome_dither_m: f64,
    /// Modified blocks above which a chunk delta switches to a palette.
    pub delta_palette_above: usize,
}

/// Soil of one biome: `top` block over `under`, `depth_m` thick in all (on flat ground; it
/// thins with the slope, down to bare rock at `rock_slope`). Blocks are named as in the
/// registry; a name missing from an older world's registry falls back to grass or dirt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BiomeSoil {
    /// Biome name, as in `nascio_world::biomes::NAMES`.
    pub biome: String,
    pub top: String,
    pub under: String,
    pub depth_m: i32,
}

fn default_soils() -> Vec<BiomeSoil> {
    [
        // Land cells of the ocean biome (coastal strips above the sea): beach sand.
        ("ocean", "sand", "sand", 3),
        ("lake", "mud", "mud", 2),
        // Under the glacier ice, and bare where too steep for ice.
        ("ice cap / glacier", "gravel", "gravel", 1),
        ("tundra", "moss", "dirt", 1),
        ("boreal forest", "podzol", "dirt", 2),
        ("temperate forest", "grass", "dirt", 3),
        ("mediterranean", "grass", "dirt", 2),
        ("steppe", "grass", "dirt", 3),
        ("hot desert", "sand", "sand", 3),
        ("cold desert", "gravel", "gravel", 1),
        ("savanna", "grass", "red_soil", 3),
        ("tropical rainforest", "grass", "red_soil", 3),
        ("mangrove", "mud", "mud", 3),
        ("marsh", "mud", "mud", 2),
    ]
    .into_iter()
    .map(|(biome, top, under, depth_m)| BiomeSoil {
        biome: biome.into(),
        top: top.into(),
        under: under.into(),
        depth_m,
    })
    .collect()
}

/// A biome soil resolved to block ids.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Soil {
    top: BlockId,
    under: BlockId,
    depth: i32,
    class: Class,
}

/// Per-column part of [`Terrain::rock_block`] (see [`Terrain::rock_column`]).
#[derive(Clone, Copy, Debug)]
pub struct RockColumn {
    glacier: Option<(i32, i32)>,
    /// Soil, of depth 0 under a glacier.
    soil: Soil,
    /// Rock depths (below the soil) where the weathered layer and the sediments end.
    weathered: f64,
    sediment_end: f64,
    noise: RockNoise,
}

/// Soil table indexed by biome code.
fn resolve_soils(cfg: &VoxelConfig, blocks: &Blocks) -> [Soil; biomes::code::COUNT] {
    let ids = &blocks.ids;
    let resolve = |s: &BiomeSoil| {
        let top = blocks.id(&s.top).unwrap_or(ids.grass);
        let under = blocks.id(&s.under).unwrap_or(ids.dirt);
        let loose = [ids.sand, ids.gravel].contains(&under) || blocks.id("mud") == Some(under);
        Soil {
            top,
            under,
            depth: s.depth_m.max(0),
            class: if loose { Class::Loose } else { Class::Soil },
        }
    };
    let find = |name: &str| cfg.soils.iter().find(|s| s.biome == name);
    let fallback = find(biomes::NAMES[usize::from(biomes::code::TEMPERATE_FOREST)]).map_or(
        Soil {
            top: ids.grass,
            under: ids.dirt,
            depth: cfg.dirt_depth_m + 1,
            class: Class::Soil,
        },
        resolve,
    );
    std::array::from_fn(|b| find(biomes::NAMES[b]).map_or(fallback, resolve))
}

impl Default for VoxelConfig {
    fn default() -> Self {
        Self {
            grass_max_elevation_m: 2_200.0,
            rock_slope: 1.2,
            dirt_depth_m: 3,
            beach_low_m: -4,
            beach_high_m: 0,
            sand_depth_m: 3,
            shelf_sand_m: -40,
            gravel_depth_m: 2,
            river_min_width_m: 1.5,
            river_bank_m: 2.0,
            river_smooth_passes: 2,
            river_gauge: Gauge::Game,
            river_min_order: 4,
            river_order_width_m: vec![3.0, 6.0, 12.0, 24.0, 40.0],
            river_order_depth_m: vec![1.0, 1.5, 2.5, 4.0, 5.0],
            river_meander_wavelength: 12.0,
            river_meander_angle_deg: 70.0,
            river_meander_max_amplitude: 3.0,
            river_meander_flat: 0.015,
            river_meander_steep: 0.08,
            river_valley_slope: 0.6,
            river_valley_height_m: 16.0,
            weathering_slope: [0.4, 1.0],
            weathering_alt_m: [2_200.0, 3_200.0],
            weathering_alt_keep: 0.25,
            water_ice_dither_c: 1.5,
            water_ice_dither_m: 300.0,
            underground_chunks: 2,
            min_crust_m: 50,
            glacier_warm_c: 0.0,
            glacier_snow_c: -3.0,
            glacier_snow_m: 2,
            ice_max_m: 12.0,
            ice_min_m: 2.0,
            snow_layer_warm_c: 2.0,
            water_ice_warm_c: 0.5,
            water_ice_m: 2,
            grass_wet: [64, 134, 44],
            grass_dry: [168, 158, 82],
            grass_cold: [112, 124, 74],
            grass_wet_mm: 1_200.0,
            grass_dry_mm: 300.0,
            grass_warm_c: 10.0,
            grass_cold_c: -2.0,
            soils: default_soils(),
            tree_line_warm_c: 10.0,
            plant_max_slope: 0.7,
            plant_cell_m: 5,
            vegetation: default_vegetation(),
            far_canopy_gain: 3.0,
            far_canopy_shade: 0.5,
            far_canopy_relief: 1.0,
            biome_dither_m: 64.0,
            delta_palette_above: crate::delta::DEFAULT_PALETTE_ABOVE,
        }
    }
}

impl VoxelConfig {
    /// Parameters of the river water.
    pub fn river_params(&self) -> RiverParams {
        RiverParams {
            min_width_m: self.river_min_width_m,
            bank_m: self.river_bank_m,
            smooth_passes: self.river_smooth_passes,
            gauge: self.river_gauge,
            min_order: self.river_min_order,
            order_width_m: self.river_order_width_m.clone(),
            order_depth_m: self.river_order_depth_m.clone(),
            meander_wavelength: self.river_meander_wavelength,
            meander_angle_deg: self.river_meander_angle_deg,
            meander_max_amplitude: self.river_meander_max_amplitude,
            meander_flat: self.river_meander_flat,
            meander_steep: self.river_meander_steep,
            valley_slope: self.river_valley_slope,
            valley_height_m: self.river_valley_height_m,
        }
    }

    pub fn from_toml(s: &str) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            voxel: VoxelConfig,
        }
        Ok(toml::from_str::<File>(s)?.voxel)
    }
}

/// Plant table indexed by biome code: `(density, models)`.
fn resolve_plants(cfg: &VoxelConfig) -> [(f64, Vec<PlantKind>); biomes::code::COUNT] {
    std::array::from_fn(|b| {
        cfg.vegetation
            .iter()
            .find(|v| v.biome == biomes::NAMES[b])
            .map_or((0.0, Vec::new()), |v| {
                let kinds: Vec<_> = v
                    .plants
                    .iter()
                    .filter_map(|n| PlantKind::from_name(n))
                    .collect();
                (v.density.clamp(0.0, 1.0), kinds)
            })
    })
}

/// The surface of a world: everything chunk generation needs, immutable and thread-safe.
#[derive(Clone, Debug)]
pub struct Terrain {
    cfg: VoxelConfig,
    elevation: PeriodicGrid<f32>,
    /// Lake surface per elevation cell, `-∞` where there is no lake.
    lake_level: PeriodicGrid<f32>,
    /// 0 (flat) to 1 (most rugged), per tectonic cell.
    ruggedness: PeriodicGrid<f32>,
    detail: Detail,
    /// Octaves at least this wide are already in `elevation`.
    baked_spacing: f64,
    /// Depth of the Moho below sea level (m, positive downwards), per tectonic cell.
    moho: PeriodicGrid<f32>,
    /// Block registry of the world.
    blocks: Blocks,
    /// Rock column and ore bodies.
    geology: Geology,
    /// Soil by biome code.
    soils: [Soil; biomes::code::COUNT],
    /// Surface climate.
    climate: SurfaceClimate,
    /// River water.
    rivers: Rivers,
    /// Plants by biome code.
    plants: [(f64, Vec<PlantKind>); biomes::code::COUNT],
    /// Hash salt of the vegetation cells.
    plant_salt: u64,
    /// Noise of the sea and lake freezing threshold.
    ice_noise: nascio_world::noise::Fbm,
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Radius (cells) of the box blur applied three times to the Moho: the tectonic grid changes
/// crust type from one 160 m cell to the next, which would make vertical steps of up to 3 km
/// and needles of magma under single oceanic cells.
const MOHO_SMOOTH_CELLS: usize = 2;

/// Three periodic box blurs of radius `r` (about a Gaussian of σ ≈ 1.4 r cells).
fn smooth(g: &PeriodicGrid<f32>, r: usize) -> PeriodicGrid<f32> {
    let (w, h) = (g.width(), g.height());
    let mut v: Vec<f64> = g.data().iter().map(|&x| f64::from(x)).collect();
    let norm = 1.0 / (2 * r + 1) as f64;
    for _ in 0..3 {
        let mut tmp = vec![0.0; v.len()];
        for j in 0..h {
            for i in 0..w {
                let mut s = 0.0;
                for d in 0..=2 * r {
                    s += v[j * w + (i + w + d - r) % w];
                }
                tmp[j * w + i] = s * norm;
            }
        }
        for j in 0..h {
            for i in 0..w {
                let mut s = 0.0;
                for d in 0..=2 * r {
                    s += tmp[((j + h + d - r) % h) * w + i];
                }
                v[j * w + i] = s * norm;
            }
        }
    }
    PeriodicGrid::from_vec(w, h, g.cell_size(), v.iter().map(|&x| x as f32).collect())
}

fn grid_f32(layer: &Layer) -> PeriodicGrid<f32> {
    let data = (0..layer.width * layer.height)
        .map(|k| layer.data.get_f64(k) as f32)
        .collect();
    PeriodicGrid::from_vec(layer.width, layer.height, layer.cell_m, data)
}

/// Lake surface per elevation cell from the lake depth, `-∞` where there is no lake.
fn lake_level(elevation: &PeriodicGrid<f32>, depth: &PeriodicGrid<f32>) -> PeriodicGrid<f32> {
    PeriodicGrid::from_fn(depth.width(), depth.height(), depth.cell_size(), |i, j| {
        let d = *depth.get(i as i64, j as i64);
        if d > 0.0 {
            elevation.get(i as i64, j as i64) + d
        } else {
            f32::NEG_INFINITY
        }
    })
}

impl Terrain {
    /// Builds the terrain of a generated world.
    pub fn from_world(world: &World, cfg: VoxelConfig) -> anyhow::Result<Self> {
        let layer = |name: &str| {
            world
                .layer(name)
                .with_context(|| format!("world has no {name} layer (made before M5?)"))
        };
        let tectonic = grid_f32(layer("elevation_tectonic")?);
        let flags = grid_f32(layer("history_flags")?);
        let elevation = grid_f32(layer("elevation")?);
        let lake_level = lake_level(&elevation, &grid_f32(layer("lake_depth")?));
        let moho = smooth(&grid_f32(layer("moho_depth")?), MOHO_SMOOTH_CELLS);
        let blocks = Blocks::new(world.blocks()?)?;
        let geology = Geology::from_world(world, &blocks)?;
        let climate = SurfaceClimate::from_world(world, &elevation)?;
        let d = layer("flow_direction")?;
        ensure!(
            d.width == elevation.width() && d.height == elevation.height(),
            "flow_direction and elevation grids differ"
        );
        // Cells under permanent ice carry no river water.
        // (Land cells only; the costly biome lookup only in cold places.)
        let frozen: Vec<bool> = (0..elevation.width() * elevation.height())
            .into_par_iter()
            .map(|k| {
                let h = f64::from(elevation.data()[k]);
                if d.data.get_f64(k) as u8 == NO_DIR || h <= 0.0 {
                    return false;
                }
                let (x, z) = elevation.cell_center(k % elevation.width(), k / elevation.width());
                let tw = f64::from(climate.column(x, z).t_warm_sl) - climate.lapse * h;
                tw < cfg.glacier_warm_c
                    || (tw < cfg.glacier_warm_c + ICE_BIOME_MARGIN_C
                        && geology.biome(x, z) == biomes::code::ICE)
            })
            .collect();
        let dir = PeriodicGrid::from_vec(
            d.width,
            d.height,
            d.cell_m,
            (0..d.width * d.height)
                .map(|k| d.data.get_f64(k) as u8)
                .collect(),
        );
        let rivers = Rivers::new(
            &dir,
            &grid_f32(layer("river_width")?),
            &grid_f32(layer("river_depth")?),
            &elevation,
            &lake_level,
            &frozen,
            &cfg.river_params(),
        );
        let seed = world.config.seed;
        Ok(Self {
            ruggedness: ruggedness(
                &tectonic,
                |i, j| *flags.get(i, j) as u8,
                &world.config.erosion,
            ),
            baked_spacing: baked_min_spacing(elevation.cell_size()),
            detail: Detail::new(seed, &world.config.erosion),
            soils: resolve_soils(&cfg, &blocks),
            plants: resolve_plants(&cfg),
            plant_salt: nascio_world::rng::stage_u64(seed, "vegetation"),
            ice_noise: nascio_world::noise::Fbm::new(
                nascio_world::rng::stage_u64(seed, "water-ice-dither"),
                cfg.water_ice_dither_m,
                2,
                0.5,
            ),
            cfg,
            elevation,
            lake_level,
            moho,
            blocks,
            geology,
            climate,
            rivers,
        })
    }

    pub fn blocks(&self) -> &Blocks {
        &self.blocks
    }

    pub fn geology(&self) -> &Geology {
        &self.geology
    }

    /// Geology and climate of the column at `(x, z)` (defaults without them).
    pub fn column_geo(&self, x: f64, z: f64) -> ColumnGeo {
        let r = self.cfg.biome_dither_m;
        // With the dither, the biome is read below at another point.
        let g = &self.geology;
        let mut c = if r > 0.0 {
            g.column_without_biome(x, z)
        } else {
            g.column(x, z)
        };
        c.climate = self.climate(x, z);
        if r > 0.0 {
            // Hash of the block column (wrapped): the same draw in every chunk.
            let (i, k) = (
                crate::geology::wrap(x.floor() as i32),
                crate::geology::wrap(z.floor() as i32),
            );
            let h = nascio_world::rng::mix64(
                self.plant_salt ^ 0xB10E_D17E ^ (((i as u64) << 32) | k as u64),
            );
            let u = |s: u32| ((h >> s) & 0xFFFF) as f64 / 65_536.0;
            c.biome = g.biome(x + r * (u(0) + u(16) - 1.0), z + r * (u(32) + u(48) - 1.0));
        }
        c.river = matches!(
            self.river(x, z),
            Some(RiverHit::Channel { .. } | RiverHit::Bank { .. })
        );
        c
    }

    /// The river at `(x, z)` (column centre), if any: none where the ground is already under
    /// the sea or a lake (the trace ends at the shore).
    #[inline]
    pub fn river(&self, x: f64, z: f64) -> Option<RiverHit> {
        let hit = self.rivers.at(x, z)?;
        (self.height_lod(x, z, 0.0) >= self.water_level(x, z) + 0.5).then_some(hit)
    }

    /// Surface climate at `(x, z)`.
    pub fn climate(&self, x: f64, z: f64) -> ColumnClimate {
        let mut c = self.climate.column(x, z);
        // Dithered freezing threshold: ragged ice edges instead of an isotherm of the coarse
        // climate grid (a straight line where the climate depends on latitude only).
        c.t_warm_water += (self.cfg.water_ice_dither_c * self.ice_noise.sample(x, z)) as f32;
        c
    }

    fn lapse(&self) -> f64 {
        self.climate.lapse
    }

    /// Warmest-month temperature at height `y` of a column, °C.
    pub fn t_warm(&self, c: &ColumnClimate, y: i32) -> f64 {
        f64::from(c.t_warm_sl) - self.lapse() * f64::from(y.max(0))
    }

    /// Glacier of a column whose top solid block is at `top`: `(ice thickness, snow on top)`.
    fn glacier(&self, col: &ColumnGeo, top: i32, slope: f64) -> Option<(i32, i32)> {
        let c = &self.cfg;
        self.blocks.ids.ice?;
        if top <= c.beach_high_m || slope > c.rock_slope {
            return None;
        }
        let tw = self.t_warm(&col.climate, top);
        let ice_biome = col.biome == biomes::code::ICE;
        if !(tw < c.glacier_warm_c || ice_biome) {
            return None;
        }
        let ice = (c.ice_max_m + (c.ice_min_m - c.ice_max_m) * slope / c.rock_slope).round() as i32;
        let ice = ice.max(1);
        let snow = if self.blocks.ids.snow.is_some() && tw < c.glacier_snow_c {
            c.glacier_snow_m.clamp(0, ice)
        } else {
            0
        };
        Some((ice, snow))
    }

    /// Thin snow cover in the air cell above the top block `top` of a column, if any.
    pub fn snow_layer(&self, col: &ColumnGeo, top: i32, slope: f64) -> Option<BlockId> {
        let id = self.blocks.ids.snow_layer?;
        if self.glacier(col, top, slope).is_some() {
            return None;
        }
        (self.t_warm(&col.climate, top + 1) < self.cfg.snow_layer_warm_c).then_some(id)
    }

    /// Whether the water (sea, lake, river) of a column carries permanent ice: its warmest month,
    /// dithered, is below `water_ice_warm_c`.
    pub fn water_frozen(&self, c: &ColumnClimate) -> bool {
        self.blocks.ids.ice.is_some() && f64::from(c.t_warm_water) < self.cfg.water_ice_warm_c
    }

    /// Block at height `y` in the water of a column (`top < y < water_top`): water, or ice in
    /// its top `water_ice_m` blocks where it freezes.
    #[inline]
    pub fn water_block(&self, col: &ColumnGeo, y: i32, water_top: i32) -> BlockId {
        let ids = &self.blocks.ids;
        match ids.ice {
            Some(ice)
                if y >= water_top - self.cfg.water_ice_m && self.water_frozen(&col.climate) =>
            {
                ice
            }
            _ => ids.water,
        }
    }

    /// Grass colour (linear RGB) at `(x, z)` whose surface is at height `h`.
    pub fn grass_color(&self, x: f64, z: f64, h: f64) -> [f32; 3] {
        let c = &self.cfg;
        let k = self.climate(x, z);
        let t = f64::from(k.t_mean_sl) - self.lapse() * h.max(0.0);
        let dry = smoothstep(c.grass_wet_mm, c.grass_dry_mm, f64::from(k.precip)) as f32;
        let cold = smoothstep(c.grass_warm_c, c.grass_cold_c, t) as f32;
        let lin = |rgb: [u8; 3]| rgb.map(|v| srgb_to_linear(f32::from(v) / 255.0));
        let (wet, dryc, coldc) = (lin(c.grass_wet), lin(c.grass_dry), lin(c.grass_cold));
        std::array::from_fn(|i| {
            let base = wet[i] + (dryc[i] - wet[i]) * dry;
            base + (coldc[i] - base) * cold
        })
    }

    /// Display colour (linear RGB) of `block` at the top of column `(x, z)` of height `h`:
    /// its registry colour, the climate tint for grass.
    pub fn surface_color(&self, block: BlockId, x: f64, z: f64, h: f64) -> [f32; 3] {
        if block == self.blocks.ids.grass {
            self.grass_color(x, z, h)
        } else {
            self.blocks.linear(block)
        }
    }

    /// Grass tint of chunk column `(cx, cz)` for [`crate::mesh_chunk`].
    pub fn chunk_tint(&self, cx: i32, cz: i32) -> ChunkTint {
        let size = f64::from(nascio_core::consts::CHUNK_SIZE);
        let corner = |dx: f64, dz: f64| {
            let x = (f64::from(cx) + dx) * size;
            let z = (f64::from(cz) + dz) * size;
            self.grass_color(x, z, self.height(x, z))
        };
        ChunkTint {
            grass: Some(self.blocks.ids.grass),
            corners: [
                corner(0.0, 0.0),
                corner(1.0, 0.0),
                corner(0.0, 1.0),
                corner(1.0, 1.0),
            ],
            water_depth: [u8::MAX; 1024],
        }
    }

    /// First magma block of a column whose top solid block is at `top` (stone above it): the
    /// Moho, kept at least `min_crust_m` below the surface.
    pub fn moho_y(&self, x: f64, z: f64, top: i32) -> i32 {
        let y = (-self.moho.sample_bilinear(x, z)).floor() as i32;
        y.min(top - self.cfg.min_crust_m)
    }

    pub fn config(&self) -> &VoxelConfig {
        &self.cfg
    }

    /// Spacing of the finest detail octave added by the chunks.
    pub fn finest_spacing(&self) -> f64 {
        self.detail
            .spacings()
            .filter(|&s| s < self.baked_spacing)
            .last()
            .unwrap_or(f64::INFINITY)
    }

    /// Surface height at a world position, full detail, river channels and banks included.
    #[inline]
    pub fn height(&self, x: f64, z: f64) -> f64 {
        let h = self.height_lod(x, z, 0.0);
        match self.river(x, z) {
            Some(RiverHit::Channel { bed, .. }) => h.min(f64::from(bed) + 0.5),
            Some(RiverHit::Bank { top }) => f64::from(top) + 0.5,
            Some(RiverHit::Valley { top, d }) => {
                let (b, s) = (f64::from(top) + 0.5, self.cfg.river_valley_slope);
                h.clamp(b - s * d, b + s * d)
            }
            None => h,
        }
    }

    /// Macro elevation (eroded grid), without the chunk detail.
    #[inline]
    pub fn base_height(&self, x: f64, z: f64) -> f64 {
        self.elevation.sample_bicubic_monotone(x, z)
    }

    /// Surface height keeping only the octaves at least twice as wide as `sample_spacing`
    /// (for coarse horizon meshes, which would alias finer detail).
    pub fn height_lod(&self, x: f64, z: f64, sample_spacing: f64) -> f64 {
        let e = self.elevation.sample_bicubic_monotone(x, z);
        let min = 2.0 * sample_spacing;
        if min >= self.baked_spacing {
            return e;
        }
        let r = self.ruggedness.sample_bilinear(x, z);
        e + self.detail.value(x, z, e, r, min, self.baked_spacing)
    }

    /// Water surface at a world position: the surface of a lake whose cells surround the point
    /// (any of the four nearest cell centres), otherwise sea level.
    pub fn water_level(&self, x: f64, z: f64) -> f64 {
        let l = &self.lake_level;
        let c = l.cell_size();
        let gx = (nascio_core::torus::wrap_f64(x) / c - 0.5).floor() as i64;
        let gz = (nascio_core::torus::wrap_f64(z) / c - 0.5).floor() as i64;
        let mut level = f32::NEG_INFINITY;
        for (di, dj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            level = level.max(*l.get(gx + di, gz + dj));
        }
        if level.is_finite() {
            f64::from(level)
        } else {
            0.0
        }
    }

    /// Highest water block + 1 of a column: water fills `top < y < water_top` (sea, lakes and
    /// river channels).
    #[inline]
    pub fn water_top(&self, x: f64, z: f64) -> i32 {
        let still = self.water_level(x, z).round() as i32;
        match self.river(x, z) {
            Some(RiverHit::Channel { water_top, .. }) => still.max(water_top),
            _ => still,
        }
    }

    /// Soil of a column whose top solid block is at `top`; thickness 0 for bare rock.
    fn soil(&self, col: &ColumnGeo, top: i32, slope: f64) -> Soil {
        let c = &self.cfg;
        let ids = &self.blocks.ids;
        let sand = |depth| Soil {
            top: ids.sand,
            under: ids.sand,
            depth,
            class: Class::Loose,
        };
        if top <= c.beach_high_m {
            // Beaches and shallow sea floors: quartz sand; deep sea floors: bare sediment.
            return if top > c.shelf_sand_m {
                sand(c.sand_depth_m)
            } else {
                sand(0)
            };
        }
        if col.river {
            return Soil {
                top: ids.gravel,
                under: ids.gravel,
                depth: c.gravel_depth_m,
                class: Class::Loose,
            };
        }
        let biome = col.biome;
        let s = self.soils[usize::from(biome).min(biomes::code::COUNT - 1)];
        if slope > c.rock_slope || f64::from(top) > c.grass_max_elevation_m {
            return Soil { depth: 0, ..s };
        }
        // Thinner on slopes, at least one block below the bare-rock slope.
        let depth = (f64::from(s.depth) * (1.0 - slope / c.rock_slope)).ceil() as i32;
        Soil {
            depth: depth.clamp(s.depth.min(1), s.depth),
            ..s
        }
    }

    /// Block at depth `depth ≥ 0` below the top `top` of column `(x, z)` (world block
    /// coordinates), whose Moho is at `moho`, without ore. Also returns its class.
    #[allow(clippy::too_many_arguments)]
    pub fn rock_block(
        &self,
        col: &ColumnGeo,
        x: i32,
        z: i32,
        top: i32,
        slope: f64,
        moho: i32,
        depth: i32,
    ) -> (BlockId, Class) {
        let mut rc = self.rock_column(col, x, z, top, slope);
        self.rock_at(&mut rc, col, top, moho, depth)
    }

    /// The part of [`Self::rock_block`] shared by all blocks of column `(x, z)` (world block
    /// coordinates): glacier, soil, weathered and sedimentary thicknesses, rock noise.
    pub fn rock_column(&self, col: &ColumnGeo, x: i32, z: i32, top: i32, slope: f64) -> RockColumn {
        let mut soil = self.soil(col, top, slope);
        let glacier = self.glacier(col, top, slope);
        if glacier.is_some() {
            // The soil under ice and snow is gone.
            soil.depth = 0;
        }
        let (x, z) = (crate::geology::wrap(x), crate::geology::wrap(z));
        let noise = RockNoise::new(f64::from(x) + 0.5, f64::from(z) + 0.5);
        // Weathered rock thins out on steep blocks and at altitude; the dither turns the
        // contour of the coarse layer into a ragged fringe (audit M7, anomaly c).
        let c = &self.cfg;
        let thin = (1.0 - smoothstep(c.weathering_slope[0], c.weathering_slope[1], slope))
            * (1.0
                - (1.0 - c.weathering_alt_keep)
                    * smoothstep(c.weathering_alt_m[0], c.weathering_alt_m[1], f64::from(top)));
        let u = (nascio_world::rng::mix64(
            self.plant_salt ^ 0x3EA7_4E11 ^ ((u64::from(x as u32) << 32) | u64::from(z as u32)),
        ) >> 11) as f64
            / (1u64 << 53) as f64;
        let weathering = f64::from(col.weathering) * thin + u;
        RockColumn {
            glacier,
            soil,
            weathered: weathering.floor(),
            sediment_end: (weathering + f64::from(col.sediment)).floor(),
            noise,
        }
    }

    /// Block at depth `depth ≥ 0` of the column of `rc` (see [`Self::rock_block`]).
    pub fn rock_at(
        &self,
        rc: &mut RockColumn,
        col: &ColumnGeo,
        top: i32,
        moho: i32,
        depth: i32,
    ) -> (BlockId, Class) {
        let ids = &self.blocks.ids;
        if let Some((ice, snow)) = rc.glacier {
            // Ice and snow hold no ore (class `Soil`).
            if depth < snow
                && let Some(b) = ids.snow
            {
                return (b, Class::Soil);
            }
            if depth < ice
                && let Some(b) = ids.ice
            {
                return (b, Class::Soil);
            }
        }
        let soil = rc.soil;
        if depth < soil.depth {
            return (if depth == 0 { soil.top } else { soil.under }, soil.class);
        }
        let g = &self.geology;
        let r = f64::from(depth - soil.depth);
        if r < rc.weathered {
            let b = if col.laterite {
                ids.laterite
            } else {
                ids.regolith
            };
            return (b, Class::Weathered);
        }
        if r < rc.sediment_end {
            return (
                g.sediment_block(ids, col, &mut rc.noise, top - depth),
                Class::Sediment,
            );
        }
        let crust = f64::from((top - moho).max(1));
        let u = f64::from(depth) / crust;
        (
            g.basement_block(ids, col, &mut rc.noise, u),
            Class::Basement,
        )
    }

    fn plant_cell(&self) -> i32 {
        let c = self.cfg.plant_cell_m;
        if c >= 3 && WORLD_SIZE % c == 0 { c } else { 5 }
    }

    /// Block of a plant part (`None` in registries without vegetation).
    #[inline]
    pub fn part_block(&self, part: Part) -> Option<BlockId> {
        let ids = &self.blocks.ids;
        match part {
            Part::Log => ids.spruce_log,
            Part::Leaves => ids.spruce_leaves,
            Part::Cactus => ids.cactus,
        }
    }

    /// Plants whose trunk is in the block range `x0..=x1`, `z0..=z1` (unwrapped; the plants
    /// come back in the same frame), in a fixed order.
    pub fn plants_in(&self, x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<Plant> {
        let mut out = Vec::new();
        if self.blocks.ids.spruce_log.is_none() {
            return out;
        }
        let cell = self.plant_cell();
        let n = i64::from(WORLD_SIZE / cell);
        let max_density = self.plants.iter().map(|p| p.0).fold(0.0, f64::max);
        for cj in z0.div_euclid(cell)..=z1.div_euclid(cell) {
            for ci in x0.div_euclid(cell)..=x1.div_euclid(cell) {
                let (ox, oz, u, pick) = cell_draw(
                    self.plant_salt,
                    i64::from(ci).rem_euclid(n),
                    i64::from(cj).rem_euclid(n),
                    cell,
                );
                let (x, z) = (ci * cell + ox, cj * cell + oz);
                if u >= max_density || !(x0..=x1).contains(&x) || !(z0..=z1).contains(&z) {
                    continue;
                }
                if let Some(p) = self.plant_at(x, z, u, pick) {
                    out.push(p);
                }
            }
        }
        out
    }

    /// The plant drawn at column `(x, z)`, if its biome and ground allow it.
    fn plant_at(&self, x: i32, z: i32, u: f64, pick: f64) -> Option<Plant> {
        let (fx, fz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
        let col = self.column_geo(fx, fz);
        let biome = col.biome;
        let (density, kinds) = &self.plants[usize::from(biome).min(biomes::code::COUNT - 1)];
        if u >= *density || kinds.is_empty() || col.river {
            return None;
        }
        let kind = kinds[((pick * kinds.len() as f64) as usize).min(kinds.len() - 1)];
        let top = self.height(fx, fz).floor() as i32;
        if top <= self.cfg.beach_high_m || self.water_top(fx, fz) > top + 1 {
            return None;
        }
        // Same slope as the chunk columns.
        let slope = ((self.height(fx + 1.0, fz) - self.height(fx - 1.0, fz)) * 0.5)
            .det_hypot((self.height(fx, fz + 1.0) - self.height(fx, fz - 1.0)) * 0.5);
        if slope > self.cfg.plant_max_slope || self.glacier(&col, top, slope).is_some() {
            return None;
        }
        let soil = self.soil(&col, top, slope);
        let ids = &self.blocks.ids;
        let ok = soil.depth > 0
            && if kind.is_tree() {
                ![ids.sand, ids.gravel].contains(&soil.top)
                    && self.t_warm(&col.climate, top) >= self.cfg.tree_line_warm_c
            } else {
                soil.top == ids.sand
            };
        ok.then_some(Plant {
            x,
            z,
            base: top + 1,
            kind,
        })
    }

    /// Top block of column `(x, z)` (world block coordinates) without ore, snow cover
    /// included (horizon colours).
    pub fn top_block(&self, x: i32, z: i32, top: i32, slope: f64) -> BlockId {
        let col = self.column_geo(f64::from(x) + 0.5, f64::from(z) + 0.5);
        self.top_block_of(&col, x, z, top, slope)
    }

    fn top_block_of(&self, col: &ColumnGeo, x: i32, z: i32, top: i32, slope: f64) -> BlockId {
        if let Some(l) = self.snow_layer(col, top, slope) {
            return l;
        }
        self.rock_block(col, x, z, top, slope, i32::MIN / 2, 0).0
    }

    /// The far ground at `(x, z)` (world metres), surface height `h`, top block at `top`:
    /// colour of the top block mixed with the plants' crowns by their expected cover, and how
    /// far the horizon tiles rise over the crowns, m.
    pub fn far_ground(&self, x: f64, z: f64, top: i32, h: f64, slope: f64) -> ([f32; 3], f64) {
        let (xi, zi) = (
            nascio_core::torus::wrap(x.floor() as i32),
            nascio_core::torus::wrap(z.floor() as i32),
        );
        let col = self.column_geo(f64::from(xi) + 0.5, f64::from(zi) + 0.5);
        let ground = self.surface_color(self.top_block_of(&col, xi, zi, top, slope), x, z, h);
        let (cover, crown, height) = self.canopy(&col, x, z, h.floor() as i32, slope);
        let cover = (cover * self.cfg.far_canopy_gain).min(1.0);
        let (c, shade) = (cover as f32, self.cfg.far_canopy_shade as f32);
        (
            [0, 1, 2].map(|i| ground[i] * (1.0 - c) + crown[i] * shade * c),
            self.cfg.far_canopy_relief * height * cover,
        )
    }

    /// Expected plant cover seen from above at `(x, z)`, ground top block at `top`, without
    /// drawing the plants: `(cover in [0, 1], mean crown colour, mean plant height)`. The same
    /// ground rules as [`Self::plant_at`]; the models of a biome are drawn evenly.
    fn canopy(
        &self,
        col: &ColumnGeo,
        x: f64,
        z: f64,
        top: i32,
        slope: f64,
    ) -> (f64, [f32; 3], f64) {
        let none = (0.0, [0.0; 3], 0.0);
        let (density, kinds) = &self.plants[usize::from(col.biome).min(biomes::code::COUNT - 1)];
        if *density <= 0.0 || kinds.is_empty() || col.river || self.blocks.ids.spruce_log.is_none()
        {
            return none;
        }
        if top <= self.cfg.beach_high_m || self.water_top(x, z) > top + 1 {
            return none;
        }
        if slope > self.cfg.plant_max_slope || self.glacier(col, top, slope).is_some() {
            return none;
        }
        let soil = self.soil(col, top, slope);
        if soil.depth == 0 {
            return none;
        }
        let ids = &self.blocks.ids;
        let trees_ok = ![ids.sand, ids.gravel].contains(&soil.top)
            && self.t_warm(&col.climate, top) >= self.cfg.tree_line_warm_c;
        let (mut area, mut color, mut height) = (0.0, [0.0f32; 3], 0.0);
        for &k in kinds {
            let ok = if k.is_tree() {
                trees_ok
            } else {
                soil.top == ids.sand
            };
            let Some(block) = self.part_block(k.crown_part()).filter(|_| ok) else {
                continue;
            };
            let (a, h) = k.footprint();
            let a = a as f64;
            let rgb = self.blocks.linear(block);
            area += a;
            height += a * f64::from(h);
            for i in 0..3 {
                color[i] += a as f32 * rgb[i];
            }
        }
        if area <= 0.0 {
            return none;
        }
        let cell = f64::from(self.plant_cell());
        (
            (density * area / kinds.len() as f64 / (cell * cell)).min(1.0),
            color.map(|c| c / area as f32),
            height / area,
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use nascio_core::consts::WORLD_SIZE_F64;
    use proptest::prelude::*;

    /// A small synthetic world: a mountain ridge crossing both seams, an ocean, and a lake
    /// straddling the x seam; a temperate climate everywhere, continental crust under the land
    /// and oceanic crust under the sea, no river and no ore body.
    pub fn test_terrain() -> Terrain {
        static T: std::sync::OnceLock<Terrain> = std::sync::OnceLock::new();
        T.get_or_init(|| Terrain::from_world(&test_world(), VoxelConfig::default()).unwrap())
            .clone()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Two trunks are never adjacent (Chebyshev distance >= 2), also across the seam.
        #[test]
        fn trunks_are_at_least_one_block_apart(
            x0 in prop_oneof![-40i32..40, (WORLD_SIZE - 40)..WORLD_SIZE],
            z0 in prop_oneof![-40i32..40, (WORLD_SIZE - 40)..WORLD_SIZE],
        ) {
            let t = test_terrain();
            let plants = t.plants_in(x0, z0, x0 + 63, z0 + 63);
            for (i, a) in plants.iter().enumerate() {
                for b in &plants[i + 1..] {
                    let dx = nascio_core::torus::delta(a.x, b.x);
                    let dz = nascio_core::torus::delta(a.z, b.z);
                    prop_assert!(dx.abs().max(dz.abs()) >= 2, "{a:?} {b:?}");
                }
            }
        }
    }

    pub fn test_world() -> World {
        use nascio_world::config::{Scale, WorldConfig};
        use nascio_world::geology::setting;
        use nascio_world::tectonics::{CONTINENTAL, OCEANIC};

        let n = 125;
        let cell = WORLD_SIZE_F64 / n as f64;
        let wave = |i: usize, j: usize, n: usize| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            let b = std::f64::consts::TAU * j as f64 / n as f64;
            (1_200.0 * a.det_cos() + 900.0 * b.det_sin() + 300.0 * (a + b).det_cos()) as f32
        };
        let m = 5 * n;
        let fine_cell = cell / 5.0;
        let tect = |f: &dyn Fn(usize, usize) -> f32| PeriodicGrid::from_fn(n, n, cell, f);
        let fine = |f: &dyn Fn(usize, usize) -> f32| PeriodicGrid::from_fn(m, m, fine_cell, f);
        let land = |i: usize, j: usize| wave(i, j, n) > 0.0;
        let f32s = |name: &str, g: PeriodicGrid<f32>| Layer::from_f32(name, "", &g);
        let u8s = |name: &str, g: PeriodicGrid<u8>| Layer::from_u8(name, "", &g);
        let layers =
            vec![
                f32s("elevation_tectonic", tect(&|i, j| wave(i, j, n))),
                f32s("history_flags", tect(&|i, _| if i < 5 { 1.0 } else { 0.0 })),
                // Moho 700 m under the sea floor, 3 500 m under the land, varying across both seams.
                f32s(
                    "moho_depth",
                    tect(&|i, j| {
                        let e = f64::from(wave(i, j, n));
                        (if e < 0.0 { 700.0 - e } else { 3_500.0 + e }) as f32
                    }),
                ),
                u8s(
                    "geo_context",
                    PeriodicGrid::from_fn(n, n, cell, |i, j| {
                        if land(i, j) {
                            setting::CRATON
                        } else {
                            setting::OCEAN
                        }
                    }),
                ),
                u8s(
                    "crust_type",
                    PeriodicGrid::from_fn(n, n, cell, |i, j| {
                        if land(i, j) { CONTINENTAL } else { OCEANIC }
                    }),
                ),
                f32s("sediment_thickness", tect(&|_, _| 0.0)),
                f32s("weathering_depth", tect(&|_, _| 0.0)),
                f32s("laterite", tect(&|_, _| 0.0)),
                f32s("temperature_x0", tect(&|_, _| 12.0)),
                f32s("temperature_a1", tect(&|_, _| 8.0)),
                f32s("temperature_p1", tect(&|_, _| 0.0)),
                f32s("temperature_a2", tect(&|_, _| 0.0)),
                f32s("temperature_p2", tect(&|_, _| 0.0)),
                f32s("precipitation_x0", tect(&|_, _| 800.0)),
                u8s(
                    "biome",
                    PeriodicGrid::from_fn(n, n, cell, |_, _| {
                        nascio_world::biomes::code::TEMPERATE_FOREST
                    }),
                ),
                f32s("elevation", fine(&|i, j| wave(i, j, m))),
                f32s(
                    "lake_depth",
                    fine(&|i, j| {
                        if (i < 3 || i > m - 4) && (300..306).contains(&j) {
                            5.0
                        } else {
                            0.0
                        }
                    }),
                ),
                u8s(
                    "flow_direction",
                    PeriodicGrid::from_fn(m, m, fine_cell, |_, _| NO_DIR),
                ),
                f32s("river_width", fine(&|_, _| 0.0)),
                f32s("river_depth", fine(&|_, _| 0.0)),
                u8s(
                    "river_order",
                    PeriodicGrid::from_fn(m, m, fine_cell, |_, _| 0),
                ),
            ];
        let catalog = nascio_world::deposits::Catalog::default_catalog();
        let registry = nascio_world::blocks::registry_with_catalog(&catalog).unwrap();
        let mut files = std::collections::BTreeMap::new();
        files.insert(
            "deposits.toml".to_string(),
            nascio_world::deposits::DEFAULT_CATALOG.as_bytes().to_vec(),
        );
        files.insert(
            "blocks.toml".to_string(),
            registry.to_toml().unwrap().into_bytes(),
        );
        World {
            config: WorldConfig::new(42, Scale::Fifth),
            layers,
            files,
        }
    }

    #[test]
    fn legacy_worlds_are_refused() {
        let mut w = test_world();
        w.layers.retain(|l| l.name != "geo_context");
        let e = Terrain::from_world(&w, VoxelConfig::default())
            .err()
            .unwrap();
        assert!(e.to_string().contains("geo_context"), "{e}");
        let mut w = test_world();
        w.layers.retain(|l| l.name != "temperature_a2");
        assert!(Terrain::from_world(&w, VoxelConfig::default()).is_err());
    }

    #[test]
    fn deterministic() {
        let a = test_terrain();
        let b = test_terrain();
        for k in 0..100 {
            let x = f64::from(k) * 997.3;
            assert_eq!(
                a.height(x, 3.0 * x).to_bits(),
                b.height(x, 3.0 * x).to_bits()
            );
        }
    }

    #[test]
    fn continuous_across_both_seams() {
        let t = test_terrain();
        for k in 0..200 {
            let v = f64::from(k) * 499.0;
            assert!((t.height(WORLD_SIZE_F64 - 1e-3, v) - t.height(0.0, v)).abs() < 0.05);
            assert!((t.height(v, WORLD_SIZE_F64 - 1e-3) - t.height(v, 0.0)).abs() < 0.05);
        }
    }

    #[test]
    fn chunks_add_only_unbaked_octaves() {
        let t = test_terrain();
        // Fine grid of 160 m: octaves of 320 m and more are baked.
        assert!(t.finest_spacing() >= 16.0 && t.finest_spacing() < 320.0);
        let (mut fine, mut coarse) = (0.0, 0.0);
        for k in 0..2_000 {
            let x = f64::from(k) * 4.0;
            let d = t.height(x + 4.0, 500.0) - t.height(x, 500.0);
            fine += d * d;
            let d = t.height_lod(x + 4.0, 500.0, 512.0) - t.height_lod(x, 500.0, 512.0);
            coarse += d * d;
            assert_eq!(t.height(x, 500.0), t.height_lod(x, 500.0, 8.0));
            // Coarse LOD = the grid itself.
            assert_eq!(t.height_lod(x, 500.0, 512.0), t.base_height(x, 500.0));
        }
        assert!(coarse < fine);
    }

    #[test]
    fn lake_water_across_the_seam() {
        let t = test_terrain();
        let z = 303.0 * 160.0;
        let level = t.water_level(10.0, z);
        assert!(level > 0.0);
        assert_eq!(t.water_level(WORLD_SIZE_F64 - 10.0, z), level);
        assert_eq!(t.water_level(50_000.0, z), 0.0);
    }

    #[test]
    fn config_round_trip() {
        let c = VoxelConfig::default();
        let s = format!("[voxel]\n{}", toml::to_string(&c).unwrap());
        assert_eq!(VoxelConfig::from_toml(&s).unwrap(), c);
        assert_eq!(VoxelConfig::from_toml("").unwrap(), c);
    }

    proptest! {
        #[test]
        fn periodic(x in -2e5f64..2e5, z in -2e5f64..2e5, kx in -2i32..2, kz in -2i32..2) {
            let t = test_terrain();
            let a = t.height(x, z);
            let b = t.height(x + f64::from(kx) * WORLD_SIZE_F64, z + f64::from(kz) * WORLD_SIZE_F64);
            prop_assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn far_ground_cover_is_bounded() {
        use nascio_core::torus::wrap;
        let t = test_terrain();
        let cfg0 = VoxelConfig {
            far_canopy_gain: 0.0,
            ..Default::default()
        };
        let t0 = Terrain::from_world(&test_world(), cfg0).unwrap();
        let relief = VoxelConfig::default().far_canopy_relief;
        let mut raised = 0;
        let mut sampled = 0;
        for i in 0..40 {
            for j in 0..40 {
                let fx = |k: i32| match k {
                    0 => 0.0,
                    39 => WORLD_SIZE_F64 - 0.5,
                    k => f64::from(k) * WORLD_SIZE_F64 / 40.0 + 13.3,
                };
                let (x, z) = (fx(i), fx(j));
                let h = t.height(x, z);
                if h < t.water_level(x, z) {
                    continue;
                }
                sampled += 1;
                let top = h.floor() as i32;
                let (c, rise) = t.far_ground(x, z, top, h, 0.0);
                for v in c {
                    assert!(
                        v.is_finite() && (0.0..=1.5).contains(&v),
                        "{c:?} at {x} {z}"
                    );
                }
                assert!(
                    (0.0..=9.0 * relief).contains(&rise),
                    "rise {rise} at {x} {z}"
                );
                if rise > 0.0 {
                    raised += 1;
                }
                let (c0, rise0) = t0.far_ground(x, z, top, h, 0.0);
                let (xi, zi) = (wrap(x.floor() as i32), wrap(z.floor() as i32));
                let col = t0.column_geo(f64::from(xi) + 0.5, f64::from(zi) + 0.5);
                let block = t0.top_block_of(&col, xi, zi, top, 0.0);
                assert_eq!(c0, t0.surface_color(block, x, z, h));
                assert_eq!(rise0, 0.0);
            }
        }
        assert!(sampled > 0, "no ground sampled");
        assert!(raised > 0, "no sampled point has plants");
    }
}
