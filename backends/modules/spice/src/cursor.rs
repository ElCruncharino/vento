// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The cursor channel: the shape the guest is drawing, on a connection of its
//! own.
//!
//! It is a channel rather than a message because SPICE keeps the pointer's
//! picture apart from the screen's: the cursor moves without the desktop
//! changing, and a client that owns the pointer draws it locally rather than
//! waiting for a frame. This end needs only the shape and the hotspot from it.
//! The cursor's position is set by the guest in server mouse mode and by this
//! end in client mode, and is not carried on this channel.

use crate::error::Result;
use runtime::Handler;
use shakenfist_spice_protocol::constants::cursor_server;
use shakenfist_spice_protocol::messages::{CursorInit, CursorSet, SpiceCursorHeader};
use std::collections::HashMap;

/// The shapes the server has asked this end to keep, by its own id: SPICE
/// sends a cursor once and then names it, so a client that does not keep them
/// gets a cursor that goes blank the second time a window is resized.
///
/// Bounded by count rather than by bytes, unlike the image cache: a cursor is
/// at most [`MAX_SIDE`] square and a guest has a few dozen, so the bound is
/// there to stop a hostile server rather than to fit a real one.
const MAX_SHAPES: usize = 64;

/// The widest and tallest cursor taken, which is the largest a Windows guest
/// draws. The header's two 16-bit fields would allow four billion pixels, and
/// a shape past this is refused before anything is allocated for it.
const MAX_SIDE: usize = 256;

pub struct Cursors {
    shapes: HashMap<u64, Shape>,
    order: Vec<u64>,
}

struct Shape {
    width: usize,
    height: usize,
    hot_x: i32,
    hot_y: i32,
    /// The framebuffer's own word order, which `Handler::cursor` takes; each
    /// platform converts on its side of that call.
    pixels: Vec<u32>,
}

impl Cursors {
    pub fn new() -> Cursors {
        Cursors {
            shapes: HashMap::new(),
            order: Vec::new(),
        }
    }

    pub fn message(
        &mut self,
        message_type: u16,
        body: &[u8],
        handler: &mut dyn Handler,
    ) -> Result<()> {
        match message_type {
            // Both carry a SpiceCursor; INIT has nine bytes of its own in front
            // of it and SET has five. A body too short for those is dropped,
            // as one too short for the cursor's own header is: the pointer
            // keeps the shape it had, which is what a lost shape costs.
            cursor_server::INIT | cursor_server::SET => {
                let fields = if message_type == cursor_server::INIT {
                    CursorInit::SIZE
                } else {
                    CursorSet::SIZE
                };
                match body.get(fields..) {
                    Some(cursor) => self.cursor(cursor, handler),
                    None => log::warn!("cursor: a {} byte message {message_type}", body.len()),
                }
            }
            cursor_server::HIDE => handler.cursor(&[], 0, 0, 0, 0),
            cursor_server::INVALIDATE_ALL => {
                self.shapes.clear();
                self.order.clear();
            }
            _ => {}
        }
        Ok(())
    }

    fn cursor(&mut self, data: &[u8], handler: &mut dyn Handler) {
        let Ok(Some(header)) = SpiceCursorHeader::read(data) else {
            return; // FLAG_NONE, or a body too short to hold one
        };
        if header.flags & SpiceCursorHeader::FLAG_FROM_CACHE != 0 {
            if let Some(shape) = self.shapes.get(&header.unique_id) {
                handler.cursor(&shape.pixels, shape.width, shape.height, shape.hot_x, shape.hot_y);
            } else {
                log::warn!("cursor: {} is not one this end kept", header.unique_id);
            }
            return;
        }

        let Some(pixels) = pixels(&header, &data[SpiceCursorHeader::SIZE..]) else {
            return;
        };
        let shape = Shape {
            width: header.width as usize,
            height: header.height as usize,
            hot_x: header.hot_spot_x as i32,
            hot_y: header.hot_spot_y as i32,
            pixels,
        };
        handler.cursor(&shape.pixels, shape.width, shape.height, shape.hot_x, shape.hot_y);
        if header.flags & SpiceCursorHeader::FLAG_CACHE_ME != 0 {
            if self.order.len() >= MAX_SHAPES && let Some(oldest) = self.order.first().copied() {
                self.order.remove(0);
                self.shapes.remove(&oldest);
            }
            self.order.push(header.unique_id);
            self.shapes.insert(header.unique_id, shape);
        }
    }
}

impl Default for Cursors {
    fn default() -> Cursors {
        Cursors::new()
    }
}

