// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where the desktop lands in the window: the scale modes, the pan, and the
//! two conversions that follow from them — a pointer position into desktop
//! coordinates, and a pointer near an edge into a pan velocity.
//!
//! Nothing here touches egui's state or the backend. The window hands over
//! two rectangles in points — the part of the desktop being shown (the whole
//! of it, or one monitor) and the area it may use — and gets back a
//! [`Layout`] it draws from and asks questions of. The unit is the egui point
//! rather than the device pixel, so the arithmetic is the same whatever the
//! scale factor. Pixels matter in two places. `Original` asks for a desktop
//! pixel per *device* pixel, since a 1:1 mode drawn at a fraction of a pixel
//! is neither sharp nor 1:1; and the rectangle the desktop lands in is put
//! on device pixels, here, once, so that what is drawn and what the pointer
//! is mapped through are the same rectangle.

use egui::{Pos2, Rect, Vec2, vec2};

/// How the desktop is scaled to the window. The five cycle in this order
/// from the toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaleMode {
    /// The desktop's width is the window's, never enlarged; the height may
    /// overflow and pan.
    FitWidth,
    /// The same for the height.
    FitHeight,
    /// The width is the window's even if that means enlarging.
    FillWidth,
    FillHeight,
    /// A desktop pixel per device pixel.
    Original,
}

impl ScaleMode {
    pub fn next(self) -> ScaleMode {
        match self {
            ScaleMode::FitWidth => ScaleMode::FitHeight,
            ScaleMode::FitHeight => ScaleMode::FillWidth,
            ScaleMode::FillWidth => ScaleMode::FillHeight,
            ScaleMode::FillHeight => ScaleMode::Original,
            ScaleMode::Original => ScaleMode::FitWidth,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ScaleMode::FitWidth => "Fit width",
            ScaleMode::FitHeight => "Fit height",
            ScaleMode::FillWidth => "Fill width",
            ScaleMode::FillHeight => "Fill height",
            ScaleMode::Original => "1:1",
        }
    }

    /// Points per desktop pixel for a `source` this big in a `window` this
    /// big, with `pixels_per_point` saying what 1:1 is.
    pub fn scale(self, source: Vec2, window: Vec2, pixels_per_point: f32) -> f32 {
        let native = 1.0 / pixels_per_point.max(0.01);
        if source.x <= 0.0 || source.y <= 0.0 || window.x <= 0.0 || window.y <= 0.0 {
            return native;
        }
        match self {
            ScaleMode::FitWidth => (window.x / source.x).min(native),
            ScaleMode::FitHeight => (window.y / source.y).min(native),
            ScaleMode::FillWidth => window.x / source.x,
            ScaleMode::FillHeight => window.y / source.y,
            ScaleMode::Original => native,
        }
    }
}

/// The part of the view that persists between frames: the mode, and how far
/// the desktop has been panned in each axis, in points, from the top left.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub mode: ScaleMode,
    pub pan: Vec2,
}

impl Default for View {
    fn default() -> View {
        View {
            mode: ScaleMode::FitWidth,
            pan: Vec2::ZERO,
        }
    }
}

/// Where the desktop is this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Points per desktop pixel.
    pub scale: f32,
    /// The desktop pixels being shown, in desktop coordinates.
    pub source: Rect,
    /// Where `source` lands, in points; may extend past `window`. Its edges
    /// are on device pixels.
    pub rect: Rect,
    /// The area the desktop may use.
    pub window: Rect,
    /// How much of `rect` is past `window` in each axis, zero where the
    /// desktop fits — which is what says whether a pan means anything.
    pub overflow: Vec2,
    /// The pan, clamped: between zero and `overflow`, and exactly one of
    /// them at a limit.
    pub pan: Vec2,
    /// Device pixels per point, which `rect` was put on.
    pub pixels_per_point: f32,
}

/// How close to an edge, in points, the pointer starts the desktop moving.
pub const EDGE: f32 = 48.0;
/// Points per second at the edge itself.
pub const EDGE_SPEED: f32 = 1500.0;

