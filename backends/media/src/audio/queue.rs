// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sound between a protocol's writes and a device's reads, and the
//! policy over it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::resample::Resampler;
use super::rhythm::{Rhythm, WINDOW};
use super::{CLEAN, CLEAN_START, Controls, Format, LIMIT, MARGIN, MARGIN_MAX, MARGIN_START, MARGIN_STEP};

/// How long a queue that ran dry may stay dry with the far end's sound
/// still to come: sound that arrives again sooner was late, and the session
/// remembers how late ([`Controls::late`]); sound that arrives later is the
/// far end's next sound after a pause of its own — the end of a
/// notification, a guest that stopped playing — which no margin is for.
const RESUME: Duration = Duration::from_millis(500);

/// How much more than its margin a queue has to have kept, at its least,
/// for a whole `WINDOW` before the excess is dropped: a skip, if a short
/// and faded one, so not for every millisecond.
const SETTLE: Duration = Duration::from_millis(5);

/// How long a skip is faded over, out of what would have played next and
/// into what plays instead: long enough that it is no click, short enough to
/// be one sound's step rather than two sounds over each other.
const FADE: Duration = Duration::from_micros(2500);

/// What a queue has done so far, for a log line, a test and an overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Frames written.
    pub written: u64,
    /// Frames dropped: past [`LIMIT`], beyond the target before the device
    /// first played, or kept beyond what was needed for a whole `WINDOW`
    /// (see [`Queue`]).
    pub dropped: u64,
    /// Times the device found the queue empty mid-stream and played silence
    /// until it had its target again. The end of every stream is one.
    pub underruns: u64,
}

/// One stream's samples, written by the protocol and taken by the device,
/// and how many of them it keeps in front of the device: its **target**.
///
/// The target is the least that keeps the device from finding the queue
/// empty as the far end's sound arrives — a period of the device's, which
/// it takes at once ([`Queue::set_device`]), and what the far end's rhythm
/// has needed of late, its largest write or bunch of writes and how late
/// they come (`Rhythm`) — and a **margin** on top, for lateness the
/// rhythm has not shown lately: [`MARGIN`], and as much of the lateness the
/// session remembers ([`Controls::late`]) as the rhythm does not already
/// cover, or, where it is more, what is left of the stream's start: a
/// stream's margin starts at [`MARGIN_START`] and comes down a
/// [`MARGIN_STEP`] after every [`CLEAN_START`] of sound that did not run dry.
/// The device's own buffer is behind the queue and comes on top of all of
/// it: what is heard is the target and the device's buffer behind what
/// arrived.
///
/// The queue starts playing once it has the target, and plays silence when
/// it runs dry until it has it again. Where it ran dry with the far end's
/// sound arriving again within `RESUME` — the sound was later than
/// anything the rhythm had shown — the session remembers that lateness, so
/// that the next time it is not too late, and forgets it again slowly after
/// a long while of sound that did not run dry; and the queue starts again
/// as soon as what it has lasts, with the period and the margin, until the
/// far end's next write if that is no later against its sound than this one
/// was (`Rhythm::keep`), rather than once it has the whole target, which
/// counts the lateness from the earliest write of the window on (for
/// Windows' PCM, whose next pair is 372 ms away, the difference between a
/// gap of the lateness and one of the pair); unless it was later than
/// [`MARGIN_MAX`](super::MARGIN_MAX), a stall, after which the rhythm starts
/// a new run, what the far end sends to catch up counts as a burst, and the
/// queue starts again only once it has caught up, at its target, the rest
/// of the backlog dropped while the device was still playing silence. Where the queue has kept more
/// than its margin beyond what it needed for a whole `WINDOW` — it
/// started on a write that arrived late, the lateness remembered was
/// forgotten, a late write the rhythm remembered has been, the far end's
/// clock runs faster than the device's — the excess is dropped at once,
/// faded over `FADE`, so that the sound is no further behind than it has
/// to be.
///
/// A lock rather than a lock-free ring, because what the policy needs —
/// dropping the oldest when the writer has got too far ahead — is the writer
/// moving the reader's end, which a single-producer ring does not allow.
/// The lock is held for a copy of one write or one device buffer and the
/// bookkeeping of the policy, never across a call into the device or the
/// sound server or a log line (which may block on a pipe or logcat),
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
    /// What the platform calls the output the device plays on, where it has
    /// more than one: see [`Queue::set_output`].
    output: Mutex<Option<&'static str>>,
    /// When the queue was made: what the far end's writes are timed from.
    born: Instant,
    inner: Mutex<Inner>,
}

