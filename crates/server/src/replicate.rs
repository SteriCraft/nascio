//! Entity replication (S5): for each client, which entities go out this tick and how.
//!
//! - A spatial grid of `GRID_CELL_M` cells finds the entities within `entity_radius_m`.
//! - Each client has a priority accumulator per entity: every tick it grows by a weight that
//!   falls with distance; the `entity_fast` highest go out, plus every entity not sent for
//!   `max_gap` ticks (at least `entity_min_hz`).
//! - An entity goes as a difference from its state in the last datagram the client
//!   acknowledged (within [`BASELINE_WINDOW`] datagrams), otherwise in full.
//! - An entity that left the radius is sent as removed until a datagram carrying the removal
//!   is acknowledged.
//!
//! Everything is ordered (`BTreeMap`, ids): the same inputs give the same datagrams.

use std::collections::{BTreeMap, HashMap, VecDeque};

use nascio_core::consts::WORLD_SIZE_F64;
use nascio_core::torus;
use nascio_protocol::entity::{BASELINE_WINDOW, QState, Writer};

/// Side of a grid cell, metres (divides `WORLD_SIZE`).
pub const GRID_CELL_M: f64 = 32.0;
const CELLS: i32 = (WORLD_SIZE_F64 / GRID_CELL_M) as i32;

/// An entity of the tick: id, position (world metres) and quantized state.
#[derive(Clone, Copy, Debug)]
pub struct Entity {
    pub id: u32,
    pub pos: [f64; 3],
    pub q: QState,
}

/// The entities of a tick by grid cell.
pub struct Grid<'a> {
    entities: &'a [Entity],
    cells: HashMap<(i32, i32), Vec<usize>>,
}

fn cell_of(v: f64) -> i32 {
    ((torus::wrap_f64(v) / GRID_CELL_M).floor() as i32).rem_euclid(CELLS)
}

impl<'a> Grid<'a> {
    /// `entities` in increasing id order.
    pub fn new(entities: &'a [Entity]) -> Self {
        let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, e) in entities.iter().enumerate() {
            cells
                .entry((cell_of(e.pos[0]), cell_of(e.pos[2])))
                .or_default()
                .push(i);
        }
        Self { entities, cells }
    }

    /// Entities within `radius` metres of `pos` (horizontal torus distance, vertical
    /// difference), except `skip`, with their distance, in increasing id order.
    pub fn near(&self, pos: [f64; 3], radius: f64, skip: u32) -> Vec<(f64, &'a Entity)> {
        let r = (radius / GRID_CELL_M).ceil() as i32;
        let (cx, cz) = (cell_of(pos[0]), cell_of(pos[2]));
        let mut out = Vec::new();
        // At most one visit per cell, even when the range wraps over the whole world.
        let span = (2 * r + 1).min(CELLS);
        for dz in 0..span {
            for dx in 0..span {
                let key = (
                    (cx - r + dx).rem_euclid(CELLS),
                    (cz - r + dz).rem_euclid(CELLS),
                );
                // Lookups only: the map's order never shows.
                let Some(list) = self.cells.get(&key) else {
                    continue;
                };
                for &i in list {
                    let e = &self.entities[i];
                    if e.id == skip {
                        continue;
                    }
                    let d = [
                        torus::delta_f64(pos[0], e.pos[0]),
                        e.pos[1] - pos[1],
                        torus::delta_f64(pos[2], e.pos[2]),
                    ];
                    let d = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if d <= radius {
                        out.push((d, e));
                    }
                }
            }
        }
        out.sort_by_key(|(_, e)| e.id);
        out
    }
}

/// Settings of the replication.
#[derive(Clone, Copy, Debug)]
pub struct ReplConfig {
    pub radius_m: f64,
    /// Entities sent each tick by priority.
    pub fast: usize,
    /// An entity in range is sent at least once every `max_gap` ticks.
    pub max_gap: u32,
    /// Bytes per tick for the entities of one client.
    pub bytes_per_tick: usize,
    /// Size of a datagram, bytes.
    pub max_datagram: usize,
}

/// One datagram sent, kept until it is acknowledged or too old.
struct Sent {
    seq: u32,
    states: Vec<(u32, QState)>,
    removed: Vec<u32>,
}

/// What a client's replication knows of one entity in range.
#[derive(Clone, Copy, Debug)]
struct Slot {
    id: u32,
    /// Priority accumulator.
    acc: f32,
    /// Tick the entity was last sent (0: never).
    last_tick: u32,
    /// Seq of the last datagram carrying its state (0: none; the client does not hold it).
    seq: u32,
    /// Last acknowledged state, with its datagram.
    acked: Option<(u32, QState)>,
}

