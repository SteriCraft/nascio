//! Tunable values of the player's physics (`[player]` of the server configuration). The
//! server sends them to the client in its handshake, so both simulate with the same values.

use serde::{Deserialize, Serialize};

/// Player physics parameters. Lengths in m, speeds in m/s, accelerations in m/s².
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlayerParams {
    /// Steps per second of the simulation; a multiple of the server's 20 Hz tick.
    pub step_hz: u32,
    /// Side of the player's square footprint.
    pub width: f64,
    pub height: f64,
    /// Eyes above the feet (the camera).
    pub eye_height: f64,
    pub walk_speed: f64,
    pub run_speed: f64,
    pub crouch_speed: f64,
    /// Horizontal acceleration on the ground and in a fluid.
    pub ground_accel: f64,
    /// Horizontal acceleration in the air.
    pub air_accel: f64,
    pub gravity: f64,
    /// Height of a jump from the ground.
    pub jump_height: f64,
    /// Fastest fall.
    pub max_fall_speed: f64,
    /// Highest ledge climbed without jumping (a snow layer, a slab later; not a full block).
    pub step_height: f64,
    /// Swimming speed, horizontal and up (jump) or down (crouch).
    pub swim_speed: f64,
    /// Sinking speed in a fluid with no key held.
    pub sink_speed: f64,
    /// Vertical acceleration in a fluid.
    pub fluid_accel: f64,
    /// Fastest flight (administrators); the client asks a speed up to this one.
    pub fly_max_speed: f64,
}

impl Default for PlayerParams {
    fn default() -> Self {
        Self {
            step_hz: 60,
            width: 0.6,
            height: 1.8,
            eye_height: 1.62,
            walk_speed: 4.3,
            run_speed: 5.6,
            crouch_speed: 1.3,
            ground_accel: 50.0,
            air_accel: 10.0,
            gravity: 28.0,
            jump_height: 1.25,
            max_fall_speed: 60.0,
            step_height: 0.5,
            swim_speed: 2.0,
            sink_speed: 1.0,
            fluid_accel: 10.0,
            fly_max_speed: 120.0,
        }
    }
}

impl PlayerParams {
    /// Duration of a step, s.
    pub fn dt(&self) -> f64 {
        1.0 / f64::from(self.step_hz)
    }

    /// Checks values the simulation relies on.
    pub fn validate(&self, tick_hz: u32) -> Result<(), String> {
        if self.step_hz == 0 || tick_hz == 0 || !self.step_hz.is_multiple_of(tick_hz) {
            return Err(format!(
                "player.step_hz ({}) must be a multiple of the tick rate ({tick_hz} Hz)",
                self.step_hz
            ));
        }
        let positive = [
            ("width", self.width),
            ("height", self.height),
            ("walk_speed", self.walk_speed),
            ("run_speed", self.run_speed),
            ("crouch_speed", self.crouch_speed),
            ("ground_accel", self.ground_accel),
            ("air_accel", self.air_accel),
            ("gravity", self.gravity),
            ("max_fall_speed", self.max_fall_speed),
            ("swim_speed", self.swim_speed),
            ("fluid_accel", self.fluid_accel),
            ("fly_max_speed", self.fly_max_speed),
        ];
        for (name, v) in positive {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("player.{name} must be positive ({v})"));
            }
        }
        let non_negative = [
            ("jump_height", self.jump_height),
            ("step_height", self.step_height),
            ("sink_speed", self.sink_speed),
        ];
        for (name, v) in non_negative {
            if !(v.is_finite() && v >= 0.0) {
                return Err(format!("player.{name} must not be negative ({v})"));
            }
        }
        if self.width >= 1.0 {
            return Err("player.width must stay under one block".into());
        }
        if !(self.eye_height > 0.0 && self.eye_height <= self.height) {
            return Err("player.eye_height must lie within the player's height".into());
        }
        if self.step_height >= 1.0 {
            return Err("player.step_height must stay under one block".into());
        }
        Ok(())
    }
}
