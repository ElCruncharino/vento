// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The platform's sound output as `vento/audio.h`'s table, for a C backend
//! whose far end sends sound: TigerVNC's QEMU audio and FreeRDP's rdpsnd,
//! which until now each had a sink of their own on each platform.
//!
//! What that buys is the same thing [`video`](super::video) buys for
//! pictures: one queue and one policy for every backend on both platforms —
//! how far ahead sound is kept, what is dropped when it gets too far ahead,
//! the device opened off the protocol thread — and the session's mute,
//! which a window sets on the [`Outputs`]' controls without the backend
//! hearing of it.
//!
//! The platform's decoders ride along in the same table
//! ([`Outputs::decoders`]): a backend whose far end sends AAC or Opus
//! (FreeRDP's rdpsnd) offers what they decode and hands the packets here, so
//! the decoding is the platform's on both platforms rather than whatever the
//! library would have linked.

use std::ffi::{c_int, c_void};
use std::sync::Arc;

use common::audio::decode::{Codec, Decoder};
use common::audio::{Controls, Format, Outputs, Stream};

use super::ffi;

/// `outputs` as the table a backend's config carries. The table's `user` is
/// a box holding its own count of `outputs`, which is the backend's from the
/// moment the table is handed over and comes back through `release`.
pub fn table(outputs: Arc<dyn Outputs>) -> ffi::vento_audio {
    ffi::vento_audio {
        user: Box::into_raw(Box::new(outputs)) as *mut c_void,
        open: Some(open),
        write: Some(write),
        set_gain: Some(set_gain),
        latency: Some(latency),
        close: Some(close),
        decodable: Some(decodable),
        decoder_open: Some(decoder_open),
        decode: Some(decode),
        decoder_close: Some(decoder_close),
        offer: Some(offer),
        withdraw: Some(withdraw),
        release: Some(release),
    }
}

/// The codec a C caller names, by `VENTO_AUDIO_CODEC_*`.
fn codec(number: c_int) -> Option<Codec> {
    match number as u32 {
        ffi::VENTO_AUDIO_CODEC_AAC => Some(Codec::Aac),
        ffi::VENTO_AUDIO_CODEC_OPUS => Some(Codec::Opus),
        _ => None,
    }
}

unsafe extern "C" fn decodable(user: *mut c_void, number: c_int) -> bool {
    let outputs = unsafe { &*(user as *const Arc<dyn Outputs>) };
    match (codec(number), outputs.decoders()) {
        (Some(codec), Some(decoders)) => decoders.decodable(codec),
        _ => false,
    }
}

/// A decoder behind a handle, with the samples of its last packet: what
/// `decode` lends the caller until its next call.
struct Decoding {
    decoder: Box<dyn Decoder>,
    samples: Vec<i16>,
    /// The session's, told the codec while this decodes, for the facts row
    /// (the stream the samples go to never sees it).
    controls: Arc<Controls>,
}

impl Drop for Decoding {
    fn drop(&mut self) {
        self.controls.set_codec(None);
    }
}

unsafe extern "C" fn decoder_open(
    user: *mut c_void,
    number: c_int,
    rate: c_int,
    channels: c_int,
    config: *const u8,
    config_len: usize,
) -> *mut ffi::vento_audio_decoder {
    let outputs = unsafe { &*(user as *const Arc<dyn Outputs>) };
    let Some(codec) = codec(number) else {
        return std::ptr::null_mut();
    };
    let (Ok(rate), Ok(channels)) = (u32::try_from(rate), usize::try_from(channels)) else {
        return std::ptr::null_mut();
    };
    let Some(format) = Format::new(rate, channels) else {
        log::warn!("audio: no {} decoder for {rate} Hz in {channels} channels", codec.name());
        return std::ptr::null_mut();
    };
    let Some(decoders) = outputs.decoders() else {
        return std::ptr::null_mut();
    };
    let config = if config.is_null() || config_len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(config, config_len) }
    };
    match decoders.decoder(codec, format, config) {
        Ok(decoder) => {
            log::info!("audio: decoding {} at {rate} Hz in {channels} channels with {}", codec.name(), decoder.name());
            let controls = Arc::clone(outputs.controls());
            controls.set_codec(Some(codec));
            Box::into_raw(Box::new(Decoding {
                decoder,
                samples: Vec::new(),
                controls,
            })) as *mut ffi::vento_audio_decoder
        }
        Err(why) => {
            log::warn!("audio: no {} decoder: {why}", codec.name());
            std::ptr::null_mut()
        }
    }
}

