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
//!
//! And relative mode's [`Fence`]: four XFixes pointer barriers on the
//! window's edges, so that a captured pointer cannot leave the window
//! between a move and the warp back to the centre that undoes it.

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use std::time::{Duration, Instant};
use winit::dpi::PhysicalSize;
use winit::window::Window;
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{BarrierDirections, ConnectionExt as _};
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

/// winit's own connection and the window's id, or `None` on anything but
/// an X11 window. `what` is for the log, should the connection be refused.
fn winit_connection(window: &Window, what: &str) -> Option<(XCBConnection, u32)> {
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
    match unsafe { XCBConnection::from_raw_xcb_connection(ptr, false) } {
        Ok(conn) => Some((conn, raw_window)),
        Err(e) => {
            log::warn!("X11: cannot use winit's connection for {what}: {e}");
            None
        }
    }
}

impl Keyboard {
    /// `None` on anything but an X11 window.
    pub fn new(window: &Window) -> Option<Keyboard> {
        let (conn, raw_window) = winit_connection(window, "the keyboard grab")?;
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

/// Relative mode's fence: XFixes pointer barriers on the window's four
/// edges while the pointer is captured.
///
/// The capture warps the pointer back to the centre each time it moves,
/// but only once the event loop has the move, and until then the pointer
/// is wherever the move took it. A fast one goes past the edge, and a click
/// in that moment went to the window under it: `xdotool mousemove_relative
/// -- 700 0 click 1` activated the xterm beside the window and ended the
/// capture every time. A barrier stops the pointer at the edge, inside, so
/// the click is the window's.
///
/// Barriers rather than the active pointer grab that `confine_to` would be,
/// because with a grab the X server delivers each XI2 raw motion twice, once
/// for the grab and once for the root window's selection, and relative mode
/// is made of raw motion. A barrier changes nothing of what is delivered:
/// raw motion is the device's, measured before the barrier clamps the
/// pointer, so a move into the edge still reaches the far end at its full
/// length. They only hold relative motion — a warp passes, which is what the
/// centring and the restore are, and so does an absolute device, which
/// relative mode cannot use anyway.
///
/// Made over winit's connection like the keyboard grab, so they are gone
/// with the process whatever becomes of it. XFixes 5 is needed, asked for on
/// that connection (winit itself uses no XFixes); without it the capture
/// works as before, unfenced.
pub struct Fence {
    conn: XCBConnection,
    window: u32,
    barriers: Vec<u32>,
    /// The rectangle the barriers are on, in root coordinates.
    around: Option<(i32, i32, u32, u32)>,
}

impl Fence {
    /// `None` on anything but an X11 window, or an X server without
    /// XFixes 5.
    pub fn new(window: &Window) -> Option<Fence> {
        let (conn, raw_window) = winit_connection(window, "the pointer barriers")?;
        let version = conn.xfixes_query_version(5, 0).ok().and_then(|c| c.reply().ok());
        match version {
            Some(v) if v.major_version >= 5 => {}
            Some(v) => {
                log::warn!(
                    "X11: XFixes {}.{} has no pointer barriers; a captured pointer can leave \
                     the window on a fast move",
                    v.major_version,
                    v.minor_version
                );
                return None;
            }
            None => {
                log::warn!("X11: no XFixes; a captured pointer can leave the window on a fast move");
                return None;
            }
        }
        Some(Fence { conn, window: raw_window, barriers: Vec::new(), around: None })
    }

    /// Barriers on the window's edges as it is now, moved there if they
    /// were somewhere else.
    pub fn raise(&mut self) {
        let Some(geometry) = self.conn.get_geometry(self.window).ok().and_then(|c| c.reply().ok()) else {
            return;
        };
        let Some(at) = self
            .conn
            .translate_coordinates(self.window, geometry.root, 0, 0)
            .ok()
            .and_then(|c| c.reply().ok())
        else {
            return;
        };
        let around = (
            i32::from(at.dst_x),
            i32::from(at.dst_y),
            u32::from(geometry.width),
            u32::from(geometry.height),
        );
        if self.around == Some(around) {
            return;
        }
        self.lower();
        for (x1, y1, x2, y2) in edges(around) {
            let Ok(id) = self.conn.generate_id() else {
                break;
            };
            // No direction let through, and every master pointer.
            let made = self
                .conn
                .xfixes_create_pointer_barrier(id, geometry.root, x1, y1, x2, y2, BarrierDirections::from(0u8), &[])
                .map(|c| c.check());
            match made {
                Ok(Ok(())) => self.barriers.push(id),
                Ok(Err(e)) => log::warn!("X11: pointer barrier refused: {e:?}"),
                Err(e) => log::warn!("X11: pointer barrier failed: {e}"),
            }
        }
        self.around = Some(around);
        let (x, y, w, h) = around;
        log::debug!("X11: pointer fenced to {w}x{h} at {x},{y} ({} barriers)", self.barriers.len());
    }

    /// No barriers.
    pub fn lower(&mut self) {
        for id in self.barriers.drain(..) {
            if let Ok(cookie) = self.conn.xfixes_delete_pointer_barrier(id) {
                cookie.ignore_error();
            }
        }
        if self.around.take().is_some() {
            let _ = self.conn.flush();
        }
    }
}

/// The four barriers around a rectangle in root coordinates, as x1, y1, x2,
/// y2: left, right, top, bottom. A vertical barrier stops a pointer moving
/// right at its x - 1 and one moving left at its x, a horizontal one
/// likewise, so on the edges themselves they keep the pointer on the
/// window's own pixels. Coordinates are unsigned: an edge off the screen's
/// top or left is held at 0, which is still inside the window. One that
/// would be a point (a window of no width) is left out, as the server
/// refuses it.
fn edges((x, y, w, h): (i32, i32, u32, u32)) -> Vec<(u16, u16, u16, u16)> {
    let clamp = |v: i64| v.clamp(0, i64::from(u16::MAX)) as u16;
    let (left, top) = (clamp(x.into()), clamp(y.into()));
    let (right, bottom) = (clamp(i64::from(x) + i64::from(w)), clamp(i64::from(y) + i64::from(h)));
    [
        (left, top, left, bottom),
        (right, top, right, bottom),
        (left, top, right, top),
        (left, bottom, right, bottom),
    ]
    .into_iter()
    .filter(|&(x1, y1, x2, y2)| x1 != x2 || y1 != y2)
    .collect()
}

impl Drop for Fence {
    fn drop(&mut self) {
        self.lower();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fence_is_on_the_window_s_edges() {
        assert_eq!(
            edges((10, 107, 1280, 800)),
            vec![(10, 107, 10, 907), (1290, 107, 1290, 907), (10, 107, 1290, 107), (10, 907, 1290, 907)]
        );
        // Off the screen's top left: held at 0, which is still the window.
        assert_eq!(
            edges((-100, -20, 300, 200)),
            vec![(0, 0, 0, 180), (200, 0, 200, 180), (0, 0, 200, 0), (0, 180, 200, 180)]
        );
        // Wholly off it: nothing that is a point.
        assert!(edges((-500, -500, 100, 100)).is_empty());
    }
}
