//! Far ground colours of the whole world (`GROUND_RES`² px), with and without the plants'
//! expected cover: `cargo run -p nascio-tools --release --example far_ground -- <world> <out dir>`

use nascio_voxel::{Terrain, VoxelConfig, horizon::ground_colors};
use nascio_worldgen::world_io::World;

const RES: usize = 512;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let world = World::load(std::path::Path::new(&a[1]))?;
    let out = std::path::Path::new(&a[2]);
    std::fs::create_dir_all(out)?;
    for (name, gain) in [
        ("far-ground-bare.png", 0.0),
        (
            "far-ground-canopy.png",
            VoxelConfig::default().far_canopy_gain,
        ),
    ] {
        let cfg = VoxelConfig {
            far_canopy_gain: gain,
            ..VoxelConfig::default()
        };
        let t = Terrain::from_world(&world, cfg)?;
        let rgb = ground_colors(&t, RES);
        image::RgbImage::from_raw(RES as u32, RES as u32, rgb)
            .expect("size")
            .save(out.join(name))?;
    }
    Ok(())
}