unsafe extern "C" fn decode(
    d: *mut ffi::vento_audio_decoder,
    data: *const u8,
    len: usize,
    samples: *mut *const i16,
    rate: *mut c_int,
    channels: *mut c_int,
) -> c_int {
    if d.is_null() || samples.is_null() || rate.is_null() || channels.is_null() {
        return -1;
    }
    // The handle is the caller's alone for the call, as the header says.
    let d = unsafe { &mut *(d as *mut Decoding) };
    d.samples.clear();
    unsafe { *samples = d.samples.as_ptr() };
    if data.is_null() || len == 0 {
        return 0;
    }
    let packet = unsafe { std::slice::from_raw_parts(data, len) };
    match d.decoder.decode(packet, &mut d.samples) {
        Ok(Some(format)) => {
            unsafe {
                *samples = d.samples.as_ptr();
                *rate = format.rate as c_int;
                *channels = format.channels as c_int;
            }
            (d.samples.len() / format.channels).min(c_int::MAX as usize) as c_int
        }
        Ok(None) => 0,
        Err(why) => {
            log::debug!("audio: a packet would not decode: {why}");
            -1
        }
    }
}

unsafe extern "C" fn decoder_close(d: *mut ffi::vento_audio_decoder) {
    if !d.is_null() {
        drop(unsafe { Box::from_raw(d as *mut Decoding) });
    }
}

unsafe extern "C" fn open(user: *mut c_void, format: c_int, rate: c_int, channels: c_int) -> *mut ffi::vento_audio_stream {
    let outputs = unsafe { &*(user as *const Arc<dyn Outputs>) };
    if format as u32 != ffi::VENTO_AUDIO_S16 {
        return std::ptr::null_mut();
    }
    let (Ok(rate), Ok(channels)) = (u32::try_from(rate), usize::try_from(channels)) else {
        return std::ptr::null_mut();
    };
    match Format::new(rate, channels).and_then(|f| outputs.open(f)) {
        Some(stream) => Box::into_raw(Box::new(stream)) as *mut ffi::vento_audio_stream,
        None => std::ptr::null_mut(),
    }
}

/// The stream behind a handle, this thread's for the call.
///
/// # Safety
///
/// `s` came from [`open`] and is not closed, and nothing else is inside a
/// call on it, as the header says. A [`Stream`] is `Send`, so the thread may
/// differ from call to call.
unsafe fn stream<'a>(s: *mut ffi::vento_audio_stream) -> &'a mut Stream {
    unsafe { &mut *(s as *mut Stream) }
}

unsafe extern "C" fn write(s: *mut ffi::vento_audio_stream, data: *const c_void, len: usize) {
    if s.is_null() || data.is_null() || len == 0 {
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, len) };
    unsafe { stream(s) }.write_bytes(bytes);
}

unsafe extern "C" fn set_gain(s: *mut ffi::vento_audio_stream, gain: f32) {
    if !s.is_null() {
        unsafe { stream(s) }.set_gain(gain);
    }
}

unsafe extern "C" fn latency(s: *mut ffi::vento_audio_stream) -> c_int {
    if s.is_null() {
        return 0;
    }
    unsafe { stream(s) }.latency().as_millis().min(c_int::MAX as u128) as c_int
}

