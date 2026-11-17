//! World configuration, stored as `world.toml` (spec §6).

use crate::biomes::BiomeConfig;
use crate::climate::ClimateConfig;
use crate::erosion::ErosionConfig;
use crate::geology::GeologyConfig;
use nascio_core::astro::AstroConfig;
use nascio_core::time::GameClock;
use nascio_core::{AtmosphereConfig, GeothermConfig};
use serde::{Deserialize, Serialize};

/// Bumped whenever a change alters generated output for a given config.
pub const GENERATOR_VERSION: u32 = 11;

/// Full-resolution cell size of the tectonic grid, in metres (spec §1.4).
pub const TECTONIC_CELL_M: u32 = 160;
/// Full-resolution width of the tectonic grid.
pub const TECTONIC_RES: u32 = 625;
/// Full-resolution cell size of the hydrology / fine relief grid, in metres (one chunk).
pub const FINE_CELL_M: u32 = 32;
/// Full-resolution width of the hydrology / fine relief grid.
pub const FINE_RES: u32 = 3_125;

/// Resolution divisor. Factors divide 625 so grids stay aligned with chunks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub enum Scale {
    /// 625² tectonic cells of 160 m.
    Full,
    /// 125² tectonic cells of 800 m.
    Fifth,
    /// 25² tectonic cells of 4 km (fast tests).
    TwentyFifth,
}

impl Scale {
    /// Resolution divisor: 1, 5 or 25.
    pub fn divisor(self) -> u32 {
        match self {
            Scale::Full => 1,
            Scale::Fifth => 5,
            Scale::TwentyFifth => 25,
        }
    }

    /// Width (and height) of the tectonic grid.
    pub fn tectonic_res(self) -> usize {
        (TECTONIC_RES / self.divisor()) as usize
    }

    /// Cell size of the tectonic grid, in metres.
    pub fn tectonic_cell_m(self) -> f64 {
        f64::from(TECTONIC_CELL_M * self.divisor())
    }

    /// Width (and height) of the hydrology / fine relief grid.
    pub fn fine_res(self) -> usize {
        (FINE_RES / self.divisor()) as usize
    }

    /// Cell size of the hydrology / fine relief grid, in metres.
    pub fn fine_cell_m(self) -> f64 {
        f64::from(FINE_CELL_M * self.divisor())
    }
}

impl TryFrom<u32> for Scale {
    type Error = String;

    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            1 => Ok(Scale::Full),
            5 => Ok(Scale::Fifth),
            25 => Ok(Scale::TwentyFifth),
            _ => Err(format!("scale must be 1, 5 or 25 (got {v})")),
        }
    }
}

impl From<Scale> for u32 {
    fn from(s: Scale) -> u32 {
        s.divisor()
    }
}

