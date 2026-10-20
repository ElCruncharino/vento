// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The far end's sound, where a protocol sends it: the format a stream is
//! in, what a platform's output promises, and the policy between the two.
//!
//! A protocol asks for a stream and does not know whose it is, as with
//! [`video::Decoders`](crate::media::video::Decoders): what it holds is an
//! [`Outputs`], handed to it by whoever started the session — AAudio in
//! `platform_android`, PipeWire or ALSA in `platform_linux`, [`NoOutputs`]
//! for a test with neither. Before this each backend that had sound had a sink of its
//! own on each platform (TigerVNC's ALSA and AAudio ones, FreeRDP's
//! PulseAudio and OpenSL ES ones), each with its own idea of how much to
//! queue, and none that a window could mute.
//!
//! What every platform does alike is here, in [`Queue`]: a protocol thread
//! writes into it and never waits, and the platform's device drains it on a
//! thread of the device's own. The queue keeps as little in front of the
//! device as keeps it from running dry — a period of the device's, what the
//! far end's own rhythm of writes has needed of late, and a margin that
//! starts higher and comes down over a stream's first clean seconds, grows
//! when the far end's sound turns out later than that and shrinks back
//! after a long while of it not — and the device's own buffer is behind
//! that; never more than [`LIMIT`] together — past that the queue drops the
//! oldest, so that a burst or a stall costs a skip rather than a lag that
//! grows for the rest of the session — and the queue plays silence when it
//! runs dry until it has its target again. The gain and the mute are applied
//! as the device takes the samples, so a mute is heard at once rather than
//! after what is queued, and [`Controls`] can be set from any thread: a
//! window's toolbar, while the protocol thread writes.
//!
//! The same controls carry the session's sound switch ([`Sound`]): what
//! the backend has said the far end can be asked — to stop sending its
//! sound and to send it again ([`Switch::Enable`]), where the protocol has
//! a message for that, or nothing, so that the switch only mutes it here
//! ([`Switch::Mute`]) — said as soon as the protocol says the far end may
//! send any, so that a window shows the switch from then on rather than
//! from the first sound.
//!
//! A platform's half is small for that: [`Outputs::device`] starts whatever
//! drains a queue — opening the device where opening takes the time rather
//! than on the caller's thread — and [`Outputs::open`] builds the
//! [`Stream`] a protocol writes to around it. Where the far end's sound
//! arrives compressed, the platform's decoders for it ([`decode`]) are the
//! same object's, [`Outputs::decoders`]: one thing a session is handed for
//! its sound, however it arrives.
//!
//! A protocol written in Rust plays through a [`Player`], which is that
//! object's streams and decoders as the far end's sound arrives: a stream
//! per format, a decoder per codec, and a stream closed once it has gone
//! quiet.

pub mod decode;
mod player;
mod queue;
mod resample;
mod rhythm;

pub use player::Player;
pub use queue::{Queue, Stats};

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use decode::Codec;

/// The least margin a queue keeps: what it keeps beyond the device's period
/// and what the far end's rhythm has needed of late ([`Queue`] works those
/// out), for a write later than any of late. Measured on an idle machine
/// (`linux/README.md`'s Sound paragraph has the rest): minute-long sessions
/// to Windows and SPICE ran dry with no margin in three of four and with
/// 5 ms in one of four; with 10 ms, one of seventeen sessions across the six
/// backends with sound did. Each millisecond of it is a millisecond the sound
/// is behind the picture.
pub const MARGIN: Duration = Duration::from_millis(10);

/// The margin a stream starts with, coming down a [`MARGIN_STEP`] at a time
/// to [`MARGIN`] after every [`CLEAN_START`] of sound that did not run dry: for
/// the first seconds, when the stream's rhythm has shown little of how the
/// far end sends and the session nothing of how late, and for a loaded
/// machine, which a margin learnt only after a gap is late for. Each
/// stream's own, since what it allows for is the rhythm the stream has yet
/// to see; the session's lateness ([`Controls::late`]) is a floor under it,
/// not added to it. Measured: see `linux/README.md`'s Sound paragraph.
pub const MARGIN_START: Duration = Duration::from_millis(30);

