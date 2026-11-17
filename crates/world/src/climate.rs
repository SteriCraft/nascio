//! Climate parameters (spec §4.3).

use serde::{Deserialize, Serialize};

/// Tunable climate parameters (`[climate]` in TOML).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClimateConfig {
    /// Samples of the typical year (spec: 24 to 48).
    pub steps: u32,
    // --- Temperature ---
    /// Annual mean sea-level temperature against the annual mean insolation relative to the
    /// equator's: `[ratio, °C]`, ascending ratios (Earth zonal means: 26 °C at the equator,
    /// −20 °C at the seam once the polar cooling is added).
    pub mean_profile: Vec<[f64; 2]>,
    /// Seasonal sensitivity of the equilibrium temperature, °C per W/m² of departure of the
    /// daily insolation from its annual mean.
    pub seasonal_c_per_w: f64,
    /// Phase lag of the temperature behind the insolation on land, days.
    pub lag_land_days: f64,
    /// Phase lag at sea, days.
    pub lag_sea_days: f64,
    /// Distance inland over which the maritime influence decays (e-folding), m.
    pub coast_decay_m: f64,
    /// Seasonal swing of the sea surface relative to the relaxed equilibrium at sea (the mixed
    /// layer damps it more than a first-order lag alone): sea and coasts get mild winters.
    pub sea_amplitude: f64,
    /// Lapse rate, °C per m.
    pub lapse_rate_c_per_m: f64,
    /// Sea surfaces never go below this temperature (sea ice), °C.
    pub sea_ice_c: f64,
    /// The floor `sea_ice_c` fades out as the warmest month of the sea goes from `sea_ice_c`
    /// to `sea_ice_c` minus this, °C: over perennial sea ice the surface is ice and follows
    /// the air (zonal mean near the seam ≈ −20 °C, §4.3); seas that thaw keep the floor.
    pub perennial_ice_fade_c: f64,
    /// Albedo cooling of the polar band (snow, ice, clouds), °C at `c_min(s) = 1`.
    pub polar_cooling_c: f64,
    // --- Ocean currents ---
    /// Warm anomaly of the western boundary currents, °C.
    pub current_warm_c: f64,
    /// Cold anomaly of the eastern boundary upwellings, °C.
    pub current_cold_c: f64,
    /// Width of a boundary current (e-folding distance from the coast), m.
    pub current_width_m: f64,
    /// Boundary current speed, m/s.
    pub current_speed_m_s: f64,
    // --- Winds ---
    /// Trade wind speed, m/s.
    pub trade_wind_m_s: f64,
    /// Westerly speed, m/s.
    pub westerly_m_s: f64,
    /// Polar easterly speed, m/s.
    pub polar_wind_m_s: f64,
    /// Shift of the wind belts and of the ITCZ, as a share of `δ_eff`.
    pub itcz_shift: f64,
    /// Half-width of the blend between wind belts, degrees of latitude.
    pub wind_blend_deg: f64,
    /// The wind vanishes at the seam and is full beyond this seam distance, m.
    pub seam_calm_m: f64,
    // --- Moisture and precipitation ---
    /// Zonal profile `[|latitude| in degrees, mm/yr]`, ascending latitudes, centred on the ITCZ.
    pub zonal_precip: Vec<[f64; 2]>,
    /// Distance travelled per advection iteration at the reference wind speed, m.
    pub advection_step_m: f64,
    /// Reference wind speed of `advection_step_m`, m/s.
    pub advection_ref_wind_m_s: f64,
    /// Advection iterations per sample (memory of the air: iterations × step).
    pub advection_iterations: u32,
    /// Distance over the sea for the air to recover its moisture (e-folding), m.
    pub ocean_recharge_m: f64,
    /// Distance over land for the air to lose its moisture without relief (e-folding), m.
    pub land_decay_m: f64,
    /// Share of the zonal precipitation kept by fully dried air.
    pub dry_floor: f64,
    /// Relative humidity the air reaches over the sea (0–1).
    pub sea_humidity: f64,
    /// Effective mass of air whose condensed vapour rains out, kg/m²: the rain of a cell is
    /// this mass times the wind speed times the condensed mixing ratio per metre travelled.
    /// It lumps the depth of the lifted layer, the share of the time it rains and the
    /// precipitation efficiency (calibrated on the orographic rain of the M4 climate).
    pub rain_column_kg_m2: f64,
    /// Precipitation forced in the polar band, mm/yr at `c_min(s) = 1` (§2.7: heavy).
    pub polar_precip_mm: f64,
    // --- Clouds and snow ---
    /// Precipitation rate giving a full cloud cover, mm/yr.
    pub cloud_full_mm: f64,
    /// Smallest weather cloud cover.
    pub cloud_min: f64,
    /// All precipitation is snow below this temperature, °C…
    pub snow_below_c: f64,
    /// …and rain above this one.
    pub rain_above_c: f64,
    /// Degree-day melt factor, mm of water per °C per day.
    pub melt_mm_per_degree_day: f64,
    /// Snow water equivalent that covers the ground completely, mm.
    pub snow_full_mm: f64,
    // --- Diurnal cycle ---
    /// Daily temperature range at sea, °C.
    pub diurnal_sea_c: f64,
    /// Daily temperature range of a dry continental interior, °C.
    pub diurnal_dry_c: f64,
    /// Daily range of a wet coastal land, °C.
    pub diurnal_wet_c: f64,
}

