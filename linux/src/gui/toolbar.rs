// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The toolbar's geometry: where along the window's top edge it sits, and
//! which strip of that edge brings it out. The session draws it and asks
//! these; nothing here touches egui's state, so it is tested here. The
//! unit is the egui point, as in `view.rs`.

use egui::{Pos2, Rect};

/// How far down from the top of a window the pointer brings the toolbar
/// out. A window's top row is a hair under its title bar, and a pointer
/// coming down off the title bar is as likely to land a few points lower;
/// fullscreen has no such problem, since the pointer stops at the screen's
/// edge by itself, and there it is the top row alone (see [`reveals`]).
pub const WINDOWED_REACH: f32 = 8.0;

/// Where the toolbar sits along the top edge: how far along the room the
/// window leaves it its left edge is, 0 flush left, 1 flush right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    along: f32,
}

impl Default for Placement {
    /// Centred.
    fn default() -> Placement {
        Placement { along: 0.5 }
    }
}

impl Placement {
    /// The left edge of a toolbar `width` wide along the top of `window`.
    /// A window narrower than the toolbar centres it, as egui's anchor did.
    pub fn left(self, window: Rect, width: f32) -> f32 {
        let room = window.width() - width;
        if room <= 0.0 {
            window.center().x - width / 2.0
        } else {
            window.min.x + room * self.along
        }
    }
}

/// Whether a pointer at `pointer` brings the toolbar out: within the
/// toolbar's own span, which is where it would appear, and within reach of
/// the top edge — [`WINDOWED_REACH`] in a window, the top row of device
/// pixels in fullscreen. Half-open, so that a reach of one pixel is one row
/// and not two.
pub fn reveals(window: Rect, left: f32, width: f32, fullscreen: bool, pixels_per_point: f32, pointer: Pos2) -> bool {
    let reach = if fullscreen { 1.0 / pixels_per_point } else { WINDOWED_REACH };
    (left..left + width).contains(&pointer.x) && (window.min.y..window.min.y + reach).contains(&pointer.y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn window(width: f32, height: f32) -> Rect {
        Rect::from_min_max(pos2(0.0, 0.0), pos2(width, height))
    }

    #[test]
    fn centred_by_default_and_in_a_window_too_narrow_for_it() {
        let p = Placement::default();
        assert_eq!(p.left(window(1280.0, 800.0), 480.0), 400.0);
        assert_eq!(p.left(window(400.0, 800.0), 480.0), -40.0);
    }

    #[test]
    fn only_the_toolbars_own_span_reveals_it() {
        let w = window(1280.0, 800.0);
        let (left, width) = (400.0, 480.0);
        let at = |x, y| reveals(w, left, width, false, 1.0, pos2(x, y));
        assert!(at(400.0, 0.0));
        assert!(at(640.0, 0.0));
        assert!(at(879.5, 0.0));
        // Either side of it, at the very top, is the desktop's.
        assert!(!at(399.5, 0.0));
        assert!(!at(880.0, 0.0));
        assert!(!at(20.0, 0.0));
        assert!(!at(1270.0, 0.0));
    }

    #[test]
    fn a_window_reaches_a_few_points_down_and_fullscreen_one_row() {
        let w = window(1280.0, 800.0);
        let (left, width) = (400.0, 480.0);
        let windowed = |y| reveals(w, left, width, false, 1.0, pos2(640.0, y));
        assert!(windowed(0.0));
        assert!(windowed(7.5));
        assert!(!windowed(WINDOWED_REACH));
        assert!(!windowed(30.0));

        let fullscreen = |y, ppp| reveals(w, left, width, true, ppp, pos2(640.0, y));
        assert!(fullscreen(0.0, 1.0));
        assert!(!fullscreen(1.0, 1.0));
        assert!(!fullscreen(4.0, 1.0));
        // At 2 pixels per point the top row is half a point.
        assert!(fullscreen(0.0, 2.0));
        assert!(!fullscreen(0.5, 2.0));
    }
}
