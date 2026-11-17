//! Offset of the client clock from the server's UTC (spec: game time derives from UTC; no
//! internal clock). The client probes with `Ping` and keeps the sample of shortest round
//! trip, NTP-style: the server time is taken at the middle of the round trip.

/// One probe: client send and receive times, server time in the answer (Unix ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    pub sent_ms: i64,
    pub received_ms: i64,
    pub server_ms: i64,
}

/// Best estimate so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockSync {
    best: Option<(i64, i64)>,
}

impl ClockSync {
    /// Adds a probe; returns the current offset (ms to add to the client clock).
    pub fn add(&mut self, p: Probe) -> i64 {
        let rtt = (p.received_ms - p.sent_ms).max(0);
        let offset = p.server_ms - (p.sent_ms + rtt / 2);
        if self.best.is_none_or(|(r, _)| rtt < r) {
            self.best = Some((rtt, offset));
        }
        self.offset()
    }

    /// Offset in ms (0 before any probe).
    pub fn offset(&self) -> i64 {
        self.best.map_or(0, |b| b.1)
    }

    /// Round trip of the best probe, ms.
    pub fn rtt(&self) -> Option<i64> {
        self.best.map(|b| b.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_shortest_round_trip() {
        let mut c = ClockSync::default();
        // Client 1 000 ms behind; a slow probe, then a fast one.
        assert_eq!(
            c.add(Probe {
                sent_ms: 0,
                received_ms: 400,
                server_ms: 1_100,
            }),
            900
        );
        assert_eq!(
            c.add(Probe {
                sent_ms: 1_000,
                received_ms: 1_020,
                server_ms: 2_010,
            }),
            1_000
        );
        assert_eq!(
            c.add(Probe {
                sent_ms: 2_000,
                received_ms: 2_300,
                server_ms: 3_000,
            }),
            1_000
        );
        assert_eq!(c.rtt(), Some(20));
    }
}