/// How long a stream's sound has to have played without running dry for
/// its margin to come down a [`MARGIN_STEP`] from [`MARGIN_START`]: from
/// 30 ms to 10 in 40 s, each step dropped (faded) where the queue has kept
/// it.
pub const CLEAN_START: Duration = Duration::from_secs(10);

/// The most lateness a session remembers ([`Controls::late`]). Lateness
/// beyond a tenth of a second is a stall rather than a rhythm, and is a gap
/// whatever is kept for it; keeping that much more for the rest of the
/// session would be heard as sound behind the picture. So a queue that ran
/// dry on a write later than this does not remember it at all, nor measure
/// it into the stream's rhythm, and skips what the far end sends to catch
/// up in one jump rather than playing it behind (`Queue`): a SPICE
/// session started under 64 busy loops ran dry 111-411 ms late as its sound
/// began in four of seven tries, and remembering that as this much kept it
/// 70 ms further behind for minutes. (What ran such sessions dry later on
/// was 26-60 ms late.)
pub const MARGIN_MAX: Duration = Duration::from_millis(100);

/// What a session remembers beyond the lateness that ran a queue dry, and
/// what it forgets of it after every [`CLEAN`].
pub const MARGIN_STEP: Duration = Duration::from_millis(5);

/// How long the far end's sound has to have played without running a queue
/// dry before the session forgets a [`MARGIN_STEP`] of the lateness it
/// remembers: longer than a loaded machine went between one late write and
/// the next (18 s, under 64 busy loops), so that a margin that was needed is
/// not given back to be needed again, and short enough that one hiccup does
/// not cost the rest of a session: 100 ms is forgotten in ten minutes.
pub const CLEAN: Duration = Duration::from_secs(30);

/// How much sound the queue and the device hold together before the queue
/// drops the oldest, back down to its target — or twice the target, for a
/// far end whose own rhythm needs more than half of this (Windows' PCM).
/// TigerVNC's viewer kept a second, and a stream that ran even slightly
/// faster than the device lagged further behind the picture for as long as
/// it ran.
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

/// A gain, as a factor: 1 is as sent, 0 is silence. Clamped to 0..=4, since
/// past that a 16-bit sample only clips, and 1 for anything that is not a
/// number. Kept as an `f32`'s bits, so that the device's thread reads it
/// without a lock. The session's ([`Controls::set_gain`]) and each stream's
/// ([`Stream::set_gain`]) are one of these each, over one another.
pub(crate) struct Gain(AtomicU32);

impl Gain {
    pub(crate) fn new() -> Gain {
        Gain(AtomicU32::new(1.0f32.to_bits()))
    }

