//! Geology tools: vertical sections as PNG, measured ore shares, underground temperature curves.
//!
//! ```text
//! cargo run -p nascio-voxel --release --bin geology -- section out/m5/world-42 --from 1000,2000 --to 9000,2000 --out out/section.png
//! cargo run -p nascio-voxel --release --bin geology -- stats out/m5/world-42
//! cargo run -p nascio-voxel --release --bin geology -- geotherm --out out/geotherm.png
//! cargo run -p nascio-voxel --release --bin geology -- depths out/m5/world-42
//! ```

// Tool output (colours, statistics): platform maths is fine here.
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use nascio_core::GeothermConfig;
use nascio_core::consts::WORLD_SIZE_F64;
use nascio_tools::section::{SectionSpec, SurfaceTemperature, render};
use nascio_voxel::geology::Class;
use nascio_voxel::{Terrain, VoxelConfig};
use nascio_worldgen::geology::setting;
use nascio_worldgen::plot::{Canvas, rgb, text_width};
use nascio_worldgen::rng::mix64;
use nascio_worldgen::world_io::World;

#[derive(Parser)]
#[command(name = "geology", about = "Nascio geology tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Vertical section along a line, as PNG.
    Section {
        world: PathBuf,
        /// Start `x,z` (m).
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
        from: Vec<f64>,
        /// End `x,z` (m).
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
        to: Vec<f64>,
        /// Top and bottom heights of the picture (m).
        #[arg(long, default_value_t = 2_000.0, allow_hyphen_values = true)]
        top: f64,
        #[arg(long, default_value_t = -7_000.0, allow_hyphen_values = true)]
        bottom: f64,
        #[arg(long, default_value_t = 1_400)]
        width: u32,
        #[arg(long, default_value_t = 700)]
        height: u32,
        #[arg(long, default_value = "")]
        title: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Measured shares of rock and ore in the first `depth` m under continental land.
    Stats {
        world: PathBuf,
        #[arg(long, default_value_t = 20_000)]
        columns: usize,
        #[arg(long, default_value_t = 500)]
        depth: i32,
    },
    /// Depth and rock temperature of the deposit bodies, per type (actual columns).
    Depths {
        world: PathBuf,
        /// Temperature that a type must not lie entirely beyond, °C.
        #[arg(long, default_value_t = 60.0)]
        limit: f64,
    },
    /// Underground temperature curves (ocean, continent, orogen), as PNG and a table.
    Geotherm {
        /// World whose `[geotherm]` is used (default parameters otherwise).
        #[arg(long)]
        world: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Section {
            world,
            from,
            to,
            top,
            bottom,
            width,
            height,
            title,
            out,
        } => {
            anyhow::ensure!(from.len() == 2 && to.len() == 2, "--from and --to take x,z");
            let w = World::load(&world)?;
            let terrain = Terrain::from_world(&w, VoxelConfig::default())?;
            let temp = SurfaceTemperature::from_world(&w);
            let catalog = w.catalog()?;
            let spec = SectionSpec {
                from: (from[0], from[1]),
                to: (to[0], to[1]),
                top_m: top,
                bottom_m: bottom,
                width,
                height,
                title,
            };
            render(&terrain, &temp, catalog.as_ref(), &spec).save(&out)?;
            println!("wrote {}", out.display());
        }
        Command::Stats {
            world,
            columns,
            depth,
        } => stats(&World::load(&world)?, columns, depth)?,
        Command::Depths { world, limit } => depths(&World::load(&world)?, limit)?,
        Command::Geotherm { world, out } => {
            let g = match world {
                Some(p) => World::load(&p)?.config.geotherm,
                None => GeothermConfig::default(),
            };
            geotherm(&g, &out)?;
        }
    }
    Ok(())
}

