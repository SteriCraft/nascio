//! The simulation: one thread, a fixed tick. Inputs of a tick are applied in increasing
//! player id order (arrival order within a player); then the edits ready, in player id
//! order; block changes are broadcast at the end of the edits, one message per modified
//! chunk, to the players holding its column; then every body moves ([`crate::movement`])
//! against the blocks of [`WorldView`]; last, each player gets its state and the entities.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nascio_chunk::BlockId;
use nascio_core::coords::BlockPos;
use nascio_physics::{Mode, PlayerParams, Shapes};
use nascio_protocol::ServerDatagram;
use nascio_protocol::entity::QState;
use nascio_protocol::messages::{ClientMsg, DeathCause, ServerMsg};
use nascio_protocol::player::{InputQueue, PlayerState};
use nascio_voxel::DeltaStore;
use rayon::prelude::*;
use tokio::sync::mpsc::UnboundedSender;

use crate::accounts::Change;
use crate::columns::{ColumnCache, wrap_column};
use crate::movement::{Body, FREE_RADIUS_M, Rules, Settle, reach, solid_top};
use crate::replicate::{Entity, Grid, ReplConfig, Replicator};
use crate::spawn::{Home, Spawns};
use crate::world_view::WorldView;

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
    /// The account's spawn point (respawn; arrival without a last position).
    pub home: Home,
    /// Where the player left (the arrival point when `Some`).
    pub last: Option<[f64; 3]>,
    /// Address of the connection (abuse log).
    pub ip: std::net::IpAddr,
    /// Control messages to the player.
    pub out: UnboundedSender<ServerMsg>,
    /// Columns sent to the player or on their way (it gets their block changes).
    pub subs: Mutex<BTreeSet<(i32, i32)>>,
    /// Position from the simulation; heading and pitch from the newest commands.
    pub view: Mutex<View>,
    /// Datagrams (state, entities), handed to the connection by a network task: the
    /// simulation never waits on a connection's lock (`None`: tests without a connection).
    pub datagrams: Option<UnboundedSender<Vec<u8>>>,
    /// Bytes of datagrams queued and not yet handed to the connection.
    pub queued: AtomicUsize,
    /// Largest datagram the path allows, bytes (0: unknown), kept by the network task.
    pub max_datagram: AtomicUsize,
    /// Free space of the connection's datagram buffer, bytes, refreshed by the network task
    /// after each send and every 25 ms (`usize::MAX`: no connection).
    pub send_space: AtomicUsize,
    /// Entity bytes sent since the column streamer last looked (shared budget).
    pub entity_bytes: AtomicU64,
    /// Horizon tiles asked and not sent yet, oldest first (sent after the near columns).
    pub tiles: Mutex<VecDeque<nascio_chunk::horizon::TileKey>>,
    /// Movement commands received and not applied yet.
    pub inputs: Mutex<InputQueue>,
}

/// Commands a player may have waiting: one second of steps.
pub const MAX_QUEUED_COMMANDS: usize = 64;

pub enum Input {
    Join(Arc<PlayerLink>),
    Leave(u32),
    /// A message of the player; `Fly` comes only from an administrator or the console.
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
    /// Steps taken: from commands, and without commands (quiet clients).
    pub steps: u64,
    pub filled_steps: u64,
    pub deaths: u64,
    /// Columns decoded for the simulation, and their bytes.
    pub sim_columns: usize,
    pub sim_column_bytes: usize,
    /// Durations of the last ticks, µs (at most 1 200: one minute at 20 Hz).
    pub recent_us: Vec<u32>,
    /// The entity replication part of them, µs.
    pub recent_repl_us: Vec<u32>,
    /// The movement part of them, µs.
    pub recent_move_us: Vec<u32>,
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