    pub(crate) fn set(&self, gain: f32) {
        let gain = if gain.is_finite() { gain.clamp(0.0, 4.0) } else { 1.0 };
        self.0.store(gain.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// What a session's sound is played at, which whoever holds the session can
/// change while it plays, from any thread: a gain over every stream, a mute,
/// and whether anything is playing at all — and what the far end can be
/// asked about its sound, which the backend says ([`offer_mute`],
/// [`offer_enable`]) and a window's switch sets ([`set_enabled`]).
///
/// One a session, shared by its streams: a toolbar mutes the session, not
/// one of the far end's streams, and a backend that opens a new stream for
/// a new format (RDP's sound does, per format) must not come back unmuted.
/// The lateness its queues remember ([`late`](Controls::late)) is the
/// session's for the same reason: how late the far end's sound comes is the
/// network's and the machine's, and RDP's next sound is a new stream.
///
/// [`offer_mute`]: Controls::offer_mute
/// [`offer_enable`]: Controls::offer_enable
/// [`set_enabled`]: Controls::set_enabled
pub struct Controls {
    gain: Gain,
    muted: AtomicBool,
    /// The lateness remembered, in microseconds.
    late: AtomicU64,
    playing: Mutex<Playing>,
    offer: Mutex<Offer>,
    /// The backend's way of asking the far end, apart from [`Offer`] so that
    /// reading the state never waits on a backend being asked: held while
    /// it is called, which is what lets [`withdraw`](Controls::withdraw)
    /// promise that it is not called again once that returns.
    enable: Mutex<Option<Enable>>,
    /// Told the session's [`Sound`] whenever it changes. Held while it is
    /// told, so that two changes on two threads are said in the order the
    /// state was read and the last said is the state as it is.
    listener: Mutex<Option<Listener>>,
}

/// How a backend asks its far end for its sound (`true`) or to stop it.
type Enable = Box<dyn Fn(bool) + Send + Sync>;

/// Who is told the session's sound as it changes.
type Listener = Box<dyn Fn(Sound) + Send + Sync>;

struct Playing {
    /// Streams that have been written to and not yet closed, the latest
    /// last: what [`Controls::playback`] reads. Weak, since a queue holds
    /// these controls.
    streams: Vec<Weak<Queue>>,
    /// What the far end's sound arrives as, where it is decoded here.
    codec: Option<Codec>,
}

/// What the backend has said the far end's sound can be asked.
struct Offer {
    switch: Option<Switch>,
    /// Whether the far end is asked for its sound, for [`Switch::Enable`].
    enabled: bool,
}

/// What the session's sound switch does, which is what the far end's
/// protocol lets a client ask of it while the session runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    /// Silences the far end's sound here and nowhere else: its protocol has
    /// no way to ask it to stop sending during a session (RDP's sound), so
    /// the sound keeps arriving and the device plays it silently. Its words
    /// are "Mute audio", on while muted.
    Mute,
    /// Asks the far end to stop sending its sound, and to send it again:
    /// QEMU's audio extension, SPICE's playback channel, RustDesk's option.
    /// Off, nothing arrives and nothing is played. Its words are "Enable
    /// audio", on while the far end is asked for its sound.
    Enable,
}

impl Switch {
    /// The switch's words, the same on every screen that shows it.
    pub fn label(self) -> &'static str {
        match self {
            Switch::Mute => "Mute audio",
            Switch::Enable => "Enable audio",
        }
    }
}

/// The session's sound as a switch shows it, which [`Controls::sound`]
/// answers and the session says as an event whenever it changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sound {
    /// What the switch does, or `None` while nothing says the far end may
    /// send sound: no switch, since it would do nothing. Once a backend has
    /// offered one it stays, whether or not anything plays, until the far
    /// end will send no more (its channel ended, its permission taken away).
    pub switch: Option<Switch>,
    /// Whether the far end is asked for its sound: what an Enable switch is
    /// set to. True for a Mute switch, whose far end always sends.
    pub enabled: bool,
    /// Whether the session's sound is muted here (a Mute switch's state).
    pub muted: bool,
    /// Whether a stream is playing: from the first sound to the last
    /// stream's closing, a couple of seconds after the far end falls quiet.
    pub playing: bool,
}

impl Sound {
    /// Whether the switch is on: an Enable switch while the far end is
    /// asked for its sound, a Mute switch while muted.
    pub fn on(&self) -> bool {
        match self.switch {
            Some(Switch::Enable) => self.enabled,
            Some(Switch::Mute) => self.muted,
            None => false,
        }
    }

    /// Whether the far end's sound is heard here, or would be as soon as it
    /// sent some: not while an Enable switch is off or a Mute switch on.
    pub fn audible(&self) -> bool {
        self.enabled && !self.muted
    }

