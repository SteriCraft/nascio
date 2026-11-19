//! Chantier 5.3: the server moves the player from its commands. Datagrams of commands lost
//! (up to three in a row), duplicated or late give the very trajectory a local simulation of
//! `nascio-physics` gives over the blocks the client received — bit for bit, at every
//! acknowledged command.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use nascio_client::{Event, Mirror, connect};
use nascio_core::coords::BlockPos;
use nascio_physics::{Input, Shapes, State, Stuck, keys, step, unstuck};
use nascio_protocol::messages::ServerMsg;
use nascio_protocol::player::{Commands, MAX_COMMANDS, PlayerState};
use nascio_server::config::SecretLogin;
use nascio_server::{RunningServer, ServerConfig};
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

/// Datagrams of the script (three commands each, one every 50 ms).
const DATAGRAMS: u32 = 120;

/// The command of step `n`: walk, run while turning, jumps, back and left, crouch, rest.
fn command(n: u32) -> Input {
    let mut k = match n {
        0..60 => keys::FORWARD,
        60..180 => keys::FORWARD | keys::RUN,
        180..240 => keys::BACK | keys::LEFT,
        240..300 => keys::FORWARD | keys::CROUCH,
        _ => 0,
    };
    if n % 40 < 5 {
        k |= keys::JUMP;
    }
    let yaw = if n < 60 {
        0.0
    } else {
        0.02 * f64::from(n.min(180) - 60)
    };
    Input {
        keys: k,
        yaw: Input::quantize_yaw(yaw),
        fly_speed: 0,
    }
}

fn bits(s: &State) -> Vec<u64> {
    s.pos
        .iter()
        .chain(&s.vel)
        .map(|v| v.to_bits())
        .chain([u64::from(s.on_ground), u64::from(s.in_fluid)])
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_duplicated_and_late_commands_give_the_local_trajectory() {
    let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
        .expect("world")
        .world;
    let dir = std::env::temp_dir().join(format!("nascio-movement-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        chunk_radius_m: 96,
        generation_threads: 2,
        spawn_xz: Some([0.0, 0.0]),
        open_registration: true,
        secret_login: SecretLogin::All,
        ..ServerConfig::default()
    };
    let server = RunningServer::start(&world, cfg, &dir).unwrap();
    let mut c = connect(server.addr, &server.cert_der, "walker", "test-secret")
        .await
        .unwrap();
    let params = c.hello.physics.clone();
    let shapes = Shapes::new(&c.blocks);
    let spawn = c.hello.spawn;
    let mut mirror = Mirror::default();
    // Server states by acknowledged command.
    let mut states: BTreeMap<u32, State> = BTreeMap::new();
    let mut acked: Option<u32> = None;
    let mut unacked: VecDeque<(u32, Input)> = VecDeque::new();
    let mut late: Vec<(u32, Vec<u8>)> = Vec::new();
    let (mut lost, mut dups, mut lates) = (0, 0, 0);
    let mut every = tokio::time::interval(Duration::from_millis(50));
    let mut t = 0u32;
    let last = DATAGRAMS * 3 - 1;
    let mut waited = 0;
    // Let the columns around the spawn arrive first (the server waits for its own too).
    while waited < 100 && !mirror.can_mesh(0, 0) {
        every.tick().await;
        waited += 1;
        while let Ok(ev) = c.events.try_recv() {
            if let Event::Column(col) = ev {
                mirror.column(*col);
            }
        }
    }
    loop {
        every.tick().await;
        while let Ok(ev) = c.events.try_recv() {
            match ev {
                Event::Column(col) => mirror.column(*col),
                Event::Msg(ServerMsg::Blocks {
                    chunk,
                    lsn,
                    changes,
                }) => {
                    mirror.blocks(chunk, lsn, changes);
                }
                Event::Msg(ServerMsg::Died { cause }) => panic!("died: {cause:?}"),
                Event::Player(PlayerState {
                    ack: Some(a),
                    state,
                    ..
                }) => {
                    states.entry(a).or_insert(state);
                    acked = acked.max(Some(a));
                }
                Event::Closed(r) => panic!("closed: {r}"),
                _ => {}
            }
        }
        if let Some(a) = acked {
            while unacked.front().is_some_and(|c| c.0 <= a) {
                unacked.pop_front();
            }
        }
        if acked == Some(last) || t > DATAGRAMS + 60 {
            break;
        }
        if t < DATAGRAMS {
            for n in 3 * t..3 * t + 3 {
                unacked.push_back((n, command(n)));
            }
        }
        // The newest commands not acknowledged (repeated while the script has ended).
        let k = unacked.len().min(MAX_COMMANDS);
        let Some(&(newest, _)) = unacked.back() else {
            t += 1;
            continue;
        };
        let d = Commands {
            last: newest,
            pitch: 0,
            inputs: unacked
                .iter()
                .skip(unacked.len() - k)
                .map(|c| c.1)
                .collect(),
        };
        // Late copies of earlier datagrams arrive after newer ones.
        let (due, keep): (Vec<_>, Vec<_>) = late.drain(..).partition(|(at, _)| *at <= t);
        late = keep;
        for (_, d) in due {
            c.send_commands(&Commands::decode(&d).unwrap());
            lates += 1;
        }
        let lose = matches!(t % 23, 5 | 11 | 12 | 17..=19);
        if t < DATAGRAMS && lose {
            lost += 1;
        } else {
            c.send_commands(&d);
            if t % 7 == 3 {
                c.send_commands(&d);
                dups += 1;
            }
            if t % 9 == 4 {
                late.push((t + 2, d.encode()));
            }
        }
        t += 1;
    }
    c.close();
    assert_eq!(acked, Some(last), "every command applied");
    let stats = server.stats.lock().unwrap().clone();
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    // The local simulation over the client's blocks.
    let src = |x: i32, y: i32, z: i32| mirror.block(BlockPos::new(x, y, z));
    let mut s = State::new(spawn, nascio_physics::Mode::Walk);
    assert_ne!(
        unstuck(&mut s, &params, &shapes, &src),
        Ok(Stuck::Suffocated)
    );
    let (mut compared, mut moved) = (0, 0.0f64);
    for n in 0..=last {
        let before = s.pos;
        step(&mut s, &command(n), &params, &shapes, &src).expect("columns held");
        moved += nascio_core::torus::delta_f64(before[0], s.pos[0])
            .hypot(nascio_core::torus::delta_f64(before[2], s.pos[2]));
        if let Some(server) = states.get(&n) {
            assert_eq!(
                bits(server),
                bits(&s),
                "command {n}: server {server:?}, local {s:?}"
            );
            compared += 1;
        }
    }
    eprintln!(
        "movement: {lost} datagrams lost, {dups} duplicated, {lates} late; {compared} states \
         compared bit for bit; {moved:.1} m walked; server steps {} (without commands {})",
        stats.steps, stats.filled_steps
    );
    assert!(compared > 100, "{compared} states compared");
    assert!(moved > 10.0, "the player moved {moved} m");
    assert_eq!(stats.filled_steps, 0, "no step without commands");
}
