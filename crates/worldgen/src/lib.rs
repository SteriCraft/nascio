//! Macro world simulation: tectonics, erosion, climate, hydrology, biomes, geology.
//! The modules shared with the chunk generator live in `nascio-world` and are re-exported here.

pub use nascio_world::{blocks, config, grid, noise, rng, world_io};

pub mod biomes;
pub mod climate;
pub mod deposits;
pub mod erosion;
pub mod generate;
pub mod geology;
pub mod hypsometry;
pub mod plot;
pub mod preview;
pub mod progress;
pub mod seafloor;
pub mod sun_curves;
pub mod tectonics;
