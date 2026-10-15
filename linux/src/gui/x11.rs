// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! X11's stand-in for the keyboard shortcuts inhibitor: an active keyboard
//! grab on the window while it has focus and a session takes input
//! (connected, not view only, no prompt up), so the window manager's
//! chords (Alt+Tab, Super+…) reach the far end.
//!
//! It is sent over winit's own connection — the Xlib display from the raw
//! handle, as XCB — because X reports a grabbed key only to the client that
//! grabbed it: a connection of our own would take every key away from
//! winit. Xlib keeps owning the event queue; this only sends requests and
//! waits for their replies, never reads events, which libxcb is safe for.
//! winit itself wraps the same display the same way. With
//! `owner_events` on, winit keeps receiving every key on its window as an
//! ordinary `KeyboardInput`.
//!
//! The grab can be refused — the window manager's Alt+Tab switcher or a
//! menu holds the keyboard — and is then asked for again every
//! [`RETRY`] while it is still wanted; the overlay says why meanwhile.
//! The way out of a grabbed window is the mouse: a click elsewhere has the
//! window manager move the focus, and the focus-out ends the grab.
//!
//! Also here: a focus-out the grab itself caused, told apart from a real
//! one. A grab and an ungrab make the server send FocusOut and FocusIn with
//! the Grab and Ungrab modes, which winit reports as plain focus changes;
//! where the focus is PointerRoot (no window manager) that becomes a loop of
//! ungrab and grab, thousands a second. [`Keyboard::focus_still_ours`] asks
//! the server where the focus really is.

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use std::time::{Duration, Instant};
use winit::dpi::PhysicalSize;
use winit::window::Window;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, GrabStatus, InputFocus};
use x11rb::xcb_ffi::XCBConnection;

/// How long a refused grab waits before it is asked for again.
pub const RETRY: Duration = Duration::from_millis(250);

/// What became of the last grab asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Last {
    Released,
    Grabbed,
    Refused(GrabStatus),
    Failed,
}

pub struct Keyboard {
    conn: XCBConnection,
    window: u32,
    held: bool,
    wanted: bool,
    last: Last,
    retry_at: Option<Instant>,
}

