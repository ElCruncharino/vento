// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The phone's sound output, through the NDK's AAudio: [`AAudio`] is this
//! platform's `media::audio::Outputs`.
//!
//! Declared here rather than taken from a crate, for the reason
//! [`mediacodec`](crate::mediacodec) gives: a couple of dozen functions with
//! a stable ABI, and `#[link]` on the extern block is the whole of the build
//! configuration. Everything used is from API 26 or 28, and the app's
//! minimum is 34, so nothing is looked up at run time.
//!
//! **A data callback**, rather than the non-blocking writes TigerVNC's old
//! sink (`AndroidAudioOutput`, which this replaced) made: AAudio asks for each buffer on a thread of its own, and the
//! callback fills it from the stream's queue, which is exactly the shape the
//! queue was made for — the protocol thread only ever queues, and the
//! callback never waits on anything but the queue's copy. Opening the stream
//! takes a trip to the audio server, so it is done on a thread started for
//! it, and closing (which waits for a callback in flight) likewise.
//!
//! **A disconnected stream is opened again**, off the callback: AAudio says
//! so through the error callback when the output moves (headphones, a car,
//! Bluetooth), and the stream is dead from then on. AAudio forbids closing a
//! stream from its own callback, so a thread is started for it, as the
//! AAudio documentation and Oboe do, and that thread opens the replacement
//! on whatever the output is now. A few times per stream; an output that
//! keeps failing is given up on rather than retried for ever.

use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;
use std::sync::{Arc, Mutex};

use media::audio::decode::Decoders;
use media::audio::{Controls, Device, Format, Outputs, Queue};

use crate::mediacodec_audio::MediaCodecAudio;

#[repr(C)]
struct AAudioStreamBuilder {
    _private: [u8; 0],
}

#[repr(C)]
struct AAudioStream {
    _private: [u8; 0],
}

type DataCallback = unsafe extern "C" fn(*mut AAudioStream, *mut c_void, *mut c_void, i32) -> i32;
type ErrorCallback = unsafe extern "C" fn(*mut AAudioStream, *mut c_void, i32);

const AAUDIO_OK: i32 = 0;
const AAUDIO_DIRECTION_OUTPUT: i32 = 0;
const AAUDIO_FORMAT_PCM_I16: i32 = 1;
const AAUDIO_USAGE_MEDIA: i32 = 1;
const AAUDIO_CONTENT_TYPE_MUSIC: i32 = 2;
const AAUDIO_CALLBACK_RESULT_CONTINUE: i32 = 0;