/// Tunable tectonics parameters (spec §4.1). Lengths in metres, ages in Ma.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TectonicsConfig {
    /// Number of plates, inclusive range.
    pub plate_count: [u32; 2],
    /// Amplitude of the domain warp that makes plate boundaries irregular.
    pub plate_warp_amplitude_m: f64,
    /// Base wavelength of that warp.
    pub plate_warp_period_m: f64,
    /// Target share of continental crust at the end of the run (the initial share is
    /// calibrated to reach it).
    pub continental_fraction: f64,
    /// Allowed deviation of the final continental share from the target.
    pub continental_fraction_tolerance: f64,
    /// Base wavelength of the continent noise (low = few large masses).
    pub continent_noise_period_m: f64,
    /// Distance travelled by the fastest plate over the whole run.
    pub total_displacement_m: f64,
    /// Plate speeds as a fraction of the fastest, inclusive range.
    pub plate_speed_fraction: [f64; 2],
    /// Duration of the run, used to express ages in Ma.
    pub duration_ma: f64,
    /// Oceanic ridge depth.
    pub ridge_depth_m: f64,
    /// Abyssal plain depth.
    pub abyssal_depth_m: f64,
    /// Age at which the ocean floor reaches the abyssal depth (depth ∝ √age).
    pub abyssal_age_ma: f64,
    /// Depth at the axis of a subduction trench.
    pub trench_depth_m: f64,
    /// Half-width of a trench.
    pub trench_width_m: f64,
    /// Distance from the trench to the volcanic arc, on the overriding plate.
    pub arc_distance_m: f64,
    /// Half-width of the volcanic arc.
    pub arc_width_m: f64,
    /// Elevation added by an arc above its base.
    pub arc_uplift_m: f64,
    /// Half-width of the orogen band on each side of a collision.
    pub orogeny_width_m: f64,
    /// Relative displacement two plates keep after their continents meet, before they weld
    /// and move together (bounded crustal shortening).
    pub collision_shortening_m: f64,
    /// Highest summits.
    pub max_elevation_m: f64,
    /// Elevation of unthickened continental crust.
    pub continent_base_elevation_m: f64,
    /// Moho depth below the ocean floor.
    pub moho_ocean_m: f64,
    /// Moho depth below an ordinary continent.
    pub moho_continent_m: f64,
    /// Maximum Moho depth below an orogen.
    pub moho_orogen_max_m: f64,
    /// Number of hotspots, inclusive range.
    pub hotspot_count: [u32; 2],
    /// Radius of a hotspot volcanic edifice.
    pub hotspot_radius_m: f64,
    /// Height of a fresh hotspot edifice above the surrounding floor.
    pub hotspot_edifice_height_m: f64,
    /// Narrow features (trenches, arcs, hotspots) never get narrower than this many cells,
    /// so they survive coarse resolutions.
    pub min_feature_cells: f64,
    /// Uplift potential deposited by each continental collision event; after smoothing over the
    /// orogen width it scales like `uplift × shortening / width`, independently of resolution.
    pub orogeny_event_uplift_m: f64,
    /// Crest depth of an intra-oceanic island arc between its islands.
    pub island_arc_ridge_m: f64,
    /// Summit elevation of the islands of an intra-oceanic arc.
    pub island_arc_peak_m: f64,
    /// Distance of plate travel between two islands of a hotspot chain.
    pub hotspot_island_spacing_m: f64,
    /// Share of each spacing during which the hotspot builds, in `(0, 1]`.
    pub hotspot_duty: f64,
    /// Age over which a hotspot edifice loses most of its height (subsidence, erosion).
    pub hotspot_decay_ma: f64,
    /// Uplift of a continental plateau (trapps) above a hotspot.
    pub hotspot_plateau_m: f64,
    /// Relative normal speed, as a fraction of the fastest plate, separating convergent and
    /// divergent boundaries from transform ones.
    pub boundary_threshold: f64,
    /// Depth of the outer edge of a continental shelf (passive margins).
    pub shelf_depth_m: f64,
    /// Maximum width of a continental shelf on passive margins (active margins keep a minimal
    /// one). The actual width is set by `shelf_share`.
    pub shelf_width_m: f64,
    /// Share of continental crust under shelves: 7.6 / (29.2 + 7.6) on Earth.
    pub shelf_share: f64,
    /// Width of the continental slope, from the shelf edge down to the deep ocean.
    pub slope_width_m: f64,
    /// Steepest slope of the sea floor (m/m): the floor is raised wherever it drops faster
    /// away from the coast, a shallower sea cell or a sediment fan (chains plunging into the
    /// sea, fan fronts). Applied to the tectonic and to the eroded fine elevation; 0 disables.
    pub max_seafloor_slope: f64,
    /// Steepest coastal land (m/m): land is lowered to at most this slope times its distance
    /// to the sea, so chains do not plunge into it as walls (inland slopes are untouched).
    /// Applied to the tectonic and to the eroded fine elevation; 0 disables.
    pub max_coastal_slope: f64,
    /// Rank-calibrate the deep ocean floor on `hypsometry.deep` like the land. Off by default:
    /// the old floor is nearly flat, so the ranks were decided by the abyssal hills and the
    /// calibration stretched them into hundreds of metres of relief; the floor then follows
    /// the age–depth law directly.
    pub calibrate_deep: bool,
    /// Distance inland over which coastal plains rise (orders land before calibration).
    pub coastal_plain_width_m: f64,
    /// Amplitude of the low-frequency relief of continental plains (before calibration).
    pub plains_relief_m: f64,
    /// Amplitude of abyssal hills on the ocean floor (before calibration).
    pub abyssal_hill_m: f64,
    /// Oceanic regions smaller than this are inland water (lochs in rift zones), not open
    /// ocean: no shelves around them.
    pub inland_water_max_km2: f64,
    /// Water depth of inland water.
    pub inland_water_depth_m: f64,
    /// Target elevation distribution (Earth hypsometry scaled to the spec §3 targets).
    pub hypsometry: HypsometryConfig,
}

