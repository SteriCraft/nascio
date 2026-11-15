//! Surface height function shared by the chunks and the horizon (spec §5).
//!
//! `height = bicubic(elevation_tectonic) + amplitude · detail`, where `detail` is a periodic
//! fractal noise and `amplitude` depends on the context: a few metres on plains and near the
//! coast, hundreds of metres in mountains (orogens, steep tectonic slopes), tens of metres on
//! the sea floor. The tectonic grid (160 m) is provisional until the 32 m grid of M3.

use anyhow::Context;
use nascio_core::math::smoothstep;
use nascio_worldgen::grid::PeriodicGrid;
use nascio_worldgen::noise::{PeriodicNoise, divisor_at_most};
use nascio_worldgen::rng::{mix64, stage_u64};
use nascio_worldgen::tectonics::flags;
use nascio_worldgen::world_io::{Layer, World};
use serde::{Deserialize, Serialize};

/// Tunable parameters of chunk generation (`[voxel]` in TOML).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VoxelConfig {
    /// Spacing of the coarsest detail octave (snapped down to a divisor of 100 000).
    pub detail_base_spacing_m: f64,
    /// Finest detail octave spacing.
    pub detail_min_spacing_m: f64,
    /// Amplitude ratio between consecutive octaves.
    pub detail_gain: f64,
    /// Detail amplitude on lowland plains.
    pub plain_amplitude_m: f64,
    /// Detail amplitude in the most rugged mountains.
    pub mountain_amplitude_m: f64,
    /// Detail amplitude on the sea floor.
    pub seabed_amplitude_m: f64,
    /// Tectonic elevation at which land counts as fully mountainous.
    pub mountain_elevation_m: f64,
    /// Tectonic slope (m/m) at which land counts as fully mountainous.
    pub mountain_slope: f64,
    /// Ruggedness given to orogens whatever their elevation.
    pub orogeny_ruggedness: f64,
    /// Share of ridged noise at full ruggedness (sharp crests).
    pub ridged_share: f64,
    /// Within this height of sea level, detail is damped to keep coasts clean.
    pub coast_band_m: f64,
    /// Detail amplitude factor right at sea level.
    pub coast_damping: f64,
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
            detail_base_spacing_m: 1_000.0,
            detail_min_spacing_m: 16.0,
            detail_gain: 0.5,
            plain_amplitude_m: 8.0,
            mountain_amplitude_m: 320.0,
            seabed_amplitude_m: 25.0,
            mountain_elevation_m: 1_800.0,
            mountain_slope: 0.15,
            orogeny_ruggedness: 0.6,
            ridged_share: 0.6,
            coast_band_m: 40.0,
            coast_damping: 0.4,
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
    /// 0 (flat) to 1 (most rugged), smoothed, per tectonic cell.
    ruggedness: PeriodicGrid<f32>,
    /// `(noise, amplitude)` from the coarsest octave.
    octaves: Vec<(PeriodicNoise, f64)>,
    norm: f64,
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
        let elevation = grid_f32(
            world
                .layer("elevation_tectonic")
                .context("world has no elevation_tectonic layer")?,
        );
        let flags = world
            .layer("history_flags")
            .map(grid_f32)
            .unwrap_or_else(|| elevation.map(|_| 0.0));
        Ok(Self::new(world.config.seed, elevation, &flags, cfg))
    }

    /// Builds a terrain from an elevation grid and a `history_flags` grid of the same shape.
    pub fn new(
        seed: u64,
        elevation: PeriodicGrid<f32>,
        history_flags: &PeriodicGrid<f32>,
        cfg: VoxelConfig,
    ) -> Self {
        let ruggedness = ruggedness(&elevation, history_flags, &cfg);
        let base = stage_u64(seed, "voxel-detail");
        let mut octaves = Vec::new();
        let mut spacing = divisor_at_most(cfg.detail_base_spacing_m);
        let mut amp = 1.0;
        while f64::from(spacing) >= cfg.detail_min_spacing_m {
            let k = octaves.len() as u64;
            octaves.push((PeriodicNoise::new(mix64(base ^ k), spacing), amp));
            let next = divisor_at_most(f64::from(spacing) / 2.0);
            if next == spacing {
                break;
            }
            spacing = next;
            amp *= cfg.detail_gain;
        }
        let norm = octaves.iter().map(|(_, a)| a).sum::<f64>().max(1e-9);
        Self {
            cfg,
            elevation,
            ruggedness,
            octaves,
            norm,
        }
    }

    pub fn config(&self) -> &VoxelConfig {
        &self.cfg
    }

    /// Spacing of the finest detail octave.
    pub fn finest_spacing(&self) -> f64 {
        self.octaves
            .last()
            .map_or(f64::INFINITY, |(n, _)| n.spacing())
    }

    /// Surface height at a world position, full detail.
    #[inline]
    pub fn height(&self, x: f64, z: f64) -> f64 {
        self.height_lod(x, z, 0.0)
    }

    /// Tectonic (base) elevation, without detail.
    #[inline]
    pub fn base_height(&self, x: f64, z: f64) -> f64 {
        self.elevation.sample_bicubic(x, z)
    }

    /// Surface height keeping only the octaves at least twice as wide as `sample_spacing`
    /// (for coarse horizon meshes, which would alias finer detail).
    pub fn height_lod(&self, x: f64, z: f64, sample_spacing: f64) -> f64 {
        let c = &self.cfg;
        let e = self.elevation.sample_bicubic(x, z);
        let r = self.ruggedness.sample_bilinear(x, z).clamp(0.0, 1.0);
        let land = c.plain_amplitude_m + (c.mountain_amplitude_m - c.plain_amplitude_m) * r;
        let w = smoothstep(-c.coast_band_m, c.coast_band_m, e);
        let damp =
            c.coast_damping + (1.0 - c.coast_damping) * smoothstep(0.0, c.coast_band_m, e.abs());
        let amplitude = (c.seabed_amplitude_m + (land - c.seabed_amplitude_m) * w) * damp;

        let min_spacing = 2.0 * sample_spacing;
        let (mut smooth, mut ridged) = (0.0, 0.0);
        for (n, a) in &self.octaves {
            if n.spacing() < min_spacing {
                break;
            }
            let v = n.sample(x, z);
            smooth += a * v;
            ridged += a * (1.0 - 2.0 * v.abs());
        }
        let k = c.ridged_share * r * w;
        e + amplitude * ((1.0 - k) * smooth + k * ridged) / self.norm
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

/// Ruggedness per tectonic cell: elevation, slope and orogeny, then smoothed.
fn ruggedness(
    elevation: &PeriodicGrid<f32>,
    history_flags: &PeriodicGrid<f32>,
    c: &VoxelConfig,
) -> PeriodicGrid<f32> {
    let (w, h) = (elevation.width(), elevation.height());
    let cell = elevation.cell_size();
    let raw = PeriodicGrid::from_fn(w, h, cell, |i, j| {
        let (i, j) = (i as i64, j as i64);
        let e = f64::from(*elevation.get(i, j));
        if e <= 0.0 {
            return 0.0f32;
        }
        let mut grade = 0.0f64;
        for (di, dj) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            grade = grade.max((f64::from(*elevation.get(i + di, j + dj)) - e).abs() / cell);
        }
        let mut r = smoothstep(150.0, c.mountain_elevation_m, e).max(smoothstep(
            0.0,
            c.mountain_slope,
            grade,
        ));
        let f = *history_flags.get(i, j) as u8;
        if f & flags::OROGENY != 0 {
            r = r.max(c.orogeny_ruggedness);
        }
        r as f32
    });
    box_blur(&box_blur(&raw))
}

