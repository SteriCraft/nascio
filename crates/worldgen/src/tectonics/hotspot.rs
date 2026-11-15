//! Hotspots: fixed in the mantle frame, they build volcanic edifices on whatever plate passes
//! over them. As the plate moves on, the edifices drift away and age, which leaves a chain of
//! islands and seamounts whose age grows with distance (Hawaii-like archipelagos). Over a
//! continent they leave a plateau (traps).

use nascio_core::torus::distance_f64;

use super::{Cell, Tectonics, flags};

impl Tectonics {
    /// Current height of a cell's hotspot edifice, after subsidence with age, in metres.
    pub fn edifice_height(&self, c: &Cell) -> f64 {
        if c.hotspot <= 0.0 || c.hotspot_age < 0.0 {
            return 0.0;
        }
        let decay =
            (1.0 - f64::from(c.hotspot_age) / self.cfg.tectonics.hotspot_decay_ma).max(0.15);
        f64::from(c.hotspot) * decay
    }

    /// Lets every active hotspot build on the plate above it. Hotspots are processed in a fixed
    /// order. A hotspot is active during the first `hotspot_duty` of every island spacing of
    /// plate travel, so islands are evenly spaced whatever the plate speed.
    pub(super) fn apply_hotspots(&mut self) {
        let t = &self.cfg.tectonics;
        let r = self.feature(t.hotspot_radius_m);
        let spacing = t.hotspot_island_spacing_m.max(3.0 * r);
        let (duty, height) = (t.hotspot_duty, t.hotspot_edifice_height_m);
        let reach = (r / self.cell_m).ceil() as i64;
        for h in 0..self.hotspots.len() {
            let pos = self.hotspots[h].pos;
            let (ci, cj) = self.grid.cell_of(pos.0, pos.1);
            let plate = self.grid.get(ci as i64, cj as i64).plate as usize;
            let v = self.plates[plate].velocity;
            self.hotspots[h].travel_m += v.0.hypot(v.1);
            if self.hotspots[h].travel_m.rem_euclid(spacing) >= duty * spacing {
                continue;
            }
            for dj in -reach..=reach {
                for di in -reach..=reach {
                    let idx = self.grid.index(ci as i64 + di, cj as i64 + dj);
                    let (i, j) = self.grid.coords(idx);
                    let d = distance_f64(self.grid.cell_center(i, j), pos);
                    if d >= r {
                        continue;
                    }
                    let k = 1.0 - (d / r) * (d / r);
                    let new = height * k * k;
                    let existing = self.edifice_height(&self.grid.data()[idx]);
                    if new >= existing {
                        let c = &mut self.grid.data_mut()[idx];
                        c.hotspot = new as f32;
                        c.hotspot_age = 0.0;
                        c.flags |= flags::HOTSPOT;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{Scale, WorldConfig};
    use crate::tectonics::Tectonics;

    #[test]
    fn hotspot_leaves_an_age_progressive_chain() {
        let mut cfg = WorldConfig::new(11, Scale::Fifth);
        cfg.tectonics.hotspot_count = [1, 1];
        let mut t = Tectonics::new(&cfg);
        t.run();
        let built: Vec<f32> = t
            .grid
            .data()
            .iter()
            .filter(|c| c.hotspot > 0.0)
            .map(|c| c.hotspot_age)
            .collect();
        assert!(!built.is_empty(), "the hotspot built nothing");
        // Edifices of different ages: the plate carried older ones away.
        let (lo, hi) = built
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &v| {
                (a.min(v), b.max(v))
            });
        assert!(hi - lo > 10.0 * t.dt_ma as f32, "ages {lo}..{hi}");
    }
}
