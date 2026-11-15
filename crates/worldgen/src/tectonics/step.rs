//! One simulation step (placeholder until the advection is implemented).

use super::Tectonics;

impl Tectonics {
    /// Advances the simulation by one step.
    pub fn step(&mut self) {
        self.step_index += 1;
    }
}
