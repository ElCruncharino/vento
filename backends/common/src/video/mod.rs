// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The picture, where a protocol sends it as video: which codecs there are,
//! what a decoder promises, and what happens to its planes afterwards.
//!
//! A protocol asks for a decoder and does not know whose it is. What it holds
//! is a [`Decoders`], handed to it by whoever started the session: the phone's
//! MediaCodec in `platform_android` and [`NoDecoders`] for a test that has
//! none: the answer is the platform's rather than the protocol's.
//!
//! Everything after the decode is here and is the same for every decoder: a
//! decoder gets its planes into a [`convert::Frame`] and hands it to an
//! [`Output`], which finds what moved ([`diff`]) and converts only that
//! ([`convert`]). That is the expensive half of a frame on a phone whose
//! decode is hardware.

pub mod convert;
pub mod diff;
pub mod i420;

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

/// How a platform's decoders may decode: on the GPU or not. The person's
/// choice, off unless asked for.
///
/// Here rather than beside a platform's decoder because it is what a session
/// is started with, whichever backend it is: RustDesk's picture is decoded by
/// the platform's [`Decoders`], which carry it, and a library with decoders
/// of its own is handed it at connect. A phone's MediaCodec is whatever the
/// phone has and takes no such choice; its [`Decoders`] say `Off`, which to
/// the phone means nothing more than "nothing was asked".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hardware {
    /// Software, and nothing of a device opened or loaded: the default.
    #[default]
    Off,
    /// VAAPI, through libva, on the first DRM render node.
    Vaapi,
    /// Per stream: VAAPI where it takes the stream (a device, with a decoder
    /// for the profile), else software. The same as [`Vaapi`](Self::Vaapi)
    /// while VAAPI is the one API there is; what the setting means is "the
    /// GPU where it can", whatever decodes there.
    Auto,
}

impl Hardware {
    /// Every mode, in declaration order, which is also how the C side numbers
    /// them (`vento_video_hardware`, `vento_video_decoders.mode`).
    pub const ALL: [Hardware; 3] = [Hardware::Off, Hardware::Vaapi, Hardware::Auto];

