//! Share of each surface block (top block, snow cover, sea and lake ice) over a regular sample
//! of the world, every 200 m.
//!
//! `cargo run -p nascio-voxel --release --example surface_stats -- out/m6/world-42`

use std::path::PathBuf;

use nascio_core::consts::WORLD_SIZE;
use nascio_voxel::{Terrain, VoxelConfig};
use nascio_worldgen::world_io::World;

const STEP: i32 = 200;

fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or("out/m6/world-42".into()));
    let world = World::load(&dir)?;
    let t = Terrain::from_world(&world, VoxelConfig::default())?;
    let blocks = t.blocks();
    let mut count = vec![0usize; blocks.len()];
    let mut n = 0usize;
    for z in (0..WORLD_SIZE).step_by(STEP as usize) {
        for x in (0..WORLD_SIZE).step_by(STEP as usize) {
            let (fx, fz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
            let h = t.height(fx, fz);
            let top = h.floor() as i32;
            let wt = t.water_top(fx, fz);
            let b = if top + 1 < wt {
                t.water_block(&t.column_geo(fx, fz), wt - 1, wt)
            } else {
                let slope = (t.height(fx + 1.0, fz) - t.height(fx - 1.0, fz))
                    .hypot(t.height(fx, fz + 1.0) - t.height(fx, fz - 1.0))
                    / 2.0;
                t.top_block(x, z, top, slope)
            };
            count[usize::from(b)] += 1;
            n += 1;
        }
    }
    let mut rows: Vec<_> = count.iter().enumerate().filter(|(_, c)| **c > 0).collect();
    rows.sort_by(|a, b| b.1.cmp(a.1));
    for (id, c) in rows {
        println!(
            "{:<14} {:6.2} %",
            blocks.def(id as u16).name,
            100.0 * *c as f64 / n as f64
        );
    }
    Ok(())
}
