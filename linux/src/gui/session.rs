// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A live session on screen: the desktop drawn, the mouse and keyboard
//! forwarded, the toolbar, the info panel, the prompts, and the cursor.
//!
//! Everything the window does with a backend goes through here, and all of
//! it is done from the main thread without waiting on the backend: events
//! come off the [`Mailbox`] when the window is repainted (the mailbox's wake
//! hook asks for the repaint), pixels come out through `read_region`, which
//! never queues behind the protocol, and every call into the backend is one
//! that returns at once. The facts and the connection info are asked for
//! once a second, which is what names the window and fills the panel; the
//! same tick is what the follower (`follow.rs`) is asked on. What is changed
//! on the panel comes back as actions and is applied here, so the session
//! is the one owner of what an option is set to.
//!
//! Input goes to the far end only while there is somewhere for it to go:
//! connected, not view-only, no prompt on screen, and no egui widget asking
//! for the keyboard. The mouse is forwarded from the desktop's own widget
//! response, so a click on the toolbar is a click on the toolbar; the
//! keyboard comes straight from winit (see `keys.rs`) because egui's key
//! events do not carry the layout's idea of a key.
//!
//! The cursor is the one place the two pointer modes differ on screen. In
//! absolute mode the far end sends its cursor's shape and this draws it
//! under the local pointer, hiding the OS cursor over the desktop, as the
//! Android app does — or, with the native cursor setting, makes it the
//! window's own pointer (`cursor.rs`). In relative mode the far end draws
//! its own cursor into the picture and owns the motion, so the local
//! pointer is captured and hidden while the window has focus and its raw
//! motion is sent as deltas; Ctrl+Alt held together lets it go, and a click
//! on the desktop takes it back. On Wayland the capture is a pointer lock;
//! on X11, which has none, the pointer is hidden and warped back to the
//! window's centre whenever it moves (see `sync_capture`).
//!
//! The clipboard crosses in both directions under the two settings the
//! phone has (`settings.rs`), with its rule: the last text seen crossing
//! either way is remembered, and is never sent back where it came from.
//!
//! The debug overlay (`hud.rs`) is the phone's `SessionHud`: every
//! collaborator's state, in a corner, from the toolbar or Ctrl+Alt+D.

use super::cursor::{self as native, Key, Native};
use super::desktop::Desktop;
use super::follow::Follower;
use super::hud::{self, Hud};

use super::keys::{self, KeyIds};
use super::panel::{Action, Controls, Panel, Snapshot};
use super::platform::{self, Platform};
use super::prompts::{self, Answer, Form, Prompt};
use super::settings::Settings;
use super::view::{Layout, View};
use super::{Host, Request};
use crate::backend::Backend;
use crate::backends::{self, Connect, Dirs, Pinned};
use crate::events::{Cursor, Event, Identity, Mailbox, State};
use crate::known_hosts::{self, KnownHosts, Store, Verdict};
use crate::options::Options;
use egui::{
    Align2, Color32, CursorIcon, Frame, Id, LayerId, Order, PointerButton, Pos2, Rect, Response, Sense,
    TextureHandle, TextureOptions, Vec2, pos2, vec2,
};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use winit::keyboard::{Key as LogicalKey, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen};
use xkeysym::Keysym;

/// RFB's button bits, which every backend takes.
const LEFT: i32 = 1;
const MIDDLE: i32 = 2;
const RIGHT: i32 = 4;
const WHEEL_UP: i32 = 8;
const WHEEL_DOWN: i32 = 16;
const WHEEL_LEFT: i32 = 32;
const WHEEL_RIGHT: i32 = 64;

/// How many points of smooth scrolling make one notch of the wheel; what
/// egui-winit turns one line of a wheel into.
const NOTCH_POINTS: f32 = 50.0;

/// How long the toolbar stays out after connecting, so that it is seen once.
const TOOLBAR_REVEAL: Duration = Duration::from_secs(3);
/// How long the bell rings in the title.
const BELL: Duration = Duration::from_millis(600);
/// How long a disconnect is given before the session is closed regardless.
const DISCONNECT_PATIENCE: Duration = Duration::from_secs(10);
/// How often the facts and the info are asked for.
const POLL: Duration = Duration::from_secs(1);
/// How early a frame may come and still count as the poll's. egui takes a
/// frame's predicted time (1/60 s by default) off every repaint asked for
/// later, so the wake for the poll lands that much before the second is up;
/// counted as not due, it asked again and a second frame followed straight
/// after, two draws a second where one will do.
const POLL_EARLY: Duration = Duration::from_millis(50);

/// How the local pointer is held in relative mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Capture {
    /// Wayland: the compositor's pointer lock.
    Locked,
    /// X11: hidden, no grab, and warped back to the window's centre.
    Centred,
}

/// The far end's cursor, uploaded once per shape, and kept as it arrived
/// for the native cursor, which resamples it to the view's scale.
struct CursorShape {
    texture: TextureHandle,
    hot: Vec2,
    size: Vec2,
    source: Cursor,
    hash: u64,
}

