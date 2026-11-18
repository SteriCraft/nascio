//! The players' bodies, moved by the server (chantier 5.3).
//!
//! Each tick a body earns `steps_per_tick` steps of credit (3 at 60 Hz steps and a 20 Hz
//! tick), kept up to [`CREDIT_TICKS`] ticks' worth, and applies at most one step more than
//! that per tick: queued commands catch up after jitter, but over any span a client gets no
//! more than real time plus the cap — sending commands faster gains nothing.
//!
//! A command waits in the queue while a cell it needs is unknown to the server (it is
//! replayed later exactly as the client predicted it). When no command came for
//! [`STARVE_TICKS`] ticks (after the first one) — no datagram at all, not merely nothing new —
//! the server moves the body itself with no key
//! held, numbering these steps after the last command: they are acknowledged, and the real
//! commands, if they come later, are ignored. A client cannot hover by holding its commands.

use std::collections::VecDeque;

use nascio_chunk::BlockId;
use nascio_chunk::mesh::LAYER_H;
use nascio_core::torus;
use nascio_physics::{
    BlockSource, Input, Mode, Placed, PlayerParams, Shape, Shapes, State, Stuck, Unloaded,
    free_place, step, unstuck,
};
use nascio_protocol::messages::DeathCause;
use nascio_protocol::player::InputQueue;

/// Ticks of credit a body may keep (150 ms at 20 Hz).
pub const CREDIT_TICKS: u32 = 3;
/// Ticks without any command before the server moves the body without keys (250 ms).
pub const STARVE_TICKS: u32 = 5;
/// An arriving body inside blocks is put in the nearest free place within this distance of
/// its feet, m; the server loads the columns within one metre more first.
pub const FREE_RADIUS_M: f64 = 10.0;
/// Eye positions kept, by command number (one second of steps).
pub const EYES_KEPT: usize = 64;
/// Gap that counts as touching a deadly block, m.
const TOUCH_M: f64 = 0.01;
/// Faces closer than this touch without overlapping (as in the physics), m.
const EPS: f64 = 1e-6;

/// A player's body as the server simulates it.
#[derive(Clone, Debug)]
pub struct Body {
    pub state: State,
    /// Steps the body may still take.
    pub credit: u32,
    /// Ticks in a row with no command waiting and no datagram received.
    pub idle: u32,
    /// Datagrams of commands seen (`InputQueue::pushes`) at the last tick.
    pushes: u64,
    pub dead: bool,
    /// To be placed before moving (arrival, respawn): see [`Settle`].
    pub settle: Option<Settle>,
    /// To be freed by `unstuck` (shut in by an edit while its cells were unknown).
    pub unstick: bool,
    /// The last command applied: steps without commands keep its heading and flight speed.
    pub last: Input,
    /// Eyes after each recent command, oldest first.
    eyes: VecDeque<(u32, [f64; 3])>,
}

/// Where an arriving body is placed ([`free_place`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settle {
    /// At its last position (login): no free place kills it (suffocation).
    Last,
    /// At its spawn point (first login, respawn): no free place, another point is drawn
    /// (`redraws` so far).
    Home { redraws: u32 },
}

/// What a tick did to a body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Advance {
    /// Commands applied.
    pub applied: u32,
    /// Steps taken without commands.
    pub filled: u32,
    /// A command waits for unknown cells.
    pub waiting: bool,
    pub died: Option<DeathCause>,
    /// The body was being placed and no free place was found (the caller decides).
    pub nowhere: Option<Settle>,
}

/// What the physics needs, shared by every body.
pub struct Rules<'a> {
    pub params: &'a PlayerParams,
    pub shapes: &'a Shapes,
    /// The mantle's magma (deadly to touch), if the world has it.
    pub magma: Option<BlockId>,
    pub steps_per_tick: u32,
}

impl Body {
    /// A body arriving at its spawn point.
    pub fn new(pos: [f64; 3]) -> Self {
        Self::arriving(pos, Settle::Home { redraws: 0 })
    }

    /// A body arriving at `pos`, placed as `settle` says.
    pub fn arriving(pos: [f64; 3], settle: Settle) -> Self {
        Self {
            state: State::new(pos, Mode::Walk),
            credit: 0,
            idle: 0,
            pushes: 0,
            dead: false,
            settle: Some(settle),
            unstick: false,
            last: Input::default(),
            eyes: VecDeque::with_capacity(EYES_KEPT),
        }
    }