impl View {
    /// The layout for showing `source` in `window`, clamping the pan to what
    /// the overflow allows: a pan that made sense at one size is clamped at
    /// the next rather than remembered past the edge.
    pub fn layout(&mut self, source: Rect, window: Rect, pixels_per_point: f32) -> Layout {
        let scale = self.mode.scale(source.size(), window.size(), pixels_per_point);
        let scaled = source.size() * scale;
        let overflow = (scaled - window.size()).max(Vec2::ZERO);
        self.pan = self.pan.clamp(Vec2::ZERO, overflow);
        // Centred in an axis where it fits, panned in one where it does not.
        let slack = (window.size() - scaled).max(Vec2::ZERO) / 2.0;
        let origin = window.min + slack - self.pan;
        // On device pixels. Half the slack of an odd window, or a pan that
        // has moved a fraction of a pixel (the edge pan moves by whatever
        // the frame's time makes it), put the desktop between pixels: the
        // GPU, sampling at each pixel's centre, then showed some columns
        // twice and skipped others at 1:1, and the pointer, mapped from the
        // unrounded corner, named the pixel beside the one drawn under it.
        // The pan itself keeps its fraction, so that a slow one still moves.
        // The size is rounded on its own rather than the far edge with the
        // near one, which at a half split differently and made a 1:1
        // desktop a pixel wider than itself.
        let round = |v: Vec2| (v * pixels_per_point).round() / pixels_per_point;
        let rect = Rect::from_min_size(round(origin.to_vec2()).to_pos2(), round(scaled));
        Layout {
            scale,
            source,
            rect,
            window,
            overflow,
            pan: self.pan,
            pixels_per_point,
        }
    }

    /// Switch modes, which is also the one time a pan is let go of.
    pub fn cycle(&mut self) {
        self.mode = self.mode.next();
        self.pan = Vec2::ZERO;
    }
}

impl Layout {
    /// The desktop pixel under a point, clamped into the source so that a
    /// pointer dragged past the edge keeps pressing the edge.
    ///
    /// By the device pixel the point is in, and that pixel's centre, through
    /// `rect` as drawn: the centre is where the GPU samples the picture, so
    /// the pixel named is the one drawn there. From the pixel's corner, as it
    /// was, a downscaled desktop's last column and row could not be reached
    /// at all (at 1920 in 1280 the window's last pixel was desktop 1918),
    /// which is where an auto-hiding taskbar or a hot corner waits. The
    /// first and last device pixels always name the first and last desktop
    /// pixels, since past twice smaller the centre of the first is already
    /// beyond the desktop's first column.
    pub fn desktop_at(&self, pos: Pos2) -> (i32, i32) {
        let ppp = self.pixels_per_point;
        let axis = |pos: f32, min: f32, len: f32, source_min: f32, source_len: f32| -> i32 {
            let pixels = (len * ppp).round().max(1.0);
            // A point from a whole device pixel can come back a hair under
            // it after the division into points and back.
            let pixel = ((pos - min) * ppp + 1e-3).floor();
            let last = (source_len - 1.0).max(0.0);
            let at = if pixel <= 0.0 {
                0.0
            } else if pixel >= pixels - 1.0 {
                last
            } else {
                ((pixel + 0.5) * source_len / pixels).floor().clamp(0.0, last)
            };
            (source_min + at) as i32
        };
        (
            axis(pos.x, self.rect.min.x, self.rect.width(), self.source.min.x, self.source.width()),
            axis(pos.y, self.rect.min.y, self.rect.height(), self.source.min.y, self.source.height()),
        )
    }

    /// The desktop pixel under a point outside the window, during a drag
    /// that began in it: the window's own mapping carried on past its edges
    /// at the same scale, and clamped to the whole `desktop` (its width and
    /// height) rather than to the monitor the window shows. A drag that
    /// leaves a window showing one monitor for a window showing the one
    /// beside it is still the first window's (it holds the pointer until
    /// the buttons are up), and so it is this mapping that carries the far
    /// end's pointer onto the next monitor rather than pressing it against
    /// the edge of the first. Inside the window it is [`desktop_at`]'s.
    ///
    /// [`desktop_at`]: Layout::desktop_at
    pub fn desktop_beyond(&self, pos: Pos2, desktop: (usize, usize)) -> (i32, i32) {
        if self.window.contains(pos) {
            return self.desktop_at(pos);
        }
        // By the device pixel's centre, as `desktop_at` maps, so that the
        // two agree at the edge where one hands over to the other.
        let ppp = self.pixels_per_point;
        let axis = |pos: f32, min: f32, len: f32, source_min: f32, source_len: f32, whole: usize| -> i32 {
            let pixel = ((pos - min) * ppp + 1e-3).floor();
            let at = source_min + (pixel + 0.5) * source_len / (len * ppp).max(f32::EPSILON);
            (at.floor() as i64).clamp(0, (whole as i64 - 1).max(0)) as i32
        };
        (
            axis(pos.x, self.rect.min.x, self.rect.width(), self.source.min.x, self.source.width(), desktop.0),
            axis(pos.y, self.rect.min.y, self.rect.height(), self.source.min.y, self.source.height(), desktop.1),
        )
    }

