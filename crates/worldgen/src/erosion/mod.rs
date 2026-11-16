//! Erosion and hydrography on the fine grid of 32 m (spec §4.2, M3).
//!
//! Pipeline, after tectonics:
//! 1. [`upsample`]: tectonic elevation → fine grid (periodic bicubic), with a continuous graben
//!    along continental rifts and periodic detail noise whose amplitude follows the ruggedness;
//! 2. [`precip`]: provisional precipitation (zonal profile, orographic effect along zonal winds,
//!    polar band), on the tectonic grid;
//! 3. erosion steps (stream power and hillslope diffusion) and coastal sediments;
//! 4. final hydrography: lakes, flow directions, accumulation, basins, rivers.

pub mod flow;
pub mod hydro;
pub mod precip;
pub mod process;
pub mod uplift;
pub mod upsample;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::config::WorldConfig;
use crate::grid::PeriodicGrid;
use crate::tectonics::TectonicOutput;
use crate::world_io::Layer;

/// Tunable erosion and hydrography parameters (`[erosion]` in TOML).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ErosionConfig {
    // --- Upsampling and detail relief ---
    /// Spacing of the coarsest detail octave (snapped down to a divisor of 100 000).
    pub detail_base_spacing_m: f64,
    /// Finest detail octave. The fine grid bakes the octaves at least two cells wide; chunks
    /// add the finer ones.
    pub detail_finest_m: f64,
    /// Amplitude ratio between consecutive octaves.
    pub detail_gain: f64,
    /// Detail amplitude on lowland plains.
    pub detail_plain_m: f64,
    /// Detail amplitude in the most rugged mountains.
    pub detail_mountain_m: f64,
    /// Detail amplitude on the sea floor.
    pub detail_seabed_m: f64,
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
    /// Depth of a continental rift graben below the surrounding land.
    pub rift_depth_m: f64,
    /// Width of the graben shoulders, from the rift axis to the undisturbed land.
    pub rift_width_m: f64,
    /// Rift holes (inland water) up to this distance apart along a plate boundary are joined
    /// by a continuous graben.
    pub rift_link_m: f64,
    /// Domain warp of the tectonic elevation when upsampling, in tectonic cells (breaks the
    /// 160 m stairs of the coastlines; 0 disables it).
    pub coast_warp_cells: f64,

    // --- Provisional precipitation (the climate of M4 replaces it) ---
    /// Zonal profile `[|latitude| in degrees, mm/yr]`, ascending latitudes.
    pub zonal_precip: Vec<[f64; 2]>,
    /// Precipitation forced in the polar band, scaled by `c_min(s)` (spec §2.7).
    pub polar_precip_mm: f64,
    /// Distance over the sea for the air to recover its moisture (e-folding).
    pub ocean_recharge_m: f64,
    /// Distance over land for the air to lose its moisture without relief (e-folding).
    pub land_decay_m: f64,
    /// Share of the zonal precipitation kept by fully dried air.
    pub dry_floor: f64,
    /// Moisture flux of saturated air, mm/yr × m: the rain released over a climb is
    /// `flux × Δq / cell`, scaled by the zonal profile (relative to 1 000 mm/yr).
    pub orographic_flux: f64,
    /// Climb that releases 63 % of the moisture (e-folding height).
    pub orographic_height_m: f64,
    /// Half-width of the blend between wind belts, degrees of latitude.
    pub wind_blend_deg: f64,

    // --- Erosion ---
    /// Simulated duration.
    pub duration_yr: f64,
    /// Time step.
    pub dt_yr: f64,
    /// Stream power coefficient `K` of `E = K · A^m · S` (n = 1), with `A` the discharge
    /// normalised by 1 m/yr of rain (m²).
    pub k_fluvial: f64,
    /// Area exponent `m`.
    pub m_exponent: f64,
    /// Hillslope diffusivity, m²/yr.
    pub hillslope_diffusivity: f64,
    /// Critical slope of the nonlinear (Roering) hillslope flux.
    pub critical_slope: f64,
    /// Cap of the nonlinear factor `1 / (1 − (S/Sc)²)`.
    pub max_diffusion_factor: f64,
    /// Slope of the sediment fans built under the sea at river mouths.
    pub deposit_slope: f64,
    /// Sediment never builds above this depth.
    pub deposit_top_m: f64,
    /// Tectonic uplift rate along a convergent boundary closing at the fastest plate speed
    /// (m/yr); slower boundaries rise proportionally, over the orogen width. 0 disables uplift.
    pub uplift_max_m_yr: f64,
    /// Share of the driving convergence kept across a welded continental suture.
    pub suture_uplift_share: f64,

    // --- Final hydrography ---
    /// Depressions shallower than this are filled, not lakes.
    pub lake_min_depth_m: f64,
    /// Depressions smaller than this are filled, not lakes.
    pub lake_min_area_m2: f64,
    /// Discharge from which a cell is a river, m³/s.
    pub river_threshold_m3s: f64,
    /// River width `a·Q^b` (m, Q in m³/s): coefficient and exponent (hydraulic geometry).
    pub river_width_coef: f64,
    pub river_width_exp: f64,
    /// River depth `c·Q^f` (m): coefficient and exponent.
    pub river_depth_coef: f64,
    pub river_depth_exp: f64,
}

