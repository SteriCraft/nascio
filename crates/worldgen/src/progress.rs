//! Generation progress, for display only.
//!
//! The pipeline reports which stage runs and how far it got; a display (console line, viewer
//! window) polls [`Progress::snapshot`] from another thread. Reporting only writes these
//! counters: it never reads anything back into the generation, so it cannot change the world.

use std::sync::Mutex;
use std::time::Instant;

/// A stage of the generation, in pipeline order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Tectonics,
    Erosion,
    Climate,
    Hydrology,
    Biomes,
    Geology,
    Deposits,
    Saving,
    Previews,
}

impl Stage {
    pub const ALL: [Stage; 9] = [
        Stage::Tectonics,
        Stage::Erosion,
        Stage::Climate,
        Stage::Hydrology,
        Stage::Biomes,
        Stage::Geology,
        Stage::Deposits,
        Stage::Saving,
        Stage::Previews,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Stage::Tectonics => "tectonics",
            Stage::Erosion => "erosion",
            Stage::Climate => "climate",
            Stage::Hydrology => "hydrology",
            Stage::Biomes => "biomes",
            Stage::Geology => "geology",
            Stage::Deposits => "deposits",
            Stage::Saving => "saving",
            Stage::Previews => "previews",
        }
    }

    /// Typical share of the total time (seed 42, full resolution), for the overall percentage.
    pub fn weight(self) -> f64 {
        match self {
            Stage::Tectonics => 8.0,
            Stage::Erosion => 82.0,
            Stage::Climate => 9.5,
            Stage::Hydrology => 2.5,
            Stage::Biomes => 0.5,
            Stage::Geology => 1.5,
            Stage::Deposits => 1.0,
            Stage::Saving => 1.5,
            Stage::Previews => 4.0,
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).expect("listed")
    }
}

/// What a display shows.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// The running stage (`None` before the first and after the last).
    pub stage: Option<Stage>,
    /// Progress of the running stage, 0..1.
    pub stage_fraction: f64,
    /// Short note on the running stage (e.g. the calibration run).
    pub detail: String,
    /// Overall progress, 0..1.
    pub total_fraction: f64,
    /// Seconds since the start.
    pub elapsed_s: f64,
    /// Finished stages and their durations, s.
    pub done: Vec<(Stage, f64)>,
    pub finished: bool,
    /// Set when the generation failed.
    pub error: Option<String>,
}

struct State {
    stage: Option<Stage>,
    stage_start: Instant,
    fraction: f64,
    detail: String,
    done: Vec<(Stage, f64)>,
    finished: bool,
    error: Option<String>,
}

/// Shared progress of one generation (thread-safe).
pub struct Progress {
    start: Instant,
    state: Mutex<State>,
}

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}

impl Progress {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            state: Mutex::new(State {
                stage: None,
                stage_start: now,
                fraction: 0.0,
                detail: String::new(),
                done: Vec::new(),
                finished: false,
                error: None,
            }),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut s)
    }

    fn close_stage(s: &mut State) {
        if let Some(st) = s.stage.take() {
            s.done.push((st, s.stage_start.elapsed().as_secs_f64()));
        }
    }

    /// Starts `stage` (closing the running one).
    pub fn begin(&self, stage: Stage) {
        self.with(|s| {
            Self::close_stage(s);
            s.stage = Some(stage);
            s.stage_start = Instant::now();
            s.fraction = 0.0;
            s.detail.clear();
        });
    }

    /// Progress of the running stage, 0..1 (never goes back).
    pub fn set(&self, fraction: f64) {
        self.with(|s| s.fraction = s.fraction.max(fraction.clamp(0.0, 1.0)));
    }

    /// Progress `part` of `parts` equal sub-steps, `within` (0..1) into the current one,
    /// mapped onto the range `[from, to]` of the stage.
    pub fn set_range(&self, from: f64, to: f64, part: usize, parts: usize, within: f64) {
        let f = (part as f64 + within.clamp(0.0, 1.0)) / parts.max(1) as f64;
        self.set(from + (to - from) * f);
    }

    /// Note on the running stage.
    pub fn detail(&self, text: impl Into<String>) {
        let text = text.into();
        self.with(|s| s.detail = text);
    }

    /// Closes the last stage.
    pub fn finish(&self) {
        self.with(|s| {
            Self::close_stage(s);
            s.finished = true;
        });
    }

    /// Records a failure (and finishes).
    pub fn fail(&self, error: impl Into<String>) {
        let error = error.into();
        self.with(|s| {
            Self::close_stage(s);
            s.error = Some(error);
            s.finished = true;
        });
    }

    pub fn snapshot(&self) -> Snapshot {
        self.with(|s| {
            let total: f64 = Stage::ALL.iter().map(|st| st.weight()).sum();
            let reached = s.stage.map_or_else(
                || s.done.last().map_or(0, |d| d.0.index() + 1),
                Stage::index,
            );
            let before: f64 = Stage::ALL[..reached].iter().map(|st| st.weight()).sum();
            let current = s.stage.map_or(0.0, |st| st.weight() * s.fraction);
            Snapshot {
                stage: s.stage,
                stage_fraction: s.fraction,
                detail: s.detail.clone(),
                total_fraction: if s.finished && s.error.is_none() {
                    1.0
                } else {
                    ((before + current) / total).min(1.0)
                },
                elapsed_s: self.start.elapsed().as_secs_f64(),
                done: s.done.clone(),
                finished: s.finished,
                error: s.error.clone(),
            }
        })
    }

    /// One console line: `[ 42%] erosion 61% (step 12/20) · 0:47`.
    pub fn line(&self) -> String {
        let s = self.snapshot();
        let secs = s.elapsed_s as u64;
        let stage = s.stage.map_or_else(
            || {
                if s.finished {
                    "done".to_string()
                } else {
                    "starting".to_string()
                }
            },
            |st| format!("{} {:3.0}%", st.name(), s.stage_fraction * 100.0),
        );
        let detail = if s.detail.is_empty() {
            String::new()
        } else {
            format!(" ({})", s.detail)
        };
        format!(
            "[{:3.0}%] {stage}{detail} · {}:{:02}",
            s.total_fraction * 100.0,
            secs / 60,
            secs % 60
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_grows_with_the_stages() {
        let p = Progress::new();
        assert_eq!(p.snapshot().total_fraction, 0.0);
        let mut last = 0.0;
        for st in Stage::ALL {
            p.begin(st);
            for k in 0..=4 {
                p.set(f64::from(k) / 4.0);
                let t = p.snapshot().total_fraction;
                assert!(t >= last, "{st:?}");
                last = t;
            }
        }
        p.set(0.5); // never goes back
        assert_eq!(p.snapshot().stage_fraction, 1.0);
        p.finish();
        let s = p.snapshot();
        assert_eq!(s.total_fraction, 1.0);
        assert_eq!(s.done.len(), Stage::ALL.len());
        assert!(s.finished && s.stage.is_none());
        assert!(p.line().contains("done"));
    }
}
