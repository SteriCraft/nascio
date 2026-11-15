//! The generation pipeline. Only tectonics exists so far (M1).

use crate::config::WorldConfig;
use crate::tectonics::{TectonicOutput, Tectonics};
use crate::world_io::World;

/// Generates a world from a config. Pure and deterministic.
pub fn generate(cfg: &WorldConfig) -> anyhow::Result<(World, TectonicOutput)> {
    cfg.validate()?;
    let mut t = Tectonics::new(cfg);
    t.run();
    let out = t.finish();
    let world = World {
        config: cfg.clone(),
        layers: out.layers(),
    };
    Ok((world, out))
}
