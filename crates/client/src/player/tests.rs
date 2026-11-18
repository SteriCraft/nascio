use super::*;
use nascio_chunk::Chunk;
use nascio_physics::Shape;
use nascio_protocol::chunk::Column;

const STONE: u16 = 1;

fn shapes() -> Shapes {
    Shapes::from_shapes(vec![Shape::Empty, Shape::Full])
}

/// Flat stone from y = 0 to 31 (the ground at 32) over the columns around the origin, but
/// those listed in `missing`.
fn ground(missing: &[(i32, i32)]) -> Mirror {
    let mut m = Mirror::default();
    for cz in -2..=2 {
        for cx in -2..=2 {
            if !missing.contains(&(cx, cz)) {
                m.column(column(cx, cz));
            }
        }
    }
    m
}

fn column(cx: i32, cz: i32) -> Column {
    Column {
        cx,
        cz,
        cy_min: 0,
        version: 0,
        chunks: vec![Chunk::filled(STONE)],
        tint: nascio_chunk::ChunkTint::NONE,
    }
}

fn player(t0: Instant) -> Player {
    Player::with(
        PlayerParams::default(),
        shapes(),
        [16.5, 32.0, 16.5],
        Mode::Walk,
        t0,
    )
}

fn state(tick: u32, ack: Option<u32>, pos: [f64; 3]) -> PlayerState {
    PlayerState {
        tick,
        ack,
        state: State {
            on_ground: true,
            ..State::new(pos, Mode::Walk)
        },
    }
}

fn ms(t0: Instant, ms: u64) -> Instant {
    t0 + Duration::from_millis(ms)
}

#[test]
fn forward_twice_within_300_ms_runs() {
    let t0 = Instant::now();
    let m = ground(&[]);
    let mut p = player(t0);
    let fwd = Controls {
        forward: true,
        ..Controls::default()
    };
    let idle = Controls::default();
    p.update(ms(t0, 0), &fwd, &m);
    p.update(ms(t0, 100), &idle, &m);
    assert!(!p.running());
    p.update(ms(t0, 250), &fwd, &m);
    assert!(p.running(), "second press 250 ms after the first");
    p.update(ms(t0, 2_000), &fwd, &m);
    assert!(p.running(), "held");
    p.update(ms(t0, 2_100), &idle, &m);
    assert!(!p.running(), "released");
    // Two presses 400 ms apart: walking.
    p.update(ms(t0, 2_500), &fwd, &m);
    assert!(!p.running());
    p.update(ms(t0, 2_600), &idle, &m);
    p.update(ms(t0, 2_900), &fwd, &m);
    assert!(!p.running(), "400 ms apart");
}

#[test]
fn small_corrections_are_smoothed_large_ones_snap() {
    let t0 = Instant::now();
    let m = ground(&[]);
    let mut p = player(t0);
    let here = [16.5, 32.0, 16.5];
    p.on_state(&state(1, None, here), &m);
    assert_eq!(p.display_pos(), here);
    assert!(p.events().contains(&PlayerEvent::Snapped) || p.stats().corrections == 0);
    // The server puts the player 0.5 m east: the view stays, then follows.
    p.on_state(&state(2, None, [17.0, 32.0, 16.5]), &m);
    assert_eq!(p.stats().corrections, 1);
    assert!((p.display_pos()[0] - 16.5).abs() < 1e-12);
    assert_eq!(p.state().pos[0], 17.0);
    p.update(ms(t0, 100), &Controls::default(), &m);
    let left = 17.0 - p.display_pos()[0];
    assert!((left - 0.5 * (-1.0f64).exp()).abs() < 1e-9, "{left}");
    p.update(ms(t0, 1_000), &Controls::default(), &m);
    assert_eq!(p.display_pos()[0], p.state().pos[0], "under 1 mm: none");
    assert!(!p.events().contains(&PlayerEvent::Snapped));
    // Three metres: at once.
    p.on_state(&state(3, None, [20.0, 32.0, 16.5]), &m);
    assert_eq!(p.display_pos()[0], 20.0);
    assert!(p.events().contains(&PlayerEvent::Snapped));
    // An older state is ignored.
    p.on_state(&state(2, None, [17.0, 32.0, 16.5]), &m);
    assert_eq!(p.state().pos[0], 20.0);
    assert_eq!(p.stats().stale, 1);
}

