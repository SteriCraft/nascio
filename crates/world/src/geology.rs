//! Geological contexts and settings (codes of the layers), and the geology parameters
//! (spec §4.6).

use serde::{Deserialize, Serialize};

/// Geological contexts known to the code (names used by the deposit catalogue).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Context {
    Ocean,
    Ridge,
    Trench,
    Arc,
    Orogen,
    Rift,
    Hotspot,
    Ophiolite,
    Ultramafic,
    Craton,
    Platform,
    Basin,
    MarineShelf,
    /// Oceanic crust next to a continent, away from trenches and arcs: the sediment wedge of
    /// a passive margin.
    PassiveMargin,
    Humid,
    Arid,
    Laterite,
    /// Hot and wet climate on land, whatever the slope (laterite also needs flat ground).
    Tropical,
    RiverSource,
    Coast,
}

/// Number of contexts.
pub const CONTEXTS: usize = 20;

impl Context {
    pub const ALL: [Context; CONTEXTS] = [
        Context::Ocean,
        Context::Ridge,
        Context::Trench,
        Context::Arc,
        Context::Orogen,
        Context::Rift,
        Context::Hotspot,
        Context::Ophiolite,
        Context::Ultramafic,
        Context::Craton,
        Context::Platform,
        Context::Basin,
        Context::MarineShelf,
        Context::PassiveMargin,
        Context::Humid,
        Context::Arid,
        Context::Laterite,
        Context::Tropical,
        Context::RiverSource,
        Context::Coast,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Context::Ocean => "ocean",
            Context::Ridge => "ridge",
            Context::Trench => "trench",
            Context::Arc => "arc",
            Context::Orogen => "orogen",
            Context::Rift => "rift",
            Context::Hotspot => "hotspot",
            Context::Ophiolite => "ophiolite",
            Context::Ultramafic => "ultramafic",
            Context::Craton => "craton",
            Context::Platform => "platform",
            Context::Basin => "basin",
            Context::MarineShelf => "marine_shelf",
            Context::PassiveMargin => "passive_margin",
            Context::Humid => "humid",
            Context::Arid => "arid",
            Context::Laterite => "laterite",
            Context::Tropical => "tropical",
            Context::RiverSource => "river_source",
            Context::Coast => "coast",
        }
    }

    pub fn from_name(name: &str) -> Option<Context> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Dominant geological setting of a cell (`geo_context` layer): what the rock column is built
/// from. Codes are stable within a generator version.
pub mod setting {
    pub const OCEAN: u8 = 0;
    pub const RIDGE: u8 = 1;
    pub const TRENCH: u8 = 2;
    pub const ARC: u8 = 3;
    pub const OROGEN: u8 = 4;
    pub const RIFT: u8 = 5;
    pub const HOTSPOT: u8 = 6;
    pub const OPHIOLITE: u8 = 7;
    pub const CRATON: u8 = 8;
    pub const PLATFORM: u8 = 9;
    pub const BASIN: u8 = 10;
    pub const MARINE_SHELF: u8 = 11;
    pub const PASSIVE_MARGIN: u8 = 12;
    pub const COUNT: usize = 13;

    pub const NAMES: [&str; COUNT] = [
        "ocean",
        "ridge",
        "trench",
        "volcanic arc",
        "orogen",
        "rift",
        "hotspot",
        "ophiolite",
        "craton",
        "platform",
        "sedimentary basin",
        "marine shelf",
        "passive margin",
    ];

    /// Whether the basement of this setting is oceanic crust.
    pub fn oceanic(code: u8) -> bool {
        matches!(code, OCEAN | RIDGE | TRENCH | PASSIVE_MARGIN)
    }
}