    /// The pan velocity a pointer at `pointer` asks for, in points per
    /// second: zero away from the edges, and towards an edge faster the
    /// nearer it is, in each direction the pan can still go. A pan already
    /// at its limit asks for nothing, since a velocity is also a request for
    /// the next frame, and a pointer resting by an edge (on its way to the
    /// toolbar, say) would otherwise redraw the window as fast as it can for
    /// a picture that does not move.
    pub fn edge_velocity(&self, pointer: Pos2) -> Vec2 {
        let axis = |p: f32, min: f32, max: f32, pan: f32, overflow: f32| -> f32 {
            if p < min + EDGE && pan > 0.0 {
                -EDGE_SPEED * (1.0 - ((p - min) / EDGE).clamp(0.0, 1.0))
            } else if p > max - EDGE && pan < overflow {
                EDGE_SPEED * (1.0 - ((max - p) / EDGE).clamp(0.0, 1.0))
            } else {
                0.0
            }
        };
        vec2(
            axis(pointer.x, self.window.min.x, self.window.max.x, self.pan.x, self.overflow.x),
            axis(pointer.y, self.window.min.y, self.window.max.y, self.pan.y, self.overflow.y),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(x, y), vec2(w, h))
    }

    #[test]
    fn fit_never_enlarges_and_fill_does() {
        let small = vec2(800.0, 600.0);
        let window = vec2(1600.0, 900.0);
        assert_eq!(ScaleMode::FitWidth.scale(small, window, 1.0), 1.0);
        assert_eq!(ScaleMode::FillWidth.scale(small, window, 1.0), 2.0);
        assert_eq!(ScaleMode::FillHeight.scale(small, window, 1.0), 1.5);
        let big = vec2(1920.0, 1200.0);
        assert_eq!(ScaleMode::FitWidth.scale(big, window, 1.0), 1600.0 / 1920.0);
        assert_eq!(ScaleMode::FitHeight.scale(big, window, 1.0), 0.75);
        // 1:1 is a device pixel, so on a 2x screen it is half a point.
        assert_eq!(ScaleMode::Original.scale(big, window, 2.0), 0.5);
        assert_eq!(ScaleMode::FitWidth.scale(small, window, 2.0), 0.5);
    }

    #[test]
    fn a_desktop_that_fits_is_centred_and_one_that_does_not_pans() {
        let mut view = View {
            mode: ScaleMode::Original,
            ..View::default()
        };
        let layout = view.layout(rect(0.0, 0.0, 800.0, 600.0), rect(0.0, 20.0, 1000.0, 1000.0), 1.0);
        assert_eq!(layout.rect, rect(100.0, 220.0, 800.0, 600.0));
        assert_eq!(layout.overflow, Vec2::ZERO);

        view.pan = vec2(5000.0, 10.0);
        let layout = view.layout(rect(0.0, 0.0, 1920.0, 1200.0), rect(0.0, 0.0, 1000.0, 1000.0), 1.0);
        // Clamped to the overflow, and the rectangle slides the other way.
        assert_eq!(view.pan, vec2(920.0, 10.0));
        assert_eq!(layout.overflow, vec2(920.0, 200.0));
        assert_eq!(layout.rect.min, pos2(-920.0, -10.0));
    }

    #[test]
    fn the_desktop_lands_on_device_pixels_and_the_pointer_agrees() {
        let mut view = View {
            mode: ScaleMode::Original,
            ..View::default()
        };
        // An odd window leaves half a pixel of slack: rounded, not kept.
        let desktop = rect(0.0, 0.0, 1024.0, 768.0);
        let layout = view.layout(desktop, rect(0.0, 0.0, 1201.0, 901.0), 1.0);
        assert_eq!(layout.rect, rect(89.0, 67.0, 1024.0, 768.0));
        assert_eq!(layout.desktop_at(pos2(89.0, 67.0)), (0, 0));
        assert_eq!(layout.desktop_at(pos2(500.0, 400.0)), (411, 333));
        // A pan of a fraction: the pixel under the pointer is the one drawn
        // there, which is where the rounded rectangle puts it.
        view.pan = vec2(14.6, 0.0);
        let layout = view.layout(desktop, rect(0.0, 0.0, 800.0, 600.0), 1.0);
        assert_eq!(layout.rect.min, pos2(-15.0, 0.0));
        assert_eq!(view.pan, vec2(14.6, 0.0));
        assert_eq!(layout.desktop_at(pos2(400.0, 300.0)), (415, 300));
        // At 1.25 the rectangle is on device pixels, not on points: half a
        // pixel of slack goes one way or the other, but to a whole pixel.
        let ppp = 1.25;
        view.pan = Vec2::ZERO;
        let layout = view.layout(desktop, rect(0.0, 0.0, 1251.0 / ppp, 1000.0 / ppp), ppp);
        let min = layout.rect.min * ppp;
        let size = layout.rect.size() * ppp;
        let whole = |v: f32| (v - v.round()).abs() < 1e-3;
        assert!(whole(min.x) && (min.x - 113.5).abs() <= 0.5 && (min.y - 116.0).abs() < 1e-3, "{min:?}");
        assert!((size.x - 1024.0).abs() < 1e-3 && (size.y - 768.0).abs() < 1e-3, "{size:?}");
    }

