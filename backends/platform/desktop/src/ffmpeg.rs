// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A desktop's decoder, libavcodec's: [`FFmpeg`] is this platform's
//! `common::video::Decoders`, and the counterpart of
//! `platform_android::mediacodec` call for call.
//!
//! The same contract as the phone's, because it is the same trait and the same
//! [`Output`] after it: a frame in, the rows that moved converted into the
//! caller's buffer, and nothing else written. What differs is only how the
//! planes are got at:
//!
//! - **Through `vento_video.h`, not FFmpeg's API.** libavcodec is reached
//!   through six C calls of our own (`c/vento_video.c`), and this file
//!   declares those by hand: no generated bindings, no FFmpeg headers, and
//!   nothing here that changes with an FFmpeg release. The choices that are
//!   libavcodec's — low delay, slice threads, AV1 as dav1d, only the last of
//!   several pictures — are made there, and the file says why.
//! - **Three allocations, not one.** libavcodec hands back a data pointer and a
//!   line size per plane, each in a buffer of its own, which is what
//!   `convert::Frame` takes three slices for. Nothing is copied to make them
//!   look like MediaCodec's single buffer.
//!
//! The FFmpeg is the build's own (see `build.rs`): FFmpeg and dav1d from their
//! submodules, configured for exactly the decoders vento's desktop backends
//! use, and linked statically, so nothing of a system FFmpeg is asked for.
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

use std::ffi::{CStr, c_char, c_int};
use std::sync::OnceLock;

use common::video::convert::{Frame, Plane};
use common::video::{Changes, Codec, Decoders, Output};

/// `c/vento_video.h`, declared by hand: the whole of what this crate calls.
mod sys {
    use std::ffi::{c_char, c_int};

    pub const PICTURE: c_int = 1;
    pub const NONE: c_int = 0;
    pub const REFUSED: c_int = -1;
    pub const UNSUPPORTED: c_int = -2;

    /// `struct vento_video_picture`, which [`super::Decoder::new`] checks the
    /// size of against the C side's before anything is decoded into it.
    #[repr(C)]
    pub struct Picture {
        pub data: [*const u8; 3],
        pub stride: [i32; 3],
        pub width: i32,
        pub height: i32,
        pub chroma_shift_x: i32,
        pub chroma_shift_y: i32,
        pub interleaved: i32,
        pub v_first: i32,
        pub depth: i32,
        pub bt709: i32,
        pub full_range: i32,
        pub error: i32,
        pub skipped: u32,
    }

    impl Picture {
        pub fn new() -> Picture {
            Picture {
                data: [std::ptr::null(); 3],
                stride: [0; 3],
                width: 0,
                height: 0,
                chroma_shift_x: 0,
                chroma_shift_y: 0,
                interleaved: 0,
                v_first: 0,
                depth: 0,
                bt709: 0,
                full_range: 0,
                error: 0,
                skipped: 0,
            }
        }
    }

    #[repr(C)]
    pub struct Video {
        _opaque: [u8; 0],
    }

    unsafe extern "C" {
        pub fn vento_video_picture_size() -> usize;
        pub fn vento_video_decodable(codec: c_int) -> c_int;
        pub fn vento_video_open(
            codec: c_int,
            width: c_int,
            height: c_int,
            error: *mut c_char,
            error_len: usize,
        ) -> *mut Video;
        pub fn vento_video_name(video: *const Video) -> *const c_char;
        pub fn vento_video_decode(
            video: *mut Video,
            data: *const u8,
            len: usize,
            out: *mut Picture,
            format: *mut c_char,
            format_len: usize,
        ) -> c_int;
        pub fn vento_video_strerror(error: c_int, buf: *mut c_char, len: usize);
        pub fn vento_video_close(video: *mut Video);
    }
}

/// libavcodec's decoders, as a session is handed them.
pub struct FFmpeg;

