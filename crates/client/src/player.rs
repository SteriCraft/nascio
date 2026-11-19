//! The player's own movement, predicted (chantier 5.4).
//!
//! The client runs the server's physics (`nascio-physics`, same parameters, same blocks: those
//! of the [`Mirror`]) at fixed steps of 1/60 s, one numbered command per step, so that the
//! player moves at once. The server simulates the same commands with the authority and sends
//! back its state with the number of the last command applied; on each state the client
//! starts again from it and replays the commands not acknowledged yet. When both see the same
//! blocks, the replay gives the same bits: nothing moves. When they differ (a block changed,
//! a step made by the server for a quiet client), the difference is smoothed out on screen
//! (τ = 100 ms), or the view jumps at once beyond 2 m.
//!
//! - **Clock**: fed by an `Instant` from the caller (frames in the viewer, simulated time in
//!   tests); 0 to 4 steps per update, 3 after a freeze (the rest is dropped). The pace follows
//!   the server's ticks: the depth of the server's queue is estimated as the minimum over 2 s
//!   of (commands not acknowledged − RTT × 60); the client steps up to 2 % faster or slower to
//!   keep it between 2 and 6 commands.
//! - **Unknown cells**: the prediction stops at the first command that needs a cell the
//!   mirror does not hold, and tries it again at each step. While it is stopped, or while
//!   more than 60 commands wait for their acknowledgement, no new command is made (the player
//!   is frozen); the waiting ones are still sent.
//! - **Sending**: every 3 steps, the 32 newest commands not acknowledged; [`Player::update`]
//!   returns them and the caller sends them.
//! - **Respawn**: states come by datagrams, `Respawned` by the control stream; until a state
//!   acknowledges a command made after `Respawned`, states may predate it and are ignored.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use nascio_core::coords::BlockPos;
use nascio_core::torus;
use nascio_physics::{BlockSource, Input, Mode, PlayerParams, Shapes, State, keys};
use nascio_protocol::entity::quantize_pitch;
use nascio_protocol::messages::{DeathCause, ServerHello, ServerMsg};
use nascio_protocol::player::{Commands, MAX_COMMANDS, PlayerState};

use crate::Mirror;

/// Steps between two datagrams of commands (20 Hz).
pub const STEPS_PER_SEND: u32 = 3;
/// Steps an update may take.
pub const MAX_STEPS: u32 = 4;
/// Steps taken after a freeze (more were due); the rest is dropped.
pub const STEPS_AFTER_FREEZE: u32 = 3;
/// Beyond this many commands not acknowledged, no new one is made.
pub const MAX_UNACKED: usize = 60;
/// Time constant of the smoothing of a correction, s.
pub const SMOOTH_TAU_S: f64 = 0.1;
/// A correction larger than this is not smoothed, m.
pub const SNAP_M: f64 = 2.0;
/// Two presses of forward within this time make the player run.
pub const RUN_TAP: Duration = Duration::from_millis(300);
/// Window of the estimate of the server's queue.
pub const DEPTH_WINDOW: Duration = Duration::from_secs(2);
/// Wanted depth of the server's queue, commands.
pub const DEPTH_MIN: f64 = 2.0;
pub const DEPTH_MAX: f64 = 6.0;
/// Largest change of the client's pace.
pub const PACE_SLEW: f64 = 0.02;

/// What the player asks, from the keyboard and the mouse.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Controls {
    pub forward: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    /// Jump; swim up; fly up.
    pub jump: bool,
    /// Crouch (Shift); swim down; fly down.
    pub crouch: bool,
    /// Heading from north (−z) towards east (+x), radians.
    pub yaw: f64,
    /// Pitch of the view, radians (replicated to the others).
    pub pitch: f32,
    /// Asked flight speed, 0.1 m/s.
    pub fly_speed: u16,
}

/// What happened to the player, for the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerEvent {
    Died(DeathCause),
    Respawned,
    /// Walking or flying, decided by the server.
    Mode(Mode),
    /// The view jumped to the predicted position (no smoothing).
    Snapped,
}

/// Counters of the prediction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayerStats {
    /// States that changed the present prediction.
    pub corrections: u64,
    /// Commands replayed on the server's states.
    pub replayed: u64,
    /// Acknowledgements beyond the newest command: numbering restarted after them.
    pub renumbered: u64,
    /// States ignored (older than one already received).
    pub stale: u64,
}

/// The blocks of the mirror, as the physics reads them.
struct Blocks<'a>(&'a Mirror);

impl BlockSource for Blocks<'_> {
    fn block(&self, x: i32, y: i32, z: i32) -> Option<u16> {
        self.0.block(BlockPos::new(x, y, z))
    }
}

