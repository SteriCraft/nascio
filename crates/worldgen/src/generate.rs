//! The generation pipeline: tectonics (M1), erosion (M3), then climate, final hydrology and
//! biomes (M4).

use crate::blocks::BlockRegistry;
use crate::climate::{self, ClimateOutput};
use crate::config::WorldConfig;
use crate::deposits::{self, Catalog, DepositsOutput};
use crate::erosion::{self, ErosionOutput};
use crate::geology::{self, GeologyOutput};
use crate::progress::{Progress, Stage};
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
    pub geology: GeologyOutput,
    pub deposits: DepositsOutput,
    /// `(stage, seconds)` of the stages after erosion.
    pub timings: Vec<(&'static str, f64)>,
}

/// [`generate`], also returning every stage's output.
pub fn generate_full(cfg: &WorldConfig) -> anyhow::Result<Generated> {
    generate_full_with(cfg, &Catalog::default_catalog(), &Progress::new())
}

/// [`generate_full`], reporting its progress to `progress` (display only: the result does not
/// depend on it).
pub fn generate_full_with(
    cfg: &WorldConfig,
    catalog: &Catalog,
    progress: &Progress,
) -> anyhow::Result<Generated> {
    cfg.validate()?;
    let registry = BlockRegistry::with_catalog(catalog)?;
    progress.begin(Stage::Tectonics);
    let target = cfg.tectonics.continental_fraction;
    let mut initial = target;
    let mut best: Option<(f64, Tectonics)> = None;
    let mut runs = 0;
    for _ in 0..MAX_CALIBRATION_RUNS {
        runs += 1;
        progress.detail(format!("calibration run {runs}"));
        let mut t = Tectonics::with_initial_fraction(cfg, initial);
        // Unknown number of runs: each one fills half of what is left of the bar.
        let done = 1.0 - 0.5f64.powi(runs as i32 - 1);
        t.run_with(|f| progress.set(done + (1.0 - done) * 0.5 * f));
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
    progress.begin(Stage::Erosion);
    let mut ero = erosion::run(cfg, &out, progress);
    let mut timings = Vec::new();
    let mut timed = |name: &'static str, start: std::time::Instant| {
        timings.push((name, start.elapsed().as_secs_f64()));
    };
    progress.begin(Stage::Climate);
    let start = std::time::Instant::now();
    let sea: Vec<bool> = ero.basin_id.data().iter().map(|&b| b == 0).collect();
    let clim = climate::run(cfg, &ero.elevation, &sea, progress);
    timed("climate", start);
    progress.begin(Stage::Hydrology);
    let start = std::time::Instant::now();
    erosion::final_hydrology(cfg, &mut ero, &clim.precipitation.x0);
    timed("final hydrology", start);
    progress.begin(Stage::Biomes);
    let start = std::time::Instant::now();
    let biome = biomes::biomes(cfg, &ero, &clim);
    timed("biomes", start);
    progress.begin(Stage::Geology);
    let start = std::time::Instant::now();
    let geo = geology::run(cfg, &out, &ero, &clim, progress);
    timed("geology", start);
    progress.begin(Stage::Deposits);
    let start = std::time::Instant::now();
    let heat = deposits::Heat {
        moho_depth: &out.moho_depth,
        air_c: &clim.temperature.x0,
        geotherm: cfg.geotherm,
    };
    let dep = deposits::run(cfg.seed, catalog, &geo, &ero, heat, progress);
    timed("deposits", start);
    let mut layers = out.layers();
    layers.extend(ero.layers());
    layers.extend(clim.layers());
    layers.push(biomes::layer(&biome));
    layers.extend(geo.layers());
    layers.extend(dep.layers());
    let files = [
        (
            "deposits.toml".to_string(),
            catalog.text.clone().into_bytes(),
        ),
        ("blocks.toml".to_string(), registry.to_toml()?.into_bytes()),
        (
            "deposits.bin".to_string(),
            deposits::bodies_to_bytes(&dep.bodies),
        ),
    ]
    .into();
    let world = World {
        config: cfg.clone(),
        layers,
        files,
    };
    Ok(Generated {
        world,
        tectonics: out,
        erosion: ero,
        climate: clim,
        biome,
        geology: geo,
        deposits: dep,
        timings,
    })
}

/// Generates a world and writes it under `dir` (`world.toml`, layers, previews), reporting the
/// saving and preview stages too, then [`Progress::finish`] (or [`Progress::fail`]).
pub fn generate_to_disk(
    cfg: &WorldConfig,
    catalog: &Catalog,
    dir: &std::path::Path,
    progress: &Progress,
) -> anyhow::Result<(Generated, usize)> {
    let run = || -> anyhow::Result<(Generated, usize)> {
        let g = generate_full_with(cfg, catalog, progress)?;
        progress.begin(Stage::Saving);
        g.world.save(dir)?;
        progress.begin(Stage::Previews);
        let previews =
            crate::preview::write_previews(&g.world, &g.tectonics.plates, &dir.join("previews"))?;
        Ok((g, previews.len()))
    };
    match run() {
        Ok(r) => {
            progress.finish();
            Ok(r)
        }
        Err(e) => {
            progress.fail(format!("{e:#}"));
            Err(e)
        }
    }
}