impl Decoders for FFmpeg {
    /// Looked up once and remembered: the answer is the library's, which is
    /// linked in and cannot change under a running process.
    fn decodable(&self, codec: Codec) -> bool {
        static FOUND: OnceLock<[bool; Codec::ALL.len()]> = OnceLock::new();
        let found = FOUND.get_or_init(|| {
            Codec::ALL.map(|c| unsafe { sys::vento_video_decodable(number(c)) } != 0)
        });
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

/// The codec as `vento_video.h` numbers it.
fn number(codec: Codec) -> c_int {
    match codec {
        Codec::H264 => 1,
        Codec::H265 => 2,
        Codec::Vp8 => 3,
        Codec::Vp9 => 4,
        Codec::Av1 => 5,
    }
}

/// One libavcodec decoder for one codec, which is what [`FFmpeg::decoder`]
/// hands out boxed — and what a caller that wants [`skipped`](Self::skipped)
/// can make directly.
pub struct Decoder {
    video: *mut sys::Video,
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
        // The one place the two copies of the struct could disagree, checked
        // before the C side ever writes one.
        assert_eq!(
            unsafe { sys::vento_video_picture_size() },
            size_of::<sys::Picture>(),
            "vento_video.h and this file describe a picture differently"
        );
        let mut error = [0 as c_char; 128];
        let video = unsafe {
            sys::vento_video_open(
                number(codec),
                c_int::try_from(width).unwrap_or(0),
                c_int::try_from(height).unwrap_or(0),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if video.is_null() {
            log::warn!(
                "could not open a {} decoder at {width}x{height}: {}",
                codec.name(),
                text(&error)
            );
            return None;
        }
        let name = unsafe { CStr::from_ptr(sys::vento_video_name(video)) }.to_string_lossy();
        log::info!(
            "{} decoder ({name}) started for {width}x{height}",
            codec.name()
        );
        Some(Decoder {
            video,
            failed: false,
            output: Output::new(),
            narrow: Default::default(),
            skipped: 0,
        })
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
        let mut picture = sys::Picture::new();
        let mut format = [0 as c_char; 64];
        let status = unsafe {
            sys::vento_video_decode(
                self.video,
                data.as_ptr(),
                data.len(),
                &mut picture,
                format.as_mut_ptr(),
                format.len(),
            )
        };
        self.skipped += u64::from(picture.skipped);
        match status {
            sys::PICTURE | sys::NONE if picture.error != 0 => {
                log::warn!("the decoder failed a frame: {}", error(picture.error));
            }
            sys::REFUSED => {
                // A frame the decoder refuses is one picture lost rather than
                // a session: the next key frame starts it again.
                log::warn!(
                    "a {} byte frame was refused: {}",
                    data.len(),
                    error(picture.error)
                );
                return false;
            }
            sys::UNSUPPORTED => {
                log::warn!(
                    "the decoder's output is {}, which is not YUV this can convert",
                    text(&format)
                );
                self.failed = true;
                return false;
            }
            _ => {}
        }
        if status != sys::PICTURE {
            return false;
        }
        let Some((bytes, planes)) = planes(&picture, &mut self.narrow) else {
            log::warn!("the decoder's output has planes this cannot read");
            self.failed = true;
            return false;
        };
        let frame = Frame {
            bytes,
            planes,
            width: picture.width as usize,
            height: picture.height as usize,
            bt709: picture.bt709 != 0,
            full_range: picture.full_range != 0,
        };
        self.output.write(&frame, out, width, height, changed);
        true
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { sys::vento_video_close(self.video) };
    }
}

/// A picture's planes, as `convert` reads them: in place at eight bits, and
/// narrowed into `into` deeper than that. `None` where the description does
/// not hold together, which the C side should never hand back.
fn planes<'a>(
    p: &'a sys::Picture,
    into: &'a mut [Vec<u8>; 3],
) -> Option<([&'a [u8]; 3], [Plane; 3])> {
    let (width, height) = (
        usize::try_from(p.width).ok()?,
        usize::try_from(p.height).ok()?,
    );
    if width == 0
        || height == 0
        || !(0..=1).contains(&p.chroma_shift_x)
        || !(0..=1).contains(&p.chroma_shift_y)
    {
        return None;
    }
    let (horiz, vert) = (1usize << p.chroma_shift_x, 1usize << p.chroma_shift_y);
    let (cw, ch) = (width.div_ceil(horiz), height.div_ceil(vert));
    let depth = u32::try_from(p.depth)
        .ok()
        .filter(|d| (8..=16).contains(d))?;
    let interleaved = p.interleaved != 0;
    if interleaved && depth != 8 {
        return None;
    }
    let bytes = if depth > 8 { 2 } else { 1 };
    let row_bytes = [
        width * bytes,
        if interleaved { cw * 2 } else { cw * bytes },
        cw * bytes,
    ];
    let rows = [height, ch, ch];

    // Exactly the bytes the picture covers in each plane: every row but the
    // last at its full line size, and the last only as far as its last
    // sample, since that is all the frame promises is there.
    let plane = |i: usize| -> Option<(&'a [u8], usize)> {
        let stride = usize::try_from(p.stride[i])
            .ok()
            .filter(|s| *s >= row_bytes[i])?;
        if p.data[i].is_null() {
            return None;
        }
        let len = stride * (rows[i] - 1) + row_bytes[i];
        // SAFETY: a decoded plane has `stride` bytes for every row of the
        // picture, and the last row as far as its last sample is the least of
        // that. The picture is the decoder's until the next `decode`, and
        // `convert` is done with it before then.
        Some((
            unsafe { std::slice::from_raw_parts(p.data[i], len) },
            stride,
        ))
    };
    let at = |offset: usize, col_inc: usize, row_inc: usize, h: usize, v: usize| Plane {
        offset,
        col_inc,
        row_inc,
        horiz_subsampling: h,
        vert_subsampling: v,
    };

    if depth == 8 {
        let (y, ys) = plane(0)?;
        let luma = at(0, 1, ys, 1, 1);
        if interleaved {
            let (uv, uvs) = plane(1)?;
            let (u, v) = if p.v_first != 0 { (1, 0) } else { (0, 1) };
            return Some((
                [y, uv, uv],
                [luma, at(u, 2, uvs, horiz, vert), at(v, 2, uvs, horiz, vert)],
            ));
        }
        let ((u, us), (v, vs)) = (plane(1)?, plane(2)?);
        return Some((
            [y, u, v],
            [luma, at(0, 1, us, horiz, vert), at(0, 1, vs, horiz, vert)],
        ));
    }

    // Deeper: the whole picture every frame, which is a copy the 8-bit path
    // does not make; what is in doubt for a deep stream is whether there is a
    // picture at all.
    let sources = [plane(0)?, plane(1)?, plane(2)?];
    for (i, ((src, stride), dst)) in sources.into_iter().zip(into.iter_mut()).enumerate() {
        let w = if i == 0 { width } else { cw };
        dst.resize(w * rows[i], 0);
        for (row, out) in dst.chunks_exact_mut(w).enumerate() {
            narrow(&src[row * stride..row * stride + w * 2], depth, out);
        }
    }
    let [y, u, v] = into;
    Some((
        [&y[..], &u[..], &v[..]],
        [
            at(0, 1, width, 1, 1),
            at(0, 1, cw, horiz, vert),
            at(0, 1, cw, horiz, vert),
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
    let mut buf = [0 as c_char; 128];
    unsafe { sys::vento_video_strerror(code, buf.as_mut_ptr(), buf.len()) };
    text(&buf)
}

/// A NUL-terminated buffer the C side wrote, as text.
fn text(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
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
