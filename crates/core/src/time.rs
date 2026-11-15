//! Game time, derived from real UTC time (spec §2.3).
//!
//! `t` is the number of real seconds elapsed since the world's `GAME_EPOCH`, in Unix time
//! (POSIX: no leap seconds). `t < 0` is valid and gives negative years.
//! 1 game day = 3 600 real seconds, 1 game year = 365 game days.

use std::f64::consts::{PI, TAU};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::consts::{DAYS_PER_YEAR, SECONDS_PER_DAY, WORLD_SIZE_F64};

/// Default `GAME_EPOCH`: 2026-01-01T00:00:00Z.
pub const DEFAULT_EPOCH_UNIX_SECONDS: i64 = 1_767_225_600;

/// Maps real UTC instants to game time. Stored in `world.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameClock {
    /// Real instant at which game time is zero (year 0, day 0, midnight at x = 0).
    pub epoch: Timestamp,
}

impl Default for GameClock {
    fn default() -> Self {
        Self {
            epoch: Timestamp::from_second(DEFAULT_EPOCH_UNIX_SECONDS)
                .expect("default epoch is a valid timestamp"),
        }
    }
}

impl GameClock {
    /// Game time at a given real instant.
    pub fn time_at(&self, instant: Timestamp) -> GameTime {
        GameTime::from_seconds(instant.duration_since(self.epoch).as_secs_f64())
    }

    /// Game time now, from the system's real-time clock.
    pub fn now(&self) -> GameTime {
        self.time_at(Timestamp::now())
    }
}

/// A game instant: real seconds since `GAME_EPOCH`.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct GameTime {
    t: f64,
}

impl GameTime {
    /// Game time from real seconds since the epoch (may be negative).
    #[inline]
    pub fn from_seconds(t: f64) -> Self {
        Self { t }
    }

    /// Game time at a given year and continuous day of year `n ∈ [0, 365)`.
    /// `n`'s fractional part is the time of day at x = 0.
    #[inline]
    pub fn from_year_day(year: i64, n: f64) -> Self {
        Self::from_seconds(((year * DAYS_PER_YEAR) as f64 + n) * SECONDS_PER_DAY)
    }

    /// Real seconds since the epoch.
    #[inline]
    pub fn seconds(self) -> f64 {
        self.t
    }

    /// Index of the current game day since the epoch, `floor(t / 3600)`.
    #[inline]
    pub fn day_index(self) -> i64 {
        (self.t / SECONDS_PER_DAY).floor() as i64
    }

    /// Fraction of the day elapsed at x = 0, in `[0, 1)`. 0 is midnight at x = 0.
    #[inline]
    pub fn day_frac(self) -> f64 {
        let f = self.t.rem_euclid(SECONDS_PER_DAY) / SECONDS_PER_DAY;
        if f >= 1.0 { 0.0 } else { f }
    }

    /// Game year (0 at the epoch, negative before).
    #[inline]
    pub fn year(self) -> i64 {
        self.day_index().div_euclid(DAYS_PER_YEAR)
    }

    /// Continuous day of year `N ∈ [0, 365)`, including the time of day.
    #[inline]
    pub fn day_of_year(self) -> f64 {
        self.day_index().rem_euclid(DAYS_PER_YEAR) as f64 + self.day_frac()
    }

    /// Local solar time at world x, as a fraction of the day in `[0, 1)` (0.5 is local noon).
    #[inline]
    pub fn local_frac(self, x: f64) -> f64 {
        local_frac(self.day_frac(), x)
    }

    /// Local hour angle at world x, in radians, in `[-π, π)`. 0 at local noon, positive afternoon.
    #[inline]
    pub fn hour_angle(self, x: f64) -> f64 {
        TAU * self.local_frac(x) - PI
    }

    /// Time moved forward by `dt` real seconds.
    #[inline]
    pub fn add_seconds(self, dt: f64) -> Self {
        Self::from_seconds(self.t + dt)
    }
}