pub struct Session {
    pub request: Request,
    /// The client's settings as this session runs with them: what it
    /// started with, then whatever the panel changed.
    pub settings: Settings,
    backend: Arc<dyn Backend>,
    mailbox: Arc<Mailbox>,
    started: Instant,
    desktop: Option<Desktop>,
    snapshot: Snapshot,
    last_poll: Option<Instant>,
    view: View,
    /// Which of `facts.monitors` is on screen, or `None` for the whole
    /// desktop.
    monitor: Option<usize>,
    toolbar_height: f32,
    toolbar_hovered: bool,
    panel: Panel,
    /// Every option's value as the session has it: what it connected with,
    /// then whatever has been set since, which is what the panel's controls
    /// show.
    live: Options,
    follower: Follower,
    prompts: VecDeque<Prompt>,
    form: Form,
    /// The pins a certificate is checked against, and what this session's
    /// are pinned as.
    known_hosts: KnownHosts,
    pinned: Pinned,
    /// Where RealVNC's remembered identity is kept, by address.
    identities: Store,
    cursor: Option<CursorShape>,
    /// The far end's cursor as the window's own, where that is wanted.
    native: Native,
    /// The shape the window's cursor should be this frame, if the far
    /// end's: the native cursor setting, absolute mode, the pointer over
    /// the desktop, and a shape to show.
    native_key: Option<Key>,
    /// What the far end said about who owns the pointer.
    relative: bool,
    /// How the local pointer is held right now, if it is.
    capture: Option<Capture>,
    /// X11: where the pointer was before it was captured, put back when it
    /// is let go.
    restore: Option<PhysicalPosition<f64>>,
    /// Raw motion sent while captured, in all: the overlay's `Δ`, which is
    /// how doubled motion would show.
    motion_sent: (i64, i64),
    /// The escape chord was used; a click on the desktop ends it.
    escaped: bool,
    key_ids: KeyIds,
    /// The physical keys this session has down at the far end, and the
    /// keysym each went down as.
    held: HashMap<PhysicalKey, u32>,
    /// Every physical key seen down and not yet up, whether or not it was
    /// sent: what a local chord is read from, since a chord's keys are
    /// let go of at the far end as soon as the chord is seen.
    down: HashMap<PhysicalKey, u32>,
    /// The last pointer event sent, so the same one is not sent twice.
    last_pointer: (i32, i32, i32),
    /// Smooth scrolling not yet amounting to a notch.
    wheel: Vec2,
    /// Raw motion not yet amounting to a pixel.
    motion_rest: (f64, f64),
    /// The buttons held while captured, where egui's response is not asked.
    captured_mask: i32,
    view_only: bool,
    /// The last clipboard text that crossed in either direction, so that it
    /// is not sent back where it came from.
    clipboard_seen: Option<String>,
    bell_until: Option<Instant>,
    title: String,
    disconnecting: Option<Instant>,
    hud: Hud,
    /// Damage rectangles received, in all.
    damage: u64,
    /// The desktop's upload count at the last frame, for the per-frame
    /// figure.
    uploads_seen: u64,
    /// Whether the pointer was over the desktop in the last frame.
    hovered: bool,
    /// The session is over, with the detail the backend gave (empty for an
    /// ending that was asked for).
    pub closed: Option<String>,
}

impl Session {
    /// Starts connecting. `screen` is the window's size in pixels, which
    /// is what the RDP backends' `DesktopSize=device` asks for.
    pub fn start(
        request: Request,
        dirs: &Dirs,
        settings: Settings,
        screen: (u32, u32),
        wake: Box<dyn Fn() + Send + Sync>,
    ) -> Result<Session, String> {
        let mailbox = Mailbox::new(Some(wake));
        let connect = Connect {
            address: &request.address,
            user: &request.user,
            domain: &request.domain,
            password: request.password.as_deref(),
            options: &request.options,
            mailbox: Arc::clone(&mailbox),
            dirs,
            screen: Some(screen),
        };
        let backend = (request.kind.create)(&connect)?;
        let title = request.address.clone();
        let live = request.options.clone();
        let pinned = backends::pinned_as(request.kind, &request.address, &request.options);
        Ok(Session {
            request,
            settings,
            backend,
            mailbox,
            started: Instant::now(),
            desktop: None,
            snapshot: Snapshot::default(),
            last_poll: None,
            view: View::default(),
            monitor: None,
            toolbar_height: 40.0,
            toolbar_hovered: false,
            panel: Panel::default(),
            live,
            follower: Follower::default(),
            prompts: VecDeque::new(),
            form: Form::default(),
            known_hosts: KnownHosts::in_dir(&dirs.config),
            pinned,
            identities: known_hosts::realvnc_identities(&dirs.config),
            cursor: None,
            native: Native::default(),
            native_key: None,
            relative: false,
            capture: None,
            restore: None,
            motion_sent: (0, 0),
            escaped: false,
            key_ids: KeyIds::default(),
            held: HashMap::new(),
            down: HashMap::new(),
            last_pointer: (-1, -1, 0),
            wheel: Vec2::ZERO,
            motion_rest: (0.0, 0.0),
            captured_mask: 0,
            view_only: false,
            clipboard_seen: None,
            bell_until: None,
            title,
            disconnecting: None,
            hud: Hud::default(),
            damage: 0,
            uploads_seen: 0,
            hovered: false,
            closed: None,
        })
    }

    pub fn connected(&self) -> bool {
        self.mailbox.state() == State::Connected && self.closed.is_none()
    }

    /// Whether input has somewhere to go.
    fn accepts_input(&self) -> bool {
        self.connected() && !self.view_only && self.prompts.is_empty() && self.disconnecting.is_none()
    }

    /// Whether a key event from winit is the far end's rather than egui's.
    pub fn takes_keys(&self, ctx: &egui::Context) -> bool {
        self.accepts_input() && !ctx.egui_wants_keyboard_input()
    }

    /// Whether mouse events from winit go to the far end as raw input,
    /// which is only while the pointer is captured.
    pub fn takes_raw_mouse(&self) -> bool {
        self.captured() && self.accepts_input()
    }

    fn captured(&self) -> bool {
        self.capture.is_some()
    }

    /// Whether the pointer is being held at the window's centre (X11's
    /// capture), so the window swallows its motion and warps it back.
    pub fn centred(&self) -> bool {
        self.capture == Some(Capture::Centred) && self.accepts_input()
    }

    /// What the window is called: the desktop's name once known, with the
    /// bell on it while it rings.
    pub fn title(&self) -> String {
        let name = self.snapshot.desktop_name().unwrap_or(&self.title);
        let ringing = self.bell_until.is_some_and(|t| Instant::now() < t);
        if ringing {
            format!("🔔 {name} — Vento")
        } else {
            format!("{name} — Vento")
        }
    }

    // ---- events from the backend -------------------------------------------

