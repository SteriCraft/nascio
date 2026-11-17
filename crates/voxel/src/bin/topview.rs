//! Top views of generated chunks, as PNG (one pixel per block).
//!
//! For each site, the highest non-air block of every column of a square patch, coloured as
//! the viewer does (registry colour, climate tint for grass), water blended over its bed by
//! depth, with a hill shade from the block heights. Sites are fixed by `--seed` (the same as
//! `chunkbench`) or given with `--at x,z`.
//!
//! ```text
//! cargo run -p nascio-voxel --release --bin topview -- out/m6/world-42 --out out/m6/topview
//! cargo run -p nascio-voxel --release --bin topview -- out/m6/world-42 --at 51200,7300 --side 16
//! ```

// Tool output (colours, statistics): platform maths is fine here.
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;

use clap::Parser;
use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS};
use nascio_core::torus::wrap;
use nascio_voxel::mesh::water_alpha;
use nascio_voxel::rivers::RiverHit;
use nascio_voxel::{ColumnSet, GenConfig, Terrain};
use nascio_worldgen::rng::mix64;
use nascio_worldgen::world_io::World;
use rayon::prelude::*;

#[derive(Parser)]
#[command(name = "topview", about = "Nascio top views of generated chunks (PNG)")]
struct Cli {
    world: PathBuf,
    /// Output directory.
    #[arg(long, default_value = "out/topview")]
    out: PathBuf,
    /// Number of sites spread over the world (ignored with `--at`).
    #[arg(long, default_value_t = 8)]
    sites: u64,
    /// Seed choosing the sites.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Explicit sites, world block coordinates `x,z` (repeatable).
    #[arg(long, value_parser = parse_xz)]
    at: Vec<(i32, i32)>,
    /// Adds the `N` deepest river channels found on a 200 m grid (at least 5 km apart).
    #[arg(long, default_value_t = 0)]
    rivers: usize,
    /// Side of the patch, in chunk columns (32 blocks each).
    #[arg(long, default_value_t = 32)]
    side: i32,
    /// TOML file with a `[voxel]` section (needs `--dev-override`; outputs are marked).
    #[arg(long)]
    voxel_config: Option<PathBuf>,
    /// Allow `--voxel-config`.
    #[arg(long)]
    dev_override: bool,
}

fn parse_xz(s: &str) -> Result<(i32, i32), String> {
    let (x, z) = s.split_once(',').ok_or("expected x,z")?;
    let p = |v: &str| v.trim().parse::<i32>().map_err(|e| e.to_string());
    Ok((p(x)?, p(z)?))
}

