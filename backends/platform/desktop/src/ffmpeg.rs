// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A desktop's decoder, through libavcodec's C API: [`FFmpeg`] is this
//! platform's `common::video::Decoders`, and the counterpart of
//! `platform_android::mediacodec` call for call.
//!
//! The same contract as the phone's, because it is the same trait and the same
//! [`Output`] after it: a frame in, the rows that moved converted into the
//! caller's buffer, and nothing else written. What differs is only how the
//! planes are got at:
//!
//! - **Three allocations, not one.** libavcodec hands back a data pointer and a
//!   line size per plane, each in a buffer of its own, which is what
//!   `convert::Frame` takes three slices for. Nothing is copied to make them
//!   look like MediaCodec's single buffer.
//! - **Low delay, and slice threads only.** A decoder with frame threads holds
//!   one frame per thread before the first comes out, and H.264 without
//!   `LOW_DELAY` holds frames back to reorder them — a desktop that lags the
//!   finger on it, for a stream that has nothing to reorder. dav1d reads the
//!   same flag as a frame delay of one.
//! - **AV1 by name.** libavcodec's own `av1` decoder is a front for hardware
//!   acceleration and decodes nothing without it, so AV1 is dav1d, then
//!   libaom, and otherwise not on offer at all — rather than a decoder that
//!   opens and then fails every frame.
//!
//! Colour follows the frame, as MediaCodec's follows the buffer's format: the
//! two BT.601 matrices are taken for BT.601, BT.709 for BT.709 (and for
//! anything else that names itself), and BT.601 again for a stream that says
//! nothing — which is what the encoders this has met produce untagged, and
//! what libavcodec's own scaler takes silence for. A RustDesk peer converts
//! its desktop with libyuv's `ARGBToI420`, BT.601 limited range, and hands
//! that to libvpx and libaom with no colour description at all; VP8's
//! bitstream has nowhere to put one, and libavcodec reports its one colour
//! space, BT.601, as unspecified too.
//!
//! **Deeper than eight bits** — a High 10 H.264 stream, or VP9's and AV1's
//! profiles that carry ten or twelve — is narrowed to eight bits plane by plane
//! into buffers the decoder keeps, and then converted like any other frame,
//! rather than refused: a picture a little less precise than the stream is
//! better than none, and a refusal is a session with no picture for good and an
//! info row that still names the codec.
//!
//! **H.264 stays libavcodec's**, though two decoders without FFmpeg were tried
//! in its place (October 2026): the `openh264` crate, Cisco's library, and
//! `rusty_h264`, which is pure Rust. On what the rigs send, RustDesk's VAAPI
//! High profile and wayvnc's Constrained Baseline, both were bit-exact with
//! libavcodec, and neither crashed or panicked on 3000 truncated and bit-flipped
//! streams. Neither was better at anything, though. Both took 1.5 to 2 times as
//! long a frame at 1920x1200 (openh264's threads segfault). Both refuse High 10,
//! 4:2:2, 4:4:4 and interlaced streams, which libavcodec decodes. openh264 is not
//! bit-exact on B-frames. Neither reports the stream's colour, which the rule
//! above is made from. And FFmpeg stays for the other four codecs anyway, so no
//! dependency would go: a second H.264 decoder would only have been more code,
//! and from source it has no Cisco patent licence either.

use std::ffi::{CStr, c_int};
use std::ptr;
use std::sync::OnceLock;

use common::video::convert::{Frame, Plane};
use common::video::{Changes, Codec, Decoders, Output};
use ffmpeg_sys_next as ff;

/// libavcodec's decoders, as a session is handed them.
pub struct FFmpeg;

impl Decoders for FFmpeg {
    /// Looked up once and remembered: the answer is the library's, and which
    /// one is loaded cannot change under a running process.
    fn decodable(&self, codec: Codec) -> bool {
        static FOUND: OnceLock<[bool; Codec::ALL.len()]> = OnceLock::new();
        let found = FOUND.get_or_init(|| Codec::ALL.map(|c| find(c).is_some()));
        found[Codec::ALL.iter().position(|c| *c == codec).unwrap()]
    }

