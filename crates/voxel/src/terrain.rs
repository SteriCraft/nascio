//! Surface height function shared by the chunks and the horizon (spec §5).
//!
//! `height = bicubic(elevation) + detail` (bicubic clamped to the surrounding cells, so steep
//! steps such as the edge of a sediment fan do not overshoot into islets). `elevation` is the eroded 32 m grid of M3 (one cell
//! per chunk at full resolution). The detail noise is the one of `nascio-worldgen`
//! ([`Detail`]): the fine grid already contains the octaves at least two cells wide, the chunks
//! add only the finer ones, with the amplitude of the context (ruggedness, coast, sea floor).
//! Worlds made before M3 have only `elevation_tectonic`: it is used instead, with every octave.
//!
//! Water: the sea fills every column up to `y = 0`; a lake fills the columns of its cells up
//! to its surface.

use anyhow::Context;
use nascio_worldgen::biomes;
use nascio_worldgen::erosion::ErosionConfig;
use nascio_worldgen::erosion::upsample::{Detail, baked_min_spacing, ruggedness};
use nascio_worldgen::grid::PeriodicGrid;
use nascio_worldgen::world_io::{Layer, World};

use crate::block::{BlockId, Blocks, srgb_to_linear};
use crate::climate::{ColumnClimate, SurfaceClimate};
use crate::geology::{Class, ColumnGeo, Geology};
use crate::rivers::{RiverHit, RiverParams, Rivers};
use serde::{Deserialize, Serialize};

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
}

/// Soil of one biome: `top` block over `under`, `depth_m` thick in all (on flat ground; it
/// thins with the slope, down to bare rock at `rock_slope`). Blocks are named as in the
/// registry; a name missing from an older world's registry falls back to grass or dirt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BiomeSoil {
    /// Biome name, as in `nascio_worldgen::biomes::NAMES`.
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
        }
    }
}

impl VoxelConfig {
    pub fn from_toml(s: &str) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            voxel: VoxelConfig,
        }
        Ok(toml::from_str::<File>(s)?.voxel)
    }
}

/// The surface of a world: everything chunk generation needs, immutable and thread-safe.
#[derive(Clone, Debug)]
pub struct Terrain {
    cfg: VoxelConfig,
    elevation: PeriodicGrid<f32>,
    /// Lake surface per elevation cell, `-∞` where there is no lake.
    lake_level: Option<PeriodicGrid<f32>>,
    /// 0 (flat) to 1 (most rugged), per tectonic cell.
    ruggedness: PeriodicGrid<f32>,
    detail: Detail,
    /// Octaves at least this wide are already in `elevation`.
    baked_spacing: f64,
    /// Depth of the Moho below sea level (m, positive downwards), per tectonic cell; `None`:
    /// no magma.
    moho: Option<PeriodicGrid<f32>>,
    /// Block registry of the world.
    blocks: Blocks,
    /// Rock column and ore bodies (`None`: worlds made before M5, granite down to the Moho).
    geology: Option<Geology>,
    /// Soil by biome code.
    soils: [Soil; biomes::code::COUNT],
    /// Surface climate (`None`: worlds made before M4, temperate everywhere).
    climate: Option<SurfaceClimate>,
    /// River water (`None`: worlds made before M4).
    rivers: Option<Rivers>,
}

/// Grass tint of the four corners of a chunk column, for meshing: bilinear inside the chunk,
/// so neighbouring chunks share their edge colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChunkTint {
    /// The tinted block (grass), `None`: no tint.
    pub grass: Option<BlockId>,
    /// Linear RGB at local `(0, 0)`, `(32, 0)`, `(0, 32)`, `(32, 32)`.
    pub corners: [[f32; 3]; 4],
    /// Water depth (blocks) of each local column, `x + 32 z`; 255: deep or unknown.
    pub water_depth: [u8; 1024],
}

impl ChunkTint {
    pub const NONE: Self = Self {
        grass: None,
        corners: [[1.0; 3]; 4],
        water_depth: [u8::MAX; 1024],
    };

