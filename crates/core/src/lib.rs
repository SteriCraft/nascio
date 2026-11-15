//! Core maths of the Nascio world: torus, coordinates, constants, game time and astronomy.
//!
//! This crate must never depend on a graphics engine.

pub mod consts;
pub mod coords;
pub mod math;
pub mod torus;

pub use consts::WORLD_SIZE;
pub use coords::{BlockPos, ChunkPos, LocalPos, WorldPos};
