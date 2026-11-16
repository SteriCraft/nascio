//! One simulation step: semi-Lagrangian advection of the plates by whole cells, then
//! resolution of gaps (divergence) and overlaps (convergence).
//!
//! Every cell is resolved from the previous state only, so cells are independent and the
//! parallel map gives the same result in any execution order. Every tie is broken by a fixed
//! rule (crust type, age, plate id, or the step index), never by thread order.

use rayon::prelude::*;

use super::{CONTINENTAL, Cell, OCEANIC, Tectonics, flags};
use crate::grid::PeriodicGrid;

/// What happened at a cell during a step (for the report).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    None,
    Ridge,
    Rift,
    OceanContinent,
    OceanOcean,
    Collision,
}

/// Up to 9 distinct plate ids of a 3×3 neighbourhood, sorted.
struct Near {
    ids: [u8; 9],
    len: usize,
}

impl Near {
    fn of(g: &PeriodicGrid<Cell>, i: i64, j: i64) -> Self {
        let mut ids = [0u8; 9];
        let mut len = 0;
        for dj in -1..=1 {
            for di in -1..=1 {
                let p = g.get(i + di, j + dj).plate;
                if !ids[..len].contains(&p) {
                    ids[len] = p;
                    len += 1;
                }
            }
        }
        ids[..len].sort_unstable();
        Self { ids, len }
    }

    fn as_slice(&self) -> &[u8] {
        &self.ids[..self.len]
    }
}

/// Constants of one step, shared by all cells.
struct StepCtx<'a> {
    prev: &'a PeriodicGrid<Cell>,
    shifts: &'a [(i64, i64)],
    step: u64,
    dt: f32,
    uplift: f32,
}

impl Tectonics {
    /// Whole-cell shift of each plate during the current step.
    fn shifts(&self) -> Vec<(i64, i64)> {
        let t = f64::from(self.step_index);
        let c = self.cell_m;
        let s = |v: f64| ((v * (t + 1.0) / c).round() - (v * t / c).round()) as i64;
        self.plates
            .iter()
            .map(|p| (s(p.velocity.0), s(p.velocity.1)))
            .collect()
    }

    /// Advances the simulation by one step.
    pub fn step(&mut self) {
        // Continental contacts of this step, detected before moving so that a collision can
        // weld the plates instead of destroying a whole row of continent.
        let mut shifts = self.shifts();
        let touching = self.continental_contacts(&shifts);
        if self.weld_colliding_plates(&touching) {
            shifts = self.shifts();
        }
        let ctx = StepCtx {
            prev: &self.grid,
            shifts: &shifts,
            step: u64::from(self.step_index),
            dt: self.dt_ma as f32,
            uplift: self.cfg.tectonics.orogeny_event_uplift_m as f32,
        };
        let n = self.grid.data().len();
        let resolved: Vec<(Cell, Event)> = (0..n)
            .into_par_iter()
            .map(|idx| resolve(&ctx, idx))
            .collect();

        let mut cells = Vec::with_capacity(n);
        for (mut c, e) in resolved {
            match e {
                Event::None => {}
                Event::Ridge => self.events.ridge_cells += 1,
                Event::Rift => self.events.rift_cells += 1,
                Event::OceanContinent => self.events.ocean_continent_subductions += 1,
                Event::OceanOcean => self.events.ocean_ocean_subductions += 1,
                Event::Collision => self.events.collisions += 1,
            }
            c.age += ctx.dt;
            if c.hotspot_age >= 0.0 {
                c.hotspot_age += ctx.dt;
            }
            cells.push(c);
        }
        self.grid.data_mut().copy_from_slice(&cells);
        self.smooth_plate_ids();
        self.apply_hotspots();
        self.step_index += 1;
    }

    /// Applies [`majority_plate`] to every cell (gather from the current state).
    fn smooth_plate_ids(&mut self) {
        let g = &self.grid;
        let ids: Vec<u8> = (0..g.data().len())
            .into_par_iter()
            .map(|idx| majority_plate(g, idx))
            .collect();
        for (c, p) in self.grid.data_mut().iter_mut().zip(ids) {
            c.plate = p;
        }
    }

    /// Plate pairs (`a * n + b`, `a < b`) whose continental crust lands on each other this step.
    fn continental_contacts(&self, shifts: &[(i64, i64)]) -> Vec<bool> {
        let g = &self.grid;
        let np = self.plates.len();
        let hits: Vec<Option<(u8, u8)>> = (0..g.data().len())
            .into_par_iter()
            .map(|idx| {
                let (i, j) = g.coords(idx);
                let c = g.data()[idx];
                if c.crust != CONTINENTAL {
                    return None;
                }
                let s = shifts[c.plate as usize];
                let d = g.get(i as i64 + s.0, j as i64 + s.1);
                (d.crust == CONTINENTAL && d.plate != c.plate && shifts[d.plate as usize] != s)
                    .then(|| (c.plate.min(d.plate), c.plate.max(d.plate)))
            })
            .collect();
        let mut touching = vec![false; np * np];
        for (a, b) in hits.into_iter().flatten() {
            touching[a as usize * np + b as usize] = true;
        }
        touching
    }

