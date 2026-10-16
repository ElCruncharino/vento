// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The far end's cursor as the window's own pointer: winit's custom cursor
//! set from the shape the protocol sent, with its hotspot, in place of the
//! shape drawn under the local pointer.
//!
//! A shape is uploaded once. winit makes a `CustomCursor` through the event
//! loop — on Wayland a set of pixels it copies into a shared-memory buffer
//! when the cursor is *set* (kept while that cursor stays the window's, by
//! `patches/winit/0004`, but made again after any other cursor, egui's
//! icons included), on X11 an Xcursor made once and set with one request —
//! so the window's cursor is set only when what it should be changes — a
//! new shape, a new scale, the pointer arriving on or leaving the desktop —
//! and never once a frame. The cache is keyed by the shape's
//! pixels and hotspot, so a cursor that flickers between two shapes (an
//! xterm's I-beam and the arrow over its scrollbar) is two uploads and not
//! one a change.
//!
//! The shape is in desktop pixels and the window's cursor is in device
//! pixels, so where the desktop is not drawn one to one the shape is
//! resampled to the view's scale, nearest neighbour, and the hotspot with
//! it: a cursor drawn at the wrong size is a cursor whose hotspot is in the
//! wrong place. winit hands the compositor the buffer at the window's scale
//! factor, so a device pixel here is a device pixel there; an X cursor has
//! no scale and is in device pixels already.
//!
//! The alpha differs by display server ([`Alpha`]): winit's Wayland side
//! takes straight alpha, which is what the shapes arrive as, and
//! premultiplies it itself; its X11 side copies the pixels as given into a
//! Render cursor, which wants them premultiplied, so they are premultiplied
//! here.
//!
//! egui-winit sets the window's cursor too, from egui's cursor icon, and
//! only when that icon changes; this runs after it each frame and re-applies
//! the shape whenever the icon changed, so the two never fight over a frame.
//! When the shape is no longer wanted the default is put back *before*
//! egui-winit's turn, so that whatever icon egui then asks for lands on top.

use super::platform::Platform;
use crate::events::Cursor;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, CustomCursor, Window};

/// How many shapes are kept before the cache is started again.
const CACHE: usize = 32;

/// One shape at one scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    /// A hash of the pixels and the hotspot.
    pub shape: u64,
    /// Device pixels per desktop pixel, in thousandths.
    pub scale: u32,
}

impl Key {
    /// `shape` is [`shape_hash`]'s, kept with the shape rather than hashed
    /// a frame at a time.
    pub fn new(shape: u64, factor: f32) -> Key {
        Key {
            shape,
            scale: (factor.max(0.0) * 1000.0).round() as u32,
        }
    }

    fn factor(self) -> f32 {
        self.scale as f32 / 1000.0
    }
}

/// The pixels and the hotspot, as one number.
pub fn shape_hash(cursor: &Cursor) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cursor.width.hash(&mut h);
    cursor.height.hash(&mut h);
    cursor.hot_x.hash(&mut h);
    cursor.hot_y.hash(&mut h);
    cursor.rgba.hash(&mut h);
    h.finish()
}

/// What winit is to be handed, from the straight alpha the shapes arrive
/// with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpha {
    /// Wayland: winit premultiplies what it is given, so it is given
    /// straight alpha.
    Straight,
    /// X11: winit copies the pixels into an Xcursor as they are, and a
    /// Render cursor's colour must not exceed its alpha.
    Premultiplied,
}

impl Alpha {
    pub fn for_platform(platform: Platform) -> Alpha {
        if platform == Platform::X11 {
            Alpha::Premultiplied
        } else {
            Alpha::Straight
        }
    }
}

/// The shapes made so far, by key: the event loop's, so one shared by every
/// window, which each set it on themselves (a `CustomCursor` is not a
/// window's).
#[derive(Default)]
pub struct Cache(HashMap<Key, CustomCursor>);

/// One window's cursor, as far as the far end's shape goes.
#[derive(Default)]
pub struct Native {
    /// What the window's cursor was last set to from here, if a shape.
    shown: Option<Key>,
    /// egui's icon last frame, to see it change.
    icon: Option<egui::CursorIcon>,
}

impl Native {
    /// Before egui-winit's turn: if the shape is no longer wanted, the
    /// default goes back now, under whatever egui asks for next.
    pub fn before(&mut self, window: &Window, wanted: Option<Key>) {
        if wanted.is_none() && self.shown.take().is_some() {
            window.set_cursor(CursorIcon::Default);
            log::debug!("native cursor: default");
        }
    }

    /// After egui-winit's turn: the shape, if wanted and not already there.
    pub fn after(
        &mut self,
        cache: &mut Cache,
        window: &Window,
        event_loop: &ActiveEventLoop,
        wanted: Option<(Key, &Cursor)>,
        icon: egui::CursorIcon,
        alpha: Alpha,
    ) {
        let icon_changed = self.icon != Some(icon);
        self.icon = Some(icon);
        let Some((key, cursor)) = wanted else {
            return;
        };
        if self.shown == Some(key) && !icon_changed {
            return;
        }
        let custom = match cache.0.get(&key) {
            Some(custom) => custom.clone(),
            None => {
                let Some(source) = scaled(cursor, key.factor(), alpha) else {
                    return;
                };
                if cache.0.len() >= CACHE {
                    cache.0.clear();
                }
                let custom = event_loop.create_custom_cursor(source);
                cache.0.insert(key, custom.clone());
                custom
            }
        };
        window.set_cursor(custom);
        if self.shown != Some(key) {
            let (w, h, x, y) = scaled_geometry(cursor, key.factor());
            log::info!(
                "native cursor: {w}×{h}, hotspot {x},{y} (×{:.3} from {}×{} at {},{})",
                key.factor(),
                cursor.width,
                cursor.height,
                cursor.hot_x,
                cursor.hot_y
            );
        }
        self.shown = Some(key);
    }

