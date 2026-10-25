// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The graphics pipeline (MS-RDPEGFX), the `Graphics` option's "egfx".
//!
//! IronRDP's client for the channel decodes every codec but H.264 itself
//! (ClearCodec, planar, RemoteFX and its progressive form, solid fills, the
//! bitmap cache), composites the surfaces into the session's image, and
//! acknowledges each frame. What it does not have is an H.264 decoder: that
//! is [`H264`], over the platform's video decoders (`runtime::media::video`,
//! MediaCodec on the phone and libavcodec on a desktop), the way FreeRDP's
//! backend has its library's H.264 decoded (`h264_vento.c`). The library
//! calls it for AVC420 and AVC444 alike, takes the picture's planes, converts
//! them with the transform the protocol specifies, and for AVC444 combines
//! the two pictures itself.
//!
//! Three things of ours live inside the channel: the handler the library
//! reports to ([`Pipeline`]), the decoder ([`H264`]), and what both write
//! into for the session to read ([`State`]): the capabilities the server
//! confirmed, the surfaces and their sizes, which codecs have arrived, and
//! whether the pipeline has to be restarted. The channel itself is made by
//! [`client`] each time the server opens it, since Windows closes and reopens
//! it, and a closed one cannot be reused.

use ironrdp_egfx::client::{BitmapUpdate, GraphicsPipelineClient, GraphicsPipelineHandler, Surface};
use ironrdp_egfx::pdu::MapSurfaceToScaledOutputPdu;
use ironrdp_pdu::geometry::ExclusiveRectangle;
use ironrdp_egfx::decode::{DecodedFrame, DecoderError, DecoderResult, H264Decoder, Yuv420Frame};
use ironrdp_egfx::pdu::{
    CapabilitiesV81Flags, CapabilitiesV10Flags, CapabilitiesV103Flags, CapabilitiesV104Flags, CapabilitiesV107Flags,
    CapabilitySet, Codec1Type,
};
use runtime::media::video::convert::Frame;
use runtime::media::video::{Codec, Decoder, Decoders, Hardware};

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// How often the decoder's status line is read: formatting it costs a little
/// on every platform, and nothing reads it more than a few times a second.
const STATUS_EVERY: u64 = 30;

/// What the session reads of the pipeline. Shared between the handler and
/// the decoder, which live inside the active stage on the protocol thread,
/// and the session's info, read from any thread.
#[derive(Default)]
pub struct State {
    /// The capability set the server confirmed, or `None` until it has, and
    /// again once the channel closed.
    caps: Mutex<Option<CapabilitySet>>,
    /// The graphics output's size, from the last `ResetGraphics`, and
    /// before any the desktop's size: what a decoder is opened at for a
    /// surface whose size is not known.
    output: Mutex<(usize, usize)>,
    /// The surfaces the server has created, by id, with their sizes, which
    /// is what each one's H.264 decoder is opened at.
    surfaces: Mutex<BTreeMap<u16, (usize, usize)>>,
    /// How many updates of each codec have arrived, by `Codec1Type`'s wire
    /// value: logged the first time each is seen, and for the report.
    seen: Mutex<BTreeMap<u16, u64>>,
    /// Frames completed (`EndFrame`), each acknowledged by the library.
    frames: AtomicU64,
    /// Whether the pipeline has to be restarted (its capabilities advertised
    /// again, which makes the server start new streams): set by the decoder
    /// when the decoding mode changed under it or a decoder lost the
    /// pictures its stream refers back to; taken by the session, which sends
    /// the request.
    restart: AtomicBool,
    /// Whether the last H.264 picture was decoded in hardware, or `None`
    /// before the first one.
    hardware: Mutex<Option<bool>>,
    /// The decoder's own status line, for an overlay.
    status: Mutex<Option<String>>,
    /// Whether the session draws AVC420 itself: set once, before the channel
    /// opens, by a session whose window has a layer for video. AVC444 is
    /// always converted by the library.
    video: AtomicBool,
    /// Where on the desktop each surface that is mapped 1:1 to the output
    /// sits. Only these surfaces' pictures are handed over undrawn; a scaled
    /// one (or one not mapped) is composited, which scales it.
    origins: Mutex<BTreeMap<u16, (usize, usize)>>,
    /// The pictures made since the session last took them, at most one per
    /// rectangle.
    pictures: Mutex<Vec<Picture>>,
}

/// A decoded AVC420 picture that was not composited: the `width` by `height`
/// rectangle at (`x`, `y`) of the desktop, as NV12.
#[derive(Debug, PartialEq, Eq)]
pub struct Picture {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub nv12: Vec<u8>,
}

/// `runtime::Handler::video`'s colour mode for what MS-RDPEGFX specifies for
/// AVC420: BT.709, full range.
pub const COLOR_MODE: i32 = 0b11;

impl State {
    /// The desktop's size at the session's start, before the server has
    /// said what the graphics output's is.
    pub fn set_output(&self, width: usize, height: usize) {
        *self.output.lock().unwrap() = (width, height);
    }

