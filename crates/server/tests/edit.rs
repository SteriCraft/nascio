//! S3 validation: two clients get the columns around the spawn, one sets a block, both see
//! it; a client that comes later gets it in its column; the edit survives a restart.

use std::time::{Duration, Instant};

use nascio_client::{Connection, Event, Mirror, connect};
use nascio_core::coords::BlockPos;
use nascio_protocol::messages::{ClientMsg, ServerMsg};
use nascio_server::{RunningServer, ServerConfig};
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

fn cfg() -> ServerConfig {
    ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        chunk_radius_m: 96,
        client_budget_bytes_per_s: 4 << 20,
        generation_threads: 4,
        // The edit far away must reach the "not loaded" check.
        edit_reach_m: 0.0,
        open_registration: true,
        ..ServerConfig::default()
    }
}

/// Feeds events into the mirror until `done` holds (or panics after 60 s).
async fn pump(c: &mut Connection, m: &mut Mirror, mut done: impl FnMut(&Mirror) -> bool) {
    let start = Instant::now();
    while !done(m) {
        assert!(start.elapsed() < Duration::from_secs(60), "timed out");
        match c.next(Duration::from_millis(200)).await {
            Some(Event::Column(col)) => m.column(*col),
            Some(Event::Msg(ServerMsg::Blocks {
                chunk,
                lsn,
                changes,
            })) => m.blocks(chunk, lsn, changes),
            Some(Event::Msg(ServerMsg::Forget { columns })) => m.forget(&columns),
            Some(Event::Closed(r)) => panic!("closed: {r}"),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_is_seen_by_every_client_and_survives_a_restart() {
    let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
        .expect("world")
        .world;
    let dir = std::env::temp_dir().join(format!("nascio-server-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let server = RunningServer::start(&world, cfg(), &dir).unwrap();

    let mut a = connect(server.addr, &server.cert_der, "player_a", "test-secret")
        .await
        .unwrap();
    let mut b = connect(server.addr, &server.cert_der, "player_b", "test-secret")
        .await
        .unwrap();
    assert_ne!(a.hello.player, b.hello.player);
    let spawn = a.hello.spawn;
    let granite = a.blocks.id("granite").unwrap();
    // Above the surface: air, in the spawn column.
    let p = BlockPos::new(spawn[0] as i32, spawn[1] as i32 + 5, spawn[2] as i32);
    let (mut ma, mut mb) = (Mirror::default(), Mirror::default());
    let start = Instant::now();
    pump(&mut a, &mut ma, |m| m.block(p).is_some() && m.len() >= 29).await;
    let joined = start.elapsed();
    pump(&mut b, &mut mb, |m| m.block(p).is_some()).await;
    assert_eq!(ma.block(p), Some(0), "air above the surface");

    a.send(ClientMsg::SetBlock {
        pos: [p.x, p.y, p.z],
        block: granite,
    });
    pump(&mut a, &mut ma, |m| m.block(p) == Some(granite)).await;
    pump(&mut b, &mut mb, |m| m.block(p) == Some(granite)).await;

    // Out of the player's columns: refused.
    a.send(ClientMsg::SetBlock {
        pos: [p.x + 20_000, p.y, p.z],
        block: granite,
    });
    let refused = loop {
        match a.next(Duration::from_secs(5)).await {
            Some(Event::Msg(ServerMsg::EditRefused { reason, .. })) => break reason,
            Some(_) => {}
            None => panic!("no refusal"),
        }
    };
    assert!(refused.contains("not loaded"), "{refused}");

    // A later client gets the edit in its column (the cache was rebuilt).
    let mut c = connect(server.addr, &server.cert_der, "player_c", "test-secret")
        .await
        .unwrap();
    let mut mc = Mirror::default();
    pump(&mut c, &mut mc, |m| m.block(p).is_some()).await;
    assert_eq!(mc.block(p), Some(granite));
    assert!(mc.get(p.chunk().cx, p.chunk().cz).unwrap().version > 0);
    for x in [&a, &b, &c] {
        x.close();
    }
    let sent = server
        .shared
        .sent_bytes
        .load(std::sync::atomic::Ordering::Relaxed);
    eprintln!(
        "first client: {} columns in {:.0} ms; {sent} column bytes sent in all",
        ma.len(),
        joined.as_secs_f64() * 1e3
    );
    server.shutdown().unwrap();

    // Restarted: the edit is there.
    let server = RunningServer::start(&world, cfg(), &dir).unwrap();
    let mut d = connect(server.addr, &server.cert_der, "player_d", "test-secret")
        .await
        .unwrap();
    let mut md = Mirror::default();
    pump(&mut d, &mut md, |m| m.block(p).is_some()).await;
    assert_eq!(md.block(p), Some(granite));
    d.close();
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
