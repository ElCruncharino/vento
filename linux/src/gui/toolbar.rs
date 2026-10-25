// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The toolbar's geometry: where along the window's top edge it sits, where
//! a drag puts it, and which strip of that edge brings it out. The session
//! draws it and asks these; nothing here touches egui's state, so it is
//! tested here. The unit is the egui point, as in `view.rs`.

use egui::{Pos2, Rect};

/// How far down from the top of a window the pointer brings the toolbar
/// out. A window's top row is a hair under its title bar, and a pointer
/// coming down off the title bar is as likely to land a few points lower;
/// fullscreen has no such problem, since the pointer stops at the screen's
/// edge by itself, and there it is the top row alone (see [`reveals`]).
pub const WINDOWED_REACH: f32 = 8.0;

/// How much of a hidden toolbar is left showing at the top edge while it
/// peeks: the lower edge of the toolbar's background, under its buttons'
/// margin (six points) so that no button shows, as a mark of where along
/// the edge to put the pointer now that only the toolbar's own span brings
/// it out. A window always leaves it showing; fullscreen only while the
/// pointer is at the top edge (see [`peeks`]), as the screen is otherwise
/// the desktop's there.
pub const PEEK: f32 = 4.0;

/// Whether a hidden toolbar peeks, [`PEEK`] of it showing: always in a
/// window, and in fullscreen while the pointer is on the top row of device
/// pixels anywhere across the screen. The top row because it is the same
/// row that brings the toolbar out within its span (see [`reveals`]), so
/// that the pointer stopped by the screen's edge and run along it finds the
/// toolbar where the mark is, and any lower the desktop has the screen to
/// itself again, a menu bar at its top included.
pub fn peeks(window: Rect, fullscreen: bool, pixels_per_point: f32, pointer: Option<Pos2>) -> bool {
    !fullscreen || pointer.is_some_and(|p| within_reach(window, 1.0 / pixels_per_point, p))
}

/// Where the top of a toolbar `height` tall is, relative to the window's
/// top edge, while it is hidden: [`PEEK`] short of wholly off the top while
/// it `peeks`, and wholly off it otherwise. Shown, it is at zero.
pub fn hidden_y(height: f32, peeks: bool) -> f32 {
    if peeks { -height + PEEK.min(height) } else { -height }
}

