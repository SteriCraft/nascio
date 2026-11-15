//! Erosion and hydrography on the fine grid of 32 m (spec §4.2, M3).
//!
//! Pipeline, after tectonics:
//! 1. [`upsample`]: tectonic elevation → fine grid (periodic bicubic), with a continuous graben
//!    along continental rifts and periodic detail noise whose amplitude follows the ruggedness;
//! 2. [`precip`]: provisional precipitation (zonal profile, orographic effect along zonal winds,
//!    polar band), on the tectonic grid;
//! 3. erosion steps (stream power and hillslope diffusion) and coastal sediments;
//! 4. final hydrography: lakes, flow directions, accumulation, basins, rivers.

pub mod precip;
pub mod upsample;

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

    // --- Final hydrography ---
    /// Depressions shallower than this are filled, not lakes.
    pub lake_min_depth_m: f64,
    /// Depressions smaller than this are filled, not lakes.
    pub lake_min_area_m2: f64,
    /// Discharge from which a cell is a river, m³/s.
    pub river_threshold_m3s: f64,
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
            k_fluvial: 2.0e-6,
            m_exponent: 0.45,
            hillslope_diffusivity: 0.02,
            critical_slope: 1.0,
            max_diffusion_factor: 10.0,
            deposit_slope: 0.01,
            deposit_top_m: -1.0,
            lake_min_depth_m: 2.0,
            lake_min_area_m2: 20_000.0,
            river_threshold_m3s: 0.5,
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
    /// Final elevation on the fine grid (lake beds included), m.
    pub elevation: PeriodicGrid<f32>,
    /// Provisional precipitation on the tectonic grid, mm/yr.
    pub precipitation: PeriodicGrid<f32>,
    pub report: ErosionReport,
}

/// Summary of the erosion stage (timings are informative, not part of the world).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ErosionReport {
    /// `(stage, seconds)` in pipeline order.
    pub timings: Vec<(&'static str, f64)>,
}

impl std::fmt::Display for ErosionReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
            Layer::from_f32("precipitation", "mm/yr", &self.precipitation),
        ]
    }
}

/// Runs the erosion stage on a tectonic result.
pub fn run(cfg: &WorldConfig, tect: &TectonicOutput) -> ErosionOutput {
    let mut report = ErosionReport::default();
    let fine = timed(&mut report, "upsample", || upsample::upsample(cfg, tect));
    let precipitation = timed(&mut report, "precipitation", || {
        precip::precipitation(cfg, &fine)
    });
    ErosionOutput {
        elevation: fine.map(|&v| v as f32),
        precipitation,
        report,
    }
}