    fn drain(&mut self, ctx: &egui::Context, host: &mut Host) {
        while let Some(event) = self.mailbox.take() {
            match event {
                Event::Connected { width, height } | Event::DesktopSize { width, height } => {
                    if let Event::DesktopSize { .. } = event {
                        log::info!("desktop size now {width}×{height}");
                    }
                    if let Some(old) = self.desktop.take() {
                        old.free(ctx);
                    }
                    if width > 0 && height > 0 {
                        let mut desktop = Desktop::new(ctx, width, height);
                        // What is there already, in case the frame that drew
                        // it came before this size did.
                        desktop.update(
                            ctx,
                            &*self.backend,
                            &[crate::events::Rect {
                                x: 0,
                                y: 0,
                                width,
                                height,
                            }],
                        );
                        self.desktop = Some(desktop);
                    }
                    self.monitor = None;
                    self.last_poll = None;
                    self.relative = self.mailbox.pointer_relative();
                }
                Event::Frame(rects) => {
                    self.damage += rects.len() as u64;
                    if let Some(desktop) = &mut self.desktop {
                        desktop.update(ctx, &*self.backend, &rects);
                    }
                }
                Event::Cursor(shape) => self.set_cursor(ctx, shape),
                Event::PointerMode { relative } => {
                    self.relative = relative;
                    self.escaped = false;
                    log::info!("pointer mode: {}", if relative { "relative" } else { "absolute" });
                }
                Event::Bell => {
                    self.bell_until = Some(Instant::now() + BELL);
                    ctx.request_repaint_after(BELL);
                }
                Event::Clipboard(text) => {
                    // `SessionClipboard.fromRemote`: taken only under the
                    // setting, and what is taken is remembered so it is not
                    // offered straight back.
                    if !self.settings.clipboard_in {
                        log::debug!("clipboard from the far end: {} bytes, not taken", text.len());
                    } else if !text.is_empty() && self.clipboard_seen.as_deref() != Some(text.as_str()) {
                        log::debug!("clipboard from the far end: {} bytes", text.len());
                        host.winit.set_clipboard_text(text.clone());
                        self.clipboard_seen = Some(text);
                    }
                }
                Event::Closed { detail } => {
                    // An ending that was asked for needs no telling, whatever
                    // words the library closed with.
                    self.closed = Some(if self.disconnecting.is_some() { String::new() } else { detail });
                }
                Event::TrustNeeded { fingerprint } => self.trust_needed(fingerprint),
                other => {
                    if let Some(prompt) = Prompt::from_event(&other) {
                        self.prompts.push_back(prompt);
                    }
                }
            }
        }
    }

    fn set_cursor(&mut self, ctx: &egui::Context, shape: Option<Cursor>) {
        self.cursor = shape.map(|c| {
            // Straight alpha, as the seam has it (see `events::Cursor`).
            let image = egui::ColorImage::from_rgba_unmultiplied([c.width, c.height], &c.rgba);
            CursorShape {
                texture: ctx.load_texture("cursor", image, TextureOptions::NEAREST),
                hot: vec2(c.hot_x as f32, c.hot_y as f32),
                size: vec2(c.width as f32, c.height as f32),
                hash: native::shape_hash(&c),
                source: c,
            }
        });
    }

    /// The facts and the info, once a second: the window's title and the
    /// panel come from here, and so does the monitor list.
    fn poll(&mut self, ctx: &egui::Context, host: &mut Host) {
        if !self.connected() {
            return;
        }
        let due = self.last_poll.is_none_or(|t| t.elapsed() + POLL_EARLY >= POLL);
        if !due {
            if let Some(last) = self.last_poll {
                ctx.request_repaint_after(POLL.saturating_sub(last.elapsed()));
            }
            return;
        }
        self.last_poll = Some(Instant::now());
        self.snapshot = Snapshot {
            facts: self.backend.facts(),
            info: self.backend.info(),
            traffic: self.backend.traffic(),
        };
        ctx.request_repaint_after(POLL);
        self.follow_tick(host);
    }

    // ---- the window's size, followed -----------------------------------------

    /// The window changed shape; the follower is asked `DEBOUNCE` later.
    pub fn resized(&mut self) {
        self.follower.resized(Instant::now());
    }

    /// The window's size in desktop pixels: its device pixels, which is
    /// what a desktop of that size is one to one with at 1:1. The toolbar
    /// floats over the desktop rather than taking a strip of it, so the
    /// whole window is the rectangle the desktop is drawn in.
    fn window_size(host: &Host) -> Option<(u32, u32)> {
        let size = host.window.inner_size();
        (size.width > 0 && size.height > 0).then_some((size.width, size.height))
    }

    /// One tick of the follower: on the poll, and after the debounce.
    fn follow_tick(&mut self, host: &mut Host) {
        if !self.connected() {
            return;
        }
        let Some(window) = Self::window_size(host) else {
            return;
        };
        if let Some((width, height)) = self.follower.tick(window, self.snapshot.facts.can_resize) {
            log::info!("asking for a {width}×{height} desktop, this window's size");
            self.backend.request_desktop_size(width, height);
        }
    }

    fn set_follow(&mut self, follow: bool, host: &mut Host) {
        self.follower.set_enabled(follow);
        log::info!("follow window size: {}", if follow { "on" } else { "off" });
        if follow {
            // Asked at the next tick, which is brought forward: turning it
            // on takes effect where it was turned on.
            self.follower.resized(Instant::now());
            self.follow_tick(host);
        }
    }

    /// One option changed on the running session, from the panel or the
    /// toolbar: remembered as the value the controls show, and handed to
    /// the backend. View-only is the one the window has a say in too.
    fn set_option(&mut self, key: &str, value: &str) {
        self.live.set(key, value);
        log::info!("option {key} = {value:?}");
        if key == "ViewOnly" {
            let view_only = self.live.bool(key);
            self.view_only = view_only;
            self.backend.set_view_only(view_only);
            if view_only {
                self.release_keys();
            }
        } else {
            self.backend.set_option(key, value);
        }
    }

    // ---- focus, capture and the clipboard ----------------------------------

    pub fn focus(&mut self, focused: bool, host: &mut Host) {
        self.backend.focus(focused);
        if focused {
            // What was copied here while the window was away, offered once:
            // `SessionClipboard.read`, which remembers what it saw whether
            // or not the setting lets it be sent, so that turning sending
            // on later offers the next copy and not an old one.
            if self.accepts_input()
                && let Some(text) = host.winit.clipboard_text()
                && !text.is_empty()
                && self.clipboard_seen.as_deref() != Some(text.as_str())
            {
                if self.settings.clipboard_out {
                    log::debug!("clipboard to the far end: {} bytes", text.len());
                    self.backend.clipboard(&text);
                } else {
                    log::debug!("clipboard to the far end: {} bytes, not sent", text.len());
                }
                self.clipboard_seen = Some(text);
            }
        } else {
            self.release_keys();
            self.down.clear();
        }
        // The capture follows the focus on the next frame.
        host.window.request_redraw();
    }

