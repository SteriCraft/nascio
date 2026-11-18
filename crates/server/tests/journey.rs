//! Chantier 5.6, end to end: a complete client (`Player` + `Mirror`, as the viewer runs them)
//! walks, jumps, edits within 5 m and is refused beyond, dies and respawns; a player whose
//! spawn point is shut in (no free place within 10 m) gets another point around the same
//! site, kept in its account.

use std::time::{Duration, Instant};

use nascio_client::player::{Controls, Player, PlayerEvent};
use nascio_client::{Connection, Event, Mirror, connect};
use nascio_core::coords::BlockPos;
use nascio_core::torus;
use nascio_physics::Shapes;
use nascio_protocol::messages::{ClientMsg, DeathCause, ServerMsg};
use nascio_server::accounts::Change;
use nascio_server::config::SecretLogin;
use nascio_server::spawn::{AccountSpawn, Point, standing};
use nascio_server::{RunningServer, ServerConfig};
use nascio_voxel::{GenConfig, Terrain};
use nascio_world::world_io::World;
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

const SECRET: &str = "test-secret";

/// A client as the viewer runs it: the mirror of the columns, the predicted player.
struct Client {
    c: Connection,
    mirror: Mirror,
    player: Player,
    events: Vec<PlayerEvent>,
    refused: Vec<([i32; 3], String)>,
}

