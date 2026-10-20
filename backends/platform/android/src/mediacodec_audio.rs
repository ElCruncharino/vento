// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The phone's sound decoders, MediaCodec's AAC and Opus:
//! [`MediaCodecAudio`] is this platform's
//! `runtime::media::audio::decode::Decoders`, which [`AAudio`] hands out with
//! its streams, and the counterpart of `platform_linux::ffmpeg_audio`.
//!
//! Through the same NDK calls as the video ([`mediacodec`]), in their
//! synchronous form and on the caller's thread: a packet of sound is a
//! millisecond of a decoder's work, and a thread and a queue of our own
//! around it would only add a hop. What MediaCodec wants and a protocol does
//! not send is made up from the stream's format
//! (`runtime::media::audio::decode`): AAC's AudioSpecificConfig as `csd-0`, and
//! for Opus its identification header as `csd-0`, the pre-skip and the seek
//! pre-roll in nanoseconds as `csd-1` and `csd-2`, which is how MediaCodec's
//! Opus decoder is told them.
//!
//! **No call waits long.** A packet goes in as soon as there is an input
//! buffer for it (a few milliseconds at most; a decoder that has none has
//! what it finished taken from it, which frees one, and is asked again, and
//! one that still has none is a dropped packet rather than a stalled
//! channel), and what comes out is
//! whatever the decoder has finished by then, waiting a moment for the first
//! of it: a decoder that hands a frame back a packet late has it collected
//! on the next call, a packet's worth later, rather than the caller waiting
//! for it.
//!
//! [`AAudio`]: crate::aaudio::AAudio
//! [`mediacodec`]: crate::mediacodec

use std::ffi::{CString, c_void};
use std::sync::OnceLock;

use runtime::media::audio::Format;
use runtime::media::audio::decode::{self, Codec, Decoder, Decoders, opus_pre_skip};

use crate::mediacodec::{
    AMEDIA_OK, AMediaCodec_dequeueInputBuffer, AMediaCodec_dequeueOutputBuffer, AMediaCodec_getInputBuffer,
    AMediaCodec_getOutputBuffer, AMediaCodec_getOutputFormat, AMediaCodec_queueInputBuffer,
    AMediaCodec_releaseOutputBuffer, AMediaFormat_delete, AMediaFormat_getInt32, AMediaFormat_setBuffer,
    AMediaFormat_setInt32, BufferInfo, Handle, INFO_OUTPUT_BUFFERS_CHANGED, INFO_OUTPUT_FORMAT_CHANGED, exists,
    is_hardware,
};

/// How long a packet waits for an input buffer, and how long the first of
/// its samples is waited for; see the module's comment.
const INPUT_TIMEOUT_US: i64 = 10_000;
const OUTPUT_TIMEOUT_US: i64 = 2_000;

/// What Opus's decoder is told it may skip after a seek, which is RFC 7845's
/// recommended 80 ms: a protocol never seeks, but MediaCodec asks for it.
const OPUS_SEEK_PREROLL_NS: u64 = 80_000_000;

/// `AudioFormat.ENCODING_PCM_16BIT` and `ENCODING_PCM_FLOAT`, which a
/// decoder's output format names its samples by.
const ENCODING_PCM_16BIT: i32 = 2;
const ENCODING_PCM_FLOAT: i32 = 4;

/// The phone's sound decoders, as a session's outputs hand them out.
pub struct MediaCodecAudio;

fn mime(codec: Codec) -> &'static str {
    match codec {
        Codec::Aac => "audio/mp4a-latm",
        Codec::Opus => "audio/opus",
    }
}

impl Decoders for MediaCodecAudio {
    /// Asked once and remembered, as the video's is.
    fn decodable(&self, codec: Codec) -> bool {
        static PROBED: OnceLock<[bool; Codec::ALL.len()]> = OnceLock::new();
        let probed = PROBED.get_or_init(|| Codec::ALL.map(|codec| exists(mime(codec))));
        probed[Codec::ALL.iter().position(|c| *c == codec).unwrap()]
    }

