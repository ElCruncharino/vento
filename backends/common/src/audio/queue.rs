// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sound between a protocol's writes and a device's reads, and the
//! policy over it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::resample::Resampler;
use super::{Controls, Format, LIMIT, MARGIN, TARGET};

/// What a queue has done so far, for a log line, a test and an overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Frames written.
    pub written: u64,
    /// Frames dropped for having been queued past [`LIMIT`].
    pub dropped: u64,
    /// Times the device found the queue empty mid-stream and played silence
    /// until it had its share of [`TARGET`] again. The end of every stream is
    /// one.
    pub underruns: u64,
}

/// One stream's samples, written by the protocol and taken by the device.
///
/// A lock rather than a lock-free ring, because what the policy needs —
/// dropping the oldest when the writer has got too far ahead — is the writer
/// moving the reader's end, which a single-producer ring does not allow.
/// The lock is held for a copy of one write or one device buffer and
/// nothing else, never across a call into the device or the sound server,
/// so neither side waits longer than that copy for the other; the gain is
/// applied after it is let go.
pub struct Queue {
    format: Format,
    controls: Arc<Controls>,
    /// The stream's own gain, an `f32`'s bits.
    gain: AtomicU32,
    /// The device's buffer and the most it takes at once, in microseconds:
    /// see [`Queue::set_device`].
    buffer: AtomicU64,
    period: AtomicU64,
    inner: Mutex<Inner>,
}

struct Inner {
    samples: VecDeque<i16>,
    /// Playing, rather than waiting for the queue's share of [`TARGET`] to
    /// be queued.
    primed: bool,
    /// Device frames of silence played since the last write while waiting
    /// with something queued: a stream's tail that is shorter than the
    /// target, which no more writes will make longer, is played once it has
    /// waited a target's worth rather than never. Since the last write, so
    /// that a stream still arriving waits for the whole target: counted
    /// across writes it started short of it, wherever the two happened to
    /// cross, and played that much closer to running dry for as long as it
    /// ran.
    waited: usize,
    /// The largest write of late, in frames: how the far end sends.
    chunk: usize,
    /// Device frames taken since the stream opened: until they are the
    /// device's buffer, the device is still filling it.
    pulled: usize,
    resampler: Resampler,
    stats: Stats,
}