impl Client {
    async fn new(server: &RunningServer, name: &str) -> Self {
        // The server may hold a former session for a moment.
        for _ in 0..50 {
            if let Ok(c) = connect(server.addr, &server.cert_der, name, SECRET).await {
                let player = Player::new(&c.hello, Shapes::new(&c.blocks), Instant::now());
                return Self {
                    c,
                    mirror: Mirror::default(),
                    player,
                    events: Vec::new(),
                    refused: Vec::new(),
                };
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("cannot connect {name}");
    }

    /// What the server sent, as the viewer's network thread handles it.
    fn pump(&mut self) {
        while let Ok(ev) = self.c.events.try_recv() {
            match ev {
                Event::Column(col) => self.mirror.column(*col),
                Event::Player(s) => {
                    self.player.set_rtt(self.c.rtt());
                    self.player.on_state(&s, &self.mirror);
                }
                Event::Msg(ServerMsg::Blocks {
                    chunk,
                    lsn,
                    changes,
                }) => self.mirror.blocks(chunk, lsn, changes),
                Event::Msg(ServerMsg::Forget { columns }) => self.mirror.forget(&columns),
                Event::Msg(ServerMsg::EditRefused { pos, reason }) => {
                    self.refused.push((pos, reason));
                }
                Event::Msg(m) => self.player.on_msg(&m),
                Event::Closed(r) => panic!("closed: {r}"),
                _ => {}
            }
        }
        for column in self.mirror.take_resends() {
            self.c.send(ClientMsg::Resend { column });
        }
    }

    /// Runs `ms` milliseconds with these controls, a frame every 5 ms, until `stop` holds.
    /// Returns the highest feet reached.
    async fn run_until(
        &mut self,
        ms: u64,
        controls: Controls,
        mut stop: impl FnMut(&Self) -> bool,
    ) -> f64 {
        let end = Instant::now() + Duration::from_millis(ms);
        let mut every = tokio::time::interval(Duration::from_millis(5));
        let mut peak = f64::NEG_INFINITY;
        while Instant::now() < end {
            every.tick().await;
            self.pump();
            if let Some(c) = self.player.update(Instant::now(), &controls, &self.mirror) {
                self.c.send_commands(&c);
            }
            self.events.extend(self.player.events());
            peak = peak.max(self.player.state().pos[1]);
            if stop(self) {
                break;
            }
        }
        peak
    }

    async fn run(&mut self, ms: u64, controls: Controls) -> f64 {
        self.run_until(ms, controls, |_| false).await
    }

    /// Sends an edit from the eyes as the viewer does; `Ok` once the mirror shows the block,
    /// `Err(reason)` when refused.
    async fn edit(&mut self, pos: [i32; 3], block: u16) -> Result<(), String> {
        let before = self.refused.len();
        self.c.send(ClientMsg::SetBlock {
            pos,
            block,
            cmd: self.player.last_cmd(),
        });
        let p = BlockPos::new(pos[0], pos[1], pos[2]);
        self.run_until(5_000, Controls::default(), |c| {
            c.refused.len() > before || c.mirror.block(p) == Some(block)
        })
        .await;
        match self.refused.get(before) {
            Some((at, reason)) if *at == pos => Err(reason.clone()),
            Some(r) => panic!("unexpected refusal {r:?}"),
            None if self.mirror.block(p) == Some(block) => Ok(()),
            None => panic!("no answer to the edit at {pos:?}"),
        }
    }
}

fn world() -> World {
    generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
        .expect("world")
        .world
}

fn config(fixed: Option<[f64; 2]>) -> ServerConfig {
    ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        chunk_radius_m: 64,
        generation_threads: 2,
        account_hash_kib: 256,
        account_hash_passes: 1,
        spawn_xz: fixed,
        open_registration: true,
        secret_login: SecretLogin::All,
        ..ServerConfig::default()
    }
}

fn east() -> Controls {
    Controls {
        forward: true,
        yaw: std::f64::consts::FRAC_PI_2,
        ..Controls::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_walks_jumps_edits_dies_and_respawns() {
    let world = world();
    let dir = std::env::temp_dir().join(format!("nascio-journey-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let server = RunningServer::start(&world, config(Some([0.0, 0.0])), &dir).unwrap();

    // Standing: the columns arrive, the player lands on the ground.
    let mut a = Client::new(&server, "walker").await;
    assert!(!a.c.hello.admin, "a new account is no administrator");
    a.run_until(10_000, Controls::default(), |c| {
        c.player.state().on_ground && !c.player.blocked()
    })
    .await;
    a.run(500, Controls::default()).await;
    assert!(a.player.state().on_ground, "{:?}", a.player.state());
    let start = a.player.state().pos;

    // Walk east for 1.5 s, then stop: the prediction never disagrees with the server.
    let corrections = a.player.stats().corrections;
    a.run(1_500, east()).await;
    a.run(800, Controls::default()).await;
    let walked = a.player.state().pos;
    let dx = torus::delta_f64(start[0], walked[0]);
    eprintln!("journey: walked {dx:.2} m east, {walked:?}");
    assert!(dx > 2.0, "walked {dx} m ({start:?} → {walked:?})");
    assert_eq!(a.player.stats().corrections, corrections, "no correction");
    // Standing still, commands keep going: only those on their way are not acknowledged.
    assert!(a.player.unacked() < 15, "{} unacked", a.player.unacked());

    // Jump in place: up by the jump height (1.25 m), down to the same floor.
    let y0 = a.player.state().pos[1];
    let jump = Controls {
        jump: true,
        ..Controls::default()
    };
    let mut peak = a.run(100, jump).await;
    peak = peak.max(a.run(1_200, Controls::default()).await);
    let y1 = a.player.state().pos[1];
    eprintln!("journey: jumped {:.3} m", peak - y0);
    assert!((1.1..1.4).contains(&(peak - y0)), "jump of {} m", peak - y0);
    assert!((y1 - y0).abs() < 1e-9 && a.player.state().on_ground);
    assert_eq!(a.player.stats().corrections, corrections, "no correction");

    // Edits from the eyes: 4 m away accepted, 6 m away refused (reach 5 m).
    let granite = a.c.blocks.id("granite").unwrap();
    let e = a.player.eye();
    let cell = |ahead: f64| {
        [
            (e[0] + ahead).floor() as i32,
            e[1].floor() as i32 + 1,
            e[2].floor() as i32,
        ]
    };
    let near = cell(4.5);
    let id = |c: &Client, p: [i32; 3]| {
        let b = c.mirror.block(BlockPos::new(p[0], p[1], p[2]));
        if b == Some(granite) { 0 } else { granite }
    };
    let block = id(&a, near);
    a.edit(near, block).await.expect("within reach");
    let far = cell(6.5);
    let refused = a.edit(far, id(&a, far)).await.unwrap_err();
    assert!(refused.contains("out of reach"), "{refused}");

    // Death: back to a last position shut in rock, far below every column.
    server.shared.accounts.create("victim", SECRET, 0).unwrap();
    server
        .shared
        .accounts
        .apply(&[Change::Last("victim".into(), Some([0.5, -5_000.0, 0.5]))]);
    let mut v = Client::new(&server, "victim").await;
    v.run_until(10_000, Controls::default(), |c| c.player.dead())
        .await;
    assert!(v.player.dead());
    assert!(
        v.events
            .contains(&PlayerEvent::Died(DeathCause::Suffocation))
    );
    // Dead: the keys do nothing.
    let still = v.player.state().pos;
    v.run(500, east()).await;
    assert_eq!(v.player.state().pos, still);
    // Respawn: at the point, alive, walking again.
    v.c.send(ClientMsg::Respawn);
    v.run_until(10_000, Controls::default(), |c| !c.player.dead())
        .await;
    assert!(v.events.contains(&PlayerEvent::Respawned));
    v.run_until(10_000, Controls::default(), |c| {
        c.player.state().on_ground && !c.player.blocked()
    })
    .await;
    let back = v.player.state().pos;
    assert!(back[1] > -100.0, "{back:?}");
    v.run(1_000, east()).await;
    v.run(500, Controls::default()).await;
    let moved = torus::delta_f64(back[0], v.player.state().pos[0]);
    assert!(moved > 1.0, "walked {moved} m after the respawn");
    assert!(!v.player.dead());

    let stats = server.stats.lock().unwrap().clone();
    eprintln!(
        "journey: {} steps, {} without commands, {} deaths",
        stats.steps, stats.filled_steps, stats.deaths
    );
    assert_eq!(stats.deaths, 1);
    a.c.close();
    v.c.close();
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blocked_spawn_point_is_drawn_again() {
    let world = world();
    let dir = std::env::temp_dir().join(format!("nascio-journey-spawn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let server = RunningServer::start(&world, config(None), &dir).unwrap();
    let terrain = Terrain::from_world(&world, GenConfig::standard().voxel).unwrap();

    // The account's point is the first site; granite every other layer 11 m around it: no
    // free place for a box of 1.8 m within 10 m.
    let site = server.shared.spawns.sites[0];
    let s0 = AccountSpawn {
        site: 0,
        x: site.x,
        z: site.z,
    };
    server.shared.accounts.create("boxed", SECRET, 0).unwrap();
    server.shared.accounts.set_spawn("boxed", s0).unwrap();
    let blocked = standing(
        &terrain,
        Point {
            x: site.x,
            z: site.z,
        },
    );
    let granite = terrain.blocks().id("granite").unwrap();
    let feet = blocked.map(|v| v.floor() as i32);
    for dy in (-11..=12).filter(|d: &i32| d.rem_euclid(2) == 0) {
        for dz in -11..=11 {
            for dx in -11..=11 {
                let p = BlockPos::new(
                    torus::wrap(feet[0] + dx),
                    feet[1] + dy,
                    torus::wrap(feet[2] + dz),
                );
                server.deltas.set(p, granite).unwrap();
            }
        }
    }
    server.deltas.flush().unwrap();

    // The player arrives at its point, finds no place, and appears at another one.
    let mut c = Client::new(&server, "boxed").await;
    assert_eq!(c.c.hello.spawn, blocked, "arrival at the account's point");
    c.run_until(15_000, Controls::default(), |c| {
        let p = c.player.state().pos;
        c.player.state().on_ground
            && torus::distance_f64((p[0], p[2]), (blocked[0], blocked[2])) > 11.0
    })
    .await;
    let at = c.player.state().pos;
    let from_blocked = torus::distance_f64((at[0], at[2]), (blocked[0], blocked[2]));
    let from_site = torus::distance_f64((at[0], at[2]), (f64::from(site.x), f64::from(site.z)));
    eprintln!(
        "journey: blocked point {blocked:?}, now at {at:?}, {from_blocked:.0} m away, {from_site:.0} m from the site"
    );
    assert!(!c.player.dead(), "alive");
    assert!(from_blocked > 11.0, "{at:?}");
    assert!(from_site < 312.0, "{from_site} m from the site");
    // The new point replaces the account's (written by the accounts thread).
    let mut kept = None;
    for _ in 0..50 {
        kept = server.shared.accounts.spawn("boxed").filter(|s| *s != s0);
        if kept.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let kept = kept.expect("a new point in the account");
    assert_eq!(kept.site, 0, "around the same site");
    assert!(
        torus::distance_f64(
            (f64::from(kept.x) + 0.5, f64::from(kept.z) + 0.5),
            (at[0], at[2])
        ) < 11.0,
        "the player stands near its new point {kept:?}: {at:?}"
    );
    assert_eq!(server.stats.lock().unwrap().deaths, 0);
    c.c.close();
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
