//! The player's own movement, in datagrams (chantier 5).
//!
//! The client sends numbered commands ([`Commands`], 20 Hz): a command is the [`Input`] of one
//! physics step (1/60 s), and every datagram repeats the commands the server has not
//! acknowledged yet (at most [`MAX_COMMANDS`]), so a lost datagram loses no command. The
//! server applies them in order ([`InputQueue`]: no duplicate, no reordering) and answers each
//! tick with the authoritative [`PlayerState`], which names the last command applied: the
//! client replays its later commands on it.
//!
//! The state goes as exact `f64` bits, not quantized: client and server run the same
//! deterministic physics, so the prediction only differs where the server really decided
//! otherwise.
//!
//! Command numbers start at 0 on each connection and do not wrap (two years of steps).

use std::collections::VecDeque;

use anyhow::{Context, bail, ensure};
use nascio_core::consts::WORLD_SIZE;
use nascio_physics::{Input, Mode, State};

use crate::varint::{Reader, put};
use crate::{ClientDatagram, ServerDatagram};

/// Commands in one datagram at most: the newest, those not acknowledged yet (0.53 s of steps).
pub const MAX_COMMANDS: usize = 32;
/// Largest [`Commands`] datagram, bytes.
pub const MAX_COMMANDS_BYTES: usize =
    1 + 5 + 1 + 1 + (1 + 2 + 3) + (MAX_COMMANDS - 1) * (1 + 3 + 3);
/// Largest [`PlayerState`] datagram, bytes.
pub const MAX_STATE_BYTES: usize = 1 + 5 + 5 + 1 + 6 * 8;

/// Keys use 7 bits; in a command after the first, this bit says a change follows.
const CHANGED: u8 = 0x80;

/// Consecutive commands of the client, the newest last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commands {
    /// Number of the newest command.
    pub last: u32,
    /// Pitch of the view, as [`crate::entity::QState::pitch`]: replicated to the others, not
    /// simulated.
    pub pitch: u8,
    /// Oldest first: `inputs[i]` is command `last + 1 − len + i`. From 1 to
    /// [`MAX_COMMANDS`].
    pub inputs: Vec<Input>,
}

impl Commands {
    /// The commands with their numbers, oldest first.
    pub fn numbered(&self) -> impl Iterator<Item = (u32, Input)> + '_ {
        let first = self.last + 1 - self.inputs.len() as u32;
        self.inputs
            .iter()
            .enumerate()
            .map(move |(i, c)| (first + i as u32, *c))
    }

    /// The datagram. Panics on an empty or too long list, or numbers below 0; the eighth bit
    /// of the keys is not sent.
    ///
    /// Format: kind, `last` (varint), count, pitch; the first command whole (keys, yaw u16
    /// LE, fly speed varint); each next one a byte of keys, with [`CHANGED`] when a varint
    /// `zigzag(Δyaw) << 1 | fly speed changed` follows, then the new fly speed if it changed.
    pub fn encode(&self) -> Vec<u8> {
        let n = self.inputs.len();
        assert!((1..=MAX_COMMANDS).contains(&n), "{n} commands");
        assert!(self.last as usize + 1 >= n, "command numbers below 0");
        let mut out = Vec::with_capacity(8 + 3 * n);
        out.push(ClientDatagram::Input as u8);
        put(&mut out, u64::from(self.last));
        out.push(n as u8);
        out.push(self.pitch);
        let first = self.inputs[0];
        out.push(first.keys & !CHANGED);
        out.extend_from_slice(&first.yaw.to_le_bytes());
        put(&mut out, u64::from(first.fly_speed));
        for w in self.inputs.windows(2) {
            let (a, b) = (w[0], w[1]);
            let dyaw = i64::from(b.yaw.wrapping_sub(a.yaw) as i16);
            let fly = b.fly_speed != a.fly_speed;
            if dyaw == 0 && !fly {
                out.push(b.keys & !CHANGED);
                continue;
            }
            out.push(b.keys | CHANGED);
            let zz = ((dyaw << 1) ^ (dyaw >> 63)) as u64;
            put(&mut out, zz << 1 | u64::from(fly));
            if fly {
                put(&mut out, u64::from(b.fly_speed));
            }
        }
        out
    }

    /// Reads a datagram of [`Commands::encode`]; refuses anything else.
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        let mut r = Reader::new(bytes);
        let kind = r.u8()?;
        ensure!(
            kind == ClientDatagram::Input as u8,
            "not a command datagram ({kind})"
        );
        let last = r.var_max(u64::from(u32::MAX))? as u32;
        let n = usize::from(r.u8()?);
        ensure!((1..=MAX_COMMANDS).contains(&n), "{n} commands");
        ensure!(last as usize + 1 >= n, "command numbers below 0");
        let pitch = r.u8()?;
        let keys = r.u8()?;
        ensure!(keys & CHANGED == 0, "change bit in the first command");
        let yaw = u16::from_le_bytes(r.take(2)?.try_into()?);
        let fly_speed = r.var_max(u64::from(u16::MAX))? as u16;
        let mut inputs = Vec::with_capacity(n);
        inputs.push(Input {
            keys,
            yaw,
            fly_speed,
        });
        for _ in 1..n {
            let prev = *inputs.last().context("no command")?;
            let b = r.u8()?;
            let mut c = Input {
                keys: b & !CHANGED,
                ..prev
            };
            if b & CHANGED != 0 {
                // zigzag of an i16 is at most 65 535.
                let v = r.var_max(u64::from(u16::MAX) << 1 | 1)?;
                let zz = v >> 1;
                let dyaw = (zz >> 1) as i64 ^ -((zz & 1) as i64);
                c.yaw = prev.yaw.wrapping_add(dyaw as u16);
                if v & 1 != 0 {
                    c.fly_speed = r.var_max(u64::from(u16::MAX))? as u16;
                    ensure!(c.fly_speed != prev.fly_speed, "fly speed did not change");
                } else {
                    ensure!(dyaw != 0, "empty change");
                }
            }
            inputs.push(c);
        }
        ensure!(r.is_empty(), "trailing bytes in a command datagram");
        Ok(Self {
            last,
            pitch,
            inputs,
        })
    }
}

