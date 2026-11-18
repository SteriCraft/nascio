//! S5 validation, with the server moving the player (chantier 5.3): a player running and
//! turning (5.6 m/s, jumping over what stands in the way), seen by another one through
//! 150 ms of latency (each way), 30 ms of jitter and 5 % of lost entity datagrams. The
//! interpolated motion must follow the authoritative one (the mover's own states), without
//! gaps or jumps.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nascio_client::{Entities, Event, connect};
use nascio_core::torus;
use nascio_physics::{Input, keys};
use nascio_protocol::messages::ClientMsg;
use nascio_protocol::player::Commands;
use nascio_server::config::SecretLogin;
use nascio_server::{RunningServer, ServerConfig};
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

/// Turn rate of the heading, rad/s (a circle of 11 m at 5.6 m/s).
const OMEGA: f64 = 0.5;
const LATENCY: f64 = 0.15;
const JITTER: f64 = 0.03;

/// The mover's authoritative position at a fractional tick (linear, on the torus).
fn truth(states: &BTreeMap<u32, [f64; 3]>, tick: f64) -> Option<[f64; 3]> {
    let t0 = tick.floor() as u32;
    let (a, b) = (states.get(&t0)?, states.get(&(t0 + 1))?);
    let f = tick - f64::from(t0);
    Some([
        torus::wrap_f64(a[0] + torus::delta_f64(a[0], b[0]) * f),
        a[1] + (b[1] - a[1]) * f,
        torus::wrap_f64(a[2] + torus::delta_f64(a[2], b[2]) * f),
    ])
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
        spawn_xz: Some([0.0, 0.0]),
        open_registration: true,
        secret_login: SecretLogin::All,
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

    // The mover runs, turning at a steady rate and jumping, three steps every 50 ms; its
    // own states (by tick) are the truth.
    let start = Instant::now();
    let run = Duration::from_secs(7);
    let states: Arc<Mutex<BTreeMap<u32, [f64; 3]>>> = Arc::default();
    let truths = states.clone();
    let moving = tokio::spawn(async move {
        let mut mover = mover;
        let mut every = tokio::time::interval(Duration::from_millis(50));
        let mut last = 0u32;
        while start.elapsed() < run + Duration::from_secs(1) {
            every.tick().await;
            let t = start.elapsed().as_secs_f64();
            let step = Input {
                keys: keys::FORWARD | keys::RUN | keys::JUMP,
                yaw: Input::quantize_yaw(OMEGA * t),
                fly_speed: 0,
            };
            mover.send_commands(&Commands {
                last: last + 2,
                pitch: 0,
                inputs: vec![step; 3],
            });
            last += 3;
            while let Ok(ev) = mover.events.try_recv() {
                if let Event::Player(p) = ev {
                    truths.lock().unwrap().insert(p.tick, p.state.pos);
                }
            }
        }
        mover.close();
    });

    let mut ents = Entities::new(0.05);
    let mut rng = Lcg(42);
    let mut inbound: VecDeque<(f64, Vec<u8>)> = VecDeque::new();
    let mut acks: VecDeque<(f64, u32)> = VecDeque::new();
    let (mut lost, mut received, mut bytes) = (0, 0, 0);
    let mut shown: Vec<(f64, Option<f64>, Option<[f64; 3]>)> = Vec::new();
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
        shown.push((now, ents.render_tick(now), p.map(|p| p.pos)));
    }
    obs.close();
    moving.await.unwrap();

    // After two seconds (connection, columns, the clock settling).
    let states = states.lock().unwrap().clone();
    let steady: Vec<(f64, f64, [f64; 3])> = shown
        .iter()
        .filter(|(t, _, _)| *t > 2.0)
        .map(|(t, r, p)| (*t, r.expect("clock"), p.expect("the mover is always shown")))
        .collect();
    assert!(steady.len() > 300, "{} samples", steady.len());
    let dist = |a: [f64; 3], b: [f64; 3]| {
        torus::delta_f64(a[0], b[0])
            .hypot(b[1] - a[1])
            .hypot(torus::delta_f64(a[2], b[2]))
    };
    let (mut sum, mut worst, mut n) = (0.0, 0.0f64, 0);
    for (_, r, p) in &steady {
        if let Some(t) = truth(&states, *r) {
            let e = dist(*p, t);
            sum += e;
            worst = worst.max(e);
            n += 1;
        }
    }
    let mean = sum / f64::from(n.max(1));
    // No jump between two frames 10 ms apart (5.6 m/s: 6 cm).
    let jump = steady
        .windows(2)
        .map(|w| dist(w[0].2, w[1].2))
        .fold(0.0f64, f64::max);
    // The mover's horizontal speed over its states after two seconds.
    let ticks: Vec<(&u32, &[f64; 3])> = states.iter().filter(|(t, _)| **t > 40).collect();
    let path: f64 = ticks
        .windows(2)
        .map(|w| {
            torus::delta_f64(w[0].1[0], w[1].1[0]).hypot(torus::delta_f64(w[0].1[2], w[1].1[2]))
        })
        .sum();
    let speed = path / ((ticks.last().unwrap().0 - ticks[0].0) as f64 * 0.05);
    let span = start.elapsed().as_secs_f64();
    eprintln!(
        "entities: {received} datagrams received, {lost} lost, {:.0} B/s; {n} samples, error to the authoritative motion mean {mean:.3} m, worst {worst:.3} m; largest step between frames {jump:.3} m; mover's speed {speed:.2} m/s",
        bytes as f64 / span
    );
    assert!(n > 300, "{n} samples compared");
    assert!(mean < 0.05, "mean error {mean} m");
    assert!(worst < 0.5, "worst error {worst} m");
    assert!(jump < 0.3, "a jump of {jump} m");
    assert!((3.0..5.7).contains(&speed), "mover's speed {speed} m/s");
    let stats = server.stats.lock().unwrap().clone();
    assert!(stats.entity_datagrams > 0);
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