/// Where the toolbar sits along the top edge: how far along the room the
/// window leaves it its left edge is, 0 flush left, 1 flush right.
///
/// A fraction of the room rather than a position, so that a resize keeps
/// it where it was put relative to the edges — centred stays centred, and
/// one dragged against the right edge stays against it — and so that it is
/// inside the window whatever the window's size. The window keeps one for
/// the life of the process and hands it to each session, so that the next
/// session's toolbar is where the last one was left; it is not saved.
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

    /// Puts the toolbar's left edge at `left`, or as near to it as keeps
    /// the toolbar inside `window`. In a window narrower than the toolbar
    /// there is nowhere to move it to, and it stays where it was put.
    pub fn move_to(&mut self, window: Rect, width: f32, left: f32) {
        let room = window.width() - width;
        if room > 0.0 {
            self.along = ((left - window.min.x) / room).clamp(0.0, 1.0);
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
    (left..left + width).contains(&pointer.x) && within_reach(window, reach, pointer)
}

/// Whether `pointer` is less than `reach` below the top of `window`, and
/// not above it. Half-open, so that a reach of one pixel is one row and
/// not two.
fn within_reach(window: Rect, reach: f32, pointer: Pos2) -> bool {
    (window.min.y..window.min.y + reach).contains(&pointer.y)
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
    fn a_drag_is_kept_inside_the_window() {
        let w = window(1280.0, 800.0);
        let mut p = Placement::default();
        p.move_to(w, 480.0, 100.0);
        assert_eq!(p.left(w, 480.0), 100.0);
        // Past either edge it stops at the edge.
        p.move_to(w, 480.0, -300.0);
        assert_eq!(p.left(w, 480.0), 0.0);
        p.move_to(w, 480.0, 1000.0);
        assert_eq!(p.left(w, 480.0), 800.0);
        // A window that is not at the origin is measured from its own edge.
        let off = Rect::from_min_max(pos2(50.0, 0.0), pos2(1330.0, 800.0));
        p.move_to(off, 480.0, 0.0);
        assert_eq!(p.left(off, 480.0), 50.0);
    }

    #[test]
    fn a_resize_keeps_it_where_it_was_relative_to_the_edges() {
        let mut p = Placement::default();
        // Flush right stays flush right.
        p.move_to(window(1280.0, 800.0), 480.0, 800.0);
        assert_eq!(p.left(window(1920.0, 1080.0), 480.0), 1440.0);
        assert_eq!(p.left(window(800.0, 600.0), 480.0), 320.0);
        // A quarter of the way along stays a quarter of the way along.
        p.move_to(window(1280.0, 800.0), 480.0, 200.0);
        assert_eq!(p.left(window(2080.0, 800.0), 480.0), 400.0);
        // Too narrow a window centres it, and a drag there changes nothing
        // for when the window is wide again.
        let narrow = window(400.0, 800.0);
        assert_eq!(p.left(narrow, 480.0), -40.0);
        p.move_to(narrow, 480.0, 0.0);
        assert_eq!(p.left(window(2080.0, 800.0), 480.0), 400.0);
    }

    #[test]
    fn the_strip_follows_the_toolbar_where_it_was_dragged() {
        let w = window(1280.0, 800.0);
        let mut p = Placement::default();
        p.move_to(w, 480.0, 0.0);
        let left = p.left(w, 480.0);
        let at = |x| reveals(w, left, 480.0, false, 1.0, pos2(x, 0.0));
        assert!(at(10.0));
        assert!(at(479.0));
        // Where it was centred is now the desktop's.
        assert!(!at(640.0));
        assert!(!at(800.0));
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
    fn hidden_a_peek_leaves_its_lower_edge_and_otherwise_nothing() {
        // 28-point buttons in a 6-point margin either side.
        let height = 40.0;
        assert_eq!(hidden_y(height, false), -40.0);
        assert_eq!(hidden_y(height, true), -36.0);
        // What shows is inside the strip that brings it out in a window,
        // so the pointer on it brings it out.
        let showing = hidden_y(height, true) + height;
        assert_eq!(showing, PEEK);
        assert!(showing <= WINDOWED_REACH);
        // Before it has been measured it is nothing tall, and nothing of
        // it shows.
        assert_eq!(hidden_y(0.0, true), 0.0);
    }

    #[test]
    fn a_window_always_peeks_and_fullscreen_at_the_top_row_anywhere() {
        let w = window(1280.0, 800.0);
        // A window peeks wherever the pointer is, or with none.
        assert!(peeks(w, false, 1.0, None));
        assert!(peeks(w, false, 1.0, Some(pos2(640.0, 400.0))));
        // Fullscreen peeks with the pointer on the top row, across the
        // whole width, far outside the toolbar's span as well as in it.
        let at = |x, y, ppp| peeks(w, true, ppp, Some(pos2(x, y)));
        assert!(at(0.0, 0.0, 1.0));
        assert!(at(20.0, 0.0, 1.0));
        assert!(at(640.0, 0.0, 1.0));
        assert!(at(1279.5, 0.0, 1.0));
        assert!(at(640.0, 0.9, 1.0));
        // One row down, and anywhere lower, it does not.
        assert!(!at(20.0, 1.0, 1.0));
        assert!(!at(640.0, 4.0, 1.0));
        assert!(!at(640.0, 400.0, 1.0));
        assert!(!peeks(w, true, 1.0, None));
        // At 2 pixels per point the top row is half a point.
        assert!(at(20.0, 0.0, 2.0));
        assert!(!at(20.0, 0.5, 2.0));
        // Outside the span the top row peeks but does not bring it out;
        // inside the span the same row does both.
        let (left, width) = (400.0, 480.0);
        assert!(at(20.0, 0.0, 1.0) && !reveals(w, left, width, true, 1.0, pos2(20.0, 0.0)));
        assert!(at(640.0, 0.0, 1.0) && reveals(w, left, width, true, 1.0, pos2(640.0, 0.0)));
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
