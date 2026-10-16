// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The platform's decoders as `vento/video.h`'s table, for a C backend whose
//! video is decoded by vento's code rather than by a decoder of its library's
//! own: TigerVNC's H.264, whose rectangles reach the shim's decoder context,
//! which calls through this.
//!
//! What that buys is that a C backend's video is decoded exactly as RustDesk's
//! is — the same [`Decoders`], with the person's hardware choice, its fallback
//! to software, its status line and its answer to "was that hardware" — on
//! both platforms, the phone's MediaCodec as much as the Linux client's
//! FFmpeg, and nothing of it written twice.
//!
//! The table carries function pointers rather than naming symbols, so that a
//! backend library links nothing of the process it is loaded into: the Linux
//! client's `libvento_tigervnc.so` has no Rust in it, and its probe has no
//! table at all.

use std::ffi::{c_char, c_int, c_void};
use std::sync::Arc;

use common::video::{Changes, Codec, Decoder, Decoders, Hardware};

use super::ffi;

/// One decoder as the table hands it out: the platform's, and the rows it
/// changed, kept between frames so that a frame allocates nothing.
struct Open {
    decoder: Box<dyn Decoder>,
    changes: Changes,
}

/// `decoders` as the table a backend's config carries. The table's `user` is
/// a box holding its own count of `decoders`, which is the backend's from the
/// moment the table is handed over and comes back through `release`.
pub fn table(decoders: Arc<dyn Decoders>) -> ffi::vento_video_decoders {
    ffi::vento_video_decoders {
        user: Box::into_raw(Box::new(decoders)) as *mut c_void,
        open: Some(open),
        mode: Some(mode),
        decode: Some(decode),
        hardware: Some(hardware),
        status: Some(status),
        take_key_frame_request: Some(take_key_frame_request),
        close: Some(close),
        release: Some(release),
    }
}

/// The decoders behind a table's `user`, borrowed.
///
/// # Safety
///
/// `user` is a table's, from [`table`], not yet released.
unsafe fn decoders<'a>(user: *mut c_void) -> &'a Arc<dyn Decoders> {
    unsafe { &*(user as *const Arc<dyn Decoders>) }
}

fn codec(number: c_int) -> Option<Codec> {
    Some(match number as u32 {
        ffi::VENTO_VIDEO_H264 => Codec::H264,
        ffi::VENTO_VIDEO_H265 => Codec::H265,
        ffi::VENTO_VIDEO_VP8 => Codec::Vp8,
        ffi::VENTO_VIDEO_VP9 => Codec::Vp9,
        ffi::VENTO_VIDEO_AV1 => Codec::Av1,
        _ => return None,
    })
}

/// The mode as the header numbers it: [`Hardware`] in declaration order —
/// also how a backend whose library decodes for itself is told it
/// (`vento_freerdp_config.hardware`).
pub fn mode_number(hardware: Hardware) -> c_int {
    Hardware::ALL.iter().position(|h| *h == hardware).unwrap_or(0) as c_int
}

unsafe extern "C" fn open(
    user: *mut c_void,
    codec_number: c_int,
    width: c_int,
    height: c_int,
    hardware: bool,
) -> *mut ffi::vento_video_decoder {
    let decoders = unsafe { decoders(user) };
    let (Some(codec), Ok(width), Ok(height)) =
        (codec(codec_number), usize::try_from(width), usize::try_from(height))
    else {
        return std::ptr::null_mut();
    };
    let decoder = if hardware {
        decoders.decoder(codec, width, height)
    } else {
        decoders.decoder_as(codec, width, height, Hardware::Off)
    };
    match decoder {
        Some(decoder) => Box::into_raw(Box::new(Open {
            decoder,
            changes: Changes::new(),
        })) as *mut ffi::vento_video_decoder,
        None => std::ptr::null_mut(),
    }
}

unsafe extern "C" fn mode(user: *mut c_void) -> c_int {
    mode_number(unsafe { decoders(user) }.hardware())
}

