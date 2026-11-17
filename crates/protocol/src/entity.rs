//! Entity states in unreliable datagrams: quantized, and sent as differences from a state
//! the client confirmed (its baseline) when there is one.
//!
//! Quantization: positions in 1/64 m (x and z wrapped on the torus, `[0, 6 400 000)`), yaw
//! in 1/65 536 turn, pitch in 1/255 of a half turn. A full state takes about 11 bytes, a
//! small move 5 to 7 bytes. Differences of x and z go through `nascio_core::torus::delta`
//! on the block part: an entity crossing the seam moves by a few units, not by the world.

use std::collections::BTreeMap;

use anyhow::{Context, bail, ensure};
use nascio_core::consts::WORLD_SIZE;
use nascio_core::torus;

use crate::varint::{Reader, put, put_i};

/// Quantization steps per metre.
pub const STEPS_PER_M: i64 = 64;
/// Period of x and z in steps.
pub const PERIOD: i64 = WORLD_SIZE as i64 * STEPS_PER_M;

/// A quantized entity state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QState {
    /// x and z in `[0, PERIOD)`, y signed.
    pub pos: [i64; 3],
    pub yaw: u16,
    pub pitch: u8,
}

impl QState {
    /// From world metres (x, z wrapped) and angles in radians (pitch in `[-π/2, π/2]`).
    pub fn quantize(pos: [f64; 3], yaw: f32, pitch: f32) -> Self {
        let q = |v: f64| (v * STEPS_PER_M as f64).round() as i64;
        let turn = std::f64::consts::TAU;
        let yaw = (f64::from(yaw).rem_euclid(turn) / turn * 65_536.0).round() as i64;
        let half = std::f64::consts::FRAC_PI_2;
        let pitch = ((f64::from(pitch).clamp(-half, half) + half) / std::f64::consts::PI * 255.0)
            .round() as u8;
        Self {
            pos: [
                q(pos[0]).rem_euclid(PERIOD),
                q(pos[1]),
                q(pos[2]).rem_euclid(PERIOD),
            ],
            yaw: yaw as u16,
            pitch,
        }
    }

    /// World metres and angles (radians).
    pub fn position(&self) -> [f64; 3] {
        self.pos.map(|v| v as f64 / STEPS_PER_M as f64)
    }

    pub fn yaw(&self) -> f32 {
        (f64::from(self.yaw) / 65_536.0 * std::f64::consts::TAU) as f32
    }

    pub fn pitch(&self) -> f32 {
        (f64::from(self.pitch) / 255.0 * std::f64::consts::PI - std::f64::consts::FRAC_PI_2) as f32
    }
}

/// Shortest signed difference of two wrapped quantized coordinates (`b − a` on the torus):
/// the block parts go through `torus::delta`.
pub fn delta_q(a: i64, b: i64) -> i64 {
    let (ab, af) = (a.div_euclid(STEPS_PER_M), a.rem_euclid(STEPS_PER_M));
    let (bb, bf) = (b.div_euclid(STEPS_PER_M), b.rem_euclid(STEPS_PER_M));
    i64::from(torus::delta(ab as i32, bb as i32)) * STEPS_PER_M + (bf - af)
}

/// `a + d`, wrapped on the torus (inverse of [`delta_q`]).
pub fn offset_q(a: i64, d: i64) -> i64 {
    (a + d).rem_euclid(PERIOD)
}

/// A snapshot as decoded: every entity's state after the tick, and the removed entities.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub tick: u32,
    /// Tick of the baseline the differences refer to (0: none).
    pub baseline: u32,
    pub states: BTreeMap<u32, QState>,
    pub removed: Vec<u32>,
}

const FULL: u64 = 0;
const DELTA: u64 = 1;
const REMOVED: u64 = 2;

/// Encodes the states of `tick`; entities found in `baseline` are sent as differences.
pub fn encode(
    tick: u32,
    baseline: Option<(u32, &BTreeMap<u32, QState>)>,
    states: &BTreeMap<u32, QState>,
    removed: &[u32],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + 8 * states.len());
    put(&mut out, u64::from(tick));
    put(&mut out, u64::from(baseline.map_or(0, |b| b.0)));
    put(&mut out, (states.len() + removed.len()) as u64);
    for (&id, s) in states {
        match baseline.and_then(|b| b.1.get(&id)) {
            Some(b) => {
                put(&mut out, u64::from(id) << 2 | DELTA);
                put_i(&mut out, delta_q(b.pos[0], s.pos[0]));
                put_i(&mut out, s.pos[1] - b.pos[1]);
                put_i(&mut out, delta_q(b.pos[2], s.pos[2]));
                put_i(&mut out, i64::from(s.yaw.wrapping_sub(b.yaw) as i16));
                put_i(&mut out, i64::from(s.pitch) - i64::from(b.pitch));
            }
            None => {
                put(&mut out, u64::from(id) << 2 | FULL);
                put(&mut out, s.pos[0] as u64);
                put_i(&mut out, s.pos[1]);
                put(&mut out, s.pos[2] as u64);
                out.extend_from_slice(&s.yaw.to_le_bytes());
                out.push(s.pitch);
            }
        }
    }
    for &id in removed {
        put(&mut out, u64::from(id) << 2 | REMOVED);
    }
    out
}