fn distance(a: (i32, i32), b: (i32, i32)) -> f64 {
    let dx = f64::from(nascio_core::torus::delta(a.0, b.0));
    let dz = f64::from(nascio_core::torus::delta(a.1, b.1));
    dx.hypot(dz)
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

/// Top block height and linear colour of the 32² columns of chunk column `(cx, cz)`.
fn column(terrain: &Terrain, cx: i32, cz: i32) -> Vec<(i32, [f32; 3])> {
    let col = ColumnSet::new(terrain, cx, cz);
    let blocks = terrain.blocks();
    let water = blocks.ids.water;
    let y_max = (col.chunk_range(0).end() + 1) * CHUNK_SIZE - 1;
    let mut out = Vec::with_capacity(1024);
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let (wx, wz) = (
                f64::from(wrap(cx * CHUNK_SIZE + x)),
                f64::from(wrap(cz * CHUNK_SIZE + z)),
            );
            let mut y = y_max;
            let mut b = col.block(terrain, x, y, z);
            while b == nascio_voxel::block::AIR && y > col.top(x, z) {
                y -= 1;
                b = col.block(terrain, x, y, z);
            }
            let colour = |b, y: i32| terrain.surface_color(b, wx, wz, f64::from(y));
            let c = if b == water {
                // Water over its bed, blended by depth as the mesher does.
                let bed = col.top(x, z);
                let floor = colour(col.block(terrain, x, bed, z), bed);
                let depth = (y - bed).clamp(0, 255) as u8;
                let a = water_alpha(depth);
                let w = blocks.linear(water);
                std::array::from_fn(|k| w[k] * a + floor[k] * (1.0 - a))
            } else {
                colour(b, y)
            };
            out.push((y, c));
        }
    }
    out
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let world = World::load(&cli.world)?;
    let gen_cfg = GenConfig::load(cli.voxel_config.as_deref(), cli.dev_override)?;
    let terrain = Terrain::from_world(&world, gen_cfg.voxel.clone())?;
    std::fs::create_dir_all(&cli.out)?;

    let sites: Vec<(i32, i32)> = if cli.at.is_empty() {
        (0..cli.sites)
            .map(|s| {
                let h = mix64(cli.seed ^ mix64(s + 1));
                let cx = (h % CHUNKS_PER_AXIS as u64) as i32;
                let cz = ((h >> 32) % CHUNKS_PER_AXIS as u64) as i32;
                (cx, cz)
            })
            .collect()
    } else {
        cli.at
            .iter()
            .map(|&(x, z)| {
                (
                    wrap(x).div_euclid(CHUNK_SIZE) - cli.side / 2,
                    wrap(z).div_euclid(CHUNK_SIZE) - cli.side / 2,
                )
            })
            .collect()
    };

    let mut sites = sites;
    if cli.rivers > 0 {
        let step = 200;
        let cells = nascio_core::consts::WORLD_SIZE / step;
        let mut hits: Vec<(i32, i32, i32)> = (0..cells * cells)
            .into_par_iter()
            .filter_map(|k| {
                let (x, z) = ((k % cells) * step, (k / cells) * step);
                match terrain.river(f64::from(x), f64::from(z)) {
                    Some(RiverHit::Channel { water_top, bed }) => Some((water_top - bed, x, z)),
                    _ => None,
                }
            })
            .collect();
        hits.sort_unstable_by(|a, b| b.cmp(a));
        let mut picked: Vec<(i32, i32)> = Vec::new();
        for &(d, x, z) in &hits {
            if picked.len() == cli.rivers {
                break;
            }
            if picked
                .iter()
                .all(|&(px, pz)| distance((px, pz), (x, z)) > 5_000.0)
            {
                println!("river at ({x}, {z}), depth {d}");
                picked.push((x, z));
            }
        }
        sites.extend(picked.iter().map(|&(x, z)| {
            (
                x.div_euclid(CHUNK_SIZE) - cli.side / 2,
                z.div_euclid(CHUNK_SIZE) - cli.side / 2,
            )
        }));
    }

    let n = cli.side;
    let px = (n * CHUNK_SIZE) as usize;
    for (i, &(cx0, cz0)) in sites.iter().enumerate() {
        let keys: Vec<(i32, i32)> = (0..n * n).map(|k| (k % n, k / n)).collect();
        let cols: Vec<_> = keys
            .par_iter()
            .map(|&(dx, dz)| column(&terrain, cx0 + dx, cz0 + dz))
            .collect();
        let mut h = vec![0i32; px * px];
        let mut c = vec![[0f32; 3]; px * px];
        for (&(dx, dz), col) in keys.iter().zip(&cols) {
            for (k, &(y, rgb)) in col.iter().enumerate() {
                let x = dx as usize * 32 + k % 32;
                let z = dz as usize * 32 + k / 32;
                h[x + px * z] = y;
                c[x + px * z] = rgb;
            }
        }
        let mut img = image::RgbImage::new(px as u32, px as u32);
        let (mut lo, mut hi) = (i32::MAX, i32::MIN);
        for z in 0..px {
            for x in 0..px {
                let y = h[x + px * z];
                lo = lo.min(y);
                hi = hi.max(y);
                // Hill shade: light from the north-west.
                let up = h[x.saturating_sub(1) + px * z.saturating_sub(1)];
                let shade = (1.0 + 0.15 * (up - y) as f32).clamp(0.55, 1.35);
                let rgb = c[x + px * z];
                img.put_pixel(
                    x as u32,
                    z as u32,
                    image::Rgb(rgb.map(|v| linear_to_srgb(v * shade))),
                );
            }
        }
        let (x0, z0) = (wrap(cx0 * CHUNK_SIZE), wrap(cz0 * CHUNK_SIZE));
        let path = gen_cfg.mark_path(cli.out.join(format!("site-{i}_{x0}_{z0}.png")));
        img.save(&path)?;
        println!(
            "{}: {px}x{px} blocks from ({x0}, {z0}), heights {lo}..{hi}",
            path.display()
        );
    }
    Ok(())
}