    fn release_keys(&mut self) {
        if !self.held.is_empty() {
            self.held.clear();
            self.backend.release_all_keys();
        }
    }

    /// Capture or free the pointer to match what the mode, the focus and
    /// the escape chord say.
    fn sync_capture(&mut self, host: &mut Host) {
        let want = self.relative && host.focused && !self.escaped && self.accepts_input();
        if want == self.captured() {
            return;
        }
        if want {
            match host.platform {
                Platform::X11 => {
                    // winit's X11 `Locked` is unsupported, and `Confined` (an
                    // active pointer grab) makes the X server deliver every
                    // raw motion twice, once for the grab and once for the
                    // root window's selection. So: no grab, the cursor
                    // hidden, raw XI2 deltas, and the pointer warped back to
                    // the centre whenever it moves (the window swallows
                    // those motions; a warp makes no raw motion).
                    self.restore = host.pointer;
                    host.window.set_cursor_visible(false);
                    let _ = host.window.set_cursor_position(platform::centre(host.window));
                    self.capture = Some(Capture::Centred);
                    self.motion_rest = (0.0, 0.0);
                    log::info!("pointer captured: hidden and held at the centre (Ctrl+Alt releases it)");
                }
                _ => match host.window.set_cursor_grab(CursorGrabMode::Locked) {
                    Ok(()) => {
                        host.window.set_cursor_visible(false);
                        self.capture = Some(Capture::Locked);
                        self.motion_rest = (0.0, 0.0);
                        log::info!("pointer locked (Ctrl+Alt releases it)");
                    }
                    Err(e) => log::warn!("pointer lock refused: {e}"),
                },
            }
        } else {
            self.let_go(host);
            if self.captured_mask != 0 {
                self.captured_mask = 0;
                self.backend.pointer_relative(0, 0, 0);
            }
            log::info!("pointer released");
        }
    }

    /// The pointer as it was before the capture: unlocked, shown, and on
    /// X11 back where it was.
    fn let_go(&mut self, host: &mut Host) {
        if self.capture.take() == Some(Capture::Locked) {
            let _ = host.window.set_cursor_grab(CursorGrabMode::None);
        }
        host.window.set_cursor_visible(true);
        if let Some(at) = self.restore.take() {
            let _ = host.window.set_cursor_position(at);
        }
    }

    // ---- input from winit --------------------------------------------------

    /// A key from winit, already known to be ours (see `takes_keys`).
    pub fn key(&mut self, event: &KeyEvent) {
        let physical = event.physical_key;
        match event.state {
            ElementState::Pressed => {
                let keysym = keys::keysym(event);
                if let Some(keysym) = keysym {
                    self.down.insert(physical, keysym);
                }
                // The one chord with a key in it: Ctrl+Alt+D is the overlay,
                // and the D stays here. The chord's keys are let go of at
                // the far end, as the escape does, so they do not stay down
                // there under a key it never saw.
                if chord(&self.down) && is_d(event) {
                    self.toggle_hud();
                    self.release_keys();
                    return;
                }
                if let Some(keysym) = keysym {
                    let id = self.key_ids.id(physical);
                    self.held.insert(physical, keysym);
                    log::debug!("key down {keysym:#x} ({:?}) as {id}", event.logical_key);
                    self.backend.key_down(keysym, id);
                }
                if self.captured() && chord(&self.held) {
                    // Let go of the lock, and of the chord's keys at the far
                    // end, which otherwise stay down there.
                    self.escaped = true;
                    self.release_keys();
                }
            }
            ElementState::Released => {
                self.down.remove(&physical);
                if self.held.remove(&physical).is_some() {
                    self.backend.key_up(self.key_ids.id(physical));
                }
            }
        }
    }

    /// Raw motion while the pointer is locked.
    pub fn mouse_motion(&mut self, dx: f64, dy: f64) {
        if !self.takes_raw_mouse() {
            return;
        }
        let x = self.motion_rest.0 + dx;
        let y = self.motion_rest.1 + dy;
        let (ix, iy) = (x.trunc(), y.trunc());
        self.motion_rest = (x - ix, y - iy);
        if ix != 0.0 || iy != 0.0 {
            self.motion_sent.0 += ix as i64;
            self.motion_sent.1 += iy as i64;
            self.backend.pointer_relative(ix as i32, iy as i32, self.captured_mask);
        }
    }

    /// A button while the pointer is locked.
    pub fn mouse_button(&mut self, button: MouseButton, state: ElementState) {
        if !self.takes_raw_mouse() {
            return;
        }
        let bit = match button {
            MouseButton::Left => LEFT,
            MouseButton::Middle => MIDDLE,
            MouseButton::Right => RIGHT,
            _ => return,
        };
        self.captured_mask = match state {
            ElementState::Pressed => self.captured_mask | bit,
            ElementState::Released => self.captured_mask & !bit,
        };
        self.backend.pointer_relative(0, 0, self.captured_mask);
    }

    /// The wheel while the pointer is locked.
    pub fn mouse_wheel(&mut self, delta: MouseScrollDelta) {
        if !self.takes_raw_mouse() {
            return;
        }
        let notches = match delta {
            MouseScrollDelta::LineDelta(x, y) => vec2(x, y),
            MouseScrollDelta::PixelDelta(p) => vec2(p.x as f32, p.y as f32) / NOTCH_POINTS,
        };
        for bit in self.notches(notches) {
            self.backend.pointer_relative(0, 0, self.captured_mask | bit);
            self.backend.pointer_relative(0, 0, self.captured_mask);
        }
    }

    /// Whole notches out of `delta`, keeping the rest for next time: winit's
    /// and egui's sign, positive meaning the content moves down or right,
    /// which is the wheel rolling up or left.
    fn notches(&mut self, delta: Vec2) -> Vec<i32> {
        self.wheel += delta;
        let mut out = Vec::new();
        while self.wheel.y >= 1.0 {
            self.wheel.y -= 1.0;
            out.push(WHEEL_UP);
        }
        while self.wheel.y <= -1.0 {
            self.wheel.y += 1.0;
            out.push(WHEEL_DOWN);
        }
        while self.wheel.x >= 1.0 {
            self.wheel.x -= 1.0;
            out.push(WHEEL_LEFT);
        }
        while self.wheel.x <= -1.0 {
            self.wheel.x += 1.0;
            out.push(WHEEL_RIGHT);
        }
        out
    }