impl Default for TectonicsConfig {
    fn default() -> Self {
        Self {
            plate_count: [8, 12],
            plate_warp_amplitude_m: 6_000.0,
            plate_warp_period_m: 20_000.0,
            continental_fraction: 0.35,
            continental_fraction_tolerance: 0.02,
            continent_noise_period_m: 50_000.0,
            total_displacement_m: 40_000.0,
            plate_speed_fraction: [0.25, 1.0],
            duration_ma: 150.0,
            ridge_depth_m: -900.0,
            abyssal_depth_m: -1_800.0,
            abyssal_age_ma: 80.0,
            trench_depth_m: -2_500.0,
            trench_width_m: 3_000.0,
            arc_distance_m: 4_000.0,
            arc_width_m: 2_500.0,
            arc_uplift_m: 1_200.0,
            orogeny_width_m: 6_000.0,
            collision_shortening_m: 5_000.0,
            max_elevation_m: 3_000.0,
            continent_base_elevation_m: 200.0,
            moho_ocean_m: 700.0,
            moho_continent_m: 3_500.0,
            moho_orogen_max_m: 6_000.0,
            hotspot_count: [2, 5],
            hotspot_radius_m: 1_500.0,
            hotspot_edifice_height_m: 6_000.0,
            min_feature_cells: 1.5,
            orogeny_event_uplift_m: 3_000.0,
            island_arc_ridge_m: -800.0,
            island_arc_peak_m: 400.0,
            hotspot_island_spacing_m: 5_000.0,
            hotspot_duty: 0.35,
            hotspot_decay_ma: 120.0,
            hotspot_plateau_m: 500.0,
            boundary_threshold: 0.15,
            shelf_depth_m: -150.0,
            shelf_width_m: 5_000.0,
            shelf_share: 0.2,
            slope_width_m: 5_000.0,
            max_seafloor_slope: 0.35,
            max_coastal_slope: 0.8,
            calibrate_deep: false,
            coastal_plain_width_m: 15_000.0,
            plains_relief_m: 150.0,
            abyssal_hill_m: 100.0,
            inland_water_max_km2: 50.0,
            inland_water_depth_m: -80.0,
            hypsometry: HypsometryConfig::default(),
        }
    }
}

impl TectonicsConfig {
    /// Sanity checks; returns a human-readable reason on failure.
    pub fn validate(&self) -> Result<(), String> {
        let range_ok = |r: [u32; 2]| r[0] >= 1 && r[0] <= r[1];
        if !range_ok(self.plate_count) || self.plate_count[1] > 255 {
            return Err("plate_count must be a range within 1..=255".into());
        }
        if !range_ok([self.hotspot_count[0].max(1), self.hotspot_count[1].max(1)])
            || self.hotspot_count[0] > self.hotspot_count[1]
        {
            return Err("hotspot_count must be an increasing range".into());
        }
        if !(0.0..1.0).contains(&self.continental_fraction) {
            return Err("continental_fraction must lie in [0, 1)".into());
        }
        let [a, b] = self.plate_speed_fraction;
        if !(0.0 < a && a <= b && b <= 1.0) {
            return Err("plate_speed_fraction must satisfy 0 < min <= max <= 1".into());
        }
        if !(self.trench_depth_m < self.abyssal_depth_m
            && self.abyssal_depth_m < self.ridge_depth_m
            && self.ridge_depth_m < 0.0)
        {
            return Err("depths must satisfy trench < abyssal < ridge < 0".into());
        }
        if !(self.hotspot_duty > 0.0 && self.hotspot_duty <= 1.0)
            || self.hotspot_island_spacing_m <= 0.0
        {
            return Err(
                "hotspot_duty must lie in (0, 1] and hotspot_island_spacing_m be > 0".into(),
            );
        }
        self.hypsometry.validate()?;
        if self.max_seafloor_slope < 0.0 || self.max_coastal_slope < 0.0 {
            return Err("max_seafloor_slope and max_coastal_slope must be >= 0".into());
        }
        if !(self.shelf_depth_m < 0.0 && self.shelf_width_m > 0.0 && self.slope_width_m > 0.0) {
            return Err("shelf_depth_m must be < 0, shelf and slope widths > 0".into());
        }
        if self.total_displacement_m <= 0.0 || self.duration_ma <= 0.0 {
            return Err("total_displacement_m and duration_ma must be > 0".into());
        }
        Ok(())
    }
}

