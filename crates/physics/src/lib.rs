//! Player physics shared by the server, which has the authority, and the client, which
//! predicts its own player with the same code and the same parameters ([`PlayerParams`]).
//!
//! A player is an axis-aligned box standing on its feet. [`step`] advances it by one fixed
//! step under one [`Input`]: the same state, input, parameters and blocks give the same bits on
//! every platform (basic IEEE operations, [`DetMath`] for sine and cosine, no fused
//! multiply-add, and no order-dependent reductions).
//!
//! Horizontal coordinates are periodic: the box is tested against cells taken around its
//! unwrapped position, each looked up at its wrapped address, and the position is wrapped
//! after each step (invariant 1).

pub mod params;

pub use params::PlayerParams;

use nascio_chunk::mesh::LAYER_H;
use nascio_chunk::{BlockId, Blocks, Matter};
use nascio_core::fmath::DetMath;
use nascio_core::torus::{wrap, wrap_f64};
use serde::{Deserialize, Serialize};

/// Bits of [`Input::keys`].
pub mod keys {
    pub const FORWARD: u8 = 1;
    pub const BACK: u8 = 1 << 1;
    pub const LEFT: u8 = 1 << 2;
    pub const RIGHT: u8 = 1 << 3;
    /// Jump; swim up; fly up.
    pub const JUMP: u8 = 1 << 4;
    pub const RUN: u8 = 1 << 5;
    /// Crouch; swim down; fly down.
    pub const CROUCH: u8 = 1 << 6;
}

/// What the player asks for during one step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Input {
    /// [`keys`] held.
    pub keys: u8,
    /// Heading from north (−z) towards east (+x), in 1/65 536 of a turn.
    pub yaw: u16,
    /// Asked flight speed, 0.1 m/s (capped by [`PlayerParams::fly_max_speed`]).
    pub fly_speed: u16,
}

impl Input {
    /// Heading in radians to [`Input::yaw`] (client side: platform maths are fine here, the
    /// quantized value is what both sides simulate with).
    pub fn quantize_yaw(radians: f64) -> u16 {
        let turns = (radians / std::f64::consts::TAU).rem_euclid(1.0);
        ((turns * 65_536.0).round() as u32 % 65_536) as u16
    }

    /// [`Input::yaw`] in radians.
    pub fn yaw_radians(&self) -> f64 {
        f64::from(self.yaw) * (std::f64::consts::TAU / 65_536.0)
    }

    fn held(&self, k: u8) -> bool {
        self.keys & k != 0
    }
}

/// How the player moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    /// Gravity, walking, jumping, swimming.
    #[default]
    Walk,
    /// No gravity, blocks still collide (administrators).
    Fly,
}

/// The simulated state of a player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Centre of the feet; x and z wrapped into `[0, WORLD_SIZE)`.
    pub pos: [f64; 3],
    /// m/s.
    pub vel: [f64; 3],
    pub on_ground: bool,
    /// The lower half of the body is in a fluid.
    pub in_fluid: bool,
    pub mode: Mode,
}

impl State {
    pub fn new(pos: [f64; 3], mode: Mode) -> Self {
        Self {
            pos: [wrap_f64(pos[0]), pos[1], wrap_f64(pos[2])],
            mode,
            ..Self::default()
        }
    }

    /// Position of the eyes.
    pub fn eye(&self, p: &PlayerParams) -> [f64; 3] {
        [self.pos[0], self.pos[1] + p.eye_height, self.pos[2]]
    }
}

/// How a block collides with a player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Passes through.
    Empty,
    /// A full cube.
    Full,
    /// A fluid: passes through, the player swims.
    Fluid,
    /// A thin layer of [`LAYER_H`] at the bottom of its cell.
    Layer,
}

/// Collision shape of each block of a world, by id. Built from the block's matter today; a
/// per-block collision property can be plugged here later.
#[derive(Clone, Debug)]
pub struct Shapes(Vec<Shape>);

impl Shapes {
    pub fn new(blocks: &Blocks) -> Self {
        let n = blocks.len();
        Self(
            (0..n)
                .map(|id| match blocks.matter(id as BlockId) {
                    Matter::Empty => Shape::Empty,
                    Matter::Solid => Shape::Full,
                    Matter::Fluid => Shape::Fluid,
                    Matter::Layer => Shape::Layer,
                })
                .collect(),
        )
    }