impl Default for ErosionConfig {
    fn default() -> Self {
        Self {
            detail_base_spacing_m: 2_000.0,
            detail_finest_m: 16.0,
            detail_gain: 0.5,
            detail_plain_m: 15.0,
            detail_mountain_m: 350.0,
            detail_seabed_m: 25.0,
            mountain_elevation_m: 1_800.0,
            mountain_slope: 0.15,
            orogeny_ruggedness: 0.6,
            ridged_share: 0.6,
            coast_band_m: 40.0,
            coast_damping: 0.4,
            rift_depth_m: 250.0,
            rift_width_m: 1_200.0,
            rift_link_m: 8_000.0,
            coast_warp_cells: 0.8,
            zonal_precip: vec![
                [0.0, 2_200.0],
                [8.0, 2_000.0],
                [18.0, 1_000.0],
                [28.0, 350.0],
                [35.0, 550.0],
                [45.0, 1_000.0],
                [55.0, 1_050.0],
                [65.0, 700.0],
                [75.0, 350.0],
                [90.0, 300.0],
            ],
            polar_precip_mm: 1_200.0,
            ocean_recharge_m: 3_000.0,
            land_decay_m: 25_000.0,
            dry_floor: 0.35,
            orographic_flux: 1.0e7,
            orographic_height_m: 1_500.0,
            wind_blend_deg: 5.0,
            duration_yr: 1.0e6,
            dt_yr: 5.0e4,
            k_fluvial: 3.0e-6,
            m_exponent: 0.45,
            hillslope_diffusivity: 0.01,
            critical_slope: 1.0,
            max_diffusion_factor: 3.0,
            deposit_slope: 0.01,
            deposit_top_m: -1.0,
            uplift_max_m_yr: 5.0e-4,
            suture_uplift_share: 0.5,
            lake_min_depth_m: 2.0,
            lake_min_area_m2: 20_000.0,
            river_threshold_m3s: 0.1,
            river_width_coef: 2.5,
            river_width_exp: 0.5,
            river_depth_coef: 0.3,
            river_depth_exp: 0.4,
        }
    }
}

impl ErosionConfig {
    /// Sanity checks; returns a human-readable reason on failure.
    pub fn validate(&self) -> Result<(), String> {
        let z = &self.zonal_precip;
        if z.len() < 2 || z.windows(2).any(|w| w[0][0] >= w[1][0]) || z.iter().any(|p| p[1] < 0.0) {
            return Err("zonal_precip needs ascending latitudes and non-negative values".into());
        }
        if self.dt_yr <= 0.0 || self.duration_yr < 0.0 {
            return Err("dt_yr must be > 0 and duration_yr >= 0".into());
        }
        if self.critical_slope <= 0.0 || self.max_diffusion_factor < 1.0 {
            return Err("critical_slope must be > 0 and max_diffusion_factor >= 1".into());
        }
        if self.rift_width_m <= 0.0 || self.detail_finest_m <= 0.0 {
            return Err("rift_width_m and detail_finest_m must be > 0".into());
        }
        if self.deposit_top_m >= 0.0 || self.deposit_slope < 0.0 {
            return Err("deposit_top_m must be < 0 and deposit_slope >= 0".into());
        }
        Ok(())
    }

    /// Number of erosion steps.
    pub fn steps(&self) -> u32 {
        (self.duration_yr / self.dt_yr).round() as u32
    }
}

/// Output of the erosion stage.
#[derive(Clone, Debug)]
pub struct ErosionOutput {
    /// Final elevation on the fine grid (lake beds included, small depressions filled), m.
    pub elevation: PeriodicGrid<f32>,
    /// Lake water depth, m (0 = no lake).
    pub lake_depth: PeriodicGrid<f32>,
    /// Strahler order of rivers (discharge ≥ `river_threshold_m3s`), 0 elsewhere.
    pub river_order: PeriodicGrid<u8>,
    /// Provisional precipitation on the tectonic grid, mm/yr.
    pub precipitation: PeriodicGrid<f32>,
    /// Tectonic uplift rate on the tectonic grid, m/yr.
    pub uplift: PeriodicGrid<f32>,
    /// D8 direction code (index in [`flow::D8`]), [`flow::NO_DIR`] on the sea.
    pub flow_direction: PeriodicGrid<u8>,
    /// Mean discharge, m³/s.
    pub flow_accumulation: PeriodicGrid<f32>,
    /// Drainage basin, numbered by outlet (1, 2, … in outlet index order); 0 on the sea.
    pub basin_id: PeriodicGrid<u32>,
    /// River width (m) after the final hydrology (§4.4), 0 off rivers.
    pub river_width: PeriodicGrid<f32>,
    /// River depth (m) after the final hydrology, 0 off rivers.
    pub river_depth: PeriodicGrid<f32>,
    pub report: ErosionReport,
}