    fn decoder(&self, codec: Codec, format: Format, config: &[u8]) -> Result<Box<dyn Decoder>, String> {
        let config = decode::config(codec, format, config)
            .ok_or_else(|| format!("{} has no setup for {} Hz in {} channels", codec.name(), format.rate, format.channels))?;
        let mime = CString::new(mime(codec)).unwrap();
        let Some(mut handle) = Handle::create(&mime) else {
            return Err(format!("no {} decoder on this device", codec.name()));
        };
        let started = handle.start(&mime, |f| unsafe {
            AMediaFormat_setInt32(f, c"sample-rate".as_ptr(), format.rate as i32);
            AMediaFormat_setInt32(f, c"channel-count".as_ptr(), format.channels as i32);
            AMediaFormat_setInt32(f, c"pcm-encoding".as_ptr(), ENCODING_PCM_16BIT);
            set_buffer(f, c"csd-0", &config);
            match codec {
                Codec::Aac => AMediaFormat_setInt32(f, c"is-adts".as_ptr(), 0),
                Codec::Opus => {
                    let pre_skip = opus_pre_skip(&config).unwrap_or(0) as u64;
                    set_buffer(f, c"csd-1", &(pre_skip * 1_000_000_000 / 48_000).to_le_bytes());
                    set_buffer(f, c"csd-2", &OPUS_SEEK_PREROLL_NS.to_le_bytes());
                }
            }
        });
        if !started {
            return Err(format!(
                "MediaCodec would not start {} for {} Hz in {} channels",
                codec.name(),
                format.rate,
                format.channels
            ));
        }
        let name = handle.name().unwrap_or_else(|| "unnamed".into());
        let hardware = match is_hardware(&name) {
            Some(true) => ", hardware",
            Some(false) => ", software",
            None => "",
        };
        Ok(Box::new(Audio {
            codec: handle,
            name: format!("{name}{hardware}"),
            // Until the decoder says otherwise: what it was set up for.
            output: format,
            float: false,
            pts_us: 0,
        }))
    }
}

unsafe fn set_buffer(format: *mut crate::mediacodec::AMediaFormat, name: &std::ffi::CStr, data: &[u8]) {
    unsafe { AMediaFormat_setBuffer(format, name.as_ptr(), data.as_ptr() as *const c_void, data.len()) };
}

/// One MediaCodec decoder, stopped and deleted when dropped.
struct Audio {
    codec: Handle,
    name: String,
    /// What the decoder's output is, from its last output format.
    output: Format,
    float: bool,
    /// The next packet's timestamp, which MediaCodec wants and nothing reads.
    pts_us: u64,
}

impl Audio {
    /// The decoder's output format, read when it says it has changed.
    fn output_changed(&mut self) {
        let f = unsafe { AMediaCodec_getOutputFormat(self.codec.raw()) };
        if f.is_null() {
            return;
        }
        let (mut rate, mut channels, mut encoding) = (0i32, 0i32, ENCODING_PCM_16BIT);
        unsafe {
            AMediaFormat_getInt32(f, c"sample-rate".as_ptr(), &mut rate);
            AMediaFormat_getInt32(f, c"channel-count".as_ptr(), &mut channels);
            AMediaFormat_getInt32(f, c"pcm-encoding".as_ptr(), &mut encoding);
            AMediaFormat_delete(f);
        }
        if let Some(format) = Format::new(rate.max(0) as u32, channels.max(0) as usize) {
            self.output = format;
        }
        self.float = encoding == ENCODING_PCM_FLOAT;
    }
}

