//! Surface height function shared by the chunks and the horizon (spec §5).
//!
//! `height = bicubic(elevation) + detail`. `elevation` is the eroded 32 m grid of M3 (one cell
//! per chunk at full resolution). The detail noise is the one of `nascio-worldgen`
//! ([`Detail`]): the fine grid already contains the octaves at least two cells wide, the chunks
//! add only the finer ones, with the amplitude of the context (ruggedness, coast, sea floor).
//! Worlds made before M3 have only `elevation_tectonic`: it is used instead, with every octave.
//!
//! Water: the sea fills every column up to `y = 0`; a lake fills the columns of its cells up
//! to its surface.

use anyhow::Context;
use nascio_worldgen::erosion::ErosionConfig;
use nascio_worldgen::erosion::upsample::{Detail, baked_min_spacing, ruggedness};
use nascio_worldgen::grid::PeriodicGrid;
use nascio_worldgen::world_io::{Layer, World};
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
    /// Sand thickness on beaches and the sea floor.
    pub sand_depth_m: i32,
    /// Chunks generated below the lowest surface of a column.
    pub underground_chunks: i32,
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
            underground_chunks: 2,
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
        Ok(Self::new(
            world.config.seed,
            &world.config.erosion,
            &tectonic,
            &flags,
            elevation,
            lakes,
            cfg,
        ))
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
        Self {
            cfg,
            elevation,
            lake_level,
            ruggedness,
            detail: Detail::new(seed, erosion),
            baked_spacing,
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

    /// Surface height at a world position, full detail.
    #[inline]
    pub fn height(&self, x: f64, z: f64) -> f64 {
        self.height_lod(x, z, 0.0)
    }

    /// Macro elevation (eroded grid), without the chunk detail.
    #[inline]
    pub fn base_height(&self, x: f64, z: f64) -> f64 {
        self.elevation.sample_bicubic(x, z)
    }

    /// Surface height keeping only the octaves at least twice as wide as `sample_spacing`
    /// (for coarse horizon meshes, which would alias finer detail).
    pub fn height_lod(&self, x: f64, z: f64, sample_spacing: f64) -> f64 {
        let e = self.elevation.sample_bicubic(x, z);
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

    /// Highest water block + 1 of a column: water fills `top < y < water_top`.
    #[inline]
    pub fn water_top(&self, x: f64, z: f64) -> i32 {
        self.water_level(x, z).round() as i32
    }

    /// Block of the surface (top solid block) for a column whose top block is at `top`.
    pub fn surface_block(&self, top: i32, slope: f64) -> crate::block::BlockId {
        use crate::block::{GRASS, SAND, STONE};
        let c = &self.cfg;
        if top <= c.beach_high_m {
            SAND
        } else if slope > c.rock_slope || f64::from(top) > c.grass_max_elevation_m {
            STONE
        } else {
            GRASS
        }
    }

    /// Block at depth `depth ≥ 0` below the top of a column (0 = the top block itself).
    pub fn column_block(&self, top: i32, slope: f64, depth: i32) -> crate::block::BlockId {
        use crate::block::{DIRT, GRASS, SAND, STONE};
        let surface = self.surface_block(top, slope);
        match surface {
            SAND if depth < self.cfg.sand_depth_m => SAND,
            GRASS if depth == 0 => GRASS,
            GRASS if depth <= self.cfg.dirt_depth_m => DIRT,
            _ => STONE,
        }
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
        Terrain::new(
            42,
            &ErosionConfig::default(),
            &tectonic,
            &flags,
            Some(fine),
            Some(lakes),
            VoxelConfig::default(),
        )
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