#[test]
fn an_unknown_cell_stops_the_prediction() {
    let t0 = Instant::now();
    // The player's column is missing.
    let mut m = ground(&[(0, 0)]);
    let mut p = player(t0);
    p.on_state(&state(1, None, [16.5, 32.0, 16.5]), &m);
    let fwd = Controls {
        forward: true,
        ..Controls::default()
    };
    let mut sent = Vec::new();
    for i in 1..=30 {
        if let Some(c) = p.update(ms(t0, i * 17), &fwd, &m) {
            sent.push(c);
        }
    }
    // One command made, not applied: frozen, but still sent.
    assert!(p.blocked());
    assert_eq!(p.unacked(), 1);
    assert_eq!(p.state().pos, [16.5, 32.0, 16.5]);
    assert!(sent.len() >= 5 && sent.iter().all(|c| c.last == 0 && c.inputs.len() == 1));
    // The column arrives: the prediction goes on.
    m.column(column(0, 0));
    for i in 31..=60 {
        p.update(ms(t0, i * 17), &fwd, &m);
    }
    assert!(!p.blocked());
    assert!(p.unacked() > 20);
    assert!(p.state().pos[2] < 16.0, "walked north: {:?}", p.state().pos);
}

#[test]
fn replays_the_commands_not_acknowledged() {
    let t0 = Instant::now();
    let m = ground(&[]);
    let mut p = player(t0);
    let start = [16.5, 32.0, 16.5];
    p.on_state(&state(1, None, start), &m);
    let fwd = Controls {
        forward: true,
        yaw: std::f64::consts::FRAC_PI_2,
        ..Controls::default()
    };
    let mut last = None;
    for i in 1..=60 {
        last = p.update(ms(t0, i * 1_000 / 60), &fwd, &m).or(last);
    }
    let c = last.expect("commands");
    assert!(c.inputs.iter().all(|i| i.keys == keys::FORWARD));
    // The server applied the first 10 exactly as we did: no correction.
    let mut s = State {
        on_ground: true,
        ..State::new(start, Mode::Walk)
    };
    let src = Blocks(&m);
    let input = c.inputs[0];
    for _ in 0..10 {
        nascio_physics::step(&mut s, &input, &PlayerParams::default(), &shapes(), &src).unwrap();
    }
    let before = *p.state();
    p.on_state(
        &PlayerState {
            tick: 2,
            ack: Some(9),
            state: s,
        },
        &m,
    );
    assert_eq!(p.stats().corrections, 0);
    assert!(same_bits(&before, p.state()));
    assert!(p.last_cmd() >= Some(c.last));
    // An acknowledgement beyond the newest command: numbering goes on one round trip of
    // steps (here none) and one send after it.
    p.on_state(&state(3, Some(500), start), &m);
    assert_eq!(p.stats().renumbered, 1);
    assert_eq!(p.unacked(), 0);
    let mut next = None;
    for i in 61..=70 {
        next = p.update(ms(t0, i * 1_000 / 60), &fwd, &m).or(next);
    }
    assert_eq!(next.unwrap().numbered().next().unwrap().0, 504);
    // With 100 ms of round trip: 6 steps more.
    p.set_rtt(Duration::from_millis(100));
    p.on_state(&state(4, Some(900), start), &m);
    let mut next = None;
    for i in 71..=80 {
        next = p.update(ms(t0, i * 1_000 / 60), &fwd, &m).or(next);
    }
    assert_eq!(next.unwrap().numbered().next().unwrap().0, 910);
    assert_eq!(p.stats().renumbered, 2);
}

#[test]
fn death_and_respawn_snap() {
    let t0 = Instant::now();
    let m = ground(&[]);
    let mut p = player(t0);
    p.on_state(&state(1, None, [16.5, 32.0, 16.5]), &m);
    p.events();
    p.on_msg(&ServerMsg::Died {
        cause: DeathCause::Magma,
    });
    assert!(p.dead());
    assert_eq!(
        p.events(),
        vec![PlayerEvent::Died(DeathCause::Magma), PlayerEvent::Snapped]
    );
    // Dead: commands keep going, the player does not move.
    let fwd = Controls {
        forward: true,
        ..Controls::default()
    };
    let mut c = None;
    for i in 1..=30 {
        c = p.update(ms(t0, i * 17), &fwd, &m).or(c);
    }
    assert!(c.unwrap().inputs.iter().all(|i| i.keys == 0));
    assert_eq!(p.state().pos, [16.5, 32.0, 16.5]);
    p.on_msg(&ServerMsg::Respawned {
        pos: [40.5, 32.0, 40.5],
    });
    assert!(!p.dead());
    assert_eq!(p.display_pos(), [40.5, 32.0, 40.5]);
    assert_eq!(
        p.events(),
        vec![PlayerEvent::Respawned, PlayerEvent::Snapped]
    );
}

#[test]
fn a_freeze_takes_three_steps_and_drops_the_rest() {
    let t0 = Instant::now();
    let m = ground(&[]);
    let mut p = player(t0);
    p.on_state(&state(1, None, [16.5, 32.0, 16.5]), &m);
    p.update(ms(t0, 51), &Controls::default(), &m);
    let before = p.unacked();
    assert_eq!(before, 3);
    p.update(ms(t0, 551), &Controls::default(), &m);
    assert_eq!(p.unacked(), before + 3);
    p.update(ms(t0, 568), &Controls::default(), &m);
    assert_eq!(p.unacked(), before + 4, "the rest dropped");
}