impl Audio {
    /// What the decoder has finished, appended to `out` in one format (the
    /// first buffer's, which `format` keeps): waiting up to `wait` for the
    /// first buffer and not at all for the rest.
    fn drain(&mut self, out: &mut Vec<i16>, format: &mut Option<Format>, wait: i64) {
        let mut timeout = wait;
        loop {
            let mut info = BufferInfo::default();
            let index = unsafe { AMediaCodec_dequeueOutputBuffer(self.codec.raw(), &mut info, timeout) };
            if index == INFO_OUTPUT_FORMAT_CHANGED {
                self.output_changed();
                continue;
            }
            if index == INFO_OUTPUT_BUFFERS_CHANGED {
                continue;
            }
            if index < 0 {
                break;
            }
            timeout = 0;
            let mut size = 0usize;
            let data = unsafe { AMediaCodec_getOutputBuffer(self.codec.raw(), index as usize, &mut size) };
            let (offset, len) = (info.offset.max(0) as usize, info.size.max(0) as usize);
            // Samples of one format: a change between two buffers of a
            // packet returns what came before it, and the rest is lost.
            if !data.is_null() && offset + len <= size && format.is_none_or(|f| f == self.output) {
                let bytes = unsafe { std::slice::from_raw_parts(data.add(offset), len) };
                if self.float {
                    out.extend(
                        bytes
                            .chunks_exact(4)
                            .map(|b| decode::f32_to_s16(f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))),
                    );
                } else {
                    out.extend(bytes.chunks_exact(2).map(|b| i16::from_ne_bytes([b[0], b[1]])));
                }
                *format = Some(self.output);
            }
            unsafe { AMediaCodec_releaseOutputBuffer(self.codec.raw(), index as usize, false) };
        }
    }

    /// The samples appended since `before`, in whole frames, and the clock
    /// moved on by them: the packet's length in time is not known until it
    /// is decoded, so the clock moves by what came out, and by a
    /// microsecond when nothing did, to keep it moving forward.
    fn finish(&mut self, out: &mut Vec<i16>, before: usize, format: Option<Format>) -> Option<Format> {
        let frames = (out.len() - before) / self.output.channels;
        self.pts_us += (frames as u64 * 1_000_000 / self.output.rate as u64).max(1);
        // A partial frame never comes out of a decoder, but an odd buffer is
        // not trusted to keep the samples in step with the channels.
        out.truncate(before + frames * self.output.channels);
        format.filter(|_| frames > 0)
    }
}

impl Decoder for Audio {
    fn decode(&mut self, packet: &[u8], out: &mut Vec<i16>) -> Result<Option<Format>, String> {
        let before = out.len();
        let mut format = None;
        let mut index = unsafe { AMediaCodec_dequeueInputBuffer(self.codec.raw(), INPUT_TIMEOUT_US) };
        if index < 0 {
            // Every input buffer can be held by a decoder with nowhere to put
            // what it decoded: its output buffers all waiting to be taken, and
            // only a call that got an input buffer took them — so one packet
            // too many while the outputs were full was a decoder that took
            // nothing ever after. What it has finished is taken now, without
            // waiting, which frees it to take input, and then once more.
            self.drain(out, &mut format, 0);
            index = unsafe { AMediaCodec_dequeueInputBuffer(self.codec.raw(), INPUT_TIMEOUT_US) };
            if index < 0 {
                // The packet is dropped, and what was taken is still sound.
                let taken = self.finish(out, before, format);
                return match taken {
                    Some(_) => Ok(taken),
                    None => Err("the decoder has no input buffer free".into()),
                };
            }
        }
        let mut capacity = 0usize;
        let buffer = unsafe { AMediaCodec_getInputBuffer(self.codec.raw(), index as usize, &mut capacity) };
        let take = packet.len().min(if buffer.is_null() { 0 } else { capacity });
        if take > 0 {
            unsafe { std::ptr::copy_nonoverlapping(packet.as_ptr(), buffer, take) };
        }
        let queued = unsafe { AMediaCodec_queueInputBuffer(self.codec.raw(), index as usize, 0, take, self.pts_us, 0) };
        if queued != AMEDIA_OK {
            return Err(format!("the decoder took no packet ({queued})"));
        }
        if take < packet.len() {
            return Err(format!("a {}-byte packet in a {capacity}-byte buffer", packet.len()));
        }
        let wait = if format.is_some() { 0 } else { OUTPUT_TIMEOUT_US };
        self.drain(out, &mut format, wait);
        Ok(self.finish(out, before, format))
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}