    /// A debug overlay's line for the sound, where the session has a switch:
    /// what plays — what is queued in front of the device, what the queue
    /// aims to keep there and the margin in that, and the device's own
    /// buffer (named for the output where the platform says which, as
    /// "PipeWire 11 ms"), the queue and the device together how far behind
    /// what arrived it is heard, and what the queue has had to do about the
    /// far end's timing — or that nothing does; and DISABLED while the far
    /// end is asked not to send its sound, MUTED while it is muted here. Both
    /// clients' overlays say it in these words.
    pub fn line(&self, playback: Option<Playback>) -> Option<String> {
        self.switch?;
        let what = match playback {
            Some(p) => format!(
                "{}  queue {} ms, target {} (margin {}) + {} {} ms  underruns {}  dropped {}",
                p.describe(),
                p.queued.as_millis(),
                p.target.as_millis(),
                p.margin.as_millis(),
                p.output.unwrap_or("device"),
                p.device.as_millis(),
                p.stats.underruns,
                p.stats.dropped,
            ),
            None => "nothing playing".to_string(),
        };
        let state = if !self.enabled {
            "  DISABLED"
        } else if self.muted {
            "  MUTED"
        } else {
            ""
        };
        Some(format!("snd {what}{state}"))
    }
}

/// What a session is playing right now, as [`Controls::playback`] answers:
/// the facts row's words and the overlay's numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Playback {
    /// The stream's, which for compressed sound is what it decodes to.
    pub format: Format,
    /// What the far end sends it as; `None` for PCM, sent as it is played.
    pub codec: Option<Codec>,
    /// What is queued in front of the device.
    pub queued: Duration,
    /// What the queue keeps in front of the device, as it is now: see
    /// [`Queue`].
    pub target: Duration,
    /// What the target keeps beyond what the device and the far end's
    /// rhythm need: see [`Queue`].
    pub margin: Duration,
    /// The device's own buffer, as the platform said it when it opened
    /// ([`Queue::set_device`]); zero until then.
    pub device: Duration,
    /// Which of the platform's outputs the device is ([`Queue::set_output`]):
    /// "PipeWire" or "ALSA" on the desktop; `None` where the platform has
    /// only the one and does not say (AAudio).
    pub output: Option<&'static str>,
    pub stats: Stats,
}

impl Playback {
    /// The sound in a few words, "AAC 44.1 kHz stereo", "PCM 48 kHz stereo":
    /// what arrives, rather than what the device was opened at, which the
    /// platform's mixer resamples from anyway.
    pub fn describe(&self) -> String {
        let codec = self.codec.map_or("PCM", Codec::name);
        // 44100 as 44.1, 48000 as 48: f64's shortest display drops the
        // zeros a fixed number of decimals would leave.
        let khz = self.format.rate as f64 / 1000.0;
        let channels = match self.format.channels {
            1 => "mono".to_string(),
            2 => "stereo".to_string(),
            n => format!("{n} channels"),
        };
        format!("{codec} {khz} kHz {channels}")
    }
}

impl Controls {
    pub fn new() -> Controls {
        Controls {
            gain: Gain::new(),
            muted: AtomicBool::new(false),
            late: AtomicU64::new(0),
            playing: Mutex::new(Playing {
                streams: Vec::new(),
                codec: None,
            }),
            offer: Mutex::new(Offer {
                switch: None,
                enabled: true,
            }),
            enable: Mutex::new(None),
            listener: Mutex::new(None),
        }
    }

    /// The gain over every stream of the session, as a factor: 1 is as sent,
    /// 0 is silence. Clamped to 0..=4, since past that a 16-bit sample only
    /// clips.
    pub fn set_gain(&self, gain: f32) {
        self.gain.set(gain);
    }

    pub fn gain(&self) -> f32 {
        self.gain.get()
    }