/// The decoder behind a handle, which is this thread's for the call.
///
/// # Safety
///
/// `d` came from [`open`] and is not closed, and nothing else is inside a
/// call on it. The header says a decoder is never called into from two
/// threads at once, and that is the whole of what this relies on: TigerVNC
/// decodes each rectangle on whichever of its worker threads is free, so a
/// decoder made on one thread is fed and dropped on others, which is why the
/// trait is not `Send` and why this is an `unsafe` promise rather than a
/// bound. It holds for both platforms' decoders. MediaCodec's handle is the
/// NDK's, whose calls may come from any thread one at a time — the C++
/// context this replaced made the same calls from the same worker threads —
/// and libavcodec's context, with its VAAPI or Vulkan device, is likewise
/// any thread's as long as it is one thread's at a time. Neither keeps
/// anything thread-local between calls.
unsafe fn decoder<'a>(d: *mut ffi::vento_video_decoder) -> &'a mut Open {
    unsafe { &mut *(d as *mut Open) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn decode(
    d: *mut ffi::vento_video_decoder,
    data: *const u8,
    len: usize,
    out: *mut u32,
    width: c_int,
    height: c_int,
    whole: bool,
    changed: *mut i32,
) -> bool {
    let open = unsafe { decoder(d) };
    let (Ok(width), Ok(height)) = (usize::try_from(width), usize::try_from(height)) else {
        return false;
    };
    let mut scratch = [0i32; 4];
    let rect = if changed.is_null() {
        &mut scratch
    } else {
        unsafe { &mut *(changed as *mut [i32; 4]) }
    };
    *rect = [0; 4];
    if data.is_null() || out.is_null() || len == 0 || width == 0 || height == 0 {
        return false;
    }
    let Some(words) = width.checked_mul(height) else {
        return false;
    };
    let data = unsafe { std::slice::from_raw_parts(data, len) };
    let out = unsafe { std::slice::from_raw_parts_mut(out, words) };
    if whole {
        open.changes.all(width, height);
    } else {
        open.changes.reset(height);
    }
    if !open.decoder.decode(data, out, width, height, &mut open.changes) {
        return false;
    }
    if let Some((x, y, w, h)) = open.changes.bounds(width) {
        // Inside a picture whose sides are ints, so each fits one.
        *rect = [x as i32, y as i32, w as i32, h as i32];
    }
    true
}

unsafe extern "C" fn hardware(d: *mut ffi::vento_video_decoder) -> c_int {
    match unsafe { decoder(d) }.decoder.hardware() {
        Some(true) => 1,
        Some(false) => 0,
        None => -1,
    }
}

unsafe extern "C" fn status(d: *mut ffi::vento_video_decoder, out: *mut c_char, len: usize) -> bool {
    match unsafe { decoder(d) }.decoder.status() {
        Some(line) => unsafe { super::write_string(&line, out, len) },
        None => false,
    }
}

unsafe extern "C" fn take_key_frame_request(d: *mut ffi::vento_video_decoder) -> bool {
    unsafe { decoder(d) }.decoder.take_key_frame_request()
}

unsafe extern "C" fn close(d: *mut ffi::vento_video_decoder) {
    if !d.is_null() {
        drop(unsafe { Box::from_raw(d as *mut Open) });
    }
}

unsafe extern "C" fn release(user: *mut c_void) {
    if !user.is_null() {
        drop(unsafe { Box::from_raw(user as *mut Arc<dyn Decoders>) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A decoder that paints row 1 of its picture in a colour of the frame's
    /// first byte, and says what the decoders told it about hardware.
    struct Painter {
        hardware: bool,
    }

    impl Decoder for Painter {
        fn decode(
            &mut self,
            data: &[u8],
            out: &mut [u32],
            width: usize,
            _height: usize,
            changed: &mut Changes,
        ) -> bool {
            out[width..2 * width].fill(data[0] as u32);
            changed.mark(1, 0, width);
            true
        }

        fn hardware(&self) -> Option<bool> {
            Some(self.hardware)
        }

        fn status(&self) -> Option<String> {
            Some(format!("H.264 {}", if self.hardware { "hw" } else { "sw" }))
        }
    }

    struct Fake {
        hardware: AtomicBool,
        dropped: Arc<AtomicBool>,
    }

    impl Decoders for Fake {
        fn decodable(&self, codec: Codec) -> bool {
            codec == Codec::H264
        }

        fn decoder(&self, codec: Codec, width: usize, height: usize) -> Option<Box<dyn Decoder>> {
            let hardware = if self.hardware.load(Ordering::Relaxed) {
                Hardware::Vaapi
            } else {
                Hardware::Off
            };
            self.decoder_as(codec, width, height, hardware)
        }

        fn decoder_as(
            &self,
            codec: Codec,
            _width: usize,
            _height: usize,
            hardware: Hardware,
        ) -> Option<Box<dyn Decoder>> {
            (codec == Codec::H264).then(|| {
                Box::new(Painter {
                    hardware: hardware != Hardware::Off,
                }) as Box<dyn Decoder>
            })
        }

        fn hardware(&self) -> Hardware {
            if self.hardware.load(Ordering::Relaxed) {
                Hardware::Vaapi
            } else {
                Hardware::Off
            }
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_frame_goes_through_the_table_and_says_what_it_changed() {
        let dropped = Arc::new(AtomicBool::new(false));
        let fake = Arc::new(Fake {
            hardware: AtomicBool::new(true),
            dropped: Arc::clone(&dropped),
        });
        let t = table(Arc::clone(&fake) as Arc<dyn Decoders>);
        unsafe {
            assert_eq!(t.mode.unwrap()(t.user), mode_number(Hardware::Vaapi));
            assert!(t.open.unwrap()(t.user, ffi::VENTO_VIDEO_VP8 as c_int, 4, 3, true).is_null());
            let d = t.open.unwrap()(t.user, ffi::VENTO_VIDEO_H264 as c_int, 4, 3, true);
            assert!(!d.is_null());

            let mut out = [7u32; 12];
            let mut changed = [-1i32; 4];
            assert!(t.decode.unwrap()(d, [9u8].as_ptr(), 1, out.as_mut_ptr(), 4, 3, false, changed.as_mut_ptr()));
            assert_eq!(out, [7, 7, 7, 7, 9, 9, 9, 9, 7, 7, 7, 7]);
            // Bounds are to sixteen columns, cut to the picture.
            assert_eq!(changed, [0, 1, 4, 1]);
            // Whole: every row counts as changed, whatever the decoder found.
            assert!(t.decode.unwrap()(d, [9u8].as_ptr(), 1, out.as_mut_ptr(), 4, 3, true, changed.as_mut_ptr()));
            assert_eq!(changed, [0, 0, 4, 3]);
            // Nothing to decode is no picture, and an empty rectangle.
            assert!(!t.decode.unwrap()(d, std::ptr::null(), 0, out.as_mut_ptr(), 4, 3, false, changed.as_mut_ptr()));
            assert_eq!(changed, [0; 4]);

            assert_eq!(t.hardware.unwrap()(d), 1);
            let mut line = [0 as c_char; 8];
            assert!(t.status.unwrap()(d, line.as_mut_ptr(), line.len()));
            let line = std::ffi::CStr::from_ptr(line.as_ptr()).to_str().unwrap();
            assert_eq!(line, "H.264 h");
            assert!(!t.take_key_frame_request.unwrap()(d));
            t.close.unwrap()(d);

            // Asked for software, it is software whatever the mode.
            let d = t.open.unwrap()(t.user, ffi::VENTO_VIDEO_H264 as c_int, 4, 3, false);
            assert_eq!(t.hardware.unwrap()(d), 0);
            t.close.unwrap()(d);

            fake.hardware.store(false, Ordering::Relaxed);
            assert_eq!(t.mode.unwrap()(t.user), 0);

            drop(fake);
            assert!(!dropped.load(Ordering::Relaxed));
            t.release.unwrap()(t.user);
        }
        assert!(dropped.load(Ordering::Relaxed));
    }
}