    /// Colour at local horizontal position `(x, z)` in `[0, 32]`.
    pub fn at(&self, x: f32, z: f32) -> [f32; 3] {
        let (u, v) = (x / 32.0, z / 32.0);
        let c = &self.corners;
        std::array::from_fn(|k| {
            let a = c[0][k] + (c[1][k] - c[0][k]) * u;
            let b = c[2][k] + (c[3][k] - c[2][k]) * u;
            a + (b - a) * v
        })
    }
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

impl Terrain {
    /// Builds the terrain of a generated world.
    pub fn from_world(world: &World, cfg: VoxelConfig) -> anyhow::Result<Self> {
        let tectonic = grid_f32(
            world
                .layer("elevation_tectonic")
                .context("world has no elevation_tectonic layer")?,
        );
        let flags = world
            .layer("history_flags")
            .map(grid_f32)
            .unwrap_or_else(|| tectonic.map(|_| 0.0));
        let (elevation, lakes) = match world.layer("elevation") {
            Some(fine) => (
                Some(grid_f32(fine)),
                world.layer("lake_depth").map(grid_f32),
            ),
            None => (None, None),
        };
        let blocks = Blocks::new(world.blocks()?)?;
        let geology = Geology::from_world(world, &blocks)?;
        let mut t = Self::new(
            world.config.seed,
            &world.config.erosion,
            &tectonic,
            &flags,
            elevation,
            lakes,
            cfg,
        )
        .with_moho(
            world
                .layer("moho_depth")
                .map(|l| smooth(&grid_f32(l), MOHO_SMOOTH_CELLS)),
        )
        .with_geology(blocks, geology);
        t.climate = SurfaceClimate::from_world(world, &t.elevation);
        t.rivers = match (
            world.layer("flow_direction"),
            world.layer("river_width"),
            world.layer("river_depth"),
        ) {
            (Some(d), Some(w), Some(dp)) if d.width == t.elevation.width() => {
                let dir = PeriodicGrid::from_vec(
                    d.width,
                    d.height,
                    d.cell_m,
                    (0..d.width * d.height)
                        .map(|k| d.data.get_f64(k) as u8)
                        .collect(),
                );
                Some(Rivers::new(
                    &dir,
                    &grid_f32(w),
                    &grid_f32(dp),
                    &t.elevation,
                    t.lake_level.as_ref(),
                    RiverParams {
                        min_width_m: t.cfg.river_min_width_m,
                        bank_m: t.cfg.river_bank_m,
                        smooth_passes: t.cfg.river_smooth_passes,
                    },
                ))
            }
            _ => None,
        };
        Ok(t)
    }

    /// Sets the block registry and the geology.
    pub fn with_geology(mut self, blocks: Blocks, geology: Option<Geology>) -> Self {
        self.soils = resolve_soils(&self.cfg, &blocks);
        self.blocks = blocks;
        self.geology = geology;
        self
    }

    pub fn blocks(&self) -> &Blocks {
        &self.blocks
    }

    pub fn geology(&self) -> Option<&Geology> {
        self.geology.as_ref()
    }

    /// Geology and climate of the column at `(x, z)` (defaults without them).
    pub fn column_geo(&self, x: f64, z: f64) -> ColumnGeo {
        let mut c = self
            .geology
            .as_ref()
            .map_or_else(ColumnGeo::default, |g| g.column(x, z));
        c.climate = self.climate(x, z);
        if self.rivers.is_some() {
            c.river = self.river(x, z).is_some();
        }
        c
    }

    /// The river at `(x, z)` (column centre), if any.
    #[inline]
    pub fn river(&self, x: f64, z: f64) -> Option<RiverHit> {
        self.rivers.as_ref()?.at(x, z)
    }

    /// Surface climate at `(x, z)`.
    pub fn climate(&self, x: f64, z: f64) -> ColumnClimate {
        self.climate
            .as_ref()
            .map_or_else(ColumnClimate::default, |c| c.column(x, z))
    }

