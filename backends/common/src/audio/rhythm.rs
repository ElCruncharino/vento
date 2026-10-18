// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! How a far end's sound arrives, as far as a queue in front of a device
//! cares: how much of it comes at once, and how late a write is now and
//! then against the sound before it. What [`Queue`](super::Queue) holds back
//! from the device is worked out from this, rather than fixed, since the
//! protocols differ by an order of magnitude: QEMU sends 10 ms of sound
//! every 10 ms within a millisecond or two, Windows sends RDP's AAC in 23 ms
//! waves up to 18 ms late and its PCM in 186 ms waves two at a time, and
//! Opus over a busy network comes in bunches.
//!
//! **Lateness** is measured against the sound itself, not against the
//! write before: each write's arrival less where its first frame falls in
//! the stream, both from the stream's first write (`d` below). A far end
//! that keeps time has a `d` that stays put; a write that is late has a
//! larger one, and the writes after it, back on time, a smaller one again.
//! What a queue that started playing just after one write has to have kept
//! to still have sound when a later one lands is that write's size and how
//! much later the later one's `d` is — so the hold is the largest write and
//! the largest rise in `d`, from an earlier write to a later one, of late.
//! (A fall, writes arriving earlier than the sound before them did, only
//! fills the queue.) Two writes that land together are one: RDP's waves
//! two at a time show up as their pair from the very first pair, before the
//! rise between pairs has been seen.
//!
//! **Of late** is the last [`WINDOW`], in [`BUCKETS`] buckets of a second:
//! long enough to have seen any rhythm a protocol keeps (a Windows PCM pair
//! is 372 ms) many times over and an occasional late write more than once,
//! short enough to forget a stall that has passed in a few seconds. Clocks
//! drift, and a far end's sound runs slightly faster or slower than the
//! device plays it; within a window that is a millisecond at most, at
//! 100 ppm, and nothing to measure against.
//!
//! **After the far end paused** — the queue ran dry, and nothing came for
//! long enough that what comes next is a new sound rather than the last one
//! late — `d` starts again from the next write, since the far end's sound
//! has a hole in it that is no lateness. The buckets keep what they saw
//! before, which is still how the far end sends. A queue that ran dry and
//! had the far end's sound again soon after does not start again: that
//! write was late, and its lateness is what the queue has to know.

use std::time::Duration;

use super::Format;

/// How long the rhythm is remembered: see the module's words.
pub const WINDOW: Duration = Duration::from_secs(8);

/// How many pieces [`WINDOW`] is kept in. The window moves a bucket at a
/// time, so what is forgotten goes a second at a time.
pub const BUCKETS: usize = 8;

/// The most a rhythm holds: Windows' PCM pair (372 ms) and its lateness,
/// with room. A far end that sends slower than its sound plays — a guest
/// that has fallen behind itself — has a lateness that rises for as long as
/// it is measured, which no queue can make up for; this is how far behind it
/// is heard while it does, rather than the window's worth.
pub const MOST: Duration = Duration::from_millis(500);

/// Two writes this close are one: RDP's waves two at a time are handed over
/// back to back, microseconds apart, and a far end's next write is never
/// sooner than a few milliseconds after the last.
const TOGETHER: Duration = Duration::from_millis(2);

/// What one bucket's writes showed. Times are microseconds, signed: `d` is
/// arrival less position, either way round.
#[derive(Clone, Copy, Default)]
struct Bucket {
    /// Which run of writes (see [`Rhythm::restart`]) the `d`s are from: two
    /// runs' are not comparable.
    run: u32,
    used: bool,
    lo: i64,
    hi: i64,
    /// The largest rise in `d` from an earlier write in this bucket to a
    /// later one in it; across buckets is worked out when asked.
    rise: i64,
    /// The largest write, and the most that landed at once, in frames.
    write: usize,
    group: usize,
}

/// See the module's words.
pub struct Rhythm {
    format: Format,
    buckets: [Bucket; BUCKETS],
    current: usize,
    /// When the current bucket began.
    started: Duration,
    run: u32,
    /// The run's first write: when it arrived, and the frames written in the
    /// run before the write being measured.
    origin: Option<Duration>,
    frames: u64,
    /// When the last write arrived, and how much has landed at once with it.
    last: Option<Duration>,
    group: usize,
}

