//! Chantier 5.3bis: a player comes back where it left (to the bit, also after a clean stop),
//! its position is kept while it plays, and a player dead at its departure, or shut in with
//! no free place within 10 m, comes back alive at its account's point.

use std::path::Path;
use std::time::{Duration, Instant};

use nascio_client::{Connection, Event, connect};
use nascio_physics::{Input, State, keys};
use nascio_protocol::messages::{DeathCause, ServerMsg};
use nascio_protocol::player::{Commands, PlayerState};
use nascio_server::accounts::{Accounts, Change};
use nascio_server::config::SecretLogin;
use nascio_server::{RunningServer, ServerConfig};
use nascio_world::world_io::World;
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

/// A test client: the newest state from the server and the commands it sent.
struct Player {
    c: Connection,
    state: Option<State>,
    next: u32,
    died: Option<DeathCause>,
}

impl Player {
    async fn new(server: &RunningServer, name: &str) -> Self {
        // The server may hold the former session for a moment.
        for _ in 0..50 {
            if let Ok(c) = connect(server.addr, &server.cert_der, name, "test-secret").await {
                return Self {
                    c,
                    state: None,
                    next: 0,
                    died: None,
                };
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{name} cannot log in");
    }

    fn receive(&mut self, p: &PlayerState) {
        self.state = Some(p.state);
        if let Some(a) = p.ack {
            self.next = self.next.max(a + 1);
        }
    }

    fn event(&mut self, ev: Event) {
        match ev {
            Event::Player(p) => self.receive(&p),
            Event::Msg(ServerMsg::Died { cause }) => self.died = Some(cause),
            _ => {}
        }
    }

    /// The first state from the server.
    async fn first_state(&mut self) -> State {
        let end = Instant::now() + Duration::from_secs(20);
        while self.state.is_none() {
            let left = end.saturating_duration_since(Instant::now());
            match self.c.next(left).await {
                Some(Event::Closed(r)) => panic!("closed: {r}"),
                Some(ev) => self.event(ev),
                None => panic!("no state"),
            }
        }
        self.state.unwrap()
    }

    /// Reads events for `ms` milliseconds, sending `input` every 50 ms (`None`: nothing).
    async fn run(&mut self, ms: u64, input: Option<Input>) {
        let end = Instant::now() + Duration::from_millis(ms);
        let mut every = tokio::time::interval(Duration::from_millis(50));
        while Instant::now() < end {
            every.tick().await;
            if let Some(input) = input {
                self.next += 3;
                self.c.send_commands(&Commands {
                    last: self.next - 1,
                    pitch: 0,
                    inputs: vec![input; 3],
                });
            }
            while let Ok(ev) = self.c.events.try_recv() {
                self.event(ev);
            }
        }
    }
}

fn config() -> ServerConfig {
    ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        chunk_radius_m: 48,
        generation_threads: 2,
        account_hash_kib: 256,
        account_hash_passes: 1,
        spawn_xz: Some([0.0, 0.0]),
        open_registration: true,
        secret_login: SecretLogin::All,
        position_save_s: 1,
        ..ServerConfig::default()
    }
}

fn start(world: &World, dir: &Path) -> RunningServer {
    RunningServer::start(world, config(), dir).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn players_come_back_where_they_left() {
    let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
        .expect("world")
        .world;
    let dir = std::env::temp_dir().join(format!("nascio-arrival-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let server = start(&world, &dir);

    // Walks north-east, then stands still (the server stands in for it, without keys).
    let mut w = Player::new(&server, "walker").await;
    let home = w.c.hello.spawn;
    w.first_state().await;
    let ne = Input {
        keys: keys::FORWARD,
        yaw: Input::quantize_yaw(std::f64::consts::FRAC_PI_4),
        fly_speed: 0,
    };
    w.run(1_500, Some(ne)).await;
    w.run(1_500, None).await;
    let left = w.state.unwrap().pos;
    assert!(
        nascio_core::torus::distance_f64((home[0], home[2]), (left[0], left[2])) > 3.0,
        "walked from {home:?} to {left:?}"
    );
    // Kept every second while online.
    assert_eq!(server.shared.accounts.last("walker"), Some(left));
    w.c.close();

    // Back: the same position, to the bit.
    let mut w = Player::new(&server, "walker").await;
    assert_eq!(w.first_state().await.pos, left);
    w.run(500, None).await;
    assert_eq!(w.state.unwrap().pos, left);
    w.c.close();

    // A victim, to be buried below every column while the server is down.
    let mut v = Player::new(&server, "victim").await;
    let point = v.c.hello.spawn;
    v.first_state().await;
    v.c.close();
    tokio::time::sleep(Duration::from_millis(300)).await;
    server.shutdown().unwrap();

    // After a clean stop: kept in accounts.toml, to the bit.
    let accounts = Accounts::open(&dir.join("server")).unwrap();
    assert_eq!(accounts.last("walker"), Some(left));
    let buried = [point[0], -5_000.0, point[2]];
    assert!(accounts.apply(&[Change::Last("victim".into(), Some(buried))]));
    accounts.save().unwrap();
    drop(accounts);

    let server = start(&world, &dir);
    let mut w = Player::new(&server, "walker").await;
    assert_eq!(w.c.hello.spawn, left);
    assert_eq!(w.first_state().await.pos, left);

    // Buried: nothing free within 10 m (unknown cells are not free), suffocates at once.
    let mut v = Player::new(&server, "victim").await;
    assert_eq!(v.c.hello.spawn, buried);
    v.run(1_500, None).await;
    assert_eq!(v.died, Some(DeathCause::Suffocation));
    v.c.close();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        server.shared.accounts.last("victim"),
        None,
        "dead at departure"
    );

    // Dead at its departure: back alive at its point.
    let mut v = Player::new(&server, "victim").await;
    assert_eq!(v.c.hello.spawn, point);
    v.run(1_500, Some(Input::default())).await;
    assert_eq!(v.died, None);
    let s = v.state.unwrap();
    assert!(
        nascio_core::torus::distance_f64((s.pos[0], s.pos[2]), (point[0], point[2])) < 0.01,
        "{s:?} at {point:?}"
    );

    assert_eq!(server.stats.lock().unwrap().deaths, 1);
    v.c.close();
    w.c.close();
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