fn stats(world: &World, columns: usize, depth: i32) -> anyhow::Result<()> {
    let terrain = Terrain::from_world(world, VoxelConfig::default())?;
    let g = terrain.geology();
    let blocks = terrain.blocks();
    let builtin = nascio_worldgen::blocks::BUILTIN.len() as u16;
    let mut per_block: BTreeMap<u16, u64> = BTreeMap::new();
    let mut per_class: BTreeMap<&str, u64> = BTreeMap::new();
    let (mut rock, mut ore, mut sampled, mut tries) = (0u64, 0u64, 0usize, 0u64);
    while sampled < columns && tries < 50 * columns as u64 {
        tries += 1;
        let h = mix64(0xC0FF_EE00 ^ tries);
        let x = (h & 0xFFFF_FFFF) as f64 / 4_294_967_296.0 * WORLD_SIZE_F64;
        let z = (h >> 32) as f64 / 4_294_967_296.0 * WORLD_SIZE_F64;
        let col = terrain.column_geo(x, z);
        let top = terrain.height(x, z).floor() as i32;
        if top <= 0 || setting::oceanic(col.setting) || col.oceanic {
            continue;
        }
        sampled += 1;
        let e = 2.0;
        let slope = (terrain.height(x + e, z) - terrain.height(x - e, z))
            .hypot(terrain.height(x, z + e) - terrain.height(x, z - e))
            / (2.0 * e);
        let moho = terrain.moho_y(x, z, top);
        let (bx, bz) = (x.floor() as i32, z.floor() as i32);
        let bodies = g.bodies_near(x - 1.0, z - 1.0, 2.0);
        for d in 0..depth {
            let y = top - d;
            if y < moho {
                break;
            }
            let (b, class) = terrain.rock_block(&col, bx, bz, top, slope, moho, d);
            if class == Class::Soil {
                continue;
            }
            rock += 1;
            *per_class
                .entry(match class {
                    Class::Soil => "soil",
                    Class::Loose => "sand and gravel",
                    Class::Weathered => "weathered",
                    Class::Sediment => "sediment",
                    Class::Basement => "basement",
                })
                .or_default() += 1;
            let id = g.ore(&bodies, class, bx, y, bz, top).unwrap_or(b);
            if id >= builtin {
                ore += 1;
            }
            *per_block.entry(id).or_default() += 1;
        }
    }
    println!(
        "{sampled} continental land columns, first {depth} m: {rock} rock blocks, ore share {:.4} %",
        ore as f64 / rock.max(1) as f64 * 100.0
    );
    for (c, n) in &per_class {
        println!("  {c:<16} {:6.2} %", *n as f64 / rock.max(1) as f64 * 100.0);
    }
    println!("blocks:");
    for (id, n) in &per_block {
        println!(
            "  {:<16} {:9.4} %{}",
            blocks.def(*id).name,
            *n as f64 / rock.max(1) as f64 * 100.0,
            if *id >= builtin { "  (ore)" } else { "" }
        );
    }
    Ok(())
}

/// Per deposit type: depth of the top and bottom of its bodies below the rock surface, and
/// the rock temperature there, from the actual crust and surface temperature of each column.
fn depths(world: &World, limit: f64) -> anyhow::Result<()> {
    let terrain = Terrain::from_world(world, VoxelConfig::default())?;
    let temp = SurfaceTemperature::from_world(world);
    let catalog = world
        .catalog()?
        .ok_or_else(|| anyhow::anyhow!("world without deposit catalogue"))?;
    let bodies = world.bodies()?;
    println!(
        "{:<16} {:>6} {:>12} {:>12} {:>11} {:>11} {:>9} {:>10}",
        "type",
        "bodies",
        "top min (m)",
        "base max (m)",
        "T top min",
        "T base max",
        "top med T",
        format!("top <{limit:.0}C"),
    );
    let mut failures = Vec::new();
    for (kind, d) in catalog.deposits.iter().enumerate() {
        // (top depth, base depth, top temperature, base temperature)
        let mut rows: Vec<(f64, f64, f64, f64)> = bodies
            .iter()
            .filter(|b| usize::from(b.kind) == kind)
            .map(|b| {
                let top = terrain.height(b.x, b.z);
                let ti = top.floor() as i32;
                let water_top = f64::from(terrain.water_top(b.x, b.z));
                let moho = f64::from(terrain.moho_y(b.x, b.z, ti));
                let c = f64::from(b.c);
                let (d0, d1) = if b.shape.follows_surface() {
                    (b.y - c, b.y + c)
                } else {
                    let (lo, hi) = b.y_range();
                    (top - hi, top - lo)
                };
                let (d0, d1) = (d0.max(0.0), d1.max(0.0));
                let t = |d: f64| temp.rock(b.x, b.z, top, water_top, moho, d);
                (d0, d1, t(d0), t(d1))
            })
            .collect();
        if rows.is_empty() {
            println!("{:<16} {:>6}", d.name, 0);
            continue;
        }
        let n = rows.len();
        let top_min = rows.iter().map(|r| r.0).fold(f64::MAX, f64::min);
        let base_max = rows.iter().map(|r| r.1).fold(0.0, f64::max);
        let t_top_min = rows.iter().map(|r| r.2).fold(f64::MAX, f64::min);
        let t_base_max = rows.iter().map(|r| r.3).fold(f64::MIN, f64::max);
        rows.sort_by(|a, b| a.2.total_cmp(&b.2));
        let median = rows[n / 2].2;
        let cool = rows.iter().filter(|r| r.2 < limit).count() as f64 / n as f64;
        println!(
            "{:<16} {n:>6} {top_min:>12.0} {base_max:>12.0} {t_top_min:>9.0} C {t_base_max:>9.0} C {median:>7.0} C {:>9.0} %",
            d.name,
            cool * 100.0
        );
        if cool < 0.5 {
            failures.push(d.name.clone());
        }
    }
    if failures.is_empty() {
        println!("every type has most of its bodies starting below {limit:.0} C");
    } else {
        println!(
            "types with most bodies starting beyond {limit:.0} C: {}",
            failures.join(", ")
        );
    }
    Ok(())
}

