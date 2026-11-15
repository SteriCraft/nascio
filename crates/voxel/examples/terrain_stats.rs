//! Base-elevation statistics before and after talus relaxation.
//!
//! `cargo run -p nascio-voxel --release --example terrain_stats -- out/m2/world-42`

use std::time::Instant;

use nascio_voxel::VoxelConfig;
use nascio_voxel::terrain::relax_talus;
use nascio_worldgen::grid::PeriodicGrid;
use nascio_worldgen::world_io::World;

fn stats(name: &str, g: &PeriodicGrid<f32>) {
    let (w, h, cell) = (g.width() as i64, g.height() as i64, g.cell_size());
    let mut grades = Vec::new();
    let (mut land, mut top) = (0usize, f32::MIN);
    for j in 0..h {
        for i in 0..w {
            let v = *g.get(i, j);
            land += usize::from(v > 0.0);
            top = top.max(v);
            let d = (g.get(i + 1, j) - v).abs().max((g.get(i, j + 1) - v).abs());
            grades.push(f64::from(d) / cell);
        }
    }
    grades.sort_by(f64::total_cmp);
    let n = grades.len();
    let q = |p: f64| grades[((n as f64 * p) as usize).min(n - 1)];
    println!(
        "{name}: land {:.3}, top {top:.0} m, grade p50 {:.2} p90 {:.2} p99 {:.2} max {:.2}",
        land as f64 / n as f64,
        q(0.5),
        q(0.9),
        q(0.99),
        grades[n - 1]
    );
}

fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).unwrap_or("out/m2/world-42".into());
    let world = World::load(dir.as_ref())?;
    let l = world.layer("elevation_tectonic").expect("elevation");
    let data = (0..l.width * l.height)
        .map(|k| l.data.get_f64(k) as f32)
        .collect();
    let g = PeriodicGrid::from_vec(l.width, l.height, l.cell_m, data);
    stats("raw", &g);
    let cfg = VoxelConfig::default();
    for (grade, iters) in [
        (cfg.talus_grade, cfg.talus_iterations),
        (0.7, 1_000),
        (1.0, 400),
    ] {
        let t = Instant::now();
        let r = relax_talus(g.clone(), grade, iters);
        let ms = t.elapsed().as_millis();
        stats(&format!("grade {grade} x{iters} ({ms} ms)"), &r);
    }
    Ok(())
}