/// The authoritative state of the player, from the server.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerState {
    /// Server tick after which this was the state: newer states have higher ticks.
    pub tick: u32,
    /// Last command applied (`None`: none yet). The client replays the later ones on
    /// `state`.
    pub ack: Option<u32>,
    pub state: State,
}

const ON_GROUND: u8 = 1;
const IN_FLUID: u8 = 1 << 1;
const FLY: u8 = 1 << 2;
/// Bits 3 to 5: which velocity components follow (the others are +0.0).
const VEL_SHIFT: u8 = 3;
const FLAGS: u8 = (1 << 6) - 1;

impl PlayerState {
    /// The datagram: kind, tick (varint), `ack + 1` (varint, 0: none), flags, the position as
    /// three `f64` LE, then the velocity components whose bits are not all zero (−0.0 is
    /// sent). Refuses a state that is not finite.
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        let s = &self.state;
        ensure!(
            s.pos.iter().chain(&s.vel).all(|v| v.is_finite()),
            "player state not finite: {s:?}"
        );
        let mut out = Vec::with_capacity(MAX_STATE_BYTES);
        out.push(ServerDatagram::PlayerState as u8);
        put(&mut out, u64::from(self.tick));
        put(&mut out, self.ack.map_or(0, |a| u64::from(a) + 1));
        let mut flags = 0;
        if s.on_ground {
            flags |= ON_GROUND;
        }
        if s.in_fluid {
            flags |= IN_FLUID;
        }
        if s.mode == Mode::Fly {
            flags |= FLY;
        }
        for (i, v) in s.vel.iter().enumerate() {
            if v.to_bits() != 0 {
                flags |= 1 << (VEL_SHIFT + i as u8);
            }
        }
        out.push(flags);
        for v in s.pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in s.vel {
            if v.to_bits() != 0 {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        Ok(out)
    }

    /// Reads a datagram of [`PlayerState::encode`]; refuses anything else, a position off
    /// the torus or a value that is not finite.
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        let mut r = Reader::new(bytes);
        let kind = r.u8()?;
        ensure!(
            kind == ServerDatagram::PlayerState as u8,
            "not a player state ({kind})"
        );
        let tick = r.var_max(u64::from(u32::MAX))? as u32;
        let ack = match r.var_max(u64::from(u32::MAX) + 1)? {
            0 => None,
            a => Some((a - 1) as u32),
        };
        let flags = r.u8()?;
        ensure!(flags & !FLAGS == 0, "unknown player state flags {flags:#x}");
        let mut f64_le = || -> anyhow::Result<f64> {
            let v = f64::from_le_bytes(r.take(8)?.try_into()?);
            ensure!(v.is_finite(), "player state not finite");
            Ok(v)
        };
        let pos = [f64_le()?, f64_le()?, f64_le()?];
        let mut vel = [0.0; 3];
        for (i, v) in vel.iter_mut().enumerate() {
            if flags & 1 << (VEL_SHIFT + i as u8) != 0 {
                *v = f64_le()?;
            }
        }
        let size = f64::from(WORLD_SIZE);
        for x in [pos[0], pos[2]] {
            ensure!((0.0..size).contains(&x), "position {x} off the torus");
        }
        ensure!(r.is_empty(), "trailing bytes in a player state");
        Ok(Self {
            tick,
            ack,
            state: State {
                pos,
                vel,
                on_ground: flags & ON_GROUND != 0,
                in_fluid: flags & IN_FLUID != 0,
                mode: if flags & FLY != 0 {
                    Mode::Fly
                } else {
                    Mode::Walk
                },
            },
        })
    }
}