    // ---- the frame ---------------------------------------------------------

    /// One option of the client's own changed on the panel.
    fn set_settings(&mut self, settings: Settings) {
        if settings == self.settings {
            return;
        }
        log::info!(
            "settings: send clipboard {}, receive clipboard {}, native cursor {}",
            on_off(settings.clipboard_out),
            on_off(settings.clipboard_in),
            on_off(settings.native_cursor)
        );
        self.settings = settings;
    }

    fn toggle_hud(&mut self) {
        self.hud.on = !self.hud.on;
        log::info!("debug overlay: {}", on_off(self.hud.on));
    }

    // ---- the frame ---------------------------------------------------------

    /// One frame: the events, the poll, the desktop, the chrome.
    pub fn frame(&mut self, root: &mut egui::Ui, host: &mut Host) {
        let ctx = &root.ctx().clone();
        self.drain(ctx, host);
        self.poll(ctx, host);
        if let Some(due) = self.follower.due() {
            let now = Instant::now();
            if due <= now {
                self.follow_tick(host);
            } else {
                ctx.request_repaint_after(due - now);
            }
        }
        if let Some(since) = self.disconnecting
            && self.closed.is_none()
        {
            match DISCONNECT_PATIENCE.checked_sub(since.elapsed()) {
                // A frame at the deadline, asked for here because nothing
                // else may ask: a session that never connected has no poll
                // ticking, and a backend that will not close sends nothing.
                // Without it "Disconnecting…" stayed up until the pointer
                // moved.
                Some(left) if !left.is_zero() => ctx.request_repaint_after(left),
                _ => {
                    log::warn!("no close within {DISCONNECT_PATIENCE:?} of the disconnect; giving up on it");
                    self.closed = Some(String::new());
                }
            }
        }
        self.sync_capture(host);

        let title = self.title();
        if title != self.title_set(ctx) {
            host.window.set_title(&title);
            ctx.data_mut(|d| d.insert_temp(Id::new("title"), title));
        }

        let pixels_per_point = ctx.pixels_per_point();
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let mut layout = None;
        let mut response = None;
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(Color32::from_gray(24)))
            .show(root, |ui| {
                let window = ui.max_rect();
                let Some((width, height)) = self.desktop.as_ref().map(Desktop::size) else {
                    self.waiting(ui);
                    return;
                };
                let source = self.source_rect(width, height);
                let l = self.view.layout(source, window, pixels_per_point);
                let exact = (l.scale * pixels_per_point - 1.0).abs() < 1e-3;
                let desktop = self.desktop.as_mut().expect("checked above");
                desktop.set_exact(ctx, &*self.backend, exact);
                desktop.paint(ui.painter(), &l);
                response = Some(ui.interact(window, Id::new("desktop"), Sense::click_and_drag()));
                layout = Some(l);
            });
        self.hovered = response.as_ref().is_some_and(Response::hovered);
        self.native_key = None;
        if let (Some(layout), Some(response)) = (layout, response) {
            self.pointer(ctx, &response, &layout, dt);
            if self.settings.native_cursor {
                // Device pixels per desktop pixel: what the shape is
                // resampled by so that its hotspot lands where the pointer is.
                let factor = layout.scale * pixels_per_point;
                self.native_key = (self.hovered && !self.relative && self.connected())
                    .then(|| self.cursor.as_ref().map(|c| Key::new(c.hash, factor)))
                    .flatten();
            } else {
                self.draw_cursor(ctx, &response);
            }
        }

