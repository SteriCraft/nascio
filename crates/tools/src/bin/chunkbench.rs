//! Chunk generation and meshing benchmark, on 1 core and on N cores.
//!
//! Generates and meshes square patches of chunk columns at sites spread over the world
//! (fixed by `--seed`), the way the viewer does: columns (`ColumnSet` + tint), padded chunks,
//! meshes. No target: the numbers are a reference for later changes.
//!
//! ```text
//! cargo run -p nascio-voxel --release --bin chunkbench -- out/m6/world-42
//! cargo run -p nascio-voxel --release --bin chunkbench -- out/m6/world-42 --sites 16 --side 6 --threads 1,4,12
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use nascio_core::consts::CHUNKS_PER_AXIS;
use nascio_protocol::chunk::Column;
use nascio_voxel::{ColumnSet, GenConfig, Terrain, mesh_chunk};
use nascio_worldgen::rng::mix64;
use nascio_worldgen::world_io::World;
use rayon::prelude::*;

#[derive(Parser)]
#[command(
    name = "chunkbench",
    about = "Nascio chunk generation and meshing benchmark"
)]
struct Cli {
    world: PathBuf,
    /// Number of sites spread over the world.
    #[arg(long, default_value_t = 8)]
    sites: u64,
    /// Side of the square patch of chunk columns at each site.
    #[arg(long, default_value_t = 4)]
    side: i32,
    /// Seed choosing the sites.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Thread counts to run (default: 1 and all cores).
    #[arg(long, value_delimiter = ',')]
    threads: Vec<usize>,
    /// TOML file with a `[voxel]` section (needs `--dev-override`; outputs are marked).
    #[arg(long)]
    voxel_config: Option<PathBuf>,
    /// Allow `--voxel-config`.
    #[arg(long)]
    dev_override: bool,
    /// Sites given in world metres `x,z` (repeatable), instead of the drawn ones.
    #[arg(long, value_parser = parse_xz)]
    at: Vec<(i32, i32)>,
}

fn parse_xz(s: &str) -> Result<(i32, i32), String> {
    let (x, z) = s.split_once(',').ok_or("expected x,z")?;
    let p = |v: &str| v.trim().parse::<f64>().map_err(|e| e.to_string());
    Ok((p(x)? as i32, p(z)? as i32))
}

/// Compressed size of a column as the server sends it (codec v2, zstd, no edits), bytes.
fn column_bytes(terrain: &Terrain, cx: i32, cz: i32) -> anyhow::Result<usize> {
    let col = ColumnSet::new(terrain, cx, cz);
    let range = col.chunk_range(terrain.config().underground_chunks);
    let chunks = range
        .clone()
        .map(|cy| {
            let mut c = col.padded_chunk(terrain, cy).inner();
            c.compact();
            c
        })
        .collect();
    let mut tint = terrain.chunk_tint(cx, cz);
    tint.water_depth = col.water_depths();
    let bytes = Column {
        cx,
        cz,
        cy_min: *range.start(),
        version: 0,
        chunks,
        tint,
    }
    .compress()?;
    Ok(bytes.len())
}

/// Times summed over the work items (CPU time), and counts.
#[derive(Default, Clone, Copy)]
struct Tally {
    columns: Duration,
    generate: Duration,
    mesh: Duration,
    chunks: u64,
    meshed: u64,
    quads: u64,
}

impl Tally {
    fn add(self, o: Self) -> Self {
        Self {
            columns: self.columns + o.columns,
            generate: self.generate + o.generate,
            mesh: self.mesh + o.mesh,
            chunks: self.chunks + o.chunks,
            meshed: self.meshed + o.meshed,
            quads: self.quads + o.quads,
        }
    }
}