    fn decoder(
        &self,
        codec: Codec,
        width: usize,
        height: usize,
    ) -> Option<Box<dyn common::video::Decoder>> {
        Decoder::new(codec, width, height).map(|d| Box::new(d) as Box<dyn common::video::Decoder>)
    }
}

/// The decoder libavcodec would use for this codec, or `None` where the build
/// it was linked against has none that decodes in software.
fn find(codec: Codec) -> Option<*const ff::AVCodec> {
    let id = match codec {
        Codec::Vp9 => ff::AVCodecID::AV_CODEC_ID_VP9,
        Codec::Vp8 => ff::AVCodecID::AV_CODEC_ID_VP8,
        Codec::H264 => ff::AVCodecID::AV_CODEC_ID_H264,
        Codec::H265 => ff::AVCodecID::AV_CODEC_ID_HEVC,
        Codec::Av1 => {
            return [c"libdav1d", c"libaom-av1"].into_iter().find_map(|name| {
                let found = unsafe { ff::avcodec_find_decoder_by_name(name.as_ptr()) };
                (!found.is_null()).then_some(found)
            });
        }
    };
    let found = unsafe { ff::avcodec_find_decoder(id) };
    (!found.is_null()).then_some(found)
}

/// One libavcodec decoder for one codec, which is what [`FFmpeg::decoder`]
/// hands out boxed — and what a caller that wants [`skipped`](Self::skipped)
/// can make directly.
pub struct Decoder {
    context: *mut ff::AVCodecContext,
    packet: *mut ff::AVPacket,
    /// The frame being converted, and the one a newer picture is received
    /// into while it waits — two, so that only the last of several is read.
    frame: *mut ff::AVFrame,
    newer: *mut ff::AVFrame,
    pts: i64,
    failed: bool,
    /// The last frame's planes and the vectorised conversion's verdict, which
    /// between them are how little of this one is converted.
    output: Output,
    /// A deeper-than-eight-bit frame's planes, narrowed: kept rather than
    /// allocated per frame, since a stream that is one is one throughout.
    narrow: [Vec<u8>; 3],
    /// Frames received and dropped unread, because a newer whole picture came
    /// out of the same packet behind them.
    pub skipped: u64,
}

impl Decoder {
    pub fn new(codec: Codec, width: usize, height: usize) -> Option<Decoder> {
        let Some(found) = find(codec) else {
            log::warn!("no {} decoder in this libavcodec", codec.name());
            return None;
        };
        let name = unsafe { CStr::from_ptr((*found).name) }.to_string_lossy();
        let context = unsafe { ff::avcodec_alloc_context3(found) };
        if context.is_null() {
            return None;
        }
        // Built before anything can fail, so that `Drop` is the one place any
        // of it is freed.
        let decoder = Decoder {
            context,
            packet: unsafe { ff::av_packet_alloc() },
            frame: unsafe { ff::av_frame_alloc() },
            newer: unsafe { ff::av_frame_alloc() },
            pts: 0,
            failed: false,
            output: Output::new(),
            narrow: Default::default(),
            skipped: 0,
        };
        if decoder.packet.is_null() || decoder.frame.is_null() || decoder.newer.is_null() {
            return None;
        }
        let opened = unsafe {
            // A hint rather than a promise: the stream says what size it is,
            // and what is read is the frame's own.
            (*context).width = width as c_int;
            (*context).height = height as c_int;
            (*context).flags |= ff::AV_CODEC_FLAG_LOW_DELAY as c_int;
            (*context).thread_type = ff::FF_THREAD_SLICE;
            (*context).thread_count = 0; // as many as the machine has
            ff::avcodec_open2(context, found, ptr::null_mut())
        };
        if opened < 0 {
            log::warn!(
                "could not open {name} for {} at {width}x{height}: {}",
                codec.name(),
                error(opened)
            );
            return None;
        }
        log::info!(
            "{} decoder ({name}) started for {width}x{height}",
            codec.name()
        );
        Some(decoder)
    }

