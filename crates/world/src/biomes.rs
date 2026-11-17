//! Biome codes, names and thresholds (spec §4.5).

use serde::{Deserialize, Serialize};

/// Biome codes of the `biome` layer.
pub mod code {
    pub const OCEAN: u8 = 0;
    pub const LAKE: u8 = 1;
    pub const ICE: u8 = 2;
    pub const TUNDRA: u8 = 3;
    pub const BOREAL_FOREST: u8 = 4;
    pub const TEMPERATE_FOREST: u8 = 5;
    pub const MEDITERRANEAN: u8 = 6;
    pub const STEPPE: u8 = 7;
    pub const HOT_DESERT: u8 = 8;
    pub const COLD_DESERT: u8 = 9;
    pub const SAVANNA: u8 = 10;
    pub const RAINFOREST: u8 = 11;
    pub const MANGROVE: u8 = 12;
    pub const MARSH: u8 = 13;
    /// Number of codes.
    pub const COUNT: usize = 14;
}

/// Names of the biome codes, in code order.
pub const NAMES: [&str; code::COUNT] = [
    "ocean",
    "lake",
    "ice cap / glacier",
    "tundra",
    "boreal forest",
    "temperate forest",
    "mediterranean",
    "steppe",
    "hot desert",
    "cold desert",
    "savanna",
    "tropical rainforest",
    "mangrove",
    "marsh",
];

/// Tunable biome thresholds (`[biomes]` in TOML).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BiomeConfig {
    /// Coldest month at or above this: tropical, °C.
    pub tropical_min_c: f64,
    /// Coldest month above this: temperate (else continental), °C.
    pub temperate_min_c: f64,
    /// Warmest month below this: tundra, °C.
    pub tundra_max_c: f64,
    /// Warmest month below this: ice cap, °C.
    pub ice_max_c: f64,
    /// Snow cover all year at least this: ice cap or glacier.
    pub permanent_snow: f64,
    /// Driest month of a tropical rainforest at least this, mm.
    pub rainforest_driest_mm: f64,
    /// Continental climates with fewer months above 10 °C than this are boreal.
    pub boreal_warm_months: usize,
    /// Mangroves grow on tropical coasts up to this elevation, m.
    pub mangrove_max_m: f64,
    /// Marshes: flat land (slope below this) next to a lake or a river of at least
    /// `marsh_river_order`, with at least `marsh_min_mm` of yearly precipitation.
    pub marsh_max_slope: f64,
    pub marsh_river_order: u8,
    pub marsh_min_mm: f64,
}

impl Default for BiomeConfig {
    fn default() -> Self {
        Self {
            tropical_min_c: 18.0,
            temperate_min_c: -3.0,
            tundra_max_c: 10.0,
            ice_max_c: 0.0,
            permanent_snow: 0.95,
            rainforest_driest_mm: 60.0,
            boreal_warm_months: 4,
            mangrove_max_m: 4.0,
            marsh_max_slope: 0.004,
            marsh_river_order: 3,
            marsh_min_mm: 700.0,
        }
    }
}
