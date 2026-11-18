//! S4a validation: a client that only holds what the server sent meshes every chunk exactly
//! as the viewer does from the generator (same vertices, colours and materials), at the
//! corner of the torus and on land.

use std::time::{Duration, Instant};

use nascio_client::{Event, Mirror, connect};
use nascio_core::consts::CHUNKS_PER_AXIS;
use nascio_core::coords::ChunkPos;
use nascio_server::{RunningServer, ServerConfig};
use nascio_voxel::{ColumnSet, GenConfig, Terrain, mesh_chunk};
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;
use nascio_worldgen::world_io::World;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn network_meshes_equal_local_meshes() {
    let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
        .expect("world")
        .world;
    let terrain = Terrain::from_world(&world, GenConfig::standard().voxel).unwrap();
    // The corner of the torus (sea on this world: both seams), then the first land column met
    // on a fixed walk (relief, vegetation, rivers).
    let land = (0..CHUNKS_PER_AXIS)
        .step_by(37)
        .flat_map(|cz| (0..CHUNKS_PER_AXIS).step_by(41).map(move |cx| (cx, cz)))
        .find(|&(cx, cz)| ColumnSet::new(&terrain, cx, cz).top_range().0 > 40)
        .expect("land");
    for (k, (cx, cz)) in [(0, CHUNKS_PER_AXIS - 1), land].into_iter().enumerate() {
        site(&world, &terrain, k, cx, cz).await;
    }
}

/// Compares the meshes around column `(scx, scz)`.
async fn site(world: &World, terrain: &Terrain, k: usize, scx: i32, scz: i32) {
    let dir = std::env::temp_dir().join(format!("nascio-mesh-test-{}-{k}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // Spawn on the corner of the torus: the columns around it cross both seams.
    let cfg = ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        chunk_radius_m: 160,
        client_budget_bytes_per_s: 4 << 20,
        generation_threads: 4,
        spawn_xz: [f64::from(scx) * 32.0 + 10.0, f64::from(scz) * 32.0 + 22.0],
        ..ServerConfig::default()
    };
    let server = RunningServer::start(world, cfg, &dir).unwrap();
    let mut c = connect(server.addr, &server.cert_der, "mesh")
        .await
        .unwrap();
    let blocks = terrain.blocks();
    let names: Vec<String> = (0..blocks.len())
        .map(|i| blocks.def(i as u16).name.clone())
        .collect();
    assert_eq!(
        c.hello.blocks, names,
        "same block table: ids are the session's"
    );
    // Columns whose eight neighbours are all within the radius (5 columns) of the spawn
    // column (0, 3124).
    let inner: Vec<(i32, i32)> = (-4i32..=4)
        .flat_map(|dz| (-4i32..=4).map(move |dx| (dx, dz)))
        .filter(|&(dx, dz)| {
            (-1..=1).all(|a| (-1..=1).all(|b| (dx + a).pow(2) + (dz + b).pow(2) <= 25))
        })
        .map(|(dx, dz)| {
            (
                (scx + dx).rem_euclid(CHUNKS_PER_AXIS),
                (scz + dz).rem_euclid(CHUNKS_PER_AXIS),
            )
        })
        .collect();
    let mut m = Mirror::default();
    let start = Instant::now();
    while !inner.iter().all(|&(cx, cz)| m.can_mesh(cx, cz)) {
        assert!(start.elapsed() < Duration::from_secs(60), "timed out");
        if let Some(Event::Column(col)) = c.next(Duration::from_millis(200)).await {
            m.column(*col);
        }
    }
    let underground = terrain.config().underground_chunks;
    let (mut compared, mut quads) = (0, 0);
    for &(cx, cz) in &inner {
        let col = ColumnSet::new(terrain, cx, cz);
        let mut tint = terrain.chunk_tint(cx, cz);
        tint.water_depth = col.water_depths();
        let got = m.get(cx, cz).unwrap();
        assert_eq!(got.tint, tint, "tint of column ({cx}, {cz})");
        for cy in col.chunk_range(underground) {
            if !col.may_have_faces(cy) {
                continue;
            }
            let local = mesh_chunk(&col.padded_chunk(terrain, cy), blocks, &tint);
            let p = m.padded_chunk(ChunkPos::new(cx, cy, cz)).unwrap();
            let remote = mesh_chunk(&p, blocks, &got.tint);
            assert!(local == remote, "mesh of chunk ({cx}, {cy}, {cz})");
            compared += 1;
            quads += local.opaque.quad_count() + local.water.quad_count();
        }
    }
    eprintln!(
        "site ({scx}, {scz}): {} columns, {compared} chunks, {quads} quads identical",
        inner.len()
    );
    assert!(compared >= 30 && quads > 3_000);
    c.close();
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