impl Default for ClimateConfig {
    fn default() -> Self {
        Self {
            steps: 24,
            mean_profile: vec![
                [0.0, -14.0],
                [0.2, -11.0],
                [0.48, -6.0],
                [0.52, 0.0],
                [0.57, 5.5],
                [0.63, 8.0],
                [0.68, 9.5],
                [0.74, 12.0],
                [0.79, 14.5],
                [0.84, 17.0],
                [0.91, 20.0],
                [0.96, 24.5],
                [1.0, 26.5],
            ],
            seasonal_c_per_w: 0.13,
            lag_land_days: 30.0,
            lag_sea_days: 52.0,
            coast_decay_m: 20_000.0,
            sea_amplitude: 0.4,
            lapse_rate_c_per_m: 0.0065,
            sea_ice_c: -1.8,
            perennial_ice_fade_c: 5.0,
            polar_cooling_c: 6.0,
            current_warm_c: 4.0,
            current_cold_c: 4.0,
            current_width_m: 4_000.0,
            current_speed_m_s: 1.0,
            trade_wind_m_s: 6.0,
            westerly_m_s: 8.0,
            polar_wind_m_s: 4.0,
            itcz_shift: 0.6,
            wind_blend_deg: 5.0,
            seam_calm_m: 2_000.0,
            zonal_precip: vec![
                [0.0, 2_400.0],
                [6.0, 2_000.0],
                [15.0, 1_000.0],
                [25.0, 300.0],
                [33.0, 450.0],
                [45.0, 1_000.0],
                [55.0, 1_050.0],
                [65.0, 700.0],
                [75.0, 400.0],
                [90.0, 300.0],
            ],
            advection_step_m: 1_000.0,
            advection_ref_wind_m_s: 8.0,
            advection_iterations: 80,
            ocean_recharge_m: 3_000.0,
            land_decay_m: 40_000.0,
            dry_floor: 0.3,
            sea_humidity: 0.9,
            rain_column_kg_m2: 50.0,
            polar_precip_mm: 1_600.0,
            cloud_full_mm: 3_000.0,
            cloud_min: 0.05,
            snow_below_c: -1.0,
            rain_above_c: 3.0,
            melt_mm_per_degree_day: 4.0,
            snow_full_mm: 40.0,
            diurnal_sea_c: 1.0,
            diurnal_dry_c: 16.0,
            diurnal_wet_c: 6.0,
        }
    }
}

impl ClimateConfig {
    /// Sanity checks; returns a human-readable reason on failure.
    pub fn validate(&self) -> Result<(), String> {
        if !(5..=365).contains(&self.steps) {
            return Err("steps must lie in 5..=365".into());
        }
        let z = &self.zonal_precip;
        if z.len() < 2 || z.windows(2).any(|w| w[0][0] >= w[1][0]) || z.iter().any(|p| p[1] < 0.0) {
            return Err("zonal_precip needs ascending latitudes and non-negative values".into());
        }
        let m = &self.mean_profile;
        if m.len() < 2 || m.windows(2).any(|w| w[0][0] >= w[1][0]) {
            return Err("mean_profile needs ascending ratios".into());
        }
        if self.perennial_ice_fade_c <= 0.0 {
            return Err("perennial_ice_fade_c must be > 0".into());
        }
        if self.lag_land_days <= 0.0 || self.lag_sea_days <= 0.0 || self.lag_sea_days >= 90.0 {
            return Err("lags must lie in (0, 90) days".into());
        }
        if self.snow_below_c >= self.rain_above_c || self.snow_full_mm <= 0.0 {
            return Err("snow_below_c < rain_above_c and snow_full_mm > 0 required".into());
        }
        if !(0.0..=1.0).contains(&self.sea_humidity) || self.rain_column_kg_m2 < 0.0 {
            return Err("sea_humidity must lie in [0, 1] and rain_column_kg_m2 be >= 0".into());
        }
        if self.advection_step_m <= 0.0 || self.advection_ref_wind_m_s <= 0.0 {
            return Err("advection_step_m and advection_ref_wind_m_s must be > 0".into());
        }
        Ok(())
    }
}
