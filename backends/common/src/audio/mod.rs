// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The far end's sound, where a protocol sends it: the format a stream is
//! in, what a platform's output promises, and the policy between the two.
//!
//! A protocol asks for a stream and does not know whose it is, as with
//! [`video::Decoders`](crate::video::Decoders): what it holds is an
//! [`Outputs`], handed to it by whoever started the session — AAudio in
//! `platform_android`, ALSA in `platform_desktop`, [`NoOutputs`] for a test
//! with neither. Before this each backend that had sound had a sink of its
//! own on each platform (TigerVNC's ALSA and AAudio ones, FreeRDP's
//! PulseAudio and OpenSL ES ones), each with its own idea of how much to
//! queue, and none that a window could mute.
//!
//! What every platform does alike is here, in [`Queue`]: a protocol thread
//! writes into it and never waits, and the platform's device drains it on a
//! thread of the device's own. Between them the queue and the device's own
//! buffer keep about [`TARGET`] of sound ahead of what is heard, never more
//! than [`LIMIT`] — past that the queue drops the oldest, so that a burst or
//! a stall costs a skip rather than a lag that grows for the rest of the
//! session — and the queue plays silence when it runs dry until it has its
//! share of the target again. The gain and the mute are applied as the
//! device takes the samples, so a mute is heard at once rather than after
//! what is queued, and [`Controls`] can be set from any thread: a window's
//! toolbar, while the protocol thread writes.
//!
//! A platform's half is small for that: [`Outputs::device`] starts whatever
//! drains a queue — opening the device where opening takes the time rather
//! than on the caller's thread — and [`Outputs::open`] builds the
//! [`Stream`] a protocol writes to around it. Where the far end's sound
//! arrives compressed, the platform's decoders for it ([`decode`]) are the
//! same object's, [`Outputs::decoders`]: one thing a session is handed for
//! its sound, however it arrives.

pub mod decode;
mod queue;
mod resample;

pub use queue::{Queue, Stats};

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How far what is heard is behind what arrived: the queue and the device's
/// own buffer together, which the queue fills back up to before it plays
/// again after running dry. Enough to ride out a network that delivers in
/// bursts of a few tens of milliseconds, little enough that what is heard is
/// not noticeably behind what is seen.
///
/// The total rather than the queue's alone, since the device's buffer is
/// delay just as the queue is: with a tenth of a second queued in front of
/// ALSA's 50 ms, RDP's sound was heard 100 ms later than through the
/// PulseAudio device it replaced. So the queue keeps the target less what the
/// device holds ([`Queue::set_device`]), down to what keeps it from running
/// dry between the far end's writes.
pub const TARGET: Duration = Duration::from_millis(100);

/// What the queue keeps beyond what the far end's own rhythm and the
/// device's take from it ([`Queue`] works the rest out from the writes),
/// for a write that is later than any before it.
pub const MARGIN: Duration = Duration::from_millis(10);

/// How much sound the queue and the device hold together before the queue
/// drops the oldest, back down to its share of [`TARGET`] — or twice that
/// share, for a far end whose own rhythm needs more than the target (see
/// [`Queue`]'s, which follows the size of its writes). TigerVNC's viewer kept
/// a second, and a stream that ran even slightly faster than the device
/// lagged further behind the picture for as long as it ran.
pub const LIMIT: Duration = Duration::from_millis(250);

/// The most channels a stream may have: 7.1, which is more than any
/// protocol here sends.
pub const MAX_CHANNELS: usize = 8;

/// What a stream's samples are: signed 16-bit, native-endian, a sample for
/// each channel in turn (interleaved). It is what QEMU's audio and RDP's PCM
/// are, and what the AAC and Opus decoders hand back; floating-point samples
/// would be a field here, when something sends them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// Frames a second.
    pub rate: u32,
    pub channels: usize,
}

impl Format {
    /// A format a queue can hold: a rate from 8 kHz to 384 kHz and one to
    /// [`MAX_CHANNELS`] channels. The bounds are what keeps the arithmetic
    /// below from overflowing, and are wider than anything sent.
    pub fn new(rate: u32, channels: usize) -> Option<Format> {
        ((8_000..=384_000).contains(&rate) && (1..=MAX_CHANNELS).contains(&channels))
            .then_some(Format { rate, channels })
    }

    /// Bytes in one frame, a sample of every channel.
    pub fn frame_bytes(self) -> usize {
        self.channels * 2
    }

