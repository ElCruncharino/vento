// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The one Wayland protocol winit does not speak for us: the keyboard
//! shortcuts inhibitor, so that Alt+Tab and the compositor's other chords
//! reach the far end while the window has focus.
//!
//! winit owns the connection and the surface; this module borrows both
//! through the window's raw handles — the `wl_display` and `wl_surface`
//! pointers are wrapped as a foreign `wayland_client::Connection` and a
//! proxy on it — and binds what it needs from the registry on an event queue
//! of its own. Nothing here blocks: the registry is asked for its globals and
//! the answer arrives on later frames, read by winit's own socket read and
//! dispatched by [`Shortcuts::pump`], so the inhibitor appears a frame or two
//! after focus and the event loop never waits on the compositor from inside
//! one of its own callbacks. The `active`/`inactive` events say whether the
//! compositor granted it, and are logged, since the only way to see them
//! otherwise is `WAYLAND_DEBUG=1`.
//!
//! The pointer lock for relative mode needs none of this: winit's
//! `CursorGrabMode::Locked` is `zwp_pointer_constraints_v1` already, and its
//! raw motion is `zwp_relative_pointer_v1` as `DeviceEvent::MouseMotion`.
//!
//! X11 has no such protocol; its stand-in is a keyboard grab, in `x11.rs`,
//! and `platform.rs` puts the two behind one `Shortcuts`.

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::protocol::{wl_registry, wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::{
    zwp_keyboard_shortcuts_inhibit_manager_v1 as manager, zwp_keyboard_shortcuts_inhibitor_v1 as inhibitor,
};
use winit::window::Window;

/// What the registry has said so far, and what the inhibitor has.
#[derive(Default)]
struct Globals {
    manager: Option<manager::ZwpKeyboardShortcutsInhibitManagerV1>,
    seat: Option<wl_seat::WlSeat>,
    /// The compositor's last word on the inhibitor: granted or not.
    active: bool,
}

pub struct Shortcuts {
    _conn: Connection,
    queue: EventQueue<Globals>,
    globals: Globals,
    surface: wl_surface::WlSurface,
    inhibitor: Option<inhibitor::ZwpKeyboardShortcutsInhibitorV1>,
    /// Asked for before the manager had arrived; granted on the first pump
    /// that finds it.
    wanted: bool,
}

impl Shortcuts {
    /// `None` on anything that is not a Wayland window.
    pub fn new(window: &Window) -> Option<Shortcuts> {
        let display = match window.display_handle().ok()?.as_raw() {
            RawDisplayHandle::Wayland(h) => h.display.as_ptr(),
            _ => return None,
        };
        let surface = match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Wayland(h) => h.surface.as_ptr(),
            _ => return None,
        };
        // SAFETY: both pointers come from winit's live window and outlive
        // this struct, which is dropped with the window; the display is
        // marked foreign so the connection is never closed from here.
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        let surface_id =
            unsafe { ObjectId::from_ptr(wl_surface::WlSurface::interface(), surface.cast()) }.ok()?;
        let surface = wl_surface::WlSurface::from_id(&conn, surface_id).ok()?;

        let queue = conn.new_event_queue::<Globals>();
        let qh = queue.handle();
        // The registry's globals arrive as events; `pump` binds them.
        conn.display().get_registry(&qh, ());
        let _ = conn.flush();
        Some(Shortcuts {
            _conn: conn,
            queue,
            globals: Globals::default(),
            surface,
            inhibitor: None,
            wanted: false,
        })
    }

    /// Dispatch what has arrived on this queue, and make good on a pending
    /// `inhibit` once the manager and a seat are known.
    pub fn pump(&mut self) {
        let _ = self.queue.dispatch_pending(&mut self.globals);
        if self.wanted && self.inhibitor.is_none() {
            self.create();
        }
        let _ = self.queue.flush();
    }

    /// Ask for the compositor's shortcuts, from now until `release`.
    pub fn inhibit(&mut self) {
        self.wanted = true;
        if self.inhibitor.is_none() {
            self.create();
        }
    }

    fn create(&mut self) {
        let (Some(manager), Some(seat)) = (&self.globals.manager, &self.globals.seat) else {
            return;
        };
        let qh = self.queue.handle();
        let inhibitor = manager.inhibit_shortcuts(&self.surface, seat, &qh, ());
        let _ = self.queue.flush();
        log::info!("keyboard shortcuts inhibitor created");
        self.inhibitor = Some(inhibitor);
    }

    pub fn release(&mut self) {
        self.wanted = false;
        if let Some(inhibitor) = self.inhibitor.take() {
            inhibitor.destroy();
            self.globals.active = false;
            let _ = self.queue.flush();
            log::info!("keyboard shortcuts inhibitor released");
        }
    }

    /// Where the inhibitor is, in a word, for the debug overlay.
    pub fn state(&self) -> &'static str {
        match (&self.inhibitor, self.wanted, self.globals.active) {
            (Some(_), _, true) => "active",
            (Some(_), _, false) => "inactive",
            (None, true, _) => "waiting",
            (None, false, _) => "released",
        }
    }
}

impl Drop for Shortcuts {
    fn drop(&mut self) {
        self.release();
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Globals {
    fn event(
        state: &mut Globals,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Globals>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "zwp_keyboard_shortcuts_inhibit_manager_v1" if state.manager.is_none() => {
                    state.manager = Some(registry.bind(name, version.min(1), qh, ()));
                }
                // The first seat is the one with the keyboard on every
                // desktop this will run on; a second would need a choice.
                "wl_seat" if state.seat.is_none() => {
                    state.seat = Some(registry.bind(name, version.min(1), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<inhibitor::ZwpKeyboardShortcutsInhibitorV1, ()> for Globals {
    fn event(
        state: &mut Globals,
        _: &inhibitor::ZwpKeyboardShortcutsInhibitorV1,
        event: inhibitor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Globals>,
    ) {
        match event {
            inhibitor::Event::Active => {
                state.active = true;
                log::info!("keyboard shortcuts inhibitor active");
            }
            inhibitor::Event::Inactive => {
                state.active = false;
                log::info!("keyboard shortcuts inhibitor inactive");
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Globals: ignore manager::ZwpKeyboardShortcutsInhibitManagerV1);
wayland_client::delegate_noop!(Globals: ignore wl_seat::WlSeat);