    /// The size an H.264 decoder for `surface_id` is opened at: the
    /// surface's own, else the output's.
    fn size_of(&self, surface_id: u16) -> (usize, usize) {
        self.surfaces
            .lock()
            .unwrap()
            .get(&surface_id)
            .copied()
            .unwrap_or_else(|| *self.output.lock().unwrap())
    }

    /// Whether the session's window draws AVC420 itself.
    pub fn set_video(&self, on: bool) {
        self.video.store(on, Ordering::Relaxed);
    }

    /// The pictures that were handed over undrawn since the last call, in
    /// the order they were made.
    pub fn take_pictures(&self) -> Vec<Picture> {
        std::mem::take(&mut *self.pictures.lock().unwrap())
    }

    /// Whether a restart was asked for since the last call.
    pub fn take_restart(&self) -> bool {
        self.restart.swap(false, Ordering::Relaxed)
    }

    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// How many updates of each codec have arrived, by name.
    pub fn codecs(&self) -> Vec<(&'static str, u64)> {
        self.seen.lock().unwrap().iter().map(|(&codec, &n)| (codec_name(codec), n)).collect()
    }

    /// The decoder's status line, or `None` without one.
    pub fn status(&self) -> Option<String> {
        self.status.lock().unwrap().clone()
    }

    /// The encoding as the connection panel shows it, worded as FreeRDP's
    /// backend words its own: the version the server confirmed, whether it
    /// may send H.264, and where that is decoded once some has been. Empty
    /// while the channel is not open.
    pub fn encoding(&self) -> String {
        let Some(caps) = self.caps.lock().unwrap().clone() else {
            return String::new();
        };
        let version = caps.version().0;
        let mut encoding = format!("EGFX {}.{}", (version >> 16) & 0xffff, (version >> 8) & 0xff);
        if avc_of(&caps) != (false, false) {
            encoding += ", H.264";
            match *self.hardware.lock().unwrap() {
                Some(true) => encoding += " (hardware)",
                Some(false) => encoding += " (software)",
                None => {}
            }
        }
        encoding
    }
}

/// Whether a confirmed capability set lets the server send AVC420 and
/// AVC444: version 8 neither, 8.1 by its flag, and 10 and later both unless
/// the server disabled them. The library's own reading of the set, which it
/// does not expose.
fn avc_of(caps: &CapabilitySet) -> (bool, bool) {
    match caps {
        CapabilitySet::V8 { .. } => (false, false),
        CapabilitySet::V8_1 { flags } => (flags.contains(CapabilitiesV81Flags::AVC420_ENABLED), false),
        CapabilitySet::V10 { flags } | CapabilitySet::V10_2 { flags } => {
            let on = !flags.contains(CapabilitiesV10Flags::AVC_DISABLED);
            (on, on)
        }
        CapabilitySet::V10_1 => (true, true),
        CapabilitySet::V10_3 { flags } => {
            let on = !flags.contains(CapabilitiesV103Flags::AVC_DISABLED);
            (on, on)
        }
        CapabilitySet::V10_4 { flags }
        | CapabilitySet::V10_5 { flags }
        | CapabilitySet::V10_6 { flags }
        | CapabilitySet::V10_6Err { flags } => {
            let on = !flags.contains(CapabilitiesV104Flags::AVC_DISABLED);
            (on, on)
        }
        CapabilitySet::V10_7 { flags } => {
            let on = !flags.contains(CapabilitiesV107Flags::AVC_DISABLED);
            (on, on)
        }
    }
}

/// A codec's name, by its wire value.
fn codec_name(codec: u16) -> &'static str {
    match Codec1Type::try_from(codec) {
        Ok(Codec1Type::Uncompressed) => "uncompressed",
        Ok(Codec1Type::RemoteFx) => "RemoteFX",
        Ok(Codec1Type::ClearCodec) => "ClearCodec",
        Ok(Codec1Type::Planar) => "planar",
        Ok(Codec1Type::Avc420) => "AVC420",
        Ok(Codec1Type::Alpha) => "alpha",
        Ok(Codec1Type::Avc444) => "AVC444",
        Ok(Codec1Type::Avc444v2) => "AVC444v2",
        Err(_) => "unknown",
    }
}

/// The channel's client for one opening of it, reporting to `state`, with
/// an H.264 decoder over `decoders` where there are any. Without them the
/// channel offers no AVC capability set and the server sends its other
/// codecs.
pub fn client(state: Arc<State>, decoders: Option<Arc<dyn Decoders>>) -> GraphicsPipelineClient {
    let decoder = decoders.map(|decoders| Box::new(H264::new(decoders, Arc::clone(&state))) as Box<dyn H264Decoder>);
    GraphicsPipelineClient::new(Box::new(Pipeline { state }), decoder)
}

/// What the library tells about the channel, written into the [`State`]
/// and logged. The picture itself mostly does not come through here: the
/// library composites it into the session's image, and the handler's copy of
/// each update is only counted. The exception is AVC420 on a surface mapped
/// 1:1, when the session's window draws video itself ([`State::set_video`]):
/// the library hands the decoder's planes over undrawn, and they are cut
/// into [`Picture`]s for the session to take.
struct Pipeline {
    state: Arc<State>,
}