/// Summary of the erosion stage (timings are informative, not part of the world).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ErosionReport {
    /// `(stage, seconds)` in pipeline order.
    pub timings: Vec<(&'static str, f64)>,
    pub stats: process::ErosionStats,
    /// Mean lowering of the land, m.
    pub mean_land_lowering_m: f64,
    pub lakes: usize,
    pub lake_area_km2: f64,
}

impl std::fmt::Display for ErosionReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = &self.stats;
        writeln!(
            f,
            "erosion: {} steps ({} diffusion sub-steps each), uplift {:.3} km3 (max {:.0} m), fluvial {:.3} km3, coastal {:.3} km3, deposited {:.3} km3 (lost {:.3}), sea-floor slope fill {:.3} km3, mean land lowering {:.1} m",
            s.steps,
            s.diffusion_substeps,
            s.uplift_m3 / 1e9,
            s.max_uplift_m,
            s.fluvial_m3 / 1e9,
            s.coastal_m3 / 1e9,
            s.deposited_m3 / 1e9,
            s.lost_m3 / 1e9,
            s.seafloor_m3 / 1e9,
            self.mean_land_lowering_m
        )?;
        writeln!(f, "lakes: {} ({:.1} km2)", self.lakes, self.lake_area_km2)?;
        write!(f, "erosion timings:")?;
        for (name, t) in &self.timings {
            write!(f, " {name} {t:.2} s,")?;
        }
        Ok(())
    }
}

/// Runs `f` and records its duration under `name`.
fn timed<T>(report: &mut ErosionReport, name: &'static str, f: impl FnOnce() -> T) -> T {
    let start = std::time::Instant::now();
    let out = f();
    report.timings.push((name, start.elapsed().as_secs_f64()));
    out
}

impl ErosionOutput {
    /// Layers in the on-disk format.
    pub fn layers(&self) -> Vec<Layer> {
        vec![
            Layer::from_f32("elevation", "m", &self.elevation),
            Layer::from_f32("precipitation_erosion", "mm/yr", &self.precipitation),
            Layer::from_f32("uplift_rate", "mm/yr", &self.uplift.map(|&v| v * 1_000.0)),
            Layer::from_u8("flow_direction", "D8 code, 255=none", &self.flow_direction),
            Layer::from_f32("flow_accumulation", "m3/s", &self.flow_accumulation),
            Layer::from_u32("basin_id", "id, 0=sea", &self.basin_id),
            Layer::from_f32("lake_depth", "m", &self.lake_depth),
            Layer::from_u8("river_order", "Strahler, 0=none", &self.river_order),
            Layer::from_f32("river_width", "m", &self.river_width),
            Layer::from_f32("river_depth", "m", &self.river_depth),
        ]
    }
}

/// Seconds in a year (Julian), to turn m³/yr into m³/s.
pub const SECONDS_PER_YEAR: f64 = 31_557_600.0;

/// Rain per fine cell (m³/yr) from the precipitation grid (mm/yr).
fn rain_volume(precip: &PeriodicGrid<f32>, shape: flow::Shape) -> Vec<f64> {
    let area = shape.cell * shape.cell;
    let mut v = vec![0.0; shape.n()];
    v.par_chunks_mut(shape.res)
        .enumerate()
        .for_each(|(j, row)| {
            let z = (j as f64 + 0.5) * shape.cell;
            for (i, o) in row.iter_mut().enumerate() {
                let x = (i as f64 + 0.5) * shape.cell;
                *o = precip.sample_bilinear(x, z).max(0.0) / 1_000.0 * area;
            }
        });
    v
}