/// Reads the kind of a datagram from the server.
pub fn server_datagram_kind(bytes: &[u8]) -> anyhow::Result<ServerDatagram> {
    let k = *bytes.first().context("empty datagram")?;
    match ServerDatagram::from_u8(k) {
        Some(k) => Ok(k),
        None => bail!("unknown datagram kind {k}"),
    }
}

/// Commands received by the server and not applied yet: in order, each once.
///
/// A command already received is ignored (repeats, duplicated or late datagrams). Commands
/// the client stopped repeating before they arrived (more than [`MAX_COMMANDS`] lost in a
/// row) are skipped and counted. When the queue is full, newer commands are refused without
/// being marked received: the client repeats them until they are acknowledged.
#[derive(Clone, Debug)]
pub struct InputQueue {
    queue: VecDeque<(u32, Input)>,
    /// Number after the newest command received.
    end: u64,
    applied: Option<u32>,
    cap: usize,
}

/// What [`InputQueue::push`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Received {
    /// New commands queued.
    pub new: usize,
    /// Commands that will never come (lost in every datagram that carried them).
    pub skipped: u64,
    /// New commands refused because the queue was full.
    pub refused: usize,
}

impl InputQueue {
    /// A queue holding at most `cap` commands.
    pub fn new(cap: usize) -> Self {
        Self {
            queue: VecDeque::with_capacity(cap),
            end: 0,
            applied: None,
            cap,
        }
    }

    pub fn push(&mut self, c: &Commands) -> Received {
        let mut r = Received::default();
        for (n, input) in c.numbered() {
            let n64 = u64::from(n);
            if n64 < self.end {
                continue;
            }
            if self.queue.len() >= self.cap {
                r.refused += 1;
                continue;
            }
            r.skipped += n64 - self.end;
            r.new += 1;
            self.queue.push_back((n, input));
            self.end = n64 + 1;
        }
        r
    }

    /// The next command to apply, which becomes the last applied.
    pub fn pop(&mut self) -> Option<(u32, Input)> {
        let c = self.queue.pop_front()?;
        self.applied = Some(c.0);
        Some(c)
    }