    #[test]
    fn pointer_positions_land_on_desktop_pixels_and_are_clamped() {
        let mut view = View {
            mode: ScaleMode::FitWidth,
            ..View::default()
        };
        let layout = view.layout(rect(0.0, 0.0, 1920.0, 1200.0), rect(0.0, 0.0, 960.0, 1000.0), 1.0);
        assert_eq!(layout.scale, 0.5);
        assert_eq!(layout.desktop_at(pos2(0.0, 200.0)), (0, 0));
        // A device pixel here covers two desktop pixels, and its centre is
        // the second's left edge, which is the second.
        assert_eq!(layout.desktop_at(pos2(480.0, 500.0)), (961, 601));
        assert_eq!(layout.desktop_at(pos2(5000.0, -5.0)), (1919, 0));
        // A monitor's rectangle maps into its own coordinates.
        let layout = view.layout(rect(1920.0, 0.0, 1280.0, 800.0), rect(0.0, 0.0, 1280.0, 800.0), 1.0);
        assert_eq!(layout.desktop_at(pos2(10.0, 10.0)), (1930, 10));
    }

    #[test]
    fn every_edge_pixel_of_the_desktop_can_be_pointed_at() {
        let mut view = View {
            mode: ScaleMode::FitWidth,
            ..View::default()
        };
        // Downscaled by a third: the window's last pixel is the desktop's.
        let layout = view.layout(rect(0.0, 0.0, 1920.0, 1200.0), rect(0.0, 0.0, 1280.0, 800.0), 1.0);
        assert_eq!(layout.desktop_at(pos2(0.0, 0.0)), (0, 0));
        assert_eq!(layout.desktop_at(pos2(1279.0, 799.0)), (1919, 1199));
        assert_eq!(layout.desktop_at(pos2(1278.0, 798.0)), (1917, 1197));
        assert_eq!(layout.desktop_at(pos2(640.0, 400.0)), (960, 600));
        // Downscaled by four, both ends still.
        let layout = view.layout(rect(0.0, 0.0, 5120.0, 2880.0), rect(0.0, 0.0, 1280.0, 800.0), 1.0);
        assert_eq!(layout.desktop_at(pos2(0.0, layout.rect.min.y)), (0, 0));
        assert_eq!(layout.desktop_at(pos2(1279.0, layout.rect.max.y - 1.0)), (5119, 2879));
        assert_eq!(layout.desktop_at(pos2(1.0, layout.rect.min.y + 1.0)), (6, 6));
        // Enlarged by two: each device pixel is the desktop pixel drawn there.
        view.mode = ScaleMode::FillWidth;
        let layout = view.layout(rect(0.0, 0.0, 800.0, 600.0), rect(0.0, 0.0, 1600.0, 1200.0), 1.0);
        assert_eq!(layout.desktop_at(pos2(0.0, 0.0)), (0, 0));
        assert_eq!(layout.desktop_at(pos2(1.0, 2.0)), (0, 1));
        assert_eq!(layout.desktop_at(pos2(3.0, 3.0)), (1, 1));
        assert_eq!(layout.desktop_at(pos2(1599.0, 1199.0)), (799, 599));
    }

    #[test]
    fn at_a_fractional_scale_1_1_maps_each_device_pixel_to_its_own() {
        let mut view = View {
            mode: ScaleMode::Original,
            ..View::default()
        };
        let ppp = 1.25;
        view.pan = vec2(37.3, 0.0);
        let layout = view.layout(rect(0.0, 0.0, 1920.0, 1200.0), rect(0.0, 0.0, 1280.0 / ppp, 800.0 / ppp), ppp);
        let origin = (layout.rect.min.x * ppp).round() as i32;
        for pixel in 0..1280 {
            let (x, _) = layout.desktop_at(pos2(pixel as f32 / ppp, 0.0));
            assert_eq!(x, pixel - origin, "device pixel {pixel}");
        }
    }