impl Rhythm {
    pub fn new(format: Format) -> Rhythm {
        Rhythm {
            format,
            buckets: [Bucket::default(); BUCKETS],
            current: 0,
            started: Duration::ZERO,
            run: 0,
            origin: None,
            frames: 0,
            last: None,
            group: 0,
        }
    }

    /// A write of `frames` frames that arrived at `at`, on any clock that
    /// only goes forward (the queue's own, from its creation). `burst` is a
    /// write too large to be how the far end sends (a stall's backlog all at
    /// once): it still moves the stream's position on, but is no write size
    /// to keep.
    pub fn wrote(&mut self, at: Duration, frames: usize, burst: bool) {
        self.advance(at);
        let origin = *self.origin.get_or_insert(at);
        let position = self.format.duration(self.frames.min(usize::MAX as u64) as usize);
        let d = micros(at.saturating_sub(origin)) - micros(position);
        let run = self.run;
        self.frames += frames as u64;
        let together = self.last.is_some_and(|last| at.saturating_sub(last) <= TOGETHER);
        self.last = Some(at);
        let size = if burst { 0 } else { frames };
        self.group = if together { self.group + size } else { size };
        let b = &mut self.buckets[self.current];
        if !b.used || b.run != run {
            // A bucket that saw the run before keeps what it measured of it
            // (see `restart`), and measures the new one's `d`s afresh.
            *b = Bucket {
                run,
                used: true,
                lo: d,
                hi: d,
                ..*b
            };
        }
        b.rise = b.rise.max(d - b.lo);
        b.lo = b.lo.min(d);
        b.hi = b.hi.max(d);
        b.write = b.write.max(size);
        b.group = b.group.max(self.group);
    }

    /// How much later than its sound a write that arrives at `at` is, in
    /// frames of the stream, against the earliest of the run's writes in
    /// the window: what [`lateness`](Rhythm::lateness) would be for it alone,
    /// without measuring it (a queue that ran dry asks before it decides
    /// whether the write is late or a stall).
    pub fn late_by(&mut self, at: Duration) -> usize {
        self.advance(at);
        let Some(origin) = self.origin else { return 0 };
        let position = self.format.duration(self.frames.min(usize::MAX as u64) as usize);
        let d = micros(at.saturating_sub(origin)) - micros(position);
        let run = self.run;
        let least = self.buckets.iter().filter(|b| b.used && b.run == run).map(|b| b.lo).min();
        let late = least.map_or(0, |least| (d - least).max(0));
        self.format.frames(Duration::from_micros(late as u64))
    }

    /// The far end paused: the next write starts a new run of `d`s.
    pub fn restart(&mut self) {
        // The run's rise across buckets, kept as the current bucket's own,
        // since the next run's `d`s are not measured against it.
        let rise = self.rise(Some(self.run));
        let b = &mut self.buckets[self.current];
        if b.used {
            b.rise = b.rise.max(rise);
        }
        self.run = self.run.wrapping_add(1);
        self.origin = None;
        self.frames = 0;
        self.last = None;
        self.group = 0;
    }

    /// How much the queue has to hold, in frames of the stream, for the far
    /// end's writes of late not to run it dry: the largest write and the
    /// largest rise in lateness after one, or the most that has landed at
    /// once, whichever is more, and never more than [`MOST`]. Nothing before
    /// the first write.
    pub fn hold(&self) -> usize {
        let (mut write, mut group) = (0, 0);
        for b in self.buckets.iter().filter(|b| b.used) {
            write = write.max(b.write);
            group = group.max(b.group);
        }
        group.max(write + self.lateness()).min(self.format.frames(MOST))
    }

    /// The largest rise in lateness of the window, in frames of the stream:
    /// how much later than its sound a write has been, against one before
    /// it. Never more than [`MOST`].
    pub fn lateness(&self) -> usize {
        self.format.frames(Duration::from_micros(self.rise(None) as u64)).min(self.format.frames(MOST))
    }

    /// The largest rise in `d` of the window, in microseconds, from an
    /// earlier write to a later one of the same run (of `only`, if given).
    fn rise(&self, only: Option<u32>) -> i64 {
        let mut rise = 0i64;
        // Oldest first, so that each bucket's writes are measured against
        // the least `d` before them in the same run.
        let mut lo: Option<(u32, i64)> = None;
        for i in 1..=BUCKETS {
            let b = &self.buckets[(self.current + i) % BUCKETS];
            if !b.used || only.is_some_and(|run| run != b.run) {
                continue;
            }
            rise = rise.max(b.rise);
            match lo {
                Some((run, least)) if run == b.run => {
                    rise = rise.max(b.hi - least);
                    lo = Some((run, least.min(b.lo)));
                }
                _ => lo = Some((b.run, b.lo)),
            }
        }
        rise.max(0)
    }