impl GraphicsPipelineHandler for Pipeline {
    fn on_capabilities_confirmed(&mut self, caps: &CapabilitySet) {
        let (avc420, avc444) = avc_of(caps);
        let version = caps.version().0;
        log::info!(
            "graphics pipeline: version {}.{} confirmed, AVC420 {}, AVC444 {}",
            (version >> 16) & 0xffff,
            (version >> 8) & 0xff,
            if avc420 { "on" } else { "off" },
            if avc444 { "on" } else { "off" }
        );
        *self.state.caps.lock().unwrap() = Some(caps.clone());
    }

    fn on_reset_graphics(&mut self, width: u32, height: u32) {
        log::info!("graphics pipeline: output reset to {width}x{height}");
        *self.state.output.lock().unwrap() = (width as usize, height as usize);
        // The reset deletes every surface (MS-RDPEGFX 3.3.5.14), and the
        // library tells of none of them.
        self.state.surfaces.lock().unwrap().clear();
        self.state.origins.lock().unwrap().clear();
    }

    fn on_reset_graphics_rejected(&mut self, width: u32, height: u32) {
        log::warn!("graphics pipeline: the server reset the output to {width}x{height}, which is too large to draw");
    }

    fn on_surface_created(&mut self, surface: &Surface) {
        log::debug!("graphics pipeline: surface {} created, {}x{}", surface.id, surface.width, surface.height);
        self.state
            .surfaces
            .lock()
            .unwrap()
            .insert(surface.id, (usize::from(surface.width), usize::from(surface.height)));
    }

    fn on_surface_deleted(&mut self, surface_id: u16) {
        log::debug!("graphics pipeline: surface {surface_id} deleted");
        self.state.surfaces.lock().unwrap().remove(&surface_id);
        self.state.origins.lock().unwrap().remove(&surface_id);
    }

    fn on_surface_mapped(&mut self, surface_id: u16, origin_x: u32, origin_y: u32) {
        self.state
            .origins
            .lock()
            .unwrap()
            .insert(surface_id, (origin_x as usize, origin_y as usize));
    }

    /// Reported after `on_surface_mapped` for the same surface, which a
    /// scaled mapping also is: the picture is then scaled by the compositor
    /// and not ours to draw.
    fn on_map_surface_to_scaled_output(&mut self, pdu: &MapSurfaceToScaledOutputPdu) {
        self.state.origins.lock().unwrap().remove(&pdu.surface_id);
    }

    fn takes_video_frames(&self, surface_id: u16) -> bool {
        self.state.video.load(Ordering::Relaxed) && self.state.origins.lock().unwrap().contains_key(&surface_id)
    }

    fn on_video_frame(&mut self, surface_id: u16, destination: &ExclusiveRectangle, frame: &Yuv420Frame) {
        let Some(&(origin_x, origin_y)) = self.state.origins.lock().unwrap().get(&surface_id) else {
            return;
        };
        let (output_width, output_height) = *self.state.output.lock().unwrap();
        // The picture's pixel is the surface's, so the rectangle is cut from
        // it where it is: from the even pixel at or before its corner, since
        // NV12 shares a chroma sample between two, and to the least of the
        // rectangle's, the picture's and the desktop's far edges.
        let left = usize::from(destination.left) & !1;
        let top = usize::from(destination.top) & !1;
        let right = usize::from(destination.right)
            .min(usize::from(frame.width()))
            .min(output_width.saturating_sub(origin_x));
        let bottom = usize::from(destination.bottom)
            .min(usize::from(frame.height()))
            .min(output_height.saturating_sub(origin_y));
        if right <= left || bottom <= top {
            return;
        }
        let Some(nv12) = nv12(frame, left, top, right, bottom) else {
            log::warn!("graphics pipeline: a picture smaller than the rows it was cut from; not drawn");
            return;
        };
        let picture = Picture {
            x: origin_x + left,
            y: origin_y + top,
            width: right - left,
            height: bottom - top,
            nv12,
        };
        // A session that has not taken the last picture of this rectangle
        // never will need it.
        let mut pictures = self.state.pictures.lock().unwrap();
        pictures.retain(|p| (p.x, p.y, p.width, p.height) != (picture.x, picture.y, picture.width, picture.height));
        pictures.push(picture);
    }

    fn on_bitmap_updated(&mut self, update: &BitmapUpdate) {
        let mut seen = self.state.seen.lock().unwrap();
        let n = seen.entry(update.codec_id as u16).or_insert(0);
        if *n == 0 {
            log::info!(
                "graphics pipeline: the first {} update, {}x{} on surface {}",
                codec_name(update.codec_id as u16),
                update.width,
                update.height,
                update.surface_id
            );
        }
        *n += 1;
    }

    fn on_frame_complete(&mut self, _frame_id: u32) {
        let frames = self.state.frames.fetch_add(1, Ordering::Relaxed) + 1;
        if frames.is_multiple_of(256) {
            log::debug!("graphics pipeline: {frames} frames, updates by codec {:?}", self.state.codecs());
        }
    }

