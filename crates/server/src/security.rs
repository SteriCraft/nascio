//! Limits and the abuse log (S7): token buckets for the messages of a client, and a log of
//! what was refused (`<data>/server/abuse.log`, one line per event, appended).

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A token bucket: `rate` tokens per second, at most `burst` saved.
#[derive(Clone, Debug)]
pub struct Bucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl Bucket {
    pub fn new(rate: f64, burst: f64, now: Instant) -> Self {
        Self {
            rate,
            burst,
            tokens: burst,
            last: now,
        }
    }

    /// Takes one token if there is one.
    pub fn take(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// An episode of messages over the limit that are dropped without counting as violations:
/// it ends after a pause of [`Overrun::GAP`] without such a message. A backlog sent at once
/// after a stall lasts a moment; a flood goes on.
#[derive(Clone, Debug, Default)]
pub struct Overrun {
    /// Start of the current episode and its last message.
    episode: Option<(Instant, Instant)>,
}

impl Overrun {
    pub const GAP: Duration = Duration::from_secs(1);

    /// Records a message over the limit at `now`; returns how long the episode has lasted.
    pub fn over(&mut self, now: Instant) -> Duration {
        let start = match self.episode {
            Some((start, last)) if now.saturating_duration_since(last) <= Self::GAP => start,
            _ => now,
        };
        self.episode = Some((start, now));
        now.saturating_duration_since(start)
    }
}

/// The abuse log.
pub struct AbuseLog {
    file: Mutex<Option<std::fs::File>>,
    pub events: AtomicU64,
}

impl AbuseLog {
    /// Appends to `path` (created if needed); without a file, events are only counted.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            file: Mutex::new(Some(f)),
            events: AtomicU64::new(0),
        })
    }

    /// One event: who (address, name), what kind, details. Also printed on stderr.
    pub fn log(&self, unix_ms: i64, ip: IpAddr, name: &str, kind: &str, detail: &str) {
        self.events.fetch_add(1, Ordering::Relaxed);
        let line = format!("{unix_ms}\t{ip}\t{name}\t{kind}\t{detail}\n");
        eprint!("abuse: {line}");
        if let Some(f) = self.file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// Failed logins by address over the last minute.
#[derive(Default)]
pub struct LoginGuard {
    failures: Mutex<BTreeMap<IpAddr, VecDeque<Instant>>>,
}

impl LoginGuard {
    /// Whether `ip` may try again (fewer than `limit` failures in the last minute).
    pub fn allowed(&self, ip: IpAddr, limit: usize, now: Instant) -> bool {
        let mut f = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let Some(q) = f.get_mut(&ip) else {
            return true;
        };
        while q
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) > Duration::from_secs(60))
        {
            q.pop_front();
        }
        if q.is_empty() {
            f.remove(&ip);
            return true;
        }
        q.len() < limit
    }

    pub fn failed(&self, ip: IpAddr, now: Instant) {
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(ip)
            .or_default()
            .push_back(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_overrun_lasts_until_a_pause() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut o = Overrun::default();
        assert_eq!(o.over(t0), Duration::ZERO);
        // A steady flood: messages every 100 ms keep the episode going.
        let mut last = Duration::ZERO;
        for i in 1..=120 {
            last = o.over(ms(100 * i));
        }
        assert_eq!(last, Duration::from_secs(12));
        // After a pause longer than the gap, a new episode starts.
        assert_eq!(o.over(ms(12_000 + 1_001)), Duration::ZERO);
        assert_eq!(o.over(ms(14_000)), Duration::from_millis(999));
    }

    #[test]
    fn buckets_allow_bursts_then_the_rate() {
        let t0 = Instant::now();
        let mut b = Bucket::new(10.0, 5.0, t0);
        assert_eq!((0..8).filter(|_| b.take(t0)).count(), 5);
        // One second later: 10 more at most, capped by the burst.
        let t1 = t0 + Duration::from_secs(1);
        assert_eq!((0..8).filter(|_| b.take(t1)).count(), 5);
        let t2 = t1 + Duration::from_millis(100);
        assert_eq!((0..3).filter(|_| b.take(t2)).count(), 1);
    }

    #[test]
    fn failed_logins_block_an_address_for_a_minute() {
        let g = LoginGuard::default();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(g.allowed(ip, 3, t0));
            g.failed(ip, t0);
        }
        assert!(!g.allowed(ip, 3, t0));
        assert!(g.allowed("10.0.0.2".parse().unwrap(), 3, t0));
        assert!(g.allowed(ip, 3, t0 + Duration::from_secs(61)));
    }
}