    /// How many frames last `d`, rounded down.
    pub fn frames(self, d: Duration) -> usize {
        (d.as_micros() * self.rate as u128 / 1_000_000) as usize
    }

    /// How long `frames` frames last.
    pub fn duration(self, frames: usize) -> Duration {
        Duration::from_micros((frames as u128 * 1_000_000 / self.rate as u128) as u64)
    }
}

/// What a session's sound is played at, which whoever holds the session can
/// change while it plays, from any thread: a gain over every stream, a mute,
/// and whether anything is playing at all.
///
/// One a session, shared by its streams: a toolbar mutes the session, not
/// one of the far end's streams, and a backend that opens a new stream for
/// a new format (RDP's sound does, per format) must not come back unmuted.
pub struct Controls {
    /// An `f32`'s bits, so that the device's thread reads it without a lock.
    gain: AtomicU32,
    muted: AtomicBool,
    playing: Mutex<Playing>,
}

struct Playing {
    /// Streams that have been written to and not yet closed.
    streams: usize,
    listener: Option<Box<dyn Fn(bool) + Send + Sync>>,
}

impl Controls {
    pub fn new() -> Controls {
        Controls {
            gain: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
            playing: Mutex::new(Playing {
                streams: 0,
                listener: None,
            }),
        }
    }

    /// The gain over every stream of the session, as a factor: 1 is as sent,
    /// 0 is silence. Clamped to 0..=4, since past that a 16-bit sample only
    /// clips.
    pub fn set_gain(&self, gain: f32) {
        let gain = if gain.is_finite() { gain.clamp(0.0, 4.0) } else { 1.0 };
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    /// Silence, at once and until unmuted: what is queued keeps draining
    /// under the mute, so that unmuting is the stream as it is now and not
    /// where it was when it was muted.
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    /// Whether any stream of the session has had sound written to it and is
    /// still open: what a toolbar shows its mute by, so that a session that
    /// never plays anything has no button for it.
    pub fn playing(&self) -> bool {
        self.playing.lock().unwrap().streams > 0
    }

    /// `listener` is told whenever [`playing`](Controls::playing) changes,
    /// with the new answer — on the protocol thread that wrote or closed the
    /// stream, so it must not block, and must not call back into these
    /// controls' `playing`. The session hands over one that says it as an
    /// event. Replaces any listener before it.
    pub fn on_playing(&self, listener: Box<dyn Fn(bool) + Send + Sync>) {
        self.playing.lock().unwrap().listener = Some(listener);
    }

    /// A stream has had its first write (`true`) or has closed after one
    /// (`false`).
    fn stream_playing(&self, started: bool) {
        let mut p = self.playing.lock().unwrap();
        let before = p.streams > 0;
        if started {
            p.streams += 1;
        } else {
            p.streams = p.streams.saturating_sub(1);
        }
        let now = p.streams > 0;
        if before != now
            && let Some(listener) = &p.listener
        {
            listener(now);
        }
    }

    /// The factor the device scales samples by: the session's gain, times a
    /// stream's own, or nothing at all under the mute.
    fn factor(&self, stream: f32) -> f32 {
        if self.muted() { 0.0 } else { self.gain() * stream }
    }
}

impl Default for Controls {
    fn default() -> Controls {
        Controls::new()
    }
}

/// A platform's side of one stream: whatever drains its queue into the
/// platform's output, for as long as it is held. Dropping it stops the
/// device **without waiting for it** — a device still opening, or one
/// blocked on a sound server that has gone away, closes itself on its own
/// thread when it gets there — since the stream is dropped on the protocol
/// thread, which must not wait on sound.
pub trait Device: Send {
    /// How long a sample the device has taken from the queue takes to be
    /// heard: the device's own buffer, as far as it says. Zero where it does
    /// not know, or has not opened yet.
    fn latency(&self) -> Duration {
        Duration::ZERO
    }
}

/// Where a session's sound goes: the platform's output, and the controls a
/// session's streams share.
///
/// Shared between the protocol thread, which opens and writes streams, and
/// whoever holds the session, which sets the controls.
pub trait Outputs: Send + Sync {
    /// The session's controls, which every stream opened here plays under.
    fn controls(&self) -> &Arc<Controls>;