    /// From explicit shapes (tests).
    pub fn from_shapes(shapes: Vec<Shape>) -> Self {
        Self(shapes)
    }

    /// Shape of a block; an id the table does not know is a full cube.
    pub fn get(&self, id: BlockId) -> Shape {
        self.0.get(usize::from(id)).copied().unwrap_or(Shape::Full)
    }
}

/// The blocks around a player: `None` where they are not known yet (column not loaded).
pub trait BlockSource {
    /// Block at a cell; `x` and `z` are already wrapped into `[0, WORLD_SIZE)`.
    fn block(&self, x: i32, y: i32, z: i32) -> Option<BlockId>;
}

impl<F: Fn(i32, i32, i32) -> Option<BlockId>> BlockSource for F {
    fn block(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        self(x, y, z)
    }
}

/// A cell needed by the step is not known: the player waits, unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unloaded;

/// Result of [`unstuck`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stuck {
    /// The player overlaps no block.
    Free,
    /// The player was inside blocks and was put on top of them.
    Pushed,
    /// The player is inside blocks under a solid block: suffocation.
    Suffocated,
}

/// Tolerance of contacts, m: faces closer than this touch without overlapping.
const EPS: f64 = 1e-6;

#[derive(Clone, Copy, Debug)]
struct Aabb {
    min: [f64; 3],
    max: [f64; 3],
}

impl Aabb {
    fn of(pos: [f64; 3], p: &PlayerParams) -> Self {
        let h = p.width * 0.5;
        Self {
            min: [pos[0] - h, pos[1], pos[2] - h],
            max: [pos[0] + h, pos[1] + p.height, pos[2] + h],
        }
    }
}

/// Cells overlapping `[lo, hi]` (contacts within [`EPS`] excluded).
fn cells(lo: f64, hi: f64) -> std::ops::RangeInclusive<i32> {
    ((lo + EPS).floor() as i32)..=((hi - EPS).ceil() as i32 - 1)
}

struct World<'a, S> {
    p: &'a PlayerParams,
    shapes: &'a Shapes,
    src: &'a S,
}