    /// Back to life at its spawn point `pos`.
    pub fn respawn(&mut self, pos: [f64; 3]) {
        self.restart(pos, Settle::Home { redraws: 0 });
    }

    /// Starts again at `pos`, placed as `settle` says (keeps the credit, the last command
    /// and the mode).
    pub fn restart(&mut self, pos: [f64; 3], settle: Settle) {
        let mode = self.state.mode;
        *self = Self {
            credit: self.credit,
            last: self.last,
            ..Self::arriving(pos, settle)
        };
        self.state.mode = mode;
    }

    /// Flight on or off (administrators).
    pub fn set_mode(&mut self, mode: Mode) {
        if self.state.mode != mode {
            self.state.mode = mode;
            self.state.vel = [0.0; 3];
            self.state.on_ground = false;
        }
    }

    /// Eyes after command `cmd`, or the present ones when it is not kept (`None`, too old).
    pub fn eye_at(&self, cmd: Option<u32>, p: &PlayerParams) -> [f64; 3] {
        cmd.and_then(|c| self.eyes.iter().rev().find(|e| e.0 == c))
            .map_or_else(|| self.state.eye(p), |e| e.1)
    }

    /// Whether the body overlaps cell `(x, y, z)` filled from its bottom to `top` metres.
    pub fn overlaps_cell(&self, p: &PlayerParams, cell: [i32; 3], top: f64) -> bool {
        let s = &self.state;
        let half = p.width * 0.5 + 0.5 - EPS;
        let dx = torus::delta_f64(f64::from(cell[0]) + 0.5, s.pos[0]);
        let dz = torus::delta_f64(f64::from(cell[2]) + 0.5, s.pos[2]);
        let y = f64::from(cell[1]);
        dx.abs() < half
            && dz.abs() < half
            && s.pos[1] < y + top - EPS
            && s.pos[1] + p.height > y + EPS
    }

    /// Frees the body from the blocks it overlaps; `Some` when it suffocates.
    pub fn unstuck(
        &mut self,
        r: &Rules,
        src: &impl BlockSource,
    ) -> Result<Option<DeathCause>, Unloaded> {
        Ok(match unstuck(&mut self.state, r.params, r.shapes, src)? {
            Stuck::Free | Stuck::Pushed => None,
            Stuck::Suffocated => Some(DeathCause::Suffocation),
        })
    }

    /// Arrival or respawn: the body is put in the nearest place where it fits within
    /// [`FREE_RADIUS_M`] ([`free_place`]; unknown cells count as taken). False: nowhere.
    pub fn place(&mut self, r: &Rules, src: &impl BlockSource) -> bool {
        free_place(&mut self.state, r.params, r.shapes, src, FREE_RADIUS_M) != Placed::Nowhere
    }

    /// One tick: credit, placing, the queued commands, then steps without commands if the
    /// client went quiet. A dead body only drains its queue (acknowledged, without effect).
    pub fn advance(&mut self, q: &mut InputQueue, r: &Rules, src: &impl BlockSource) -> Advance {
        let mut a = Advance::default();
        let spt = r.steps_per_tick;
        self.credit = (self.credit + spt).min(spt * CREDIT_TICKS);
        if self.dead {
            while q.pop().is_some() {}
            self.idle = 0;
            return a;
        }
        if let Some(settle) = self.settle {
            if !self.place(r, src) {
                a.nowhere = Some(settle);
                return a;
            }
            self.settle = None;
        }
        // Commands waiting, or a datagram came (even one bringing only commands already
        // filled): the client is still sending, the server does not step for it.
        let had = !q.is_empty() || q.pushes() != self.pushes;
        self.pushes = q.pushes();
        while a.applied <= spt && self.credit > 0 {
            let Some((n, input)) = q.peek() else { break };
            if step(&mut self.state, &input, r.params, r.shapes, src).is_err() {
                a.waiting = true;
                break;
            }
            q.pop();
            self.credit -= 1;
            a.applied += 1;
            self.last = input;
            if let Some(d) = self.after_step(n, r, src) {
                a.died = Some(d);
                return a;
            }
        }
        self.idle = if had { 0 } else { self.idle + 1 };
        if !had && self.idle > STARVE_TICKS && q.started() {
            let none = Input {
                keys: 0,
                ..self.last
            };
            while a.filled < spt && self.credit > 0 {
                if step(&mut self.state, &none, r.params, r.shapes, src).is_err() {
                    break;
                }
                if q.fill(1) == 0 {
                    break;
                }
                self.credit -= 1;
                a.filled += 1;
                let n = q.applied().unwrap_or(0);
                if let Some(d) = self.after_step(n, r, src) {
                    a.died = Some(d);
                    return a;
                }
            }
        }
        a
    }