    /// Starts draining `queue` into this platform's output, and returns what
    /// keeps it draining. Must not wait on the device: opening it, where
    /// that talks to a sound server, is done on a thread of the device's
    /// own, and until it is open the queue fills to its limit and drops the
    /// oldest like any other queue nobody is draining. An error is a platform
    /// that cannot even start (no thread), and is a stream with no sound.
    fn device(&self, queue: Arc<Queue>) -> Result<Box<dyn Device>, String>;

    /// What decodes compressed sound for these outputs' streams: the
    /// platform's AAC and Opus decoders, or `None` for a platform with none,
    /// whose sessions take the far end's sound only as PCM.
    fn decoders(&self) -> Option<&dyn decode::Decoders> {
        None
    }

    /// A stream in `format`, writable at once — whether or not the device
    /// behind it has opened yet, or ever does. `None` where the format is
    /// not one [`Format::new`] takes, or the platform has no output.
    fn open(&self, format: Format) -> Option<Stream> {
        let format = Format::new(format.rate, format.channels)?;
        let queue = Arc::new(Queue::new(format, Arc::clone(self.controls())));
        match self.device(Arc::clone(&queue)) {
            Ok(device) => Some(Stream {
                queue,
                device,
                partial: [0; MAX_CHANNELS * 2],
                partial_len: 0,
                playing: false,
            }),
            Err(why) => {
                log::warn!("audio: could not start an output ({why}); the far end's sound is dropped");
                None
            }
        }
    }
}

/// No sound: for a test of a protocol that is not about its sound, and for
/// a host with no output. A backend handed this plays nothing, and says so
/// once, at the first stream.
pub struct NoOutputs {
    controls: Arc<Controls>,
}

impl NoOutputs {
    pub fn new() -> NoOutputs {
        NoOutputs {
            controls: Arc::new(Controls::new()),
        }
    }
}

impl Default for NoOutputs {
    fn default() -> NoOutputs {
        NoOutputs::new()
    }
}

impl Outputs for NoOutputs {
    fn controls(&self) -> &Arc<Controls> {
        &self.controls
    }

    fn device(&self, _queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
        Err("this build has no audio output".into())
    }
}

/// One stream of the far end's sound, as a protocol writes it. Every call
/// returns at once: a write goes into the [`Queue`] and the device takes it
/// from there on a thread of its own.
///
/// `Send`, and one thread's at a time: a protocol writes from whichever
/// thread its sound arrives on.
pub struct Stream {
    queue: Arc<Queue>,
    device: Box<dyn Device>,
    /// The start of a frame whose end has not arrived: QEMU's audio chunks
    /// are aligned to nothing.
    partial: [u8; MAX_CHANNELS * 2],
    partial_len: usize,
    /// Whether this stream counts towards [`Controls::playing`].
    playing: bool,
}

impl Stream {
    pub fn format(&self) -> Format {
        self.queue.format()
    }

    /// Whole frames, as samples. A trailing part of a frame is dropped.
    pub fn write(&mut self, samples: &[i16]) {
        self.started();
        self.queue.write(samples);
    }

    /// Samples as native-endian bytes, cut anywhere: a part of a frame left
    /// at the end is kept and finished by the next write.
    pub fn write_bytes(&mut self, mut bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.started();
        let frame = self.format().frame_bytes();
        if self.partial_len > 0 {
            let take = (frame - self.partial_len).min(bytes.len());
            self.partial[self.partial_len..self.partial_len + take].copy_from_slice(&bytes[..take]);
            self.partial_len += take;
            bytes = &bytes[take..];
            if self.partial_len < frame {
                return;
            }
            self.queue.write_bytes(&self.partial[..frame]);
            self.partial_len = 0;
        }
        let whole = bytes.len() / frame * frame;
        self.queue.write_bytes(&bytes[..whole]);
        let rest = &bytes[whole..];
        self.partial[..rest.len()].copy_from_slice(rest);
        self.partial_len = rest.len();
    }

    /// The stream's own gain, over the session's: what a far end that sets
    /// its own volume (RDP's Volume PDU) sets. 1 is as sent.
    pub fn set_gain(&self, gain: f32) {
        self.queue.set_gain(gain);
    }

    /// How long until a sample written now is heard: what is queued, and
    /// the device's own buffer behind it. What RDP's sound tells the server
    /// in its confirmations.
    pub fn latency(&self) -> Duration {
        self.queue.queued() + self.device.latency()
    }

    pub fn stats(&self) -> Stats {
        self.queue.stats()
    }

