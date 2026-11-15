//! The generation pipeline: tectonics (M1), then erosion and hydrography (M3).

use crate::config::WorldConfig;
use crate::erosion::{self, ErosionOutput};
use crate::tectonics::{TectonicOutput, Tectonics};
use crate::world_io::World;

/// Maximum number of tectonic runs used to calibrate the continental share.
const MAX_CALIBRATION_RUNS: u32 = 4;
/// Calibration stops once the final share is this close to the target.
const CALIBRATION_EPS: f64 = 0.005;

/// Generates a world from a config. Pure and deterministic.
///
/// Collisions consume some continental crust, by an amount that depends on the seed. The
/// tectonic run is therefore repeated (at most [`MAX_CALIBRATION_RUNS`] times), shifting the
/// initial continental share by the observed error, and the run whose *final* share is closest
/// to `continental_fraction` is kept.
pub fn generate(cfg: &WorldConfig) -> anyhow::Result<(World, TectonicOutput)> {
    let (world, tect, _) = generate_full(cfg)?;
    Ok((world, tect))
}

/// [`generate`], also returning the erosion output and its timings.
pub fn generate_full(cfg: &WorldConfig) -> anyhow::Result<(World, TectonicOutput, ErosionOutput)> {
    cfg.validate()?;
    let target = cfg.tectonics.continental_fraction;
    let mut initial = target;
    let mut best: Option<(f64, Tectonics)> = None;
    let mut runs = 0;
    for _ in 0..MAX_CALIBRATION_RUNS {
        runs += 1;
        let mut t = Tectonics::with_initial_fraction(cfg, initial);
        t.run();
        let err = t.continental_fraction() - target;
        let better = best.as_ref().is_none_or(|(e, _)| err.abs() < e.abs());
        if better {
            best = Some((err, t));
        }
        if err.abs() < CALIBRATION_EPS {
            break;
        }
        initial = (initial - err).clamp(0.01, 0.95);
    }
    let (_, t) = best.expect("at least one run");
    let mut out = t.finish();
    out.report.calibration_runs = runs;
    let ero = erosion::run(cfg, &out);
    let mut layers = out.layers();
    layers.extend(ero.layers());
    let world = World {
        config: cfg.clone(),
        layers,
    };
    Ok((world, out, ero))
}