#[link(name = "aaudio")]
unsafe extern "C" {
    fn AAudio_createStreamBuilder(builder: *mut *mut AAudioStreamBuilder) -> i32;
    fn AAudio_convertResultToText(result: i32) -> *const c_char;
    fn AAudioStreamBuilder_setDirection(builder: *mut AAudioStreamBuilder, direction: i32);
    fn AAudioStreamBuilder_setFormat(builder: *mut AAudioStreamBuilder, format: i32);
    fn AAudioStreamBuilder_setChannelCount(builder: *mut AAudioStreamBuilder, channels: i32);
    fn AAudioStreamBuilder_setSampleRate(builder: *mut AAudioStreamBuilder, rate: i32);
    fn AAudioStreamBuilder_setUsage(builder: *mut AAudioStreamBuilder, usage: i32);
    fn AAudioStreamBuilder_setContentType(builder: *mut AAudioStreamBuilder, content: i32);
    fn AAudioStreamBuilder_setDataCallback(builder: *mut AAudioStreamBuilder, callback: DataCallback, user: *mut c_void);
    fn AAudioStreamBuilder_setErrorCallback(builder: *mut AAudioStreamBuilder, callback: ErrorCallback, user: *mut c_void);
    fn AAudioStreamBuilder_openStream(builder: *mut AAudioStreamBuilder, stream: *mut *mut AAudioStream) -> i32;
    fn AAudioStreamBuilder_delete(builder: *mut AAudioStreamBuilder) -> i32;
    fn AAudioStream_requestStart(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_requestStop(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_close(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getSampleRate(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getChannelCount(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getBufferSizeInFrames(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getFramesPerBurst(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getFramesRead(stream: *mut AAudioStream) -> i64;
}

/// How many times a stream is opened again after its output went away.
const REOPENS: u32 = 3;

fn text(result: i32) -> String {
    unsafe { CStr::from_ptr(AAudio_convertResultToText(result)) }.to_string_lossy().into_owned()
}

/// The phone's output, as a session is handed it: one a session, for its
/// controls.
pub struct AAudio {
    controls: Arc<Controls>,
}

impl AAudio {
    pub fn new() -> AAudio {
        AAudio {
            controls: Arc::new(Controls::new()),
        }
    }
}

impl Default for AAudio {
    fn default() -> AAudio {
        AAudio::new()
    }
}

impl Outputs for AAudio {
    fn controls(&self) -> &Arc<Controls> {
        &self.controls
    }

    /// MediaCodec's, the phone's own.
    fn decoders(&self) -> Option<&dyn Decoders> {
        Some(&MediaCodecAudio)
    }

    fn device(&self, queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
        let shared = Arc::new(Shared {
            queue,
            state: Mutex::new(State {
                open: None,
                closed: false,
                reopens: 0,
            }),
            latency_us: AtomicU64::new(0),
        });
        let theirs = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("vento-aaudio".into())
            .spawn(move || open(&theirs))
            .map_err(|e| e.to_string())?;
        Ok(Box::new(AAudioDevice { shared }))
    }
}

/// What a stream, its callbacks and the threads that open and close it
/// share.
struct Shared {
    queue: Arc<Queue>,
    state: Mutex<State>,
    latency_us: AtomicU64,
}

struct State {
    /// The stream while it is open.
    open: Option<Open>,
    /// The session's stream has gone: nothing more is opened, and what is
    /// open is closed by whoever finds it.
    closed: bool,
    reopens: u32,
}

/// An open AAudio stream and what its callbacks were handed, which lives
/// until the stream is closed: AAudio calls no callback after `close`
/// returns.
struct Open {
    stream: *mut AAudioStream,
    context: *mut Context,
}

// The stream handle is AAudio's, whose calls are any thread's; it is only
// ever used under the state's lock or by the one thread that took it out.
unsafe impl Send for Open {}

impl Open {
    /// Stops and closes the stream, which waits for a callback in flight —
    /// so never on the protocol thread, nor on AAudio's own.
    fn close(self) {
        unsafe {
            AAudioStream_requestStop(self.stream);
            AAudioStream_close(self.stream);
            drop(Box::from_raw(self.context));
        }
    }
}

/// What the callbacks are handed: the queue to fill from, the format the
/// stream opened at, which the queue converts to, and what the device holds
/// and takes at once, which the data callback keeps the queue told of.
struct Context {
    shared: Arc<Shared>,
    format: Format,
    buffered: Duration,
    burst: u64,
    /// AAudio's count of the frames its mixer has read, as the data
    /// callback last saw it, and the period the queue was last told, in
    /// frames.
    read: AtomicI64,
    period: AtomicU64,
}

/// The stream's hold on its AAudio stream. Dropping it closes the stream on
/// a thread of its own, without waiting.
struct AAudioDevice {
    shared: Arc<Shared>,
}

impl Device for AAudioDevice {
    fn latency(&self) -> std::time::Duration {
        std::time::Duration::from_micros(self.shared.latency_us.load(Ordering::Relaxed))
    }
}

impl Drop for AAudioDevice {
    fn drop(&mut self) {
        let open = {
            let mut state = self.shared.state.lock().unwrap();
            state.closed = true;
            state.open.take()
        };
        if let Some(open) = open
            && std::thread::Builder::new().name("vento-aaudio".into()).spawn(move || open.close()).is_err()
        {
            // No thread to close it on: the stream is leaked rather than
            // closed here, which could wait on a callback.
            log::warn!("audio: could not start a thread to close the output stream");
        }
    }
}

/// Opens a stream for `shared`'s queue and starts it, on the thread started
/// for that. The stream's own format first; where AAudio refuses it, its
/// own rate in stereo, which the queue converts to.
fn open(shared: &Arc<Shared>) {
    let format = shared.queue.format();
    let mut result = 0;
    for (rate, channels) in [(format.rate as i32, format.channels as i32), (0, 2)] {
        match try_open(shared, rate, channels) {
            Ok(open) => {
                let mut state = shared.state.lock().unwrap();
                if state.closed {
                    drop(state);
                    open.close();
                } else {
                    state.open = Some(open);
                }
                return;
            }
            Err(r) => result = r,
        }
    }
    log::warn!("audio: could not open an output stream ({}); the far end's sound is dropped", text(result));
}

fn try_open(shared: &Arc<Shared>, rate: i32, channels: i32) -> Result<Open, i32> {
    let mut builder = std::ptr::null_mut();
    let result = unsafe { AAudio_createStreamBuilder(&mut builder) };
    if result != AAUDIO_OK {
        return Err(result);
    }
    // Filled in with the stream's real format once it is open, before it
    // starts: no callback comes before `requestStart`.
    let context = Box::into_raw(Box::new(Context {
        shared: Arc::clone(shared),
        format: shared.queue.format(),
        buffered: Duration::ZERO,
        burst: 0,
        read: AtomicI64::new(0),
        period: AtomicU64::new(0),
    }));
    let mut stream = std::ptr::null_mut();
    let result = unsafe {
        AAudioStreamBuilder_setDirection(builder, AAUDIO_DIRECTION_OUTPUT);
        AAudioStreamBuilder_setFormat(builder, AAUDIO_FORMAT_PCM_I16);
        AAudioStreamBuilder_setChannelCount(builder, channels);
        if rate > 0 {
            AAudioStreamBuilder_setSampleRate(builder, rate);
        }
        AAudioStreamBuilder_setUsage(builder, AAUDIO_USAGE_MEDIA);
        AAudioStreamBuilder_setContentType(builder, AAUDIO_CONTENT_TYPE_MUSIC);
        // The performance mode is left as it is (none): the queue in front
        // keeps a few tens of milliseconds anyway, and a low-latency
        // stream's small bursts would only wake the phone more often for
        // nothing.
        AAudioStreamBuilder_setDataCallback(builder, data, context.cast());
        AAudioStreamBuilder_setErrorCallback(builder, error, context.cast());
        let result = AAudioStreamBuilder_openStream(builder, &mut stream);
        AAudioStreamBuilder_delete(builder);
        result
    };
    if result != AAUDIO_OK {
        drop(unsafe { Box::from_raw(context) });
        return Err(result);
    }
    let open = Open { stream, context };
    let (rate, channels, buffer, burst) = unsafe {
        (
            AAudioStream_getSampleRate(stream),
            AAudioStream_getChannelCount(stream),
            AAudioStream_getBufferSizeInFrames(stream),
            AAudioStream_getFramesPerBurst(stream),
        )
    };
    let Some(device) = Format::new(rate.max(0) as u32, channels.max(0) as usize) else {
        open.close();
        return Err(-1);
    };
    let buffered = device.duration(buffer.max(0) as usize);
    let burst = burst.max(0) as u64;
    unsafe {
        (*context).format = device;
        (*context).buffered = buffered;
        (*context).burst = burst;
        (*context).period = AtomicU64::new(burst);
    }
    shared.latency_us.store(buffered.as_micros() as u64, Ordering::Relaxed);
    // AAudio keeps its buffer topped up from the callback, delay behind the
    // queue; and each callback asks for a burst, which the queue has to have
    // left over — until the data callback has seen what the mixer reads,
    // which is the most the queue is asked for at once (see `data`).
    shared.queue.set_device(buffered, device.duration(burst as usize));
    let result = unsafe { AAudioStream_requestStart(stream) };
    if result != AAUDIO_OK {
        open.close();
        return Err(result);
    }
    let format = shared.queue.format();
    log::info!(
        "audio: playing {} Hz in {} channels on AAudio at {} Hz in {} (buffer {buffer} frames, burst {burst})",
        format.rate,
        format.channels,
        device.rate,
        device.channels
    );
    Ok(open)
}

/// AAudio asking for `frames` frames, on its own thread.
///
/// The queue's period is what the mixer reads at a time rather than a
/// burst: on the Pixel 8 (Android 17) this was measured on, a Legacy stream
/// (what a stream that does not ask for low latency is) is read by the
/// mixer two bursts at a time — 1920 frames every 40 ms, refilled in two or
/// three callbacks within 1.5 ms — from the first read after the buffer is
/// filled on. A queue told a burst started with a millisecond or less to
/// spare after that first read, and ran dry 0.3 s in on a write 3 ms late;
/// it would again wherever it starts over, after running dry or a pause.
/// The first read comes before the callback that starts the queue (it is
/// what frees the room that callback fills), so the queue has the period
/// from the start.
unsafe extern "C" fn data(stream: *mut AAudioStream, user: *mut c_void, audio: *mut c_void, frames: i32) -> i32 {
    let context = unsafe { &*(user as *const Context) };
    let read = unsafe { AAudioStream_getFramesRead(stream) };
    let step = read - context.read.swap(read, Ordering::Relaxed);
    if step > 0 {
        let period = (step as u64).max(context.burst);
        if context.period.swap(period, Ordering::Relaxed) != period {
            context.shared.queue.set_device(context.buffered, context.format.duration(period as usize));
        }
    }
    let samples = frames.max(0) as usize * context.format.channels;
    let out = unsafe { std::slice::from_raw_parts_mut(audio as *mut i16, samples) };
    context.shared.queue.pull(out, context.format);
    AAUDIO_CALLBACK_RESULT_CONTINUE
}

/// AAudio saying the stream is dead, on its own thread, where the stream
/// must not be closed: a thread is started to close it and open another.
unsafe extern "C" fn error(stream: *mut AAudioStream, user: *mut c_void, result: i32) {
    let context = unsafe { &*(user as *const Context) };
    let shared = Arc::clone(&context.shared);
    // Only compared, never called through: which stream failed, as a number
    // a thread may carry.
    let failed = stream as usize;
    log::info!("audio: the output stream stopped ({}); opening another", text(result));
    let spawned = std::thread::Builder::new().name("vento-aaudio".into()).spawn(move || {
        let (open, again) = {
            let mut state = shared.state.lock().unwrap();
            // The stream that failed, unless it has been closed already.
            let open = match &state.open {
                Some(o) if o.stream as usize == failed => state.open.take(),
                _ => None,
            };
            let again = open.is_some() && !state.closed && state.reopens < REOPENS;
            if again {
                state.reopens += 1;
            }
            (open, again)
        };
        let Some(open) = open else { return };
        open.close();
        if again {
            self::open(&shared);
        } else {
            log::warn!("audio: the output stream stopped working ({}); the far end's sound is dropped", text(result));
        }
    });
    if spawned.is_err() {
        log::warn!("audio: could not start a thread to open the output stream again");
    }
}
