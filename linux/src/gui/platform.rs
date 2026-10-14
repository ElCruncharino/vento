// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which display server the window is on, and the per-server pieces that
//! the rest of the window reaches through a `match` rather than a cfg.
//!
//! winit chooses: Wayland where `WAYLAND_DISPLAY` (or `WAYLAND_SOCKET`) is
//! set, X11 where only `DISPLAY` is, and no falling back from one to the
//! other. `--x11` and `--wayland` override that, and the window's raw
//! display handle says afterwards which it got.

use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
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
