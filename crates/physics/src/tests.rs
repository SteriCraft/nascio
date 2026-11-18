use super::*;
use nascio_core::WORLD_SIZE;
use nascio_core::torus::delta_f64;
use proptest::prelude::*;
use std::collections::BTreeMap;

const AIR: BlockId = 0;
const STONE: BlockId = 1;
const WATER: BlockId = 2;
const SNOW: BlockId = 3;

fn shapes() -> Shapes {
    Shapes::from_shapes(vec![Shape::Empty, Shape::Full, Shape::Fluid, Shape::Layer])
}

/// Flat stone ground (top at y = 10) plus explicit cells, keyed by wrapped address.
#[derive(Default)]
struct Scene {
    cells: BTreeMap<(i32, i32, i32), BlockId>,
    /// Columns that are not loaded (wrapped x, z).
    unloaded: Vec<(i32, i32)>,
}

impl Scene {
    fn set(&mut self, x: i32, y: i32, z: i32, b: BlockId) {
        self.cells.insert((wrap(x), y, wrap(z)), b);
    }
}

impl BlockSource for Scene {
    fn block(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        assert!((0..WORLD_SIZE).contains(&x) && (0..WORLD_SIZE).contains(&z));
        if self.unloaded.contains(&(x, z)) {
            return None;
        }
        Some(
            self.cells
                .get(&(x, y, z))
                .copied()
                .unwrap_or(if y < 10 { STONE } else { AIR }),
        )
    }
}

fn run(s: &mut State, input: Input, steps: usize, scene: &Scene) {
    let p = PlayerParams::default();
    for _ in 0..steps {
        step(s, &input, &p, &shapes(), scene).unwrap();
    }
}

fn keys(k: u8, yaw_deg: f64) -> Input {
    Input {
        keys: k,
        yaw: Input::quantize_yaw(yaw_deg.to_radians()),
        fly_speed: 0,
    }
}

const EAST: f64 = 90.0;

#[test]
fn falls_and_lands_on_the_ground() {
    let mut s = State::new([500.5, 20.0, 500.5], Mode::Walk);
    run(&mut s, Input::default(), 120, &Scene::default());
    assert_eq!(s.pos[1], 10.0);
    assert!(s.on_ground);
    assert_eq!(s.vel, [0.0; 3]);
}

#[test]
fn fall_speed_is_capped() {
    let mut s = State::new([500.5, 5_000.0, 500.5], Mode::Walk);
    run(&mut s, Input::default(), 600, &Scene::default());
    assert_eq!(s.vel[1], -PlayerParams::default().max_fall_speed);
}

#[test]
fn walks_at_walking_speed_and_runs() {
    let mut s = State::new([500.5, 10.0, 500.5], Mode::Walk);
    run(&mut s, keys(keys::FORWARD, EAST), 60, &Scene::default());
    assert!((s.vel[0] - 4.3).abs() < 1e-9, "{:?}", s.vel);
    assert!(s.vel[2].abs() < 1e-9);
    run(
        &mut s,
        keys(keys::FORWARD | keys::RUN, EAST),
        60,
        &Scene::default(),
    );
    assert!((s.vel[0] - 5.6).abs() < 1e-9);
    run(
        &mut s,
        keys(keys::FORWARD | keys::CROUCH, EAST),
        60,
        &Scene::default(),
    );
    assert!((s.vel[0] - 1.3).abs() < 1e-9);
    // North is −z.
    let mut s = State::new([500.5, 10.0, 500.5], Mode::Walk);
    run(&mut s, keys(keys::FORWARD, 0.0), 60, &Scene::default());
    assert!((s.vel[2] + 4.3).abs() < 1e-9 && s.vel[0].abs() < 1e-9);
}

#[test]
fn a_wall_stops_the_player_at_its_face() {
    let mut scene = Scene::default();
    scene.set(503, 10, 500, STONE);
    scene.set(503, 11, 500, STONE);
    let mut s = State::new([500.5, 10.0, 500.5], Mode::Walk);
    run(&mut s, keys(keys::FORWARD, EAST), 120, &scene);
    assert!((s.pos[0] - 502.7).abs() < 1e-9, "{:?}", s.pos);
    assert_eq!(s.vel[0], 0.0);
    assert_eq!(s.pos[1], 10.0);
}