    fn started(&mut self) {
        if !self.playing {
            self.playing = true;
            self.queue.controls().stream_playing(true);
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if self.playing {
            self.queue.controls().stream_playing(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A device that drains nothing, and counts how many are held.
    struct Held(Arc<AtomicUsize>);

    impl Device for Held {
        fn latency(&self) -> Duration {
            Duration::from_millis(20)
        }
    }

    impl Drop for Held {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::Relaxed);
        }
    }

    struct Fake {
        controls: Arc<Controls>,
        held: Arc<AtomicUsize>,
        queues: Mutex<Vec<Arc<Queue>>>,
    }

    impl Outputs for Fake {
        fn controls(&self) -> &Arc<Controls> {
            &self.controls
        }

        fn device(&self, queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
            self.queues.lock().unwrap().push(queue);
            self.held.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(Held(Arc::clone(&self.held))))
        }
    }

    fn fake() -> Fake {
        Fake {
            controls: Arc::new(Controls::new()),
            held: Arc::new(AtomicUsize::new(0)),
            queues: Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn formats_out_of_bounds_are_refused() {
        assert!(Format::new(48_000, 2).is_some());
        assert!(Format::new(0, 2).is_none());
        assert!(Format::new(48_000, 0).is_none());
        assert!(Format::new(48_000, MAX_CHANNELS + 1).is_none());
        let f = Format::new(48_000, 2).unwrap();
        assert_eq!(f.frames(TARGET), 4800);
        assert_eq!(f.duration(4800), TARGET);
        assert_eq!(f.frame_bytes(), 4);
        let outputs = fake();
        assert!(outputs.open(Format { rate: 1, channels: 2 }).is_none());
        assert!(NoOutputs::new().open(f).is_none());
    }

    #[test]
    fn bytes_cut_anywhere_arrive_as_whole_frames() {
        let outputs = fake();
        let format = Format::new(48_000, 2).unwrap();
        let mut stream = outputs.open(format).unwrap();
        let samples: Vec<i16> = (0..64).map(|i| i * 100 - 3000).collect();
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_ne_bytes()).collect();
        // Cut at every odd place, a byte or three at a time.
        let mut at = 0;
        for (i, len) in [1usize, 3, 2, 5, 7, 1, 1, 9].iter().cycle().enumerate() {
            if at >= bytes.len() {
                break;
            }
            let end = (at + len + i % 2).min(bytes.len());
            stream.write_bytes(&bytes[at..end]);
            at = end;
        }
        let queue = Arc::clone(&outputs.queues.lock().unwrap()[0]);
        assert_eq!(queue.queued(), format.duration(32));
        assert_eq!(queue.take_all(), samples);
        assert_eq!(stream.latency(), Duration::from_millis(20));
    }

    #[test]
    fn playing_is_said_when_the_first_stream_starts_and_the_last_closes() {
        let outputs = fake();
        let said = Arc::new(Mutex::new(Vec::new()));
        let s = Arc::clone(&said);
        outputs.controls().on_playing(Box::new(move |p| s.lock().unwrap().push(p)));
        let format = Format::new(44_100, 1).unwrap();

        let mut a = outputs.open(format).unwrap();
        let mut b = outputs.open(format).unwrap();
        // Open is not playing: a stream nobody writes to is no sound.
        assert!(!outputs.controls().playing());
        a.write(&[1, 2]);
        b.write_bytes(&[0, 1]);
        a.write(&[3]);
        assert!(outputs.controls().playing());
        drop(a);
        assert!(outputs.controls().playing());
        drop(b);
        assert!(!outputs.controls().playing());
        // An unwritten stream closing says nothing.
        drop(outputs.open(format).unwrap());
        assert_eq!(*said.lock().unwrap(), [true, false]);
        assert_eq!(outputs.held.load(Ordering::Relaxed), 0, "every device let go");
    }

    #[test]
    fn controls_clamp_what_they_are_given() {
        let c = Controls::new();
        assert_eq!(c.gain(), 1.0);
        c.set_gain(9.0);
        assert_eq!(c.gain(), 4.0);
        c.set_gain(-1.0);
        assert_eq!(c.gain(), 0.0);
        c.set_gain(f32::NAN);
        assert_eq!(c.gain(), 1.0);
        c.set_muted(true);
        assert_eq!(c.factor(1.0), 0.0);
        c.set_muted(false);
        c.set_gain(0.5);
        assert_eq!(c.factor(0.5), 0.25);
    }
}
