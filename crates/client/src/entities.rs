//! Entities seen by the client (S5): datagrams decoded against the ones acknowledged before,
//! a sample track per entity, and interpolation a little in the past of the server.
//!
//! Server ticks are mapped to local time with the fastest datagrams of the last two seconds
//! (smallest delay); entities are shown `interp_delay_s` behind that, so a lost datagram or
//! some jitter falls inside a known interval. When the samples run out, an entity moves on
//! along its last velocity for at most `max_extrapolation_s`, then stops.

use std::collections::{BTreeMap, VecDeque};

use nascio_core::torus;
use nascio_protocol::entity::{BASELINE_WINDOW, QState, STEPS_PER_M, decode, delta_q};

/// Samples kept per entity.
const TRACK: usize = 40;

/// Where an entity is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub id: u32,
    /// World metres; x and z wrapped.
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Default)]
struct Track {
    /// `(tick, state)` in tick order.
    samples: VecDeque<(u32, QState)>,
    /// Seq of the newest datagram that carried this entity.
    seq: u32,
}

/// Local time ↔ server tick.
struct TickClock {
    tick_s: f64,
    /// `(local time, tick time − local time)` of the recent datagrams.
    recent: VecDeque<(f64, f64)>,
    /// Offset in use (smoothed), seconds.
    offset: Option<f64>,
}

impl TickClock {
    fn observe(&mut self, tick: u32, now: f64) {
        let o = f64::from(tick) * self.tick_s - now;
        self.recent.push_back((now, o));
        while self.recent.front().is_some_and(|&(t, _)| now - t > 2.0) {
            self.recent.pop_front();
        }
        let target = self
            .recent
            .iter()
            .map(|&(_, o)| o)
            .fold(f64::NEG_INFINITY, f64::max);
        self.offset = Some(match self.offset {
            Some(cur) if (target - cur).abs() < 0.25 => cur + (target - cur) * 0.05,
            _ => target,
        });
    }

    /// Server tick (fractional) at local time `now`.
    fn tick_at(&self, now: f64) -> Option<f64> {
        Some((now + self.offset?) / self.tick_s)
    }
}

/// The entities around the player.
pub struct Entities {
    /// Decoded datagrams by seq (baselines of later ones).
    history: BTreeMap<u32, BTreeMap<u32, QState>>,
    tracks: BTreeMap<u32, Track>,
    /// Seq of the datagram that removed an entity (an older state must not bring it back).
    removed: BTreeMap<u32, u32>,
    clock: TickClock,
    pub interp_delay_s: f64,
    pub max_extrapolation_s: f64,
}

impl Entities {
    /// `tick_s`: duration of a server tick.
    pub fn new(tick_s: f64) -> Self {
        Self {
            history: BTreeMap::new(),
            tracks: BTreeMap::new(),
            removed: BTreeMap::new(),
            clock: TickClock {
                tick_s,
                recent: VecDeque::new(),
                offset: None,
            },
            interp_delay_s: 0.1,
            max_extrapolation_s: 0.25,
        }
    }