/// Generates and meshes one chunk column, as the viewer does.
fn column(terrain: &Terrain, cx: i32, cz: i32) -> Tally {
    let t0 = Instant::now();
    let col = ColumnSet::new(terrain, cx, cz);
    let mut tint = terrain.chunk_tint(cx, cz);
    tint.water_depth = col.water_depths();
    let mut t = Tally {
        columns: t0.elapsed(),
        ..Tally::default()
    };
    let underground = terrain.config().underground_chunks;
    for cy in col.chunk_range(underground) {
        if !col.may_have_faces(cy) {
            continue;
        }
        let t1 = Instant::now();
        let p = col.padded_chunk(terrain, cy);
        let t2 = Instant::now();
        let m = mesh_chunk(&p, terrain.blocks(), &tint);
        t.generate += t2 - t1;
        t.mesh += t2.elapsed();
        t.chunks += 1;
        if !m.is_empty() {
            t.meshed += 1;
            t.quads += m.quad_count() as u64;
        }
    }
    t
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let t = Instant::now();
    let world = World::load(&cli.world)?;
    let gen_cfg = GenConfig::load(cli.voxel_config.as_deref(), cli.dev_override)?;
    let terrain = Terrain::from_world(&world, gen_cfg.voxel.clone())?;
    println!(
        "{}terrain loaded in {:.0} ms",
        gen_cfg.mark(),
        ms(t.elapsed())
    );

    let side = cli.side;
    let origins: Vec<(i32, i32)> = if cli.at.is_empty() {
        (0..cli.sites)
            .map(|s| {
                let h = mix64(cli.seed ^ mix64(s + 1));
                (
                    (h % CHUNKS_PER_AXIS as u64) as i32,
                    ((h >> 32) % CHUNKS_PER_AXIS as u64) as i32,
                )
            })
            .collect()
    } else {
        let c = nascio_core::consts::CHUNK_SIZE;
        cli.at
            .iter()
            .map(|&(x, z)| (x.div_euclid(c) - side / 2, z.div_euclid(c) - side / 2))
            .collect()
    };
    let keys: Vec<(i32, i32)> = origins
        .iter()
        .flat_map(|&(cx0, cz0)| (0..side * side).map(move |i| (cx0 + i % side, cz0 + i / side)))
        .map(|(cx, cz)| {
            (
                cx.rem_euclid(CHUNKS_PER_AXIS),
                cz.rem_euclid(CHUNKS_PER_AXIS),
            )
        })
        .collect();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads = if cli.threads.is_empty() {
        vec![1, cores]
    } else {
        cli.threads.clone()
    };
    println!(
        "{} sites x {}x{} columns = {} columns, {cores} cores available",
        origins.len(),
        cli.side,
        cli.side,
        keys.len()
    );

    // Warm-up (lazy tables, caches) on a column outside the measured set.
    column(&terrain, keys[0].0 + cli.side, keys[0].1);

    let mut base = None;
    for n in threads {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(n).build()?;
        let start = Instant::now();
        let tally = pool.install(|| {
            keys.par_iter()
                .map(|&(cx, cz)| column(&terrain, cx, cz))
                .reduce(Tally::default, Tally::add)
        });
        let wall = start.elapsed().as_secs_f64();
        let base_wall = *base.get_or_insert(wall);
        let cols = keys.len() as f64;
        let ch = tally.chunks.max(1) as f64;
        println!(
            "\n{n} thread(s): {:.2} s wall, x{:.2} vs first run",
            wall,
            base_wall / wall
        );
        println!(
            "  {:.1} columns/s, {:.0} chunks/s ({} chunks, {} with faces, {:.0} quads/chunk meshed)",
            cols / wall,
            ch / wall,
            tally.chunks,
            tally.meshed,
            tally.quads as f64 / tally.meshed.max(1) as f64
        );
        println!(
            "  CPU time: columns {:.2} ms/column · generation {:.2} ms/chunk · meshing {:.2} ms/chunk",
            ms(tally.columns) / cols,
            ms(tally.generate) / ch,
            ms(tally.mesh) / ch
        );
    }
    let sizes = keys
        .par_iter()
        .map(|&(cx, cz)| column_bytes(&terrain, cx, cz))
        .collect::<anyhow::Result<Vec<_>>>()?;
    println!(
        "
network: {:.0} bytes/column compressed (mean), {} max",
        sizes.iter().sum::<usize>() as f64 / sizes.len() as f64,
        sizes.iter().max().unwrap_or(&0)
    );
    Ok(())
}