fn geotherm(g: &GeothermConfig, out: &std::path::Path) -> anyhow::Result<()> {
    // (label, surface °C, crust thickness m, colour)
    let profiles = [
        (
            "ocean (sea floor 2 C, crust 900 m)",
            2.0,
            900.0,
            [40, 110, 200],
        ),
        (
            "continent (10 C, crust 3 500 m)",
            10.0,
            3_500.0,
            [40, 150, 60],
        ),
        ("orogen (0 C, crust 5 500 m)", 0.0, 5_500.0, [180, 60, 40]),
    ];
    let (ml, mr, mt, mb) = (80i32, 30i32, 50i32, 60i32);
    let (w, h) = (900i32, 640i32);
    let max_d = 6_000.0;
    let ink = rgb([30, 30, 34]);
    let mut cv = Canvas::new(
        (w + ml + mr) as u32,
        (h + mt + mb) as u32,
        rgb([250, 250, 246]),
    );
    let px = |t: f64| ml as f64 + t / 1_250.0 * w as f64;
    let py = |d: f64| mt as f64 + d / max_d * h as f64;
    // Grid.
    let grid = rgb([222, 222, 216]);
    for t in (0..=1_200).step_by(100) {
        cv.vline(px(f64::from(t)) as i32, mt, mt + h, 1, grid);
        let s = format!("{t}");
        cv.text(
            px(f64::from(t)) as i32 - text_width(&s, 1) / 2,
            mt + h + 8,
            &s,
            1,
            ink,
        );
    }
    for d in (0..=6_000).step_by(500) {
        cv.hline(ml, ml + w, py(f64::from(d)) as i32, 1, grid);
        let s = format!("{d}");
        cv.text(
            ml - 8 - text_width(&s, 1),
            py(f64::from(d)) as i32 - 3,
            &s,
            1,
            ink,
        );
    }
    cv.text(ml, mt + h + 26, "rock temperature (C)", 1, ink);
    cv.text(4, mt - 14, "depth below the rock surface (m)", 1, ink);
    cv.text(
        ml,
        12,
        "underground temperature: gentle near the surface, steep near the Moho",
        2,
        ink,
    );
    // 50 C at 500 m mark.
    cv.rect(px(50.0) as i32 - 3, py(500.0) as i32 - 3, 7, 7, ink);
    cv.text(
        px(50.0) as i32 + 8,
        py(500.0) as i32 - 3,
        "50 C at 500 m",
        1,
        ink,
    );
    println!(
        "depth m   {}",
        profiles
            .map(|p| format!("{:>12}", p.0.split(' ').next().unwrap_or("")))
            .join("")
    );
    for d in [
        0.0, 100.0, 250.0, 500.0, 750.0, 1_000.0, 2_000.0, 3_000.0, 4_000.0, 5_000.0,
    ] {
        let row: Vec<String> = profiles
            .iter()
            .map(|&(_, ts, crust, _)| {
                if d <= crust {
                    format!("{:>10.0} C", g.temperature(ts, d, crust))
                } else {
                    format!("{:>12}", "magma")
                }
            })
            .collect();
        println!("{d:>7.0}   {}", row.join(""));
    }
    for (k, &(label, ts, crust, color)) in profiles.iter().enumerate() {
        let pts: Vec<(f64, f64)> = (0..=400)
            .map(|i| {
                let d = crust * f64::from(i) / 400.0;
                (px(g.temperature(ts, d, crust)), py(d))
            })
            .collect();
        cv.polyline(&pts, 2, rgb(color), None);
        // Moho marker.
        cv.hline(
            px(g.moho_c) as i32 - 30,
            px(g.moho_c) as i32 + 10,
            py(crust) as i32,
            2,
            rgb(color),
        );
        let ly = mt + h - 100 + 18 * k as i32;
        cv.rect(ml + 420, ly, 16, 4, rgb(color));
        cv.text(ml + 444, ly - 2, label, 1, ink);
        println!(
            "{label}: 60 C at {:.0} m, 100 C at {:.0} m, 300 C at {:.0} m, gradient at the surface {:.3} C/m, above the Moho {:.2} C/m",
            g.depth_of(ts, crust, 60.0),
            g.depth_of(ts, crust, 100.0),
            g.depth_of(ts, crust, 300.0),
            g.temperature(ts, 1.0, crust) - ts,
            g.moho_c - g.temperature(ts, crust - 1.0, crust),
        );
    }
    cv.text(
        ml + 420,
        mt + h - 100 + 18 * 3,
        "short bars: Moho (1 200 C, magma below)",
        1,
        ink,
    );
    cv.save(out)?;
    println!("wrote {}", out.display());
    Ok(())
}