/// A cursor's pixels as `R, G, B, A` words, which is the framebuffer's order
/// and what `Handler::cursor` expects.
///
/// Three of SPICE's cursor types are 32 or 24 bits, and are the ones a guest
/// with a driver sends. The monochrome ones come from a text-mode guest and
/// are two bitplanes rather than pixels; they are not decoded, since no guest
/// tested has sent one.
fn pixels(header: &SpiceCursorHeader, data: &[u8]) -> Option<Vec<u32>> {
    let (width, height) = (header.width as usize, header.height as usize);
    if width > MAX_SIDE || height > MAX_SIDE {
        log::warn!("cursor: {width}x{height} is bigger than any cursor");
        return None;
    }
    let count = width.checked_mul(height)?;
    if count == 0 {
        return None;
    }
    // The bytes are taken before the pixels are made, so what is allocated is
    // what arrived rather than what the header said would.
    let mut out = Vec::new();
    match header.cursor_type {
        // ALPHA: BGRA, and the alpha is the cursor's own shape.
        0 => {
            let bytes = data.get(..count.checked_mul(4)?)?;
            out.reserve_exact(count);
            for px in bytes.chunks_exact(4) {
                out.push(u32::from_le_bytes([px[2], px[1], px[0], px[3]]));
            }
        }
        // COLOR24: BGR, every pixel opaque.
        5 => {
            let bytes = data.get(..count.checked_mul(3)?)?;
            out.reserve_exact(count);
            for px in bytes.chunks_exact(3) {
                out.push(u32::from_le_bytes([px[2], px[1], px[0], 0xff]));
            }
        }
        // COLOR32: BGRX, where the fourth byte is padding rather than alpha.
        6 => {
            let bytes = data.get(..count.checked_mul(4)?)?;
            out.reserve_exact(count);
            for px in bytes.chunks_exact(4) {
                out.push(u32::from_le_bytes([px[2], px[1], px[0], 0xff]));
            }
        }
        other => {
            log::warn!("cursor: type {other} is not built here");
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(cursor_type: u8, width: u16, height: u16) -> SpiceCursorHeader {
        SpiceCursorHeader {
            flags: 0,
            unique_id: 1,
            cursor_type,
            width,
            height,
            hot_spot_x: 0,
            hot_spot_y: 0,
        }
    }

    /// A wrong colour order fails silently: a red cursor comes out blue and
    /// nothing else changes.
    #[test]
    fn an_alpha_cursor_comes_out_in_the_framebuffers_order() {
        // One pixel, BGRA on the wire: blue 0x11, green 0x22, red 0x33.
        let pixels = pixels(&header(0, 1, 1), &[0x11, 0x22, 0x33, 0x80]).unwrap();
        assert_eq!(pixels[0].to_le_bytes(), [0x33, 0x22, 0x11, 0x80]);
    }

    #[test]
    fn a_24_bit_cursor_is_opaque() {
        let pixels = pixels(&header(5, 1, 1), &[0x11, 0x22, 0x33]).unwrap();
        assert_eq!(pixels[0].to_le_bytes(), [0x33, 0x22, 0x11, 0xff]);
    }

    /// A truncated cursor is refused rather than read past: the header's size
    /// is what the server claims, and the body is what actually arrived.
    #[test]
    fn a_cursor_shorter_than_its_header_says_is_refused() {
        assert!(pixels(&header(0, 4, 4), &[0; 8]).is_none());
    }

    /// INIT and SET carry fields of their own in front of the cursor, and a
    /// body too short to hold them is the server's mistake: the message is
    /// dropped, as one too short for the cursor's header is, rather than read
    /// past.
    #[test]
    fn a_cursor_message_shorter_than_its_own_fields_is_dropped() {
        let mut cursors = Cursors::new();
        for (kind, size) in [
            (cursor_server::INIT, CursorInit::SIZE),
            (cursor_server::SET, CursorSet::SIZE),
        ] {
            for len in 0..size {
                let body = vec![0u8; len];
                assert!(cursors.message(kind, &body, &mut crate::testing::Nothing).is_ok());
            }
        }
    }

    /// The header's size is two 16-bit fields, which is four billion pixels:
    /// a cursor bigger than any cursor is refused before anything is
    /// allocated for it, whatever arrived behind it.
    #[test]
    fn a_cursor_bigger_than_any_cursor_is_refused() {
        assert!(pixels(&header(0, 65535, 65535), &[0; 64]).is_none());
        assert!(pixels(&header(0, 257, 1), &[0; 257 * 4]).is_none());
        assert!(pixels(&header(0, 256, 256), &[0; 256 * 256 * 4]).is_some());
    }
}