    /// Number of the last command popped (the acknowledgement sent to the client).
    pub fn applied(&self) -> Option<u32> {
        self.applied
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nascio_physics::keys;
    use proptest::prelude::*;

    fn input(keys: u8, yaw: u16, fly_speed: u16) -> Input {
        Input {
            keys,
            yaw,
            fly_speed,
        }
    }

    #[test]
    fn commands_round_trip() {
        let cases = [
            Commands {
                last: 0,
                pitch: 127,
                inputs: vec![input(0, 0, 0)],
            },
            Commands {
                last: 1_000_000,
                pitch: 3,
                inputs: vec![
                    input(keys::FORWARD | keys::RUN, 65_535, 40),
                    // Across 0, by the short way.
                    input(keys::FORWARD | keys::RUN, 2, 40),
                    input(keys::JUMP, 2, 1_200),
                    input(0x7f, 32_770, 1_200),
                    input(0, 2, 0),
                    input(0, 2, 0),
                ],
            },
            Commands {
                last: u32::MAX,
                pitch: 255,
                inputs: (0..MAX_COMMANDS as u16)
                    .map(|i| input((i % 128) as u8, i.wrapping_mul(9_001), i / 4))
                    .collect(),
            },
        ];
        for c in cases {
            let b = c.encode();
            assert!(b.len() <= MAX_COMMANDS_BYTES, "{} bytes", b.len());
            assert_eq!(Commands::decode(&b).unwrap(), c);
            // Every truncation is refused.
            for k in 0..b.len() {
                assert!(Commands::decode(&b[..k]).is_err(), "{k} bytes accepted");
            }
            let mut more = b.clone();
            more.push(0);
            assert!(Commands::decode(&more).is_err());
        }
        let n: Vec<u32> = Commands {
            last: 9,
            pitch: 0,
            inputs: vec![Input::default(); 3],
        }
        .numbered()
        .map(|(n, _)| n)
        .collect();
        assert_eq!(n, [7, 8, 9]);
    }

    #[test]
    fn bad_commands_are_refused() {
        let c = Commands {
            last: 5,
            pitch: 0,
            inputs: vec![input(1, 7, 0), input(1, 9, 0)],
        };
        let good = c.encode();
        let refused = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut b = good.clone();
            f(&mut b);
            Commands::decode(&b).is_err()
        };
        // Kind, count 0 and above the maximum, numbers below 0, change bit first.
        assert!(refused(&|b| b[0] = 1));
        assert!(refused(&|b| b[2] = 0));
        assert!(refused(&|b| b[2] = MAX_COMMANDS as u8 + 1));
        assert!(refused(&|b| b[1] = 0));
        assert!(refused(&|b| b[4] |= CHANGED));
        // An empty change.
        assert!(refused(&|b| {
            b.truncate(b.len() - 2);
            b.extend([1 | CHANGED, 0]);
        }));
        assert!(Commands::decode(&[]).is_err());
    }

    fn state() -> State {
        State {
            pos: [99_999.999_999, -12.5, 0.0],
            vel: [4.3, 0.0, -0.0],
            on_ground: true,
            in_fluid: false,
            mode: Mode::Walk,
        }
    }

    fn same_bits(a: &State, b: &State) -> bool {
        a.pos
            .iter()
            .chain(&a.vel)
            .zip(b.pos.iter().chain(&b.vel))
            .all(|(x, y)| x.to_bits() == y.to_bits())
            && (a.on_ground, a.in_fluid, a.mode) == (b.on_ground, b.in_fluid, b.mode)
    }

    #[test]
    fn player_state_round_trips_bit_exact() {
        for (ack, s) in [
            (None, State::default()),
            (Some(0), state()),
            (
                Some(u32::MAX),
                State {
                    vel: [-1e-300, 59.999, f64::MIN_POSITIVE],
                    on_ground: false,
                    in_fluid: true,
                    mode: Mode::Fly,
                    ..state()
                },
            ),
        ] {
            let p = PlayerState {
                tick: 77,
                ack,
                state: s,
            };
            let b = p.encode().unwrap();
            assert!(b.len() <= MAX_STATE_BYTES);
            let back = PlayerState::decode(&b).unwrap();
            assert_eq!((back.tick, back.ack), (p.tick, p.ack));
            assert!(same_bits(&back.state, &s), "{back:?}");
            for k in 0..b.len() {
                assert!(PlayerState::decode(&b[..k]).is_err());
            }
            assert_eq!(
                server_datagram_kind(&b).unwrap(),
                ServerDatagram::PlayerState
            );
        }
        // −0.0 is sent as such.
        let b = PlayerState {
            tick: 0,
            ack: None,
            state: state(),
        }
        .encode()
        .unwrap();
        assert_eq!(b.len(), 1 + 1 + 1 + 1 + 24 + 16);
    }

