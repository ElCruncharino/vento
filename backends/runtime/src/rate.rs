// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The line speed a session's facts show, for a protocol that never states
//! one and is left to measure it: RFB's and IronRDP's, each from the bytes
//! its frames took to arrive. Where a library measures or is told one
//! (TigerVNC's, FreeRDP's, RealVNC's core) the backend shows that instead.

use std::time::Instant;

/// A throughput estimate for the connection panel, smoothed so it reads as a
/// speed rather than as whatever the last frame happened to cost.
pub struct Rate {
    last: Instant,
    bytes_per_second: f64,
}

impl Rate {
    pub fn new() -> Rate {
        Rate {
            last: Instant::now(),
            bytes_per_second: 0.0,
        }
    }

    /// `bytes` more have arrived since the last sample (or since this was
    /// made): the speed as the panel says it, "800 kbit/s" or "12.3 Mbit/s",
    /// or `None` where this sample says nothing about the line.
    pub fn sample(&mut self, bytes: u64) -> Option<String> {
        self.sample_at(Instant::now(), bytes)
    }

    fn sample_at(&mut self, now: Instant, bytes: u64) -> Option<String> {
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        // An idle desktop produces one update every few seconds, and dividing
        // by that gap would report a trickle as the line's speed. Only samples
        // taken while something is actually arriving say anything about the
        // line.
        if elapsed <= 0.0 || elapsed > 1.0 || bytes < 1024 {
            return None;
        }
        let instant = bytes as f64 / elapsed;
        self.bytes_per_second = if self.bytes_per_second == 0.0 {
            instant
        } else {
            self.bytes_per_second * 0.7 + instant * 0.3
        };
        let kbit = self.bytes_per_second * 8.0 / 1000.0;
        Some(if kbit >= 1000.0 {
            format!("{:.1} Mbit/s", kbit / 1000.0)
        } else {
            format!("{kbit:.0} kbit/s")
        })
    }
}

impl Default for Rate {
    fn default() -> Rate {
        Rate::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_busy_line_reads_as_a_smoothed_speed() {
        let start = Instant::now();
        let mut rate = Rate { last: start, bytes_per_second: 0.0 };
        let at = |ms| start + Duration::from_millis(ms);
        // 100 kB in 100 ms is 8 Mbit/s, taken as it is the first time.
        assert_eq!(rate.sample_at(at(100), 100_000).as_deref(), Some("8.0 Mbit/s"));
        // Then 10 kB in 100 ms (800 kbit/s) moves it by three tenths.
        assert_eq!(rate.sample_at(at(200), 10_000).as_deref(), Some("5.8 Mbit/s"));
        let mut slow = Rate { last: start, bytes_per_second: 0.0 };
        assert_eq!(slow.sample_at(at(500), 50_000).as_deref(), Some("800 kbit/s"));
    }

    #[test]
    fn a_trickle_or_a_gap_says_nothing_about_the_line() {
        let start = Instant::now();
        let mut rate = Rate { last: start, bytes_per_second: 0.0 };
        let at = |ms| start + Duration::from_millis(ms);
        assert_eq!(rate.sample_at(at(100), 1023), None, "under a kilobyte");
        assert_eq!(rate.sample_at(at(1200), 1_000_000), None, "over a second since the last");
        assert_eq!(rate.sample_at(at(1200), 1_000_000), None, "no time at all");
        // A gap does not count against the next sample, which is timed from
        // the gap's end, and nothing before it was kept.
        assert_eq!(rate.sample_at(at(1300), 100_000).as_deref(), Some("8.0 Mbit/s"));
    }
}
