// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A desktop's decoder, libavcodec's: [`FFmpeg`] is this platform's
//! `runtime::media::video::Decoders`, and the counterpart of
//! `platform_android::mediacodec` call for call.
//!
//! The same contract as the phone's, because it is the same trait and the same
//! [`Output`] after it: a frame in, the rows that moved converted into the
//! caller's buffer, and nothing else written. What differs is only how the
//! planes are got at:
//!
//! - **Through `vento_video.h`, not FFmpeg's API.** libavcodec is reached
//!   through seventeen C calls of our own (`cpp/vento_video.c`), and this file
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
//! **Hardware decoding is opt-in** ([`Hardware`], off by default): asked for,
//! a decoder decodes on VAAPI where the device has a decoder
//! for the stream's profile, and in software where it has not, with the
//! reason kept for [`status`](runtime::media::video::Decoder::status). What
//! comes back is still planes in memory — the hardware's surface mapped for
//! reading, in NV12, P010 or XV30 — converted by the same code as software's,
//! so the picture is the same either way. A hardware failure mid-stream is a
//! software decoder in its place and a key frame asked for
//! ([`take_key_frame_request`](runtime::media::video::Decoder::take_key_frame_request)),
//! not a lost picture. Off, nothing of libva is loaded.
//!
//! **Or left on the GPU**, for a window that draws there and says so
//! ([`FFmpeg::set_gpu_pictures`]): a caller that asks
//! ([`decode_picture`](runtime::media::video::Decoder::decode_picture)) is
//! handed a VAAPI picture in NV12 or P010 as it is, a [`GpuPicture`] holding
//! the surface, which it exports as DMA-BUFs ([`export`]) and converts on its
//! GPU; nothing is mapped or converted here, which is most of what a
//! hardware picture cost.
//! Read back ([`GpuPicture::read`]), it is mapped and converted as a decode
//! would have, the same picture to the bit.
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

use std::any::Any;
use std::ffi::{CStr, c_char, c_int};
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use runtime::media::video::convert::{Frame, Plane};
use runtime::media::video::{Changes, Codec, Decoded, Decoders, GpuPicture, Output};
/// How a decoder may decode, which whoever starts a session asks for (a
/// setting): `runtime`'s, since it is handed to every backend. Here
/// [`Hardware::On`] is VAAPI, and the modes are named as [`hardware_name`]
/// says.
pub use runtime::media::video::Hardware;

/// `cpp/vento_video.h`, declared by hand: the whole of what this crate calls.
mod sys {
    use std::ffi::{c_char, c_int};

    pub const PICTURE: c_int = 1;
    pub const NONE: c_int = 0;
    pub const REFUSED: c_int = -1;
    pub const UNSUPPORTED: c_int = -2;

    pub const XV30: i32 = 1;

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
        pub shift: i32,
        pub packed: i32,
        pub hardware: i32,
        pub fell_back: i32,
        pub decode_us: u32,
        pub map_us: u32,
        pub held: *mut Frame,
        pub pool: u64,
        pub surface: u32,
    }

    /// `hardware`'s numbers, past 1 for mapped.
    pub const COPIED: i32 = 2;
    pub const HELD: i32 = 3;

    /// `struct vento_video_layer` and `struct vento_video_export`, checked
    /// as the picture is.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Layer {
        pub fd: i32,
        pub fourcc: u32,
        pub modifier: u64,
        pub offset: u32,
        pub pitch: u32,
        pub width: u32,
        pub height: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct Export {
        pub layers: i32,
        pub layer: [Layer; 2],
        pub bt709: i32,
        pub full_range: i32,
    }

    #[repr(C)]
    pub struct Frame {
        _opaque: [u8; 0],
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
                shift: 0,
                packed: 0,
                hardware: 0,
                fell_back: 0,
                decode_us: 0,
                map_us: 0,
                held: std::ptr::null_mut(),
                pool: 0,
                surface: 0,
            }
        }
    }

    #[repr(C)]
    pub struct Video {
        _opaque: [u8; 0],
    }

    unsafe extern "C" {
        pub fn vento_video_picture_size() -> usize;
        pub fn vento_video_export_size() -> usize;
        pub fn vento_video_decodable(codec: c_int) -> c_int;
        pub fn vento_video_open(
            codec: c_int,
            width: c_int,
            height: c_int,
            hardware: c_int,
            error: *mut c_char,
            error_len: usize,
        ) -> *mut Video;
        pub fn vento_video_name(video: *const Video) -> *const c_char;
        pub fn vento_video_method(video: *const Video) -> c_int;
        pub fn vento_video_software_reason(video: *const Video) -> *const c_char;
        pub fn vento_video_maps_in_place(video: *const Video) -> c_int;
        pub fn vento_video_device_state(method: c_int) -> c_int;
        pub fn vento_video_fail_next(video: *mut Video, software_too: c_int);
        pub fn vento_video_decode(
            video: *mut Video,
            data: *const u8,
            len: usize,
            hold: c_int,
            out: *mut Picture,
            format: *mut c_char,
            format_len: usize,
        ) -> c_int;
        pub fn vento_video_frame_export(
            frame: *mut Frame,
            out: *mut Export,
            error: *mut c_char,
            error_len: usize,
        ) -> c_int;
        pub fn vento_video_frame_map(frame: *mut Frame, out: *mut Picture) -> c_int;
        pub fn vento_video_frame_unmap(frame: *mut Frame);
        pub fn vento_video_frame_free(frame: *mut Frame);
        pub fn vento_video_strerror(error: c_int, buf: *mut c_char, len: usize);
        pub fn vento_video_close(video: *mut Video);
    }
}