#[test]
fn a_full_block_is_not_climbed_without_jumping() {
    let mut scene = Scene::default();
    scene.set(503, 10, 500, STONE);
    let mut s = State::new([500.5, 10.0, 500.5], Mode::Walk);
    run(&mut s, keys(keys::FORWARD, EAST), 120, &scene);
    assert!((s.pos[0] - 502.7).abs() < 1e-9);
    assert_eq!(s.pos[1], 10.0);
    // Jumping while walking climbs it.
    run(&mut s, keys(keys::FORWARD | keys::JUMP, EAST), 30, &scene);
    run(&mut s, keys(keys::FORWARD, EAST), 60, &scene);
    assert!(s.pos[0] > 503.3, "{:?}", s.pos);
    assert_eq!(s.pos[1], 10.0);
}

#[test]
fn a_snow_layer_is_climbed_without_jumping() {
    let mut scene = Scene::default();
    for x in 503..510 {
        scene.set(x, 10, 500, SNOW);
    }
    let mut s = State::new([500.5, 10.0, 500.5], Mode::Walk);
    run(&mut s, keys(keys::FORWARD, EAST), 120, &scene);
    assert!(s.pos[0] > 505.0, "{:?}", s.pos);
    assert_eq!(s.pos[1], 10.125);
    assert!(s.on_ground);
}

#[test]
fn a_jump_rises_by_the_jump_height() {
    let mut s = State::new([500.5, 10.0, 500.5], Mode::Walk);
    run(&mut s, Input::default(), 2, &Scene::default());
    let mut top = 0.0f64;
    let p = PlayerParams::default();
    for i in 0..120 {
        let k = if i == 0 { keys::JUMP } else { 0 };
        step(&mut s, &keys(k, 0.0), &p, &shapes(), &Scene::default()).unwrap();
        top = top.max(s.pos[1]);
    }
    assert!((top - 11.25).abs() < 0.1, "{top}");
    assert_eq!(s.pos[1], 10.0);
}

#[test]
fn the_ceiling_stops_a_jump() {
    let mut scene = Scene::default();
    scene.set(500, 12, 500, STONE);
    let mut s = State::new([500.5, 10.0, 500.5], Mode::Walk);
    run(&mut s, Input::default(), 2, &scene);
    let p = PlayerParams::default();
    let mut top = 0.0f64;
    for _ in 0..60 {
        step(&mut s, &keys(keys::JUMP, 0.0), &p, &shapes(), &scene).unwrap();
        top = top.max(s.pos[1]);
    }
    assert!(top <= 10.2 + 1e-9, "{top}");
}

#[test]
fn swims_sinks_and_rises() {
    let mut scene = Scene::default();
    for y in 10..20 {
        for x in 495..506 {
            for z in 495..506 {
                scene.set(x, y, z, WATER);
            }
        }
    }
    let mut s = State::new([500.5, 15.0, 500.5], Mode::Walk);
    run(&mut s, Input::default(), 120, &scene);
    assert!(s.in_fluid);
    assert!((s.vel[1] + 1.0).abs() < 1e-9, "{:?}", s.vel);
    let y = s.pos[1];
    run(&mut s, keys(keys::JUMP, 0.0), 60, &scene);
    assert!(s.pos[1] > y + 1.0);
    run(&mut s, keys(keys::FORWARD | keys::RUN, EAST), 60, &scene);
    assert!(s.vel[0] <= 2.0 + 1e-9);
}

#[test]
fn flight_has_no_gravity_and_collides() {
    let mut scene = Scene::default();
    scene.set(503, 30, 500, STONE);
    scene.set(503, 31, 500, STONE);
    let mut s = State::new([500.5, 30.0, 500.5], Mode::Fly);
    let mut i = keys(keys::FORWARD, EAST);
    i.fly_speed = 100;
    run(&mut s, i, 120, &scene);
    assert_eq!(s.pos[1], 30.0);
    assert!((s.pos[0] - 502.7).abs() < 1e-9);
    // Capped at 120 m/s.
    let mut s = State::new([500.5, 30.0, 500.5], Mode::Fly);
    i.keys = keys::JUMP;
    i.fly_speed = 60_000;
    run(&mut s, i, 1, &scene);
    assert_eq!(s.vel[1], 120.0);
    // Down to the ground and no further.
    i.keys = keys::CROUCH;
    run(&mut s, i, 60, &scene);
    assert_eq!(s.pos[1], 10.0);
}