    /// Keeps the eyes of command `n`; a death if the body touches magma.
    fn after_step(&mut self, n: u32, r: &Rules, src: &impl BlockSource) -> Option<DeathCause> {
        if self.eyes.len() >= EYES_KEPT {
            self.eyes.pop_front();
        }
        self.eyes.push_back((n, self.state.eye(r.params)));
        let magma = r.magma?;
        self.touches(r.params, src, magma)
            .then_some(DeathCause::Magma)
    }

    /// Whether a block `id` lies within [`TOUCH_M`] of the body.
    pub fn touches(&self, p: &PlayerParams, src: &impl BlockSource, id: BlockId) -> bool {
        let s = &self.state;
        let h = p.width * 0.5 + TOUCH_M;
        let range = |lo: f64, hi: f64| (lo.floor() as i32)..=(hi.floor() as i32);
        for y in range(s.pos[1] - TOUCH_M, s.pos[1] + p.height + TOUCH_M) {
            for z in range(s.pos[2] - h, s.pos[2] + h) {
                for x in range(s.pos[0] - h, s.pos[0] + h) {
                    let (wx, wz) = (
                        x.rem_euclid(nascio_core::consts::WORLD_SIZE),
                        z.rem_euclid(nascio_core::consts::WORLD_SIZE),
                    );
                    if src.block(wx, y, wz) == Some(id) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Height of what a block fills in its cell for the players (0: it does not collide).
pub fn solid_top(shapes: &Shapes, id: BlockId) -> f64 {
    match shapes.get(id) {
        Shape::Full => 1.0,
        Shape::Layer => f64::from(LAYER_H),
        Shape::Empty | Shape::Fluid => 0.0,
    }
}

/// Distance from `eye` to the nearest point of the cell `(x, y, z)`, m (torus).
pub fn reach(eye: [f64; 3], cell: [i32; 3]) -> f64 {
    let d = [
        torus::delta_f64(eye[0], f64::from(cell[0]) + 0.5),
        f64::from(cell[1]) + 0.5 - eye[1],
        torus::delta_f64(eye[2], f64::from(cell[2]) + 0.5),
    ];
    d.iter()
        .map(|v| (v.abs() - 0.5).max(0.0).powi(2))
        .sum::<f64>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nascio_physics::keys;
    use nascio_protocol::player::Commands;

    const GROUND: i32 = 10;
    const WORLD: f64 = nascio_core::consts::WORLD_SIZE as f64;
    const MAGMA: BlockId = 2;

    fn shapes() -> Shapes {
        Shapes::from_shapes(vec![Shape::Empty, Shape::Full, Shape::Full])
    }

    /// Flat ground: stone up to y = 10 (its top at 11); magma at x ≥ 50.
    fn flat(x: i32, y: i32, _z: i32) -> Option<BlockId> {
        Some(if y > GROUND {
            0
        } else if x >= 50 && y == GROUND {
            MAGMA
        } else {
            1
        })
    }

    fn rules<'a>(p: &'a PlayerParams, s: &'a Shapes) -> Rules<'a> {
        Rules {
            params: p,
            shapes: s,
            magma: Some(MAGMA),
            steps_per_tick: 3,
        }
    }

    fn forward(yaw: f64) -> Input {
        Input {
            keys: keys::FORWARD,
            yaw: Input::quantize_yaw(yaw),
            fly_speed: 0,
        }
    }

    fn push(q: &mut InputQueue, first: u32, inputs: Vec<Input>) {
        q.push(&Commands {
            last: first + inputs.len() as u32 - 1,
            pitch: 0,
            inputs,
        });
    }

    #[test]
    fn three_steps_a_tick_and_one_more_to_catch_up() {
        let (p, s) = (PlayerParams::default(), shapes());
        let r = rules(&p, &s);
        let mut b = Body::new([0.5, 11.0, 0.5]);
        let mut q = InputQueue::new(64);
        // A burst of 30 commands: at most 4 a tick, and the credit caps the total.
        push(&mut q, 0, vec![forward(0.0); 30]);
        let mut applied = Vec::new();
        for _ in 0..12 {
            applied.push(b.advance(&mut q, &r, &flat).applied);
        }
        assert_eq!(applied[0], 3, "credit of one tick");
        assert!(applied.iter().all(|&a| a <= 4), "{applied:?}");
        assert_eq!(applied.iter().sum::<u32>(), 30);
        // Commands sent too fast pile up: 12 ticks give at most 36 + 9 steps.
        let mut q = InputQueue::new(64);
        let mut b = Body::new([0.5, 11.0, 0.5]);
        let mut total = 0;
        for t in 0..12 {
            push(&mut q, t * 6, vec![forward(0.0); 6]);
            total += b.advance(&mut q, &r, &flat).applied;
        }
        assert!(total <= 36 + 9, "{total}");
    }

    #[test]
    fn a_quiet_client_falls_after_250_ms() {
        let (p, s) = (PlayerParams::default(), shapes());
        let r = rules(&p, &s);
        let mut b = Body::new([0.5, 30.0, 0.5]);
        b.settle = None;
        let mut q = InputQueue::new(64);
        // Nothing before the first command: the body waits where it is.
        for _ in 0..10 {
            assert_eq!(b.advance(&mut q, &r, &flat), Advance::default());
        }
        assert_eq!(b.state.pos[1], 30.0);
        push(&mut q, 0, vec![Input::default(); 3]);
        b.advance(&mut q, &r, &flat);
        let y = b.state.pos[1];
        for _ in 0..STARVE_TICKS {
            assert_eq!(b.advance(&mut q, &r, &flat).filled, 0);
        }
        assert_eq!(b.state.pos[1], y, "frozen for 250 ms");
        let a = b.advance(&mut q, &r, &flat);
        assert_eq!(a.filled, 3);
        assert!(b.state.pos[1] < y, "then falls");
        assert_eq!(q.applied(), Some(5), "the filled steps are acknowledged");
        // The late commands 3 to 5 are ignored, 6 is applied.
        push(&mut q, 3, vec![forward(0.0); 4]);
        assert_eq!(b.advance(&mut q, &r, &flat).applied, 1);
        assert_eq!(q.applied(), Some(6));
        // Quiet again, filled again; then datagrams of old commands only (a client that kept
        // its own numbering): the server stops filling, and the client catches up.
        for _ in 0..=STARVE_TICKS {
            b.advance(&mut q, &r, &flat);
        }
        let end = q.applied().unwrap();
        assert!(end > 6);
        let mut next = 7;
        let mut filled = 0;
        for _ in 0..20 {
            push(&mut q, next, vec![Input::default(); 3]);
            next += 3;
            filled += b.advance(&mut q, &r, &flat).filled;
        }
        assert_eq!(filled, 0, "a client still sending is not stood in for");
        assert_eq!(q.applied(), Some(next - 1), "caught up");
    }

    #[test]
    fn unknown_cells_keep_the_commands() {
        let (p, s) = (PlayerParams::default(), shapes());
        let r = rules(&p, &s);
        let unknown = |_: i32, _: i32, _: i32| None;
        // Placed already (the simulation places a body once its columns are there).
        let mut b = Body::new([0.5, 11.0, 0.5]);
        b.settle = None;
        let mut q = InputQueue::new(64);
        push(&mut q, 0, vec![forward(0.0); 3]);
        let a = b.advance(&mut q, &r, &unknown);
        assert!(a.waiting && a.applied == 0);
        assert_eq!(q.len(), 3);
        assert_eq!(b.advance(&mut q, &r, &flat).applied, 3);
    }

    #[test]
    fn magma_kills_and_the_dead_drain_their_commands() {
        let (p, s) = (PlayerParams::default(), shapes());
        let r = rules(&p, &s);
        // East (yaw π/2) towards the magma at x ≥ 50.
        let mut b = Body::new([45.5, 11.0, 0.5]);
        let mut q = InputQueue::new(64);
        let mut died = None;
        for t in 0..60 {
            push(&mut q, t * 3, vec![forward(std::f64::consts::FRAC_PI_2); 3]);
            died = died.or(b.advance(&mut q, &r, &flat).died);
            if died.is_some() {
                break;
            }
        }
        assert_eq!(died, Some(DeathCause::Magma));
        assert!(b.state.pos[0] + p.width * 0.5 > 50.0 - TOUCH_M);
        b.dead = true;
        push(&mut q, 500, vec![forward(0.0); 9]);
        let x = b.state.pos;
        b.advance(&mut q, &r, &flat);
        assert!(q.is_empty() && q.applied() == Some(508));
        assert_eq!(b.state.pos, x);
        b.respawn([0.5, 11.0, 0.5]);
        assert!(!b.dead && b.settle == Some(Settle::Home { redraws: 0 }));
    }

    #[test]
    fn an_arrival_inside_blocks_is_put_beside_them() {
        let (p, s) = (PlayerParams::default(), shapes());
        let r = rules(&p, &s);
        // A trunk of stone from 11 to 20 at x = 0, z = 0 over the flat ground.
        let trunk = |x: i32, y: i32, z: i32| {
            if x == 0 && z == 0 && (11..=20).contains(&y) {
                Some(1)
            } else {
                flat(x, y, z)
            }
        };
        let mut b = Body::arriving([0.5, 11.0, 0.5], Settle::Last);
        let mut q = InputQueue::new(64);
        assert_eq!(b.advance(&mut q, &r, &trunk), Advance::default());
        assert_eq!(b.state.pos, [WORLD - 0.5, 11.0, 0.5], "beside the trunk");
        assert_eq!(b.settle, None);
        // The same body shut in by an edit suffocates.
        let mut b = Body::new([0.5, 11.0, 0.5]);
        assert_eq!(b.unstuck(&r, &trunk), Ok(Some(DeathCause::Suffocation)));
        // Nowhere to go: the caller decides (death, or another spawn point).
        let solid = |_: i32, _: i32, _: i32| Some(1);
        let mut b = Body::arriving([0.5, 11.0, 0.5], Settle::Last);
        let a = b.advance(&mut q, &r, &solid);
        assert_eq!(a.nowhere, Some(Settle::Last));
        assert_eq!(b.state.pos, [0.5, 11.0, 0.5]);
        assert!(!b.dead && b.settle.is_some());
    }

    #[test]
    fn eyes_and_reach() {
        let p = PlayerParams::default();
        let s = shapes();
        let r = rules(&p, &s);
        let mut b = Body::new([0.5, 11.0, 0.5]);
        let mut q = InputQueue::new(64);
        push(&mut q, 0, vec![forward(0.0); 3]);
        b.advance(&mut q, &r, &flat);
        let e1 = b.eye_at(Some(1), &p);
        assert_ne!(e1, b.eye_at(Some(2), &p));
        assert_eq!(b.eye_at(Some(2), &p), b.state.eye(&p));
        assert_eq!(b.eye_at(Some(99), &p), b.state.eye(&p));
        // Reach to the nearest point of a cell, across the seam.
        assert!((reach([99_999.9, 0.5, 0.5], [0, 0, 0]) - 0.1).abs() < 1e-9);
        assert_eq!(reach([99_999.9, 0.5, 0.5], [99_999, 0, 0]), 0.0);
        assert!((reach([0.5, 0.5, 0.5], [3, 0, 0]) - 2.5).abs() < 1e-12);
        assert!((reach([0.5, 0.5, 0.5], [-4, 0, 0]) - 3.5).abs() < 1e-12);
        // Overlap of a cell.
        let b = Body::new([0.5, 11.0, 0.5]);
        assert!(b.overlaps_cell(&p, [0, 12, 0], 1.0));
        let seam = Body::new([0.1, 11.0, 0.5]);
        assert!(
            seam.overlaps_cell(&p, [99_999, 11, 0], 1.0),
            "x from −0.2, across the seam"
        );
        assert!(!b.overlaps_cell(&p, [99_999, 11, 0], 1.0));
        assert!(
            !b.overlaps_cell(&p, [0, 10, 0], 1.0),
            "the ground under the feet"
        );
        assert!(
            !b.overlaps_cell(&p, [0, 13, 0], 1.0),
            "above the head (12.8)"
        );
        assert!(!b.overlaps_cell(&p, [1, 11, 0], 1.0), "x 0.8 < 1");
    }
}
