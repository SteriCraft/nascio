//! Top view of the surface blocks and tree trunks around a point, 1 px per metre.
//! `cargo run -p nascio-voxel --release --example veg_map -- x z out.png`

// Tool output (colours, statistics): platform maths is fine here.
#![allow(clippy::disallowed_methods)]

use nascio_voxel::{Terrain, VoxelConfig};
use nascio_worldgen::world_io::World;

const N: i32 = 768;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (cx, cz): (i32, i32) = (a[1].parse()?, a[2].parse()?);
    let world = World::load(std::path::Path::new("out/m6/world-42"))?;
    let t = Terrain::from_world(&world, VoxelConfig::default())?;
    let (x0, z0) = (cx - N / 2, cz - N / 2);
    let mut img = image::RgbImage::new(N as u32, N as u32);
    for j in 0..N {
        for i in 0..N {
            let (x, z) = (x0 + i, z0 + j);
            let (fx, fz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
            let h = t.height(fx, fz);
            let top = h.floor() as i32;
            let rgb = if t.water_top(fx, fz) > top + 1 {
                [40, 90, 150]
            } else {
                let slope = (t.height(fx + 1.0, fz) - t.height(fx - 1.0, fz))
                    .hypot(t.height(fx, fz + 1.0) - t.height(fx, fz - 1.0))
                    / 2.0;
                let b = t.top_block(x, z, top, slope);
                let c = if b == t.blocks().ids.grass {
                    t.surface_color(b, fx, fz, h)
                } else {
                    t.blocks().linear(b)
                };
                c.map(|v| (v.powf(1.0 / 2.2) * 255.0) as u8)
            };
            img.put_pixel(i as u32, j as u32, image::Rgb(rgb));
        }
    }
    for p in t.plants_in(x0, z0, x0 + N - 1, z0 + N - 1) {
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (i, j) = (p.x - x0 + dx, p.z - z0 + dz);
            if (0..N).contains(&i) && (0..N).contains(&j) {
                img.put_pixel(i as u32, j as u32, image::Rgb([20, 45, 25]));
            }
        }
    }
    img.save(&a[3])?;
    Ok(())
}
