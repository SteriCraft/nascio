//! What a world is made of, as read by the chunk generator (and later the server): the
//! configuration, periodic grids, noise and RNG, the world files, the block registry, the
//! codes of the layers, the deposit catalogue and bodies, and the detail noise. The simulation
//! that produces a world stays in `nascio-worldgen`.

pub mod biomes;
pub mod blocks;
pub mod climate;
pub mod config;
pub mod deposits;
pub mod erosion;
pub mod geology;
pub mod grid;
pub mod noise;
pub mod rng;
pub mod tectonics;
pub mod world_io;
