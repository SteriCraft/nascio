//! Writes `out/noise-tiled.png`: periodic fBm noise tiled 2×2, to eyeball the seam.
//!
//! `cargo run -p nascio-worldgen --example noise_preview --release`

use nascio_core::consts::WORLD_SIZE_F64;
use nascio_worldgen::noise::Fbm;
use nascio_worldgen::plot::{Canvas, hex};

fn main() -> anyhow::Result<()> {
    let tile = 300u32;
    let fbm = Fbm::new(42, 25_000.0, 7, 0.5);
    let mut cv = Canvas::new(2 * tile, 2 * tile, hex("#000000"));
    for py in 0..2 * tile {
        for px in 0..2 * tile {
            let x = f64::from(px % tile) / f64::from(tile) * WORLD_SIZE_F64;
            let z = f64::from(py % tile) / f64::from(tile) * WORLD_SIZE_F64;
            let v = ((fbm.sample(x, z) * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            cv.img.put_pixel(px, py, image::Rgb([v, v, v]));
        }
    }
    let out = std::path::Path::new("out/noise-tiled.png");
    cv.save(out)?;
    println!("wrote {}", out.display());
    Ok(())
}
