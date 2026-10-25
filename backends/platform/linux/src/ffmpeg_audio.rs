// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A desktop's sound decoders, libavcodec's AAC and Opus: [`FFmpegAudio`] is
//! this platform's `runtime::media::audio::decode::Decoders`, which [`Audio`]
//! hands out with its streams, and the counterpart of
//! `platform_android::mediacodec_audio`.
//!
//! Through `cpp/vento_ffaudio.h`, as the video is through `cpp/vento_video.h`:
//! five functions declared by hand, and the conversion to interleaved 16-bit
//! samples done on the C side, where the decoder's own sample format is
//! known. The FFmpeg is the client's one, the same build its video decodes
//! with (`build.rs`); FreeRDP's sound is decoded here rather than by the
//! library's own FFmpeg path.
//!
//! [`Audio`]: crate::audio::Audio

use std::ffi::{CStr, c_char, c_int};

use runtime::media::audio::Format;
use runtime::media::audio::decode::{self, Codec, Decoder, Decoders};

/// `cpp/vento_ffaudio.h`, declared by hand.
mod sys {
    use std::ffi::{c_char, c_int};

    #[repr(C)]
    pub struct Audio {
        _private: [u8; 0],
    }

    unsafe extern "C" {
        pub fn vento_ffaudio_decodable(codec: c_int) -> c_int;
        pub fn vento_ffaudio_open(
            codec: c_int,
            rate: c_int,
            channels: c_int,
            config: *const u8,
            config_len: usize,
            error: *mut c_char,
            error_len: usize,
        ) -> *mut Audio;
        pub fn vento_ffaudio_name(audio: *const Audio) -> *const c_char;
        pub fn vento_ffaudio_decode(
            audio: *mut Audio,
            data: *const u8,
            len: usize,
            samples: *mut *const i16,
            rate: *mut c_int,
            channels: *mut c_int,
            error: *mut c_char,
            error_len: usize,
        ) -> c_int;
        pub fn vento_ffaudio_close(audio: *mut Audio);
    }
}

/// The C side's number for a codec: `enum vento_ffaudio_codec`.
fn number(codec: Codec) -> c_int {
    match codec {
        Codec::Aac => 1,
        Codec::Opus => 2,
    }
}

fn message(buf: &[c_char]) -> String {
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

/// libavcodec's sound decoders, as a session's outputs hand them out.
pub struct FFmpegAudio;

impl Decoders for FFmpegAudio {
    fn decodable(&self, codec: Codec) -> bool {
        unsafe { sys::vento_ffaudio_decodable(number(codec)) != 0 }
    }

    fn decoder(&self, codec: Codec, format: Format, config: &[u8]) -> Result<Box<dyn Decoder>, String> {
        let config = decode::config(codec, format, config)
            .ok_or_else(|| format!("{} has no setup for {} Hz in {} channels", codec.name(), format.rate, format.channels))?;
        let mut error = [0 as c_char; 256];
        let audio = unsafe {
            sys::vento_ffaudio_open(
                number(codec),
                format.rate as c_int,
                format.channels as c_int,
                config.as_ptr(),
                config.len(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if audio.is_null() {
            return Err(format!("libavcodec would not open {}: {}", codec.name(), message(&error)));
        }
        Ok(Box::new(Audio(audio)))
    }
}

/// One libavcodec decoder, closed when dropped.
struct Audio(*mut sys::Audio);

// The decoder is libavcodec's context and two buffers of ours, used from one
// thread at a time as `Decoder` promises, never from two at once.
unsafe impl Send for Audio {}

impl Drop for Audio {
    fn drop(&mut self) {
        unsafe { sys::vento_ffaudio_close(self.0) };
    }
}

impl Decoder for Audio {
    fn decode(&mut self, packet: &[u8], out: &mut Vec<i16>) -> Result<Option<Format>, String> {
        let mut samples: *const i16 = std::ptr::null();
        let (mut rate, mut channels): (c_int, c_int) = (0, 0);
        let mut error = [0 as c_char; 256];
        let frames = unsafe {
            sys::vento_ffaudio_decode(
                self.0,
                packet.as_ptr(),
                packet.len(),
                &mut samples,
                &mut rate,
                &mut channels,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if frames < 0 {
            return Err(message(&error));
        }
        if frames == 0 || samples.is_null() {
            return Ok(None);
        }
        let format = Format::new(rate.max(0) as u32, channels.max(0) as usize)
            .ok_or_else(|| format!("the decoder made {rate} Hz in {channels} channels"))?;
        let samples = unsafe { std::slice::from_raw_parts(samples, frames as usize * format.channels) };
        out.extend_from_slice(samples);
        Ok(Some(format))
    }

    fn name(&self) -> String {
        let name = unsafe { CStr::from_ptr(sys::vento_ffaudio_name(self.0)) };
        format!("{} (libavcodec)", name.to_string_lossy())
    }
}