    /// Percentile `q` of the recent movement durations, ms.
    pub fn move_ms(&self, q: f64) -> f64 {
        percentile(&self.recent_move_us, q)
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

/// An edit waiting for the command it was made after.
struct Edit {
    pos: [i32; 3],
    block: u16,
    cmd: Option<u32>,
    /// Tick it arrived.
    since: u32,
}

/// A player in the simulation.
struct Player {
    link: Arc<PlayerLink>,
    body: Body,
    /// The spawn point (a new one is drawn when it is blocked).
    home: Home,
    edits: VecDeque<Edit>,
    /// Edits refused out of reach (the first and every hundredth are logged).
    out_of_reach: u64,
    repl: Replicator,
    /// The `PlayerState` datagram of this tick, sent with the entities.
    state: Option<Vec<u8>>,
}

pub struct Sim {
    pub deltas: Arc<DeltaStore>,
    pub cache: Arc<ColumnCache>,
    pub blocks: usize,
    pub tick: Duration,
    pub tick_hz: u32,
    pub stats: Arc<Mutex<SimStats>>,
    pub repl: ReplConfig,
    /// Pool of the replication (small: the network threads need the cores too).
    pub pool: rayon::ThreadPool,
    pub player: PlayerParams,
    pub shapes: Shapes,
    pub magma: Option<BlockId>,
    /// Farthest cell a player may edit from its eyes, m (0: not checked).
    pub edit_reach_m: f64,
    /// Columns decoded for the simulation (about).
    pub sim_columns: usize,
    /// The runtime that builds the columns.
    pub rt: tokio::runtime::Handle,
    pub abuse: Arc<crate::security::AbuseLog>,
    /// Spawn points: another one is drawn for a player whose point is blocked.
    pub spawns: Arc<Spawns>,
    /// Last positions and new points, to the accounts' writer.
    pub accounts: std::sync::mpsc::Sender<Vec<Change>>,
    /// Last positions of the players online kept this often, s (0: at departure only).
    pub position_save_s: u32,
}

/// Slack of the reach for rounding, m.
const REACH_SLACK_M: f64 = 0.01;
/// Spawn points drawn for a player whose point is blocked, before it suffocates.
pub const MAX_REDRAWS: u32 = 8;

/// What a player leaves to its account: its position, or nothing when dead (it comes back at
/// its point).
fn last_of(p: &Player) -> Change {
    Change::Last(
        p.link.name.clone(),
        (!p.body.dead).then_some(p.body.state.pos),
    )
}

impl Sim {
    pub fn run(self, rx: Receiver<Input>, stop: Arc<AtomicBool>) {
        let mut players: BTreeMap<u32, Player> = BTreeMap::new();
        let mut world = WorldView::new(
            self.cache.clone(),
            self.rt.clone(),
            self.blocks,
            self.sim_columns,
        );
        let rules = Rules {
            params: &self.player,
            shapes: &self.shapes,
            magma: self.magma,
            steps_per_tick: (self.player.step_hz / self.tick_hz.max(1)).max(1),
        };
        let mut next = Instant::now();
        let mut tick: u32 = 0;
        // Players gone, by name, with what they left: a login handled before the account
        // has it (one tick) still arrives there.
        let mut departed: BTreeMap<String, Option<[f64; 3]>> = BTreeMap::new();
        let save_every = self.position_save_s.saturating_mul(self.tick_hz);
        while !stop.load(Ordering::Relaxed) {
            next += self.tick;
            tick = tick.wrapping_add(1).max(1);
            let start = Instant::now();
            world.begin_tick();
            // Inputs of the tick, by player id (stable: arrival order within a player).
            let mut inputs: Vec<Input> = rx.try_iter().collect();
            inputs.sort_by_key(|i| match i {
                Input::Join(p) => p.id,
                Input::Leave(id) | Input::Msg(id, _) => *id,
            });
            for input in inputs {
                self.input(input, tick, &mut players, &mut world, &mut departed);
            }
            if save_every > 0 && tick.is_multiple_of(save_every) && !players.is_empty() {
                let _ = self.accounts.send(players.values().map(last_of).collect());
            }
            let (edits, refused, changed) = self.edits(tick, &mut players, &mut world);
            let m0 = Instant::now();
            let mut moved = self.movement(tick, &mut players, &mut world, &rules, &changed);
            let move_us = m0.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
            let r0 = Instant::now();
            let (datagrams, bytes, skipped) = self.replicate(tick, &mut players);
            let repl_us = r0.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
            {
                let mut s = self.stats.lock().unwrap_or_else(|e| e.into_inner());
                s.ticks += 1;
                s.edits += edits;
                s.refused += refused;
                s.entity_datagrams += datagrams;
                s.entity_bytes += bytes;
                s.entity_skipped += skipped;
                s.steps += moved.0;
                s.filled_steps += moved.1;
                s.deaths += std::mem::take(&mut moved.2);
                s.players = players.len();
                let (held, _) = world.len();
                s.sim_columns = held;
                if tick.is_multiple_of(20) {
                    s.sim_column_bytes = world.heap_bytes();
                }
                if s.recent_us.len() >= 1_200 {
                    s.recent_us.remove(0);
                    s.recent_repl_us.remove(0);
                    s.recent_move_us.remove(0);
                }
                s.recent_repl_us.push(repl_us);
                s.recent_move_us.push(move_us);
                s.recent_us
                    .push(start.elapsed().as_micros().min(u128::from(u32::MAX)) as u32);
            }
            match next.checked_duration_since(Instant::now()) {
                Some(wait) => std::thread::sleep(wait),
                // Late: do not try to catch up with a burst of ticks.
                None => next = Instant::now(),
            }
        }
        // Clean stop: every player online keeps its position.
        let _ = self.accounts.send(players.values().map(last_of).collect());
    }

    fn input(
        &self,
        input: Input,
        tick: u32,
        players: &mut BTreeMap<u32, Player>,
        world: &mut WorldView,
        departed: &mut BTreeMap<String, Option<[f64; 3]>>,
    ) {
        match input {
            Input::Join(link) => {
                let last = departed.remove(&link.name).unwrap_or(link.last);
                let body = match last {
                    Some(pos) => Body::arriving(pos, Settle::Last),
                    None => Body::new(link.home.pos),
                };
                let pos = body.state.pos;
                world.want_around(pos[0], pos[2], FREE_RADIUS_M + 1.0);
                link.view.lock().unwrap_or_else(|e| e.into_inner()).pos = pos;
                players.insert(
                    link.id,
                    Player {
                        body,
                        home: link.home,
                        link,
                        edits: VecDeque::new(),
                        out_of_reach: 0,
                        repl: Replicator::default(),
                        state: None,
                    },
                );
            }
            Input::Leave(id) => {
                if let Some(p) = players.remove(&id) {
                    let change = last_of(&p);
                    if let Change::Last(name, pos) = &change {
                        departed.insert(name.clone(), *pos);
                    }
                    let _ = self.accounts.send(vec![change]);
                }
            }
            Input::Msg(id, msg) => {
                let Some(p) = players.get_mut(&id) else {
                    return;
                };
                match msg {
                    ClientMsg::Ack { seqs } => {
                        for s in seqs {
                            p.repl.ack(s);
                        }
                    }
                    ClientMsg::SetBlock { pos, block, cmd } => p.edits.push_back(Edit {
                        pos,
                        block,
                        cmd,
                        since: tick,
                    }),
                    ClientMsg::Respawn if p.body.dead => {
                        let at = p.home.pos;
                        p.body.respawn(at);
                        world.want_around(at[0], at[2], FREE_RADIUS_M + 1.0);
                        p.link.view.lock().unwrap_or_else(|e| e.into_inner()).pos =
                            p.body.state.pos;
                        let _ = p.link.out.send(ServerMsg::Respawned {
                            pos: p.body.state.pos,
                        });
                    }
                    ClientMsg::Fly(on) => {
                        p.body.set_mode(if on { Mode::Fly } else { Mode::Walk });
                    }
                    _ => {}
                }
            }
        }
    }

    /// Applies the edits whose command was applied (refused after a second of waiting), in
    /// player id order; returns edits, refusals, and the cells changed with their height.
    fn edits(
        &self,
        tick: u32,
        players: &mut BTreeMap<u32, Player>,
        world: &mut WorldView,
    ) -> (u64, u64, Vec<([i32; 3], f64)>) {
        // The edits ready, with the eyes they were made from.
        let mut ready = Vec::new();
        for (&id, p) in players.iter_mut() {
            let applied = p
                .link
                .inputs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .applied();
            while let Some(e) = p.edits.front() {
                let early = e.cmd.is_some_and(|c| applied.is_none_or(|a| a < c));
                if early && tick.wrapping_sub(e.since) < self.tick_hz {
                    break;
                }
                let e = p.edits.pop_front().expect("front");
                let eye = (!early).then(|| p.body.eye_at(e.cmd, &self.player));
                ready.push((id, e, eye));
            }
        }
        let mut changes: BTreeMap<(i32, i32, i32), Changes> = BTreeMap::new();
        let mut changed = Vec::new();
        let (mut edits, mut refused) = (0, 0);
        for (id, e, eye) in ready {
            let p = &players[&id];
            let mut far = None;
            let at = BlockPos::new(e.pos[0], e.pos[1], e.pos[2]);
            let cell = [at.x, at.y, at.z];
            let c = at.chunk();
            let held = p
                .link
                .subs
                .lock()
                .unwrap_or_else(|er| er.into_inner())
                .contains(&wrap_column(c.cx, c.cz));
            let top = solid_top(&self.shapes, e.block);
            let result = if usize::from(e.block) >= self.blocks {
                Err(format!("unknown block {}", e.block))
            } else if !held {
                Err("column not loaded by the player".to_owned())
            } else if p.body.dead {
                Err("dead".to_owned())
            } else if let Some(d) = match eye {
                None => Some(f64::INFINITY),
                Some(eye) => {
                    let d = reach(eye, cell);
                    (self.edit_reach_m > 0.0 && d > self.edit_reach_m + REACH_SLACK_M).then_some(d)
                }
            } {
                Err(if d.is_finite() {
                    far = Some(d);
                    format!("out of reach ({d:.1} m)")
                } else {
                    "edit ahead of its movement".to_owned()
                })
            } else if top > 0.0
                && players
                    .values()
                    .any(|q| !q.body.dead && q.body.overlaps_cell(&self.player, cell, top))
            {
                Err("inside a player".to_owned())
            } else {
                self.deltas.set(at, e.block).map_err(|er| format!("{er:#}"))
            };
            match result {
                Ok(lsn) => {
                    edits += 1;
                    self.cache.invalidate(c.cx, c.cz);
                    let ch = changes.entry((c.cx, c.cy, c.cz)).or_default();
                    ch.0 = lsn;
                    ch.1.push((at.local().index() as u16, e.block));
                    if top > 0.0 {
                        changed.push((cell, top));
                    }
                }
                Err(reason) => {
                    refused += 1;
                    let _ = p
                        .link
                        .out
                        .send(ServerMsg::EditRefused { pos: e.pos, reason });
                }
            }
            if let Some(d) = far {
                let p = players.get_mut(&id).expect("player");
                p.out_of_reach += 1;
                let n = p.out_of_reach;
                if n == 1 || n.is_multiple_of(100) {
                    self.abuse.log(
                        crate::net::unix_ms(),
                        p.link.ip,
                        &p.link.name,
                        "reach",
                        &format!("edit {d:.1} m from the eyes (#{n})"),
                    );
                }
            }
        }
        // Broadcast the changes, and apply them to the simulation's blocks.
        for ((cx, cy, cz), (lsn, list)) in changes {
            world.blocks([cx, cy, cz], lsn, &list);
            let col = wrap_column(cx, cz);
            for p in players.values() {
                if p.link
                    .subs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains(&col)
                {
                    let _ = p.link.out.send(ServerMsg::Blocks {
                        chunk: [cx, cy, cz],
                        lsn,
                        changes: list.clone(),
                    });
                }
            }
        }
        (edits, refused, changed)
    }

    /// Moves every body; returns steps from commands, steps without, deaths.
    fn movement(
        &self,
        tick: u32,
        players: &mut BTreeMap<u32, Player>,
        world: &mut WorldView,
        rules: &Rules,
        changed: &[([i32; 3], f64)],
    ) -> (u64, u64, u64) {
        let (mut steps, mut filled, mut deaths) = (0, 0, 0);
        for p in players.values_mut() {
            let b = &mut p.body;
            let pos = b.state.pos;
            // The cells a step may reach (2 m at 120 m/s), asked ahead; around a body being
            // placed, every column within reach of its free places.
            let placing = b.settle.is_some() && !b.dead;
            let margin = if placing { FREE_RADIUS_M + 1.0 } else { 2.5 };
            world.want_around(pos[0], pos[2], margin);
            let mut died = None;
            // Shut in by a block changed this tick (or earlier, its cells then unknown).
            if !b.dead
                && b.settle.is_none()
                && (b.unstick
                    || changed
                        .iter()
                        .any(|(c, top)| b.overlaps_cell(&self.player, *c, *top)))
            {
                match b.unstuck(rules, &*world) {
                    Ok(d) => {
                        died = d;
                        b.unstick = false;
                    }
                    Err(_) => b.unstick = true,
                }
            }
            let mut q = p.link.inputs.lock().unwrap_or_else(|e| e.into_inner());
            // A body is placed once every column around it is there.
            let ready = !placing || world.has_around(pos[0], pos[2], margin);
            if died.is_none() && ready {
                let a = b.advance(&mut q, rules, &*world);
                steps += u64::from(a.applied);
                filled += u64::from(a.filled);
                died = a.died;
                match a.nowhere {
                    None => {}
                    Some(Settle::Home { redraws }) if redraws < MAX_REDRAWS => {
                        match self.spawns.redraw(&p.home) {
                            Ok(home) => {
                                crate::say!(
                                    "{}: spawn point blocked, another one drawn at {:.0}, {:.0}",
                                    p.link.name,
                                    home.pos[0],
                                    home.pos[2]
                                );
                                if let Some(s) = home.spawn {
                                    let _ = self
                                        .accounts
                                        .send(vec![Change::Spawn(p.link.name.clone(), s)]);
                                }
                                p.home = home;
                                b.restart(
                                    home.pos,
                                    Settle::Home {
                                        redraws: redraws + 1,
                                    },
                                );
                                world.want_around(home.pos[0], home.pos[2], margin);
                            }
                            Err(e) => {
                                crate::say_err!("{}: no other spawn point: {e:#}", p.link.name);
                                died = Some(DeathCause::Suffocation);
                            }
                        }
                    }
                    Some(_) => died = Some(DeathCause::Suffocation),
                }
            }
            if let Some(cause) = died {
                b.dead = true;
                b.state.vel = [0.0; 3];
                deaths += 1;
                let _ = p.link.out.send(ServerMsg::Died { cause });
                crate::say!(
                    "{} died ({})",
                    p.link.name,
                    match cause {
                        DeathCause::Suffocation => "suffocation",
                        DeathCause::Magma => "magma",
                    }
                );
            }
            let state = PlayerState {
                tick,
                ack: q.applied(),
                state: b.state,
            };
            drop(q);
            p.link.view.lock().unwrap_or_else(|e| e.into_inner()).pos = b.state.pos;
            // Sent with the entities (same budget, same skipping when the link is full).
            p.state = state.encode().ok();
        }
        (steps, filled, deaths)
    }

    /// Sends every player its state, then the entities around it; returns datagrams and
    /// bytes of entities sent, and the players skipped.
    fn replicate(&self, tick: u32, players: &mut BTreeMap<u32, Player>) -> (u64, u64, u64) {
        // The entities are the players (by id: `players` is ordered); the dead are not seen.
        let entities: Vec<Entity> = players
            .values()
            .map(|p| {
                let v = *p.link.view.lock().unwrap_or_else(|e| e.into_inner());
                Entity {
                    id: p.link.id,
                    pos: v.pos,
                    q: QState::quantize(v.pos, v.yaw, v.pitch),
                }
            })
            .collect();
        let alive: Vec<Entity> = entities
            .iter()
            .zip(players.values())
            .filter(|(_, p)| !p.body.dead)
            .map(|(e, _)| *e)
            .collect();
        let grid = Grid::new(&alive);
        // One client per task: each one's datagrams depend on its own state only, so the
        // result does not depend on the order (determinism).
        let mut work: Vec<(&Entity, &mut Player)> =
            entities.iter().zip(players.values_mut()).collect();
        debug_assert!(work.iter().all(|(e, p)| e.id == p.link.id));
        self.pool.install(|| {
            work.par_iter_mut()
                .map(|(e, p)| {
                    let mut cfg = self.repl;
                    let link = &p.link;
                    let m = link.max_datagram.load(Ordering::Relaxed);
                    if m > 0 {
                        cfg.max_datagram = cfg.max_datagram.min(m);
                    }
                    let state = p.state.take();
                    // The link is not keeping up (datagrams still queued from earlier ticks):
                    // skip this client for a tick rather than queue stale states.
                    let per_tick = cfg.bytes_per_tick.max(cfg.max_datagram);
                    if link.queued.load(Ordering::Relaxed) > 2 * per_tick
                        || link.send_space.load(Ordering::Relaxed) < 2 * per_tick
                    {
                        return (0, 0, 1);
                    }
                    let send = |d: Vec<u8>| {
                        if let Some(tx) = &link.datagrams {
                            link.queued.fetch_add(d.len(), Ordering::Relaxed);
                            let _ = tx.send(d);
                        }
                    };
                    // The player's own state first, out of the same budget.
                    if let Some(s) = state {
                        debug_assert_eq!(s[0], ServerDatagram::PlayerState as u8);
                        cfg.bytes_per_tick = cfg.bytes_per_tick.saturating_sub(s.len());
                        send(s);
                    }
                    let near = grid.near(e.pos, cfg.radius_m, link.id);
                    let (mut n, mut sent) = (0u64, 0u64);
                    for d in p.repl.tick(tick, &near, &cfg) {
                        sent += d.len() as u64;
                        n += 1;
                        send(d);
                    }
                    link.entity_bytes.fetch_add(sent, Ordering::Relaxed);
                    (n, sent, 0)
                })
                .reduce(|| (0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2))
        })
    }
}