    /// Takes a datagram received at local time `now` (seconds); returns its seq, to
    /// acknowledge (`ClientMsg::Ack`).
    pub fn receive(&mut self, bytes: &[u8], now: f64) -> anyhow::Result<u32> {
        let snap = decode(bytes, |seq, id| self.history.get(&seq)?.get(&id).copied())?;
        let seq = snap.seq;
        for id in &snap.removed {
            if self.tracks.get(id).is_none_or(|t| t.seq < seq) {
                self.tracks.remove(id);
                self.removed.insert(*id, seq);
            }
        }
        for (&id, &s) in &snap.states {
            if self.removed.get(&id).is_some_and(|&r| r > seq) {
                continue;
            }
            self.removed.remove(&id);
            let t = self.tracks.entry(id).or_default();
            t.seq = t.seq.max(seq);
            let at = t.samples.partition_point(|&(k, _)| k < snap.tick);
            match t.samples.get_mut(at) {
                Some(x) if x.0 == snap.tick => x.1 = s,
                _ => t.samples.insert(at, (snap.tick, s)),
            }
            while t.samples.len() > TRACK {
                t.samples.pop_front();
            }
        }
        self.history.insert(seq, snap.states);
        // Keep twice the window: baselines are never older than the window.
        let newest = *self.history.keys().next_back().expect("just inserted");
        let keep = newest.saturating_sub(2 * BASELINE_WINDOW);
        self.history = self.history.split_off(&keep);
        self.removed.retain(|_, &mut r| r >= keep);
        self.clock.observe(snap.tick, now);
        Ok(seq)
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// The server tick shown at local time `now`.
    pub fn render_tick(&self, now: f64) -> Option<f64> {
        Some(self.clock.tick_at(now)? - self.interp_delay_s / self.clock.tick_s)
    }

    /// Where every entity is shown at local time `now`, by id.
    pub fn poses(&self, now: f64) -> Vec<Pose> {
        let Some(r) = self.render_tick(now) else {
            return Vec::new();
        };
        let max_ext = self.max_extrapolation_s / self.clock.tick_s;
        self.tracks
            .iter()
            .filter_map(|(&id, t)| pose(id, &t.samples, r, max_ext))
            .collect()
    }
}

/// `a + (b − a)·f` on the torus, in metres.
fn lerp(a: &QState, b: &QState, f: f64) -> ([f64; 3], f32, f32) {
    let m = STEPS_PER_M as f64;
    let p = a.position();
    let pos = [
        torus::wrap_f64(p[0] + delta_q(a.pos[0], b.pos[0]) as f64 / m * f),
        p[1] + (b.pos[1] - a.pos[1]) as f64 / m * f,
        torus::wrap_f64(p[2] + delta_q(a.pos[2], b.pos[2]) as f64 / m * f),
    ];
    let dyaw = f64::from(b.yaw.wrapping_sub(a.yaw) as i16) / 65_536.0 * std::f64::consts::TAU;
    let yaw = (f64::from(a.yaw()) + dyaw * f).rem_euclid(std::f64::consts::TAU) as f32;
    let pitch = a.pitch() + (b.pitch() - a.pitch()) * f as f32;
    (pos, yaw, pitch)
}

fn pose(id: u32, s: &VecDeque<(u32, QState)>, r: f64, max_ext: f64) -> Option<Pose> {
    let first = s.front()?;
    let (pos, yaw, pitch) = if r <= f64::from(first.0) {
        lerp(&first.1, &first.1, 0.0)
    } else {
        let i = s.partition_point(|&(k, _)| f64::from(k) <= r);
        if i < s.len() {
            // Between two samples.
            let (a, b) = (&s[i - 1], &s[i]);
            let f = (r - f64::from(a.0)) / f64::from(b.0 - a.0);
            lerp(&a.1, &b.1, f)
        } else if s.len() >= 2 {
            // Past the newest: along the last velocity, for a while.
            let (a, b) = (&s[s.len() - 2], &s[s.len() - 1]);
            let span = f64::from(b.0 - a.0);
            let ahead = (r - f64::from(b.0)).min(max_ext);
            lerp(&a.1, &b.1, 1.0 + ahead / span)
        } else {
            lerp(&first.1, &first.1, 0.0)
        }
    };
    Some(Pose {
        id,
        pos,
        yaw,
        pitch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nascio_protocol::entity::encode;

    #[test]
    fn interpolation_is_smooth_across_the_seam_with_a_lost_datagram() {
        let mut e = Entities::new(0.05);
        // An entity moving at 4 m/s in +x through the seam; datagram 4 is lost.
        let at = |tick: u32| [torus::wrap_f64(99_998.0 + 0.2 * f64::from(tick)), 60.0, 7.0];
        for tick in 1..=20u32 {
            if tick == 4 {
                continue;
            }
            let q = QState::quantize(at(tick), 0.0, 0.0);
            let d = encode(tick, tick, &[(9, q)].into(), |_| None, &[]);
            // Delivered 120 ms after the tick.
            e.receive(&d, f64::from(tick) * 0.05 + 0.12).unwrap();
        }
        // Shown at tick (now·20 − 2.4 − 2): around tick 4 at now = 0.42 s.
        let mut prev: Option<[f64; 3]> = None;
        for k in 0..30 {
            let now = 0.42 + f64::from(k) * 0.01;
            let p = e.poses(now)[0];
            let r = e.render_tick(now).unwrap();
            let truth = at(0);
            let want = torus::wrap_f64(truth[0] + 0.2 * r);
            assert!(
                torus::delta_f64(p.pos[0], want).abs() < 0.02,
                "{p:?} vs {want}"
            );
            if let Some(q) = prev {
                let step = torus::delta_f64(q[0], p.pos[0]);
                assert!((0.0..0.06).contains(&step), "step {step}");
            }
            prev = Some(p.pos);
        }
    }

    #[test]
    fn removals_and_stale_states() {
        let mut e = Entities::new(0.05);
        let q = QState::quantize([10.0, 60.0, 10.0], 0.0, 0.0);
        e.receive(&encode(1, 1, &[(3, q)].into(), |_| None, &[]), 0.0)
            .unwrap();
        e.receive(&encode(3, 3, &BTreeMap::new(), |_| None, &[3]), 0.1)
            .unwrap();
        assert!(e.is_empty());
        // An older datagram arriving late does not bring it back.
        e.receive(&encode(2, 2, &[(3, q)].into(), |_| None, &[]), 0.12)
            .unwrap();
        assert!(e.is_empty());
        // A newer one does.
        e.receive(&encode(4, 4, &[(3, q)].into(), |_| None, &[]), 0.15)
            .unwrap();
        assert_eq!(e.len(), 1);
    }
}