/// Decodes a snapshot; `baseline` finds the states of a past tick (the client keeps the
/// snapshots it acknowledged).
pub fn decode<'a>(
    bytes: &[u8],
    baseline: impl Fn(u32) -> Option<&'a BTreeMap<u32, QState>>,
) -> anyhow::Result<Snapshot> {
    let mut r = Reader::new(bytes);
    let tick = r.var_max(u64::from(u32::MAX))? as u32;
    let base_tick = r.var_max(u64::from(u32::MAX))? as u32;
    let base = if base_tick == 0 {
        None
    } else {
        Some(baseline(base_tick).with_context(|| format!("baseline {base_tick} unknown"))?)
    };
    let n = r.var_max(1 << 20)?;
    let mut snap = Snapshot {
        tick,
        baseline: base_tick,
        ..Snapshot::default()
    };
    for _ in 0..n {
        let tag = r.var_max(u64::from(u32::MAX) << 2 | 3)?;
        let id = (tag >> 2) as u32;
        let s = match tag & 3 {
            FULL => {
                let x = r.var_max(PERIOD as u64 - 1)? as i64;
                let y = r.var_i()?;
                let z = r.var_max(PERIOD as u64 - 1)? as i64;
                let yaw = u16::from_le_bytes(r.take(2)?.try_into()?);
                QState {
                    pos: [x, y, z],
                    yaw,
                    pitch: r.u8()?,
                }
            }
            DELTA => {
                let b = base
                    .and_then(|b| b.get(&id))
                    .with_context(|| format!("entity {id} not in the baseline"))?;
                let dx = r.var_i()?;
                let dy = r.var_i()?;
                let dz = r.var_i()?;
                let dyaw = r.var_i()?;
                let dpitch = r.var_i()?;
                let pitch = i64::from(b.pitch) + dpitch;
                ensure!((0..=255).contains(&pitch), "pitch out of range");
                QState {
                    pos: [
                        offset_q(b.pos[0], dx),
                        b.pos[1] + dy,
                        offset_q(b.pos[2], dz),
                    ],
                    yaw: b.yaw.wrapping_add(dyaw as u16),
                    pitch: pitch as u8,
                }
            }
            REMOVED => {
                snap.removed.push(id);
                continue;
            }
            k => bail!("unknown entity tag {k}"),
        };
        snap.states.insert(id, s);
    }
    ensure!(r.is_empty(), "trailing bytes in a snapshot");
    Ok(snap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn quantization_error_is_below_a_centimetre() {
        let s = QState::quantize([99_999.993, -12.345, 0.004], -0.5, 1.2);
        let p = s.position();
        assert!(torus::delta_f64(p[0], 99_999.993).abs() <= 0.5 / 64.0 + 1e-9);
        assert!((p[1] + 12.345).abs() <= 0.5 / 64.0);
        assert!((s.yaw() - (std::f32::consts::TAU - 0.5)).abs() < 1e-3);
        assert!((s.pitch() - 1.2).abs() < 0.01);
    }

    #[test]
    fn small_moves_across_the_seam_are_small() {
        let a = QState::quantize([99_999.9, 64.0, 0.05], 0.1, 0.0);
        let b = QState::quantize([0.2, 64.1, 99_999.95], 0.12, 0.01);
        let base: BTreeMap<u32, QState> = [(7, a)].into();
        let now: BTreeMap<u32, QState> = [(7, b)].into();
        let bytes = encode(12, Some((11, &base)), &now, &[]);
        assert!(bytes.len() <= 3 + 7, "{} bytes", bytes.len());
        let back = decode(&bytes, |t| (t == 11).then_some(&base)).unwrap();
        assert_eq!(back.states, now);
        // Without a baseline: full states.
        let full = encode(12, None, &now, &[3]);
        let back = decode(&full, |_| None).unwrap();
        assert_eq!(back.states, now);
        assert_eq!(back.removed, vec![3]);
        // A delta whose baseline is unknown is refused.
        assert!(decode(&bytes, |_| None).is_err());
    }

    fn qstate() -> impl Strategy<Value = QState> {
        (
            0..PERIOD,
            -(1i64 << 30)..(1i64 << 30),
            0..PERIOD,
            any::<u16>(),
            any::<u8>(),
        )
            .prop_map(|(x, y, z, yaw, pitch)| QState {
                pos: [x, y, z],
                yaw,
                pitch,
            })
    }

    proptest! {
        #[test]
        fn snapshots_round_trip(
            base in prop::collection::btree_map(0u32..50, qstate(), 0..20),
            now in prop::collection::btree_map(0u32..50, qstate(), 0..20),
            removed in prop::collection::vec(0u32..50, 0..5),
        ) {
            let bytes = encode(5, Some((4, &base)), &now, &removed);
            let back = decode(&bytes, |t| (t == 4).then_some(&base)).unwrap();
            prop_assert_eq!(back.states, now);
            prop_assert_eq!(back.removed, removed);
        }

        #[test]
        fn delta_q_inverts_offset_q(a in 0..PERIOD, d in -(PERIOD / 2 - 64)..(PERIOD / 2 - 64)) {
            let b = offset_q(a, d);
            prop_assert_eq!(delta_q(a, b), d);
        }
    }
}