    #[test]
    fn bad_player_states_are_refused() {
        let mut s = state();
        s.vel[1] = f64::NAN;
        let p = |state| PlayerState {
            tick: 1,
            ack: Some(3),
            state,
        };
        assert!(p(s).encode().is_err());
        let good = p(state()).encode().unwrap();
        // Off the torus (x = 100 000), an unknown flag, the kind of an entity datagram,
        // trailing bytes.
        let mut b = good.clone();
        b[4..12].copy_from_slice(&100_000f64.to_le_bytes());
        assert!(PlayerState::decode(&b).is_err());
        let mut b = good.clone();
        b[3] |= 1 << 6;
        assert!(PlayerState::decode(&b).is_err());
        let mut b = good.clone();
        b[0] = ServerDatagram::Entities as u8;
        assert!(PlayerState::decode(&b).is_err());
        let mut b = good;
        b.push(0);
        assert!(PlayerState::decode(&b).is_err());
        assert!(server_datagram_kind(&[9]).is_err());
        assert!(server_datagram_kind(&[]).is_err());
    }

    /// Typical sizes (payload; QUIC, UDP and IP add about 55 bytes per datagram).
    #[test]
    fn sizes() {
        // 9 commands (3 new, 6 not acknowledged after 100 ms), an hour into the session.
        let at = 216_000;
        let idle = Commands {
            last: at,
            pitch: 120,
            inputs: vec![input(0, 1_000, 0); 9],
        };
        // Turning at 90°/s: 273 units of yaw per step.
        let turning = Commands {
            inputs: (0..9)
                .map(|i| input(keys::FORWARD, 1_000 + 273 * i, 0))
                .collect(),
            ..idle.clone()
        };
        let worst = Commands {
            inputs: (0..MAX_COMMANDS as u16)
                .map(|i| input(0, i.wrapping_mul(20_000), 100 + i % 2))
                .collect(),
            ..idle.clone()
        };
        let still = State {
            vel: [0.0; 3],
            ..state()
        };
        let walking = State {
            vel: [3.1, 0.0, -2.9],
            ..state()
        };
        let jumping = State {
            vel: [3.1, 4.9, -2.9],
            on_ground: false,
            ..state()
        };
        let st = |s| {
            PlayerState {
                tick: 72_000,
                ack: Some(at),
                state: s,
            }
            .encode()
            .unwrap()
            .len()
        };
        let sizes = [
            idle.encode().len(),
            turning.encode().len(),
            worst.encode().len(),
            st(still),
            st(walking),
            st(jumping),
        ];
        eprintln!(
            "commands idle/turning/32: {:?}, state still/walking/jumping: {:?}",
            &sizes[..3],
            &sizes[3..]
        );
        assert_eq!(sizes, [18, 34, 165, 32, 48, 56]);
        assert!(sizes[2] <= MAX_COMMANDS_BYTES);
    }

    /// The client of the tests: numbers its commands and repeats those not acknowledged.
    struct Client {
        next: u32,
        unacked: VecDeque<(u32, Input)>,
    }

    impl Client {
        fn new() -> Self {
            Self {
                next: 0,
                unacked: VecDeque::new(),
            }
        }

        /// Three new steps, then the datagram of the newest commands not acknowledged.
        fn send(&mut self) -> Vec<u8> {
            for _ in 0..3 {
                let n = self.next;
                self.unacked
                    .push_back((n, input((n % 128) as u8, n as u16, 0)));
                self.next += 1;
            }
            let k = self.unacked.len().min(MAX_COMMANDS);
            let inputs: Vec<Input> = self
                .unacked
                .iter()
                .skip(self.unacked.len() - k)
                .map(|c| c.1)
                .collect();
            Commands {
                last: self.next - 1,
                pitch: 0,
                inputs,
            }
            .encode()
        }

        fn ack(&mut self, ack: Option<u32>) {
            if let Some(a) = ack {
                while self.unacked.front().is_some_and(|c| c.0 <= a) {
                    self.unacked.pop_front();
                }
            }
        }
    }

