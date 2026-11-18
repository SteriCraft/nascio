//! Chantier 5.4: the client's prediction (`nascio_client::player::Player`) against the
//! server, through 150 ms of latency each way, 30 ms of jitter and 5 % of lost datagrams
//! (both ways). A walk with no edit gives no correction at all; an edit on the path gives a
//! correction, then the prediction agrees again; a client frozen for 500 ms is renumbered and
//! goes on without looping; a death then a respawn snap the view.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use nascio_client::player::{Controls, Player, PlayerEvent};
use nascio_client::{Connection, Event, Mirror, connect};
use nascio_physics::Shapes;
use nascio_protocol::messages::{ClientMsg, DeathCause, ServerMsg};
use nascio_protocol::player::Commands;
use nascio_server::accounts::Change;
use nascio_server::config::SecretLogin;
use nascio_server::{RunningServer, ServerConfig};
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

const LATENCY: f64 = 0.15;
const JITTER: f64 = 0.03;
const LOSS: f64 = 0.05;

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

/// What travels, delayed.
enum Out {
    Commands(Commands),
    Msg(ClientMsg),
}

/// A client with its prediction, behind a slow and lossy link.
struct Client {
    c: Connection,
    mirror: Mirror,
    player: Player,
    t0: Instant,
    rng: Lcg,
    /// Events from the server and messages to it, with the time they arrive (s from `t0`).
    inbound: VecDeque<(f64, Event)>,
    outbound: VecDeque<(f64, Out)>,
    /// Arrival of the last reliable message each way (they keep their order).
    last_in: f64,
    last_out: f64,
    events: Vec<(f64, PlayerEvent)>,
    /// Corrections counted at each update.
    corrections: Vec<(f64, u64)>,
}

impl Client {
    async fn new(server: &RunningServer, name: &str, seed: u64) -> Self {
        let c = connect(server.addr, &server.cert_der, name, "test-secret")
            .await
            .unwrap();
        let t0 = Instant::now();
        let player = Player::new(&c.hello, Shapes::new(&c.blocks), t0);
        Self {
            c,
            mirror: Mirror::default(),
            player,
            t0,
            rng: Lcg(seed),
            inbound: VecDeque::new(),
            outbound: VecDeque::new(),
            last_in: 0.0,
            last_out: 0.0,
            events: Vec::new(),
            corrections: Vec::new(),
        }
    }

    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    /// When a datagram sent now arrives (`None`: lost).
    fn datagram(&mut self, now: f64) -> Option<f64> {
        (self.rng.next() >= LOSS).then(|| now + LATENCY + JITTER * self.rng.next())
    }

    fn send(&mut self, m: ClientMsg) {
        let at = (self.now() + LATENCY).max(self.last_out);
        self.last_out = at;
        self.outbound.push_back((at, Out::Msg(m)));
    }

    /// Takes in what the server sent (delayed), delivers what is due both ways; `frozen`:
    /// the client does not run (nothing delivered to it), the network does.
    fn pump(&mut self, frozen: bool) {
        let now = self.now();
        while let Ok(ev) = self.c.events.try_recv() {
            let at = match ev {
                Event::Player(_) => match self.datagram(now) {
                    Some(at) => at,
                    None => continue,
                },
                Event::Entities(_) => continue,
                _ => {
                    self.last_in = (now + LATENCY).max(self.last_in);
                    self.last_in
                }
            };
            let i = self.inbound.partition_point(|(t, _)| *t <= at);
            self.inbound.insert(i, (at, ev));
        }
        while !frozen && self.inbound.front().is_some_and(|(t, _)| *t <= now) {
            let (_, ev) = self.inbound.pop_front().unwrap();
            match ev {
                Event::Column(col) => self.mirror.column(*col),
                Event::Player(s) => self.player.on_state(&s, &self.mirror),
                Event::Msg(ServerMsg::Blocks {
                    chunk,
                    lsn,
                    changes,
                }) => self.mirror.blocks(chunk, lsn, changes),
                Event::Msg(ServerMsg::Forget { columns }) => self.mirror.forget(&columns),
                Event::Msg(m) => self.player.on_msg(&m),
                Event::Closed(r) => panic!("closed: {r}"),
                _ => {}
            }
        }
        for column in self.mirror.take_resends() {
            self.send(ClientMsg::Resend { column });
        }
        while self.outbound.front().is_some_and(|(t, _)| *t <= now) {
            match self.outbound.pop_front().unwrap().1 {
                Out::Commands(c) => self.c.send_commands(&c),
                Out::Msg(m) => self.c.send(m),
            }
        }
    }

