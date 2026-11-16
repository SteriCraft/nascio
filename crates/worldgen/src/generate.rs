//! The generation pipeline: tectonics (M1), erosion (M3), then climate, final hydrology and
//! biomes (M4).

use crate::climate::{self, ClimateOutput};
use crate::config::WorldConfig;
use crate::erosion::{self, ErosionOutput};
use crate::tectonics::{TectonicOutput, Tectonics};
use crate::world_io::World;
use crate::{biomes, grid::PeriodicGrid};

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
    let g = generate_full(cfg)?;
    Ok((g.world, g.tectonics))
}

/// Every stage's output.
pub struct Generated {
    pub world: World,
    pub tectonics: TectonicOutput,
    pub erosion: ErosionOutput,
    pub climate: ClimateOutput,
    pub biome: PeriodicGrid<u8>,
    /// `(stage, seconds)` of the stages after erosion.
    pub timings: Vec<(&'static str, f64)>,
}

/// [`generate`], also returning every stage's output.
pub fn generate_full(cfg: &WorldConfig) -> anyhow::Result<Generated> {
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
    let mut ero = erosion::run(cfg, &out);
    let mut timings = Vec::new();
    let mut timed = |name: &'static str, start: std::time::Instant| {
        timings.push((name, start.elapsed().as_secs_f64()));
    };
    let start = std::time::Instant::now();
    let sea: Vec<bool> = ero.basin_id.data().iter().map(|&b| b == 0).collect();
    let clim = climate::run(cfg, &ero.elevation, &sea);
    timed("climate", start);
    let start = std::time::Instant::now();
    erosion::final_hydrology(cfg, &mut ero, &clim.precipitation.x0);
    timed("final hydrology", start);
    let start = std::time::Instant::now();
    let biome = biomes::biomes(cfg, &ero, &clim);
    timed("biomes", start);
    let mut layers = out.layers();
    layers.extend(ero.layers());
    layers.extend(clim.layers());
    layers.push(biomes::layer(&biome));
    let world = World {
        config: cfg.clone(),
        layers,
    };
    Ok(Generated {
        world,
        tectonics: out,
        erosion: ero,
        climate: clim,
        biome,
        timings,
    })
}