/// The same state, to the bit.
fn same_bits(a: &State, b: &State) -> bool {
    let bits = |v: [f64; 3]| v.map(f64::to_bits);
    bits(a.pos) == bits(b.pos)
        && bits(a.vel) == bits(b.vel)
        && a.on_ground == b.on_ground
        && a.in_fluid == b.in_fluid
        && a.mode == b.mode
}

/// The player's movement on the client.
pub struct Player {
    params: PlayerParams,
    shapes: Shapes,
    /// The newest server state with the commands after it: the present.
    predicted: State,
    /// Commands not acknowledged, oldest first.
    pending: VecDeque<(u32, Input)>,
    /// How many of them `predicted` includes (fewer: stopped on an unknown cell).
    applied: usize,
    /// Number of the next command.
    next: u32,
    /// A state came: commands may be made.
    started: bool,
    last_tick: Option<u32>,
    dead: bool,
    /// After a respawn: states that do not acknowledge this command yet may predate the
    /// respawn (datagrams overtake the control stream) and are ignored.
    fence: Option<u32>,
    /// Shown position − predicted position, m.
    offset: [f64; 3],
    now: Instant,
    /// Time not stepped yet, s.
    acc: f64,
    /// Pace of the steps (1 ± [`PACE_SLEW`]).
    pace: f64,
    rtt: Duration,
    /// Estimates of the server's queue depth.
    depths: VecDeque<(Instant, f64)>,
    since_send: u32,
    running: bool,
    forward_held: bool,
    last_tap: Option<Instant>,
    auto_jump: bool,
    pitch: u8,
    events: Vec<PlayerEvent>,
    stats: PlayerStats,
}

impl Player {
    /// At the handshake: where the player arrives and how it moves. `shapes` from the
    /// server's block table.
    pub fn new(hello: &ServerHello, shapes: Shapes, now: Instant) -> Self {
        Self::with(hello.physics.clone(), shapes, hello.spawn, hello.mode, now)
    }

    /// From the physics, the arrival point and the mode.
    pub fn with(
        params: PlayerParams,
        shapes: Shapes,
        spawn: [f64; 3],
        mode: Mode,
        now: Instant,
    ) -> Self {
        Self {
            params,
            shapes,
            predicted: State::new(spawn, mode),
            pending: VecDeque::new(),
            applied: 0,
            next: 0,
            started: false,
            last_tick: None,
            dead: false,
            fence: None,
            offset: [0.0; 3],
            now,
            acc: 0.0,
            pace: 1.0,
            rtt: Duration::ZERO,
            depths: VecDeque::new(),
            since_send: 0,
            running: false,
            forward_held: false,
            last_tap: None,
            auto_jump: false,
            pitch: quantize_pitch(0.0),
            events: Vec::new(),
            stats: PlayerStats::default(),
        }
    }

    /// The round trip time to the server (from the connection).
    pub fn set_rtt(&mut self, rtt: Duration) {
        self.rtt = rtt;
    }

    pub fn set_auto_jump(&mut self, on: bool) {
        self.auto_jump = on;
    }

    /// The predicted state (the present).
    pub fn state(&self) -> &State {
        &self.predicted
    }

    /// Where to draw the player: the prediction plus what remains of the corrections.
    pub fn display_pos(&self) -> [f64; 3] {
        let p = self.predicted.pos;
        [
            torus::wrap_f64(p[0] + self.offset[0]),
            p[1] + self.offset[1],
            torus::wrap_f64(p[2] + self.offset[2]),
        ]
    }

    /// The camera: the eyes above [`Self::display_pos`].
    pub fn eye(&self) -> [f64; 3] {
        let p = self.display_pos();
        [p[0], p[1] + self.params.eye_height, p[2]]
    }

    /// Running (the view widens to 60°).
    pub fn running(&self) -> bool {
        self.running
    }

    pub fn dead(&self) -> bool {
        self.dead
    }

    /// Whether `block` at `cell` would overlap the predicted player (as the server checks a
    /// placement: such an edit is refused, so it is not predicted).
    pub fn overlaps_cell(&self, cell: BlockPos, block: u16) -> bool {
        const EPS: f64 = 1e-6;
        let top = match self.shapes.get(block) {
            nascio_physics::Shape::Full => 1.0,
            nascio_physics::Shape::Layer => f64::from(nascio_chunk::mesh::LAYER_H),
            nascio_physics::Shape::Empty | nascio_physics::Shape::Fluid => return false,
        };
        let s = &self.predicted;
        let half = self.params.width * 0.5 + 0.5 - EPS;
        let dx = torus::delta_f64(f64::from(cell.x) + 0.5, s.pos[0]);
        let dz = torus::delta_f64(f64::from(cell.z) + 0.5, s.pos[2]);
        let y = f64::from(cell.y);
        !self.dead
            && dx.abs() < half
            && dz.abs() < half
            && s.pos[1] < y + top - EPS
            && s.pos[1] + self.params.height > y + EPS
    }