impl Queue {
    pub fn new(format: Format, controls: Arc<Controls>) -> Queue {
        Queue {
            format,
            controls,
            gain: AtomicU32::new(1.0f32.to_bits()),
            buffer: AtomicU64::new(0),
            period: AtomicU64::new(0),
            inner: Mutex::new(Inner {
                // Room for the limit and a write past it, allocated once.
                samples: VecDeque::with_capacity(format.frames(LIMIT * 2) * format.channels),
                primed: false,
                waited: 0,
                chunk: 0,
                pulled: 0,
                resampler: Resampler::new(),
                stats: Stats::default(),
            }),
        }
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn controls(&self) -> &Arc<Controls> {
        &self.controls
    }

    /// The stream's own gain: see [`Stream::set_gain`](super::Stream::set_gain).
    pub fn set_gain(&self, gain: f32) {
        let gain = if gain.is_finite() { gain.clamp(0.0, 4.0) } else { 1.0 };
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// What the device holds once it is running, `buffer` — which a device
    /// that is written to as soon as it has room keeps full — and the most
    /// it takes from the queue at once, `period`. The platform says them
    /// when the device has opened, before it first pulls; until then the
    /// queue plays as though the device held nothing and took a sample at a
    /// time.
    ///
    /// The queue then keeps [`TARGET`] less the buffer, so that what is
    /// heard is the target behind what arrived and not the target plus
    /// whatever the device adds; and drops the oldest past [`LIMIT`] less
    /// the buffer. See [`Queue::target`] for the least it keeps.
    pub fn set_device(&self, buffer: Duration, period: Duration) {
        let micros = |d: Duration| d.as_micros().min(u64::MAX as u128) as u64;
        self.buffer.store(micros(buffer), Ordering::Relaxed);
        self.period.store(micros(period), Ordering::Relaxed);
    }

    /// The queue's share of [`TARGET`], in frames of the stream: the target
    /// less the device's buffer, but never less than what keeps the queue
    /// from running dry between the far end's writes — a period of the
    /// device's, which it takes at once, two of the largest recent writes,
    /// and [`MARGIN`] — and never more than the target itself.
    ///
    /// Two writes because the queue is at its lowest just before one lands,
    /// a whole write after the last, and a far end that sends in chunks
    /// sends one late now and then: RDP's 23 ms waves arrive 18 to 41 ms
    /// apart, where QEMU's 10 ms of audio keeps to within a few. Measured
    /// rather than assumed, since a fixed floor for RDP's waves would hold
    /// QEMU's sound further behind than it needs, and one for QEMU's ran
    /// RDP's queue dry on the rig. The writes before the queue first fills
    /// are what it is set by, so the level is right from the first sample
    /// played.
    fn target(&self, inner: &Inner) -> usize {
        let buffer = Duration::from_micros(self.buffer.load(Ordering::Relaxed));
        let period = self.format.frames(Duration::from_micros(self.period.load(Ordering::Relaxed)));
        let least = period + 2 * inner.chunk + self.format.frames(MARGIN);
        let target = self.format.frames(TARGET);
        self.format.frames(TARGET.saturating_sub(buffer)).max(least).min(target)
    }

    /// The queue's share of [`LIMIT`], in frames of the stream: never so
    /// little that a burst of the target's size is a drop, whatever the
    /// device holds.
    fn limit(&self, inner: &Inner) -> usize {
        let buffer = Duration::from_micros(self.buffer.load(Ordering::Relaxed));
        self.format.frames(LIMIT.saturating_sub(buffer)).max(self.target(inner) * 2)
    }

    /// How long what is queued lasts.
    pub fn queued(&self) -> Duration {
        let samples = self.inner.lock().unwrap().samples.len();
        self.format.duration(samples / self.format.channels)
    }

    pub fn stats(&self) -> Stats {
        self.inner.lock().unwrap().stats
    }

    /// Whole frames, as samples; a trailing part of a frame is dropped.
    /// Never waits on the device: past its share of [`LIMIT`] the oldest
    /// goes.
    pub fn write(&self, samples: &[i16]) {
        let whole = samples.len() / self.format.channels * self.format.channels;
        let mut inner = self.inner.lock().unwrap();
        inner.samples.extend(&samples[..whole]);
        self.wrote(&mut inner, whole);
    }

    /// Whole frames, as native-endian bytes.
    pub fn write_bytes(&self, bytes: &[u8]) {
        let whole = bytes.len() / self.format.frame_bytes() * self.format.frame_bytes();
        let mut inner = self.inner.lock().unwrap();
        inner
            .samples
            .extend(bytes[..whole].chunks_exact(2).map(|b| i16::from_ne_bytes([b[0], b[1]])));
        self.wrote(&mut inner, whole / 2);
    }

    fn wrote(&self, inner: &mut Inner, samples: usize) {
        inner.waited = 0;
        let frames = samples / self.format.channels;
        inner.stats.written += frames as u64;
        // Trimmed by the writes before this one: a write that takes the
        // queue past its limit is a burst, and is no measure of how far
        // ahead the queue has to keep.
        if inner.samples.len() > self.limit(inner) * self.format.channels {
            self.trim(inner);
        }
        // A write larger than the ones before is how the far end sends from
        // now on; one that was a burst (a stall's backlog) is forgotten over
        // the next few dozen writes, and the target with it.
        let decayed = inner.chunk - inner.chunk / 16;
        inner.chunk = decayed.max(frames.min(self.format.frames(TARGET)));
    }

    /// Drops the oldest down to the queue's share of [`TARGET`]. A skip is
    /// heard once; a queue that only ever grew would be heard further behind
    /// the picture for the rest of the session.
    fn trim(&self, inner: &mut Inner) {
        let keep = self.target(inner) * self.format.channels;
        if inner.samples.len() > keep {
            let drop = inner.samples.len() - keep;
            inner.samples.drain(..drop);
            inner.stats.dropped += (drop / self.format.channels) as u64;
            // The frames the resampler holds were next to what went.
            inner.resampler.reset();
        }
    }

    /// Fills `out`, frames of `device` — the format the device opened at,
    /// which the stream is converted to where it differs — with what is
    /// queued, gained, or silence. The device's call, on its own thread;
    /// never waits on anything but the copy.
    pub fn pull(&self, out: &mut [i16], device: Format) {
        let dch = device.channels;
        let frames = out.len() / dch;
        let out = &mut out[..frames * dch];
        let filled = {
            let mut inner = self.inner.lock().unwrap();
            let inner = &mut *inner;
            // A device that has just opened takes its whole buffer at once,
            // which the target counts on being full already: it is filled
            // with silence first, rather than with the start of the sound,
            // which would leave the queue that much short of the target.
            let buffer = device.frames(Duration::from_micros(self.buffer.load(Ordering::Relaxed)));
            let filling = inner.pulled < buffer;
            inner.pulled = inner.pulled.saturating_add(frames);
            if !inner.primed && !filling {
                let queued = inner.samples.len() / self.format.channels;
                let target = self.target(inner);
                if queued >= target || (queued > 0 && inner.waited >= device.frames(self.format.duration(target))) {
                    // Whatever arrived while nothing was playing — a device
                    // that took a while to open, a burst after a stall —
                    // starts at the target and not behind it.
                    self.trim(inner);
                    inner.primed = true;
                    inner.waited = 0;
                    inner.resampler.reset();
                } else if queued > 0 {
                    inner.waited += frames;
                }
            }
            if inner.primed {
                let n = inner.resampler.run(&mut inner.samples, self.format, device, out);
                if n < frames {
                    inner.primed = false;
                    inner.waited = 0;
                    inner.stats.underruns += 1;
                }
                n
            } else {
                0
            }
        };
        out[filled * dch..].fill(0);
        let factor = self.controls.factor(f32::from_bits(self.gain.load(Ordering::Relaxed)));
        gain(&mut out[..filled * dch], factor);
    }

    /// Everything queued, for a test to compare.
    #[cfg(test)]
    pub(super) fn take_all(&self) -> Vec<i16> {
        self.inner.lock().unwrap().samples.drain(..).collect()
    }
}

/// Scales `samples` by `factor`, clipping: in 16.16 fixed point, which is
/// finer than a 16-bit sample can show.
fn gain(samples: &mut [i16], factor: f32) {
    if factor == 1.0 {
        return;
    }
    if factor <= 0.0 {
        samples.fill(0);
        return;
    }
    let f = (factor * 65536.0) as i64;
    for s in samples {
        *s = ((*s as i64 * f) >> 16).clamp(i16::MIN as i64, i16::MAX as i64) as i16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEREO: Format = Format {
        rate: 48_000,
        channels: 2,
    };

    fn queue() -> Queue {
        Queue::new(STEREO, Arc::new(Controls::new()))
    }

    /// `ms` of stereo frames, each frame's samples its index from `from`.
    fn frames(from: i16, ms: usize) -> Vec<i16> {
        let n = STEREO.frames(Duration::from_millis(ms as u64));
        (0..n).flat_map(|i| {
            let v = from.wrapping_add(i as i16);
            [v, v]
        }).collect()
    }

    /// One 10 ms device buffer.
    fn pull(q: &Queue) -> Vec<i16> {
        let mut out = vec![-1i16; 480 * 2];
        q.pull(&mut out, STEREO);
        out
    }

    #[test]
    fn nothing_plays_until_the_target_is_queued() {
        let q = queue();
        q.write(&frames(1, 50));
        assert!(pull(&q).iter().all(|&s| s == 0), "silence while priming");
        q.write(&frames(2401, 50));
        assert_eq!(q.queued(), TARGET);
        let out = pull(&q);
        assert_eq!(&out[..4], &[1, 1, 2, 2], "then from the first frame written");
        assert_eq!(q.queued(), Duration::from_millis(90));
    }

    #[test]
    fn a_tail_shorter_than_the_target_still_plays() {
        let q = queue();
        q.write(&frames(1, 30));
        // Ten buffers of silence is a target's worth of waiting.
        for _ in 0..10 {
            assert!(pull(&q).iter().all(|&s| s == 0));
        }
        assert_eq!(&pull(&q)[..2], &[1, 1]);
    }

    #[test]
    fn running_dry_is_silence_and_then_the_target_again() {
        let q = queue();
        q.write(&frames(1, 100));
        for _ in 0..10 {
            pull(&q);
        }
        assert_eq!(q.stats().underruns, 0);
        // 5 ms more: half a buffer of sound, half of silence.
        q.write(&frames(1, 5));
        let out = pull(&q);
        assert_ne!(out[0], 0);
        assert!(out[240 * 2..].iter().all(|&s| s == 0));
        assert_eq!(q.stats().underruns, 1);
        // Less than the target is not enough to start again...
        q.write(&frames(100, 60));
        assert!(pull(&q).iter().all(|&s| s == 0));
        // ...and the target is.
        q.write(&frames(100, 40));
        assert_eq!(pull(&q)[0], 100);
        assert_eq!(q.stats().underruns, 1);
    }

    #[test]
    fn past_the_limit_the_oldest_goes_down_to_the_target() {
        let q = queue();
        // A burst of two seconds at once: what is kept is the newest tenth
        // of a second, and the queue never holds more than the limit.
        let burst = frames(0, 2000);
        q.write(&burst);
        assert_eq!(q.queued(), TARGET);
        let stats = q.stats();
        assert_eq!(stats.written, 96_000);
        assert_eq!(stats.dropped, 96_000 - 4800);
        assert_eq!(pull(&q)[0], burst[burst.len() - 4800 * 2]);
        // Steady writes a little ahead of the device: bounded, however long.
        for _ in 0..1000 {
            q.write(&frames(0, 11));
            pull(&q);
            assert!(q.queued() <= LIMIT);
        }
        assert!(q.stats().dropped > stats.dropped);
    }

    #[test]
    fn what_arrived_before_the_device_started_is_cut_to_the_target() {
        let q = queue();
        q.write(&frames(0, 240));
        assert_eq!(q.queued(), Duration::from_millis(240));
        pull(&q);
        assert_eq!(q.queued(), Duration::from_millis(90));
    }

    /// `ms` of frames, written 10 ms at a time as QEMU's audio arrives.
    fn write_steady(q: &Queue, from: i16, ms: usize) {
        for i in 0..ms / 10 {
            q.write(&frames(from.wrapping_add(i as i16 * 480), 10));
        }
    }

    /// A device opened with `buffer` and `period` that has filled its buffer
    /// with silence, 10 ms at a time.
    fn open(q: &Queue, buffer: u64, period_us: u64) {
        q.set_device(Duration::from_millis(buffer), Duration::from_micros(period_us));
        for _ in 0..buffer / 10 {
            assert!(pull(q).iter().all(|&s| s == 0));
        }
    }

    #[test]
    fn a_device_that_just_opened_is_filled_with_silence_first() {
        let q = queue();
        q.set_device(Duration::from_millis(50), Duration::ZERO);
        write_steady(&q, 0, 200);
        for _ in 0..5 {
            assert!(pull(&q).iter().all(|&s| s == 0));
        }
        assert_eq!(q.queued(), Duration::from_millis(200), "kept while it fills");
        // Then the newest of what arrived meanwhile, to the target.
        assert_ne!(pull(&q)[0], 0);
        assert_eq!(q.queued(), Duration::from_millis(40));
    }

    #[test]
    fn the_device_buffer_is_part_of_the_target() {
        // ALSA's 50 ms and 12.5 ms periods, and a far end writing 10 ms at a
        // time: the queue starts at 50 and is trimmed back to it.
        let q = queue();
        open(&q, 50, 12_500);
        write_steady(&q, 1, 40);
        assert!(pull(&q).iter().all(|&s| s == 0), "silence while priming");
        write_steady(&q, 1921, 10);
        assert_eq!(pull(&q)[0], 1, "50 ms queued is enough to start");
        assert_eq!(q.queued(), Duration::from_millis(40));
        // Its share of the limit is 200 ms, and a burst past it is cut back
        // to its share of the target.
        write_steady(&q, 0, 160);
        assert_eq!(q.queued(), Duration::from_millis(200));
        assert_eq!(q.stats().dropped, 0);
        q.write(&frames(0, 10));
        assert_eq!(q.queued(), Duration::from_millis(50));
    }

    #[test]
    fn the_least_kept_follows_how_the_far_end_writes() {
        // RDP's 23 ms waves in front of the same device: a period, two
        // waves and the margin, 69 ms, rather than the 50 the device leaves.
        let wave = 1024 * 48 / 44; // 23.3 ms at 48 kHz, near enough
        let least = 600 + 2 * wave + STEREO.frames(MARGIN);
        let q = queue();
        open(&q, 50, 12_500);
        let mut written = 0;
        while written + wave < least {
            q.write(&vec![1i16; wave * 2]);
            written += wave;
            assert!(pull(&q).iter().all(|&s| s == 0), "{written} frames are not enough");
        }
        q.write(&vec![1i16; wave * 2]);
        assert_eq!(pull(&q)[0], 1);
        assert_eq!(q.queued(), STEREO.duration(least - 480));

        // A device that holds nearly the target still has that much in
        // front of it, but no more than the target: AAudio's 80 ms buffer
        // and 40 ms bursts.
        let q = queue();
        open(&q, 80, 40_000);
        write_steady(&q, 1, 60);
        assert!(pull(&q).iter().all(|&s| s == 0));
        write_steady(&q, 1, 10);
        assert_ne!(pull(&q)[0], 0, "a burst, two writes and the margin start it");

        // A burst raises the least kept for a few dozen writes and no more.
        let q = queue();
        q.write(&frames(0, 200));
        q.write(&frames(0, 200));
        assert_eq!(q.queued(), TARGET, "the least kept is never past the target");
        q.set_device(Duration::from_millis(50), Duration::ZERO);
        assert_eq!(q.target(&q.inner.lock().unwrap()), STEREO.frames(TARGET));
        for _ in 0..80 {
            q.write(&frames(0, 10));
            pull(&q);
        }
        assert_eq!(q.target(&q.inner.lock().unwrap()), STEREO.frames(Duration::from_millis(50)));
    }

    #[test]
    fn a_stream_still_arriving_waits_for_the_whole_target() {
        // 5 ms written for each 10 ms pulled: the queue grows slower than
        // the waiting, and is still only started once it has the target.
        let q = queue();
        for i in 0..19 {
            q.write(&frames(i * 240, 5));
            assert!(pull(&q).iter().all(|&s| s == 0), "pull {i}");
        }
        q.write(&frames(0, 5));
        assert_eq!(q.queued(), TARGET);
        assert_ne!(pull(&q)[2], 0);
    }

    #[test]
    fn gain_and_mute_apply_to_what_the_device_takes() {
        let controls = Arc::new(Controls::new());
        let q = Queue::new(STEREO, Arc::clone(&controls));
        q.write(&[20_000i16; 4800 * 2]);
        assert!(pull(&q).iter().all(|&s| s == 20_000));
        controls.set_gain(0.5);
        assert!(pull(&q).iter().all(|&s| s == 10_000));
        q.set_gain(0.5);
        assert!(pull(&q).iter().all(|&s| s == 5_000));
        controls.set_muted(true);
        assert!(pull(&q).iter().all(|&s| s == 0));
        // Under the mute the stream drains as it would have played.
        assert_eq!(q.queued(), Duration::from_millis(60));
        controls.set_muted(false);
        controls.set_gain(4.0);
        q.set_gain(1.0);
        assert!(pull(&q).iter().all(|&s| s == i16::MAX), "clipped, not wrapped");
    }

    #[test]
    fn another_device_rate_and_layout_are_converted() {
        let q = Queue::new(Format::new(44_100, 1).unwrap(), Arc::new(Controls::new()));
        q.write(&vec![1000i16; 4410]);
        let mut out = vec![0i16; 480 * 2];
        q.pull(&mut out, STEREO);
        assert!(out.iter().all(|&s| s == 1000));
        // 480 frames at 48 kHz are 441 at 44.1, give or take the one the
        // conversion holds.
        let left = q.queued().as_micros() as i64;
        assert!((left - 90_000).abs() <= 50, "{left}");
    }
}
