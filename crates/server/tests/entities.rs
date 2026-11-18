//! S5 validation: a player moving on a circle across the seam, seen by another one through
//! 150 ms of latency (each way), 30 ms of jitter and 5 % of lost entity datagrams. The
//! interpolated motion must stay on the circle, without gaps, jumps or going back.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use nascio_client::{Entities, Event, connect};
use nascio_core::torus;
use nascio_protocol::messages::ClientMsg;
use nascio_server::{RunningServer, ServerConfig};
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

const CENTRE: [f64; 2] = [3.0, 4.0];
const RADIUS: f64 = 10.0;
/// Angular speed, rad/s (5 m/s on the circle).
const OMEGA: f64 = 0.5;
const LATENCY: f64 = 0.15;
const JITTER: f64 = 0.03;

fn on_circle(t: f64) -> [f64; 3] {
    let a = OMEGA * t;
    [
        torus::wrap_f64(CENTRE[0] + RADIUS * a.cos()),
        80.0,
        torus::wrap_f64(CENTRE[1] + RADIUS * a.sin()),
    ]
}

/// Angle of a point around the centre.
fn angle(p: [f64; 3]) -> f64 {
    torus::delta_f64(CENTRE[1], p[2]).atan2(torus::delta_f64(CENTRE[0], p[0]))
}

/// Deterministic pseudo-random numbers in [0, 1).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn motion_is_smooth_through_150_ms_of_latency() {
    let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
        .expect("world")
        .world;
    let dir = std::env::temp_dir().join(format!("nascio-entities-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        chunk_radius_m: 64,
        generation_threads: 2,
        spawn_xz: [0.0, 0.0],
        // The mover jumps from the spawn onto its circle.
        max_speed_m_s: 0.0,
        ..ServerConfig::default()
    };
    let server = RunningServer::start(&world, cfg, &dir).unwrap();
    let mover = connect(server.addr, &server.cert_der, "mover", "test-secret")
        .await
        .unwrap();
    let mut obs = connect(server.addr, &server.cert_der, "observer", "test-secret")
        .await
        .unwrap();
    let id = mover.hello.player;
    obs.send(ClientMsg::Move {
        pos: [0.0, 80.0, 0.0],
        yaw: 0.0,
        pitch: 0.0,
    });

    // The mover sends its position every 10 ms, like a client at 100 frames per second.
    let start = Instant::now();
    let run = Duration::from_secs(7);
    let moving = tokio::spawn(async move {
        let mut mover = mover;
        let mut every = tokio::time::interval(Duration::from_millis(10));
        while start.elapsed() < run + Duration::from_secs(1) {
            every.tick().await;
            let t = start.elapsed().as_secs_f64();
            mover.send(ClientMsg::Move {
                pos: on_circle(t),
                yaw: (OMEGA * t) as f32,
                pitch: 0.0,
            });
            while mover.events.try_recv().is_ok() {}
        }
        mover.close();
    });

    let mut ents = Entities::new(0.05);
    let mut rng = Lcg(42);
    let mut inbound: VecDeque<(f64, Vec<u8>)> = VecDeque::new();
    let mut acks: VecDeque<(f64, u32)> = VecDeque::new();
    let (mut lost, mut received, mut bytes) = (0, 0, 0);
    let mut shown: Vec<(f64, Option<[f64; 3]>)> = Vec::new();
    let mut every = tokio::time::interval(Duration::from_millis(10));
    while start.elapsed() < run {
        every.tick().await;
        let now = start.elapsed().as_secs_f64();
        while let Ok(ev) = obs.events.try_recv() {
            if let Event::Entities(d) = ev {
                if rng.next() < 0.05 {
                    lost += 1;
                    continue;
                }
                let at = now + LATENCY + JITTER * rng.next();
                let i = inbound.partition_point(|(t, _)| *t <= at);
                inbound.insert(i, (at, d));
            }
        }
        while inbound.front().is_some_and(|(t, _)| *t <= now) {
            let (_, d) = inbound.pop_front().unwrap();
            received += 1;
            bytes += d.len();
            let seq = ents.receive(&d, now).expect("datagram");
            acks.push_back((now + LATENCY, seq));
        }
        let mut seqs = Vec::new();
        while acks.front().is_some_and(|(t, _)| *t <= now) {
            seqs.push(acks.pop_front().unwrap().1);
        }
        if !seqs.is_empty() {
            obs.send(ClientMsg::Ack { seqs });
        }
        let p = ents.poses(now).into_iter().find(|p| p.id == id);
        shown.push((now, p.map(|p| p.pos)));
    }
    obs.close();
    moving.await.unwrap();

    // After two seconds (connection, columns, the clock settling).
    let steady: Vec<(f64, [f64; 3])> = shown
        .iter()
        .filter(|(t, _)| *t > 2.0)
        .map(|(t, p)| (*t, p.expect("the mover is always shown")))
        .collect();
    assert!(steady.len() > 300, "{} samples", steady.len());
    let mut worst_radius: f64 = 0.0;
    for (_, p) in &steady {
        let r = torus::delta_f64(CENTRE[0], p[0]).hypot(torus::delta_f64(CENTRE[1], p[2]));
        worst_radius = worst_radius.max((r - RADIUS).abs());
    }
    // Angular speed over 50 ms windows, and in all.
    let unwrap = |a: f64, b: f64| {
        let d = b - a;
        d - (d / std::f64::consts::TAU).round() * std::f64::consts::TAU
    };
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for w in steady.windows(6) {
        let (t0, p0) = w[0];
        let (t1, p1) = w[5];
        let s = unwrap(angle(p0), angle(p1)) / (t1 - t0);
        lo = lo.min(s);
        hi = hi.max(s);
    }
    let total: f64 = steady
        .windows(2)
        .map(|w| unwrap(angle(w[0].1), angle(w[1].1)))
        .sum::<f64>()
        / (steady.last().unwrap().0 - steady[0].0);
    let span = start.elapsed().as_secs_f64();
    eprintln!(
        "entities: {received} datagrams received, {lost} lost, {:.0} B/s; radius error {worst_radius:.3} m; \
         angular speed {total:.3} rad/s (50 ms windows {lo:.3}–{hi:.3}, nominal {OMEGA})",
        bytes as f64 / span
    );
    assert!(worst_radius < 0.1, "off the circle by {worst_radius} m");
    assert!((total - OMEGA).abs() < 0.05 * OMEGA, "mean speed {total}");
    assert!(
        lo > 0.5 * OMEGA && hi < 1.5 * OMEGA,
        "uneven motion: {lo}–{hi} rad/s"
    );
    let stats = server.stats.lock().unwrap().clone();
    assert!(stats.entity_datagrams > 0);
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