    /// The prediction is stopped on a cell the mirror does not hold.
    pub fn blocked(&self) -> bool {
        self.applied < self.pending.len()
    }

    /// The newest command the shown position includes: `SetBlock.cmd` (the server measures
    /// the reach from the eyes after it).
    pub fn last_cmd(&self) -> Option<u32> {
        match self.applied {
            0 => self.next.checked_sub(1 + self.pending.len() as u32),
            n => Some(self.pending[n - 1].0),
        }
    }

    /// Events since the last call.
    pub fn events(&mut self) -> Vec<PlayerEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn stats(&self) -> PlayerStats {
        self.stats
    }

    /// Pace of the steps, 1 ± 2 % (tests, diagnostics).
    pub fn pace(&self) -> f64 {
        self.pace
    }

    /// Commands not acknowledged.
    pub fn unacked(&self) -> usize {
        self.pending.len()
    }

    /// Advances the clock to `now`: the steps due (each one a command), the smoothing; returns
    /// the commands to send, every [`STEPS_PER_SEND`] steps.
    pub fn update(&mut self, now: Instant, c: &Controls, mirror: &Mirror) -> Option<Commands> {
        let dt = now.saturating_duration_since(self.now).as_secs_f64();
        self.now = now;
        // Run: forward pressed twice within 300 ms; stops when forward is released.
        if c.forward && !self.forward_held {
            if self
                .last_tap
                .is_some_and(|t| now.duration_since(t) <= RUN_TAP)
            {
                self.running = true;
            }
            self.last_tap = Some(now);
        }
        if !c.forward {
            self.running = false;
        }
        self.forward_held = c.forward;
        self.pitch = quantize_pitch(c.pitch);
        // Smoothing.
        let k = (-dt / SMOOTH_TAU_S).exp();
        self.offset = self.offset.map(|o| o * k);
        if self.offset.iter().map(|o| o * o).sum::<f64>() < 1e-6 {
            self.offset = [0.0; 3];
        }
        // Steps due.
        let step_s = self.params.dt();
        self.acc += dt * self.pace;
        let mut n = (self.acc / step_s).floor() as u32;
        if n > MAX_STEPS {
            n = STEPS_AFTER_FREEZE;
            self.acc = 0.0;
        } else {
            self.acc -= f64::from(n) * step_s;
        }
        let src = Blocks(mirror);
        let mut send = false;
        for _ in 0..n {
            self.catch_up(&src);
            let frozen = !self.started || self.blocked() || self.pending.len() > MAX_UNACKED;
            if !frozen {
                self.make_command(c, &src);
            }
            self.since_send += 1;
            if self.since_send >= STEPS_PER_SEND {
                self.since_send = 0;
                send = true;
            }
        }
        if send { self.commands() } else { None }
    }

    /// A new command from the controls, applied to the prediction.
    fn make_command(&mut self, c: &Controls, src: &Blocks) {
        let mut k = 0;
        for (held, bit) in [
            (c.forward, keys::FORWARD),
            (c.back, keys::BACK),
            (c.left, keys::LEFT),
            (c.right, keys::RIGHT),
            (c.jump, keys::JUMP),
            (c.crouch, keys::CROUCH),
            (self.running && c.forward, keys::RUN),
        ] {
            if held {
                k |= bit;
            }
        }
        let mut input = Input {
            keys: k,
            yaw: Input::quantize_yaw(c.yaw),
            fly_speed: c.fly_speed,
        };
        if self.dead {
            // The server drops them: they only keep the stream of commands going.
            input.keys = 0;
        } else if self.auto_jump
            && nascio_physics::auto_jump(&self.predicted, &input, &self.params, &self.shapes, src)
        {
            input.keys |= keys::JUMP;
        }
        self.pending.push_back((self.next, input));
        self.next = self.next.wrapping_add(1);
        self.catch_up(src);
    }

    /// Applies the commands the prediction does not include yet, up to the first that needs
    /// an unknown cell. A dead player does not move.
    fn catch_up(&mut self, src: &Blocks) -> usize {
        let mut n = 0;
        while self.applied < self.pending.len() {
            if !self.dead {
                let input = self.pending[self.applied].1;
                let stepped = nascio_physics::step(
                    &mut self.predicted,
                    &input,
                    &self.params,
                    &self.shapes,
                    src,
                );
                if stepped.is_err() {
                    break;
                }
                n += 1;
            }
            self.applied += 1;
        }
        n
    }