    /// Moves the window on to `at`, forgetting a bucket a second.
    fn advance(&mut self, at: Duration) {
        let bucket = WINDOW / BUCKETS as u32;
        if at < self.started + bucket {
            return;
        }
        let steps = ((at - self.started).as_micros() / bucket.as_micros()) as u32;
        for _ in 0..steps.min(BUCKETS as u32) {
            self.current = (self.current + 1) % BUCKETS;
            self.buckets[self.current] = Bucket::default();
        }
        self.started += bucket * steps;
    }
}

fn micros(d: Duration) -> i64 {
    d.as_micros().min(i64::MAX as u128) as i64
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn ms(v: f64) -> Duration {
        Duration::from_secs_f64(v / 1000.0)
    }

    /// A deterministic lateness from 0 to 1, the same every run.
    pub(in super::super) struct Lcg(pub u32);

    impl Lcg {
        pub(in super::super) fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (self.0 >> 8) as f64 / (1u32 << 24) as f64
        }
    }

    fn rhythm(rate: u32) -> Rhythm {
        Rhythm::new(Format::new(rate, 2).unwrap())
    }

    fn held_ms(r: &Rhythm) -> f64 {
        r.format.duration(r.hold()).as_secs_f64() * 1000.0
    }

    #[test]
    fn steady_writes_hold_one_write() {
        // QEMU's 10 ms on time, and Opus's 20 ms.
        for size in [10.0, 20.0] {
            let mut r = rhythm(RATE);
            for k in 0..500 {
                r.wrote(ms(k as f64 * size), RATE as usize * size as usize / 1000, false);
            }
            assert!((held_ms(&r) - size).abs() < 0.1, "{size}: {}", held_ms(&r));
        }
    }

    #[test]
    fn lateness_adds_its_spread() {
        // 10 ms writes up to 2 ms late: a write and the 2 ms.
        let mut r = rhythm(RATE);
        let mut late = Lcg(1);
        for k in 0..1000 {
            r.wrote(ms(k as f64 * 10.0 + 2.0 * late.next()), 480, false);
        }
        assert!((11.8..=12.0).contains(&held_ms(&r)), "{}", held_ms(&r));
    }

    #[test]
    fn rdp_waves_hold_a_wave_and_their_lateness() {
        // Windows' AAC: 1024 frames at 44.1 kHz due every 23.2 ms and sent
        // up to 18 ms late, so up to two land together.
        let mut r = rhythm(44_100);
        let wave = 1024.0 / 44.1;
        let mut late = Lcg(12345);
        for k in 0..2000 {
            r.wrote(ms(k as f64 * wave + 18.0 * late.next()), 1024, false);
        }
        let held = held_ms(&r);
        assert!((40.0..=41.3).contains(&held), "{held}");
    }

    #[test]
    fn waves_two_at_a_time_hold_the_pair_from_the_first() {
        // RDP's waves two at a time, 10 ms early to 10 ms late.
        let mut r = rhythm(44_100);
        let wave = 1024.0 / 44.1;
        r.wrote(ms(0.0), 1024, false);
        r.wrote(ms(0.01), 1024, false);
        assert!((held_ms(&r) - 2.0 * wave).abs() < 0.1, "the pair at once: {}", held_ms(&r));
        let mut late = Lcg(7);
        for k in 1..400 {
            let at = k as f64 * 2.0 * wave + 10.0 - 20.0 * late.next();
            r.wrote(ms(at), 1024, false);
            r.wrote(ms(at + 0.01), 1024, false);
        }
        // The pair and up to 20 ms between pairs.
        let held = held_ms(&r);
        assert!((2.0 * wave + 18.0..=2.0 * wave + 20.0).contains(&held), "{held}");
    }

    #[test]
    fn windows_pcm_holds_its_pair() {
        // 186 ms waves, two at once every 372 ms: the pair, from the first.
        let mut r = rhythm(44_100);
        let wave = 8192.0 / 44.1;
        for k in 0..40 {
            let at = k as f64 * 2.0 * wave + if k % 3 == 0 { 5.0 } else { 0.0 };
            r.wrote(ms(at), 8192, false);
            r.wrote(ms(at + 0.05), 8192, false);
            let held = held_ms(&r);
            assert!(held >= 2.0 * wave - 0.1 && held <= 2.0 * wave + 5.1, "{k}: {held}");
        }
    }

    #[test]
    fn bunched_frames_hold_the_bunch() {
        // 20 ms Opus frames let go of in bunches, 20 to 100 ms apart.
        let mut r = rhythm(RATE);
        let mut net = Lcg(3);
        let mut release = 0.0;
        for k in 0..1500 {
            let due = k as f64 * 20.0;
            while release < due {
                release += 20.0 + 80.0 * net.next();
            }
            r.wrote(ms(release), 960, false);
        }
        // The longest wait for a frame is under 100 ms, and what lands at
        // once is never more than that.
        let held = held_ms(&r);
        assert!((90.0..=120.0).contains(&held), "{held}");
    }

    #[test]
    fn a_late_write_is_remembered_for_the_window_and_then_forgotten() {
        // A write is never handed over before the one before it: the writes
        // due while one was late land with it.
        let mut r = rhythm(RATE);
        let (mut due, mut last) = (0.0f64, 0.0f64);
        let mut write = |r: &mut Rhythm, late: f64| {
            last = last.max(due + late);
            r.wrote(ms(last), 480, false);
            due += 10.0;
        };
        for _ in 0..300 {
            write(&mut r, 0.0);
        }
        write(&mut r, 40.0);
        assert!((held_ms(&r) - 50.0).abs() < 0.1, "{}", held_ms(&r));
        // Back on time: still held while the window has it.
        for _ in 0..500 {
            write(&mut r, 0.0);
        }
        assert!((held_ms(&r) - 50.0).abs() < 0.1);
        for _ in 0..400 {
            write(&mut r, 0.0);
        }
        assert!((held_ms(&r) - 10.0).abs() < 0.1, "forgotten: {}", held_ms(&r));
    }

    #[test]
    fn lateness_that_creeps_up_across_buckets_counts() {
        // Each write 0.05 ms later than the last, for two seconds: no bucket
        // sees more than a second of it, the window sees both.
        let mut r = rhythm(RATE);
        for k in 0..200 {
            r.wrote(ms(k as f64 * 10.05), 480, false);
        }
        assert!((held_ms(&r) - 19.95).abs() < 0.1, "{}", held_ms(&r));
    }

    #[test]
    fn a_new_run_is_not_measured_against_the_last() {
        // The queue ran dry and the far end's sound came back 300 ms later
        // than it would have: that is no lateness, the stream starts again.
        let mut r = rhythm(RATE);
        for k in 0..50 {
            r.wrote(ms(k as f64 * 10.0), 480, false);
        }
        r.restart();
        for k in 0..50 {
            r.wrote(ms(800.0 + k as f64 * 10.0), 480, false);
        }
        assert!((held_ms(&r) - 10.0).abs() < 0.1, "{}", held_ms(&r));
    }

    #[test]
    fn a_burst_is_no_write_size() {
        let mut r = rhythm(RATE);
        for k in 0..20 {
            r.wrote(ms(k as f64 * 10.0), 480, false);
        }
        r.wrote(ms(200.0), 96_000, true);
        for k in 21..40 {
            r.wrote(ms(k as f64 * 10.0), 480, false);
        }
        assert!((held_ms(&r) - 10.0).abs() < 0.1, "{}", held_ms(&r));
    }

    #[test]
    fn a_writes_own_lateness_is_apart_from_the_windows() {
        // 10 ms writes, one held back 250 ms (and the ones due meanwhile
        // with it), one 30 ms late a second later: the window's lateness is
        // the stall's, and a write to come 30 ms late is 30 ms late on its
        // own, one on time not at all.
        let f = Format::new(RATE, 2).unwrap();
        let mut r = rhythm(RATE);
        let mut at = 0.0f64;
        for k in 0..200 {
            let late = match k {
                50 => 250.0,
                150 => 30.0,
                _ => 0.0,
            };
            at = at.max(k as f64 * 10.0 + late);
            if k == 50 {
                assert_eq!(r.late_by(ms(at)), f.frames(ms(250.0)));
            }
            r.wrote(ms(at), 480, false);
        }
        assert_eq!(r.lateness(), f.frames(ms(250.0)));
        assert_eq!(r.late_by(ms(2000.0)), 0);
        assert_eq!(r.late_by(ms(2030.0)), f.frames(ms(30.0)));
    }
}