    fn lapse(&self) -> f64 {
        self.climate.as_ref().map_or(0.0065, |c| c.lapse)
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
        let ice_biome = self.geology.is_some() && col.biome == biomes::code::ICE;
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

    /// Whether sea or lake water whose highest block is `level` carries permanent ice.
    pub fn water_frozen(&self, c: &ColumnClimate, level: i32) -> bool {
        self.blocks.ids.ice.is_some() && self.t_warm(c, level) < self.cfg.water_ice_warm_c
    }

    /// Block at height `y` in the water of a column (`top < y < water_top`): water, or ice in
    /// its top `water_ice_m` blocks where it freezes.
    #[inline]
    pub fn water_block(&self, col: &ColumnGeo, y: i32, water_top: i32) -> BlockId {
        let ids = &self.blocks.ids;
        match ids.ice {
            Some(ice)
                if y >= water_top - self.cfg.water_ice_m
                    && self.water_frozen(&col.climate, water_top - 1) =>
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

    /// Sets the Moho depth grid (m below sea level, positive downwards).
    pub fn with_moho(mut self, moho: Option<PeriodicGrid<f32>>) -> Self {
        self.moho = moho;
        self
    }

    /// First magma block of a column whose top solid block is at `top` (stone above it): the
    /// Moho, kept at least `min_crust_m` below the surface. `i32::MIN` without Moho data.
    pub fn moho_y(&self, x: f64, z: f64, top: i32) -> i32 {
        match &self.moho {
            Some(m) => {
                let y = (-m.sample_bilinear(x, z)).floor() as i32;
                y.min(top - self.cfg.min_crust_m)
            }
            None => i32::MIN,
        }
    }

    /// Builds a terrain. `tectonic` and `history_flags` share the tectonic grid (they set the
    /// ruggedness); `elevation` is the eroded fine grid with its `lake_depth`, or `None` to
    /// use the tectonic elevation itself (worlds made before M3).
    pub fn new(
        seed: u64,
        erosion: &ErosionConfig,
        tectonic: &PeriodicGrid<f32>,
        history_flags: &PeriodicGrid<f32>,
        elevation: Option<PeriodicGrid<f32>>,
        lake_depth: Option<PeriodicGrid<f32>>,
        cfg: VoxelConfig,
    ) -> Self {
        let ruggedness = ruggedness(tectonic, |i, j| *history_flags.get(i, j) as u8, erosion);
        let (elevation, baked_spacing) = match elevation {
            Some(e) => {
                let b = baked_min_spacing(e.cell_size());
                (e, b)
            }
            None => (tectonic.clone(), f64::INFINITY),
        };
        let lake_level = lake_depth.map(|d| {
            PeriodicGrid::from_fn(d.width(), d.height(), d.cell_size(), |i, j| {
                let depth = *d.get(i as i64, j as i64);
                if depth > 0.0 {
                    elevation.get(i as i64, j as i64) + depth
                } else {
                    f32::NEG_INFINITY
                }
            })
        });
        let blocks = Blocks::builtin();
        Self {
            climate: None,
            rivers: None,
            soils: resolve_soils(&cfg, &blocks),
            cfg,
            elevation,
            lake_level,
            ruggedness,
            detail: Detail::new(seed, erosion),
            baked_spacing,
            moho: None,
            blocks,
            geology: None,
        }
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
            Some(RiverHit::Bank { top }) => h.max(f64::from(top) + 0.5),
            None => h,
        }
    }

    /// Macro elevation (eroded grid), without the chunk detail.
    #[inline]
    pub fn base_height(&self, x: f64, z: f64) -> f64 {
        self.elevation.sample_bicubic_clamped(x, z)
    }

    /// Surface height keeping only the octaves at least twice as wide as `sample_spacing`
    /// (for coarse horizon meshes, which would alias finer detail).
    pub fn height_lod(&self, x: f64, z: f64, sample_spacing: f64) -> f64 {
        let e = self.elevation.sample_bicubic_clamped(x, z);
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
        let Some(l) = &self.lake_level else {
            return 0.0;
        };
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
            // Beaches and shallow sea floors: quartz sand; deep sea floors: bare sediment
            // (sand everywhere for worlds without geology).
            return if top > c.shelf_sand_m || self.geology.is_none() {
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
        let biome = if self.geology.is_some() {
            col.biome
        } else {
            biomes::code::TEMPERATE_FOREST
        };
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
        let ids = &self.blocks.ids;
        let mut soil = self.soil(col, top, slope);
        if let Some((ice, snow)) = self.glacier(col, top, slope) {
            // Ice and snow hold no ore (class `Soil`); the soil under them is gone.
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
            soil.depth = 0;
        }
        let soil_depth = soil.depth;
        if depth < soil_depth {
            return (if depth == 0 { soil.top } else { soil.under }, soil.class);
        }
        let (fx, fz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
        let Some(g) = &self.geology else {
            return (ids.granite, Class::Basement);
        };
        let r = f64::from(depth - soil_depth);
        let weathering = f64::from(col.weathering);
        if r < weathering.floor() {
            let b = if col.laterite {
                ids.laterite
            } else {
                ids.regolith
            };
            return (b, Class::Weathered);
        }
        if r < (weathering + f64::from(col.sediment)).floor() {
            return (
                g.sediment_block(ids, col, fx, top - depth, fz),
                Class::Sediment,
            );
        }
        let crust = f64::from((top - moho).max(1));
        let u = f64::from(depth) / crust;
        (g.basement_block(ids, col, fx, fz, u), Class::Basement)
    }

    /// Top block of column `(x, z)` (world block coordinates) without ore, snow cover
    /// included (horizon colours).
    pub fn top_block(&self, x: i32, z: i32, top: i32, slope: f64) -> BlockId {
        let col = self.column_geo(f64::from(x) + 0.5, f64::from(z) + 0.5);
        if let Some(l) = self.snow_layer(&col, top, slope) {
            return l;
        }
        self.rock_block(&col, x, z, top, slope, i32::MIN / 2, 0).0
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use nascio_core::consts::WORLD_SIZE_F64;
    use proptest::prelude::*;

    /// A small synthetic world: a mountain ridge crossing both seams, an ocean, and a lake
    /// straddling the x seam.
    pub fn test_terrain() -> Terrain {
        static T: std::sync::OnceLock<Terrain> = std::sync::OnceLock::new();
        T.get_or_init(build_test_terrain).clone()
    }

    fn build_test_terrain() -> Terrain {
        let n = 125;
        let cell = WORLD_SIZE_F64 / n as f64;
        let wave = |i: usize, j: usize, n: usize| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            let b = std::f64::consts::TAU * j as f64 / n as f64;
            (1_200.0 * a.cos() + 900.0 * b.sin() + 300.0 * (a + b).cos()) as f32
        };
        let tectonic = PeriodicGrid::from_fn(n, n, cell, |i, j| wave(i, j, n));
        let flags = PeriodicGrid::from_fn(n, n, cell, |i, _| if i < 5 { 1.0 } else { 0.0 });
        let m = 5 * n;
        let fine = PeriodicGrid::from_fn(m, m, cell / 5.0, |i, j| wave(i, j, m));
        let lakes = PeriodicGrid::from_fn(m, m, cell / 5.0, |i, j| {
            if (i < 3 || i > m - 4) && (300..306).contains(&j) {
                5.0
            } else {
                0.0
            }
        });
        // Moho 700 m under the sea floor, 3 500 m under the land, varying across both seams.
        let moho = PeriodicGrid::from_fn(n, n, cell, |i, j| {
            let e = f64::from(wave(i, j, n));
            (if e < 0.0 { 700.0 - e } else { 3_500.0 + e }) as f32
        });
        Terrain::new(
            42,
            &ErosionConfig::default(),
            &tectonic,
            &flags,
            Some(fine),
            Some(lakes),
            VoxelConfig::default(),
        )
        .with_moho(Some(moho))
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
}
