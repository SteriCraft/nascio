//! Chantier 5.3, the rules of the simulated players: no block placed inside a player, edits
//! within 5 m of the eyes, a player arriving inside blocks put beside them (5.3bis), flight
//! for administrators only. (Deaths and respawns: unit tests of `movement`, `arrival.rs`.)

use std::time::{Duration, Instant};

use nascio_client::{Connection, Event, connect};
use nascio_physics::{Input, Mode, State, keys};
use nascio_protocol::messages::{ClientMsg, ServerMsg};
use nascio_protocol::player::{Commands, PlayerState};
use nascio_server::config::SecretLogin;
use nascio_server::{RunningServer, ServerConfig};
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

/// A test client: the newest state from the server and the commands it sent.
struct Player {
    c: Connection,
    state: Option<State>,
    ack: Option<u32>,
    next: u32,
}

impl Player {
    async fn new(server: &RunningServer, name: &str) -> Self {
        let c = connect(server.addr, &server.cert_der, name, "test-secret")
            .await
            .unwrap();
        Self {
            c,
            state: None,
            ack: None,
            next: 0,
        }
    }

    /// A state from the server. Steps the server took without commands are numbered after
    /// ours: the next command follows the acknowledgement.
    fn receive(&mut self, p: &PlayerState) {
        self.state = Some(p.state);
        self.ack = p.ack;
        if let Some(a) = p.ack {
            self.next = self.next.max(a + 1);
        }
    }

    /// Sends three commands (one tick of steps).
    fn send(&mut self, input: Input) {
        self.next += 3;
        self.c.send_commands(&Commands {
            last: self.next - 1,
            pitch: 0,
            inputs: vec![input; 3],
        });
    }

    /// Waits for a control message matching `f` (keeping the state fresh), at most 20 s.
    async fn wait(&mut self, mut f: impl FnMut(&ServerMsg) -> bool) -> ServerMsg {
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.c.next(left).await {
                Some(Event::Msg(m)) if f(&m) => return m,
                Some(Event::Player(p)) => self.receive(&p),
                Some(Event::Closed(r)) => panic!("closed: {r}"),
                Some(_) => {}
                None => panic!("timed out"),
            }
        }
    }

    /// Keeps reading events for `ms` milliseconds, sending `input` every 50 ms.
    async fn run(&mut self, ms: u64, input: Input) {
        let end = Instant::now() + Duration::from_millis(ms);
        let mut every = tokio::time::interval(Duration::from_millis(50));
        while Instant::now() < end {
            every.tick().await;
            self.send(input);
            while let Ok(ev) = self.c.events.try_recv() {
                if let Event::Player(p) = ev {
                    self.receive(&p);
                }
            }
        }
    }

    fn set_block(&self, pos: [i32; 3], block: u16) {
        self.c.send(ClientMsg::SetBlock {
            pos,
            block,
            cmd: self.ack,
        });
    }

    /// The answer to an edit: `Ok` when the block changed, `Err(reason)` when refused.
    async fn edit(&mut self, pos: [i32; 3], block: u16) -> Result<(), String> {
        self.set_block(pos, block);
        match self
            .wait(|m| {
                matches!(m, ServerMsg::EditRefused { pos: p, .. } if *p == pos)
                    || matches!(m, ServerMsg::Blocks { changes, .. } if changes.iter().any(|c| c.1 == block))
            })
            .await
        {
            ServerMsg::EditRefused { reason, .. } => Err(reason),
            _ => Ok(()),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edits_arrivals_and_flight() {
    let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
        .expect("world")
        .world;
    let dir = std::env::temp_dir().join(format!("nascio-rules-test-{}", std::process::id()));
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
    let mut a = Player::new(&server, "builder").await;
    let granite = a.c.blocks.id("granite").unwrap();
    let spawn = a.c.hello.spawn;
    let cell = |dy: i32| {
        [
            spawn[0].floor() as i32,
            spawn[1].floor() as i32 + dy,
            spawn[2].floor() as i32,
        ]
    };
    // Standing still until the server moves it (its columns are there).
    a.run(1_000, Input::default()).await;
    let here = a.state.expect("a state").pos;

    // Inside the player: refused; out of reach: refused.
    let refused = a.edit(cell(1), granite).await.unwrap_err();
    assert!(refused.contains("inside a player"), "{refused}");
    let far = [cell(1)[0] + 7, cell(1)[1], cell(1)[2]];
    let refused = a.edit(far, granite).await.unwrap_err();
    assert!(refused.contains("out of reach"), "{refused}");

    // Walk east about two metres, off the spawn.
    let east = Input {
        keys: keys::FORWARD,
        yaw: Input::quantize_yaw(std::f64::consts::FRAC_PI_2),
        fly_speed: 0,
    };
    a.run(500, east).await;
    a.run(500, Input::default()).await;
    let moved = a.state.unwrap().pos;
    let dx = nascio_core::torus::delta_f64(here[0], moved[0]);
    assert!(dx > 1.2, "walked {dx} m east ({here:?} → {moved:?})");

    // Two blocks where a newcomer will stand: it is put beside them, not on top.
    let beside = |p: [f64; 3]| {
        let d = nascio_core::torus::distance_f64((p[0], p[2]), (spawn[0], spawn[2]));
        (0.9..3.0).contains(&d) && p[1] < f64::from(cell(2)[1])
    };
    for dy in [0, 1] {
        a.edit(cell(dy), granite).await.unwrap();
    }
    let mut b = Player::new(&server, "newcomer").await;
    b.run(1_500, Input::default()).await;
    let pushed = b.state.expect("a state").pos;
    assert!(
        beside(pushed),
        "beside the blocks: {pushed:?} (spawn {spawn:?})"
    );
    b.c.close();
    a.run(300, Input::default()).await;

    // One more above the head: still beside them, alive.
    a.edit(cell(2), granite).await.unwrap();
    let mut c = Player::new(&server, "unlucky").await;
    c.run(1_500, Input::default()).await;
    let out = c.state.expect("a state").pos;
    assert!(beside(out), "beside the blocks: {out:?} (spawn {spawn:?})");
    assert_eq!(c.state.unwrap().mode, Mode::Walk);

    // Flight: refused to a player, granted to an administrator and by the console.
    c.c.send(ClientMsg::Fly(true));
    c.run(500, Input::default()).await;
    assert_eq!(c.state.unwrap().mode, Mode::Walk, "not an administrator");
    server.shared.accounts.set_admin("unlucky", true).unwrap();
    c.c.send(ClientMsg::Fly(true));
    c.run(500, Input::default()).await;
    assert_eq!(c.state.unwrap().mode, Mode::Fly);
    assert!(server.shared.fly("unlucky", false));
    c.run(500, Input::default()).await;
    assert_eq!(c.state.unwrap().mode, Mode::Walk);
    assert!(!server.shared.fly("nobody", true));

    let stats = server.stats.lock().unwrap().clone();
    assert_eq!(stats.deaths, 0);
    a.c.close();
    c.c.close();
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
