//! The simulation: one thread, a fixed tick. Inputs of a tick are applied in increasing
//! player id order (arrival order within a player); block changes are broadcast at the end
//! of the tick, one message per modified chunk, to the players holding its column.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nascio_core::coords::BlockPos;
use nascio_protocol::messages::{ClientMsg, ServerMsg};
use nascio_voxel::DeltaStore;
use tokio::sync::mpsc::UnboundedSender;

use crate::columns::{ColumnCache, wrap_column};
use crate::replicate::{Entity, Grid, ReplConfig, Replicator};
use nascio_protocol::entity::QState;
use nascio_protocol::player::InputQueue;
use rayon::prelude::*;

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
    /// Entity datagrams, handed to the connection by a network task: the simulation never
    /// waits on a connection's lock (`None`: tests without a connection).
    pub datagrams: Option<UnboundedSender<Vec<u8>>>,
    /// Bytes of entity datagrams queued and not yet handed to the connection.
    pub queued: AtomicUsize,
    /// Largest datagram the path allows, bytes (0: unknown), kept by the network task.
    pub max_datagram: AtomicUsize,
    /// Free space of the connection's datagram buffer, bytes, refreshed by the network task
    /// after each send and every 25 ms (`usize::MAX`: no connection).
    pub send_space: AtomicUsize,
    /// Entity bytes sent since the column streamer last looked (shared budget).
    pub entity_bytes: AtomicU64,
    /// Horizon tiles asked and not sent yet, oldest first (sent after the near columns).
    pub tiles: Mutex<std::collections::VecDeque<nascio_chunk::horizon::TileKey>>,
    /// Movement commands received and not applied yet (applied by the simulation in 5.3).
    pub inputs: Mutex<InputQueue>,
}

/// Commands a player may have waiting: one second of steps.
pub const MAX_QUEUED_COMMANDS: usize = 64;

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
    /// Entity datagrams and bytes sent.
    pub entity_datagrams: u64,
    pub entity_bytes: u64,
    /// Client ticks skipped because the client's link was full.
    pub entity_skipped: u64,
    /// Durations of the last ticks, µs (at most 1 200: one minute at 20 Hz).
    pub recent_us: Vec<u32>,
    /// The entity replication part of them, µs.
    pub recent_repl_us: Vec<u32>,
}

impl SimStats {
    /// Percentile `q` (0 to 1) of the recent tick durations, ms.
    pub fn tick_ms(&self, q: f64) -> f64 {
        percentile(&self.recent_us, q)
    }

    /// Percentile `q` of the recent replication durations, ms.
    pub fn repl_ms(&self, q: f64) -> f64 {
        percentile(&self.recent_repl_us, q)
    }
}

/// Percentile `q` of durations in µs, ms.
fn percentile(v: &[u32], q: f64) -> f64 {
    let mut v = v.to_vec();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable();
    f64::from(v[((v.len() - 1) as f64 * q) as usize]) / 1e3
}

pub struct Sim {
    pub deltas: Arc<DeltaStore>,
    pub cache: Arc<ColumnCache>,
    pub blocks: usize,
    pub tick: Duration,
    pub stats: Arc<Mutex<SimStats>>,
    pub repl: ReplConfig,
    /// Pool of the replication (small: the network threads need the cores too).
    pub pool: rayon::ThreadPool,
}

impl Sim {
    pub fn run(self, rx: Receiver<Input>, stop: Arc<AtomicBool>) {
        let mut players: BTreeMap<u32, Arc<PlayerLink>> = BTreeMap::new();
        let mut repl: BTreeMap<u32, Replicator> = BTreeMap::new();
        let mut next = Instant::now();
        let mut tick: u32 = 0;
        while !stop.load(Ordering::Relaxed) {
            next += self.tick;
            tick = tick.wrapping_add(1).max(1);
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
                        repl.insert(p.id, Replicator::default());
                        players.insert(p.id, p);
                    }
                    Input::Leave(id) => {
                        players.remove(&id);
                        repl.remove(&id);
                    }
                    Input::Msg(id, ClientMsg::Ack { seqs }) => {
                        if let Some(r) = repl.get_mut(&id) {
                            for s in seqs {
                                r.ack(s);
                            }
                        }
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
            let r0 = Instant::now();
            let (datagrams, bytes, skipped) = self.replicate(tick, &players, &mut repl);
            let repl_us = r0.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
            {
                let mut s = self.stats.lock().unwrap_or_else(|e| e.into_inner());
                s.ticks += 1;
                s.edits += edits;
                s.refused += refused;
                s.entity_datagrams += datagrams;
                s.entity_bytes += bytes;
                s.entity_skipped += skipped;
                s.players = players.len();
                if s.recent_us.len() >= 1_200 {
                    s.recent_us.remove(0);
                    s.recent_repl_us.remove(0);
                }
                s.recent_repl_us.push(repl_us);
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

    /// Sends every player the entities around them; returns datagrams and bytes sent, and
    /// the players skipped.
    fn replicate(
        &self,
        tick: u32,
        players: &BTreeMap<u32, Arc<PlayerLink>>,
        repl: &mut BTreeMap<u32, Replicator>,
    ) -> (u64, u64, u64) {
        // The entities are the players (by id: `players` is ordered).
        let entities: Vec<Entity> = players
            .values()
            .map(|p| {
                let v = *p.view.lock().unwrap_or_else(|e| e.into_inner());
                Entity {
                    id: p.id,
                    pos: v.pos,
                    q: QState::quantize(v.pos, v.yaw, v.pitch),
                }
            })
            .collect();
        let grid = Grid::new(&entities);
        // One client per task: each one's datagrams depend on its own state only, so the
        // result does not depend on the order (determinism).
        let mut work: Vec<(&Entity, &Arc<PlayerLink>, &mut Replicator)> = entities
            .iter()
            .zip(players.values())
            .zip(repl.values_mut())
            .map(|((e, p), r)| (e, p, r))
            .collect();
        debug_assert!(work.iter().all(|(e, p, _)| e.id == p.id));
        self.pool.install(|| {
            work.par_iter_mut()
                .map(|(e, p, r)| {
                    let mut cfg = self.repl;
                    let m = p.max_datagram.load(Ordering::Relaxed);
                    if m > 0 {
                        cfg.max_datagram = cfg.max_datagram.min(m);
                    }
                    // The link is not keeping up (datagrams still queued from earlier ticks):
                    // skip this client for a tick rather than queue stale states.
                    let per_tick = cfg.bytes_per_tick.max(cfg.max_datagram);
                    if p.queued.load(Ordering::Relaxed) > 2 * per_tick
                        || p.send_space.load(Ordering::Relaxed) < 2 * per_tick
                    {
                        return (0, 0, 1);
                    }
                    let near = grid.near(e.pos, cfg.radius_m, p.id);
                    let (mut n, mut sent) = (0u64, 0u64);
                    for d in r.tick(tick, &near, &cfg) {
                        sent += d.len() as u64;
                        n += 1;
                        if let Some(tx) = &p.datagrams {
                            p.queued.fetch_add(d.len(), Ordering::Relaxed);
                            let _ = tx.send(d);
                        }
                    }
                    p.entity_bytes.fetch_add(sent, Ordering::Relaxed);
                    (n, sent, 0)
                })
                .reduce(|| (0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2))
        })
    }
}