    fn on_close(&mut self) {
        log::info!("graphics pipeline: channel closed");
        *self.state.caps.lock().unwrap() = None;
        self.state.surfaces.lock().unwrap().clear();
        self.state.origins.lock().unwrap().clear();
    }
}

/// The `left, top` to `right, bottom` rectangle of a picture as NV12: its Y
/// rows, then its U and V rows interleaved. `left` and `top` are even, and
/// the rectangle is within the picture; `None` where the picture has fewer
/// rows than that.
fn nv12(frame: &Yuv420Frame, left: usize, top: usize, right: usize, bottom: usize) -> Option<Vec<u8>> {
    let (width, height) = (right - left, bottom - top);
    let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
    let mut out = Vec::with_capacity(width * height + chroma_width * chroma_height * 2);
    for row in top..bottom {
        out.extend_from_slice(frame.luma_row(row)?.get(left..right)?);
    }
    let first = left / 2;
    for row in top / 2..top / 2 + chroma_height {
        let u = frame.u_row(row)?.get(first..first + chroma_width)?;
        let v = frame.v_row(row)?.get(first..first + chroma_width)?;
        for (u, v) in u.iter().zip(v) {
            out.extend([*u, *v]);
        }
    }
    Some(out)
}

/// The library's H.264 decoder, over the platform's.
///
/// One of the platform's decoders per surface, opened at the surface's size
/// on its first frame: each surface's updates are an H.264 stream of their
/// own (Windows encodes each monitor separately), whose pictures refer back
/// to that stream's alone, so two surfaces through one decoder would each
/// corrupt the other. A surface's decoder goes with the surface, and all of
/// them with a reset.
///
/// Two things the decoders report are answered by restarting the pipeline
/// rather than by the decoder, since a new decoder mid-stream has none of
/// the pictures the next frames refer to and Windows sends a key frame only
/// when a stream starts: the decoding mode changing under a session (the
/// user's hardware setting, read on every frame as RustDesk's client reads
/// it), and a decoder that lost its pictures (a hardware decoder that failed
/// mid-stream and was replaced by a software one). Both set [`State`]'s
/// restart flag; the session advertises the capabilities again and the
/// server answers with new surfaces and new streams.
pub struct H264 {
    decoders: Arc<dyn Decoders>,
    state: Arc<State>,
    streams: BTreeMap<u16, Stream>,
    /// Surfaces no decoder would open for at their size, so that the
    /// failure is logged once rather than for every frame.
    refused: BTreeSet<u16>,
    /// A restart has been asked for a mode change and has not come yet.
    restart_asked: bool,
    /// AVC-format input converted to Annex B, kept to be reused.
    annex_b: Vec<u8>,
    /// Pictures decoded, for the status line's cadence.
    decoded: u64,
}

/// One surface's stream.
struct Stream {
    decoder: Box<dyn Decoder>,
    /// The decoding mode it was opened in.
    hardware: Hardware,
}

// SAFETY: the library requires `Send` of a decoder because its channel
// processors are boxed as `Send`, and `Decoder` is not `Send` (libavcodec's
// context is a raw one, and the trait asks nothing more of a decoder than
// that it is made, fed and dropped on one thread). This one is: the channel
// lives inside the active stage, which the protocol thread owns for the whole
// session, and `client` makes it there. The decoders themselves are the ones
// `api::c::video` already calls from TigerVNC's worker threads, one call at
// a time, which is more than this asks.
unsafe impl Send for H264 {}

impl H264 {
    fn new(decoders: Arc<dyn Decoders>, state: Arc<State>) -> H264 {
        H264 {
            decoders,
            state,
            streams: BTreeMap::new(),
            refused: BTreeSet::new(),
            restart_asked: false,
            annex_b: Vec::new(),
            decoded: 0,
        }
    }

    /// Opens the stream of `surface_id` at the surface's size, in the
    /// decoding mode the platform answers now, if it is not open yet.
    fn open(&mut self, surface_id: u16) -> DecoderResult<()> {
        if self.streams.contains_key(&surface_id) {
            return Ok(());
        }
        if self.refused.contains(&surface_id) {
            return Err(DecoderError::msg("no decoder for this surface"));
        }
        let (width, height) = self.state.size_of(surface_id);
        let mode = self.decoders.hardware();
        let Some(decoder) = self.decoders.decoder(Codec::H264, width, height) else {
            log::warn!(
                "graphics pipeline: no H.264 decoder for {width}x{height}; \
                 surface {surface_id} is not drawn"
            );
            self.refused.insert(surface_id);
            return Err(DecoderError::msg(format!("no H.264 decoder for {width}x{height}")));
        };
        log::info!("graphics pipeline: H.264 decoder for surface {surface_id} at {width}x{height}, {mode:?}");
        self.streams.insert(surface_id, Stream { decoder, hardware: mode });
        Ok(())
    }

    fn ask_restart(&self, why: &str) {
        log::info!("graphics pipeline: {why}: restarting the pipeline");
        self.state.restart.store(true, Ordering::Relaxed);
    }
}

impl H264Decoder for H264 {
    fn decode(&mut self, data: &[u8]) -> DecoderResult<DecodedFrame> {
        self.decode_yuv420(data).map(|picture| picture.to_rgba())
    }