    /// One encoded frame in. libavcodec copies a packet it does not own into
    /// one with the padding its bitstream readers need, so `data` is lent
    /// rather than kept.
    fn feed(&mut self, data: &[u8]) -> bool {
        let Ok(size) = c_int::try_from(data.len()) else {
            return false;
        };
        let sent = unsafe {
            (*self.packet).data = data.as_ptr() as *mut u8;
            (*self.packet).size = size;
            (*self.packet).pts = self.pts;
            let sent = ff::avcodec_send_packet(self.context, self.packet);
            (*self.packet).data = ptr::null_mut();
            (*self.packet).size = 0;
            sent
        };
        self.pts += 1;
        if sent < 0 {
            // A frame the decoder refuses is one picture lost rather than a
            // session: the next key frame starts it again.
            log::warn!("a {} byte frame was refused: {}", data.len(), error(sent));
            return false;
        }
        true
    }

    /// Every frame the decoder has ready, keeping the last. False when there
    /// was none.
    ///
    /// **Only the last one is converted**, for the reason MediaCodec's is: each
    /// is a whole picture, and one the next overwrites before anybody sees it
    /// is a conversion for nothing.
    fn receive(&mut self) -> bool {
        let mut got = false;
        loop {
            let received = unsafe { ff::avcodec_receive_frame(self.context, self.newer) };
            if received < 0 {
                if received != ff::AVERROR(ff::EAGAIN) && received != ff::AVERROR_EOF {
                    log::warn!("the decoder failed a frame: {}", error(received));
                }
                return got;
            }
            if got {
                self.skipped += 1;
            }
            unsafe {
                ff::av_frame_unref(self.frame);
                ff::av_frame_move_ref(self.frame, self.newer);
            }
            got = true;
        }
    }

    /// The received frame into `out`, and what moved in it into `changed`.
    fn convert(&mut self, out: &mut [u32], width: usize, height: usize, changed: &mut Changes) {
        use ff::AVColorRange::AVCOL_RANGE_JPEG;
        use ff::AVColorSpace::{AVCOL_SPC_BT470BG, AVCOL_SPC_SMPTE170M, AVCOL_SPC_UNSPECIFIED};
        use ff::AVPixelFormat::{AV_PIX_FMT_YUVJ420P, AV_PIX_FMT_YUVJ422P, AV_PIX_FMT_YUVJ444P};

        let f = unsafe { &*self.frame };
        // The context's format is the frame's for a decoder that is not
        // handing back hardware surfaces, and is typed where the frame's is a
        // bare integer.
        let format = unsafe { (*self.context).pix_fmt };
        let Some((bytes, planes)) = planes(f).or_else(|| narrowed(f, format, &mut self.narrow))
        else {
            let name = unsafe { ff::av_get_pix_fmt_name(format) };
            let name = if name.is_null() {
                "an unknown format".into()
            } else {
                unsafe { CStr::from_ptr(name) }.to_string_lossy()
            };
            log::warn!("the decoder's output is {name}, which is not YUV this can convert");
            self.failed = true;
            return;
        };
        let jpeg = [
            AV_PIX_FMT_YUVJ420P,
            AV_PIX_FMT_YUVJ422P,
            AV_PIX_FMT_YUVJ444P,
        ]
        .iter()
        .any(|fmt| f.format == *fmt as c_int);
        let bt601 = matches!(
            f.colorspace,
            AVCOL_SPC_BT470BG | AVCOL_SPC_SMPTE170M | AVCOL_SPC_UNSPECIFIED
        );
        let frame = Frame {
            bytes,
            planes,
            width: f.width as usize,
            height: f.height as usize,
            bt709: !bt601,
            full_range: f.color_range == AVCOL_RANGE_JPEG || jpeg,
        };
        self.output.write(&frame, out, width, height, changed);
    }
}

impl common::video::Decoder for Decoder {
    fn decode(
        &mut self,
        data: &[u8],
        out: &mut [u32],
        width: usize,
        height: usize,
        changed: &mut Changes,
    ) -> bool {
        if self.failed || data.is_empty() || out.len() < width * height {
            return false;
        }
        if !self.feed(data) || !self.receive() {
            return false;
        }
        self.convert(out, width, height, changed);
        unsafe { ff::av_frame_unref(self.frame) };
        !self.failed
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            ff::av_frame_free(&mut self.newer);
            ff::av_frame_free(&mut self.frame);
            ff::av_packet_free(&mut self.packet);
            ff::avcodec_free_context(&mut self.context);
        }
    }
}

