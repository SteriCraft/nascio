//! Erosion and hydrography on the fine grid of 32 m (spec §4.2, M3).
//!
//! Pipeline, after tectonics:
//! 1. [`upsample`]: tectonic elevation → fine grid (periodic bicubic), with a continuous graben
//!    along continental rifts and periodic detail noise whose amplitude follows the ruggedness;
//! 2. [`precip`]: provisional precipitation (zonal profile, orographic effect along zonal winds,
//!    polar band), on the tectonic grid;
//! 3. erosion steps (stream power and hillslope diffusion) and coastal sediments;
//! 4. final hydrography: lakes, flow directions, accumulation, basins, rivers.

use nascio_core::fmath::DetMath;
pub mod flow;
pub mod hydro;
pub mod precip;
pub mod process;
pub mod uplift;
pub mod upsample;

use rayon::prelude::*;

use crate::config::WorldConfig;
use crate::grid::PeriodicGrid;
use crate::tectonics::TectonicOutput;
use crate::world_io::Layer;

pub use nascio_world::erosion::ErosionConfig;

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
pub fn run(
    cfg: &WorldConfig,
    tect: &TectonicOutput,
    progress: &crate::progress::Progress,
) -> ErosionOutput {
    let mut report = ErosionReport::default();
    let fine = timed(&mut report, "upsample", || upsample::upsample(cfg, tect));
    progress.set(0.02);
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
        process::erode(
            &mut h,
            &rain,
            &uplift_fine,
            shape,
            &sea,
            &cfg.erosion,
            progress,
        )
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
    timed(&mut report, "coastal slope", || {
        // Uplift may have steepened the coastal flanks again.
        crate::seafloor::limit_coastal_land(
            &mut h,
            shape.res,
            shape.res,
            shape.cell,
            cfg.tectonics.max_coastal_slope,
            &sea,
        )
    });
    let land: Vec<usize> = (0..shape.n()).filter(|&k| !sea[k]).collect();
    report.mean_land_lowering_m =
        land.iter().map(|&k| fine.data()[k] - h[k]).sum::<f64>() / land.len().max(1) as f64;
    progress.set(0.96);
    progress.detail("hydrography");
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
        .map(|k| on_river(k, c.river_width_coef * q[k].det_powf(c.river_width_exp)))
        .collect();
    let depth: Vec<f64> = (0..q.len())
        .map(|k| on_river(k, c.river_depth_coef * q[k].det_powf(c.river_depth_exp)))
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