    /// Runs for `ms` milliseconds with these controls (every 5 ms).
    async fn run(&mut self, ms: u64, controls: Controls) {
        let end = Instant::now() + Duration::from_millis(ms);
        let mut every = tokio::time::interval(Duration::from_millis(5));
        while Instant::now() < end {
            every.tick().await;
            self.pump(false);
            self.player.set_rtt(Duration::from_secs_f64(2.0 * LATENCY));
            let now = self.now();
            if let Some(c) = self.player.update(Instant::now(), &controls, &self.mirror)
                && let Some(at) = self.datagram(now)
            {
                let i = self.outbound.partition_point(|(t, _)| *t <= at);
                self.outbound.insert(i, (at, Out::Commands(c)));
            }
            for e in self.player.events() {
                self.events.push((now, e));
            }
            self.corrections
                .push((now, self.player.stats().corrections));
        }
    }

    /// The client stops running for `ms` milliseconds (the network goes on).
    async fn freeze(&mut self, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        let mut every = tokio::time::interval(Duration::from_millis(5));
        while Instant::now() < end {
            every.tick().await;
            self.pump(true);
        }
    }

    /// Corrections between `from` and `to` seconds.
    fn corrections_between(&self, from: f64, to: f64) -> u64 {
        let at = |t: f64| {
            self.corrections
                .iter()
                .take_while(|c| c.0 <= t)
                .last()
                .map_or(0, |c| c.1)
        };
        at(to) - at(from)
    }
}

fn east(run: bool) -> Controls {
    Controls {
        forward: true,
        yaw: std::f64::consts::FRAC_PI_2,
        jump: run,
        ..Controls::default()
    }
}

