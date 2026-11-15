//! Core maths of the Nascio world: torus, coordinates, constants, game time and astronomy.
//!
//! This crate must never depend on a graphics engine.

pub mod consts;
pub mod coords;
pub mod geo;
pub mod math;
pub mod time;
pub mod torus;

pub use consts::WORLD_SIZE;
pub use coords::{BlockPos, ChunkPos, LocalPos, WorldPos};
pub use time::{GameClock, GameTime};
