// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The picture, where a protocol sends it as video: which codecs there are,
//! what a decoder promises, and what happens to its planes afterwards.
//!
//! A protocol asks for a decoder and does not know whose it is. What it holds
//! is a [`Decoders`], handed to it by whoever started the session: the phone's
//! MediaCodec in `platform_android`, libavcodec in `platform_desktop`, and
//! [`NoDecoders`] for a test that has neither. Which one that is was a
//! `cfg(target_os)` inside the protocol until a second platform could decode,
//! and the answer is the platform's rather than the protocol's — the same
//! RustDesk session wants VP9 on both, and only the thing that gets the
//! picture out of VP9 differs.
//!
//! Everything after the decode is here and is the same for every decoder: a
//! decoder gets its planes into a [`convert::Frame`] and hands it to an
//! [`Output`], which finds what moved ([`diff`]) and converts only that
//! ([`convert`]). That is the expensive half of a frame on a phone whose
//! decode is hardware, and the half the two platforms share exactly.

pub mod convert;
pub mod diff;

pub use diff::Changes;

use convert::{Frame, Layout};
use diff::Previous;

/// Which codec a frame arrived in. RustDesk's `VideoFrame` has an arm for each
/// of these, and SPICE's video streams name most of the same ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Vp9,
    Vp8,
    Av1,
    H264,
    H265,
}

impl Codec {
    /// Every codec there is, which is what a far end is told this end can take
    /// once each has been asked about.
    pub const ALL: [Codec; 5] = [
        Codec::Vp9,
        Codec::Vp8,
        Codec::Av1,
        Codec::H264,
        Codec::H265,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Codec::Vp9 => "VP9",
            Codec::Vp8 => "VP8",
            Codec::Av1 => "AV1",
            Codec::H264 => "H.264",
            Codec::H265 => "H.265",
        }
    }
}

/// One decoder, for one codec at one size.
///
/// A frame goes in and what moved in it comes out into `out`, which is the
/// caller's back buffer rather than the framebuffer: every frame of a video
/// stream is a whole one, so the picture is swapped in afterwards rather than
/// converted in place under a lock the drawing thread wants.
///
/// Not `Send`: a decoder is made, fed and dropped on the protocol thread, and
/// MediaCodec's is a raw handle nothing has promised may move.
pub trait Decoder {
    /// True when a frame arrived, with the rows and columns it changed added to
    /// `changed`. `out` is `width * height` words, row-major, `R, G, B, A` in
    /// memory, and **only the rows `changed` names are written**: the rest are
    /// left as they were, since converting a row nothing moved in is the bulk
    /// of what a frame costs here.
    ///
    /// Rows already marked in `changed` on the way in are converted whether
    /// they moved or not, which is how a caller whose buffer is out of step
    /// with the picture on screen asks for a whole frame.
    ///
    /// A decoder whose picture is not the session's size — which is every
    /// frame between a resize being asked for and arriving — writes the part
    /// the two have in common and leaves the rest alone.
    fn decode(
        &mut self,
        data: &[u8],
        out: &mut [u32],
        width: usize,
        height: usize,
        changed: &mut Changes,
    ) -> bool;
}

/// What this platform can decode, and a decoder when asked for one.
///
/// Shared between the protocol thread, which makes decoders, and whichever
/// thread changes a live option, which is when a far end is told again what
/// this end can take.
pub trait Decoders: Send + Sync {
    /// Whether this codec can be decoded here at all. It is what the far end
    /// is told before it picks one: a peer told this end takes H.265 and
    /// sending it to a phone with no HEVC decoder is a session with a login, a
    /// cursor and no picture. Cheap to ask again — an implementation that has
    /// to probe probes once.
    fn decodable(&self, codec: Codec) -> bool;

    /// A decoder for this codec at this size, or `None` where there is none —
    /// which is a session with no picture and is reported as such rather than
    /// retried.
    fn decoder(&self, codec: Codec, width: usize, height: usize) -> Option<Box<dyn Decoder>>;
}

/// Nothing decodes: for a test of a protocol that is not about its picture, and
/// for a host with nothing better. A peer told this is told it may send
/// anything, and its picture is then reported as having no decoder.
pub struct NoDecoders;

impl Decoders for NoDecoders {
    fn decodable(&self, _codec: Codec) -> bool {
        false
    }

    fn decoder(&self, _codec: Codec, _width: usize, _height: usize) -> Option<Box<dyn Decoder>> {
        None
    }
}

/// What a decoder keeps between frames to turn its planes into the caller's
/// words: the last frame's planes, which say how much of this one is worth
/// converting, and whether the vectorised conversion agreed with this layout,
/// which is checked once per layout rather than once per frame.
pub struct Output {
    previous: Previous,
    checked: Option<(Layout, bool)>,
}

impl Output {
    pub fn new() -> Output {
        Output {
            previous: Previous::new(),
            checked: None,
        }
    }

    /// `frame` into `out`, which is `width * height` words, as
    /// [`Decoder::decode`] describes: what moved is marked into `changed`, and
    /// only the rows `changed` then names are written.
    ///
    /// Of a frame that is not the session's size, the part both have is
    /// written and the rest of `out` is left alone. A frame whose planes do not
    /// fit the bytes they came in writes nothing at all.
    pub fn write(
        &mut self,
        frame: &Frame,
        out: &mut [u32],
        width: usize,
        height: usize,
        changed: &mut Changes,
    ) {
        let w = width.min(frame.width);
        let h = height.min(frame.height);
        if w == 0 || h == 0 || out.len() < width * height {
            return;
        }
        let frame = Frame {
            width: w,
            height: h,
            ..*frame
        };
        if !frame.fits() {
            log::warn!("a {w}x{h} frame does not fit the buffers it arrived in");
            return;
        }

        let c = convert::coefficients(frame.bt709, frame.full_range);
        let layout = frame.layout();
        let vectorise = match self.checked {
            Some((checked, ok)) if checked == layout => ok,
            _ => {
                let ok = convert::vectorised_agrees(&frame.row(0), &layout, w, &c);
                log::info!(
                    "the vectorised conversion is {} for this decoder's layout",
                    if ok { "in use" } else { "off" }
                );
                self.checked = Some((layout, ok));
                ok
            }
        };

        self.previous.compare(&frame, changed);

        for row in 0..h {
            if !changed.row(row) {
                continue;
            }
            let planes = frame.row(row);
            let line = &mut out[row * width..row * width + w];
            let mut col = 0;
            if vectorise {
                col = convert::row_vectorised(&planes, &layout, line, w, &c);
            }
            convert::row_scalar(&planes, &layout, line, col, w, &c);
        }
    }
}

impl Default for Output {
    fn default() -> Output {
        Output::new()
    }
}
