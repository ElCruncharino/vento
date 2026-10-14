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
//! scale factor; the one place pixels matter is `Original`, which asks for a
//! desktop pixel per *device* pixel, since a 1:1 mode drawn at a fraction of
//! a pixel is neither sharp nor 1:1.

use egui::{Pos2, Rect, Vec2, pos2, vec2};

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
    /// Where `source` lands, in points; may extend past `window`.
    pub rect: Rect,
    /// The area the desktop may use.
    pub window: Rect,
    /// How much of `rect` is past `window` in each axis, zero where the
    /// desktop fits — which is what says whether a pan means anything.
    pub overflow: Vec2,
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
        Layout {
            scale,
            source,
            rect: Rect::from_min_size(origin, scaled),
            window,
            overflow,
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
    pub fn desktop_at(&self, pos: Pos2) -> (i32, i32) {
        let x = self.source.min.x + (pos.x - self.rect.min.x) / self.scale;
        let y = self.source.min.y + (pos.y - self.rect.min.y) / self.scale;
        let max_x = (self.source.max.x - 1.0).max(self.source.min.x);
        let max_y = (self.source.max.y - 1.0).max(self.source.min.y);
        (
            x.floor().clamp(self.source.min.x, max_x) as i32,
            y.floor().clamp(self.source.min.y, max_y) as i32,
        )
    }

    /// The pan velocity a pointer at `pointer` asks for, in points per
    /// second: zero away from the edges, and towards an edge faster the
    /// nearer it is, in each axis where there is anything to pan to.
    pub fn edge_velocity(&self, pointer: Pos2) -> Vec2 {
        let axis = |p: f32, min: f32, max: f32, overflow: f32| -> f32 {
            if overflow <= 0.0 {
                return 0.0;
            }
            if p < min + EDGE {
                -EDGE_SPEED * (1.0 - ((p - min) / EDGE).clamp(0.0, 1.0))
            } else if p > max - EDGE {
                EDGE_SPEED * (1.0 - ((max - p) / EDGE).clamp(0.0, 1.0))
            } else {
                0.0
            }
        };
        vec2(
            axis(pointer.x, self.window.min.x, self.window.max.x, self.overflow.x),
            axis(pointer.y, self.window.min.y, self.window.max.y, self.overflow.y),
        )
    }

    /// The `source` rectangle's share of a texture `width × height`, as the
    /// UV rectangle to draw it with.
    pub fn uv(&self, width: f32, height: f32) -> Rect {
        if width <= 0.0 || height <= 0.0 {
            return Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        }
        Rect::from_min_max(
            pos2(self.source.min.x / width, self.source.min.y / height),
            pos2(self.source.max.x / width, self.source.max.y / height),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn pointer_positions_land_on_desktop_pixels_and_are_clamped() {
        let mut view = View {
            mode: ScaleMode::FitWidth,
            ..View::default()
        };
        let layout = view.layout(rect(0.0, 0.0, 1920.0, 1200.0), rect(0.0, 0.0, 960.0, 1000.0), 1.0);
        assert_eq!(layout.scale, 0.5);
        assert_eq!(layout.desktop_at(pos2(0.0, 200.0)), (0, 0));
        assert_eq!(layout.desktop_at(pos2(480.0, 500.0)), (960, 600));
        assert_eq!(layout.desktop_at(pos2(5000.0, -5.0)), (1919, 0));
        // A monitor's rectangle maps into its own coordinates.
        let layout = view.layout(rect(1920.0, 0.0, 1280.0, 800.0), rect(0.0, 0.0, 1280.0, 800.0), 1.0);
        assert_eq!(layout.desktop_at(pos2(10.0, 10.0)), (1930, 10));
        assert_eq!(layout.uv(3200.0, 800.0), Rect::from_min_max(pos2(0.6, 0.0), pos2(1.0, 1.0)));
    }

    #[test]
    fn the_edge_moves_only_where_there_is_overflow_and_faster_nearer_it() {
        let mut view = View {
            mode: ScaleMode::Original,
            ..View::default()
        };
        let layout = view.layout(rect(0.0, 0.0, 1920.0, 500.0), rect(0.0, 0.0, 1000.0, 1000.0), 1.0);
        assert_eq!(layout.edge_velocity(pos2(500.0, 500.0)), Vec2::ZERO);
        // Vertical overflow is nil, so a pointer at the top asks for nothing.
        assert_eq!(layout.edge_velocity(pos2(500.0, 0.0)), Vec2::ZERO);
        assert_eq!(layout.edge_velocity(pos2(0.0, 500.0)), vec2(-EDGE_SPEED, 0.0));
        assert_eq!(layout.edge_velocity(pos2(1000.0, 500.0)), vec2(EDGE_SPEED, 0.0));
        let half = layout.edge_velocity(pos2(EDGE / 2.0, 500.0));
        assert!((half.x + EDGE_SPEED / 2.0).abs() < 1e-3);
    }
}
