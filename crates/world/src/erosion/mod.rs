//! Erosion parameters, flow direction codes and the detail noise shared with the chunks
//! (spec §4.2).

pub mod flow;
pub mod upsample;

use serde::{Deserialize, Serialize};

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
    /// Nor above `deposit_top_m − deposit_shelf_slope · distance to the coast`: the fans of
    /// neighbouring mouths merge into a shelf that deepens offshore (about −20 m 1 km out)
    /// instead of a flat at `deposit_top_m`.
    pub deposit_shelf_slope: f64,
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
            deposit_shelf_slope: 0.02,
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
        if self.deposit_top_m >= 0.0 || self.deposit_slope < 0.0 || self.deposit_shelf_slope < 0.0 {
            return Err(
                "deposit_top_m must be < 0, deposit_slope and deposit_shelf_slope >= 0".into(),
            );
        }
        Ok(())
    }

    /// Number of erosion steps.
    pub fn steps(&self) -> u32 {
        (self.duration_yr / self.dt_yr).round() as u32
    }
}