impl Keyboard {
    /// `None` on anything but an X11 window.
    pub fn new(window: &Window) -> Option<Keyboard> {
        let raw_window = match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(w) => u32::try_from(w.window).ok()?,
            RawWindowHandle::Xcb(w) => w.window.get(),
            _ => return None,
        };
        let ptr = match window.display_handle().ok()?.as_raw() {
            RawDisplayHandle::Xlib(h) => {
                let display = h.display?;
                let xlib_xcb = x11_dl::xlib_xcb::Xlib_xcb::open().ok()?;
                // SAFETY: the display is winit's process-wide connection,
                // live for as long as the window is.
                unsafe { (xlib_xcb.XGetXCBConnection)(display.as_ptr().cast()) }.cast()
            }
            RawDisplayHandle::Xcb(h) => h.connection?.as_ptr(),
            _ => return None,
        };
        if ptr.is_null() {
            return None;
        }
        // SAFETY: winit's connection, which is never closed while the window
        // lives; `should_drop` false, so dropping this wrapper leaves it be.
        let conn = match unsafe { XCBConnection::from_raw_xcb_connection(ptr, false) } {
            Ok(conn) => conn,
            Err(e) => {
                log::warn!("X11: cannot use winit's connection for the keyboard grab: {e}");
                return None;
            }
        };
        let xwayland = conn
            .query_extension(b"XWAYLAND")
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.present);
        if xwayland {
            log::info!(
                "X11: this X server is Xwayland; its compositor decides whether a keyboard grab holds \
                 off the compositor's own shortcuts — the Wayland backend is the better choice here"
            );
        }
        Some(Keyboard {
            conn,
            window: raw_window,
            held: false,
            wanted: false,
            last: Last::Released,
            retry_at: None,
        })
    }

    /// Grab the keyboard, from now until `release`; a refusal is retried.
    pub fn inhibit(&mut self) {
        self.wanted = true;
        if !self.held && self.retry_at.is_none_or(|t| t <= Instant::now()) {
            self.grab();
        }
    }

    /// A refused grab, asked for again once it is due.
    pub fn pump(&mut self) {
        if self.wanted && !self.held && self.retry_at.is_some_and(|t| t <= Instant::now()) {
            self.grab();
        }
    }

    fn grab(&mut self) {
        let reply = self
            .conn
            .grab_keyboard(true, self.window, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)
            .map_err(|e| e.to_string())
            .and_then(|c| c.reply().map_err(|e| e.to_string()));
        match reply {
            Ok(r) if r.status == GrabStatus::SUCCESS => {
                self.held = true;
                self.retry_at = None;
                self.last = Last::Grabbed;
                log::info!("X11 keyboard grabbed");
            }
            Ok(r) => {
                self.retry_at = Some(Instant::now() + RETRY);
                if self.last != Last::Refused(r.status) {
                    log::info!("X11 keyboard grab refused: {}; retrying", refusal(r.status));
                }
                self.last = Last::Refused(r.status);
            }
            Err(e) => {
                self.retry_at = Some(Instant::now() + RETRY);
                if self.last != Last::Failed {
                    log::warn!("X11 keyboard grab failed: {e}; retrying");
                }
                self.last = Last::Failed;
            }
        }
    }

    pub fn release(&mut self) {
        self.wanted = false;
        self.retry_at = None;
        if self.held {
            if let Ok(cookie) = self.conn.ungrab_keyboard(x11rb::CURRENT_TIME) {
                cookie.ignore_error();
            }
            let _ = self.conn.flush();
            self.held = false;
            log::info!("X11 keyboard released");
        }
        self.last = Last::Released;
    }

    /// Where the grab is, in a few words, for the debug overlay.
    pub fn state(&self) -> &'static str {
        if self.held {
            return "grabbed";
        }
        if !self.wanted {
            return "released";
        }
        match self.last {
            Last::Refused(s) if s == GrabStatus::ALREADY_GRABBED => "refused (already grabbed)",
            Last::Refused(s) if s == GrabStatus::FROZEN => "refused (frozen)",
            Last::Refused(s) if s == GrabStatus::NOT_VIEWABLE => "refused (not viewable)",
            Last::Refused(s) if s == GrabStatus::INVALID_TIME => "refused (invalid time)",
            Last::Refused(_) => "refused",
            Last::Failed => "failed",
            Last::Released | Last::Grabbed => "waiting",
        }
    }

    /// When a refused grab is due to be asked for again.
    pub fn retry_at(&self) -> Option<Instant> {
        self.retry_at.filter(|_| self.wanted && !self.held)
    }

    /// Whether the X focus is still this window although winit said
    /// otherwise: the focus is the window itself, or PointerRoot with the
    /// pointer inside it. Two round trips, and only on a focus-out while
    /// the grab is held.
    pub fn focus_still_ours(&self, size: PhysicalSize<u32>) -> bool {
        if !self.held {
            return false;
        }
        let Some(focus) = self.conn.get_input_focus().ok().and_then(|c| c.reply().ok()) else {
            return false;
        };
        if focus.focus == self.window {
            return true;
        }
        if focus.focus == u32::from(InputFocus::POINTER_ROOT) {
            let Some(p) = self.conn.query_pointer(self.window).ok().and_then(|c| c.reply().ok()) else {
                return false;
            };
            return p.same_screen
                && p.win_x >= 0
                && p.win_y >= 0
                && (p.win_x as u32) < size.width
                && (p.win_y as u32) < size.height;
        }
        false
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        self.release();
    }
}

fn refusal(status: GrabStatus) -> &'static str {
    match status {
        GrabStatus::ALREADY_GRABBED => "already grabbed",
        GrabStatus::FROZEN => "frozen",
        GrabStatus::NOT_VIEWABLE => "not viewable",
        GrabStatus::INVALID_TIME => "invalid time",
        _ => "unknown status",
    }
}