/// Target elevation distribution: Earth's hypsometric curve with its elevations remapped to
/// the spec §3 targets. Each band is `[low m, high m, share of Earth's surface in %]`, values
/// uniform inside a band, bands ascending and contiguous.
///
/// Earth → Nascio remapping: land 0/1 000/2 000/4 000/8 848 m → 0/400/900/1 800/3 000 m;
/// ocean −2 000/−3 000/−4 000/−5 000/−6 000/−11 000 m → −900/−1 300/−1 650/−1 900/−2 050/
/// −2 500 m (pre-M5 review: a shallower ocean than the §3 Earth-like scaling, abyss around
/// −1 500 to −2 000 m). The deep bands are the reference of the hypsometry chart and only
/// calibrate the floor when `calibrate_deep` is set. Shelves (0 to −200 m on Earth, 0 to −150 m here) and slopes are built
/// geometrically and are not remapped; their Earth shares are kept for the histogram.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HypsometryConfig {
    /// Land above sea level (continents outside shelves).
    pub land: Vec<[f64; 3]>,
    /// Deep ocean floor (outside shelves and slopes; edifices and arcs are added afterwards).
    pub deep: Vec<[f64; 3]>,
    /// Earth's share of continental shelves, %.
    pub shelf_pct: f64,
    /// Earth's share of continental slopes, %.
    pub slope_pct: f64,
}

impl Default for HypsometryConfig {
    fn default() -> Self {
        Self {
            land: vec![
                [0.0, 400.0, 20.8],
                [400.0, 900.0, 4.5],
                [900.0, 1_800.0, 3.9],
                [1_800.0, 3_000.0, 0.9],
            ],
            deep: vec![
                [-2_500.0, -2_050.0, 1.0],
                [-2_050.0, -1_900.0, 13.8],
                [-1_900.0, -1_650.0, 23.3],
                [-1_650.0, -1_300.0, 13.6],
                [-1_300.0, -900.0, 4.5],
            ],
            shelf_pct: 7.6,
            slope_pct: 6.1,
        }
    }
}

impl HypsometryConfig {
    /// Bands must be ascending, contiguous, non-empty, with positive shares.
    pub fn validate(&self) -> Result<(), String> {
        for (name, bands) in [("land", &self.land), ("deep", &self.deep)] {
            let ok = !bands.is_empty()
                && bands.iter().all(|b| b[0] < b[1] && b[2] > 0.0)
                && bands.windows(2).all(|w| w[0][1] == w[1][0]);
            if !ok {
                return Err(format!(
                    "hypsometry.{name} bands must be ascending, contiguous, with positive shares"
                ));
            }
        }
        Ok(())
    }
}

/// Everything needed to regenerate a world (spec §6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldConfig {
    /// World seed (TOML integers are signed: keep it below 2⁶³).
    pub seed: u64,
    pub generator_version: u32,
    pub scale: Scale,
    #[serde(default)]
    pub clock: GameClock,
    #[serde(default)]
    pub astro: AstroConfig,
    #[serde(default)]
    pub atmosphere: AtmosphereConfig,
    #[serde(default)]
    pub geotherm: GeothermConfig,
    #[serde(default)]
    pub tectonics: TectonicsConfig,
    #[serde(default)]
    pub erosion: ErosionConfig,
    #[serde(default)]
    pub climate: ClimateConfig,
    #[serde(default)]
    pub biomes: BiomeConfig,
    #[serde(default)]
    pub geology: GeologyConfig,
}