#[test]
fn an_unloaded_column_freezes_the_player() {
    let mut scene = Scene::default();
    scene.unloaded.push((501, 500));
    let mut s = State::new([500.5, 10.0, 500.5], Mode::Walk);
    run(&mut s, Input::default(), 2, &scene);
    let p = PlayerParams::default();
    let i = keys(keys::FORWARD, EAST);
    let mut res = Ok(());
    for _ in 0..60 {
        res = step(&mut s, &i, &p, &shapes(), &scene);
        if res.is_err() {
            break;
        }
    }
    assert_eq!(res, Err(Unloaded));
    let before = s;
    assert_eq!(step(&mut s, &i, &p, &shapes(), &scene), Err(Unloaded));
    assert_eq!(s, before);
    assert!(s.pos[0] < 501.0);
}

#[test]
fn crossing_the_seam() {
    // A wall just across the seam stops a player walking east.
    let mut scene = Scene::default();
    scene.set(0, 10, 500, STONE);
    scene.set(0, 11, 500, STONE);
    let mut s = State::new([99_998.5, 10.0, 500.5], Mode::Walk);
    run(&mut s, keys(keys::FORWARD, EAST), 120, &scene);
    assert!((s.pos[0] - 99_999.7).abs() < 1e-9, "{:?}", s.pos);
    // Without it, the player wraps and keeps going.
    let mut s = State::new([99_998.5, 10.0, 500.5], Mode::Walk);
    run(&mut s, keys(keys::FORWARD, EAST), 120, &Scene::default());
    assert!(s.pos[0] > 0.0 && s.pos[0] < 10.0, "{:?}", s.pos);
    // Westward, from 0.5 across the seam to 99 99x.
    let mut s = State::new([0.5, 10.0, 500.5], Mode::Walk);
    run(&mut s, keys(keys::FORWARD, 270.0), 60, &Scene::default());
    assert!(s.pos[0] > 99_990.0, "{:?}", s.pos);
}

#[test]
fn a_block_on_the_feet_pushes_the_player_up() {
    let mut scene = Scene::default();
    scene.set(500, 10, 500, STONE);
    let mut s = State::new([500.5, 10.0, 500.5], Mode::Walk);
    let p = PlayerParams::default();
    assert_eq!(unstuck(&mut s, &p, &shapes(), &scene), Ok(Stuck::Pushed));
    assert_eq!(s.pos[1], 11.0);
    assert_eq!(unstuck(&mut s, &p, &shapes(), &scene), Ok(Stuck::Free));
}

#[test]
fn a_solid_block_above_the_head_suffocates() {
    let mut scene = Scene::default();
    scene.set(500, 10, 500, STONE);
    scene.set(500, 11, 500, STONE);
    scene.set(500, 12, 500, STONE);
    let mut s = State::new([500.5, 10.0, 500.5], Mode::Walk);
    let p = PlayerParams::default();
    assert_eq!(
        unstuck(&mut s, &p, &shapes(), &scene),
        Ok(Stuck::Suffocated)
    );
    // Head inside a block but free above it: put on top.
    scene.set(500, 12, 500, AIR);
    scene.set(500, 10, 500, AIR);
    assert_eq!(unstuck(&mut s, &p, &shapes(), &scene), Ok(Stuck::Pushed));
    assert_eq!(s.pos[1], 12.0);
}

#[test]
fn auto_jump_sees_a_one_block_ledge() {
    let mut scene = Scene::default();
    scene.set(501, 10, 500, STONE);
    let p = PlayerParams::default();
    let mut s = State::new([500.6, 10.0, 500.5], Mode::Walk);
    run(&mut s, Input::default(), 2, &scene);
    let i = keys(keys::FORWARD, EAST);
    assert!(auto_jump(&s, &i, &p, &shapes(), &scene));
    // Not a two-block wall.
    scene.set(501, 11, 500, STONE);
    assert!(!auto_jump(&s, &i, &p, &shapes(), &scene));
}