impl<S: BlockSource> World<'_, S> {
    fn shape(&self, x: i32, y: i32, z: i32) -> Result<Shape, Unloaded> {
        self.src
            .block(wrap(x), y, wrap(z))
            .map(|id| self.shapes.get(id))
            .ok_or(Unloaded)
    }

    /// Top of a cell's collision box above its floor, if it collides.
    fn top(shape: Shape) -> Option<f64> {
        match shape {
            Shape::Full => Some(1.0),
            Shape::Layer => Some(f64::from(LAYER_H)),
            Shape::Empty | Shape::Fluid => None,
        }
    }

    /// Calls `f` with the collision box of each colliding cell overlapping `b`.
    fn each_collider(
        &self,
        b: &Aabb,
        mut f: impl FnMut([f64; 3], [f64; 3]),
    ) -> Result<(), Unloaded> {
        for y in cells(b.min[1], b.max[1]) {
            for x in cells(b.min[0], b.max[0]) {
                for z in cells(b.min[2], b.max[2]) {
                    if let Some(top) = Self::top(self.shape(x, y, z)?) {
                        let (fx, fy, fz) = (f64::from(x), f64::from(y), f64::from(z));
                        f([fx, fy, fz], [fx + 1.0, fy + top, fz + 1.0]);
                    }
                }
            }
        }
        Ok(())
    }

    /// The box overlaps a colliding cell.
    fn collides(&self, b: &Aabb) -> Result<bool, Unloaded> {
        let mut hit = false;
        self.each_collider(b, |lo, hi| {
            hit |= (0..3).all(|a| hi[a] > b.min[a] + EPS && lo[a] < b.max[a] - EPS);
        })?;
        Ok(hit)
    }

    /// The lower half of the box is in a fluid.
    fn in_fluid(&self, b: &Aabb) -> Result<bool, Unloaded> {
        let top = b.min[1] + (b.max[1] - b.min[1]) * 0.5;
        for y in cells(b.min[1], top) {
            for x in cells(b.min[0], b.max[0]) {
                for z in cells(b.min[2], b.max[2]) {
                    if self.shape(x, y, z)? == Shape::Fluid {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    /// How far the box can move along axis `a` up to `d` before touching a colliding cell.
    /// Cells it already overlaps do not stop it (see [`unstuck`]).
    fn clip(&self, b: &Aabb, a: usize, d: f64) -> Result<f64, Unloaded> {
        if d == 0.0 {
            return Ok(0.0);
        }
        let mut swept = *b;
        if d > 0.0 {
            swept.max[a] += d;
        } else {
            swept.min[a] += d;
        }
        let mut d = d;
        self.each_collider(&swept, |lo, hi| {
            let across =
                (0..3).all(|o| o == a || (hi[o] > b.min[o] + EPS && lo[o] < b.max[o] - EPS));
            if !across {
                return;
            }
            // Order-independent reductions: the same result whatever the cell order.
            if d > 0.0 && lo[a] >= b.max[a] - EPS {
                d = d.min((lo[a] - b.max[a]).max(0.0));
            } else if d < 0.0 && hi[a] <= b.min[a] + EPS {
                d = d.max((hi[a] - b.min[a]).min(0.0));
            }
        })?;
        Ok(d)
    }

    /// Moves the box by `d`, axis by axis (y, x, z); returns the position and the axes cut
    /// short by a block.
    fn slide(&self, pos: [f64; 3], d: [f64; 3]) -> Result<([f64; 3], [bool; 3]), Unloaded> {
        let mut pos = pos;
        let mut cut = [false; 3];
        for a in [1, 0, 2] {
            let m = self.clip(&Aabb::of(pos, self.p), a, d[a])?;
            cut[a] = m != d[a];
            pos[a] += m;
        }
        Ok((pos, cut))
    }
}

/// Moves `v` towards `t` by at most `max` (horizontal vectors).
fn approach(v: [f64; 2], t: [f64; 2], max: f64) -> [f64; 2] {
    let d = [t[0] - v[0], t[1] - v[1]];
    let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if l <= max {
        t
    } else {
        let k = max / l;
        [v[0] + d[0] * k, v[1] + d[1] * k]
    }
}

/// Moves `v` towards `t` by at most `max`.
fn approach1(v: f64, t: f64, max: f64) -> f64 {
    if (t - v).abs() <= max {
        t
    } else if t > v {
        v + max
    } else {
        v - max
    }
}

/// Unit horizontal direction asked by the keys (x, z), or zero.
fn wish(input: &Input) -> [f64; 2] {
    let axis = |pos: u8, neg: u8| {
        f64::from(u8::from(input.held(pos))) - f64::from(u8::from(input.held(neg)))
    };
    let fwd = axis(keys::FORWARD, keys::BACK);
    let right = axis(keys::RIGHT, keys::LEFT);
    if fwd == 0.0 && right == 0.0 {
        return [0.0, 0.0];
    }
    let (s, c) = input.yaw_radians().det_sin_cos();
    // Forward is (sin, −cos), right is (cos, sin).
    let v = [fwd * s + right * c, -fwd * c + right * s];
    let l = (v[0] * v[0] + v[1] * v[1]).sqrt();
    [v[0] / l, v[1] / l]
}

/// Advances the player by one step of [`PlayerParams::dt`]. If a cell it needs is not loaded,
/// the state is left as it was and [`Unloaded`] is returned.
pub fn step(
    state: &mut State,
    input: &Input,
    p: &PlayerParams,
    shapes: &Shapes,
    src: &impl BlockSource,
) -> Result<(), Unloaded> {
    let w = World { p, shapes, src };
    let dt = p.dt();
    let mut s = *state;
    let b = Aabb::of(s.pos, p);
    s.in_fluid = w.in_fluid(&b)?;
    let dir = wish(input);
    let jump = input.held(keys::JUMP);
    let crouch = input.held(keys::CROUCH);

    match s.mode {
        Mode::Fly => {
            let speed = (f64::from(input.fly_speed) * 0.1).min(p.fly_max_speed);
            let up = f64::from(u8::from(jump)) - f64::from(u8::from(crouch));
            s.vel = [dir[0] * speed, up * speed, dir[1] * speed];
        }
        Mode::Walk => {
            let speed = if s.in_fluid {
                p.swim_speed
            } else if crouch {
                p.crouch_speed
            } else if input.held(keys::RUN) {
                p.run_speed
            } else {
                p.walk_speed
            };
            let accel = if s.on_ground || s.in_fluid {
                p.ground_accel
            } else {
                p.air_accel
            };
            let h = approach(
                [s.vel[0], s.vel[2]],
                [dir[0] * speed, dir[1] * speed],
                accel * dt,
            );
            s.vel[0] = h[0];
            s.vel[2] = h[1];
            if s.in_fluid {
                let target = if jump {
                    p.swim_speed
                } else if crouch {
                    -p.swim_speed
                } else {
                    -p.sink_speed
                };
                s.vel[1] = approach1(s.vel[1], target, p.fluid_accel * dt);
            } else {
                if s.on_ground && jump {
                    s.vel[1] = (2.0 * p.gravity * p.jump_height).sqrt();
                }
                s.vel[1] = (s.vel[1] - p.gravity * dt).max(-p.max_fall_speed);
            }
        }
    }

    let d = [s.vel[0] * dt, s.vel[1] * dt, s.vel[2] * dt];
    let (mut pos, mut cut) = w.slide(s.pos, d)?;

    // Step up a low ledge (a snow layer; never a full block) while walking on the ground.
    if s.mode == Mode::Walk
        && state.on_ground
        && d[1] <= 0.0
        && (cut[0] || cut[2])
        && p.step_height > 0.0
    {
        let up = w.clip(&Aabb::of(s.pos, p), 1, p.step_height)?;
        let (mut alt, alt_cut) = w.slide([s.pos[0], s.pos[1] + up, s.pos[2]], [d[0], 0.0, d[2]])?;
        let down = w.clip(&Aabb::of(alt, p), 1, -up)?;
        alt[1] += down;
        let gained = |q: [f64; 3]| {
            let (dx, dz) = (q[0] - s.pos[0], q[2] - s.pos[2]);
            dx * dx + dz * dz
        };
        if gained(alt) > gained(pos) + EPS * EPS {
            pos = alt;
            // Landed on the ledge: the vertical motion of this step stopped there.
            cut = [alt_cut[0], down != -up, alt_cut[2]];
        }
    }

    // Leap out of a fluid against a bank.
    if s.mode == Mode::Walk && s.in_fluid && jump && (cut[0] || cut[2]) {
        s.vel[1] = s.vel[1].max((2.0 * p.gravity * p.jump_height).sqrt());
    }

    for (v, c) in s.vel.iter_mut().zip(cut) {
        if c {
            *v = 0.0;
        }
    }
    s.on_ground = cut[1] && d[1] < 0.0;
    // Coming down onto a block ends the flight (author's decision).
    if s.mode == Mode::Fly && s.on_ground {
        s.mode = Mode::Walk;
    }
    s.pos = [wrap_f64(pos[0]), pos[1], wrap_f64(pos[2])];
    *state = s;
    Ok(())
}

/// Frees a player found inside blocks (a block placed or a pit filled on it): put on top of
/// the blocks it overlaps, unless the cells just above its head collide (suffocation).
pub fn unstuck(
    state: &mut State,
    p: &PlayerParams,
    shapes: &Shapes,
    src: &impl BlockSource,
) -> Result<Stuck, Unloaded> {
    let w = World { p, shapes, src };
    let b = Aabb::of(state.pos, p);
    let mut top = f64::NEG_INFINITY;
    w.each_collider(&b, |lo, hi| {
        if (0..3).all(|a| hi[a] > b.min[a] + EPS && lo[a] < b.max[a] - EPS) {
            top = top.max(hi[1]);
        }
    })?;
    if top == f64::NEG_INFINITY {
        return Ok(Stuck::Free);
    }
    // The cells right above the head.
    let above = Aabb {
        min: [b.min[0], b.max[1], b.min[2]],
        max: [b.max[0], b.max[1] + 1.0, b.max[2]],
    };
    let row = (b.max[1] - EPS).floor() + 1.0;
    let mut blocked = false;
    w.each_collider(&above, |lo, hi| {
        let across = [0, 2]
            .iter()
            .all(|&a| hi[a] > b.min[a] + EPS && lo[a] < b.max[a] - EPS);
        blocked |= across && lo[1] == row;
    })?;
    if blocked {
        return Ok(Stuck::Suffocated);
    }
    let pos = [state.pos[0], top, state.pos[2]];
    if w.collides(&Aabb::of(pos, p))? {
        return Ok(Stuck::Suffocated);
    }
    state.pos = pos;
    state.vel = [0.0; 3];
    state.on_ground = true;
    Ok(Stuck::Pushed)
}

/// Result of [`free_place`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    /// The box was free where it stood: unchanged.
    Stayed,
    /// Moved to the nearest free place.
    Moved,
    /// No free place within the radius: the player suffocates.
    Nowhere,
}

/// Puts a player arriving inside blocks (login at its last position, respawn) in the nearest
/// place where its box fits, within `radius` metres of its feet.
///
/// Unknown cells (`None`: a column not loaded, or below a column's chunks) count as taken: the
/// caller loads the columns within `radius` plus one metre first. A box free where it stands
/// does not move. Otherwise the candidates are the centres of the cells, the feet on the floor
/// of the cell or on a snow layer in it, sorted by distance to the feet (on the torus), then
/// highest first, then by x and by z; the first where the box fits wins. Its speed is reset;
/// it falls from there.
pub fn free_place(
    state: &mut State,
    p: &PlayerParams,
    shapes: &Shapes,
    src: &impl BlockSource,
    radius: f64,
) -> Placed {
    let w = World { p, shapes, src };
    let fits = |pos: [f64; 3]| matches!(w.collides(&Aabb::of(pos, p)), Ok(false));
    if fits(state.pos) {
        return Placed::Stayed;
    }
    // Offsets from the cell of the feet, unwrapped: the order does not depend on where the
    // seam is.
    let [px, py, pz] = state.pos;
    let (bx, by, bz) = (px.floor(), py.floor(), pz.floor());
    let r = radius.max(0.0).ceil() as i32 + 1;
    let mut cands: Vec<(f64, f64, i32, i32, [f64; 3])> = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            for dz in -r..=r {
                let (cx, cy, cz) = (bx + f64::from(dx), by + f64::from(dy), bz + f64::from(dz));
                let floors = [Some(0.0), {
                    let cell = w.shape(cx as i32, cy as i32, cz as i32);
                    (cell == Ok(Shape::Layer)).then_some(f64::from(LAYER_H))
                }];
                for lift in floors.into_iter().flatten() {
                    let q = [cx + 0.5, cy + lift, cz + 0.5];
                    let d = [q[0] - px, q[1] - py, q[2] - pz];
                    let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                    if d2 <= radius * radius {
                        cands.push((d2, q[1], dx, dz, q));
                    }
                }
            }
        }
    }
    cands.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then(b.1.total_cmp(&a.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(&b.3))
    });
    match cands.into_iter().find(|c| fits(c.4)) {
        Some((.., q)) => {
            state.pos = [wrap_f64(q[0]), q[1], wrap_f64(q[2])];
            state.vel = [0.0; 3];
            state.on_ground = false;
            state.in_fluid = false;
            Placed::Moved
        }
        None => Placed::Nowhere,
    }
}

/// Whether the client should jump for the player this step (the auto-jump setting): walking
/// on the ground into a one-block ledge with room above it.
pub fn auto_jump(
    state: &State,
    input: &Input,
    p: &PlayerParams,
    shapes: &Shapes,
    src: &impl BlockSource,
) -> bool {
    if state.mode != Mode::Walk || !state.on_ground || state.in_fluid {
        return false;
    }
    let dir = wish(input);
    if dir == [0.0, 0.0] {
        return false;
    }
    let w = World { p, shapes, src };
    let reach = p.width * 0.5;
    let ahead = [
        state.pos[0] + dir[0] * reach,
        state.pos[1],
        state.pos[2] + dir[1] * reach,
    ];
    let raised = |q: [f64; 3]| [q[0], q[1] + 1.0 + EPS, q[2]];
    matches!(
        (
            w.collides(&Aabb::of(ahead, p)),
            w.collides(&Aabb::of(raised(ahead), p)),
            w.collides(&Aabb::of(raised(state.pos), p)),
        ),
        (Ok(true), Ok(false), Ok(false))
    )
}

#[cfg(test)]
mod tests;
