// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop's sound output: ALSA's `default` device, which on a desktop
//! with a sound server is that server's (PipeWire's ALSA plugin, here), as
//! this platform's `common::audio::Outputs`.
//!
//! Declared here rather than taken from a crate, as `ffmpeg.rs` declares
//! its C: a dozen functions of `libasound.so.2`, whose ABI has not changed
//! in twenty years, linked by name. It is the one library the Linux client
//! loads for sound: TigerVNC's QEMU audio and FreeRDP's rdpsnd play through
//! this (handed to their sessions as `vento/audio.h`'s table), and neither
//! `libvento_tigervnc.so`, whose own ALSA sink this replaced, nor
//! `libvento_freerdp.so`, whose PulseAudio device it replaced, links a sound
//! library at all.
//!
//! **A thread per stream does everything that can wait.** Opening the
//! device is a round trip to the sound server, which can take a while and,
//! with a server that has hung, forever; writing to it blocks until the
//! device has room. Both happen on the stream's own thread, which takes
//! from the stream's queue a period at a time, so the protocol thread only
//! ever queues. That is the off-thread open TigerVNC's old
//! `AlsaAudioOutput` did, carried over, with the policy it kept per sink (a
//! second's buffer, a pre-roll) replaced by the queue's: its non-blocking
//! writes into that second's buffer filled it with whatever arrived at once
//! (a pause's backlog, a late read), and a stream that the device drained
//! no faster than the server sent kept that delay for good — a session
//! paused for a moment on the rig played everything a second late from
//! then on.
//!
//! The device's own buffer is kept short — 50 ms requested — since the
//! queue in front of it is what absorbs the network's jitter; and the queue
//! is told what the device ended up with, and keeps that much less of the
//! target itself, so that the two together are the target. The device is fed
//! silence while the queue has nothing to play, so it never runs dry and
//! never has to be restarted mid-stream; a stream that is open is a device
//! that is running.

use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use common::audio::{Controls, Device, Format, Outputs, Queue};

#[repr(C)]
struct SndPcm {
    _private: [u8; 0],
}

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
/// `SND_PCM_FORMAT_S16`, which ALSA's header defines as the native one of
/// `S16_LE` (2) and `S16_BE` (3).
#[cfg(target_endian = "little")]
const SND_PCM_FORMAT_S16: c_int = 2;
#[cfg(target_endian = "big")]
const SND_PCM_FORMAT_S16: c_int = 3;

#[link(name = "asound")]
unsafe extern "C" {
    fn snd_pcm_open(pcm: *mut *mut SndPcm, name: *const c_char, stream: c_int, mode: c_int) -> c_int;
    fn snd_pcm_set_params(
        pcm: *mut SndPcm,
        format: c_int,
        access: c_int,
        channels: c_uint,
        rate: c_uint,
        soft_resample: c_int,
        latency_us: c_uint,
    ) -> c_int;
    fn snd_pcm_get_params(pcm: *mut SndPcm, buffer_size: *mut c_ulong, period_size: *mut c_ulong) -> c_int;
    fn snd_pcm_writei(pcm: *mut SndPcm, buffer: *const c_void, frames: c_ulong) -> c_long;
    fn snd_pcm_recover(pcm: *mut SndPcm, err: c_int, silent: c_int) -> c_int;
    fn snd_pcm_delay(pcm: *mut SndPcm, frames: *mut c_long) -> c_int;
    fn snd_pcm_drop(pcm: *mut SndPcm) -> c_int;
    fn snd_pcm_close(pcm: *mut SndPcm) -> c_int;
    fn snd_strerror(err: c_int) -> *const c_char;
}

/// What ALSA calls the device the sound server stands behind, on every
/// desktop that has one.
const DEVICE: &CStr = c"default";

/// The device buffer asked for; see the module's comment.
const DEVICE_LATENCY: Duration = Duration::from_millis(50);

/// How many times a stream's thread opens the device again after it failed
/// or stopped working, a second apart, before the stream goes without sound
/// for the rest of its life. A sound server restarting is back well within
/// that; one that is not coming back is not worth a thread that never ends.
const ATTEMPTS: u32 = 3;

fn error(err: c_int) -> String {
    unsafe { CStr::from_ptr(snd_strerror(err)) }.to_string_lossy().into_owned()
}

/// ALSA's default device, as a session is handed it: one a session, for its
/// controls.
pub struct Alsa {
    controls: Arc<Controls>,
}

impl Alsa {
    pub fn new() -> Alsa {
        Alsa {
            controls: Arc::new(Controls::new()),
        }
    }
}

impl Default for Alsa {
    fn default() -> Alsa {
        Alsa::new()
    }
}

impl Outputs for Alsa {
    fn controls(&self) -> &Arc<Controls> {
        &self.controls
    }

    fn device(&self, queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            latency_us: AtomicU64::new(0),
        });
        let theirs = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("vento-audio".into())
            .spawn(move || run(&queue, &theirs))
            .map_err(|e| e.to_string())?;
        Ok(Box::new(AlsaDevice { shared }))
    }
}