fn box_blur(g: &PeriodicGrid<f32>) -> PeriodicGrid<f32> {
    PeriodicGrid::from_fn(g.width(), g.height(), g.cell_size(), |i, j| {
        let (i, j) = (i as i64, j as i64);
        let mut s = 0.0;
        for dj in -1..=1 {
            for di in -1..=1 {
                s += g.get(i + di, j + dj);
            }
        }
        s / 9.0
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use nascio_core::consts::WORLD_SIZE_F64;
    use proptest::prelude::*;

    /// A small synthetic world: a mountain ridge crossing both seams, and an ocean.
    pub fn test_terrain() -> Terrain {
        let n = 125;
        let cell = WORLD_SIZE_F64 / n as f64;
        let elevation = PeriodicGrid::from_fn(n, n, cell, |i, j| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            let b = std::f64::consts::TAU * j as f64 / n as f64;
            (1_200.0 * a.cos() + 900.0 * b.sin() + 300.0 * (a + b).cos()) as f32
        });
        let flags = PeriodicGrid::from_fn(n, n, cell, |i, _| if i < 5 { 1.0 } else { 0.0 });
        Terrain::new(42, elevation, &flags, VoxelConfig::default())
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
    fn lod_zero_is_full_detail_and_coarse_is_smoother() {
        let t = test_terrain();
        assert!(t.finest_spacing() >= 16.0);
        let (mut fine, mut coarse) = (0.0, 0.0);
        for k in 0..2_000 {
            let x = f64::from(k) * 4.0;
            let d = t.height(x + 4.0, 500.0) - t.height(x, 500.0);
            fine += d * d;
            let d = t.height_lod(x + 4.0, 500.0, 512.0) - t.height_lod(x, 500.0, 512.0);
            coarse += d * d;
            assert_eq!(t.height(x, 500.0), t.height_lod(x, 500.0, 8.0));
        }
        assert!(coarse < fine);
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