    /// The library calls the per-surface form; a caller without a surface
    /// gets a stream of its own.
    fn decode_yuv420(&mut self, data: &[u8]) -> DecoderResult<Yuv420Frame> {
        self.decode_yuv420_for_surface(u16::MAX, data)
    }

    fn decode_for_surface(&mut self, surface_id: u16, data: &[u8]) -> DecoderResult<DecodedFrame> {
        self.decode_yuv420_for_surface(surface_id, data)
            .map(|picture| picture.to_rgba())
    }

    fn decode_yuv420_for_surface(&mut self, surface_id: u16, data: &[u8]) -> DecoderResult<Yuv420Frame> {
        let mode = self.decoders.hardware();
        if !self.restart_asked && self.streams.get(&surface_id).is_some_and(|s| s.hardware != mode) {
            // Asked once; this decoder keeps decoding until the new streams
            // come, whose decoders open in the new mode.
            self.restart_asked = true;
            self.ask_restart(&format!("video decoding changed to {mode:?}"));
        }
        self.open(surface_id)?;
        // The protocol's stream is Annex B; a server that followed the
        // library's earlier documentation sends length-prefixed units, which
        // both platforms' decoders would refuse.
        let data = if ironrdp_egfx::pdu::is_avc_format(data) {
            ironrdp_egfx::pdu::avc_to_annex_b_into(data, &mut self.annex_b);
            &self.annex_b[..]
        } else {
            data
        };
        let decoder = &mut self.streams.get_mut(&surface_id).expect("opened above").decoder;
        let mut picture = None;
        let made = decoder.decode_planes(data, &mut |frame| picture = Some(planes(frame)));
        let lost = decoder.take_key_frame_request();
        *self.state.hardware.lock().unwrap() = decoder.hardware();
        self.decoded += 1;
        if self.decoded % STATUS_EVERY == 1 {
            *self.state.status.lock().unwrap() = decoder.status();
        }
        if lost {
            self.ask_restart("the H.264 decoder lost the pictures its stream refers back to");
        }
        match picture {
            Some(picture) if made => picture,
            _ => Err(DecoderError::msg("the decoder made no picture of this frame")),
        }
    }

    fn supports_yuv420(&self) -> bool {
        true
    }

    /// Whether the platform decodes H.264 at all: asked when the channel
    /// opens, and without it no AVC capability set is advertised, so the
    /// server sends its other codecs.
    fn wait_until_ready(&mut self) -> bool {
        let ready = self.decoders.decodable(Codec::H264);
        if !ready {
            log::info!("graphics pipeline: no H.264 decoder on this platform; AVC is not offered");
        }
        ready
    }

    fn delete_surface(&mut self, surface_id: u16) {
        self.streams.remove(&surface_id);
        self.refused.remove(&surface_id);
    }

    fn reset(&mut self) {
        self.streams.clear();
        self.refused.clear();
        self.restart_asked = false;
    }
}