/// What a stream and its thread share: the stream says stop, the thread
/// says how far behind the device is.
struct Shared {
    stop: AtomicBool,
    latency_us: AtomicU64,
}

/// The stream's hold on its thread. Dropping it asks the thread to stop and
/// does not wait: the thread notices within a period, or, if it is still
/// opening the device, once the open returns, and closes what it has.
struct AlsaDevice {
    shared: Arc<Shared>,
}

impl Device for AlsaDevice {
    fn latency(&self) -> Duration {
        Duration::from_micros(self.shared.latency_us.load(Ordering::Relaxed))
    }
}

impl Drop for AlsaDevice {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// An open PCM, closed when dropped.
struct Pcm(*mut SndPcm);

impl Drop for Pcm {
    fn drop(&mut self) {
        unsafe {
            snd_pcm_drop(self.0);
            snd_pcm_close(self.0);
        }
    }
}

/// The device, set up for `format` where it takes it — and where it does
/// not, at the format nearest it that it does, which the queue converts to.
/// No resampling of ALSA's own is allowed: the plug layer's would be a second
/// resampler in front of the sound server's, where the queue's is only ever
/// the one there is.
fn open(format: Format) -> Result<(Pcm, Format), String> {
    let mut pcm = std::ptr::null_mut();
    let err = unsafe { snd_pcm_open(&mut pcm, DEVICE.as_ptr(), SND_PCM_STREAM_PLAYBACK, 0) };
    if err < 0 {
        return Err(format!("could not open ALSA's default device: {}", error(err)));
    }
    let pcm = Pcm(pcm);
    let mut tried = Vec::new();
    for channels in [format.channels, 2] {
        for rate in [format.rate, 48_000, 44_100] {
            let Some(candidate) = Format::new(rate, channels) else { continue };
            if tried.contains(&candidate) {
                continue;
            }
            tried.push(candidate);
            let err = unsafe {
                snd_pcm_set_params(
                    pcm.0,
                    SND_PCM_FORMAT_S16,
                    SND_PCM_ACCESS_RW_INTERLEAVED,
                    channels as c_uint,
                    rate,
                    0,
                    DEVICE_LATENCY.as_micros() as c_uint,
                )
            };
            if err >= 0 {
                return Ok((pcm, candidate));
            }
        }
    }
    Err(format!(
        "ALSA's default device takes none of {} Hz, 48 kHz or 44.1 kHz in {} or 2 channels",
        format.rate, format.channels
    ))
}

/// The stream's thread: open, play until told to stop, and open again where
/// the device stops working.
fn run(queue: &Queue, shared: &Shared) {
    let mut attempts = 0;
    while !shared.stop.load(Ordering::Relaxed) {
        attempts += 1;
        let why = match open(queue.format()) {
            Ok((pcm, device)) => match play(&pcm, device, queue, shared) {
                Ok(()) => return,
                Err(why) => why,
            },
            Err(why) => why,
        };
        if attempts >= ATTEMPTS {
            log::warn!("audio: {why}; the far end's sound is dropped");
            return;
        }
        log::warn!("audio: {why}; trying again in a second");
        for _ in 0..10 {
            if shared.stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Feeds the device from the queue, a period at a time, until the stream
/// says stop (`Ok`) or the device stops working (`Err`).
fn play(pcm: &Pcm, device: Format, queue: &Queue, shared: &Shared) -> Result<(), String> {
    let (mut buffer, mut period) = (0, 0);
    let err = unsafe { snd_pcm_get_params(pcm.0, &mut buffer, &mut period) };
    if err < 0 || period == 0 {
        // A period of a hundredth of a second, if the device will not say.
        period = (device.rate / 100) as c_ulong;
    }
    // Writes block until there is room, so the buffer is full from the first
    // few periods on, and is that much of the target the queue need not keep.
    queue.set_device(device.duration(buffer as usize), device.duration(period as usize));
    let format = queue.format();
    log::info!(
        "audio: playing {} Hz in {} channels on ALSA's default device at {} Hz in {} (buffer {buffer} frames, period {period})",
        format.rate,
        format.channels,
        device.rate,
        device.channels
    );
    let period = period as usize;
    let mut samples = vec![0i16; period * device.channels];
    while !shared.stop.load(Ordering::Relaxed) {
        queue.pull(&mut samples, device);
        let mut written = 0;
        while written < period {
            let rest = &samples[written * device.channels..];
            let result = unsafe { snd_pcm_writei(pcm.0, rest.as_ptr().cast(), (period - written) as c_ulong) };
            if result < 0 {
                // The device ran dry (this thread was held up), or the
                // machine slept: recovered here, on the thread that may
                // wait for it, and the period written again.
                let err = unsafe { snd_pcm_recover(pcm.0, result as c_int, 1) };
                if err < 0 {
                    return Err(format!("the output device stopped working: {}", error(err)));
                }
                continue;
            }
            written += result as usize;
        }
        let mut delay: c_long = 0;
        if unsafe { snd_pcm_delay(pcm.0, &mut delay) } == 0 {
            let latency = device.duration(delay.max(0) as usize);
            shared.latency_us.store(latency.as_micros() as u64, Ordering::Relaxed);
        }
    }
    Ok(())
}