    /// The window is no longer the session's: the default, if a shape was
    /// set.
    pub fn reset(&mut self, window: &Window) {
        if self.shown.take().is_some() {
            window.set_cursor(CursorIcon::Default);
        }
    }

    pub fn shown(&self) -> Option<Key> {
        self.shown
    }
}

/// The shape's size and hotspot at `factor` device pixels per desktop pixel.
fn scaled_geometry(cursor: &Cursor, factor: f32) -> (u16, u16, u16, u16) {
    let w = ((cursor.width as f32 * factor).ceil() as u16).max(1);
    let h = ((cursor.height as f32 * factor).ceil() as u16).max(1);
    let x = ((cursor.hot_x.max(0) as f32 * factor).round() as u16).min(w - 1);
    let y = ((cursor.hot_y.max(0) as f32 * factor).round() as u16).min(h - 1);
    (w, h, x, y)
}

/// The shape's pixels resampled to `factor`, nearest neighbour, with its
/// size and hotspot: straight alpha as they arrived, or premultiplied for
/// [`Alpha::Premultiplied`].
fn resample(cursor: &Cursor, factor: f32, alpha: Alpha) -> Option<(Vec<u8>, u16, u16, u16, u16)> {
    if cursor.width == 0 || cursor.height == 0 || factor <= 0.0 {
        return None;
    }
    let (w, h, x, y) = scaled_geometry(cursor, factor);
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for oy in 0..h as usize {
        let sy = ((oy as f32 / factor) as usize).min(cursor.height - 1);
        for ox in 0..w as usize {
            let sx = ((ox as f32 / factor) as usize).min(cursor.width - 1);
            let p = &cursor.rgba[(sy * cursor.width + sx) * 4..][..4];
            let a = p[3] as u32;
            if alpha == Alpha::Straight || a == 255 {
                rgba.extend_from_slice(p);
            } else {
                let times = |c: u8| ((c as u32 * a + 127) / 255) as u8;
                rgba.extend_from_slice(&[times(p[0]), times(p[1]), times(p[2]), p[3]]);
            }
        }
    }
    Some((rgba, w, h, x, y))
}

/// The shape as winit takes it, no bigger than winit's limit, which is far
/// past any cursor a desktop sends.
fn scaled(cursor: &Cursor, factor: f32, alpha: Alpha) -> Option<winit::window::CustomCursorSource> {
    let (rgba, w, h, x, y) = resample(cursor, factor, alpha)?;
    match CustomCursor::from_rgba(rgba, w, h, x, y) {
        Ok(source) => Some(source),
        Err(e) => {
            log::warn!("native cursor: {w}×{h} refused: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> Cursor {
        // 2×2: opaque red, clear, half-covered green, white.
        Cursor {
            width: 2,
            height: 2,
            hot_x: 1,
            hot_y: 0,
            rgba: vec![255, 0, 0, 255, 0, 0, 0, 0, 0, 128, 0, 128, 255, 255, 255, 255],
        }
    }

    #[test]
    fn the_key_is_the_pixels_the_hotspot_and_the_scale() {
        let a = Key::new(shape_hash(&shape()), 1.0);
        assert_eq!(a, Key::new(shape_hash(&shape()), 1.0));
        assert_ne!(a, Key::new(shape_hash(&shape()), 0.5));
        let mut moved = shape();
        moved.hot_x = 0;
        assert_ne!(a, Key::new(shape_hash(&moved), 1.0));
        let mut painted = shape();
        painted.rgba[0] = 0;
        assert_ne!(a, Key::new(shape_hash(&painted), 1.0));
    }

    #[test]
    fn scaling_is_nearest_neighbour_and_moves_the_hotspot() {
        assert_eq!(scaled_geometry(&shape(), 1.0), (2, 2, 1, 0));
        assert_eq!(scaled_geometry(&shape(), 2.0), (4, 4, 2, 0));
        // Rounded up, so that no pixel is lost, and the hotspot kept inside.
        assert_eq!(scaled_geometry(&shape(), 0.6), (2, 2, 1, 0));
        let big = Cursor {
            width: 32,
            height: 32,
            hot_x: 31,
            hot_y: 15,
            rgba: vec![0; 32 * 32 * 4],
        };
        assert_eq!(scaled_geometry(&big, 0.5), (16, 16, 15, 8));
        assert!(scaled(&shape(), 2.0, Alpha::Straight).is_some());
        assert!(scaled(&shape(), 0.0, Alpha::Straight).is_none());
    }

    #[test]
    fn x11_gets_the_pixels_premultiplied() {
        let (straight, ..) = resample(&shape(), 1.0, Alpha::Straight).unwrap();
        let (premultiplied, ..) = resample(&shape(), 1.0, Alpha::Premultiplied).unwrap();
        // The half-covered green: as it arrived for winit's Wayland side,
        // which premultiplies it itself, and premultiplied for X11's.
        assert_eq!(straight[8..12], [0, 128, 0, 128]);
        assert_eq!(premultiplied[8..12], [0, 64, 0, 128]);
        // Opaque and clear pixels are the same either way.
        for i in [0..4, 4..8, 12..16] {
            assert_eq!(straight[i.clone()], premultiplied[i]);
        }
        assert_eq!(Alpha::for_platform(Platform::X11), Alpha::Premultiplied);
        assert_eq!(Alpha::for_platform(Platform::Wayland), Alpha::Straight);
    }
}