/// A decoded picture's planes copied into the library's, which owns its
/// planes and reads them with a stride of its own: the decoders lend theirs
/// for one call, in whatever layout the decoder left them (NV12's
/// interleaved chroma off a GPU, MediaCodec's one buffer with three planes
/// at offsets), as `i420::I420` copies them for FreeRDP.
fn planes(frame: &Frame<'_>) -> DecoderResult<Yuv420Frame> {
    let (Ok(width), Ok(height)) = (u16::try_from(frame.width), u16::try_from(frame.height)) else {
        return Err(DecoderError::msg("the picture is larger than 65535 pixels a side"));
    };
    let luma = &frame.planes[0];
    if !frame.fits()
        || luma.horiz_subsampling != 1
        || luma.vert_subsampling != 1
        || frame.planes[1..]
            .iter()
            .any(|p| p.horiz_subsampling != 2 || p.vert_subsampling != 2)
    {
        return Err(DecoderError::msg("the decoder's picture is not 4:2:0 in the bytes it came in"));
    }
    let chroma = (frame.width.div_ceil(2), frame.height.div_ceil(2));
    let copy = |i: usize, (w, h): (usize, usize)| -> Vec<u8> {
        let plane = &frame.planes[i];
        let bytes = frame.bytes[i];
        let mut out = Vec::with_capacity(w * h);
        for row in 0..h {
            let src = &bytes[plane.offset + row * plane.row_inc..];
            if plane.col_inc == 1 {
                out.extend_from_slice(&src[..w]);
            } else {
                out.extend(src.iter().step_by(plane.col_inc).take(w));
            }
        }
        out
    };
    let y = copy(0, (frame.width, frame.height));
    let u = copy(1, chroma);
    let v = copy(2, chroma);
    Yuv420Frame::new(width, height, y, frame.width, u, v, chroma.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp_dvc::DvcProcessor as _;
    use runtime::media::video::convert::Plane;
    use runtime::media::video::Changes;
    use std::sync::atomic::AtomicUsize;

    /// Decoders that count what they make and hand back a fixed 4x2 NV12
    /// picture for any frame starting with a byte of 1, and nothing for any
    /// other; a frame starting with 2 is one after which the decoder says it
    /// lost its pictures.
    struct Fake {
        made: AtomicUsize,
        sizes: Mutex<Vec<(usize, usize)>>,
        hardware: Mutex<Hardware>,
        h264: bool,
        fed: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl Fake {
        fn new(h264: bool) -> Arc<Fake> {
            Arc::new(Fake {
                made: AtomicUsize::new(0),
                sizes: Mutex::new(Vec::new()),
                hardware: Mutex::new(Hardware::Off),
                h264,
                fed: Arc::new(Mutex::new(Vec::new())),
            })
        }
    }

    struct FakeDecoder {
        fed: Arc<Mutex<Vec<Vec<u8>>>>,
        lost: bool,
    }

    impl Decoder for FakeDecoder {
        fn decode(&mut self, _: &[u8], _: &mut [u32], _: usize, _: usize, _: &mut Changes) -> bool {
            false
        }

        fn decode_planes(&mut self, data: &[u8], planes: &mut dyn FnMut(&Frame<'_>)) -> bool {
            self.fed.lock().unwrap().push(data.to_vec());
            self.lost = data.first() == Some(&2);
            if data.first() != Some(&1) && !self.lost {
                return false;
            }
            // Y 4x2 then NV12 chroma for one row of two pairs.
            let bytes = [10, 11, 12, 13, 14, 15, 16, 17, 100, 200, 101, 201];
            let at = |offset, col_inc, row_inc, sub| Plane {
                offset,
                col_inc,
                row_inc,
                horiz_subsampling: sub,
                vert_subsampling: sub,
            };
            planes(&Frame::one(&bytes, [at(0, 1, 4, 1), at(8, 2, 4, 2), at(9, 2, 4, 2)], 4, 2, true, true));
            true
        }

        fn hardware(&self) -> Option<bool> {
            Some(false)
        }

        fn take_key_frame_request(&mut self) -> bool {
            std::mem::take(&mut self.lost)
        }
    }

    impl Decoders for Fake {
        fn decodable(&self, codec: Codec) -> bool {
            self.h264 && codec == Codec::H264
        }

        fn decoder(&self, codec: Codec, width: usize, height: usize) -> Option<Box<dyn Decoder>> {
            if !self.decodable(codec) {
                return None;
            }
            self.made.fetch_add(1, Ordering::Relaxed);
            self.sizes.lock().unwrap().push((width, height));
            Some(Box::new(FakeDecoder {
                fed: Arc::clone(&self.fed),
                lost: false,
            }))
        }

        fn hardware(&self) -> Hardware {
            *self.hardware.lock().unwrap()
        }
    }

    fn surface(state: &State, id: u16, w: usize, h: usize) {
        state.surfaces.lock().unwrap().insert(id, (w, h));
    }

    /// One decoder per surface, at the surface's size, gone with the
    /// surface and with a reset.
    #[test]
    fn a_decoder_per_surface_at_its_size() {
        let decoders = Fake::new(true);
        let state = Arc::new(State::default());
        state.set_output(1920, 1080);
        surface(&state, 1, 800, 600);
        surface(&state, 2, 1024, 768);
        let mut h264 = H264::new(Arc::clone(&decoders) as Arc<dyn Decoders>, Arc::clone(&state));
        assert!(h264.wait_until_ready());
        assert!(h264.decode_yuv420_for_surface(1, &[1]).is_ok());
        assert!(h264.decode_yuv420_for_surface(2, &[1]).is_ok());
        assert!(h264.decode_yuv420_for_surface(1, &[1]).is_ok());
        // An unknown surface is opened at the output's size.
        assert!(h264.decode_yuv420_for_surface(7, &[1]).is_ok());
        assert_eq!(
            *decoders.sizes.lock().unwrap(),
            vec![(800, 600), (1024, 768), (1920, 1080)]
        );
        h264.delete_surface(1);
        assert!(h264.decode_yuv420_for_surface(1, &[1]).is_ok());
        assert_eq!(decoders.made.load(Ordering::Relaxed), 4);
        h264.reset();
        assert!(h264.decode_yuv420_for_surface(2, &[1]).is_ok());
        assert_eq!(decoders.made.load(Ordering::Relaxed), 5);
        // A frame that makes no picture is an error, not a stale picture.
        assert!(h264.decode_yuv420_for_surface(2, &[0]).is_err());
        assert!(!state.take_restart());
    }

    /// The planes come out as the library reads them, whatever layout the
    /// decoder lent them in (NV12 here).
    #[test]
    fn the_planes_are_copied_row_by_row() {
        let decoders = Fake::new(true);
        let state = Arc::new(State::default());
        state.set_output(4, 2);
        let mut h264 = H264::new(decoders as Arc<dyn Decoders>, state);
        let picture = h264.decode_yuv420(&[1]).unwrap();
        assert_eq!((picture.width(), picture.height()), (4, 2));
        assert_eq!(picture.luma_row(0).unwrap(), [10, 11, 12, 13]);
        assert_eq!(picture.luma_row(1).unwrap(), [14, 15, 16, 17]);
        assert_eq!(picture.u_row(0).unwrap(), [100, 101]);
        assert_eq!(picture.v_row(0).unwrap(), [200, 201]);
        assert!(picture.luma_row(2).is_none());
    }

    /// Length-prefixed units reach the decoder as Annex B; a stream that
    /// already is reaches it as it came.
    #[test]
    fn avc_format_input_is_converted_to_annex_b() {
        let decoders = Fake::new(true);
        let state = Arc::new(State::default());
        state.set_output(4, 2);
        let mut h264 = H264::new(Arc::clone(&decoders) as Arc<dyn Decoders>, state);
        let _ = h264.decode_yuv420(&[0, 0, 0, 2, 1, 0xaa]);
        let _ = h264.decode_yuv420(&[0, 0, 0, 1, 1, 0xaa]);
        assert_eq!(
            *decoders.fed.lock().unwrap(),
            vec![vec![0, 0, 0, 1, 1, 0xaa], vec![0, 0, 0, 1, 1, 0xaa]]
        );
    }

    /// A mode change under a stream asks for one restart; a decoder losing
    /// its pictures asks for one too.
    #[test]
    fn a_mode_change_or_lost_pictures_restart_the_pipeline() {
        let decoders = Fake::new(true);
        let state = Arc::new(State::default());
        state.set_output(4, 2);
        let mut h264 = H264::new(Arc::clone(&decoders) as Arc<dyn Decoders>, Arc::clone(&state));
        assert!(h264.decode_yuv420_for_surface(1, &[1]).is_ok());
        assert!(!state.take_restart());
        *decoders.hardware.lock().unwrap() = Hardware::Auto;
        assert!(h264.decode_yuv420_for_surface(1, &[1]).is_ok());
        assert!(state.take_restart());
        // Asked once, and the stream keeps decoding meanwhile.
        assert!(h264.decode_yuv420_for_surface(1, &[1]).is_ok());
        assert!(!state.take_restart());
        assert_eq!(decoders.made.load(Ordering::Relaxed), 1);
        // The reset the restart brings opens the next stream in the new mode.
        h264.reset();
        assert!(h264.decode_yuv420_for_surface(1, &[1]).is_ok());
        assert_eq!(decoders.made.load(Ordering::Relaxed), 2);
        assert!(h264.decode_yuv420_for_surface(1, &[1]).is_ok());
        assert!(!state.take_restart());
        assert!(h264.decode_yuv420_for_surface(1, &[2]).is_ok());
        assert!(state.take_restart());
    }

    /// Without an H.264 decoder the channel advertises no AVC set, and with
    /// one it advertises AVC444 (every version 10 set) down to version 8.
    #[test]
    fn the_capabilities_advertised_follow_the_decoder() {
        use ironrdp_egfx::pdu::{CapabilitiesAdvertisePdu, GfxPdu};
        let advertised = |decoders: Arc<Fake>| {
            let mut client = client(Arc::new(State::default()), Some(decoders as Arc<dyn Decoders>));
            let messages = client.start(1).unwrap();
            assert_eq!(messages.len(), 1);
            let bytes = ironrdp_core::encode_vec(messages[0].as_ref()).unwrap();
            let GfxPdu::CapabilitiesAdvertise(CapabilitiesAdvertisePdu(sets)) =
                ironrdp_core::decode::<GfxPdu>(&bytes).unwrap()
            else {
                panic!("not a capabilities advertise");
            };
            sets.iter()
                .map(|set| {
                    let caps = set.parsed().unwrap().unwrap();
                    (caps.version().0, avc_of(&caps))
                })
                .collect::<Vec<_>>()
        };
        let with = advertised(Fake::new(true));
        assert_eq!(with[0], (0xa_0701, (true, true)));
        assert!(with.iter().any(|&(v, _)| v == 0x8_0105));
        let without = advertised(Fake::new(false));
        assert!(without.iter().all(|&(_, avc)| avc == (false, false)));
        assert!(without.iter().any(|&(v, _)| v == 0x8_0004));
        assert!(without.iter().all(|&(v, _)| v != 0x8_0105 && v < 0xa_0000));
    }

    /// The panel's wording: version, H.264 if the server may send it, and
    /// where it is decoded once it has been.
    #[test]
    fn the_encoding_is_worded_as_freerdps() {
        let state = State::default();
        assert_eq!(state.encoding(), "");
        let mut pipeline = Pipeline {
            state: Arc::new(State::default()),
        };
        pipeline.on_capabilities_confirmed(&CapabilitySet::V8 {
            flags: ironrdp_egfx::pdu::CapabilitiesV8Flags::empty(),
        });
        assert_eq!(pipeline.state.encoding(), "EGFX 8.0");
        pipeline.on_capabilities_confirmed(&CapabilitySet::V10_6 {
            flags: CapabilitiesV104Flags::empty(),
        });
        assert_eq!(pipeline.state.encoding(), "EGFX 10.6, H.264");
        *pipeline.state.hardware.lock().unwrap() = Some(true);
        assert_eq!(pipeline.state.encoding(), "EGFX 10.6, H.264 (hardware)");
        pipeline.on_capabilities_confirmed(&CapabilitySet::V10_6 {
            flags: CapabilitiesV104Flags::AVC_DISABLED,
        });
        assert_eq!(pipeline.state.encoding(), "EGFX 10.6");
        pipeline.on_close();
        assert_eq!(pipeline.state.encoding(), "");
    }

    /// An 8x4 picture whose Y is `10 * row + col` and whose chroma is the
    /// row and column of its sample in U (`16 * row + col`) and V (the same
    /// plus 100).
    fn picture() -> Yuv420Frame {
        let y = (0..4).flat_map(|r| (0..8).map(move |c| (10 * r + c) as u8)).collect();
        let u: Vec<u8> = (0..2).flat_map(|r| (0..4).map(move |c| (16 * r + c) as u8)).collect();
        let v = u.iter().map(|b| b + 100).collect();
        Yuv420Frame::new(8, 4, y, 8, u, v, 4).unwrap()
    }

    /// NV12 of a rectangle: its Y rows, then U and V interleaved, from the
    /// chroma samples that sit under it.
    #[test]
    fn a_rectangle_is_cut_out_as_nv12() {
        let whole = nv12(&picture(), 0, 0, 8, 4).unwrap();
        assert_eq!(whole.len(), 8 * 4 + 4 * 2 * 2);
        assert_eq!(whole[..8], [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(whole[32..40], [0, 100, 1, 101, 2, 102, 3, 103]);
        assert_eq!(whole[40..], [16, 116, 17, 117, 18, 118, 19, 119]);
        // From (2, 2) to (7, 4): five wide, so three chroma pairs, from the
        // second sample of the second chroma row.
        let part = nv12(&picture(), 2, 2, 7, 4).unwrap();
        assert_eq!(part[..5], [22, 23, 24, 25, 26]);
        assert_eq!(part[5..10], [32, 33, 34, 35, 36]);
        assert_eq!(part[10..], [17, 117, 18, 118, 19, 119]);
        // Past the picture's rows, nothing.
        assert!(nv12(&picture(), 0, 0, 8, 6).is_none());
    }

    fn pipeline_with_video() -> Pipeline {
        let state = Arc::new(State::default());
        state.set_output(100, 50);
        state.set_video(true);
        Pipeline { state }
    }

    /// A surface mapped 1:1 hands its pictures over, placed on the desktop and
    /// cut to it; a scaled, an unmapped and a deleted one do not, and nothing
    /// does while the session has not asked.
    #[test]
    fn only_a_surface_mapped_one_to_one_hands_its_pictures_over() {
        let mut p = pipeline_with_video();
        assert!(!p.takes_video_frames(1));
        p.on_surface_mapped(1, 10, 20);
        assert!(p.takes_video_frames(1));
        let rect = ExclusiveRectangle {
            left: 3,
            top: 1,
            right: 8,
            bottom: 4,
        };
        p.on_video_frame(1, &rect, &picture());
        let pictures = p.state.take_pictures();
        // From the even pixel at or before the corner, (2, 0), to the
        // rectangle's far edge: (2, 0)-(8, 4), at (10, 20) on the desktop
        // plus that.
        assert_eq!(
            pictures.iter().map(|p| (p.x, p.y, p.width, p.height)).collect::<Vec<_>>(),
            vec![(12, 20, 6, 4)]
        );
        assert!(p.state.take_pictures().is_empty());
        // Cut to the desktop: surface 2 sits at (96, 48) on a 100x50 one.
        p.on_surface_mapped(2, 96, 48);
        p.on_video_frame(2, &ExclusiveRectangle { left: 0, top: 0, right: 8, bottom: 4 }, &picture());
        let pictures = p.state.take_pictures();
        assert_eq!(
            pictures.iter().map(|p| (p.x, p.y, p.width, p.height)).collect::<Vec<_>>(),
            vec![(96, 48, 4, 2)]
        );
        // Scaled afterwards, which the library reports after the mapping.
        p.on_map_surface_to_scaled_output(&MapSurfaceToScaledOutputPdu {
            surface_id: 1,
            output_origin_x: 10,
            output_origin_y: 20,
            target_width: 16,
            target_height: 8,
        });
        assert!(!p.takes_video_frames(1));
        p.on_surface_deleted(2);
        assert!(!p.takes_video_frames(2));
        p.state.set_video(false);
        p.on_surface_mapped(3, 0, 0);
        assert!(!p.takes_video_frames(3));
    }

    /// A newer picture of a rectangle replaces one the session has not taken.
    #[test]
    fn an_untaken_picture_is_replaced_by_the_next_of_its_rectangle() {
        let mut p = pipeline_with_video();
        p.on_surface_mapped(1, 0, 0);
        let rect = ExclusiveRectangle { left: 0, top: 0, right: 8, bottom: 4 };
        p.on_video_frame(1, &rect, &picture());
        let other = ExclusiveRectangle { left: 0, top: 0, right: 4, bottom: 4 };
        p.on_video_frame(1, &other, &picture());
        p.on_video_frame(1, &rect, &picture());
        let sizes: Vec<_> = p.state.take_pictures().iter().map(|p| p.width).collect();
        assert_eq!(sizes, vec![4, 8]);
    }
}