unsafe extern "C" fn close(s: *mut ffi::vento_audio_stream) {
    if !s.is_null() {
        drop(unsafe { Box::from_raw(s as *mut Stream) });
    }
}

/// A backend's `ctx`, carried to whichever thread turns the switch: the
/// header has the backend keep it alive until it withdraws, and the call is
/// the backend's to make safe on any thread.
struct Ctx(*mut c_void);

unsafe impl Send for Ctx {}
unsafe impl Sync for Ctx {}

unsafe extern "C" fn offer(
    user: *mut c_void,
    switch: c_int,
    enabled: bool,
    enable: Option<unsafe extern "C" fn(*mut c_void, bool)>,
    ctx: *mut c_void,
) {
    if user.is_null() {
        return;
    }
    let controls = unsafe { &*(user as *const Arc<dyn Outputs>) }.controls();
    match (switch as u32, enable) {
        (ffi::VENTO_AUDIO_SWITCH_ENABLE, Some(enable)) => {
            let ctx = Ctx(ctx);
            controls.offer_enable(
                enabled,
                Box::new(move |on| {
                    let ctx = &ctx;
                    unsafe { enable(ctx.0, on) }
                }),
            );
        }
        // A mute, or an enable with no way to ask: either way the far end
        // sends, and the switch can only silence it here.
        _ => controls.offer_mute(),
    }
}

unsafe extern "C" fn withdraw(user: *mut c_void) {
    if !user.is_null() {
        unsafe { &*(user as *const Arc<dyn Outputs>) }.controls().withdraw();
    }
}