    /// Silence, at once and until unmuted: what is queued keeps draining
    /// under the mute, so that unmuting is the stream as it is now and not
    /// where it was when it was muted.
    pub fn set_muted(&self, muted: bool) {
        if self.muted.swap(muted, Ordering::Relaxed) != muted {
            self.changed();
        }
    }

    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    /// How late the far end's sound has turned out to be, as the session
    /// remembers it: nothing to start with; whenever a queue ran dry before
    /// the far end's sound arrived, the lateness its writes showed then and
    /// a [`MARGIN_STEP`] more (to [`MARGIN_MAX`] at most, and nothing for a
    /// write later than that, which was a stall); a step less after
    /// every [`CLEAN`] of sound that did not run dry. What a [`Queue`] has
    /// seen of late covers lateness that comes every few seconds; this
    /// covers what comes less often than that, which is what ran it dry. A
    /// queue's margin is [`MARGIN`] and as much of this as its own rhythm
    /// does not already cover, or what is left of the stream's start
    /// ([`MARGIN_START`]) where that is more: the two allow for the same
    /// lateness, so the larger is kept and not both.
    pub fn late(&self) -> Duration {
        Duration::from_micros(self.late.load(Ordering::Relaxed))
    }

    /// A queue ran dry, and the far end's writes, measured with the one that
    /// came after, have been as late as `seen`: what the session remembers
    /// is at least that and a step, and is returned.
    fn ran_dry(&self, seen: Duration) -> Duration {
        let step = MARGIN_STEP.as_micros() as u64;
        let max = MARGIN_MAX.as_micros() as u64;
        let seen = seen.as_micros() as u64;
        let next = |l: u64| l.max(seen).saturating_add(step).min(max);
        let before = self.late.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |l| Some(next(l))).unwrap();
        Duration::from_micros(next(before))
    }

    /// A queue has played [`CLEAN`] of sound without running dry: the
    /// session forgets a step of the lateness it remembers, and the new one
    /// is returned if there was any to forget.
    fn clean(&self) -> Option<Duration> {
        let step = MARGIN_STEP.as_micros() as u64;
        self.late
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |l| (l > 0).then(|| l.saturating_sub(step)))
            .ok()
            .map(|before| Duration::from_micros(before.saturating_sub(step)))
    }

    /// Whether any stream of the session has had sound written to it and is
    /// still open.
    pub fn playing(&self) -> bool {
        !self.playing.lock().unwrap().streams.is_empty()
    }

    /// The backend's: the far end may send sound from now on, and nothing
    /// can ask it to stop during the session, so the session's switch is a
    /// mute ([`Switch::Mute`]). Said as early as the protocol says it — an
    /// RDP server's list of formats, of which this end plays one — so that
    /// the switch is there before the first sound rather than with it.
    pub fn offer_mute(&self) {
        *self.enable.lock().unwrap() = None;
        self.set_offer(Some(Switch::Mute), true);
    }

    /// The backend's: the far end may send sound, and can be asked to stop
    /// and to send it again ([`Switch::Enable`]); `enabled` is whether it is
    /// asked to now. `enable` asks it, and is called whenever the session's
    /// switch is turned ([`set_enabled`](Controls::set_enabled)), on that
    /// thread — a window's — so it hands the question to the protocol's own
    /// thread and returns, and never calls into these controls. Said again,
    /// it replaces what was said before, as when a backend finds its far end
    /// would not do what it was asked.
    pub fn offer_enable(&self, enabled: bool, enable: Box<dyn Fn(bool) + Send + Sync>) {
        *self.enable.lock().unwrap() = Some(enable);
        self.set_offer(Some(Switch::Enable), enabled);
    }

    /// The backend's: the far end will send no more sound in this session —
    /// its sound's channel has ended, its permission is gone, the session is
    /// over — so there is no switch. Once this returns the `enable` handed
    /// to [`offer_enable`](Controls::offer_enable) is not called again, and
    /// is dropped: a backend withdraws before what it captured goes away.
    pub fn withdraw(&self) {
        let enable = self.enable.lock().unwrap().take();
        drop(enable);
        self.set_offer(None, true);
    }

    fn set_offer(&self, switch: Option<Switch>, enabled: bool) {
        {
            let mut offer = self.offer.lock().unwrap();
            offer.switch = switch;
            offer.enabled = enabled;
        }
        self.changed();
    }

    /// The person's switch: asks the far end to send its sound (`on`) or
    /// to stop, where the backend offered that ([`Switch::Enable`]). The
    /// backend is asked only for a change; whether it is (a switch that
    /// does something) is the answer. What was queued plays out or is
    /// dropped as the backend sees fit — every backend here closes its
    /// stream at once when told to stop.
    pub fn set_enabled(&self, on: bool) -> bool {
        {
            let mut offer = self.offer.lock().unwrap();
            if offer.switch != Some(Switch::Enable) || offer.enabled == on {
                return false;
            }
            offer.enabled = on;
        }
        if let Some(enable) = self.enable.lock().unwrap().as_ref() {
            enable(on);
        }
        self.changed();
        true
    }

    /// Whether the far end is asked for its sound: false only while an
    /// Enable switch is off.
    pub fn enabled(&self) -> bool {
        self.offer.lock().unwrap().enabled
    }

    /// The session's sound as a switch shows it. A stream playing with no
    /// switch offered — a backend that never said — is a mute switch, the
    /// one any sound can have.
    pub fn sound(&self) -> Sound {
        let playing = self.playing();
        let offer = self.offer.lock().unwrap();
        Sound {
            switch: offer.switch.or(playing.then_some(Switch::Mute)),
            enabled: offer.enabled || offer.switch != Some(Switch::Enable),
            muted: self.muted(),
            playing,
        }
    }

    /// What is playing, from the stream that started last of those still
    /// open, or `None` while nothing is ([`playing`](Controls::playing)).
    /// Cheap enough for every frame of an overlay: a lock of these controls
    /// and one of the stream's queue, one after the other.
    pub fn playback(&self) -> Option<Playback> {
        let (queue, codec) = {
            let p = self.playing.lock().unwrap();
            (p.streams.iter().rev().find_map(Weak::upgrade)?, p.codec)
        };
        Some(Playback {
            format: queue.format(),
            codec,
            queued: queue.queued(),
            target: queue.target_now(),
            margin: queue.margin_now(),
            device: queue.device_buffer(),
            output: queue.output(),
            stats: queue.stats(),
        })
    }

    /// What the far end's sound arrives as, said by whoever decodes it
    /// (`api`'s table for a C backend, at a decoder's opening and
    /// closing): for [`playback`](Controls::playback)'s words, since the
    /// stream it plays on is handed samples and never sees the codec.
    pub fn set_codec(&self, codec: Option<Codec>) {
        self.playing.lock().unwrap().codec = codec;
    }

    /// `listener` is told the session's [`Sound`] whenever any of it
    /// changes — a stream's first write or last closing on the protocol
    /// thread, an offer from the backend, the switch or the mute from a
    /// window — on the thread that changed it, so it must not block, and
    /// must not set a listener here. The session hands over one that says
    /// it as an event. Replaces any listener before it.
    pub fn on_sound(&self, listener: Box<dyn Fn(Sound) + Send + Sync>) {
        *self.listener.lock().unwrap() = Some(listener);
    }

    fn changed(&self) {
        let listener = self.listener.lock().unwrap();
        if let Some(listener) = listener.as_ref() {
            listener(self.sound());
        }
    }

    /// `queue`'s stream has had its first write (`started`) or has closed
    /// after one.
    fn stream_playing(&self, queue: &Arc<Queue>, started: bool) {
        let changed = {
            let mut p = self.playing.lock().unwrap();
            let before = !p.streams.is_empty();
            let weak = Arc::downgrade(queue);
            if started {
                p.streams.push(weak);
            } else {
                p.streams.retain(|s| !s.ptr_eq(&weak));
            }
            before == p.streams.is_empty()
        };
        if changed {
            self.changed();
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
            self.queue.controls().stream_playing(&self.queue, true);
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if self.playing {
            self.queue.controls().stream_playing(&self.queue, false);
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
        assert_eq!(f.frames(Duration::from_millis(100)), 4800);
        assert_eq!(f.duration(4800), Duration::from_millis(100));
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
        outputs.controls().on_sound(Box::new(move |sound| s.lock().unwrap().push(sound.playing)));
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
    fn a_far_end_that_can_be_asked_has_an_enable_switch_that_asks_it() {
        let controls = Controls::new();
        let said = Arc::new(Mutex::new(Vec::new()));
        let s = Arc::clone(&said);
        controls.on_sound(Box::new(move |sound| s.lock().unwrap().push(sound)));
        // Nothing offered: no switch, and nothing to ask.
        assert_eq!(controls.sound().switch, None);
        assert!(!controls.set_enabled(false));

        let asked = Arc::new(Mutex::new(Vec::new()));
        let a = Arc::clone(&asked);
        controls.offer_enable(false, Box::new(move |on| a.lock().unwrap().push(on)));
        let off = controls.sound();
        assert_eq!((off.switch, off.enabled, off.on(), off.audible()), (Some(Switch::Enable), false, false, false));
        assert_eq!(Switch::Enable.label(), "Enable audio");
        // Only a change is asked for.
        assert!(controls.set_enabled(true));
        assert!(!controls.set_enabled(true));
        assert!(controls.set_enabled(false));
        assert_eq!(*asked.lock().unwrap(), [true, false]);
        assert!(!controls.enabled());

        // Withdrawn, the backend is asked nothing more, and its question is
        // let go.
        controls.withdraw();
        assert!(!controls.set_enabled(true));
        assert_eq!(*asked.lock().unwrap(), [true, false]);
        assert_eq!(Arc::strong_count(&asked), 1);
        let gone = controls.sound();
        assert_eq!((gone.switch, gone.enabled), (None, true));
        let switches: Vec<_> = said.lock().unwrap().iter().map(|s| (s.switch, s.enabled)).collect();
        assert_eq!(
            switches,
            [
                (Some(Switch::Enable), false),
                (Some(Switch::Enable), true),
                (Some(Switch::Enable), false),
                (None, true),
            ]
        );
    }

    #[test]
    fn a_far_end_that_cannot_be_asked_has_a_mute_switch() {
        let outputs = fake();
        let controls = outputs.controls();
        let said = Arc::new(Mutex::new(Vec::new()));
        let s = Arc::clone(&said);
        controls.on_sound(Box::new(move |sound| s.lock().unwrap().push(sound)));
        // A stream playing with nothing offered still has a switch: a mute.
        let mut stream = outputs.open(Format::new(48_000, 2).unwrap()).unwrap();
        stream.write(&[0, 0]);
        assert_eq!(controls.sound().switch, Some(Switch::Mute));
        drop(stream);
        assert_eq!(controls.sound().switch, None);
        // Offered, it stays with nothing playing.
        controls.offer_mute();
        let sound = controls.sound();
        assert_eq!((sound.switch, sound.enabled, sound.playing, sound.on()), (Some(Switch::Mute), true, false, false));
        assert!(!controls.set_enabled(false), "a mute switch asks the far end nothing");
        controls.set_muted(true);
        controls.set_muted(true);
        let sound = controls.sound();
        assert!(sound.on() && !sound.audible());
        assert_eq!(Switch::Mute.label(), "Mute audio");
        let muted: Vec<_> = said.lock().unwrap().iter().map(|s| (s.playing, s.muted)).collect();
        // The mute is said once, not for a repeat.
        assert_eq!(muted, [(true, false), (false, false), (false, false), (false, true)]);
    }

    #[test]
    fn playback_is_the_latest_stream_still_open() {
        let outputs = fake();
        let controls = outputs.controls();
        assert_eq!(controls.playback(), None);
        let mut pcm = outputs.open(Format::new(48_000, 2).unwrap()).unwrap();
        // Opened is not playing, here as for `playing`.
        assert_eq!(controls.playback(), None);
        pcm.write(&[1, 2, 3, 4]);
        let now = controls.playback().unwrap();
        assert_eq!(now.describe(), "PCM 48 kHz stereo");
        assert_eq!(now.queued, Format::new(48_000, 2).unwrap().duration(2));
        assert_eq!(now.device, Duration::ZERO);
        assert_eq!(now.output, None);
        outputs.queues.lock().unwrap()[0].set_device(Duration::from_millis(50), Duration::from_millis(10));
        outputs.queues.lock().unwrap()[0].set_output("ALSA");
        assert_eq!(controls.playback().unwrap().device, Duration::from_millis(50));
        assert_eq!(controls.playback().unwrap().output, Some("ALSA"));

        controls.set_codec(Some(Codec::Aac));
        let mut aac = outputs.open(Format::new(44_100, 2).unwrap()).unwrap();
        aac.write(&[0, 0]);
        assert_eq!(controls.playback().unwrap().describe(), "AAC 44.1 kHz stereo");
        drop(aac);
        controls.set_codec(None);
        // Back to the one still open.
        assert_eq!(controls.playback().unwrap().describe(), "PCM 48 kHz stereo");
        drop(pcm);
        assert_eq!(controls.playback(), None);
    }

    #[test]
    fn playback_says_rates_and_channels_as_people_do() {
        let say = |rate, channels, codec| {
            Playback {
                format: Format::new(rate, channels).unwrap(),
                codec,
                queued: Duration::ZERO,
                target: Duration::ZERO,
                margin: Duration::ZERO,
                device: Duration::ZERO,
                output: None,
                stats: Stats::default(),
            }
            .describe()
        };
        assert_eq!(say(22_050, 1, None), "PCM 22.05 kHz mono");
        assert_eq!(say(8_000, 6, None), "PCM 8 kHz 6 channels");
        assert_eq!(say(48_000, 2, Some(Codec::Opus)), "Opus 48 kHz stereo");
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

    #[test]
    fn the_overlay_says_the_sound_and_whether_it_is_off_or_muted() {
        let sound = |switch, enabled, muted| Sound {
            switch,
            enabled,
            muted,
            playing: false,
        };
        assert_eq!(sound(None, true, false).line(None), None);
        assert_eq!(sound(Some(Switch::Enable), false, false).line(None).unwrap(), "snd nothing playing  DISABLED");
        let playback = Playback {
            format: Format::new(48_000, 2).unwrap(),
            codec: None,
            queued: Duration::from_millis(32),
            target: Duration::from_millis(45),
            margin: MARGIN,
            device: Duration::from_millis(50),
            output: None,
            stats: Stats::default(),
        };
        assert_eq!(
            sound(Some(Switch::Mute), true, true).line(Some(playback)).unwrap(),
            "snd PCM 48 kHz stereo  queue 32 ms, target 45 (margin 10) + device 50 ms  underruns 0  dropped 0  MUTED"
        );
        assert_eq!(
            sound(Some(Switch::Enable), true, false).line(Some(playback)).unwrap(),
            "snd PCM 48 kHz stereo  queue 32 ms, target 45 (margin 10) + device 50 ms  underruns 0  dropped 0"
        );
        // Where the platform says which output plays it, that is its name.
        let piped = Playback {
            output: Some("PipeWire"),
            device: Duration::from_millis(11),
            ..playback
        };
        assert_eq!(
            sound(Some(Switch::Mute), true, false).line(Some(piped)).unwrap(),
            "snd PCM 48 kHz stereo  queue 32 ms, target 45 (margin 10) + PipeWire 11 ms  underruns 0  dropped 0"
        );
    }
}
