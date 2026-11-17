//! Core maths of the Nascio world: torus, coordinates, constants, game time and astronomy.
//!
//! This crate must never depend on a graphics engine.

pub mod astro;
pub mod atmosphere;
pub mod climate;
pub mod consts;
pub mod coords;
pub mod fmath;
pub mod geo;
pub mod geotherm;
pub mod math;
pub mod time;
pub mod torus;

pub use astro::{AstroConfig, SunPosition};
pub use atmosphere::AtmosphereConfig;
pub use consts::WORLD_SIZE;
pub use coords::{BlockPos, ChunkPos, LocalPos, WorldPos};
pub use geotherm::GeothermConfig;
pub use time::{GameClock, GameTime};