/// How a decoder may decode, as `vento_video_hardware` numbers it: `On`
/// is VAAPI.
fn hardware_number(hardware: Hardware) -> c_int {
    match hardware {
        Hardware::Off => 0,
        Hardware::On => 1,
        Hardware::Auto => 2,
    }
}

fn hardware_of(number: c_int) -> Hardware {
    match number {
        1 => Hardware::On,
        2 => Hardware::Auto,
        _ => Hardware::Off,
    }
}

/// The name a setting or a command line uses for a mode here: "off",
/// "vaapi" (the one API [`Hardware::On`] decodes on) or "auto".
pub fn hardware_name(hardware: Hardware) -> &'static str {
    match hardware {
        Hardware::Off => "off",
        Hardware::On => "vaapi",
        Hardware::Auto => "auto",
    }
}

/// The mode [`hardware_name`] names.
pub fn hardware_named(name: &str) -> Option<Hardware> {
    Hardware::ALL.into_iter().find(|h| hardware_name(*h) == name)
}

/// Whether the device a [`Hardware`] decodes on has been opened in this
/// process: `None` never asked for, `Some(true)` open, `Some(false)` tried
/// and failed. Every decoder that asks shares the one device, and it stays
/// open until the process ends; with hardware off it is never asked for.
pub fn device_opened(hardware: Hardware) -> Option<bool> {
    match unsafe { sys::vento_video_device_state(hardware_number(hardware)) } {
        0 => None,
        state => Some(state > 0),
    }
}

/// libavcodec's decoders, as a session is handed them, decoding as
/// [`Hardware`] says: software unless asked otherwise.
///
/// The mode can be changed under a running session ([`set_hardware`]): a
/// decoder made from then on decodes the new way, and one already made goes on
/// as it was, so a session that wants the change at once makes its decoder
/// again — which it knows to do from [`Decoders::hardware`], the same answer.
///
/// [`set_hardware`]: FFmpeg::set_hardware
#[derive(Default)]
pub struct FFmpeg {
    hardware: AtomicU8,
    gpu: AtomicBool,
}

impl FFmpeg {
    pub const fn new(hardware: Hardware) -> FFmpeg {
        FFmpeg {
            hardware: AtomicU8::new(hardware as u8),
            gpu: AtomicBool::new(false),
        }
    }

    /// Whether the session's pictures may be left on the GPU (see
    /// [`Decoders::gpu_pictures`]), which the window says where it can import
    /// them, and unsays where an import failed. Off by default.
    pub fn set_gpu_pictures(&self, gpu: bool) {
        self.gpu.store(gpu, Ordering::Relaxed);
    }

    pub fn hardware(&self) -> Hardware {
        let number = self.hardware.load(Ordering::Relaxed);
        Hardware::ALL
            .into_iter()
            .find(|h| *h as u8 == number)
            .unwrap_or_default()
    }

    pub fn set_hardware(&self, hardware: Hardware) {
        self.hardware.store(hardware as u8, Ordering::Relaxed);
    }
}