struct Inner {
    samples: VecDeque<i16>,
    /// Playing, rather than waiting for the target to be queued.
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
    /// How the far end sends.
    rhythm: Rhythm,
    /// Device frames taken since the stream opened: until they are the
    /// device's buffer, the device is still filling it.
    pulled: usize,
    /// The least queued just before a write while playing, in frames, since
    /// `since` (`None` until the first such write): what the queue turned out
    /// not to need of what it kept.
    low: usize,
    since: Option<Duration>,
    /// How long the device has played silence since the queue ran dry
    /// while playing, and how much of the pull that found it dry was
    /// silence: how late the far end's sound is, so far.
    dry: Option<Duration>,
    /// Frames written while playing since the queue last ran dry.
    clean: usize,
    /// What is left of the start's margin beyond [`MARGIN`], and the
    /// frames written while playing since it last came down or the queue
    /// last ran dry.
    start: Duration,
    fresh: usize,
    /// When the last write of the far end's catching up after a stall
    /// arrived, while it is: see `wrote`.
    catching: Option<Duration>,
    /// While the queue starts again after running dry on a late write, what
    /// it has to have beyond the period and the margin as of the last write
    /// (`Rhythm::keep`), in frames of the stream: see `pull`.
    resume: Option<usize>,
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
            output: Mutex::new(None),
            born: Instant::now(),
            inner: Mutex::new(Inner {
                // Room for the limit and a write past it, allocated once.
                samples: VecDeque::with_capacity(format.frames(LIMIT * 2) * format.channels),
                primed: false,
                waited: 0,
                rhythm: Rhythm::new(format),
                pulled: 0,
                low: usize::MAX,
                since: None,
                dry: None,
                clean: 0,
                start: MARGIN_START.saturating_sub(MARGIN),
                fresh: 0,
                catching: None,
                resume: None,
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
    /// when the device has opened, before it first pulls, and again where
    /// they change (a PipeWire graph's cycle does); until then the queue
    /// plays as though the device held nothing and took a sample at a time.
    ///
    /// The period is part of the target: the queue has to have it left
    /// when the device comes for it. The buffer is not — it is behind the
    /// queue, delay on top of the target, and what the overlay and RDP's
    /// confirmations add — but the limit is counted with it ([`LIMIT`]).
    pub fn set_device(&self, buffer: Duration, period: Duration) {
        let micros = |d: Duration| d.as_micros().min(u64::MAX as u128) as u64;
        self.buffer.store(micros(buffer), Ordering::Relaxed);
        self.period.store(micros(period), Ordering::Relaxed);
    }

    /// The target, in frames of the stream: a period of the device's, what
    /// the far end's rhythm has needed of late and the margin.
    ///
    /// Past what a fixed target would keep where the far end makes it:
    /// Windows sends PCM in 186 ms waves, two at once every 372 ms, whatever
    /// the client confirms, which a queue that kept 100 ms ran dry from and
    /// dropped most of. Such a stream is heard as far behind as its rhythm
    /// makes it, rather than in pieces.
    fn target(&self, inner: &Inner) -> usize {
        self.period() + inner.rhythm.hold() + self.margin(inner)
    }

    /// The device's period, in frames of the stream.
    fn period(&self) -> usize {
        self.format.frames(Duration::from_micros(self.period.load(Ordering::Relaxed)))
    }

    /// The margin, in frames of the stream: [`MARGIN`], and what the session
    /// remembers of lateness beyond what the rhythm has seen of late or what
    /// is left of the stream's start, whichever is more.
    fn margin(&self, inner: &Inner) -> usize {
        let late = self.format.frames(self.controls.late()).saturating_sub(inner.rhythm.lateness());
        self.format.frames(MARGIN) + late.max(self.format.frames(inner.start))
    }

    /// The queue's share of [`LIMIT`], in frames of the stream: never so
    /// little that a burst of the target's size is a drop, whatever the
    /// device holds.
    fn limit(&self, inner: &Inner) -> usize {
        let buffer = Duration::from_micros(self.buffer.load(Ordering::Relaxed));
        self.format.frames(LIMIT.saturating_sub(buffer)).max(self.target(inner) * 2)
    }

    /// The target as it is now, for the overlay: see [`Queue`].
    pub fn target_now(&self) -> Duration {
        let inner = self.inner.lock().unwrap();
        self.format.duration(self.target(&inner))
    }

    /// The margin in the target as it is now, for the overlay.
    pub fn margin_now(&self) -> Duration {
        let inner = self.inner.lock().unwrap();
        self.format.duration(self.margin(&inner))
    }

    /// Which of the platform's outputs the device plays on — the desktop's
    /// PipeWire stream, or ALSA's device where there is no PipeWire — for
    /// the facts row and the overlay to say. A platform with one way out
    /// (AAudio) need not say; a device that falls back to another output
    /// mid-stream says it again.
    pub fn set_output(&self, output: &'static str) {
        *self.output.lock().unwrap() = Some(output);
    }

    /// The output [`set_output`](Queue::set_output) was told, if any.
    pub fn output(&self) -> Option<&'static str> {
        *self.output.lock().unwrap()
    }

    /// The device's buffer, as [`set_device`](Queue::set_device) was told
    /// it; zero before the device has opened.
    pub fn device_buffer(&self) -> Duration {
        Duration::from_micros(self.buffer.load(Ordering::Relaxed))
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
        self.write_at(samples, self.born.elapsed());
    }

    fn write_at(&self, samples: &[i16], at: Duration) {
        let whole = samples.len() / self.format.channels * self.format.channels;
        let notes = {
            let mut inner = self.inner.lock().unwrap();
            let before = inner.samples.len();
            inner.samples.extend(&samples[..whole]);
            self.wrote(&mut inner, before, whole, at)
        };
        notes.log();
    }

    /// Whole frames, as native-endian bytes.
    pub fn write_bytes(&self, bytes: &[u8]) {
        let at = self.born.elapsed();
        let whole = bytes.len() / self.format.frame_bytes() * self.format.frame_bytes();
        let notes = {
            let mut inner = self.inner.lock().unwrap();
            let before = inner.samples.len();
            inner
                .samples
                .extend(bytes[..whole].chunks_exact(2).map(|b| i16::from_ne_bytes([b[0], b[1]])));
            self.wrote(&mut inner, before, whole / 2, at)
        };
        notes.log();
    }

    /// `samples` more were queued at `at`, on top of `before`. What is worth
    /// a log line is returned to be logged once the lock is let go, since
    /// the device's thread waits on it to pull.
    fn wrote(&self, inner: &mut Inner, before: usize, samples: usize, at: Duration) -> Notes {
        let mut notes = Notes::default();
        inner.waited = 0;
        let frames = samples / self.format.channels;
        inner.stats.written += frames as u64;
        // One past the limit is a burst (a stall's backlog, all at once), no
        // rhythm the queue could keep up with; and so is a write that comes
        // faster than the far end sends while it catches up after a stall.
        let mut burst = frames > self.format.frames(LIMIT);
        if let Some(last) = inner.catching {
            if at.saturating_sub(last) < self.format.duration(frames) / 2 {
                burst = true;
                inner.catching = Some(at);
            } else {
                inner.catching = None;
            }
        }
        match inner.dry.take() {
            Some(dry) if dry < RESUME => {
                inner.clean = 0;
                inner.fresh = 0;
                let late = self.format.duration(inner.rhythm.late_by(at));
                if late > MARGIN_MAX {
                    // Later than any margin is for: a stall (a machine that
                    // lost the processor, a network that paused), which is
                    // neither remembered nor measured. The far end's sound
                    // starts a new run of the rhythm here, what it sends to
                    // catch up is a burst, and the queue starts again only
                    // once it has, cut to its target in one skip.
                    inner.rhythm.restart();
                    inner.rhythm.wrote(at, frames, true);
                    inner.catching = Some(at);
                    inner.resume = None;
                    notes.dry = Some((dry, late, None));
                } else {
                    // Later than the queue kept for: measured as the late
                    // write it is, and remembered; and the queue starts
                    // again from it.
                    inner.rhythm.wrote(at, frames, burst);
                    let seen = self.format.duration(inner.rhythm.lateness());
                    let late = self.controls.ran_dry(seen);
                    inner.resume = Some(inner.rhythm.keep(at));
                    notes.dry = Some((dry, seen, Some(late)));
                }
            }
            Some(_) => {
                // A new sound after a pause of the far end's own.
                inner.rhythm.restart();
                inner.rhythm.wrote(at, frames, burst);
                inner.resume = None;
            }
            None => {
                inner.rhythm.wrote(at, frames, burst);
                if inner.resume.is_some() {
                    inner.resume = Some(inner.rhythm.keep(at));
                }
            }
        }
        if inner.primed {
            let since = *inner.since.get_or_insert(at);
            inner.low = inner.low.min(before / self.format.channels);
            inner.clean += frames;
            inner.fresh += frames;
            if !inner.start.is_zero() && self.format.duration(inner.fresh) >= CLEAN_START {
                inner.fresh = 0;
                inner.start = inner.start.saturating_sub(MARGIN_STEP);
                notes.margin = Some(self.format.duration(self.margin(inner)));
            }
            if self.format.duration(inner.clean) >= CLEAN {
                inner.clean = 0;
                notes.late = self.controls.clean();
            }
            if at.saturating_sub(since) >= WINDOW {
                notes.settled = self.settle(inner);
            }
        }
        if inner.samples.len() > self.limit(inner) * self.format.channels {
            self.trim(inner);
        }
        notes
    }

    /// Drops what the queue kept beyond the target's margin for a whole
    /// window — the least it held before a write, less the margin — where
    /// that is worth a skip, and starts the next window. What was dropped,
    /// if anything, for a log line.
    fn settle(&self, inner: &mut Inner) -> Option<Duration> {
        let margin = self.margin(inner);
        let excess = inner.low.saturating_sub(margin);
        let dropped = (excess >= self.format.frames(SETTLE)).then(|| {
            self.skip(inner, excess);
            self.format.duration(excess)
        });
        inner.low = usize::MAX;
        inner.since = None;
        dropped
    }

    /// Drops the oldest down to the target. A skip is heard once; a queue
    /// that only ever grew would be heard further behind the picture for the
    /// rest of the session.
    fn trim(&self, inner: &mut Inner) {
        let keep = self.target(inner);
        let queued = inner.samples.len() / self.format.channels;
        if queued > keep {
            self.skip(inner, queued - keep);
        }
        inner.low = usize::MAX;
        inner.since = None;
    }

    /// Drops the oldest `frames`, fading what would have played next into
    /// what plays instead over [`FADE`] (or as much as is left).
    fn skip(&self, inner: &mut Inner, frames: usize) {
        let ch = self.format.channels;
        let queued = inner.samples.len() / ch;
        let frames = frames.min(queued);
        let fade = self.format.frames(FADE).min(frames).min(queued - frames);
        let drop = frames * ch;
        for i in 0..fade {
            // From all of the old to all of the new, never quite either.
            let w = ((i + 1) << 16) as i64 / (fade + 1) as i64;
            for c in 0..ch {
                let k = i * ch + c;
                let old = inner.samples[k] as i64;
                let new = inner.samples[drop + k] as i64;
                inner.samples[drop + k] = ((old * ((1 << 16) - w) + new * w) >> 16) as i16;
            }
        }
        inner.samples.drain(..drop);
        inner.stats.dropped += frames as u64;
        // The frames the resampler holds were next to what went.
        inner.resampler.reset();
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
                let mut target = self.target(inner);
                if let Some(keep) = inner.resume {
                    // Starting again after a late write: what lasts until
                    // the next as late as the latest, less what has been
                    // waited since the last write arrived, which the far
                    // end's next write is that much nearer.
                    let waited = self.format.frames(device.duration(inner.waited));
                    target = target.min(self.period() + self.margin(inner) + keep.saturating_sub(waited));
                }
                let caught_up = inner.catching.is_none();
                if (queued >= target && caught_up)
                    || (queued > 0 && inner.waited >= device.frames(self.format.duration(target)))
                {
                    // Whatever arrived while nothing was playing — a device
                    // that took a while to open, a burst after a stall —
                    // starts at the target and not behind it.
                    self.trim(inner);
                    inner.primed = true;
                    inner.waited = 0;
                    inner.resume = None;
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
                    inner.dry = Some(device.duration(frames - n));
                }
                n
            } else {
                // Counted only as far as it matters: up to the point where
                // what arrives next is no longer late but the next sound.
                if let Some(dry) = inner.dry.as_mut()
                    && *dry < RESUME
                {
                    *dry += device.duration(frames);
                }
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

/// What a write did that is worth a log line, logged by [`Notes::log`] once
/// the queue's lock is let go.
#[derive(Default)]
struct Notes {
    /// The queue had run dry: for how long, how late the write was, and what
    /// the session allows for now — `None` for a stall, which it does not.
    dry: Option<(Duration, Duration, Option<Duration>)>,
    /// The stream's margin came down a step, to this.
    margin: Option<Duration>,
    /// The session forgot a step of lateness, down to this.
    late: Option<Duration>,
    /// The queue dropped this much it had kept beyond its margin.
    settled: Option<Duration>,
}

impl Notes {
    fn log(self) {
        match self.dry {
            Some((dry, late, None)) => log::info!(
                "audio: the far end's sound arrived {} ms after the queue ran dry, {} ms late: a stall, skipped",
                dry.as_millis(),
                late.as_millis()
            ),
            Some((dry, seen, Some(late))) => log::info!(
                "audio: the far end's sound arrived {} ms after the queue ran dry, {} ms late; the session allows for {} ms now",
                dry.as_millis(),
                seen.as_millis(),
                late.as_millis()
            ),
            None => {}
        }
        if let Some(margin) = self.margin {
            log::debug!(
                "audio: the stream has played {} s without running dry; its margin is {} ms now",
                CLEAN_START.as_secs(),
                margin.as_millis()
            );
        }
        if let Some(late) = self.late {
            log::debug!("audio: no late sound for a while; the session allows for {} ms now", late.as_millis());
        }
        if let Some(excess) = self.settled {
            log::debug!("audio: the queue kept {} ms more than it needed; dropped", excess.as_millis());
        }
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
    use super::super::{CLEAN, MARGIN, MARGIN_MAX, MARGIN_STEP};
    use super::*;

    const STEREO: Format = Format {
        rate: 48_000,
        channels: 2,
    };

    fn queue() -> Queue {
        Queue::new(STEREO, Arc::new(Controls::new()))
    }

    /// A stream past its start: the margin is down to [`MARGIN`].
    fn settled() -> Queue {
        let q = queue();
        q.inner.lock().unwrap().start = Duration::ZERO;
        q
    }

    fn ms(v: f64) -> Duration {
        Duration::from_secs_f64(v / 1000.0)
    }

    /// `ms` of stereo frames, each frame's samples its index from `from`.
    fn frames(from: i16, ms: usize) -> Vec<i16> {
        let n = STEREO.frames(Duration::from_millis(ms as u64));
        (0..n)
            .flat_map(|i| {
                let v = from.wrapping_add(i as i16);
                [v, v]
            })
            .collect()
    }

    /// One 10 ms device buffer.
    fn pull(q: &Queue) -> Vec<i16> {
        let mut out = vec![-1i16; 480 * 2];
        q.pull(&mut out, STEREO);
        out
    }

    fn target(q: &Queue) -> Duration {
        q.target_now()
    }

    #[test]
    fn nothing_plays_until_the_target_is_queued() {
        // No device said: a 10 ms write and the margin.
        let q = settled();
        q.write_at(&frames(1, 10), ms(0.0));
        assert_eq!(target(&q), Duration::from_millis(10) + MARGIN);
        assert!(pull(&q).iter().all(|&s| s == 0), "silence while priming");
        q.write_at(&frames(481, 10), ms(10.0));
        let out = pull(&q);
        assert_eq!(&out[..4], &[1, 1, 2, 2], "then from the first frame written");
        assert_eq!(q.queued(), Duration::from_millis(10));
    }

    #[test]
    fn a_tail_shorter_than_the_target_still_plays() {
        let q = settled();
        q.write(&frames(1, 5));
        // Fifteen milliseconds is the target, and two buffers of silence
        // are a target's worth of waiting.
        for _ in 0..2 {
            assert!(pull(&q).iter().all(|&s| s == 0));
        }
        assert_eq!(&pull(&q)[..2], &[1, 1]);
    }

    #[test]
    fn running_dry_is_silence_and_then_the_target_again() {
        let q = settled();
        q.write_at(&frames(1, 10), ms(0.0));
        q.write_at(&frames(1, 10), ms(10.0));
        pull(&q);
        pull(&q);
        assert_eq!(q.stats().underruns, 0);
        // 5 ms more: half a buffer of sound, half of silence.
        q.write_at(&frames(1, 5), ms(20.0));
        let out = pull(&q);
        assert_ne!(out[0], 0);
        assert!(out[240 * 2..].iter().all(|&s| s == 0));
        assert_eq!(q.stats().underruns, 1);
        // The far end's sound arrives again a buffer later: it was 15 ms
        // late, which the session remembers with a step more.
        assert!(pull(&q).iter().all(|&s| s == 0));
        q.write_at(&frames(100, 10), ms(40.0));
        assert_eq!(q.controls().late(), Duration::from_millis(20));
        // The target is a write, its lateness, and the margin with the step
        // the rhythm does not cover; but the queue starts again from this
        // write, the latest of the window, so what it needs is a write's
        // worth and the margin: 25 ms. Less is not enough...
        assert_eq!(q.margin_now(), MARGIN + MARGIN_STEP);
        assert_eq!(target(&q), Duration::from_millis(40));
        assert!(pull(&q).iter().all(|&s| s == 0));
        q.write_at(&frames(100, 10), ms(50.0));
        assert!(pull(&q).iter().all(|&s| s == 0));
        // ...and that is, without waiting for the whole target.
        q.write_at(&frames(100, 10), ms(60.0));
        assert_ne!(pull(&q)[0], 0);
        assert_eq!(q.stats().underruns, 1);
        assert_eq!(q.stats().dropped, 0);
    }

    #[test]
    fn sound_after_a_pause_of_the_far_ends_is_not_late() {
        let q = settled();
        for k in 0..3 {
            q.write_at(&frames(1, 10), ms(10.0 * k as f64));
        }
        for _ in 0..4 {
            pull(&q);
        }
        assert_eq!(q.stats().underruns, 1, "the end of the sound");
        for _ in 0..60 {
            pull(&q);
        }
        q.write_at(&frames(1, 10), ms(700.0));
        assert_eq!(q.controls().late(), Duration::ZERO, "a new sound, not a late one");
        assert_eq!(target(&q), Duration::from_millis(10) + MARGIN);
    }

    #[test]
    fn past_the_limit_the_oldest_goes_down_to_the_target() {
        let q = settled();
        // A burst of two seconds at once, after the far end's 10 ms writes:
        // what is kept is the newest of it, to the target, and the queue
        // never holds more than the limit.
        q.write_at(&frames(0, 10), ms(0.0));
        let burst = frames(0, 2000);
        q.write_at(&burst, ms(10.0));
        let kept = STEREO.frames(target(&q));
        assert_eq!(q.queued(), STEREO.duration(kept));
        let stats = q.stats();
        assert_eq!(stats.written, 96_480);
        assert_eq!(stats.dropped, 96_480 - kept as u64);
        let fade = STEREO.frames(FADE);
        assert_eq!(pull(&q)[fade * 2], burst[burst.len() - (kept - fade) * 2], "the newest, past the fade");
        // Steady writes a little ahead of the device: bounded, however long.
        for k in 0..1000 {
            q.write_at(&frames(0, 11), ms(20.0 + 10.0 * k as f64));
            pull(&q);
            assert!(q.queued() <= LIMIT);
        }
        assert!(q.stats().dropped > stats.dropped);
    }

    #[test]
    fn what_arrived_before_the_device_started_is_cut_to_the_target() {
        let q = settled();
        for k in 0..24 {
            q.write_at(&frames(0, 10), ms(10.0 * k as f64));
        }
        assert_eq!(q.queued(), Duration::from_millis(240));
        pull(&q);
        assert_eq!(q.queued(), Duration::from_millis(10), "20 ms kept, 10 of it played");
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
        let q = settled();
        q.set_device(Duration::from_millis(50), Duration::ZERO);
        for k in 0..20 {
            q.write_at(&frames(1, 10), ms(10.0 * k as f64));
        }
        for _ in 0..5 {
            assert!(pull(&q).iter().all(|&s| s == 0));
        }
        assert_eq!(q.queued(), Duration::from_millis(200), "kept while it fills");
        // Then the newest of what arrived meanwhile, to the target.
        assert_ne!(pull(&q)[0], 0);
        assert_eq!(q.queued(), Duration::from_millis(10));
    }

    #[test]
    fn the_period_is_part_of_the_target_and_the_buffer_is_not() {
        // ALSA's 50 ms and 12.5 ms periods, and a far end writing 10 ms at a
        // time: a period, a write and the margin.
        let q = settled();
        open(&q, 50, 12_500);
        q.write_at(&frames(1, 10), ms(0.0));
        assert_eq!(target(&q), Duration::from_micros(32_500));
        q.write_at(&frames(1, 10), ms(10.0));
        q.write_at(&frames(1, 10), ms(20.0));
        assert!(pull(&q).iter().all(|&s| s == 0), "silence while priming");
        q.write_at(&frames(1921, 10), ms(30.0));
        assert_ne!(pull(&q)[0], 0, "40 ms queued is enough to start");
        assert_eq!(q.queued(), Duration::from_micros(22_500), "from the target");
        // Its share of the limit is 200 ms, and a burst past it is cut back
        // to the target.
        let dropped = q.stats().dropped;
        for k in 0..17 {
            q.write_at(&frames(0, 10), ms(40.0 + 10.0 * k as f64));
        }
        assert_eq!(q.queued(), Duration::from_micros(192_500));
        assert_eq!(q.stats().dropped, dropped);
        q.write_at(&frames(0, 10), ms(210.0));
        assert_eq!(q.queued(), Duration::from_micros(32_500));
    }

    #[test]
    fn a_stream_still_arriving_waits_for_the_whole_target() {
        // 5 ms written for each 10 ms pulled: the queue grows slower than
        // the waiting, and is never started for having waited a target's
        // worth across writes — only once it has the target, here once the
        // far end has caught up and kept up for a few writes.
        let q = queue();
        for i in 0..19 {
            q.write_at(&frames(i * 240, 5), ms(i as f64 * 10.0));
            assert!(q.queued() < target(&q), "{i}");
            assert!(pull(&q).iter().all(|&s| s == 0), "pull {i}");
        }
        q.write_at(&frames(0, 100), ms(190.0));
        for i in 20..40 {
            q.write_at(&frames(1, 10), ms(i as f64 * 10.0));
            let (queued, target) = (q.queued(), target(&q));
            if pull(&q)[0] != 0 {
                assert!(queued >= target, "{queued:?} < {target:?}");
                assert!(i < 25, "{i}");
                return;
            }
        }
        panic!("never started");
    }

    #[test]
    fn gain_and_mute_apply_to_what_the_device_takes() {
        let controls = Arc::new(Controls::new());
        let q = Queue::new(STEREO, Arc::clone(&controls));
        q.inner.lock().unwrap().start = Duration::ZERO;
        let mut k = 0;
        let mut pull = || {
            q.write_at(&[20_000i16; 480 * 2], ms(10.0 * k as f64));
            k += 1;
            pull(&q)
        };
        pull();
        assert!(pull().iter().all(|&s| s == 20_000));
        controls.set_gain(0.5);
        assert!(pull().iter().all(|&s| s == 10_000));
        q.set_gain(0.5);
        assert!(pull().iter().all(|&s| s == 5_000));
        controls.set_muted(true);
        // Under the mute the stream drains as it would have played.
        let queued = q.queued();
        assert!(pull().iter().all(|&s| s == 0));
        assert_eq!(q.queued(), queued);
        controls.set_muted(false);
        controls.set_gain(4.0);
        q.set_gain(1.0);
        assert!(pull().iter().all(|&s| s == i16::MAX), "clipped, not wrapped");
    }

    #[test]
    fn another_device_rate_and_layout_are_converted() {
        let q = Queue::new(Format::new(44_100, 1).unwrap(), Arc::new(Controls::new()));
        q.inner.lock().unwrap().start = Duration::ZERO;
        for k in 0..10 {
            q.write_at(&[1000i16; 441], ms(10.0 * k as f64));
        }
        let mut out = vec![0i16; 480 * 2];
        q.pull(&mut out, STEREO);
        assert!(out.iter().all(|&s| s == 1000));
        // Trimmed to its 20 ms; 480 frames at 48 kHz are 441 at 44.1, give
        // or take the one the conversion holds.
        let left = q.queued().as_micros() as i64;
        assert!((left - 10_000).abs() <= 50, "{left}");
    }

    #[test]
    fn a_skip_is_faded() {
        // A queue cut down plays on from what it kept without a step: the
        // first frames after the cut lead from what would have played.
        let q = settled();
        q.write_at(&vec![10_000i16; 480 * 2], ms(0.0));
        q.write_at(&vec![-10_000i16; 480 * 2], ms(10.0));
        q.skip(&mut q.inner.lock().unwrap(), 480);
        assert_eq!(q.stats().dropped, 480);
        let fade = STEREO.frames(FADE);
        let out = q.take_all();
        assert_eq!(out.len(), 480 * 2);
        assert!(out[0] > 9_000 && out[0] < 10_000, "{}", out[0]);
        assert_eq!(out[fade * 2], -10_000);
        let step = 20_000 / fade as i32 + 2;
        let worst = out.windows(2).map(|w| (w[0] as i32 - w[1] as i32).abs()).max().unwrap();
        assert!(worst <= step, "{worst} > {step}");
    }

    /// A far end's writes, as (when, frames), into a queue in front of a
    /// device that takes `period` frames every `period` of time, as
    /// PipeWire's graph does: every pull, its time, how much of it was
    /// sound (the far end's samples are never zero) and what was queued
    /// before it.
    fn play(q: &Queue, writes: &[(f64, usize)], period: usize) -> Vec<(f64, usize, Duration)> {
        let format = q.format();
        q.set_device(format.duration(period), format.duration(period));
        drive(q, writes, period, 1, 1, &mut |_, _| {})
    }

    /// [`play`]'s device, but one that takes `bunch` periods back to back
    /// every `bunch` periods' time, `first` of them at the very start (its
    /// buffer, as AAudio fills it), and is shown the queue at every bunch.
    fn drive(
        q: &Queue,
        writes: &[(f64, usize)],
        period: usize,
        bunch: usize,
        first: usize,
        watch: &mut dyn FnMut(f64, &Queue),
    ) -> Vec<(f64, usize, Duration)> {
        let format = q.format();
        let p = format.duration(period).as_secs_f64() * 1000.0 * bunch as f64;
        let end = writes.last().unwrap().0 + 1000.0;
        let mut pulls = Vec::new();
        let mut out = vec![0i16; period * format.channels];
        let mut next = 0;
        let mut k = 0usize;
        loop {
            let t = k as f64 * p;
            if t > end {
                return pulls;
            }
            while next < writes.len() && writes[next].0 <= t {
                let (at, frames) = writes[next];
                q.write_at(&vec![1000i16; frames * format.channels], ms(at));
                next += 1;
            }
            watch(t, q);
            for _ in 0..if k == 0 { first } else { bunch } {
                let queued = q.queued();
                q.pull(&mut out, format);
                let sound = out.chunks_exact(format.channels).filter(|f| f[0] != 0).count();
                pulls.push((t, sound, queued));
            }
            k += 1;
        }
    }

    /// Pulls that played some silence between the first that played all
    /// sound and the last write: holes in the far end's sound, from `after`
    /// on.
    fn holes(pulls: &[(f64, usize, Duration)], period: usize, writes: &[(f64, usize)], after: f64) -> usize {
        let last = writes.last().unwrap().0;
        let first = pulls.iter().position(|p| p.1 == period).unwrap();
        pulls[first..].iter().filter(|p| p.0 >= after && p.0 < last && p.1 < period).count()
    }

    /// The median of what was queued before each pull from `after` on, in
    /// milliseconds: how far behind the device the far end's sound plays.
    fn level(pulls: &[(f64, usize, Duration)], after: f64) -> f64 {
        let mut v: Vec<f64> = pulls.iter().filter(|p| p.0 >= after).map(|p| p.2.as_secs_f64() * 1000.0).collect();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    }

    use super::super::rhythm::tests::Lcg;

    /// Writes of `frames` at `rate`, `group` at once, a group due every
    /// `group` writes' time and late by `late(k)` ms; never before the one
    /// before.
    fn rhythm(rate: u32, frames: usize, group: usize, seconds: f64, mut late: impl FnMut(usize) -> f64) -> Vec<(f64, usize)> {
        let every = frames as f64 * 1000.0 / rate as f64 * group as f64;
        let mut writes = Vec::new();
        let mut last = 0.0f64;
        for k in 0..(seconds * 1000.0 / every) as usize {
            last = last.max(k as f64 * every + late(k));
            for _ in 0..group {
                writes.push((last, frames));
            }
        }
        writes
    }

    #[test]
    fn steady_writes_play_a_write_and_the_margin_behind() {
        // QEMU's 10 ms and Opus's 20 ms, up to a millisecond late, in front
        // of a 512-frame PipeWire cycle (10.7 ms), once the start's margin
        // has come down.
        for size in [480, 960] {
            let q = queue();
            let mut late = Lcg(1);
            let writes = rhythm(48_000, size, 1, 60.0, |_| late.next());
            let pulls = play(&q, &writes, 512);
            assert_eq!(holes(&pulls, 512, &writes, 0.0), 0, "{size}");
            assert_eq!(q.controls().late(), Duration::ZERO);
            let want = STEREO.duration(512 + size + 48).as_secs_f64() * 1000.0 + 10.0;
            let level = level(&pulls, 50_000.0);
            assert!(level <= want, "{size}: {level} ms queued, {want} at most");
        }
    }

    #[test]
    fn rdp_waves_and_their_lateness_play_through() {
        // Windows' AAC: 1024 frames at 44.1 kHz due every 23.2 ms and sent up
        // to 18 ms late; and the same two at a time, 10 ms early to 10 ms
        // late. A late wave the rhythm has not shown yet may run the queue
        // dry once, early on; never once the margin has grown for it.
        let format = Format::new(44_100, 2).unwrap();
        let cases: [(usize, f64, f64); 2] = [(1, 0.0, 18.0), (2, 10.0, 20.0)];
        for (group, early, spread) in cases {
            let q = Queue::new(format, Arc::new(Controls::new()));
            let mut late = Lcg(12345);
            let writes = rhythm(44_100, 1024, group, 60.0, |_| spread * late.next() - early);
            let pulls = play(&q, &writes, 470);
            let early_holes = holes(&pulls, 470, &writes, 0.0) - holes(&pulls, 470, &writes, 5_000.0);
            assert!(early_holes <= 3, "{group}: {early_holes} early");
            assert_eq!(holes(&pulls, 470, &writes, 5_000.0), 0, "{group}");
            assert!(q.stats().underruns <= 2, "{group}: {:?}", q.stats());
            // A period, the waves that land together and their lateness,
            // and the margin: no more.
            let need = 10.7 + 23.2 * group as f64 + spread + 10.0;
            let level = level(&pulls, 20_000.0);
            let margin = q.margin_now().as_secs_f64() * 1000.0;
            assert!(level < need + margin - 10.0, "{group}: {level} ms queued");
        }
    }

    #[test]
    fn windows_pcm_is_kept_whole() {
        // 186 ms waves, two at once every 372 ms, in front of ALSA: the
        // queue keeps the pair, a period and the margin, and plays them
        // through without running dry or dropping any after the start.
        let format = Format::new(44_100, 2).unwrap();
        let q = Queue::new(format, Arc::new(Controls::new()));
        let mut late = Lcg(5);
        let writes = rhythm(44_100, 8192, 2, 50.0, |_| 5.0 * late.next());
        let pulls = play(&q, &writes, 551);
        assert_eq!(holes(&pulls, 551, &writes, 0.0), 0);
        assert_eq!(q.stats().underruns, 1, "the stream's end only: {:?}", q.stats());
        assert!(q.stats().dropped <= 2 * 8192, "{:?}", q.stats());
        // And when the waves get smaller, so does the target, once the
        // window has forgotten them (and the start's margin has come down,
        // which it has while the waves played).
        let end = writes.last().unwrap().0;
        for k in 0..1000 {
            q.write_at(&vec![1i16; 441 * 2], ms(end + 10.0 * k as f64));
        }
        // (The period, 551 frames, is kept in microseconds: 550.98 of them.)
        assert_eq!(target(&q), format.duration(550 + 441 + 441));
    }

    #[test]
    fn bunched_frames_play_through_once_seen() {
        // 20 ms Opus frames held by a network and let go of in bunches, at
        // moments 20 to 100 ms apart.
        let q = queue();
        let mut net = Lcg(3);
        let mut release = 0.0;
        let mut writes = Vec::new();
        for k in 0..1500 {
            let due = k as f64 * 20.0;
            while release < due {
                release += 20.0 + 80.0 * net.next();
            }
            writes.push((release, 960));
        }
        let pulls = play(&q, &writes, 512);
        assert_eq!(holes(&pulls, 512, &writes, 5_000.0), 0);
        assert!(q.stats().underruns <= 3, "{:?}", q.stats());
    }

    #[test]
    fn a_stall_is_remembered_and_the_next_is_ridden_out() {
        // QEMU's 10 ms writes on time, but for a 40 ms stall every twenty
        // seconds — less often than the rhythm remembers: the first runs
        // the queue dry, the session remembers how late it was, and the
        // ones after it are heard whole.
        let q = queue();
        let stall = |k: usize| if k >= 500 && (k - 500) % 2000 < 4 { 40.0 - 10.0 * ((k - 500) % 2000) as f64 } else { 0.0 };
        let writes = rhythm(48_000, 480, 1, 70.0, stall);
        let pulls = play(&q, &writes, 512);
        let first = holes(&pulls, 512, &writes, 0.0) - holes(&pulls, 512, &writes, 9_000.0);
        assert!(first > 0, "the first stall is a hole");
        assert_eq!(holes(&pulls, 512, &writes, 9_000.0), 0, "the others are not");
        assert_eq!(q.stats().underruns, 1 + 1, "{:?} (and the stream's end)", q.stats());
        // Remembered as 45 ms (the stall and a step), a step forgotten each
        // CLEAN since.
        let late = q.controls().late();
        assert_eq!(late, Duration::from_millis(45) - MARGIN_STEP * 2);
        // The rhythm has the last stall (at 65 s) in its window, so the
        // margin need not allow for it again...
        assert_eq!(q.margin_now(), MARGIN);
        // ...and between stalls, with none in the window, the margin is
        // what allows for the next (40 ms remembered then), and no more than
        // that is kept.
        let between: Vec<_> = pulls.iter().filter(|p| p.0 > 55_000.0 && p.0 < 64_000.0).copied().collect();
        let level = level(&between, 0.0);
        assert!(level < 10.7 + 10.0 + 10.0 + 40.0 + 1.0, "{level}");
    }

    #[test]
    fn stalls_the_rhythm_sees_are_not_counted_twice() {
        // The same stall every five seconds, which the rhythm remembers from
        // one to the next: the queue keeps it once, not once as the rhythm's
        // and again as the margin's.
        let q = queue();
        let writes = rhythm(48_000, 480, 1, 60.0, |k| if k >= 500 && k % 500 < 4 { 40.0 - 10.0 * (k % 500) as f64 } else { 0.0 });
        let pulls = play(&q, &writes, 512);
        assert_eq!(holes(&pulls, 512, &writes, 9_000.0), 0);
        assert_eq!(q.stats().underruns, 1 + 1, "{:?} (and the stream's end)", q.stats());
        let level = level(&pulls, 20_000.0);
        assert!(level < 10.7 + 10.0 + 40.0 + 10.0 + MARGIN_STEP.as_secs_f64() * 1000.0 + 1.0, "{level}");
    }

    #[test]
    fn lateness_is_forgotten_after_a_long_while_and_the_queue_comes_down() {
        // The most lateness remembered, then a long while of sound on time:
        // it is forgotten a step every CLEAN, and what the queue keeps
        // follows it down within a window of each.
        let q = queue();
        q.controls().ran_dry(MARGIN_MAX);
        assert_eq!(q.controls().late(), MARGIN_MAX);
        let steps = (MARGIN_MAX.as_millis() / MARGIN_STEP.as_millis()) as f64;
        let seconds = CLEAN.as_secs_f64() * (steps + 1.0);
        let writes = rhythm(48_000, 480, 1, seconds, |_| 0.0);
        let pulls = play(&q, &writes, 512);
        assert_eq!(holes(&pulls, 512, &writes, 0.0), 0);
        assert_eq!(q.controls().late(), Duration::ZERO);
        let start = level(&pulls[..pulls.len() / 20], 0.0);
        let end = level(&pulls, seconds * 1000.0 - 5_000.0);
        assert!(start > 100.0 && end < 35.0, "{start} ms queued at the start, {end} at the end");
        assert!(q.stats().dropped > 0);
    }

    #[test]
    fn a_queue_that_kept_more_than_it_needed_comes_down_to_it() {
        // The device opened late, and the far end's writes before it are
        // under the limit: the queue starts at the target, but where the
        // stream started on a late write it keeps more than it needs, which
        // is dropped once a window has shown it.
        let q = queue();
        let mut writes = vec![(0.0, 480)];
        for k in 1..6000 {
            writes.push((30.0 + k as f64 * 10.0, 480));
        }
        let pulls = play(&q, &writes, 512);
        assert_eq!(holes(&pulls, 512, &writes, 0.0), 0);
        let level = level(&pulls, 50_000.0);
        assert!(level < 10.7 + 10.0 + 10.0 + 1.0, "{level}");
    }

    #[test]
    fn a_stream_starts_with_the_higher_margin_and_comes_down_to_the_least() {
        // QEMU's 10 ms on time for a minute: the margin is MARGIN_START at
        // first, a step less after each CLEAN_START, MARGIN from 40 s on; and
        // what is queued follows it down, a window behind, without a hole.
        let q = queue();
        assert_eq!(q.margin_now(), MARGIN_START);
        let writes = rhythm(48_000, 480, 1, 60.0, |_| 0.0);
        let mut margins = Vec::new();
        q.set_device(STEREO.duration(512), STEREO.duration(512));
        let pulls = drive(&q, &writes, 512, 1, 1, &mut |t, q| margins.push((t, q.margin_now())));
        assert_eq!(holes(&pulls, 512, &writes, 0.0), 0);
        let at = |s: f64| margins.iter().find(|m| m.0 >= s * 1000.0).unwrap().1;
        assert_eq!(at(5.0), MARGIN_START);
        assert_eq!(at(15.0), MARGIN_START - MARGIN_STEP);
        assert_eq!(at(35.0), MARGIN + MARGIN_STEP);
        assert_eq!(at(45.0), MARGIN);
        let early = level(&pulls[..pulls.len() / 10], 0.0);
        let late = level(&pulls, 52_000.0);
        assert!(late < 10.7 + 10.0 + 10.0 + 1.0, "{late}");
        assert!(early > late + 20.0 - 1.0, "{early} ms queued at first, {late} at the end");
        assert_eq!(q.stats().underruns, 1, "the stream's end only");
    }

    #[test]
    fn the_start_and_the_sessions_lateness_are_not_added() {
        // A session that remembers less lateness than the start allows for
        // starts a new stream at MARGIN_START, not above it; one that
        // remembers more starts there instead, not lower.
        let controls = Arc::new(Controls::new());
        controls.ran_dry(Duration::from_millis(5));
        let q = Queue::new(STEREO, Arc::clone(&controls));
        assert_eq!(controls.late(), Duration::from_millis(10));
        assert_eq!(q.margin_now(), MARGIN_START);
        controls.ran_dry(Duration::from_millis(40));
        let q = Queue::new(STEREO, Arc::clone(&controls));
        assert_eq!(q.margin_now(), MARGIN + Duration::from_millis(45));
        // And the lateness the stream's rhythm shows comes off the
        // session's, never off the start's: 20 ms seen, 25 left of 45.
        q.write_at(&frames(1, 10), ms(0.0));
        q.write_at(&frames(1, 10), ms(30.0));
        assert_eq!(q.margin_now(), MARGIN + Duration::from_millis(25));
        let q = Queue::new(STEREO, Arc::new(Controls::new()));
        q.write_at(&frames(1, 10), ms(0.0));
        q.write_at(&frames(1, 10), ms(30.0));
        assert_eq!(q.margin_now(), MARGIN_START);
    }

    #[test]
    fn running_dry_keeps_the_start_from_coming_down_for_a_while() {
        // A 40 ms stall 5 s in: the start's margin (30 ms) is no match for
        // it, the session remembers it (45 ms, so the margin is that now),
        // and the start's own count begins again from the stall, so it is
        // a step down only CLEAN_START after it rather than after the first.
        let q = queue();
        let writes = rhythm(48_000, 480, 1, 13.0, |k| if (500..504).contains(&k) { 40.0 - 10.0 * (k - 500) as f64 } else { 0.0 });
        let pulls = play(&q, &writes, 512);
        assert!(holes(&pulls, 512, &writes, 0.0) > 0);
        assert_eq!(q.controls().late(), Duration::from_millis(45));
        assert_eq!(q.inner.lock().unwrap().start, MARGIN_START - MARGIN);
        let q2 = queue();
        let writes = rhythm(48_000, 480, 1, 13.0, |_| 0.0);
        play(&q2, &writes, 512);
        assert_eq!(q2.inner.lock().unwrap().start, MARGIN_START - MARGIN - MARGIN_STEP);
    }

    /// AAudio's Legacy stream on the phone (`aaudio.rs`): 20 ms bursts, a
    /// 60 ms buffer filled at once, then two bursts back to back every
    /// 40 ms, as its mixer reads them; the period the queue is told is
    /// `period` frames.
    fn aaudio(q: &Queue, writes: &[(f64, usize)], period: usize) -> Vec<(f64, usize, Duration)> {
        q.set_device(STEREO.duration(2880), STEREO.duration(period));
        drive(q, writes, 960, 2, 3, &mut |_, _| {})
    }

    #[test]
    fn a_device_that_takes_two_bursts_at_once_has_them_kept() {
        // QEMU's 10 ms writes, the one due at the device's sixth read 4 ms
        // late and the one due at its second after the pause, in a stream
        // past its start, and a second's pause of the far end's
        // 15 s in, after which the queue starts again from nothing — long
        // after a first window, so nothing but the period can allow for the
        // read. Told a burst as the period, the queue starts again with the
        // first read and little to spare after it, and the late writes are
        // holes; told what the mixer reads, they are not, nor at the start.
        let writes: Vec<_> = rhythm(48_000, 480, 1, 30.0, |k| if k == 24 || k == 1608 { 4.0 } else { 0.0 })
            .into_iter()
            .filter(|w| w.0 < 15_000.0 || w.0 >= 16_000.0)
            .collect();
        let after = |period| {
            let q = settled();
            let pulls = aaudio(&q, &writes, period);
            let start = holes(&pulls, 960, &writes, 0.0) - holes(&pulls, 960, &writes, 15_000.0);
            // From the first read that played sound again.
            let again = pulls.iter().position(|p| p.0 >= 16_000.0 && p.1 == 960).unwrap();
            (start, holes(&pulls[again..], 960, &writes, 0.0))
        };
        let burst = after(960);
        assert!(burst.0 > 0 && burst.1 > 0, "a burst: {burst:?}");
        assert_eq!(after(1920), (0, 0), "what the mixer reads");
    }

    #[test]
    fn a_steady_queue_keeps_the_mixers_reads_and_the_margin() {
        // The same device, told what it reads, and writes 4 ms late every
        // five seconds, from a minute's start: no hole, and before each read
        // the two bursts and the margin besides.
        let writes = rhythm(48_000, 480, 1, 60.0, |k| if k % 500 == 24 { 4.0 } else { 0.0 });
        let q = queue();
        let pulls = aaudio(&q, &writes, 1920);
        assert_eq!(holes(&pulls, 960, &writes, 0.0), 0);
        assert_eq!(q.stats().underruns, 1, "the stream's end only");
        let first = pulls.windows(2).filter(|w| w[1].0 != w[0].0).map(|w| w[1]);
        let least = first.filter(|p| p.0 > 45_000.0 && p.0 < 59_000.0).map(|p| p.2).min().unwrap();
        assert!(least >= Duration::from_millis(40) + MARGIN, "{least:?}");
    }

    /// A pull's time, the target then and what had been dropped so far.
    type Seen = (f64, Duration, u64);

    /// QEMU's 10 ms writes on time but for a stall of `ms` five seconds in,
    /// what was due meanwhile arriving at once after it, in front of a
    /// PipeWire-like device: the pulls, and at every pull from 4 s on its
    /// time, the target and what had been dropped so far.
    fn stalled(q: &Queue, ms: f64) -> (Vec<(f64, usize, Duration)>, Vec<Seen>) {
        let n = (ms / 10.0) as usize;
        let writes = rhythm(48_000, 480, 1, 30.0, |k| if (500..500 + n).contains(&k) { ms - 10.0 * (k - 500) as f64 } else { 0.0 });
        let mut seen = Vec::new();
        q.set_device(STEREO.duration(512), STEREO.duration(512));
        let pulls = drive(q, &writes, 512, 1, 1, &mut |t, q| {
            if t >= 4_000.0 {
                seen.push((t, q.target_now(), q.stats().dropped));
            }
        });
        (pulls, seen)
    }

    #[test]
    fn a_stall_past_the_most_lateness_is_skipped_in_one_jump() {
        // Held back 400 ms: the queue runs dry, and the far end's backlog
        // arriving at once neither raises the target (the rhythm starts a new
        // run, and the backlog is a burst to it) nor is kept: it is dropped
        // while the device still plays the stall's silence, and the queue
        // starts again the target behind, one jump from where it stopped;
        // the session remembers nothing of it.
        let q = queue();
        let (pulls, seen) = stalled(&q, 400.0);
        assert_eq!(q.stats().underruns, 1 + 1, "{:?} (the stall, and the stream's end)", q.stats());
        assert_eq!(q.controls().late(), Duration::ZERO);
        let before = seen.iter().filter(|s| s.0 < 5_000.0).map(|s| s.1).max().unwrap();
        let after = seen.iter().filter(|s| s.0 >= 5_000.0).map(|s| s.1).max().unwrap();
        assert!(after <= before, "the target {before:?} before the stall, {after:?} after");
        let drops: Vec<_> = seen.windows(2).filter(|w| w[1].2 > w[0].2).map(|w| (w[1].0, w[1].2 - w[0].2)).collect();
        let (jump, rest): (Vec<_>, Vec<_>) = drops.iter().copied().partition(|d| d.0 > 5_400.0 && d.0 < 5_450.0);
        assert!(jump.iter().map(|d: &(f64, u64)| d.1).sum::<u64>() >= STEREO.frames(ms(300.0)) as u64, "{drops:?}");
        assert!(rest.iter().all(|d| d.1 < STEREO.frames(ms(10.0)) as u64), "{drops:?}");
        let again = pulls.iter().find(|p| p.0 > 5_000.0 && p.1 == 512 && pulls.iter().any(|q| q.0 > 5_000.0 && q.0 < p.0 && q.1 < 512)).unwrap();
        assert!(again.0 < 5_450.0, "sound again at {} ms", again.0);
        // What is queued from then on is what it was before, give or take a
        // write.
        let level = |from: f64, to: f64| level(&pulls.iter().filter(|p| p.0 >= from && p.0 < to).copied().collect::<Vec<_>>(), 0.0);
        let (was, is) = (level(3_000.0, 5_000.0), level(5_500.0, 7_500.0));
        assert!((is - was).abs() < 10.0, "{was} ms queued before the stall, {is} after");
        assert_eq!(holes(&pulls, 512, &rhythm(48_000, 480, 1, 30.0, |_| 0.0), 5_500.0), 0);
    }

    #[test]
    fn a_stall_up_to_the_most_is_remembered_and_kept_as_before() {
        // Held back 90 ms: late, not a stall; the queue starts again with
        // the backlog as before — cut only to the target, which has the
        // lateness in it, 9 ms here — and the session remembers it and a
        // step.
        let q = queue();
        let (_, seen) = stalled(&q, 90.0);
        assert_eq!(q.stats().underruns, 1 + 1, "{:?}", q.stats());
        assert_eq!(q.controls().late(), Duration::from_millis(95));
        let dropped = |t: f64| seen.iter().find(|s| s.0 >= t).unwrap().2;
        assert!(dropped(6_000.0) - dropped(4_000.0) < STEREO.frames(ms(10.0)) as u64, "{seen:?}");
    }

    /// Windows' PCM pairs, a little late at random, in front of ALSA, and
    /// the pair due 15 s in late by `ms` (the start's margin is 25 ms then):
    /// the pulls, the late pair's arrival and what was dropped in the two
    /// seconds from just before it.
    fn pcm_late_pair(q: &Queue, ms: f64) -> (Vec<(f64, usize, Duration)>, f64, u64) {
        let mut late = Lcg(5);
        let writes = rhythm(44_100, 8192, 2, 40.0, |k| if k == 40 { ms } else { 2.0 * late.next() });
        let at = writes[80].0;
        let (mut before, mut after) = (0, 0);
        q.set_device(q.format().duration(551), q.format().duration(551));
        let pulls = drive(q, &writes, 551, 1, 1, &mut |t, q| {
            if t < at {
                before = q.stats().dropped;
            }
            if t < at + 2_000.0 {
                after = q.stats().dropped;
            }
        });
        (pulls, at, after - before)
    }

    /// How long the pulls around `at` played silence, in milliseconds.
    fn silent(pulls: &[(f64, usize, Duration)], period: usize, at: f64, rate: u32) -> f64 {
        let missing: usize = pulls.iter().filter(|p| p.0 > at - 1_000.0 && p.0 < at + 2_000.0).map(|p| period - p.1).sum();
        missing as f64 * 1000.0 / rate as f64
    }

    #[test]
    fn a_late_pair_is_late_by_its_own_lateness_and_remembered() {
        // Windows' PCM, its pair due 15 s in 60 ms late: the queue runs dry
        // for what the margin did not cover, and the pair is measured as the
        // 60 ms late it is, not as a wave more (the second of the pair before
        // it is a wave further on and arrived with the first) — which would
        // be a stall: remembered, nothing of it dropped, and the queue plays
        // on from where it stopped.
        let format = Format::new(44_100, 2).unwrap();
        let q = Queue::new(format, Arc::new(Controls::new()));
        let (pulls, at, dropped) = pcm_late_pair(&q, 60.0);
        assert_eq!(q.stats().underruns, 1 + 1, "{:?} (the late pair, and the stream's end)", q.stats());
        let late = q.controls().late();
        assert!(late >= Duration::from_millis(60) && late <= Duration::from_millis(70), "{late:?}");
        // Dry for what the margin and the period did not cover, then those
        // again before it starts: a gap of about the lateness, where waiting
        // for the whole target (the pair and its lateness) took until the
        // next pair, 312 ms on.
        let silent = silent(&pulls, 551, at, 44_100);
        assert!(silent < 75.0, "{silent} ms of silence");
        assert!(dropped < format.frames(ms(10.0)) as u64, "{dropped} frames dropped with it");
        assert_eq!(holes(&pulls, 551, &rhythm(44_100, 8192, 2, 40.0, |_| 0.0), at + 1_000.0), 0);
    }

    #[test]
    fn a_stalled_pair_is_still_a_stall() {
        // The same pair 250 ms late: past the most lateness, so not
        // remembered and its backlog skipped, as for writes one at a time.
        let format = Format::new(44_100, 2).unwrap();
        let q = Queue::new(format, Arc::new(Controls::new()));
        let (pulls, at, dropped) = pcm_late_pair(&q, 250.0);
        assert_eq!(q.stats().underruns, 1 + 1, "{:?}", q.stats());
        assert_eq!(q.controls().late(), Duration::ZERO);
        assert!(dropped >= format.frames(ms(250.0)) as u64, "{dropped} frames dropped with it");
        assert_eq!(holes(&pulls, 551, &rhythm(44_100, 8192, 2, 40.0, |_| 0.0), at + 1_000.0), 0);
    }
}
