//! World-wide constants. Values that may be tuned per world live in config structs instead.

/// Horizontal period of the world, in blocks (1 block = 1 m). x and z live in `[0, WORLD_SIZE)`.
pub const WORLD_SIZE: i32 = 100_000;
/// [`WORLD_SIZE`] as `f64`.
pub const WORLD_SIZE_F64: f64 = WORLD_SIZE as f64;
/// Half of the world period: the largest possible shortest-path offset along one axis.
pub const HALF_WORLD: i32 = WORLD_SIZE / 2;

/// Edge length of a cubic chunk, in blocks.
pub const CHUNK_SIZE: i32 = 32;
/// Number of chunks along x or z (100 000 = 32 × 3 125).
pub const CHUNKS_PER_AXIS: i32 = WORLD_SIZE / CHUNK_SIZE;

/// Real seconds per game day.
pub const SECONDS_PER_DAY: f64 = 3_600.0;
/// Game days per game year.
pub const DAYS_PER_YEAR: i64 = 365;

/// Axial tilt, in degrees. Fixed for all worlds.
pub const OBLIQUITY_DEG: f64 = 23.44;
/// Solar constant, in W/m².
pub const SOLAR_CONSTANT: f64 = 1_361.0;

const _: () = assert!(WORLD_SIZE % CHUNK_SIZE == 0);
