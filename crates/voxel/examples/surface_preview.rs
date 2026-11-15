//! Hillshaded previews of the voxel surface at 1 m around the highest summit and a coast.
//!
//! `cargo run -p nascio-voxel --release --example surface_preview -- out/m2/world-42`

use std::path::PathBuf;

use nascio_voxel::{Terrain, VoxelConfig};
use nascio_worldgen::world_io::World;

const SIZE: usize = 1_024;

fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or("out/m2/world-42".into()));
    let world = World::load(&dir)?;
    let terrain = Terrain::from_world(&world, VoxelConfig::default())?;
    let elev = world.layer("elevation_tectonic").expect("elevation");
    let (mut peak, mut coast) = ((0, f64::MIN), (0, f64::MAX));
    for k in 0..elev.width * elev.height {
        let e = elev.data.get_f64(k);
        if e > peak.1 {
            peak = (k, e);
        }
        if e > 0.0 && e < coast.1 {
            coast = (k, e);
        }
    }
    for (name, k, step) in [
        ("peak", peak.0, 4.0),
        ("peak-1m", peak.0, 1.0),
        ("coast", coast.0, 4.0),
    ] {
        let cx = (k % elev.width) as f64 * elev.cell_m;
        let cz = (k / elev.width) as f64 * elev.cell_m;
        let mut h = vec![0.0; SIZE * SIZE];
        for j in 0..SIZE {
            for i in 0..SIZE {
                let x = cx + (i as f64 - SIZE as f64 / 2.0) * step;
                let z = cz + (j as f64 - SIZE as f64 / 2.0) * step;
                h[i + SIZE * j] = terrain.height(x, z);
            }
        }
        let mut img = image::RgbImage::new(SIZE as u32, SIZE as u32);
        for j in 1..SIZE - 1 {
            for i in 1..SIZE - 1 {
                let v = h[i + SIZE * j];
                let dx = (h[i + 1 + SIZE * j] - h[i - 1 + SIZE * j]) / (2.0 * step);
                let dz = (h[i + SIZE * (j + 1)] - h[i + SIZE * (j - 1)]) / (2.0 * step);
                let n = [-dx, 1.0, -dz];
                let l = (n[0] * -0.6 + n[1] * 0.7 + n[2] * -0.4) / (dx * dx + 1.0 + dz * dz).sqrt();
                let shade = l.clamp(0.0, 1.0) * 0.8 + 0.2;
                let base = if v < 0.0 {
                    [40.0, 90.0, 150.0]
                } else if v < 1.0 {
                    [212.0, 196.0, 146.0]
                } else if v > 2_200.0 || dx.hypot(dz) > 1.2 {
                    [130.0, 125.0, 118.0]
                } else {
                    [88.0, 132.0, 58.0]
                };
                img.put_pixel(
                    i as u32,
                    j as u32,
                    image::Rgb(base.map(|c| (c * shade) as u8)),
                );
            }
        }
        let path = dir.join(format!("surface-{name}.png"));
        img.save(&path)?;
        println!(
            "{name}: centre ({cx:.0}, {cz:.0}), {step} m/px, h {:.0}..{:.0} -> {}",
            h.iter().cloned().fold(f64::MAX, f64::min),
            h.iter().cloned().fold(f64::MIN, f64::max),
            path.display()
        );
    }
    Ok(())
}