unsafe extern "C" fn release(user: *mut c_void) {
    if !user.is_null() {
        let outputs = unsafe { Box::from_raw(user as *mut Arc<dyn Outputs>) };
        // The backend's `ctx` goes with it, whether or not it withdrew.
        outputs.controls().withdraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::audio::decode::Decoders;
    use common::audio::{Controls, Device, Queue};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    struct Quiet;

    impl Device for Quiet {
        fn latency(&self) -> Duration {
            Duration::from_millis(7)
        }
    }

    struct Fake {
        controls: Arc<Controls>,
        queues: Mutex<Vec<Arc<Queue>>>,
        dropped: Arc<AtomicBool>,
    }

    impl Outputs for Fake {
        fn controls(&self) -> &Arc<Controls> {
            &self.controls
        }

        fn device(&self, queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
            self.queues.lock().unwrap().push(queue);
            Ok(Box::new(Quiet))
        }

        fn decoders(&self) -> Option<&dyn Decoders> {
            Some(&Doubler)
        }
    }

    /// Decodes AAC alone: each byte of a packet is a frame of two samples,
    /// the byte and its double, at twice the stream's rate (as SBR is); a
    /// packet of zeros is an error, and a packet of one byte is held back.
    struct Doubler;

    struct Doubling {
        format: Format,
        config: Vec<u8>,
    }

    impl Decoders for Doubler {
        fn decodable(&self, codec: Codec) -> bool {
            codec == Codec::Aac
        }

        fn decoder(&self, codec: Codec, format: Format, config: &[u8]) -> Result<Box<dyn Decoder>, String> {
            if codec != Codec::Aac {
                return Err("only AAC".into());
            }
            Ok(Box::new(Doubling {
                format: Format::new(format.rate * 2, 2).unwrap(),
                config: config.to_vec(),
            }))
        }
    }

    impl Decoder for Doubling {
        fn decode(&mut self, packet: &[u8], out: &mut Vec<i16>) -> Result<Option<Format>, String> {
            if packet.iter().all(|&b| b == 0) {
                return Err("zeros".into());
            }
            if packet.len() == 1 {
                return Ok(None);
            }
            out.extend(packet.iter().flat_map(|&b| [b as i16 + self.config.len() as i16, b as i16 * 2]));
            Ok(Some(self.format))
        }

        fn name(&self) -> String {
            "doubler".into()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_stream_goes_through_the_table() {
        let dropped = Arc::new(AtomicBool::new(false));
        let fake = Arc::new(Fake {
            controls: Arc::new(Controls::new()),
            queues: Mutex::new(Vec::new()),
            dropped: Arc::clone(&dropped),
        });
        let t = table(Arc::clone(&fake) as Arc<dyn Outputs>);
        let s16 = ffi::VENTO_AUDIO_S16 as c_int;
        unsafe {
            assert!(t.open.unwrap()(t.user, 99, 48_000, 2).is_null(), "no such format");
            assert!(t.open.unwrap()(t.user, s16, 1, 2).is_null(), "no such rate");
            assert!(t.open.unwrap()(t.user, s16, 48_000, 0).is_null(), "no channels");
            let s = t.open.unwrap()(t.user, s16, 48_000, 2);
            assert!(!s.is_null());
            assert!(!fake.controls.playing());

            // 100 ms of stereo, written three bytes at a time.
            let bytes: Vec<u8> = (0..4800 * 2).flat_map(|i: i32| (i as i16).to_ne_bytes()).collect();
            for chunk in bytes.chunks(3) {
                t.write.unwrap()(s, chunk.as_ptr().cast(), chunk.len());
            }
            t.write.unwrap()(s, std::ptr::null(), 0);
            assert!(fake.controls.playing());
            assert_eq!(t.latency.unwrap()(s), 107);

            let queue = Arc::clone(&fake.queues.lock().unwrap()[0]);
            let format = queue.format();
            t.set_gain.unwrap()(s, 0.5);
            let mut out = vec![0i16; 8];
            // The 100 ms landed at once, and the queue's target is all of
            // that and its margin (common::audio): it plays what it has once
            // it has waited a target's worth, from the first frame written.
            let mut silent = 0;
            loop {
                queue.pull(&mut out, format);
                if out != [0; 8] {
                    break;
                }
                silent += 1;
                assert!(silent < 10_000, "never played");
            }
            assert!(silent > 0, "played before the target was queued or waited for");
            assert_eq!(out, [0, 0, 1, 1, 2, 2, 3, 3]);
            queue.pull(&mut out, format);
            // Samples 8 to 15 at half: 4, 4, 5, 5, ...
            assert_eq!(out, [4, 4, 5, 5, 6, 6, 7, 7]);

            t.close.unwrap()(s);
            assert!(!fake.controls.playing());
            drop(fake);
            assert!(!dropped.load(Ordering::Relaxed));
            t.release.unwrap()(t.user);
        }
        assert!(dropped.load(Ordering::Relaxed));
    }

    #[test]
    fn a_packet_goes_through_the_tables_decoder() {
        let controls = Arc::new(Controls::new());
        let fake = Arc::new(Fake {
            controls: Arc::clone(&controls),
            queues: Mutex::new(Vec::new()),
            dropped: Arc::new(AtomicBool::new(false)),
        });
        let t = table(Arc::clone(&fake) as Arc<dyn Outputs>);
        let aac = ffi::VENTO_AUDIO_CODEC_AAC as c_int;
        let opus = ffi::VENTO_AUDIO_CODEC_OPUS as c_int;
        unsafe {
            assert!(t.decodable.unwrap()(t.user, aac));
            assert!(!t.decodable.unwrap()(t.user, opus));
            assert!(!t.decodable.unwrap()(t.user, 99));
            assert!(t.decoder_open.unwrap()(t.user, opus, 48_000, 2, std::ptr::null(), 0).is_null());
            assert!(t.decoder_open.unwrap()(t.user, aac, 1, 2, std::ptr::null(), 0).is_null(), "no such rate");

            let config = [0x12u8, 0x10, 0x56];
            let d = t.decoder_open.unwrap()(t.user, aac, 22_050, 2, config.as_ptr(), config.len());
            assert!(!d.is_null());
            let (mut samples, mut rate, mut channels) = (std::ptr::null(), 0, 0);
            let packet = [1u8, 2, 3];
            let frames = t.decode.unwrap()(d, packet.as_ptr(), packet.len(), &mut samples, &mut rate, &mut channels);
            assert_eq!((frames, rate, channels), (3, 44_100, 2));
            assert_eq!(std::slice::from_raw_parts(samples, 6), [4, 2, 5, 4, 6, 6], "the config reached it");

            // Held back, broken, and nothing at all: no frames, and the
            // decoder still decodes after them.
            assert_eq!(t.decode.unwrap()(d, [9u8].as_ptr(), 1, &mut samples, &mut rate, &mut channels), 0);
            assert_eq!(t.decode.unwrap()(d, [0u8, 0].as_ptr(), 2, &mut samples, &mut rate, &mut channels), -1);
            assert_eq!(t.decode.unwrap()(d, std::ptr::null(), 0, &mut samples, &mut rate, &mut channels), 0);
            assert_eq!(t.decode.unwrap()(d, [7u8, 8].as_ptr(), 2, &mut samples, &mut rate, &mut channels), 2);
            assert_eq!(std::slice::from_raw_parts(samples, 4), [10, 14, 11, 16]);

            // What the decoded samples play as is said to be AAC for as
            // long as the decoder is open.
            let mut stream = fake.open(Format::new(44_100, 2).unwrap()).unwrap();
            stream.write(&[1, 2]);
            assert_eq!(controls.playback().unwrap().describe(), "AAC 44.1 kHz stereo");
            t.decoder_close.unwrap()(d);
            assert_eq!(controls.playback().unwrap().describe(), "PCM 44.1 kHz stereo");
            drop(stream);
            t.release.unwrap()(t.user);
        }
    }

    #[test]
    fn the_switch_goes_through_the_table() {
        use common::audio::Switch;
        use std::sync::atomic::AtomicI32;

        // What a backend's ctx would be: here, a count of what it was asked.
        static ASKED: AtomicI32 = AtomicI32::new(0);
        unsafe extern "C" fn enable(ctx: *mut c_void, on: bool) {
            assert_eq!(ctx as usize, 42);
            ASKED.fetch_add(if on { 10 } else { 1 }, Ordering::Relaxed);
        }

        let controls = Arc::new(Controls::new());
        let fake = Arc::new(Fake {
            controls: Arc::clone(&controls),
            queues: Mutex::new(Vec::new()),
            dropped: Arc::new(AtomicBool::new(false)),
        });
        let t = table(fake as Arc<dyn Outputs>);
        let ctx = 42usize as *mut c_void;
        unsafe {
            t.offer.unwrap()(t.user, ffi::VENTO_AUDIO_SWITCH_MUTE as c_int, true, None, std::ptr::null_mut());
            assert_eq!(controls.sound().switch, Some(Switch::Mute));
            t.offer.unwrap()(t.user, ffi::VENTO_AUDIO_SWITCH_ENABLE as c_int, false, Some(enable), ctx);
            assert_eq!((controls.sound().switch, controls.enabled()), (Some(Switch::Enable), false));
            assert!(controls.set_enabled(true));
            assert!(controls.set_enabled(false));
            assert_eq!(ASKED.load(Ordering::Relaxed), 11);
            t.withdraw.unwrap()(t.user);
            assert_eq!(controls.sound().switch, None);
            assert!(!controls.set_enabled(true));
            // Offered again and released without a withdraw: the release
            // withdraws.
            t.offer.unwrap()(t.user, ffi::VENTO_AUDIO_SWITCH_ENABLE as c_int, true, Some(enable), ctx);
            t.release.unwrap()(t.user);
        }
        assert_eq!(controls.sound().switch, None);
        assert!(!controls.set_enabled(false));
        assert_eq!(ASKED.load(Ordering::Relaxed), 11);
    }
}