/// Tunable parameters of the geology (`[geology]` in TOML).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeologyConfig {
    /// Thickest sedimentary cover, m.
    pub sediment_max_m: f64,
    /// Continental lowlands below this (smoothed tectonic) elevation fill with sediments, m.
    pub basin_high_m: f64,
    /// Share of `sediment_max_m` in the deepest continental basins.
    pub basin_fill: f64,
    /// Share of `sediment_max_m` on continental shelves.
    pub shelf_fill: f64,
    /// Share of `sediment_max_m` at the foot of a passive margin (the wedge thins seawards).
    pub margin_fill: f64,
    /// Width of a passive margin, from the continental crust towards the ocean, m.
    pub passive_margin_m: f64,
    /// No passive margin within this distance of a trench or a volcanic arc (active margin), m.
    pub active_margin_m: f64,
    /// Share of `sediment_max_m` in rift grabens.
    pub rift_fill: f64,
    /// Cratons keep this share of the basin cover (thin platform cover).
    pub craton_cover: f64,
    /// Pelagic sediment accumulation on the ocean floor, m per Ma of crust age.
    pub pelagic_m_per_ma: f64,
    /// Thickest pelagic cover, m.
    pub pelagic_max_m: f64,
    /// Continental crust older than this is a craton (fully above `craton_age_ma + 100`), Ma.
    pub craton_age_ma: f64,
    /// Sediment thickness from which a continental cell counts as a basin (fully at twice), m.
    pub basin_min_m: f64,
    /// Depth of the regolith on gentle land slopes, m.
    pub regolith_m: f64,
    /// Extra depth of laterite under a tropical humid climate on flat land, m.
    pub laterite_m: f64,
    /// Mean annual temperature range (°C) and precipitation range (mm/yr) over which laterite
    /// sets in.
    pub laterite_temperature_c: [f64; 2],
    pub laterite_precipitation_mm: [f64; 2],
    /// Precipitation ranges (mm/yr) of the `humid` and `arid` contexts.
    pub humid_mm: [f64; 2],
    pub arid_mm: [f64; 2],
    /// River placer sources fade downstream over this distance, m.
    pub placer_reach_m: f64,
    /// Wavelength of the relative variations of the sedimentary cover, m.
    pub cover_noise_m: f64,
}

impl Default for GeologyConfig {
    fn default() -> Self {
        Self {
            sediment_max_m: 500.0,
            basin_high_m: 400.0,
            basin_fill: 0.8,
            shelf_fill: 0.7,
            margin_fill: 0.45,
            passive_margin_m: 6_000.0,
            active_margin_m: 5_000.0,
            rift_fill: 0.9,
            craton_cover: 0.35,
            pelagic_m_per_ma: 0.4,
            pelagic_max_m: 80.0,
            craton_age_ma: 1_900.0,
            basin_min_m: 120.0,
            regolith_m: 4.0,
            laterite_m: 18.0,
            laterite_temperature_c: [16.0, 22.0],
            laterite_precipitation_mm: [900.0, 1_600.0],
            humid_mm: [700.0, 1_400.0],
            arid_mm: [250.0, 550.0],
            placer_reach_m: 15_000.0,
            cover_noise_m: 8_000.0,
        }
    }
}

impl GeologyConfig {
    pub fn validate(&self) -> Result<(), String> {
        let range = |r: [f64; 2]| r[0] < r[1];
        if !(self.sediment_max_m > 0.0
            && self.basin_min_m > 0.0
            && self.regolith_m >= 0.0
            && self.laterite_m >= 0.0
            && self.placer_reach_m > 0.0
            && self.cover_noise_m > 0.0
            && self.passive_margin_m > 0.0
            && self.active_margin_m >= 0.0
            && (0.0..=1.0).contains(&self.margin_fill))
        {
            return Err("sediment_max_m, basin_min_m, placer_reach_m, cover_noise_m, passive_margin_m must be > 0, regolith_m, laterite_m, active_margin_m >= 0, margin_fill in [0, 1]".into());
        }
        if !(range(self.laterite_temperature_c)
            && range(self.laterite_precipitation_mm)
            && range(self.humid_mm)
            && range(self.arid_mm))
        {
            return Err("climate ranges must be increasing".into());
        }
        Ok(())
    }
}