        self.toolbar(ctx, host);
        if self.panel.open {
            let state = self.state_line();
            let title = self.request.address.clone();
            let controls = Controls {
                specs: self.request.kind.options,
                values: &self.live,
                connected: self.connected(),
                window: Self::window_size(host),
                follow: self.follower.enabled(),
                settings: self.settings,
            };
            let actions = self.panel.show(ctx, &self.snapshot, &controls, &title, &state);
            for action in actions {
                match action {
                    Action::SetOption(key, value) => self.set_option(key, &value),
                    Action::DesktopSize(width, height) => {
                        log::info!("asking for a {width}×{height} desktop");
                        self.backend.request_desktop_size(width, height);
                    }
                    Action::Display(index) => self.backend.request_display(index),
                    Action::Follow(follow) => self.set_follow(follow, host),
                    Action::Settings(settings) => self.set_settings(settings),
                }
            }
        }
        if self.hud.on {
            let lines = self.hud_lines(host, layout.as_ref(), pixels_per_point);
            Hud::draw(ctx, &lines);
        }
        self.prompt(ctx);
    }

    /// Before egui-winit sets the window's cursor from egui's icon: the
    /// far end's shape taken off it where it is no longer wanted.
    pub fn cursor_before(&mut self, host: &mut Host) {
        self.native.before(host.window, self.native_key);
    }

    /// After egui-winit's turn: the far end's shape put on, where wanted.
    pub fn cursor_after(&mut self, host: &mut Host, icon: CursorIcon) {
        let wanted = match (self.native_key, &self.cursor) {
            (Some(key), Some(shape)) => Some((key, &shape.source)),
            _ => None,
        };
        let alpha = native::Alpha::for_platform(host.platform);
        self.native.after(host.window, host.event_loop, wanted, icon, alpha);
    }

    // ---- the debug overlay -------------------------------------------------

    /// `SessionHud.draw`'s lines, in its order where they apply here: the
    /// state and the pixel path, the cursor and the view, the physical
    /// mouse and keyboard, and the things this window has that the phone
    /// has not.
    fn hud_lines(&mut self, host: &Host, layout: Option<&Layout>, pixels_per_point: f32) -> Vec<String> {
        let now = Instant::now();
        self.hud.frames += 1;
        let fps = self.hud.fps.sample(self.hud.frames, now);
        let damage_rate = self.hud.damage.sample(self.damage, now);
        let (uploads, bytes, last) = self.desktop.as_ref().map(Desktop::stats).unwrap_or((0, 0, None));
        let per_frame = uploads.saturating_sub(self.uploads_seen);
        self.uploads_seen = uploads;
        let read_rate = self.hud.bytes.sample(bytes, now);
        let (width, height) = self.desktop.as_ref().map(Desktop::size).unwrap_or((0, 0));
        // `SessionClipboard.summary`: what the far end would be given.
        let clip = match &self.clipboard_seen {
            Some(text) if self.settings.clipboard_out => format!("{}c", text.chars().count()),
            _ => "-".to_string(),
        };
        let state = format!("{:?}", self.mailbox.state()).to_ascii_lowercase();
        let first = format!(
            "state {state}  desktop {width}x{height}  fps {fps}  dmg {} ({damage_rate}/s)  tex {per_frame}/frame  rd {}  last {}  clip {clip}",
            self.damage,
            hud::bytes_per_second(read_rate),
            last.map(|r| format!("{},{} {}x{}", r.x, r.y, r.width, r.height))
                .unwrap_or_else(|| "-".to_string()),
        );

        let (x, y, mask) = self.last_pointer;
        let cursor = if self.relative {
            "theirs".to_string()
        } else {
            format!("{x},{y}")
        };
        let view = match layout {
            Some(l) => {
                let origin = (l.rect.min - l.window.min) * pixels_per_point;
                let content = l.rect.size() * pixels_per_point;
                format!(
                    "scale {:.3} [{}]  origin {},{}  content {}x{}",
                    l.scale * pixels_per_point,
                    self.view.mode.label(),
                    origin.x.round(),
                    origin.y.round(),
                    content.x.round(),
                    content.y.round()
                )
            }
            None => "scale -".to_string(),
        };
        let second = format!("cursor {cursor}  btn {}  {view}", hud::buttons_name(mask));

        let mouse = match self.capture {
            Some(Capture::Locked) => "captured (locked)",
            Some(Capture::Centred) => "captured (centred)",
            None if self.hovered => "hover",
            None => "-",
        };
        // Every whole pixel of raw motion sent so far in relative mode, so
        // that motion counted twice shows as twice the distance.
        let sent = if self.relative {
            format!("  Δ {},{}", self.motion_sent.0, self.motion_sent.1)
        } else {
            String::new()
        };
        let third = format!(
            "mouse {mouse}{sent}  btn {}  keys {} held{}",
            hud::buttons_name(if self.captured() { self.captured_mask } else { mask }),
            self.held.len(),
            if self.view_only { "   VIEW ONLY" } else { "" }
        );

        let follow = if !self.follower.enabled() {
            "off".to_string()
        } else {
            match self.follower.followed() {
                Some((w, h)) => format!("{w}x{h}"),
                None => "waiting".to_string(),
            }
        };
        let shape = match &self.cursor {
            None => "none".to_string(),
            Some(c) => {
                let how = if self.native.shown().is_some() {
                    "native"
                } else if self.settings.native_cursor {
                    "native (off desktop)"
                } else {
                    "drawn"
                };
                format!("{how} {}x{} hot {},{}", c.source.width, c.source.height, c.source.hot_x, c.source.hot_y)
            }
        };
        let fourth = format!(
            "{} inhibitor {}  follow {follow}  shape {shape}  clipboard out {} in {}",
            host.platform.name(),
            host.inhibitor,
            on_off(self.settings.clipboard_out),
            on_off(self.settings.clipboard_in)
        );
        vec![first, second, third, fourth]
    }

    fn title_set(&self, ctx: &egui::Context) -> String {
        ctx.data(|d| d.get_temp::<String>(Id::new("title"))).unwrap_or_default()
    }

    /// What the panel says under the name: the state, which client, the
    /// desktop.
    fn state_line(&self) -> String {
        let backend = self.request.kind.name;
        if self.closed.is_some() {
            return "Disconnected".to_string();
        }
        if !self.connected() {
            return format!("Connecting · {backend}");
        }
        let facts = &self.snapshot.facts;
        if facts.width == 0 {
            return format!("Connected · {backend}");
        }
        let monitors = facts.monitors.len();
        if monitors > 1 {
            format!(
                "Connected · {backend} · {} × {} · {monitors} monitors",
                facts.width, facts.height
            )
        } else {
            format!("Connected · {backend} · {} × {}", facts.width, facts.height)
        }
    }

    /// Before there is a desktop: what is being waited for, and a way out.
    fn waiting(&mut self, ui: &mut egui::Ui) {
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 2.0 - 40.0);
                if self.disconnecting.is_some() {
                    ui.label("Disconnecting…");
                } else {
                    ui.label(format!("Connecting to {}…", self.request.address));
                    ui.add_space(8.0);
                    if ui.button("Cancel").clicked() {
                        self.disconnect();
                    }
                }
            });
        });
    }

    /// The desktop pixels on screen: one monitor's, or all of them.
    fn source_rect(&mut self, width: usize, height: usize) -> Rect {
        let whole = Rect::from_min_size(Pos2::ZERO, vec2(width as f32, height as f32));
        let Some(i) = self.monitor else {
            return whole;
        };
        match self.snapshot.facts.monitors.get(i) {
            Some(m) => {
                let r = Rect::from_min_size(pos2(m.x as f32, m.y as f32), vec2(m.width as f32, m.height as f32))
                    .intersect(whole);
                if r.width() >= 1.0 && r.height() >= 1.0 {
                    r
                } else {
                    whole
                }
            }
            None => {
                self.monitor = None;
                whole
            }
        }
    }

    /// The pointer over the desktop: the edge pan, and in absolute mode the
    /// position, the buttons and the wheel.
    fn pointer(&mut self, ctx: &egui::Context, response: &Response, layout: &Layout, dt: f32) {
        let over = response.hovered() || response.dragged() || response.is_pointer_button_down_on();
        let pos = ctx.input(|i| i.pointer.latest_pos());

        if over && !self.captured() && let Some(pos) = pos {
            let velocity = layout.edge_velocity(pos);
            if velocity != Vec2::ZERO {
                self.view.pan += velocity * dt;
                ctx.request_repaint();
            }
        }

        if response.clicked() || response.drag_started() {
            // A click on the desktop takes the pointer back after the chord.
            self.escaped = false;
        }
        if !self.accepts_input() || self.relative {
            return;
        }

        // The frame's events in order rather than the state at its end,
        // so that a press and a release in the same frame are both sent,
        // and in that order.
        let events = ctx.input(|i| i.events.clone());
        let (_, _, mut mask) = self.last_pointer;
        if over && let Some(pos) = pos {
            let mut at = layout.desktop_at(pos);
            for event in &events {
                match event {
                    egui::Event::PointerMoved(p) => at = layout.desktop_at(*p),
                    egui::Event::PointerButton {
                        pos: p,
                        button,
                        pressed,
                        ..
                    } => {
                        let bit = match button {
                            PointerButton::Primary => LEFT,
                            PointerButton::Middle => MIDDLE,
                            PointerButton::Secondary => RIGHT,
                            _ => continue,
                        };
                        at = layout.desktop_at(*p);
                        mask = if *pressed { mask | bit } else { mask & !bit };
                    }
                    egui::Event::MouseWheel { unit, delta, .. } => {
                        let notches = match unit {
                            egui::MouseWheelUnit::Line => *delta,
                            egui::MouseWheelUnit::Point => *delta / NOTCH_POINTS,
                            egui::MouseWheelUnit::Page => *delta,
                        };
                        for bit in self.notches(notches) {
                            self.backend.pointer(at.0, at.1, mask | bit);
                            self.backend.pointer(at.0, at.1, mask);
                        }
                        continue;
                    }
                    _ => continue,
                }
                self.send_pointer(at, mask);
            }
            self.send_pointer(at, mask);
        } else if mask != 0 {
            // A button pressed over the desktop and let go elsewhere: the
            // far end is told at the last place it saw.
            let released = events
                .iter()
                .any(|e| matches!(e, egui::Event::PointerButton { pressed: false, .. }));
            if released {
                let (x, y, _) = self.last_pointer;
                self.send_pointer((x, y), 0);
            }
        }
    }

    /// One pointer event, unless it is the one already sent.
    fn send_pointer(&mut self, at: (i32, i32), mask: i32) {
        if (at.0, at.1, mask) != self.last_pointer {
            self.backend.pointer(at.0, at.1, mask);
            self.last_pointer = (at.0, at.1, mask);
        }
    }

    /// The far end's cursor under the local pointer, in absolute mode.
    fn draw_cursor(&self, ctx: &egui::Context, response: &Response) {
        if self.relative || !self.connected() {
            return;
        }
        let Some(cursor) = &self.cursor else {
            return;
        };
        if !response.hovered() {
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        ctx.set_cursor_icon(CursorIcon::None);
        let rect = Rect::from_min_size(pos - cursor.hot, cursor.size);
        ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("remote-cursor")))
            .image(
                cursor.texture.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
    }

    // ---- the toolbar -------------------------------------------------------

    /// Slid in from the top edge while the pointer is there or on it, and
    /// away otherwise.
    fn toolbar(&mut self, ctx: &egui::Context, host: &mut Host) {
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let screen = ctx.content_rect();
        let at_edge = pointer.is_some_and(|p| screen.contains(p) && p.y <= screen.min.y + 2.0);
        let reveal = self.started.elapsed() < TOOLBAR_REVEAL;
        let show = at_edge || self.toolbar_hovered || reveal || self.disconnecting.is_some();
        let hidden = -(self.toolbar_height + 12.0);
        let y = ctx.animate_value_with_time(Id::new("toolbar-y"), if show { 6.0 } else { hidden }, 0.2);
        if reveal {
            ctx.request_repaint_after(TOOLBAR_REVEAL.saturating_sub(self.started.elapsed()));
        }

        let area = egui::Area::new(Id::new("toolbar"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_TOP, vec2(0.0, y))
            // Unconstrained, or egui would keep it on screen: off the top
            // is where it goes to hide.
            .constrain(false)
            .interactable(true)
            .show(ctx, |ui| {
                Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| self.toolbar_items(ui, host));
                });
            });
        self.toolbar_height = area.response.rect.height();
        self.toolbar_hovered = area.response.contains_pointer();
    }

    fn toolbar_items(&mut self, ui: &mut egui::Ui, host: &mut Host) {
        let connected = self.connected();
        let scale = ui.add_enabled(connected, egui::Button::new(format!("Scale: {}", self.view.mode.label())));
        if scale
            .on_hover_text(
                "Cycles fit width, fit height, fill width, fill height and 1:1.\n\
                 Where the desktop overflows the window, the pointer near an edge pans it.",
            )
            .clicked()
        {
            self.view.cycle();
        }

        let facts = &self.snapshot.facts;
        if facts.monitors.len() > 1 {
            let label = match self.monitor {
                None => "Monitor: all".to_string(),
                Some(i) => format!("Monitor: {}", i + 1),
            };
            if ui
                .add_enabled(connected, egui::Button::new(label))
                .on_hover_text("Show the whole desktop, or one of its monitors")
                .clicked()
            {
                self.monitor = match self.monitor {
                    None => Some(0),
                    Some(i) if i + 1 < facts.monitors.len() => Some(i + 1),
                    Some(_) => None,
                };
                self.view.pan = Vec2::ZERO;
            }
        } else if facts.displays.len() > 1 {
            let label = format!("Display: {} of {}", facts.display + 1, facts.displays.len());
            if ui
                .add_enabled(connected, egui::Button::new(label))
                .on_hover_text("Ask the far end for its next display")
                .clicked()
            {
                let next = (facts.display + 1).rem_euclid(facts.displays.len() as i32);
                self.backend.request_display(next);
            }
        }

        let view_only = self.view_only;
        if ui
            .add_enabled(connected, egui::Button::new(if view_only { "View only: on" } else { "View only: off" }))
            .on_hover_text("Watch without sending the mouse, the keyboard or the clipboard")
            .clicked()
        {
            self.set_option("ViewOnly", if view_only { "false" } else { "true" });
        }

        if ui
            .add_enabled(connected, egui::Button::new("Info"))
            .on_hover_text("The connection's facts, and the settings it can still be told")
            .clicked()
        {
            self.panel.open = !self.panel.open;
        }

        if ui
            .button(if self.hud.on { "Debug: on" } else { "Debug" })
            .on_hover_text(
                "The debug overlay: the state, the pixel path, the cursor, the view and the input, in a                  corner. Ctrl+Alt+D toggles it too.",
            )
            .clicked()
        {
            self.toggle_hud();
        }

        let fullscreen = host.window.fullscreen().is_some();
        if ui
            .button(if fullscreen { "Leave fullscreen" } else { "Fullscreen" })
            .on_hover_text(if fullscreen {
                "Back to a window"
            } else {
                "Fill the screen. The toolbar still slides in at the top edge; its button, or \
                 disconnecting, leaves fullscreen."
            })
            .clicked()
        {
            log::info!("{}", if fullscreen { "leaving fullscreen" } else { "fullscreen" });
            host.window
                .set_fullscreen((!fullscreen).then_some(Fullscreen::Borderless(None)));
        }

        if ui
            .add_enabled(self.disconnecting.is_none() && self.closed.is_none(), egui::Button::new("Disconnect"))
            .on_hover_text(if self.relative {
                "End the session. Ctrl+Alt held together frees the locked pointer."
            } else {
                "End the session"
            })
            .clicked()
        {
            self.disconnect();
        }
    }

    pub fn disconnect(&mut self) {
        if self.disconnecting.is_none() {
            self.disconnecting = Some(Instant::now());
            self.release_keys();
            self.backend.disconnect();
        }
    }

    // ---- the prompts -------------------------------------------------------

    /// A certificate, against what this address was accepted with before:
    /// what each Java backend's `onTrustNeeded` does. A match is silent, a
    /// RustDesk key never seen is pinned without asking, and anything else
    /// is put to the person with why.
    fn trust_needed(&mut self, fingerprint: String) {
        let key = &self.pinned.key;
        match self.known_hosts.verdict(key, &fingerprint) {
            Verdict::Matches => {
                log::info!("certificate matches the pin for {key}");
                self.backend.answer_trust(true);
            }
            // Pinned under this key is the one case that is still asked.
            Verdict::Ask { identity, .. } if self.pinned.first_sight_silent && identity != Identity::Changed => {
                self.pin(&fingerprint);
                self.backend.answer_trust(true);
            }
            Verdict::Ask {
                identity,
                matching_address,
            } => {
                log::info!("asking about the certificate for {key}: {identity:?}");
                self.prompts.push_back(Prompt::Trust {
                    fingerprint,
                    identity,
                    matching_address,
                });
            }
        }
    }

    /// A store that cannot be written costs the next connection a question,
    /// not this one its answer, so a failure is a line in the log.
    fn pin(&self, fingerprint: &str) {
        let key = &self.pinned.key;
        match self.known_hosts.pin(key, fingerprint) {
            Ok(()) => log::info!("pinned {key}"),
            Err(e) => log::warn!("could not pin {key} in {}: {e}", self.known_hosts.path().display()),
        }
    }

    fn prompt(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.prompts.front().cloned() else {
            return;
        };
        let answer = prompts::show(ctx, &prompt, &mut self.form, &self.request.address, &self.request.user);
        if let Some(answer) = answer {
            self.prompts.pop_front();
            match answer {
                Answer::Credentials(credentials) => self.backend.answer_credentials(credentials),
                Answer::Trust(accept) => self.backend.answer_trust(accept),
                Answer::Pin {
                    accept,
                    remember,
                    fingerprint,
                } => {
                    // `KnownHosts.ask`'s deliver: pinned only when both.
                    if accept && remember {
                        self.pin(&fingerprint);
                    }
                    self.backend.answer_trust(accept);
                }
                Answer::Security { accept, remember } => self.backend.answer_security(accept, remember),
            }
        }
    }

    // ---- the end -----------------------------------------------------------

    /// Ends the session and releases the texture, and starts releasing the
    /// backend: what comes back is its `destroy`, under way on a thread of
    /// its own.
    pub fn shutdown(mut self, ctx: &egui::Context, host: &mut Host) -> Teardown {
        self.release_keys();
        self.native.reset(host.window);
        if self.captured() {
            self.let_go(host);
        }
        // The screen was the session's; the dialog gets a window.
        if host.window.fullscreen().is_some() {
            host.window.set_fullscreen(None);
        }
        if let Some(desktop) = self.desktop.take() {
            desktop.free(ctx);
        }
        if self.closed.is_none() {
            self.backend.disconnect();
        }
        // `RealVncBackend.saveIdentity`: kept by the address as written, for
        // the next session to hand back.
        if let Some(identity) = self.backend.saved_identity() {
            let address = &self.request.address;
            match self.identities.set(address, &identity) {
                Ok(()) => log::info!("remembered RealVNC's identity for {address}"),
                Err(e) => log::warn!("could not keep the identity for {address} in {}: {e}", self.identities.path().display()),
            }
        }
        Teardown::start(self.backend)
    }
}