/// Runs the erosion stage on a tectonic result.
pub fn run(cfg: &WorldConfig, tect: &TectonicOutput) -> ErosionOutput {
    let mut report = ErosionReport::default();
    let fine = timed(&mut report, "upsample", || upsample::upsample(cfg, tect));
    let precipitation = timed(&mut report, "precipitation", || {
        precip::precipitation(cfg, &fine)
    });
    let shape = flow::Shape {
        res: fine.width(),
        cell: fine.cell_size(),
    };
    let rain = rain_volume(&precipitation, shape);
    let uplift = uplift::uplift_rate(cfg, tect);
    let uplift_fine = uplift::on_fine(&uplift, shape.res, shape.cell);
    let sea_min_area_m2 = cfg.tectonics.inland_water_max_km2 * 1e6;
    let sea = flow::sea_mask(fine.data(), shape, sea_min_area_m2);
    let mut h = fine.data().to_vec();
    report.stats = timed(&mut report, "erosion", || {
        process::erode(&mut h, &rain, &uplift_fine, shape, &sea, &cfg.erosion)
    });
    report.stats.seafloor_m3 = timed(&mut report, "seafloor", || {
        // Fan fronts and plunging coasts: the eroded sea floor keeps the tectonic slope limit.
        crate::seafloor::limit_slope(
            &mut h,
            shape.res,
            shape.res,
            shape.cell,
            cfg.tectonics.max_seafloor_slope,
            &sea,
        )
    });
    let land: Vec<usize> = (0..shape.n()).filter(|&k| !sea[k]).collect();
    report.mean_land_lowering_m =
        land.iter().map(|&k| fine.data()[k] - h[k]).sum::<f64>() / land.len().max(1) as f64;
    let hy = timed(&mut report, "hydrography", || {
        hydro::hydrography(&h, &rain, shape, sea_min_area_m2, &cfg.erosion)
    });
    report.lakes = hy.lakes;
    report.lake_area_km2 =
        hy.lake_depth.iter().filter(|&&d| d > 0.0).count() as f64 * shape.cell * shape.cell / 1e6;
    let (res, cell) = (shape.res, shape.cell);
    let f32_grid =
        |v: &[f64]| PeriodicGrid::from_vec(res, res, cell, v.iter().map(|&x| x as f32).collect());
    let routing = hy;
    ErosionOutput {
        elevation: f32_grid(&routing.elevation),
        lake_depth: f32_grid(&routing.lake_depth),
        river_order: PeriodicGrid::from_vec(res, res, cell, routing.river_order),
        precipitation,
        uplift,
        flow_direction: PeriodicGrid::from_vec(res, res, cell, routing.dir),
        flow_accumulation: PeriodicGrid::from_vec(
            res,
            res,
            cell,
            routing
                .discharge
                .iter()
                .map(|q| (q / SECONDS_PER_YEAR) as f32)
                .collect(),
        ),
        basin_id: PeriodicGrid::from_vec(res, res, cell, routing.basin),
        river_width: PeriodicGrid::new(res, res, cell, 0.0),
        river_depth: PeriodicGrid::new(res, res, cell, 0.0),
        report,
    }
}

/// Final hydrology (spec §4.4): discharges recomputed with the climate's mean precipitation
/// (mm/yr, climate grid) on the eroded surface, then lakes, basins, rivers and their hydraulic
/// geometry (width and depth from the discharge).
pub fn final_hydrology(cfg: &WorldConfig, ero: &mut ErosionOutput, precip: &PeriodicGrid<f32>) {
    let shape = flow::Shape {
        res: ero.elevation.width(),
        cell: ero.elevation.cell_size(),
    };
    let rain = rain_volume(precip, shape);
    let h: Vec<f64> = ero.elevation.data().iter().map(|&v| f64::from(v)).collect();
    let sea_min_area_m2 = cfg.tectonics.inland_water_max_km2 * 1e6;
    let hy = hydro::hydrography(&h, &rain, shape, sea_min_area_m2, &cfg.erosion);
    let (res, cell) = (shape.res, shape.cell);
    let f32_grid =
        |v: &[f64]| PeriodicGrid::from_vec(res, res, cell, v.iter().map(|&x| x as f32).collect());
    let c = &cfg.erosion;
    let q: Vec<f64> = hy.discharge.iter().map(|d| d / SECONDS_PER_YEAR).collect();
    let on_river = |k: usize, v: f64| if hy.river_order[k] > 0 { v } else { 0.0 };
    let width: Vec<f64> = (0..q.len())
        .map(|k| on_river(k, c.river_width_coef * q[k].powf(c.river_width_exp)))
        .collect();
    let depth: Vec<f64> = (0..q.len())
        .map(|k| on_river(k, c.river_depth_coef * q[k].powf(c.river_depth_exp)))
        .collect();
    ero.report.lakes = hy.lakes;
    ero.report.lake_area_km2 =
        hy.lake_depth.iter().filter(|&&d| d > 0.0).count() as f64 * cell * cell / 1e6;
    ero.elevation = f32_grid(&hy.elevation);
    ero.lake_depth = f32_grid(&hy.lake_depth);
    ero.flow_direction = PeriodicGrid::from_vec(res, res, cell, hy.dir);
    ero.flow_accumulation = f32_grid(&q);
    ero.basin_id = PeriodicGrid::from_vec(res, res, cell, hy.basin);
    ero.river_order = PeriodicGrid::from_vec(res, res, cell, hy.river_order);
    ero.river_width = f32_grid(&width);
    ero.river_depth = f32_grid(&depth);
}