    /// Continental collisions brake plates: each pair in continental contact accumulates its
    /// relative displacement, and once it exceeds `collision_shortening_m` the two plate groups
    /// are welded and move together (cell-weighted mean velocity). Pairs are processed in a
    /// fixed order.
    /// Returns whether any velocity changed.
    fn weld_colliding_plates(&mut self, touching: &[bool]) -> bool {
        let np = self.plates.len();
        let limit = self.cfg.tectonics.collision_shortening_m;
        let mut changed = false;
        for a in 0..np {
            for b in a + 1..np {
                if !touching[a * np + b] {
                    continue;
                }
                let (ra, rb) = (self.group_of(a), self.group_of(b));
                if ra == rb {
                    continue;
                }
                let (va, vb) = (self.plates[a].velocity, self.plates[b].velocity);
                let closing = (va.0 - vb.0).hypot(va.1 - vb.1);
                // Weld before this step's move would exceed the allowed shortening.
                if self.contact[a * np + b] + closing <= limit {
                    self.contact[a * np + b] += closing;
                } else {
                    // Union by lowest id keeps the result independent of processing details.
                    let (lo, hi) = (ra.min(rb), ra.max(rb));
                    self.group[hi] = lo as u8;
                    self.events.welds += 1;
                    changed = true;
                }
            }
        }
        if !changed {
            return false;
        }
        let mut count = vec![0f64; np];
        for c in self.grid.data() {
            count[c.plate as usize] += 1.0;
        }
        for root in 0..np {
            let members: Vec<usize> = (0..np).filter(|&p| self.group_of(p) == root).collect();
            if members.len() < 2 {
                continue;
            }
            let total: f64 = members.iter().map(|&p| count[p]).sum::<f64>().max(1.0);
            let v = members.iter().fold((0.0, 0.0), |acc, &p| {
                let w = count[p] / total;
                (
                    acc.0 + w * self.plates[p].velocity.0,
                    acc.1 + w * self.plates[p].velocity.1,
                )
            });
            for &p in &members {
                self.plates[p].velocity = v;
            }
        }
        true
    }

    /// Root of a plate's weld group.
    fn group_of(&self, mut p: usize) -> usize {
        while self.group[p] as usize != p {
            p = self.group[p] as usize;
        }
        p
    }
}

/// New state of cell `idx`, gathered from the previous state.
fn resolve(ctx: &StepCtx, idx: usize) -> (Cell, Event) {
    let prev = ctx.prev;
    let (i, j) = prev.coords(idx);
    let (i, j) = (i as i64, j as i64);
    let near = Near::of(prev, i, j);

    // Candidates: plates whose crust, moved by this step's shift, lands on this cell.
    let mut cands = [prev.data()[idx]; 9];
    let mut count = 0;
    for &k in near.as_slice() {
        let (sx, sz) = ctx.shifts[k as usize];
        let src = *prev.get(i - sx, j - sz);
        if src.plate == k {
            cands[count] = src;
            count += 1;
        }
    }

    match count {
        // Divergence: new oceanic crust at a ridge, or a rift if continents border the gap.
        0 => {
            // Plates that will occupy a 4-neighbour of this cell after the move: new crust must
            // touch its plate, otherwise a stray cell would travel at a foreign velocity.
            let mut border = [0u8; 9];
            let mut nb = 0;
            for &k in near.as_slice() {
                let (sx, sz) = ctx.shifts[k as usize];
                let touches = [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .any(|&(di, dj)| prev.get(i + di - sx, j + dj - sz).plate == k);
                if touches {
                    border[nb] = k;
                    nb += 1;
                }
            }
            let ids = if nb > 0 {
                &border[..nb]
            } else {
                near.as_slice()
            };
            // The whole ridge goes to the same neighbour plate during a step, rotating from step
            // to step: spreading is symmetric on average.
            let pick = ids[(ctx.step % ids.len() as u64) as usize];
            let rift = neighbourhood_has(prev, i, j, CONTINENTAL);
            let cell = Cell {
                plate: pick,
                crust: OCEANIC,
                // Born mid-step: half a step old once aged.
                age: -0.5 * ctx.dt,
                orogeny: 0.0,
                hotspot: 0.0,
                hotspot_age: -1.0,
                flags: if rift { flags::RIFT } else { 0 },
            };
            (cell, if rift { Event::Rift } else { Event::Ridge })
        }
        1 => (cands[0], Event::None),
        // Convergence: the most buoyant crust stays on top. Continental beats oceanic;
        // among oceanic crusts the youngest stays (the oldest subducts); remaining ties
        // go to the lowest plate id.
        _ => {
            let cands = &cands[..count];
            let key = |c: &Cell| {
                let age = if c.crust == CONTINENTAL { 0.0 } else { c.age };
                (c.crust != CONTINENTAL, age.to_bits(), c.plate)
            };
            let mut winner = *cands.iter().min_by_key(|c| key(c)).expect("count ≥ 2");
            let other_continental = cands
                .iter()
                .any(|c| c.plate != winner.plate && c.crust == CONTINENTAL);
            let event = if winner.crust == CONTINENTAL && other_continental {
                winner.orogeny += ctx.uplift;
                winner.flags |= flags::OROGENY;
                // A closing ocean next to the suture leaves ophiolites.
                if neighbourhood_has(prev, i, j, OCEANIC) {
                    winner.flags |= flags::OPHIOLITE;
                }
                Event::Collision
            } else if winner.crust == CONTINENTAL {
                Event::OceanContinent
            } else {
                Event::OceanOcean
            };
            (winner, event)
        }
    }
}

/// Majority filter on plate ids: a cell with at most one 4-neighbour of its own plate joins
/// a plate that holds at least three of its 4-neighbours. Crust is unchanged. Removes the
/// stray cells and one-cell spikes that make boundaries tangle.
fn majority_plate(g: &PeriodicGrid<Cell>, idx: usize) -> u8 {
    let (i, j) = g.coords(idx);
    let (i, j) = (i as i64, j as i64);
    let own = g.data()[idx].plate;
    let n = [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)].map(|(di, dj)| g.get(i + di, j + dj).plate);
    if n.iter().filter(|&&p| p == own).count() > 1 {
        return own;
    }
    n.iter()
        .copied()
        .find(|&p| n.iter().filter(|&&q| q == p).count() >= 3)
        .unwrap_or(own)
}

