// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which display server the window is on, and the per-server pieces that
//! the rest of the window reaches through a `match` rather than a cfg.
//!
//! winit chooses: Wayland where `WAYLAND_DISPLAY` (or `WAYLAND_SOCKET`) is
//! set, X11 where only `DISPLAY` is, and no falling back from one to the
//! other. `--x11` and `--wayland` override that, and the window's raw
//! display handle says afterwards which it got.

use super::{wayland, x11};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use std::time::Instant;
use winit::dpi::PhysicalPosition;
use winit::window::Window;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Wayland,
    X11,
    Other,
}

impl Platform {
    /// From the window's raw display handle.
    pub fn of(window: &Window) -> Platform {
        match window.display_handle().map(|h| h.as_raw()) {
            Ok(RawDisplayHandle::Wayland(_)) => Platform::Wayland,
            Ok(RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_)) => Platform::X11,
            _ => Platform::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Platform::Wayland => "Wayland",
            Platform::X11 => "X11",
            Platform::Other => "other",
        }
    }
}

/// Where the pointer is held while captured on X11: the window's centre.
pub fn centre(window: &Window) -> PhysicalPosition<i32> {
    let s = window.inner_size();
    PhysicalPosition::new((s.width / 2) as i32, (s.height / 2) as i32)
}

/// The compositor's or the X server's own shortcuts, held off while the
/// window has focus and a session is connected: Wayland's inhibitor
/// protocol (`wayland.rs`), or an X keyboard grab (`x11.rs`).
pub enum Shortcuts {
    Wayland(wayland::Shortcuts),
    X11(x11::Keyboard),
}

impl Shortcuts {
    /// `None` on a display server with neither.
    pub fn new(window: &Window) -> Option<Shortcuts> {
        match Platform::of(window) {
            Platform::Wayland => wayland::Shortcuts::new(window).map(Shortcuts::Wayland),
            Platform::X11 => x11::Keyboard::new(window).map(Shortcuts::X11),
            Platform::Other => None,
        }
    }

    /// Wayland: dispatch what the compositor said. X11: retry a refused
    /// grab once it is due.
    pub fn pump(&mut self) {
        match self {
            Shortcuts::Wayland(s) => s.pump(),
            Shortcuts::X11(k) => k.pump(),
        }
    }

    pub fn inhibit(&mut self) {
        match self {
            Shortcuts::Wayland(s) => s.inhibit(),
            Shortcuts::X11(k) => k.inhibit(),
        }
    }

    pub fn release(&mut self) {
        match self {
            Shortcuts::Wayland(s) => s.release(),
            Shortcuts::X11(k) => k.release(),
        }
    }

    /// Where it is, in a word or three, for the debug overlay.
    pub fn state(&self) -> &'static str {
        match self {
            Shortcuts::Wayland(s) => s.state(),
            Shortcuts::X11(k) => k.state(),
        }
    }

    /// X11 only: a `Focused(false)` that our own grab caused, while the
    /// focus is really still ours (see `x11.rs`). Always false on Wayland.
    pub fn focus_still_ours(&self, window: &Window) -> bool {
        matches!(self, Shortcuts::X11(k) if k.focus_still_ours(window.inner_size()))
    }

    /// X11 only: when a refused grab is to be asked for again.
    pub fn retry_at(&self) -> Option<Instant> {
        match self {
            Shortcuts::Wayland(_) => None,
            Shortcuts::X11(k) => k.retry_at(),
        }
    }
}
