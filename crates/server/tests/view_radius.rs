//! The client's view radius (`ClientMsg::ViewRadius`) bounds the columns it gets: asked
//! small, the server sends the near columns and forgets the others; asked larger, it sends
//! farther again, never beyond its own radius. Around the corner of the torus.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use nascio_client::{Event, connect};
use nascio_core::consts::CHUNKS_PER_AXIS;
use nascio_core::torus::delta_chunks;
use nascio_protocol::messages::{ClientMsg, ServerMsg};
use nascio_server::config::SecretLogin;
use nascio_server::{RunningServer, ServerConfig};
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

/// Distance in columns, squared, from the spawn column (0, 0) across the seams.
fn d2(cx: i32, cz: i32) -> i32 {
    let (dx, dz) = (delta_chunks(0, cx), delta_chunks(0, cz));
    dx * dx + dz * dz
}

/// Columns held after `secs` of events: received, minus forgotten.
async fn hold_for(c: &mut nascio_client::Connection, held: &mut BTreeSet<(i32, i32)>, secs: f64) {
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < secs {
        match c.next(Duration::from_millis(100)).await {
            Some(Event::Column(col)) => {
                held.insert((col.cx, col.cz));
            }
            Some(Event::Msg(ServerMsg::Forget { columns })) => {
                for [x, z] in columns {
                    held.remove(&(x, z));
                }
            }
            Some(Event::Closed(r)) => panic!("closed: {r}"),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn view_radius_bounds_the_columns() {
    let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
        .expect("world")
        .world;
    let dir = std::env::temp_dir().join(format!("nascio-view-radius-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        chunk_radius_m: 224,
        forget_margin_m: 0,
        client_budget_bytes_per_s: 4 << 20,
        generation_threads: 4,
        spawn_xz: Some([10.0, 22.0]),
        open_registration: true,
        secret_login: SecretLogin::All,
        ..ServerConfig::default()
    };
    let server = RunningServer::start(&world, cfg, &dir).unwrap();
    let mut c = connect(server.addr, &server.cert_der, "eye", "test-secret")
        .await
        .unwrap();
    let ring = |r: i32| -> BTreeSet<(i32, i32)> {
        (-r..=r)
            .flat_map(|dz| (-r..=r).map(move |dx| (dx, dz)))
            .filter(|&(dx, dz)| dx * dx + dz * dz <= r * r)
            .map(|(dx, dz)| {
                (
                    dx.rem_euclid(CHUNKS_PER_AXIS),
                    dz.rem_euclid(CHUNKS_PER_AXIS),
                )
            })
            .collect()
    };
    let mut held = BTreeSet::new();

    // Small: 3 columns. Whatever came before the request is forgotten.
    c.send(ClientMsg::ViewRadius { radius_m: 96 });
    hold_for(&mut c, &mut held, 4.0).await;
    assert_eq!(held, ring(3), "radius 96 m");

    // Larger than the server's (7 columns): its own radius.
    c.send(ClientMsg::ViewRadius { radius_m: 10_000 });
    hold_for(&mut c, &mut held, 6.0).await;
    assert_eq!(held, ring(7), "server radius 224 m");
    assert!(held.iter().all(|&(x, z)| d2(x, z) <= 49));

    // Below the floor (64 m: 2 columns).
    c.send(ClientMsg::ViewRadius { radius_m: 0 });
    hold_for(&mut c, &mut held, 2.0).await;
    assert_eq!(held, ring(2), "floor 64 m");
    c.close();
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