    #[test]
    fn the_edge_moves_only_where_there_is_overflow_and_faster_nearer_it() {
        let mut view = View {
            mode: ScaleMode::Original,
            ..View::default()
        };
        view.pan = vec2(400.0, 0.0);
        let layout = view.layout(rect(0.0, 0.0, 1920.0, 500.0), rect(0.0, 0.0, 1000.0, 1000.0), 1.0);
        assert_eq!(layout.edge_velocity(pos2(500.0, 500.0)), Vec2::ZERO);
        // Vertical overflow is nil, so a pointer at the top asks for nothing.
        assert_eq!(layout.edge_velocity(pos2(500.0, 0.0)), Vec2::ZERO);
        assert_eq!(layout.edge_velocity(pos2(0.0, 500.0)), vec2(-EDGE_SPEED, 0.0));
        assert_eq!(layout.edge_velocity(pos2(1000.0, 500.0)), vec2(EDGE_SPEED, 0.0));
        let half = layout.edge_velocity(pos2(EDGE / 2.0, 500.0));
        assert!((half.x + EDGE_SPEED / 2.0).abs() < 1e-3);
    }

    #[test]
    fn the_edge_asks_for_nothing_where_the_pan_is_at_its_limit() {
        let mut view = View {
            mode: ScaleMode::FitWidth,
            ..View::default()
        };
        // Fit width in a window shorter than the desktop: overflow in y.
        let desktop = rect(0.0, 0.0, 1920.0, 1200.0);
        let window = rect(0.0, 0.0, 1280.0, 700.0);
        let layout = view.layout(desktop, window, 1.0);
        assert_eq!(layout.overflow, vec2(0.0, 100.0));
        // At the top already: the top band, where the pointer passes on its
        // way to the toolbar, asks for nothing; the bottom one still pans.
        assert_eq!(layout.edge_velocity(pos2(640.0, 0.0)), Vec2::ZERO);
        assert_eq!(layout.edge_velocity(pos2(640.0, 700.0)), vec2(0.0, EDGE_SPEED));
        // Panned past the bottom, and clamped there: the other way round.
        view.pan = vec2(0.0, 1e6);
        let layout = view.layout(desktop, window, 1.0);
        assert_eq!(layout.pan, vec2(0.0, 100.0));
        assert_eq!(layout.edge_velocity(pos2(640.0, 700.0)), Vec2::ZERO);
        assert_eq!(layout.edge_velocity(pos2(640.0, 0.0)), vec2(0.0, -EDGE_SPEED));
        // Halfway, both bands pan; and a corner pans only the axis that can.
        view.pan = vec2(0.0, 50.0);
        let layout = view.layout(desktop, window, 1.0);
        assert_eq!(layout.edge_velocity(pos2(640.0, 0.0)), vec2(0.0, -EDGE_SPEED));
        assert_eq!(layout.edge_velocity(pos2(0.0, 700.0)), vec2(0.0, EDGE_SPEED));
    }

    #[test]
    fn a_drag_out_of_the_window_carries_on_onto_the_next_monitor() {
        let mut view = View {
            mode: ScaleMode::FitWidth,
            ..View::default()
        };
        // The left of two 1280x800 monitors, shown at half size.
        let layout = view.layout(rect(0.0, 0.0, 1280.0, 800.0), rect(0.0, 0.0, 640.0, 400.0), 1.0);
        let desktop = (2560, 800);
        // Inside the window: as ever.
        assert_eq!(layout.desktop_beyond(pos2(10.0, 10.0), desktop), layout.desktop_at(pos2(10.0, 10.0)));
        // Past its right edge: on into the right monitor at the same scale,
        // where `desktop_at` would press against the left one's edge.
        assert_eq!(layout.desktop_at(pos2(700.0, 100.0)), (1279, 201));
        assert_eq!(layout.desktop_beyond(pos2(700.0, 100.0), desktop), (1401, 201));
        // And held at the whole desktop's edges, not the monitor's.
        assert_eq!(layout.desktop_beyond(pos2(5000.0, -50.0), desktop), (2559, 0));
        assert_eq!(layout.desktop_beyond(pos2(-50.0, 900.0), desktop), (0, 799));
        // The right monitor, dragged out leftwards: back onto the left one.
        let layout = view.layout(rect(1280.0, 0.0, 1280.0, 800.0), rect(0.0, 0.0, 1280.0, 800.0), 1.0);
        assert_eq!(layout.desktop_beyond(pos2(-100.0, 10.0), desktop), (1180, 10));
    }
}
