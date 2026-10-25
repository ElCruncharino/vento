// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sound between a protocol's writes and a device's reads, and the
//! policy over it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::resample::Resampler;
use super::{Controls, Format, LIMIT, TARGET};

/// What a queue has done so far, for a log line, a test and an overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Frames written.
    pub written: u64,
    /// Frames dropped for having been queued past [`LIMIT`].
    pub dropped: u64,
    /// Times the device found the queue empty mid-stream and played silence
    /// until it had [`TARGET`] again. The end of every stream is one.
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
    inner: Mutex<Inner>,
}

struct Inner {
    samples: VecDeque<i16>,
    /// Playing, rather than waiting for [`TARGET`] to be queued.
    primed: bool,
    /// Device frames of silence played while waiting with something queued:
    /// a stream's tail that is shorter than the target, which no more
    /// writes will make longer, is played once it has waited a target's
    /// worth rather than never.
    waited: usize,
    resampler: Resampler,
    stats: Stats,
}

impl Queue {
    pub fn new(format: Format, controls: Arc<Controls>) -> Queue {
        Queue {
            format,
            controls,
            gain: AtomicU32::new(1.0f32.to_bits()),
            inner: Mutex::new(Inner {
                // Room for the limit and a write past it, allocated once.
                samples: VecDeque::with_capacity(format.frames(LIMIT * 2) * format.channels),
                primed: false,
                waited: 0,
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

    /// How long what is queued lasts.
    pub fn queued(&self) -> Duration {
        let samples = self.inner.lock().unwrap().samples.len();
        self.format.duration(samples / self.format.channels)
    }

    pub fn stats(&self) -> Stats {
        self.inner.lock().unwrap().stats
    }

    /// Whole frames, as samples; a trailing part of a frame is dropped.
    /// Never waits on the device: past [`LIMIT`] the oldest goes.
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
        inner.stats.written += (samples / self.format.channels) as u64;
        if inner.samples.len() > self.format.frames(LIMIT) * self.format.channels {
            self.trim(inner);
        }
    }

    /// Drops the oldest down to [`TARGET`]. A skip is heard once; a queue
    /// that only ever grew would be heard further behind the picture for
    /// the rest of the session.
    fn trim(&self, inner: &mut Inner) {
        let keep = self.format.frames(TARGET) * self.format.channels;
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
            if !inner.primed {
                let queued = inner.samples.len() / self.format.channels;
                if queued >= self.format.frames(TARGET) || (queued > 0 && inner.waited >= device.frames(TARGET)) {
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