    /// Runs `ticks` ticks; `lost(t)` loses the datagram of tick `t` (commands, and the state
    /// that would acknowledge it). Returns the commands applied and those skipped.
    fn run(ticks: u32, lost: impl Fn(u32) -> bool, dup: impl Fn(u32) -> bool) -> (Vec<u32>, u64) {
        let (mut client, mut server) = (Client::new(), InputQueue::new(4 * MAX_COMMANDS));
        let (mut applied, mut skipped) = (Vec::new(), 0);
        let mut late: Option<Vec<u8>> = None;
        for t in 0..ticks {
            let d = client.send();
            let mut arrived = Vec::new();
            if !lost(t) {
                arrived.push(d.clone());
            }
            // A copy of an earlier datagram arrives after this one.
            arrived.extend(late.take());
            if !lost(t) && dup(t) {
                arrived.push(d.clone());
                late = Some(d);
            }
            for d in arrived {
                let r = server.push(&Commands::decode(&d).unwrap());
                skipped += r.skipped;
                assert_eq!(r.refused, 0);
            }
            // Three steps per tick, and one more to catch up after losses.
            for _ in 0..4 {
                if let Some((n, c)) = server.pop() {
                    assert_eq!(c, input((n % 128) as u8, n as u16, 0));
                    applied.push(n);
                }
            }
            if !lost(t) {
                client.ack(server.applied());
            }
        }
        // What is left of a backlog, at once.
        while let Some((n, _)) = server.pop() {
            applied.push(n);
        }
        (applied, skipped)
    }

    #[test]
    fn losses_up_to_nine_datagrams_lose_no_command() {
        for k in 1..=9 {
            let (applied, skipped) = run(100, |t| (40..40 + k).contains(&t), |_| false);
            assert_eq!(skipped, 0, "{k} lost");
            assert_eq!(applied, (0..applied.len() as u32).collect::<Vec<_>>());
            assert_eq!(applied.len(), 300, "{k} lost");
        }
        // Eleven in a row: 33 new commands, more than one datagram repeats; the gap is
        // counted and the following ones are applied.
        let (applied, skipped) = run(100, |t| (40..51).contains(&t), |_| false);
        assert!(skipped > 0);
        assert!(applied.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(applied.len() as u64 + skipped, 300);
    }

    #[test]
    fn duplicates_and_late_datagrams_apply_once() {
        // The last datagram (tick 100) arrives.
        let (applied, skipped) = run(101, |t| t % 5 == 4, |t| t % 5 == 3);
        assert_eq!(skipped, 0);
        assert_eq!(applied, (0..303).collect::<Vec<_>>());
    }

    #[test]
    fn a_full_queue_refuses_without_losing() {
        let mut q = InputQueue::new(4);
        let c = |last, n| Commands {
            last,
            pitch: 0,
            inputs: vec![Input::default(); n],
        };
        let r = q.push(&c(5, 6));
        assert_eq!((r.new, r.refused), (4, 2));
        assert_eq!(q.pop().map(|c| c.0), Some(0));
        assert_eq!(q.applied(), Some(0));
        // Commands 4 and 5, refused, come again: one fits.
        let r = q.push(&c(5, 6));
        assert_eq!((r.new, r.refused, r.skipped), (1, 1, 0));
        let got: Vec<u32> = std::iter::from_fn(|| q.pop().map(|c| c.0)).collect();
        assert_eq!(got, [1, 2, 3, 4]);
        // A number far ahead skips the missing ones (a hostile client gains nothing: the
        // server applies at most a few steps per tick).
        let r = q.push(&c(1_000, 1));
        assert_eq!((r.new, r.skipped), (1, 995));
    }

    proptest! {
        #[test]
        fn commands_round_trip_any(
            last in 31u32..,
            pitch: u8,
            inputs in prop::collection::vec((0u8..128, any::<u16>(), any::<u16>()), 1..=MAX_COMMANDS),
        ) {
            let c = Commands {
                last,
                pitch,
                inputs: inputs.into_iter().map(|(k, y, f)| input(k, y, f)).collect(),
            };
            let b = c.encode();
            prop_assert!(b.len() <= MAX_COMMANDS_BYTES);
            prop_assert_eq!(Commands::decode(&b).unwrap(), c);
        }

        #[test]
        fn any_losses_keep_order(lost in prop::collection::vec(any::<bool>(), 200), dup in prop::collection::vec(any::<bool>(), 200)) {
            // The last datagrams arrive: every command is then known.
            let (applied, skipped) =
                run(200, |t| t < 190 && lost[t as usize], |t| dup[t as usize]);
            prop_assert!(applied.windows(2).all(|w| w[0] < w[1]));
            // Every command was applied or counted as skipped.
            prop_assert_eq!(applied.len() as u64 + skipped, 600);
            if skipped == 0 {
                prop_assert_eq!(applied, (0..600).collect::<Vec<_>>());
            }
        }
    }
}
