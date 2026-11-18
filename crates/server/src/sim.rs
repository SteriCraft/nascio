//! The simulation: one thread, a fixed tick. Inputs of a tick are applied in increasing
//! player id order (arrival order within a player); block changes are broadcast at the end
//! of the tick, one message per modified chunk, to the players holding its column.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nascio_core::coords::BlockPos;
use nascio_protocol::messages::{ClientMsg, ServerMsg};
use nascio_voxel::DeltaStore;
use tokio::sync::mpsc::UnboundedSender;

use crate::columns::{ColumnCache, wrap_column};

/// Block changes of one chunk in a tick: number of the last edit, `(index, block)` in order.
type Changes = (u64, Vec<(u16, u16)>);

/// Where a player is and looks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct View {
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// What the simulation and the connection of a player share.
#[derive(Debug)]
pub struct PlayerLink {
    pub id: u32,
    pub name: String,
    /// Control messages to the player.
    pub out: UnboundedSender<ServerMsg>,
    /// Columns sent to the player or on their way (it gets their block changes).
    pub subs: Mutex<BTreeSet<(i32, i32)>>,
    pub view: Mutex<View>,
}

pub enum Input {
    Join(Arc<PlayerLink>),
    Leave(u32),
    Msg(u32, ClientMsg),
}

/// Tick statistics since the start.
#[derive(Clone, Debug, Default)]
pub struct SimStats {
    pub ticks: u64,
    pub edits: u64,
    pub refused: u64,
    pub players: usize,
    /// Durations of the last ticks, µs (at most 1 200: one minute at 20 Hz).
    pub recent_us: Vec<u32>,
}

impl SimStats {
    /// Percentile `q` (0 to 1) of the recent tick durations, ms.
    pub fn tick_ms(&self, q: f64) -> f64 {
        let mut v = self.recent_us.clone();
        if v.is_empty() {
            return 0.0;
        }
        v.sort_unstable();
        f64::from(v[((v.len() - 1) as f64 * q) as usize]) / 1e3
    }
}

pub struct Sim {
    pub deltas: Arc<DeltaStore>,
    pub cache: Arc<ColumnCache>,
    pub blocks: usize,
    pub tick: Duration,
    pub stats: Arc<Mutex<SimStats>>,
}

impl Sim {
    pub fn run(self, rx: Receiver<Input>, stop: Arc<AtomicBool>) {
        let mut players: BTreeMap<u32, Arc<PlayerLink>> = BTreeMap::new();
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            next += self.tick;
            let start = Instant::now();
            // Inputs of the tick, by player id (stable: arrival order within a player).
            let mut inputs: Vec<Input> = rx.try_iter().collect();
            inputs.sort_by_key(|i| match i {
                Input::Join(p) => p.id,
                Input::Leave(id) | Input::Msg(id, _) => *id,
            });
            let mut changes: BTreeMap<(i32, i32, i32), Changes> = BTreeMap::new();
            let (mut edits, mut refused) = (0, 0);
            for input in inputs {
                match input {
                    Input::Join(p) => {
                        players.insert(p.id, p);
                    }
                    Input::Leave(id) => {
                        players.remove(&id);
                    }
                    Input::Msg(id, ClientMsg::SetBlock { pos, block }) => {
                        let Some(p) = players.get(&id) else { continue };
                        let at = BlockPos::new(pos[0], pos[1], pos[2]);
                        let c = at.chunk();
                        let held = p
                            .subs
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .contains(&wrap_column(c.cx, c.cz));
                        let result = if usize::from(block) >= self.blocks {
                            Err(anyhow::anyhow!("unknown block {block}"))
                        } else if !held {
                            Err(anyhow::anyhow!("column not loaded by the player"))
                        } else {
                            self.deltas.set(at, block)
                        };
                        match result {
                            Ok(lsn) => {
                                edits += 1;
                                self.cache.invalidate(c.cx, c.cz);
                                let e = changes.entry((c.cx, c.cy, c.cz)).or_default();
                                e.0 = lsn;
                                e.1.push((at.local().index() as u16, block));
                            }
                            Err(e) => {
                                refused += 1;
                                let _ = p.out.send(ServerMsg::EditRefused {
                                    pos,
                                    reason: format!("{e:#}"),
                                });
                            }
                        }
                    }
                    Input::Msg(_, _) => {}
                }
            }
            // Broadcast the changes of the tick.
            for ((cx, cy, cz), (lsn, list)) in changes {
                let col = wrap_column(cx, cz);
                for p in players.values() {
                    if p.subs
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .contains(&col)
                    {
                        let _ = p.out.send(ServerMsg::Blocks {
                            chunk: [cx, cy, cz],
                            lsn,
                            changes: list.clone(),
                        });
                    }
                }
            }
            {
                let mut s = self.stats.lock().unwrap_or_else(|e| e.into_inner());
                s.ticks += 1;
                s.edits += edits;
                s.refused += refused;
                s.players = players.len();
                if s.recent_us.len() >= 1_200 {
                    s.recent_us.remove(0);
                }
                s.recent_us
                    .push(start.elapsed().as_micros().min(u128::from(u32::MAX)) as u32);
            }
            match next.checked_duration_since(Instant::now()) {
                Some(wait) => std::thread::sleep(wait),
                // Late: do not try to catch up with a burst of ticks.
                None => next = Instant::now(),
            }
        }
    }
}