impl Decoders for FFmpeg {
    /// Looked up once and remembered: the answer is the library's, which is
    /// linked in and cannot change under a running process. The software
    /// decoders' answer whatever the hardware, since software is where a
    /// stream the hardware cannot take ends up.
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
    ) -> Option<Box<dyn runtime::media::video::Decoder>> {
        self.decoder_as(codec, width, height, self.hardware())
    }

    fn decoder_as(
        &self,
        codec: Codec,
        width: usize,
        height: usize,
        hardware: Hardware,
    ) -> Option<Box<dyn runtime::media::video::Decoder>> {
        Decoder::with_hardware(codec, width, height, hardware)
            .map(|d| Box::new(d) as Box<dyn runtime::media::video::Decoder>)
    }

    fn hardware(&self) -> Hardware {
        FFmpeg::hardware(self)
    }

    fn gpu_pictures(&self) -> bool {
        self.gpu.load(Ordering::Relaxed)
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
/// hands out boxed.
pub struct Decoder {
    video: *mut sys::Video,
    codec: Codec,
    failed: bool,
    /// How the last picture came out: as `hardware` in the C struct numbers
    /// it, 0 software, else mapped, copied or held; `None` before the first.
    hardware: Option<i32>,
    /// The software reason last logged, so that a stream that keeps it is
    /// logged once.
    reason: String,
    /// A mid-stream fallback the caller has not been told of.
    key_frame: bool,
    /// What a frame has been costing, in milliseconds, smoothed; and the
    /// pictures so far, since the first is not counted: it is the one the
    /// decoder (and on hardware the device's surfaces) is set up in, which
    /// takes a frame's time several times over at once.
    decode_ms: Option<f32>,
    /// Of that, mapping (or copying) a hardware picture for reading.
    map_ms: Option<f32>,
    pictures: u64,
    /// The last frame's planes and the vectorised conversion's verdict, which
    /// between them are how little of this one is converted.
    output: Output,
    /// A deeper-than-eight-bit frame's planes, narrowed: kept rather than
    /// allocated per frame, since a stream that is one is one throughout.
    narrow: [Vec<u8>; 3],
    /// What a held picture is read back with, shared by them all.
    readback: Arc<Mutex<Readback>>,
}

impl Decoder {
    /// A software decoder, which is what [`FFmpeg`] hands out by default.
    pub fn new(codec: Codec, width: usize, height: usize) -> Option<Decoder> {
        Decoder::with_hardware(codec, width, height, Hardware::Off)
    }

    /// A decoder on the device `hardware` asks for, where the device opens and
    /// the decoder on it does; a software one otherwise, with
    /// [`software_reason`](Self::software_reason) saying why. `None` only
    /// where software has no decoder either.
    pub fn with_hardware(
        codec: Codec,
        width: usize,
        height: usize,
        hardware: Hardware,
    ) -> Option<Decoder> {
        // The one place the two copies of the struct could disagree, checked
        // before the C side ever writes one.
        assert_eq!(
            unsafe { sys::vento_video_picture_size() },
            size_of::<sys::Picture>(),
            "vento_video.h and this file describe a picture differently"
        );
        assert_eq!(
            unsafe { sys::vento_video_export_size() },
            size_of::<sys::Export>(),
            "vento_video.h and this file describe an export differently"
        );
        let mut error = [0 as c_char; 128];
        let video = unsafe {
            sys::vento_video_open(
                number(codec),
                c_int::try_from(width).unwrap_or(0),
                c_int::try_from(height).unwrap_or(0),
                hardware_number(hardware),
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
        let mut decoder = Decoder {
            video,
            codec,
            failed: false,
            hardware: None,
            reason: String::new(),
            key_frame: false,
            decode_ms: None,
            map_ms: None,
            pictures: 0,
            output: Output::new(),
            narrow: Default::default(),
            readback: Arc::default(),
        };
        let name = decoder.name();
        let method = decoder.method();
        match decoder.software_reason() {
            Some(why) => {
                log::info!(
                    "{} decoder ({name}) started for {width}x{height}, in software: {why}",
                    codec.name()
                );
                decoder.reason = why;
            }
            None if method != Hardware::Off => log::info!(
                "{} decoder ({name}, {}) started for {width}x{height}",
                codec.name(),
                hardware_name(method)
            ),
            None => log::info!(
                "{} decoder ({name}) started for {width}x{height}",
                codec.name()
            ),
        }
        Some(decoder)
    }

    /// libavcodec's decoder ("h264", "av1", "libdav1d", ...).
    pub fn name(&self) -> String {
        unsafe { CStr::from_ptr(sys::vento_video_name(self.video)) }
            .to_string_lossy()
            .into_owned()
    }

    /// The device decoded on, or [`Hardware::Off`] for software — which a
    /// decoder is from the start without a device and from a mid-stream
    /// fallback on. One on a device whose stream it cannot take is still on
    /// it, and decodes in software: [`Decoder::hardware`] says which.
    ///
    /// [`Decoder::hardware`]: runtime::media::video::Decoder::hardware
    pub fn method(&self) -> Hardware {
        hardware_of(unsafe { sys::vento_video_method(self.video) })
    }

    /// Why the decoder is decoding in software though hardware was asked for
    /// ("vaapi: no device (...)", "vaapi: H.264 High 10 is not supported",
    /// "vaapi: map failed (...)"), or `None`.
    pub fn software_reason(&self) -> Option<String> {
        let why = self.reason_text().to_string_lossy().into_owned();
        (!why.is_empty()).then_some(why)
    }

    /// The C side's reason, valid until the next call on the decoder.
    fn reason_text(&self) -> &CStr {
        unsafe { CStr::from_ptr(sys::vento_video_software_reason(self.video)) }
    }

    /// What a picture has been costing lately, in milliseconds: the decoder's
    /// whole call (on hardware the decode, the wait for it and the map), and
    /// of that the map of a hardware picture, where there has been one. Not
    /// the conversion after it, which is the caller's to time.
    pub fn timing(&self) -> Option<(f32, Option<f32>)> {
        self.decode_ms.map(|decode| (decode, self.map_ms))
    }

    /// Whether VAAPI's pictures are read in place (`Some(true)`) or through
    /// an image the driver copies each into (`Some(false)`), which the
    /// decoder decides by measuring both ways on its first pictures: `None`
    /// until it has.
    pub fn maps_in_place(&self) -> Option<bool> {
        match unsafe { sys::vento_video_maps_in_place(self.video) } {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    /// For tests: the next hardware picture fails as a device that went away
    /// would, which is what a mid-stream fallback is made of; with
    /// `software_too`, so does the software decoder that would take its place.
    pub fn fail_next(&mut self, software_too: bool) {
        unsafe { sys::vento_video_fail_next(self.video, c_int::from(software_too)) };
    }

    /// What the call just made says about how the decoder decodes, logged
    /// where that changed.
    fn note(&mut self, picture: &sys::Picture, status: c_int) {
        // Below zero is no decoder at all, which `decode_with` says.
        if picture.fell_back > 0 {
            // Before the first picture the packet given again is the one the
            // stream started with, which the software decoder takes from the
            // start as the hardware's would have: nothing is missing.
            self.key_frame = self.pictures > 0;
            let why = self.software_reason().unwrap_or_default();
            self.hardware = Some(0);
            if self.key_frame {
                log::warn!(
                    "{} decoder fell back to software mid-stream: {why}",
                    self.codec.name()
                );
            } else {
                log::info!("{} decoder is in software: {why}", self.codec.name());
            }
        }
        if status == sys::PICTURE {
            self.pictures += 1;
            if self.pictures > 1 {
                let smooth = |mean: Option<f32>, us: u32| {
                    let ms = us as f32 / 1000.0;
                    Some(mean.map_or(ms, |m| m + (ms - m) / 16.0))
                };
                self.decode_ms = smooth(self.decode_ms, picture.decode_us);
                if picture.hardware != 0 {
                    self.map_ms = smooth(self.map_ms, picture.map_us);
                }
            }
            self.hardware = Some(picture.hardware);
        }
        // Compared in place: this is every frame, and the reason changes
        // about once a stream.
        if self.reason_text().to_bytes() != self.reason.as_bytes() {
            let why = self.software_reason().unwrap_or_default();
            if !why.is_empty() && picture.fell_back == 0 {
                log::info!("{} decoder is in software: {why}", self.codec.name());
            }
            self.reason = why;
        }
    }
}

impl Decoder {
    /// A decode, as [`decode`](runtime::media::video::Decoder::decode) and
    /// [`decode_picture`](runtime::media::video::Decoder::decode_picture) make
    /// one: with `hold`, a picture the C side can hold is handed back held.
    fn decode_as(
        &mut self,
        data: &[u8],
        out: &mut [u32],
        width: usize,
        height: usize,
        changed: &mut Changes,
        hold: bool,
    ) -> Decoded {
        if out.len() < width * height {
            return Decoded::Nothing;
        }
        self.decode_with(data, hold, &mut |frame, output| {
            output.write(frame, out, width, height, changed)
        })
    }

    /// The decode itself, with what a picture in memory becomes left to
    /// `picture`, which is handed the decoder's [`Output`] for the
    /// conversion where it converts: [`Decoded::Converted`] once it has
    /// been called.
    fn decode_with(
        &mut self,
        data: &[u8],
        hold: bool,
        picture_to: &mut dyn FnMut(&Frame<'_>, &mut Output),
    ) -> Decoded {
        if self.failed || data.is_empty() {
            return Decoded::Nothing;
        }
        let mut picture = sys::Picture::new();
        let mut format = [0 as c_char; 64];
        let status = unsafe {
            sys::vento_video_decode(
                self.video,
                data.as_ptr(),
                data.len(),
                c_int::from(hold),
                &mut picture,
                format.as_mut_ptr(),
                format.len(),
            )
        };
        // Owned from here, whatever else the call said, so that it is let go
        // of on every way out.
        let held = (!picture.held.is_null()).then(|| Held {
            frame: Mutex::new(FramePtr(picture.held)),
            width: picture.width.max(0) as usize,
            height: picture.height.max(0) as usize,
            pool: picture.pool,
            surface: picture.surface,
            colour: (picture.bt709 != 0, picture.full_range != 0),
            readback: Arc::clone(&self.readback),
        });
        self.note(&picture, status);
        match status {
            sys::PICTURE | sys::NONE if picture.error != 0 => {
                log::warn!("the decoder failed a frame: {}", error(picture.error));
            }
            sys::REFUSED if picture.fell_back < 0 => {
                // The device failed and the software decoder would not open
                // in its place: the C side refuses everything from here on,
                // so this is said once and nothing more is asked of it, a key
                // frame included.
                log::warn!(
                    "the {} decoder failed on the device and no software decoder opened in its \
                     place ({}); no more pictures",
                    self.codec.name(),
                    error(picture.error)
                );
                self.key_frame = false;
                self.failed = true;
                return Decoded::Nothing;
            }
            sys::REFUSED => {
                // A frame the decoder refuses is one picture lost rather than
                // a session: the next key frame starts it again.
                log::warn!(
                    "a {} byte frame was refused: {}",
                    data.len(),
                    error(picture.error)
                );
                return Decoded::Nothing;
            }
            sys::UNSUPPORTED => {
                log::warn!(
                    "the decoder's output is {}, which is not YUV this can convert",
                    text(&format)
                );
                self.failed = true;
                return Decoded::Nothing;
            }
            _ => {}
        }
        if status != sys::PICTURE {
            return Decoded::Nothing;
        }
        if let Some(held) = held {
            return Decoded::Gpu(Arc::new(held));
        }
        let Some((bytes, planes)) = planes(&picture, &mut self.narrow) else {
            log::warn!("the decoder's output has planes this cannot read");
            self.failed = true;
            return Decoded::Nothing;
        };
        let frame = Frame {
            bytes,
            planes,
            width: picture.width as usize,
            height: picture.height as usize,
            bt709: picture.bt709 != 0,
            full_range: picture.full_range != 0,
        };
        picture_to(&frame, &mut self.output);
        Decoded::Converted
    }
}

impl runtime::media::video::Decoder for Decoder {
    fn decode(
        &mut self,
        data: &[u8],
        out: &mut [u32],
        width: usize,
        height: usize,
        changed: &mut Changes,
    ) -> bool {
        matches!(
            self.decode_as(data, out, width, height, changed, false),
            Decoded::Converted
        )
    }

    fn decode_picture(
        &mut self,
        data: &[u8],
        out: &mut [u32],
        width: usize,
        height: usize,
        changed: &mut Changes,
    ) -> Decoded {
        self.decode_as(data, out, width, height, changed, true)
    }

    /// The picture as the decoder made it, or as it was read off the GPU:
    /// libavcodec is told low delay and threads over slices, never frames,
    /// so a frame that makes a picture makes it in this call.
    fn decode_planes(&mut self, data: &[u8], planes: &mut dyn FnMut(&Frame<'_>)) -> bool {
        matches!(
            self.decode_with(data, false, &mut |frame, _| planes(frame)),
            Decoded::Converted
        )
    }

    fn hardware(&self) -> Option<bool> {
        self.hardware.map(|how| how != 0)
    }

    /// `H.264 vaapi map 7.1 ms`, `copy` for a picture copied rather than
    /// mapped (where the driver will not map it), `dmabuf` for one left on
    /// the GPU, where the time
    /// is the decode and the wait for it and nothing else.
    fn status(&self) -> Option<String> {
        let mut line = self.codec.name().to_string();
        match self.hardware {
            Some(how @ 1..) => {
                line += &format!(
                    " {} {}",
                    hardware_name(self.method()),
                    match how {
                        sys::COPIED => "copy",
                        sys::HELD => "dmabuf",
                        _ => "map",
                    }
                );
            }
            Some(_) => line += " sw",
            None if self.method() != Hardware::Off => line += &format!(" {}", hardware_name(self.method())),
            None => line += " sw",
        }
        if let Some(ms) = self.decode_ms {
            line += &format!(" {ms:.1} ms");
        }
        if !matches!(self.hardware, Some(1..)) && !self.reason.is_empty() {
            line += &format!(" ({})", self.reason);
        }
        Some(line)
    }

    fn take_key_frame_request(&mut self) -> bool {
        std::mem::take(&mut self.key_frame)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { sys::vento_video_close(self.video) };
    }
}

/// A held picture's frame, which the C side lets any thread export, map and
/// free: one map at a time, which the [`Mutex`] it is kept in sees to.
struct FramePtr(*mut sys::Frame);

// SAFETY: see above; the pointer is the frame's only owner, freed once.
unsafe impl Send for FramePtr {}

/// What a held picture is read back with: the decoder's conversion state
/// and buffers for a deep picture's narrowed planes, shared by every picture
/// of a decoder, since a read is rare and each would otherwise log its
/// layout's check again.
#[derive(Default)]
struct Readback {
    output: Output,
    narrow: [Vec<u8>; 3],
}

/// A VAAPI picture left on the GPU (see the module's documentation).
struct Held {
    frame: Mutex<FramePtr>,
    width: usize,
    height: usize,
    pool: u64,
    surface: u32,
    colour: (bool, bool),
    readback: Arc<Mutex<Readback>>,
}

impl GpuPicture for Held {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn surface(&self) -> (u64, u32) {
        (self.pool, self.surface)
    }

    fn colour(&self) -> (bool, bool) {
        self.colour
    }

    fn platform(&self) -> &dyn Any {
        self
    }

    fn read(&self, out: &mut [u32], width: usize, height: usize) -> bool {
        if out.len() < width * height || height == 0 {
            return false;
        }
        let frame = self.frame.lock().unwrap();
        let mut picture = sys::Picture::new();
        if unsafe { sys::vento_video_frame_map(frame.0, &mut picture) } == 0 {
            log::warn!("a held picture could not be mapped for reading");
            return false;
        }
        let mut readback = self.readback.lock().unwrap();
        let Readback { output, narrow } = &mut *readback;
        let read = match planes(&picture, narrow) {
            Some((bytes, planes)) => {
                let frame = Frame {
                    bytes,
                    planes,
                    width: picture.width as usize,
                    height: picture.height as usize,
                    bt709: picture.bt709 != 0,
                    full_range: picture.full_range != 0,
                };
                // Every row: what is in `out` is not this picture's.
                let mut changed = Changes::new();
                changed.reset(height);
                changed.all(width, height);
                output.write(&frame, out, width, height, &mut changed);
                true
            }
            None => false,
        };
        unsafe { sys::vento_video_frame_unmap(frame.0) };
        read
    }
}

impl Held {
    /// The surface as DMA-BUFs (see [`export`]).
    fn export(&self) -> Result<Dmabuf, String> {
        let frame = self.frame.lock().unwrap();
        let mut export = sys::Export::default();
        let mut error = [0 as c_char; 128];
        let made = unsafe {
            sys::vento_video_frame_export(frame.0, &mut export, error.as_mut_ptr(), error.len())
        };
        if made < 0 {
            return Err(text(&error));
        }
        let count = usize::try_from(export.layers).unwrap_or(0).min(2);
        let layers = export.layer[..count]
            .iter()
            .map(|l| DmabufLayer {
                // SAFETY: a descriptor the export made for the caller, and
                // nobody else's.
                fd: unsafe { OwnedFd::from_raw_fd(l.fd) },
                fourcc: l.fourcc,
                modifier: l.modifier,
                offset: l.offset,
                pitch: l.pitch,
                width: l.width,
                height: l.height,
            })
            .collect();
        Ok(Dmabuf {
            layers,
            bt709: export.bt709 != 0,
            full_range: export.full_range != 0,
        })
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let frame = self.frame.get_mut().unwrap();
        unsafe { sys::vento_video_frame_free(frame.0) };
    }
}

/// A VAAPI picture's surface as DMA-BUFs, one a plane, each descriptor a new
/// one of the caller's own: what the window imports into its Vulkan device
/// to draw a picture without reading it back. An error for a picture that
/// is not one of these decoders' (another platform's, a test's) or that
/// VAAPI would not export.
pub fn export(picture: &dyn GpuPicture) -> Result<Dmabuf, String> {
    match picture.platform().downcast_ref::<Held>() {
        Some(held) => held.export(),
        None => Err("the picture is not a VAAPI picture of libavcodec's".into()),
    }
}

/// A [`GpuPicture`]'s surface as DMA-BUFs, with what its numbers mean.
pub struct Dmabuf {
    /// Y, then U and V interleaved (NV12, P010): a single-plane buffer each,
    /// in the order they are read.
    pub layers: Vec<DmabufLayer>,
    /// BT.709 rather than BT.601, and full range rather than limited, as
    /// `convert::Frame` says them.
    pub bt709: bool,
    pub full_range: bool,
}

/// One plane of a [`Dmabuf`]: its descriptor, its DRM format and layout
/// modifier, and where its samples are in it.
pub struct DmabufLayer {
    pub fd: OwnedFd,
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

/// A picture's planes, as `convert` reads them: in place at eight bits, and
/// narrowed into `into` deeper than that or packed (XV30). `None` where the
/// description does not hold together, which the C side should never hand
/// back.
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
    let at = |offset: usize, col_inc: usize, row_inc: usize, h: usize, v: usize| Plane {
        offset,
        col_inc,
        row_inc,
        horiz_subsampling: h,
        vert_subsampling: v,
    };
    // Exactly the bytes the picture covers in a plane of `rows` rows of
    // `row_bytes`: every row but the last at its full line size, and the last
    // only as far as its last sample, since that is all the frame promises is
    // there.
    let plane = |i: usize, rows: usize, row_bytes: usize| -> Option<(&'a [u8], usize)> {
        let stride = usize::try_from(p.stride[i])
            .ok()
            .filter(|s| *s >= row_bytes)?;
        if p.data[i].is_null() {
            return None;
        }
        let len = stride * (rows - 1) + row_bytes;
        // SAFETY: a decoded plane has `stride` bytes for every row of the
        // picture, and the last row as far as its last sample is the least of
        // that. The picture is the decoder's until the next `decode`, and
        // `convert` is done with it before then.
        Some((
            unsafe { std::slice::from_raw_parts(p.data[i], len) },
            stride,
        ))
    };

    if p.packed == sys::XV30 {
        let (src, stride) = plane(0, height, width * 4)?;
        for dst in into.iter_mut() {
            dst.resize(width * height, 0);
        }
        let [y, u, v] = into;
        for row in 0..height {
            let range = row * width..(row + 1) * width;
            unpack_xv30(
                &src[row * stride..row * stride + width * 4],
                [&mut y[range.clone()], &mut u[range.clone()], &mut v[range]],
            );
        }
        let full = at(0, 1, width, 1, 1);
        return Some(([&y[..], &u[..], &v[..]], [full, full, full]));
    }
    if p.packed != 0 {
        return None;
    }

    let (horiz, vert) = (1usize << p.chroma_shift_x, 1usize << p.chroma_shift_y);
    let (cw, ch) = (width.div_ceil(horiz), height.div_ceil(vert));
    let depth = u32::try_from(p.depth)
        .ok()
        .filter(|d| (8..=16).contains(d))?;
    let bytes = if depth > 8 { 2 } else { 1 };
    // The bits a sample's value spans counting from the bottom of its bytes,
    // the zeros below a shifted one included: what it is narrowed from.
    let stored = depth + u32::try_from(p.shift).ok()?;
    if stored > 8 * bytes as u32 {
        return None;
    }
    let interleaved = p.interleaved != 0;
    let row_bytes = [
        width * bytes,
        if interleaved { cw * 2 * bytes } else { cw * bytes },
        cw * bytes,
    ];
    let rows = [height, ch, ch];
    let (u_at, v_at) = if p.v_first != 0 { (1, 0) } else { (0, 1) };

    if depth == 8 {
        let (y, ys) = plane(0, rows[0], row_bytes[0])?;
        let luma = at(0, 1, ys, 1, 1);
        if interleaved {
            let (uv, uvs) = plane(1, rows[1], row_bytes[1])?;
            return Some((
                [y, uv, uv],
                [
                    luma,
                    at(u_at, 2, uvs, horiz, vert),
                    at(v_at, 2, uvs, horiz, vert),
                ],
            ));
        }
        let (u, us) = plane(1, rows[1], row_bytes[1])?;
        let (v, vs) = plane(2, rows[2], row_bytes[2])?;
        return Some((
            [y, u, v],
            [luma, at(0, 1, us, horiz, vert), at(0, 1, vs, horiz, vert)],
        ));
    }

    // Deeper: the whole picture every frame, which is a copy the 8-bit path
    // does not make; what is in doubt for a deep stream is whether there is a
    // picture at all. Interleaved (P010) stays interleaved, at eight bits.
    let count = if interleaved { 2 } else { 3 };
    for (i, dst) in into.iter_mut().enumerate().take(count) {
        let (src, stride) = plane(i, rows[i], row_bytes[i])?;
        let samples = row_bytes[i] / 2;
        dst.resize(samples * rows[i], 0);
        for (row, out) in dst.chunks_exact_mut(samples).enumerate() {
            narrow(
                &src[row * stride..row * stride + row_bytes[i]],
                stored,
                out,
            );
        }
    }
    let [y, u, v] = into;
    if interleaved {
        return Some((
            [&y[..], &u[..], &u[..]],
            [
                at(0, 1, width, 1, 1),
                at(u_at, 2, cw * 2, horiz, vert),
                at(v_at, 2, cw * 2, horiz, vert),
            ],
        ));
    }
    Some((
        [&y[..], &u[..], &v[..]],
        [
            at(0, 1, width, 1, 1),
            at(0, 1, cw, horiz, vert),
            at(0, 1, cw, horiz, vert),
        ],
    ))
}

/// One row of XV30 — a little-endian word a pixel, U in its low ten bits, Y
/// in the next ten and V in the ten above — into a row each of Y, U and V at
/// eight bits, rounded as [`narrow`] rounds.
fn unpack_xv30(src: &[u8], [y, u, v]: [&mut [u8]; 3]) {
    let eight = |ten: u32| ((ten + 2) >> 2).min(255) as u8;
    for (i, word) in src.chunks_exact(4).enumerate() {
        let word = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
        u[i] = eight(word & 0x3ff);
        y[i] = eight((word >> 10) & 0x3ff);
        v[i] = eight((word >> 20) & 0x3ff);
    }
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
    use super::{narrow, unpack_xv30};

    /// P010 has its ten bits at the top of sixteen: narrowed as sixteen bits,
    /// it comes out where the same ten bits at the bottom do.
    #[test]
    fn a_shifted_sample_narrows_as_its_value() {
        let ten = [0u16, 64, 512, 940, 1023];
        let low: Vec<u8> = ten.iter().flat_map(|s| s.to_le_bytes()).collect();
        let high: Vec<u8> = ten.iter().flat_map(|s| (s << 6).to_le_bytes()).collect();
        let (mut a, mut b) = ([0u8; 5], [0u8; 5]);
        narrow(&low, 10, &mut a);
        narrow(&high, 16, &mut b);
        assert_eq!(a, b);
        assert_eq!(a, [0, 16, 128, 235, 255]);
    }

    /// Each of XV30's three fields lands in its own plane, with the top two
    /// bits of the word ignored.
    #[test]
    fn xv30_unpacks_to_three_planes() {
        let word = |y: u32, u: u32, v: u32, x: u32| (x << 30) | (v << 20) | (y << 10) | u;
        let src: Vec<u8> = [word(64, 512, 940, 0), word(940, 64, 1023, 3)]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let (mut y, mut u, mut v) = ([0u8; 2], [0u8; 2], [0u8; 2]);
        unpack_xv30(&src, [&mut y, &mut u, &mut v]);
        assert_eq!(y, [16, 235]);
        assert_eq!(u, [128, 16]);
        assert_eq!(v, [235, 255]);
    }

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
