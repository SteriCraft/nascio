//! Plate tectonics on the periodic 625² grid (spec §4.1), after Cortial et al. (2019),
//! adapted to a torus.
//!
//! The simulation keeps a crust state per cell and moves each plate by whole cells: a plate's
//! exact translation is accumulated in `f64` and every step shifts it by
//! `round(v·(t+1)/c) − round(v·t/c)` cells, so slow plates still move at coarse resolutions.
//! Each step is a *gather*: every cell looks back at where each nearby plate came from, which is
//! independent per cell (hence parallel and deterministic).

mod finish;
mod hotspot;
mod init;
mod step;

pub(crate) use finish::box_blur;
pub use finish::{Report, TectonicOutput};

use crate::config::WorldConfig;
use crate::grid::PeriodicGrid;

/// Crust types.
pub const OCEANIC: u8 = 0;
pub const CONTINENTAL: u8 = 1;

/// Bits of the `history_flags` layer.
pub mod flags {
    pub const OROGENY: u8 = 1 << 0;
    pub const ARC: u8 = 1 << 1;
    pub const RIFT: u8 = 1 << 2;
    pub const HOTSPOT: u8 = 1 << 3;
    pub const OPHIOLITE: u8 = 1 << 4;
    pub const TRENCH: u8 = 1 << 5;
    pub const RIDGE: u8 = 1 << 6;
}

/// State of one cell of crust.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub plate: u8,
    pub crust: u8,
    /// Age in Ma (oceanic: since creation at a ridge; continental: rough craton age).
    pub age: f32,
    /// Accumulated collision uplift potential, in metres (before smoothing).
    pub orogeny: f32,
    /// Height of the hotspot edifice when built, in metres (0 if none).
    pub hotspot: f32,
    /// Ma since the hotspot edifice was last built, negative if none.
    pub hotspot_age: f32,
    pub flags: u8,
}

/// A rigid plate.
#[derive(Clone, Debug, PartialEq)]
pub struct Plate {
    pub id: u8,
    /// Initial Voronoi seed, in world metres.
    pub seed: (f64, f64),
    /// Translation per step, in metres.
    pub velocity: (f64, f64),
    /// Initial (driving) translation per step, in metres. Welding changes `velocity`, never
    /// this: the convergence of welded plates goes on slowly after a collision (uplift, §4.2).
    pub drive: (f64, f64),
}

/// A hotspot, fixed in the mantle frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Hotspot {
    pub pos: (f64, f64),
    /// Distance travelled by the plates passing over it, in metres (sets island spacing).
    pub travel_m: f64,
}

/// Event counters, for the report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventCounts {
    pub ridge_cells: u64,
    pub rift_cells: u64,
    pub ocean_continent_subductions: u64,
    pub ocean_ocean_subductions: u64,
    pub collisions: u64,
    /// Plate pairs welded after a continental collision.
    pub welds: u64,
}

/// The running simulation.
#[derive(Clone, Debug)]
pub struct Tectonics {
    pub cfg: WorldConfig,
    pub res: usize,
    pub cell_m: f64,
    pub steps: u32,
    pub dt_ma: f64,
    pub step_index: u32,
    pub plates: Vec<Plate>,
    pub hotspots: Vec<Hotspot>,
    pub grid: PeriodicGrid<Cell>,
    pub events: EventCounts,
    pub initial_continental_fraction: f64,
    /// Accumulated relative displacement of each plate pair in continental contact
    /// (`a * n + b`, `a < b`), in metres.
    pub contact: Vec<f64>,
    /// Weld groups (union-find parents; a root is its own parent).
    pub group: Vec<u8>,
}

impl Tectonics {
    /// Fastest plate speed per step, in metres (at most one cell).
    pub fn max_speed(&self) -> f64 {
        self.cfg.tectonics.total_displacement_m / f64::from(self.steps)
    }

    /// Width in metres of a narrow feature, never below `min_feature_cells` cells.
    pub fn feature(&self, metres: f64) -> f64 {
        metres.max(self.cfg.tectonics.min_feature_cells * self.cell_m)
    }

    /// Runs every remaining step.
    pub fn run(&mut self) {
        self.run_with(|_| {});
    }

    /// Runs every remaining step, calling `on_step` with the share of steps done.
    pub fn run_with(&mut self, mut on_step: impl FnMut(f64)) {
        while self.step_index < self.steps {
            self.step();
            on_step(f64::from(self.step_index) / f64::from(self.steps.max(1)));
        }
    }

    /// Share of continental cells.
    pub fn continental_fraction(&self) -> f64 {
        let n = self
            .grid
            .data()
            .iter()
            .filter(|c| c.crust == CONTINENTAL)
            .count();
        n as f64 / self.grid.data().len() as f64
    }
}