/// A received frame's planes, as `convert` reads them, where they can be read
/// in place. `None` for a format that is not 8-bit YUV, which is
/// [`narrowed`]'s to try next.
fn planes(f: &ff::AVFrame) -> Option<([&[u8]; 3], [Plane; 3])> {
    use ff::AVPixelFormat::*;
    let (width, height) = (f.width.max(0) as usize, f.height.max(0) as usize);
    if width == 0 || height == 0 {
        return None;
    }
    let format = f.format;
    let is = |fmt: ff::AVPixelFormat| format == fmt as c_int;
    // Chroma subsampling, and whether U and V share a plane.
    let (horiz, vert, interleaved) = if is(AV_PIX_FMT_YUV420P) || is(AV_PIX_FMT_YUVJ420P) {
        (2, 2, false)
    } else if is(AV_PIX_FMT_YUV422P) || is(AV_PIX_FMT_YUVJ422P) {
        (2, 1, false)
    } else if is(AV_PIX_FMT_YUV444P) || is(AV_PIX_FMT_YUVJ444P) {
        (1, 1, false)
    } else if is(AV_PIX_FMT_NV12) || is(AV_PIX_FMT_NV21) {
        (2, 2, true)
    } else {
        return None;
    };
    let (cw, ch) = (width.div_ceil(horiz), height.div_ceil(vert));

    // Exactly the bytes the picture covers in each plane: every row but the
    // last at its full line size, and the last only as far as its last
    // sample, since that is all the frame promises is there.
    let plane = |i: usize, rows: usize, row_bytes: usize| -> Option<&[u8]> {
        let stride = usize::try_from(f.linesize[i])
            .ok()
            .filter(|s| *s >= row_bytes)?;
        if f.data[i].is_null() {
            return None;
        }
        let len = stride * (rows - 1) + row_bytes;
        // SAFETY: a decoded plane has `linesize` bytes for every row of the
        // picture, and the last row as far as its last sample is the least of
        // that. The frame is not unreferenced until `convert` is done with it.
        Some(unsafe { std::slice::from_raw_parts(f.data[i], len) })
    };
    let luma = Plane {
        offset: 0,
        col_inc: 1,
        row_inc: f.linesize[0] as usize,
        horiz_subsampling: 1,
        vert_subsampling: 1,
    };
    let y = plane(0, height, width)?;
    if interleaved {
        let uv = plane(1, ch, cw * 2)?;
        let at = |offset: usize| Plane {
            offset,
            col_inc: 2,
            row_inc: f.linesize[1] as usize,
            horiz_subsampling: horiz,
            vert_subsampling: vert,
        };
        let (u, v) = if is(AV_PIX_FMT_NV12) { (0, 1) } else { (1, 0) };
        Some(([y, uv, uv], [luma, at(u), at(v)]))
    } else {
        let chroma = |i: usize| Plane {
            offset: 0,
            col_inc: 1,
            row_inc: f.linesize[i] as usize,
            horiz_subsampling: horiz,
            vert_subsampling: vert,
        };
        Some((
            [y, plane(1, ch, cw)?, plane(2, ch, cw)?],
            [luma, chroma(1), chroma(2)],
        ))
    }
}