    /// The newest commands not acknowledged (at most [`MAX_COMMANDS`]).
    fn commands(&self) -> Option<Commands> {
        let (&(last, _), len) = (self.pending.back()?, self.pending.len());
        let from = len.saturating_sub(MAX_COMMANDS);
        Some(Commands {
            last,
            pitch: self.pitch,
            inputs: self.pending.range(from..).map(|c| c.1).collect(),
        })
    }

    /// A state from the server (datagrams: late ones are ignored). The prediction starts
    /// again from it and replays every command not acknowledged.
    pub fn on_state(&mut self, s: &PlayerState, mirror: &Mirror) {
        if self.last_tick.is_some_and(|t| s.tick <= t) {
            self.stats.stale += 1;
            return;
        }
        // A state that may come from before the respawn: the server had respawned the player
        // before it applied any command made after `Respawned` reached us.
        if let Some(f) = self.fence {
            if s.ack.is_none_or(|a| a.wrapping_sub(f) > u32::MAX / 2) {
                self.stats.stale += 1;
                return;
            }
            self.fence = None;
        }
        self.last_tick = Some(s.tick);
        let first = !self.started;
        self.started = true;
        let old = self.predicted;
        let mut snap = first;
        if let Some(a) = s.ack {
            let newest = self.next.checked_sub(1);
            if newest.is_none_or(|n| a > n) {
                // The server stepped for us beyond our newest command, and goes on until our
                // next commands reach it: number them one round trip of steps (and one send)
                // ahead, so that they stay ahead of its own. The numbers in between are a gap
                // the server skips.
                let lead = if newest.is_some() {
                    self.stats.renumbered += 1;
                    snap = true;
                    (self.rtt.as_secs_f64() * f64::from(self.params.step_hz)).ceil() as u32
                        + STEPS_PER_SEND
                } else {
                    0
                };
                self.next = a.wrapping_add(1).wrapping_add(lead);
                self.pending.clear();
            } else {
                while self.pending.front().is_some_and(|c| c.0 <= a) {
                    self.pending.pop_front();
                }
            }
        }
        // The server's queue: commands not acknowledged minus those on the way.
        let depth =
            self.pending.len() as f64 - self.rtt.as_secs_f64() * f64::from(self.params.step_hz);
        self.depths.push_back((self.now, depth));
        while self
            .depths
            .front()
            .is_some_and(|d| self.now.duration_since(d.0) > DEPTH_WINDOW)
        {
            self.depths.pop_front();
        }
        let span = self.now.duration_since(self.depths[0].0);
        if span >= DEPTH_WINDOW / 2 {
            let min = self
                .depths
                .iter()
                .map(|d| d.1)
                .fold(f64::INFINITY, f64::min);
            self.pace = if min > DEPTH_MAX {
                1.0 - PACE_SLEW
            } else if min < DEPTH_MIN {
                1.0 + PACE_SLEW
            } else {
                1.0
            };
        }
        // Replay.
        self.predicted = s.state;
        self.applied = 0;
        let src = Blocks(mirror);
        self.stats.replayed += self.catch_up(&src) as u64;
        if first {
            self.offset = [0.0; 3];
            return;
        }
        if old.mode != self.predicted.mode {
            self.events.push(PlayerEvent::Mode(self.predicted.mode));
            snap = true;
        }
        if !same_bits(&old, &self.predicted) {
            self.stats.corrections += 1;
            let (a, b) = (old.pos, self.predicted.pos);
            self.offset[0] += torus::delta_f64(b[0], a[0]);
            self.offset[1] += a[1] - b[1];
            self.offset[2] += torus::delta_f64(b[2], a[2]);
            if self.offset.iter().map(|o| o * o).sum::<f64>() > SNAP_M * SNAP_M {
                snap = true;
            }
        }
        if snap {
            self.snap();
        }
    }

    /// Control messages about the player: death and respawn.
    pub fn on_msg(&mut self, m: &ServerMsg) {
        match m {
            ServerMsg::Died { cause } => {
                self.dead = true;
                self.predicted.vel = [0.0; 3];
                self.applied = self.pending.len();
                self.events.push(PlayerEvent::Died(*cause));
                self.snap();
            }
            ServerMsg::Respawned { pos } => {
                self.dead = false;
                self.fence = Some(self.next);
                self.predicted = State::new(*pos, self.predicted.mode);
                self.applied = self.pending.len();
                self.events.push(PlayerEvent::Respawned);
                self.snap();
            }
            _ => {}
        }
    }

    fn snap(&mut self) {
        self.offset = [0.0; 3];
        self.events.push(PlayerEvent::Snapped);
    }
}

#[cfg(test)]
mod tests;