/// Replication state of one client.
#[derive(Default)]
pub struct Replicator {
    next_seq: u32,
    /// The entities in range, by id (merged with each tick's neighbours in one pass).
    slots: Vec<Slot>,
    sent: VecDeque<Sent>,
    /// Entities being removed: seq of the first datagram carrying the removal (0: not yet).
    removing: BTreeMap<u32, u32>,
}

/// Priority weight of an entity at distance `d` metres.
fn weight(d: f64) -> f32 {
    let k = d / GRID_CELL_M;
    (1.0 / (1.0 + k * k)) as f32
}

/// Worst size of one entry (full state or difference), bytes.
const MAX_ENTRY: usize = 24;

/// Higher priority first, then lower id.
fn by_priority(a: &(f32, usize, u32), b: &(f32, usize, u32)) -> std::cmp::Ordering {
    b.0.total_cmp(&a.0).then(a.2.cmp(&b.2))
}

impl Replicator {
    /// The datagrams of `tick` for a client seeing `near` (from [`Grid::near`], by id).
    pub fn tick(&mut self, tick: u32, near: &[(f64, &Entity)], cfg: &ReplConfig) -> Vec<Vec<u8>> {
        if self.next_seq == 0 {
            self.next_seq = 1;
        }
        // Merge the slots with the neighbours (both by id): entities that left go to
        // `removing` if the client may hold them; new ones get a slot.
        let old = std::mem::take(&mut self.slots);
        let mut slots = Vec::with_capacity(near.len());
        let mut it = old.into_iter().peekable();
        for &(d, e) in near {
            while let Some(s) = it.next_if(|s| s.id < e.id) {
                if s.seq != 0 {
                    self.removing.insert(s.id, 0);
                }
            }
            let mut slot = it.next_if(|s| s.id == e.id).unwrap_or(Slot {
                id: e.id,
                acc: 0.0,
                last_tick: 0,
                seq: 0,
                acked: None,
            });
            slot.acc += weight(d);
            slots.push(slot);
        }
        for s in it {
            if s.seq != 0 {
                self.removing.insert(s.id, 0);
            }
        }
        if !self.removing.is_empty() {
            for &(_, e) in near {
                self.removing.remove(&e.id);
            }
        }
        // Priorities: every entity not sent for `max_gap` ticks, then the `fast` highest.
        let mut forced: Vec<usize> = Vec::new();
        let mut rest: Vec<(f32, usize, u32)> = Vec::new();
        for (i, s) in slots.iter().enumerate() {
            if s.last_tick == 0 || tick.wrapping_sub(s.last_tick) >= cfg.max_gap {
                forced.push(i);
            } else {
                rest.push((s.acc, i, s.id));
            }
        }
        if rest.len() > cfg.fast {
            if cfg.fast > 0 {
                rest.select_nth_unstable_by(cfg.fast - 1, by_priority);
            }
            rest.truncate(cfg.fast);
        }
        rest.sort_unstable_by(by_priority);
        let chosen = forced.into_iter().chain(rest.into_iter().map(|r| r.1));

        // Datagrams.
        let mut out = Vec::new();
        let mut total = 0;
        let mut w = Writer::new(self.next_seq, tick);
        let mut cur = Sent {
            seq: self.next_seq,
            states: Vec::new(),
            removed: Vec::new(),
        };
        for (&id, first) in self.removing.iter_mut() {
            w.removed(id);
            cur.removed.push(id);
            if *first == 0 {
                *first = cur.seq;
            }
        }
        for i in chosen {
            if total + w.len() + MAX_ENTRY > cfg.bytes_per_tick {
                break;
            }
            if w.len() + MAX_ENTRY > cfg.max_datagram {
                total += w.len();
                let next = cur.seq + 1;
                out.push(std::mem::replace(&mut w, Writer::new(next, tick)).finish());
                self.push_sent(std::mem::replace(
                    &mut cur,
                    Sent {
                        seq: next,
                        states: Vec::new(),
                        removed: Vec::new(),
                    },
                ));
            }
            let s = &mut slots[i];
            let q = near[i].1.q;
            let base = s
                .acked
                .as_ref()
                .filter(|(b, _)| cur.seq - b < BASELINE_WINDOW)
                .map(|(b, bq)| (*b, bq));
            w.state(s.id, &q, base);
            cur.states.push((s.id, q));
            s.acc = 0.0;
            s.last_tick = tick;
            s.seq = cur.seq;
        }
        self.slots = slots;
        if !w.is_empty() {
            out.push(w.finish());
            self.next_seq = cur.seq + 1;
            self.push_sent(cur);
        } else {
            self.next_seq = cur.seq;
        }
        out
    }

    fn push_sent(&mut self, s: Sent) {
        let newest = s.seq;
        self.sent.push_back(s);
        while self
            .sent
            .front()
            .is_some_and(|f| newest - f.seq >= BASELINE_WINDOW)
        {
            self.sent.pop_front();
        }
    }