/// A backend's `destroy`, on a thread of its own.
///
/// `destroy` joins the protocol thread, so it is as quick as the backend is
/// to let go — and one that is not, still dialling or stuck in a handshake
/// or simply past `DISCONNECT_PATIENCE` without a close, held the window's
/// thread for as long: the window would not close, nor the dialog come
/// back. Every backend now ends its dial when asked, so this is the second
/// line rather than the first; the window keeps the handle so that it can
/// give whatever is still going a moment before the process exits.
pub struct Teardown(Option<std::thread::JoinHandle<()>>);

impl Teardown {
    fn start(backend: Arc<dyn Backend>) -> Teardown {
        let destroying = Arc::clone(&backend);
        match std::thread::Builder::new()
            .name("destroy".into())
            .spawn(move || destroying.destroy())
        {
            Ok(thread) => Teardown(Some(thread)),
            Err(e) => {
                log::warn!("could not start a thread to destroy the backend on ({e}); destroying it here");
                backend.destroy();
                Teardown(None)
            }
        }
    }

    pub fn is_finished(&self) -> bool {
        self.0.as_ref().is_none_or(|thread| thread.is_finished())
    }

    /// Waits for it until `deadline` at most; whether it finished.
    pub fn wait_until(self, deadline: Instant) -> bool {
        while !self.is_finished() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(thread) = self.0 {
            let _ = thread.join();
        }
        true
    }
}

/// Ctrl and Alt, either side, among `keys` together. By keysym rather than
/// by key code, so that a keyboard whose codes are not the usual ones (a
/// virtual one, say) still has the chord.
fn chord(keys: &HashMap<PhysicalKey, u32>) -> bool {
    let down = |a: Keysym, b: Keysym| keys.values().any(|&k| k == a.raw() || k == b.raw());
    down(Keysym::Control_L, Keysym::Control_R) && down(Keysym::Alt_L, Keysym::Alt_R)
}

/// The D of Ctrl+Alt+D: the layout's, as every key is.
fn is_d(event: &KeyEvent) -> bool {
    matches!(&event.logical_key, LogicalKey::Character(c) if c.eq_ignore_ascii_case("d"))
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}