/// Local solar day fraction: `fract(day_frac + λ/360)` with `λ/360 = x / WORLD_SIZE`.
#[inline]
pub fn local_frac(day_frac: f64, x: f64) -> f64 {
    let f = (day_frac + x / WORLD_SIZE_F64).rem_euclid(1.0);
    if f >= 1.0 { 0.0 } else { f }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn default_epoch_is_2026() {
        let expected: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(GameClock::default().epoch, expected);
    }

    #[test]
    fn time_at_real_instant() {
        let clock = GameClock::default();
        let ts: Timestamp = "2026-01-01T01:30:00Z".parse().unwrap();
        let g = clock.time_at(ts);
        assert_eq!(g.seconds(), 5_400.0);
        assert_eq!(g.day_index(), 1);
        assert!(close(g.day_frac(), 0.5));
    }

    #[test]
    fn known_values() {
        let g = GameTime::from_seconds(0.0);
        assert_eq!((g.day_index(), g.year(), g.day_of_year()), (0, 0, 0.0));

        // 1 year = 365 real hours.
        let g = GameTime::from_seconds(365.0 * 3_600.0 + 900.0);
        assert_eq!(g.year(), 1);
        assert_eq!(g.day_index(), 365);
        assert!(close(g.day_of_year(), 0.25));

        let g = GameTime::from_year_day(3, 172.5);
        assert_eq!(g.year(), 3);
        assert!(close(g.day_of_year(), 172.5));
        assert!(close(g.day_frac(), 0.5));
    }

    #[test]
    fn negative_time_before_epoch() {
        let g = GameTime::from_seconds(-1.0);
        assert_eq!(g.day_index(), -1);
        assert_eq!(g.year(), -1);
        assert!(close(g.day_of_year(), 365.0 - 1.0 / 3_600.0));
        assert!(close(g.day_frac(), 1.0 - 1.0 / 3_600.0));
    }

    #[test]
    fn local_time_follows_longitude() {
        let g = GameTime::from_seconds(0.0); // midnight at x = 0
        assert_eq!(g.local_frac(0.0), 0.0);
        assert!(close(g.local_frac(50_000.0), 0.5)); // noon on the opposite meridian
        assert!(close(g.hour_angle(50_000.0), 0.0));
        assert!(close(g.local_frac(25_000.0), 0.25)); // east is later
        assert!(close(g.add_seconds(900.0).local_frac(75_000.0), 0.0));
        // Continuity across the x seam (circular difference).
        let g = GameTime::from_seconds(1_234.5);
        let a = g.local_frac(99_999.99);
        let b = g.local_frac(0.01);
        let d = (b - a).rem_euclid(1.0);
        assert!(close(d, 0.02 / 100_000.0));
    }

    proptest! {
        #[test]
        fn ranges(t in -1e10f64..1e10, x in 0f64..100_000.0) {
            let g = GameTime::from_seconds(t);
            prop_assert!((0.0..1.0).contains(&g.day_frac()));
            prop_assert!((0.0..365.0).contains(&g.day_of_year()));
            prop_assert!((0.0..1.0).contains(&g.local_frac(x)));
            let h = g.hour_angle(x);
            prop_assert!((-PI..PI).contains(&h));
        }

        #[test]
        fn day_of_year_continuous(t in -1e9f64..1e9) {
            // N advances by dt/3600 except where it wraps from 365 to 0.
            let a = GameTime::from_seconds(t).day_of_year();
            let b = GameTime::from_seconds(t + 1.0).day_of_year();
            let d = (b - a).rem_euclid(365.0);
            prop_assert!((d - 1.0 / 3_600.0).abs() < 1e-6);
        }

        #[test]
        fn decomposition_is_consistent(t in -1e10f64..1e10) {
            let g = GameTime::from_seconds(t);
            let rebuilt = GameTime::from_year_day(g.year(), g.day_of_year()).seconds();
            prop_assert!((rebuilt - t).abs() < 1e-3);
        }
    }
}