/// A planar YUV frame of nine to sixteen bits a sample, narrowed to eight into
/// `into` and described as [`planes`] describes an 8-bit one. `None` for
/// anything else: RGB, a palette, big-endian samples (which no decoder on a
/// little-endian machine hands back), or chroma subsampled other than 4:2:0,
/// 4:2:2 or 4:4:4.
///
/// The whole picture every frame, which is a copy [`planes`] does not make;
/// what is in doubt for a deep stream is whether there is a picture at all.
fn narrowed<'a>(
    f: &ff::AVFrame,
    format: ff::AVPixelFormat,
    into: &'a mut [Vec<u8>; 3],
) -> Option<([&'a [u8]; 3], [Plane; 3])> {
    let (width, height) = (f.width.max(0) as usize, f.height.max(0) as usize);
    if width == 0 || height == 0 || f.format != format as c_int {
        return None;
    }
    let desc = unsafe { ff::av_pix_fmt_desc_get(format) };
    if desc.is_null() {
        return None;
    }
    let desc = unsafe { &*desc };
    let unwanted = (ff::AV_PIX_FMT_FLAG_BE
        | ff::AV_PIX_FMT_FLAG_PAL
        | ff::AV_PIX_FMT_FLAG_BITSTREAM
        | ff::AV_PIX_FMT_FLAG_HWACCEL
        | ff::AV_PIX_FMT_FLAG_RGB
        | ff::AV_PIX_FMT_FLAG_BAYER
        | ff::AV_PIX_FMT_FLAG_FLOAT
        | ff::AV_PIX_FMT_FLAG_XYZ) as u64;
    if desc.flags & unwanted != 0
        || desc.flags & ff::AV_PIX_FMT_FLAG_PLANAR as u64 == 0
        || desc.nb_components < 3
        || desc.log2_chroma_w > 1
        || desc.log2_chroma_h > 1
    {
        return None;
    }
    // Y, U and V each in a plane of its own, two bytes a sample, the value in
    // the low bits; an alpha plane after them is not read.
    let depth = desc.comp[0].depth;
    let one_each = desc.comp[..3].iter().enumerate().all(|(i, c)| {
        c.plane == i as c_int && c.step == 2 && c.offset == 0 && c.shift == 0 && c.depth == depth
    });
    if !one_each || !(9..=16).contains(&depth) {
        return None;
    }
    let (horiz, vert) = (1usize << desc.log2_chroma_w, 1usize << desc.log2_chroma_h);
    let (cw, ch) = (width.div_ceil(horiz), height.div_ceil(vert));

    for (i, (w, h)) in [(width, height), (cw, ch), (cw, ch)]
        .into_iter()
        .enumerate()
    {
        let stride = usize::try_from(f.linesize[i])
            .ok()
            .filter(|s| *s >= w * 2)?;
        if f.data[i].is_null() {
            return None;
        }
        // SAFETY: as in `planes`, a decoded plane has `linesize` bytes for
        // every row of the picture, and the last row as far as its last
        // sample is the least of that.
        let src = unsafe { std::slice::from_raw_parts(f.data[i], stride * (h - 1) + w * 2) };
        let dst = &mut into[i];
        dst.resize(w * h, 0);
        for (row, out) in dst.chunks_exact_mut(w).enumerate() {
            narrow(&src[row * stride..row * stride + w * 2], depth as u32, out);
        }
    }
    let plane = |w: usize, horiz: usize, vert: usize| Plane {
        offset: 0,
        col_inc: 1,
        row_inc: w,
        horiz_subsampling: horiz,
        vert_subsampling: vert,
    };
    let [y, u, v] = into;
    Some((
        [&y[..], &u[..], &v[..]],
        [
            plane(width, 1, 1),
            plane(cw, horiz, vert),
            plane(cw, horiz, vert),
        ],
    ))
}

/// One row of little-endian samples of `depth` bits to eight, rounded to the
/// nearest: limited range's 64 and 940 in ten bits are 16 and 235 in eight,
/// as the encoder meant them.
fn narrow(src: &[u8], depth: u32, out: &mut [u8]) {
    let shift = depth - 8;
    let half = 1u32 << (shift - 1);
    for (sample, out) in src.chunks_exact(2).zip(out) {
        let wide = u16::from_le_bytes([sample[0], sample[1]]) as u32;
        *out = ((wide + half) >> shift).min(255) as u8;
    }
}

/// libavcodec's own words for an error code.
fn error(code: c_int) -> String {
    let mut buf = [0 as std::ffi::c_char; 128];
    unsafe { ff::av_strerror(code, buf.as_mut_ptr(), buf.len()) };
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::narrow;

    /// The ends of limited range and of the whole range land where eight bits
    /// has them, and what rounds up past the top stays at the top.
    #[test]
    fn narrowing_keeps_the_ends() {
        let ten = [0u16, 64, 512, 940, 1020, 1023];
        let bytes: Vec<u8> = ten.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = [0u8; 6];
        narrow(&bytes, 10, &mut out);
        assert_eq!(out, [0, 16, 128, 235, 255, 255]);

        let twelve = [0u16, 256, 3760, 4095];
        let bytes: Vec<u8> = twelve.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = [0u8; 4];
        narrow(&bytes, 12, &mut out);
        assert_eq!(out, [0, 16, 235, 255]);
    }
}