    /// The client received datagram `seq`.
    pub fn ack(&mut self, seq: u32) {
        let Ok(i) = self.sent.binary_search_by_key(&seq, |s| s.seq) else {
            return;
        };
        let s = &self.sent[i];
        for &(id, q) in &s.states {
            // Only for an entity still in range: a removed one starts again in full.
            if let Ok(k) = self.slots.binary_search_by_key(&id, |s| s.id) {
                let slot = &mut self.slots[k];
                if slot.seq != 0 && slot.acked.is_none_or(|a| a.0 < seq) {
                    slot.acked = Some((seq, q));
                }
            }
        }
        for id in &s.removed {
            if self
                .removing
                .get(id)
                .is_some_and(|&first| first != 0 && first <= seq)
            {
                self.removing.remove(id);
            }
        }
    }

    /// Entities the client is believed to hold.
    pub fn known(&self) -> usize {
        self.slots.iter().filter(|s| s.seq != 0).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nascio_protocol::entity::decode;

    fn cfg() -> ReplConfig {
        ReplConfig {
            radius_m: 256.0,
            fast: 64,
            max_gap: 4,
            bytes_per_tick: 64 << 10,
            max_datagram: 1_200,
        }
    }

    fn entity(id: u32, pos: [f64; 3]) -> Entity {
        Entity {
            id,
            pos,
            q: QState::quantize(pos, 0.0, 0.0),
        }
    }

    /// A client that keeps every datagram it decodes, as `nascio-client` does.
    #[derive(Default)]
    struct Client {
        history: BTreeMap<u32, BTreeMap<u32, QState>>,
        holds: BTreeMap<u32, QState>,
    }

    impl Client {
        fn receive(&mut self, d: &[u8]) -> u32 {
            let snap = decode(d, |s, id| self.history.get(&s)?.get(&id).copied()).unwrap();
            for id in &snap.removed {
                self.holds.remove(id);
            }
            self.holds.extend(snap.states.clone());
            self.history.insert(snap.seq, snap.states);
            snap.seq
        }
    }

    #[test]
    fn the_grid_finds_neighbours_across_the_seam() {
        let es = vec![
            entity(1, [99_990.0, 60.0, 5.0]),
            entity(2, [10.0, 60.0, 99_995.0]),
            entity(3, [300.0, 60.0, 0.0]),
            entity(4, [0.0, 60.0, 0.0]),
        ];
        let g = Grid::new(&es);
        let n: Vec<u32> = g
            .near([0.0, 60.0, 0.0], 50.0, 4)
            .iter()
            .map(|(_, e)| e.id)
            .collect();
        assert_eq!(n, vec![1, 2]);
    }

    #[test]
    fn crowds_are_sent_by_priority_and_every_entity_often_enough() {
        // 300 entities on a spiral around the client, which crosses the seam.
        let me = [5.0, 60.0, 99_998.0];
        let mut es: Vec<Entity> = (1..=300u32)
            .map(|i| {
                let a = f64::from(i) * 0.7;
                let r = 2.0 + f64::from(i) * 0.6;
                entity(i, [me[0] + r * a.cos(), 60.0, me[2] + r * a.sin()])
            })
            .collect();
        let mut rep = Replicator::default();
        let mut client = Client::default();
        let mut last_seen: BTreeMap<u32, u32> = BTreeMap::new();
        let mut max_gap = 0;
        let mut near_every_tick = true;
        let mut count = 0u32;
        for tick in 1..=100u32 {
            // Everyone moves a little; positions wrap on the torus.
            for e in &mut es {
                e.pos[0] = torus::wrap_f64(e.pos[0] + 0.1);
                e.pos[2] = torus::wrap_f64(e.pos[2]);
                e.q = QState::quantize(e.pos, 0.0, 0.0);
            }
            let g = Grid::new(&es);
            let near = g.near(me, 256.0, 0);
            assert_eq!(near.len(), 300);
            let ds = rep.tick(tick, &near, &cfg());
            for d in &ds {
                assert!(d.len() <= 1_200, "{} bytes", d.len());
                // One datagram in ten is lost; the others are received and acknowledged.
                count += 1;
                if count % 10 != 3 {
                    rep.ack(client.receive(d));
                }
                // What was sent (lost or not).
                let s = decode(d, |_, _| Some(QState::quantize([0.0; 3], 0.0, 0.0))).unwrap();
                for id in s.states.keys() {
                    if let Some(t) = last_seen.insert(*id, tick) {
                        max_gap = max_gap.max(tick - t);
                    }
                }
            }
            // The three closest are sent every tick.
            near_every_tick &= (1..=3).all(|id| last_seen.get(&id) == Some(&tick));
        }
        assert!(max_gap <= 4, "an entity waited {max_gap} ticks");
        assert!(near_every_tick);
        // What the client holds is exactly the last states sent.
        for e in &es {
            let held = client.holds[&e.id];
            assert!(
                (0..3).all(|k| (held.pos[k] - e.q.pos[k]).abs() <= 64 * 4),
                "entity {} too far from its state",
                e.id
            );
        }
    }

    #[test]
    fn differences_are_small_and_removals_are_sent_until_acknowledged() {
        let mut es = vec![entity(1, [10.0, 60.0, 10.0]), entity(2, [20.0, 60.0, 10.0])];
        let me = [0.0, 60.0, 0.0];
        let mut rep = Replicator::default();
        let mut client = Client::default();
        let c = cfg();
        let first = rep.tick(1, &Grid::new(&es).near(me, 256.0, 0), &c);
        assert_eq!(first.len(), 1);
        let full = first[0].len();
        rep.ack(client.receive(&first[0]));
        es[0].pos[0] += 0.2;
        es[0].q = QState::quantize(es[0].pos, 0.0, 0.0);
        let second = rep.tick(2, &Grid::new(&es).near(me, 256.0, 0), &c);
        assert!(second[0].len() < full, "{} vs {full}", second[0].len());
        rep.ack(client.receive(&second[0]));
        assert_eq!(client.holds[&1], es[0].q);

        // Entity 2 leaves the range: removed, again and again while unacknowledged.
        es[1].pos[0] = 5_000.0;
        for t in 3..6 {
            let d = rep.tick(t, &Grid::new(&es).near(me, 256.0, 0), &c);
            let s = decode(&d[0], |q, id| client.history.get(&q)?.get(&id).copied()).unwrap();
            assert_eq!(s.removed, vec![2]);
        }
        let d = rep.tick(6, &Grid::new(&es).near(me, 256.0, 0), &c);
        rep.ack(client.receive(&d[0]));
        assert!(!client.holds.contains_key(&2));
        let d = rep.tick(7, &Grid::new(&es).near(me, 256.0, 0), &c);
        let s = decode(&d[0], |q, id| client.history.get(&q)?.get(&id).copied()).unwrap();
        assert!(s.removed.is_empty());
        assert_eq!(rep.known(), 1);
    }

    /// Cost of a battle tick, one thread: `cargo test -p nascio-server --release --lib
    /// replication_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn replication_cost_of_500_players_within_200_m() {
        let mut rng = 7u64;
        let mut next = || {
            rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (rng >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut es: Vec<Entity> = (1..=500u32)
            .map(|i| {
                let (r, a) = (200.0 * next().sqrt(), next() * std::f64::consts::TAU);
                entity(i, [50_000.0 + r * a.cos(), 60.0, 50_000.0 + r * a.sin()])
            })
            .collect();
        let mut reps: Vec<Replicator> = (0..500).map(|_| Replicator::default()).collect();
        let mut heading: Vec<f64> = (0..500).map(|_| next() * std::f64::consts::TAU).collect();
        let (mut bytes, mut best) = (0usize, f64::MAX);
        let mut window = 0.0;
        let ticks = 2_000u32;
        for tick in 1..=ticks {
            // Random walks at 4 m/s within 200 m of the centre, as the swarm does.
            for (e, h) in es.iter_mut().zip(&mut heading) {
                *h += (next() - 0.5) * 0.4;
                let (dx, dz) = (e.pos[0] - 50_000.0, e.pos[2] - 50_000.0);
                if dx.hypot(dz) > 200.0 {
                    *h = (-dz).atan2(-dx);
                }
                e.pos[0] += h.cos() * 0.2;
                e.pos[2] += h.sin() * 0.2;
                e.q = QState::quantize(e.pos, *h as f32, 0.0);
            }
            let t = std::time::Instant::now();
            let g = Grid::new(&es);
            for (e, r) in es.iter().zip(&mut reps) {
                let near = g.near(e.pos, 256.0, e.id);
                for d in r.tick(tick, &near, &cfg()) {
                    bytes += d.len();
                    // Acknowledged at once.
                    let seq = nascio_protocol::varint::Reader::new(&d).var().unwrap();
                    r.ack(seq as u32);
                }
            }
            let dt = t.elapsed().as_secs_f64();
            window += dt;
            if tick % 200 == 0 {
                eprintln!(
                    "ticks {}..{}: mean {:.1} ms",
                    tick - 199,
                    tick,
                    window / 200.0 * 1e3
                );
                window = 0.0;
            }
            if tick > 10 {
                best = best.min(dt);
            }
        }
        let n = f64::from(ticks);
        eprintln!(
            "500 players: best tick {:.1} ms on one thread; {:.0} B per client per tick",
            best * 1e3,
            bytes as f64 / n / 500.0
        );
    }
}