fn neighbourhood_has(g: &PeriodicGrid<Cell>, i: i64, j: i64, crust: u8) -> bool {
    (-1..=1).any(|dj| (-1..=1).any(|di| g.get(i + di, j + dj).crust == crust))
}

#[cfg(test)]
mod tests {
    use super::super::Plate;
    use super::*;
    use crate::config::{Scale, WorldConfig};

    /// Two plates side by side, the left one moving right by one cell per step.
    fn two_plates(left: u8, right: u8) -> Tectonics {
        let cfg = WorldConfig::new(5, Scale::TwentyFifth);
        let mut t = Tectonics::new(&cfg);
        let cell = t.cell_m;
        t.plates = vec![
            Plate {
                id: 0,
                seed: (0.0, 0.0),
                velocity: (cell, 0.0),
                drive: (cell, 0.0),
            },
            Plate {
                id: 1,
                seed: (50_000.0, 0.0),
                velocity: (0.0, 0.0),
                drive: (0.0, 0.0),
            },
        ];
        t.contact = vec![0.0; 4];
        t.group = vec![0, 1];
        let res = t.res;
        let cells: Vec<Cell> = (0..res * res)
            .map(|idx| {
                let i = idx % res;
                let plate = u8::from(i >= 12);
                Cell {
                    plate,
                    crust: if plate == 0 { left } else { right },
                    age: 50.0,
                    orogeny: 0.0,
                    hotspot: 0.0,
                    hotspot_age: -1.0,
                    flags: 0,
                }
            })
            .collect();
        t.grid = PeriodicGrid::from_vec(res, res, cell, cells);
        t
    }

    #[test]
    fn divergence_creates_young_ocean_and_convergence_subducts() {
        let mut t = two_plates(OCEANIC, OCEANIC);
        t.step();
        // Plate 0 spans columns 0..12 and moves right: column 0 opens (ridge between
        // plate 1 at column 24 and plate 0), column 12 is contested.
        let g = &t.grid;
        let ridge = g.get(0, 3);
        assert!(ridge.age < 50.0, "new crust at the opening gap");
        assert_eq!(t.events.ridge_cells, 25);
        assert_eq!(t.events.ocean_ocean_subductions, 25);
        // Equal ages: the lower plate id stays on top.
        assert_eq!(g.get(12, 3).plate, 0);
    }

    #[test]
    fn continental_collision_uplifts_and_continent_beats_ocean() {
        let mut t = two_plates(CONTINENTAL, CONTINENTAL);
        t.step();
        assert_eq!(t.events.collisions, 25);
        assert!(t.grid.get(12, 3).orogeny > 0.0);
        assert_eq!(t.events.rift_cells, 25);

        let mut t = two_plates(OCEANIC, CONTINENTAL);
        t.step();
        assert_eq!(t.events.ocean_continent_subductions, 25);
        assert_eq!(t.grid.get(12, 3).plate, 1, "the continent overrides");
    }

    #[test]
    fn slow_plates_still_move() {
        let cfg = WorldConfig::new(5, Scale::TwentyFifth);
        let mut t = Tectonics::new(&cfg);
        t.plates[0].velocity = (0.25 * t.cell_m, 0.0);
        let total: i64 = (0..t.steps)
            .map(|s| {
                t.step_index = s;
                t.shifts()[0].0
            })
            .sum();
        assert_eq!(total, (0.25 * f64::from(t.steps)).round() as i64);
    }
}