    /// The name a setting or a command line uses: "off", "vaapi" or "auto".
    pub fn name(self) -> &'static str {
        match self {
            Hardware::Off => "off",
            Hardware::Vaapi => "vaapi",
            Hardware::Auto => "auto",
        }
    }

    /// The one [`name`](Self::name) names.
    pub fn from_name(name: &str) -> Option<Hardware> {
        Hardware::ALL.into_iter().find(|h| h.name() == name)
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
/// MediaCodec's is a raw handle nothing has promised may move. The one caller
/// that moves one anyway is TigerVNC's, whose library decodes each rectangle
/// on whichever of its worker threads is free: one call at a time, never two
/// at once, and that promise is written down where it is made
/// (`session::c::video`).
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

    /// Whether the last picture was decoded in hardware: `None` where the
    /// decoder does not know, or has not had a picture yet. What a session's
    /// facts say after the codec, as " (hardware)" or " (software)".
    fn hardware(&self) -> Option<bool> {
        None
    }

    /// A line for an overlay about how this decoder is decoding — the codec,
    /// the method and what a frame costs, and why it is software where
    /// hardware was asked for (`H.264 vaapi map 2.1 ms`, `H.264 sw 3.4 ms
    /// (vaapi: no device)`) — or `None` where it has nothing to say.
    fn status(&self) -> Option<String> {
        None
    }

    /// True, once, after the decoder lost the pictures the stream refers back
    /// to — a hardware decoder that failed mid-stream and was replaced by a
    /// software one — so that the caller asks the far end for a key frame
    /// rather than waiting for the next one. Polled after [`decode`].
    ///
    /// [`decode`]: Decoder::decode
    fn take_key_frame_request(&mut self) -> bool {
        false
    }

    /// One frame decoded and handed to `planes` as the decoder's own planes,
    /// unconverted: for a caller that does what comes after the decoder
    /// itself, as FreeRDP does, whose AVC444 makes one picture of two
    /// decoded ones before it converts anything. True when a picture came
    /// out and was handed over; the frame is lent for that call alone.
    ///
    /// Unlike [`decode`](Decoder::decode), **every** picture is handed
    /// over, one for each frame that makes one and in the order they were
    /// fed: none is passed over for a newer one, since such a caller pairs
    /// each picture with the frame that made it. False, always, for a
    /// decoder that cannot (the default).
    fn decode_planes(&mut self, data: &[u8], planes: &mut dyn FnMut(&Frame<'_>)) -> bool {
        let _ = (data, planes);
        false
    }

    /// [`decode`](Decoder::decode), except that a picture the decoder can
    /// leave where the hardware decoded it is left there and handed back
    /// rather than converted into `out`: what a caller that draws on the
    /// same GPU asks for, which then has no picture in `out` to read and
    /// no conversion to pay for. Every other picture — software's, one in a
    /// format the GPU picture cannot describe, the first after a fallback —
    /// is converted as [`decode`](Decoder::decode) converts it, so a caller
    /// has to take either. The default is a decoder that never can.
    fn decode_picture(
        &mut self,
        data: &[u8],
        out: &mut [u32],
        width: usize,
        height: usize,
        changed: &mut Changes,
    ) -> Decoded {
        if self.decode(data, out, width, height, changed) {
            Decoded::Converted
        } else {
            Decoded::Nothing
        }
    }
}

/// What [`Decoder::decode_picture`] made of a frame.
pub enum Decoded {
    /// No picture: the decoder is waiting for more, or refused the frame.
    Nothing,
    /// A picture, converted into the caller's buffer as
    /// [`Decoder::decode`] converts one.
    Converted,
    /// A picture left on the GPU. The caller's buffer was not touched.
    Gpu(std::sync::Arc<dyn GpuPicture>),
}

/// A decoded picture left in the memory the hardware decoded it into, for
/// whoever draws on the same GPU to import and convert there.
///
/// Holding one holds the surface: the decoder will not decode into it again
/// until every holder has let go, so a holder lets go once the GPU has
/// finished reading it. The decoder makes more surfaces as it needs them
/// rather than waiting for these, so holding one or two costs memory and
/// never a picture.
pub trait GpuPicture: Send + Sync {
    /// The visible picture, which is smaller than the surface it is in
    /// where the codec works in blocks (1080 rows of a 1088-row surface).
    fn width(&self) -> usize;
    fn height(&self) -> usize;

    /// Which of the decoder's surfaces this is: the same answer is the same
    /// memory, so an importer may keep what it made of a surface and use it
    /// again for the next picture decoded into it rather than importing it
    /// again. The first number changes whenever the surfaces do (a new
    /// decoder, a new size), and is never used twice in a process.
    fn surface(&self) -> (u64, u32);

    /// BT.709 rather than BT.601, and full range rather than limited, as
    /// [`convert::Frame`] says them: the picture's own, which may change
    /// with the stream while the surfaces stay.
    fn colour(&self) -> (bool, bool);

    /// The surface as DMA-BUFs, one a plane, each descriptor a new one of
    /// the caller's own.
    #[cfg(unix)]
    fn export(&self) -> Result<Dmabuf, String>;

    /// The picture converted on the processor into `out`, `width * height`
    /// words as [`Decoder::decode`] writes them (the part both sizes have,
    /// every row of it): for whoever needs it in memory after all, which
    /// costs the readback this picture was left on the GPU to save.
    fn read(&self, out: &mut [u32], width: usize, height: usize) -> bool;
}

/// A [`GpuPicture`]'s surface as DMA-BUFs, with what its numbers mean.
#[cfg(unix)]
pub struct Dmabuf {
    /// Y, then U and V interleaved (NV12, P010): a single-plane buffer each,
    /// in the order they are read.
    pub layers: Vec<DmabufLayer>,
    /// BT.709 rather than BT.601, and full range rather than limited, as
    /// [`convert::Frame`] says them.
    pub bt709: bool,
    pub full_range: bool,
}

/// One plane of a [`Dmabuf`]: its descriptor, its DRM format and layout
/// modifier, and where its samples are in it.
#[cfg(unix)]
pub struct DmabufLayer {
    pub fd: std::os::fd::OwnedFd,
    /// A DRM fourcc: `R8  ` and `GR88` for NV12, `R16 ` and `GR32` (GR1616)
    /// for P010.
    pub fourcc: u32,
    pub modifier: u64,
    pub offset: u32,
    pub pitch: u32,
    /// The plane's size in samples, which is the surface's and not the
    /// picture's.
    pub width: u32,
    pub height: u32,
}

/// The DRM fourccs a [`DmabufLayer`] can be in.
pub mod fourcc {
    const fn code(c: &[u8; 4]) -> u32 {
        u32::from_le_bytes(*c)
    }
    pub const R8: u32 = code(b"R8  ");
    pub const GR88: u32 = code(b"GR88");
    pub const R16: u32 = code(b"R16 ");
    pub const GR1616: u32 = code(b"GR32");
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

    /// A decoder as [`decoder`](Decoders::decoder) makes one, but decoding as
    /// `hardware` says rather than as these decoders do now: what a caller
    /// that holds many decoders at once (TigerVNC keeps up to 64) uses to
    /// keep all but a few of them off the GPU. A platform whose decoders take
    /// no such choice — the phone's MediaCodec, which is whatever the phone
    /// has — makes the one it always makes.
    fn decoder_as(
        &self,
        codec: Codec,
        width: usize,
        height: usize,
        hardware: Hardware,
    ) -> Option<Box<dyn Decoder>> {
        let _ = hardware;
        self.decoder(codec, width, height)
    }

    /// How a decoder made now would decode. It can change under a running
    /// session, which is how a session is told to switch: one whose decoder
    /// was made under another answer makes it again. Cheap enough to ask
    /// every frame.
    fn hardware(&self) -> Hardware {
        Hardware::Off
    }

    /// Whether whoever draws the session can take pictures on the GPU
    /// ([`Decoder::decode_picture`]), which it says by setting it where the
    /// platform's decoders can be told. A caller that can hand such a picture
    /// on asks this every frame, as it asks [`hardware`](Decoders::hardware):
    /// it can change under a running session, to off, where an import failed.
    /// No by default.
    fn gpu_pictures(&self) -> bool {
        false
    }
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