#[test]
fn params_round_trip_and_validate() {
    let p = PlayerParams::default();
    p.validate(20).unwrap();
    let t = toml::to_string(&p).unwrap();
    assert_eq!(toml::from_str::<PlayerParams>(&t).unwrap(), p);
    let bad = PlayerParams {
        step_hz: 50,
        ..p.clone()
    };
    assert!(bad.validate(20).is_err());
}

/// FNV-1a over the bits of a trajectory.
fn trajectory_hash(start: [f64; 3]) -> u64 {
    let mut scene = Scene::default();
    for i in 0..40 {
        scene.set(
            wrap(start[0] as i32 + 3 + i % 7),
            10 + i % 2,
            wrap(start[2] as i32 + i),
            STONE,
        );
        scene.set(
            wrap(start[0] as i32 - 4),
            10,
            wrap(start[2] as i32 + i),
            SNOW,
        );
    }
    let p = PlayerParams::default();
    let mut s = State::new(start, Mode::Walk);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for t in 0..2_000u32 {
        let k = [
            keys::FORWARD,
            keys::FORWARD | keys::JUMP,
            keys::FORWARD | keys::LEFT | keys::RUN,
            keys::BACK,
            keys::RIGHT | keys::CROUCH,
        ][(t / 97 % 5) as usize];
        let i = Input {
            keys: k,
            yaw: (t.wrapping_mul(2_654_435_761) >> 16) as u16 & 0x3fff,
            fly_speed: 0,
        };
        step(&mut s, &i, &p, &shapes(), &scene).unwrap();
        for v in s.pos.iter().chain(s.vel.iter()) {
            for b in v.to_bits().to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
            }
        }
    }
    h
}

/// The same bits on every platform (computed once, on Windows; the Linux run must match).
#[test]
fn trajectory_is_deterministic() {
    let h = trajectory_hash([1_234.5, 10.0, 99_990.25]);
    assert_eq!(h, trajectory_hash([1_234.5, 10.0, 99_990.25]));
    assert_eq!(h, TRAJECTORY_HASH, "{h:#018x}");
}

const TRAJECTORY_HASH: u64 = 0xc719_0387_5710_3832;

/// A periodic scene (period 20 divides `WORLD_SIZE`): stone pillars on a grid.
struct Pillars;

impl BlockSource for Pillars {
    fn block(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        let pillar = x % 20 == 7 && z % 20 == 3 && y < 12;
        let snow = x % 20 == 12 && y == 10;
        Some(if y < 10 || pillar {
            STONE
        } else if snow {
            SNOW
        } else {
            AIR
        })
    }
}

proptest! {
    /// The same walk near the seam and in the middle of the world gives the same motion.
    #[test]
    fn same_motion_across_the_seam(
        x in 0.0f64..20.0, z in 0.0f64..20.0, yaw in any::<u16>(), k in 0u8..128,
    ) {
        let p = PlayerParams::default();
        let input = Input { keys: k & !keys::CROUCH, yaw, fly_speed: 0 };
        // Start 0 to 20 m before the seam, and 30 000 m away (a multiple of the period).
        let near = [wrap_f64(-x), 10.0, wrap_f64(-z)];
        let far = [near[0] - 30_000.0, 10.0, near[2] - 30_000.0];
        let mut a = State::new(near, Mode::Walk);
        let mut b = State::new(far, Mode::Walk);
        for _ in 0..240 {
            step(&mut a, &input, &p, &shapes(), &Pillars).unwrap();
            step(&mut b, &input, &p, &shapes(), &Pillars).unwrap();
        }
        let dx = delta_f64(b.pos[0], a.pos[0]) - 30_000.0;
        let dz = delta_f64(b.pos[2], a.pos[2]) - 30_000.0;
        prop_assert!(dx.abs() < 1e-6 && dz.abs() < 1e-6, "{:?} {:?}", a.pos, b.pos);
        prop_assert!((a.pos[1] - b.pos[1]).abs() < 1e-6);
    }
}