impl WorldConfig {
    /// Default configuration for a seed and scale.
    pub fn new(seed: u64, scale: Scale) -> Self {
        Self {
            seed,
            generator_version: GENERATOR_VERSION,
            scale,
            clock: GameClock::default(),
            astro: AstroConfig::default(),
            atmosphere: AtmosphereConfig::default(),
            geotherm: GeothermConfig::default(),
            tectonics: TectonicsConfig::default(),
            erosion: ErosionConfig::default(),
            climate: ClimateConfig::default(),
            biomes: BiomeConfig::default(),
            geology: GeologyConfig::default(),
        }
    }

    /// Checks every section.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.seed <= i64::MAX as u64, "seed must be below 2^63");
        self.astro.validate()?;
        self.atmosphere
            .validate()
            .map_err(|e| anyhow::anyhow!("atmosphere: {e}"))?;
        self.geotherm
            .validate()
            .map_err(|e| anyhow::anyhow!("geotherm: {e}"))?;
        self.tectonics
            .validate()
            .map_err(|e| anyhow::anyhow!("tectonics: {e}"))?;
        self.erosion
            .validate()
            .map_err(|e| anyhow::anyhow!("erosion: {e}"))?;
        self.climate
            .validate()
            .map_err(|e| anyhow::anyhow!("climate: {e}"))?;
        self.geology
            .validate()
            .map_err(|e| anyhow::anyhow!("geology: {e}"))?;
        Ok(())
    }

    pub fn to_toml(&self) -> anyhow::Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn from_toml(s: &str) -> anyhow::Result<Self> {
        let c: Self = toml::from_str(s)?;
        c.validate()?;
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales() {
        assert_eq!(Scale::Full.tectonic_res(), 625);
        assert_eq!(Scale::Fifth.tectonic_res(), 125);
        assert_eq!(Scale::TwentyFifth.tectonic_res(), 25);
        for s in [Scale::Full, Scale::Fifth, Scale::TwentyFifth] {
            assert_eq!(s.tectonic_res() as f64 * s.tectonic_cell_m(), 100_000.0);
            assert_eq!(s.fine_res() as f64 * s.fine_cell_m(), 100_000.0);
            assert_eq!(s.fine_res(), 5 * s.tectonic_res());
        }
        assert!(Scale::try_from(4).is_err());
    }

    #[test]
    fn toml_roundtrip() {
        let c = WorldConfig::new(42, Scale::Fifth);
        c.validate().unwrap();
        let s = c.to_toml().unwrap();
        assert!(s.contains("scale = 5"));
        assert!(s.contains("epoch = \"2026-01-01T00:00:00Z\""));
        assert_eq!(WorldConfig::from_toml(&s).unwrap(), c);
    }

    #[test]
    fn minimal_toml_uses_defaults() {
        let c = WorldConfig::from_toml(
            "seed = 7\ngenerator_version = 1\nscale = 25\n[tectonics]\nplate_count = [9, 9]\n",
        )
        .unwrap();
        assert_eq!(c.tectonics.plate_count, [9, 9]);
        assert_eq!(c.tectonics.continental_fraction, 0.35);
        assert_eq!(c.astro, AstroConfig::default());
    }

    #[test]
    fn rejects_bad_values() {
        assert!(WorldConfig::from_toml("seed = 7\ngenerator_version = 1\nscale = 4\n").is_err());
        assert!(
            WorldConfig::from_toml(
                "seed = 7\ngenerator_version = 1\nscale = 5\n[tectonics]\nridge_depth_m = -4000\n"
            )
            .is_err()
        );
    }

    #[test]
    #[ignore = "prints the default world.toml"]
    fn print_default_toml() {
        println!("{}", WorldConfig::new(42, Scale::Fifth).to_toml().unwrap());
    }
}