/// A slow circle, jumping all the time (over what stands in the way).
fn circle(t: f64) -> Controls {
    Controls {
        forward: true,
        jump: true,
        yaw: 0.5 * t,
        ..Controls::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prediction_through_latency() {
    let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
        .expect("world")
        .world;
    let dir = std::env::temp_dir().join(format!("nascio-prediction-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        chunk_radius_m: 64,
        generation_threads: 2,
        account_hash_kib: 256,
        account_hash_passes: 1,
        spawn_xz: Some([0.0, 0.0]),
        open_registration: true,
        secret_login: SecretLogin::All,
        ..ServerConfig::default()
    };
    let server = RunningServer::start(&world, cfg, &dir).unwrap();

    // 1. A walk without edits: no correction at all once the columns are there.
    let mut a = Client::new(&server, "walker", 1).await;
    a.run(2_500, Controls::default()).await;
    let from = a.now();
    let walk = Instant::now();
    while walk.elapsed() < Duration::from_secs(8) {
        let t = walk.elapsed().as_secs_f64();
        a.run(50, circle(t)).await;
    }
    let to = a.now();
    let walked = a.player.state().pos;
    let n = a.corrections_between(from, to);
    let s = a.player.stats();
    eprintln!(
        "prediction: walk of 8 s, {n} corrections, {} commands replayed, {} stale states, pace {:.2}, {} unacked, at {walked:?}",
        s.replayed,
        s.stale,
        a.player.pace(),
        a.player.unacked()
    );
    assert_eq!(n, 0, "corrections without edits");
    assert_eq!(s.renumbered, 0);
    assert!(s.replayed > 1_000, "{} replayed", s.replayed);
    assert!(!a.player.blocked());

    // 2. A wall put just ahead: a correction, then the prediction agrees again.
    a.run(1_000, Controls::default()).await;
    let p = a.player.state().pos;
    let granite = a.c.blocks.id("granite").unwrap();
    let (x, y, z) = (
        (p[0] + 1.2).floor() as i32,
        p[1].floor() as i32,
        p[2].floor() as i32,
    );
    let from = a.now();
    let cmd = a.player.last_cmd();
    for dy in 0..3 {
        a.send(ClientMsg::SetBlock {
            pos: [x, y + dy, z],
            block: granite,
            cmd,
        });
    }
    a.run(3_000, east(false)).await;
    let mid = a.now() - 1.5;
    let total = a.corrections_between(from, a.now());
    let late = a.corrections_between(mid, a.now());
    let stopped = a.player.state().pos;
    eprintln!("prediction: wall, {total} corrections, {late} in the last 1.5 s; at {stopped:?}");
    assert!(total >= 1, "the wall changed the server's motion");
    assert_eq!(late, 0, "converged");
    assert!(
        (f64::from(x) - 0.3 - stopped[0]).abs() < 0.01,
        "against the wall at x = {x}: {stopped:?}"
    );

    // 3. The client freezes. For 500 ms, the server steps for it (no command for 250 ms)
    // but its numbers stay behind the client's: corrections, then agreement. For 1 s, they
    // pass the client's (beyond 2 × 150 ms + 300 ms): the client renumbers once, one round
    // trip of steps ahead of the server's, then agrees again.
    let mut seen = 0;
    for (freeze, renumbered) in [(500, 0..=0), (1_000, 1..=1)] {
        let walk = Instant::now();
        while walk.elapsed() < Duration::from_secs(1) {
            a.run(50, circle(10.0 + walk.elapsed().as_secs_f64())).await;
        }
        let filled = server.stats.lock().unwrap().filled_steps;
        a.freeze(freeze).await;
        let back = a.now();
        let walk = Instant::now();
        while walk.elapsed() < Duration::from_secs(4) {
            a.run(50, circle(11.0 + walk.elapsed().as_secs_f64())).await;
        }
        let s = a.player.stats();
        let filled = server.stats.lock().unwrap().filled_steps - filled;
        let total = a.corrections_between(back, a.now());
        let late = a.corrections_between(back + 2.0, a.now());
        eprintln!(
            "prediction: freeze of {freeze} ms, {filled} steps filled by the server, {} renumberings, {total} corrections, {late} in the last 2 s",
            s.renumbered - seen
        );
        assert!(filled > 0, "the server stepped for the client");
        assert!(
            renumbered.contains(&(s.renumbered - seen)),
            "{} renumberings",
            s.renumbered - seen
        );
        assert!(total > 0);
        assert_eq!(late, 0, "agreement after the freeze");
        if s.renumbered > seen {
            assert!(
                a.events
                    .iter()
                    .any(|(t, e)| *t >= back && *e == PlayerEvent::Snapped)
            );
        }
        seen = s.renumbered;
    }

    // 4. Death, then respawn: both snap the view; then no correction.
    server
        .shared
        .accounts
        .create("victim", "test-secret", 0)
        .unwrap();
    server
        .shared
        .accounts
        .apply(&[Change::Last("victim".into(), Some([0.5, -5_000.0, 0.5]))]);
    let mut v = Client::new(&server, "victim", 2).await;
    v.run(2_000, Controls::default()).await;
    assert!(v.player.dead());
    let died = v
        .events
        .iter()
        .position(|(_, e)| *e == PlayerEvent::Died(DeathCause::Suffocation))
        .expect("died");
    assert_eq!(v.events[died + 1].1, PlayerEvent::Snapped);
    v.send(ClientMsg::Respawn);
    v.run(1_500, Controls::default()).await;
    assert!(!v.player.dead());
    let back = v
        .events
        .iter()
        .position(|(_, e)| *e == PlayerEvent::Respawned)
        .expect("respawned");
    assert_eq!(v.events[back + 1].1, PlayerEvent::Snapped);
    assert!(v.player.display_pos()[1] > -100.0);
    let from = v.now();
    v.run(2_000, circle(0.0)).await;
    assert_eq!(v.corrections_between(from + 0.5, v.now()), 0);

    a.c.close();
    v.c.close();
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
