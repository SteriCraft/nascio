//! `nascio-worldgen` command-line interface.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use nascio_core::astro::AstroConfig;
use nascio_worldgen::config::{GENERATOR_VERSION, Scale, WorldConfig};
use nascio_worldgen::generate::generate_to_disk;
use nascio_worldgen::progress::Progress;
use nascio_worldgen::sun_curves::{self, Hemisphere};

#[derive(Parser)]
#[command(name = "nascio-worldgen", version, about = "Nascio world generator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum Side {
    North,
    South,
}

#[derive(Subcommand)]
enum Command {
    /// Plot the sun elevation over one game day for several seam distances.
    SunCurves {
        /// Days of the year (0..365), one panel each.
        #[arg(long, value_delimiter = ',', default_value = "172,81,355")]
        day: Vec<u32>,
        /// Seam distances s in metres (at most 8).
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "0,1000,1500,2000,3000,4125,20000,50000"
        )]
        s: Vec<f64>,
        /// Side of the seam.
        #[arg(long, value_enum, default_value = "north")]
        side: Side,
        /// Output PNG.
        #[arg(long, default_value = "out/sun-curves.png")]
        out: PathBuf,
    },
    /// Generate a world: world.toml, layers/*.bin and previews/*.png.
    Generate {
        /// World seed (below 2^63).
        #[arg(long)]
        seed: u64,
        /// Resolution divisor: 1 (full), 5 or 25.
        #[arg(long, default_value_t = 5)]
        scale: u32,
        /// Output directory (default: out/world-<seed>).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Optional TOML file overriding parameters (its seed and scale are replaced by the CLI).
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::SunCurves { day, s, side, out } => {
            anyhow::ensure!(
                !s.is_empty() && s.len() <= 8,
                "between 1 and 8 seam distances"
            );
            anyhow::ensure!(
                s.iter().all(|v| (0.0..=50_000.0).contains(v)),
                "seam distances must lie in [0, 50000]"
            );
            anyhow::ensure!(day.iter().all(|&d| d < 365), "days must lie in [0, 365)");
            let hemisphere = match side {
                Side::North => Hemisphere::North,
                Side::South => Hemisphere::South,
            };
            let cfg = AstroConfig::default();
            let steps = 24 * 60;
            let panels: Vec<_> = day
                .iter()
                .map(|&d| {
                    let curves = sun_curves::sample(&cfg, d, &s, hemisphere, steps);
                    sun_curves::print_table(d, &curves, steps);
                    (format!("Day N = {d}"), curves)
                })
                .collect();
            sun_curves::render(&panels, hemisphere, &out)?;
            println!("wrote {}", out.display());
        }
        Command::Generate {
            seed,
            scale,
            out,
            config,
        } => {
            let scale = Scale::try_from(scale).map_err(anyhow::Error::msg)?;
            let mut cfg = match config {
                Some(p) => {
                    let mut c = WorldConfig::from_toml(&std::fs::read_to_string(&p)?)?;
                    c.seed = seed;
                    c.scale = scale;
                    c
                }
                None => WorldConfig::new(seed, scale),
            };
            cfg.generator_version = GENERATOR_VERSION;
            let out = out.unwrap_or_else(|| PathBuf::from(format!("out/world-{seed}")));
            let progress = Progress::new();
            let stop = AtomicBool::new(false);
            let result = std::thread::scope(|s| {
                s.spawn(|| {
                    let mut err = std::io::stderr();
                    while !stop.load(Ordering::Relaxed) {
                        let _ = write!(err, "\r{:<72}", progress.line());
                        let _ = err.flush();
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    let _ = writeln!(err, "\r{:<72}", progress.line());
                });
                let r = generate_to_disk(&cfg, &out, &progress);
                stop.store(true, Ordering::Relaxed);
                r
            });
            let (g, previews) = result?;
            let (world, tect, ero) = (&g.world, &g.tectonics, &g.erosion);
            println!("{}", tect.report);
            println!("{}", ero.report);
            for (stage, t) in progress.snapshot().done {
                println!("{}: {t:.2} s", stage.name());
            }
            println!("generated in {:.2} s", progress.snapshot().elapsed_s);
            println!("hash: {}", world.hash());
            println!(
                "wrote {} ({} layers, {} previews)",
                out.display(),
                world.layers.len(),
                previews
            );
            if !tect.report.continental_fraction_ok {
                eprintln!("warning: final continental fraction is out of tolerance");
            }
        }
    }
    Ok(())
}
