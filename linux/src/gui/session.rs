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
//! absolute mode the far end sends its cursor's shape and this makes it the
//! window's own pointer (`cursor.rs`) — or, with the software cursor
//! setting, draws it under the local pointer, hiding the OS cursor over the
//! desktop, as the Android app does. In relative mode the far end draws
//! its own cursor into the picture and owns the motion, so the local
//! pointer is captured and hidden while the window has focus and its raw
//! motion is sent as deltas; Ctrl+Alt held together lets it go, and a click
//! on the desktop takes it back. On Wayland the capture is a pointer lock;
//! on X11, which has none, the pointer is hidden, warped back to the
//! window's centre whenever it moves, and fenced in by pointer barriers on
//! the window's edges meanwhile (see `sync_capture`).
//!
//! The clipboard crosses in both directions under the two settings the
//! phone has (`settings.rs`), with its rule: the last text seen crossing
//! either way is remembered, and is never sent back where it came from.
//!
//! The debug overlay (`hud.rs`) is the phone's `SessionHud`: every
//! collaborator's state, in a corner, from the connection panel or
//! Ctrl+Alt+D.

use super::cursor::{self as native, Key, Native};
use super::desktop::Desktop;
use super::follow::Follower;
use super::hud::{self, Hud};

use super::keys::{self, KeyIds};
use super::panel::{Action, Controls, Panel, Snapshot};
use super::platform::{self, Platform};
use super::prompts::{self, Answer, Form, Prompt};
use super::settings::Settings;
use super::toolbar::{self, Placement};
use super::view::{Layout, ScaleMode, View};
use super::x11::Fence;
use super::{Host, Request};
use crate::backend::Backend;
use crate::backends::{self, Connect, Dirs, Pinned};
use crate::events::{Cursor, Event, Identity, Mailbox, State};
use crate::known_hosts::{self, KnownHosts, Store, Verdict};
use crate::options::Options;
use egui::{
    Color32, CursorIcon, Frame, Id, LayerId, Margin, Order, PointerButton, Pos2, Rect, Response, RichText, Sense,
    Stroke, TextureHandle, TextureOptions, Vec2, pos2, vec2,
};
use super::icons;
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
/// The toolbar's buttons' height, in points.
const TOOLBAR_BUTTON: f32 = 28.0;
/// The size of the toolbar's icons, in points: Material Symbols are drawn
/// for 24, and read well down to 18.
const TOOLBAR_ICON: f32 = 20.0;
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
    /// X11: hidden, no grab, fenced, and warped back to the window's centre.
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
    /// Where the toolbar sits along the top edge: where the last session
    /// left it, then wherever it is dragged. Handed back, like `settings`.
    pub placement: Placement,
    /// Where the toolbar's left edge was when the drag under way began.
    drag_from: Option<f32>,
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
    /// The toolbar's size as last drawn, which is where it would be when
    /// out, and so the span of the edge that brings it out.
    toolbar_size: Vec2,
    /// The pointer is on the toolbar: it stays out, and the keyboard is
    /// this end's meanwhile.
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
    /// end's: the software cursor setting off, absolute mode, the pointer
    /// over the desktop, and a shape to show.
    native_key: Option<Key>,
    /// What the far end said about who owns the pointer.
    relative: bool,
    /// How the local pointer is held right now, if it is.
    capture: Option<Capture>,
    /// X11: where the pointer was before it was captured, put back when it
    /// is let go.
    restore: Option<PhysicalPosition<f64>>,
    /// X11: the barriers that keep a captured pointer on the window, made
    /// at the first capture and up while it lasts.
    fence: Option<Fence>,
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
    /// Whether the backend was last told the window is on screen; it starts
    /// out so.
    shown: bool,
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
    /// When the oldest frame taken but not yet presented ended at the
    /// backend: the start of the overlay's render latency.
    unpresented: Option<Instant>,
    /// When winit handed over the first pointer event egui has not yet
    /// read, which the next frame turns into the pointer event the backend
    /// is given: the start of the overlay's pointer latency.
    pointer_since: Option<Instant>,
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
        placement: Placement,
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
        let backend = request.kind.start(&connect)?;
        let title = request.address.clone();
        let live = request.options.clone();
        // What the backend was created with, which it already acts on: the
        // toolbar and the window's own idea of where input goes start from
        // it rather than from off, which had `-o ViewOnly=true` showing
        // "View only: off" while the window forwarded keys the backend
        // dropped, and held the keyboard grab for them.
        let view_only = live.bool("ViewOnly");
        let pinned = backends::pinned_as(request.kind, &request.address, &request.options);
        Ok(Session {
            request,
            settings,
            placement,
            drag_from: None,
            backend,
            mailbox,
            started: Instant::now(),
            desktop: None,
            snapshot: Snapshot::default(),
            last_poll: None,
            view: View::default(),
            monitor: None,
            toolbar_size: vec2(0.0, 40.0),
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
            fence: None,
            motion_sent: (0, 0),
            escaped: false,
            key_ids: KeyIds::default(),
            held: HashMap::new(),
            down: HashMap::new(),
            last_pointer: (-1, -1, 0),
            wheel: Vec2::ZERO,
            motion_rest: (0.0, 0.0),
            captured_mask: 0,
            view_only,
            shown: true,
            clipboard_seen: None,
            bell_until: None,
            title,
            disconnecting: None,
            hud: Hud::default(),
            damage: 0,
            uploads_seen: 0,
            hovered: false,
            unpresented: None,
            pointer_since: None,
            closed: None,
        })
    }

    pub fn connected(&self) -> bool {
        self.mailbox.state() == State::Connected && self.closed.is_none()
    }

    /// Whether input has somewhere to go. Also what the window holds the
    /// compositor's or the window manager's shortcuts off for: while it is
    /// not, there is nowhere at the far end for a chord to go, and it is
    /// the window manager's.
    pub fn accepts_input(&self) -> bool {
        self.connected() && !self.view_only && self.prompts.is_empty() && self.disconnecting.is_none()
    }

    /// Whether the window should hold the compositor's or the window
    /// manager's shortcuts off: while input has somewhere to go, and not
    /// while the pointer is on the toolbar. The toolbar is this end's, so
    /// while it is being used the keyboard is too — Alt+Tab included, which
    /// is how a grabbed window is otherwise left only with the mouse.
    /// Never with the grab keyboard setting off.
    pub fn holds_shortcuts(&self) -> bool {
        self.settings.grab_keyboard && self.accepts_input() && !self.toolbar_hovered
    }

    /// Whether a key event from winit is the far end's rather than egui's:
    /// not while the pointer is on the toolbar, where the shortcuts are
    /// this desktop's again and a key typed is not meant for the far end.
    pub fn takes_keys(&self, ctx: &egui::Context) -> bool {
        self.accepts_input() && !self.toolbar_hovered && !ctx.egui_wants_keyboard_input()
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
                Event::Frame { rects, ended } => {
                    self.damage += rects.len() as u64;
                    self.unpresented = Some(self.unpresented.map_or(ended, |t| t.min(ended)));
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
                        // egui's own as well, for a paste into a text field.
                        host.winit.set_clipboard_text(text.clone());
                        if let Some(clipboard) = host.clipboard {
                            clipboard.set(text.clone());
                        }
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
        self.refence();
    }

    /// X11: the window moved or changed size under a captured pointer, and
    /// the barriers go where it is now.
    pub fn refence(&mut self) {
        if self.capture == Some(Capture::Centred)
            && let Some(fence) = &mut self.fence
        {
            fence.raise();
        }
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
        // A switch that went off takes the choices that need it with it,
        // here as in the dialog, so the panel never shows (and the backend
        // never holds) an encoding that cannot be negotiated.
        for key in self.live.settle(self.request.kind.options) {
            log::info!("option {key} = {:?}, its switch being off", self.live.get(key));
            self.backend.set_option(key, self.live.get(key));
        }
    }

    // ---- focus, capture and the clipboard ----------------------------------

    /// Whether the window can be seen: the backend's `focus`, which is what
    /// lets it pause. Not the keyboard focus, which it was: a window left
    /// visible beside the one being typed in froze, since the RFB backends
    /// stop reading, FreeRDP suppresses output and RustDesk drops to a frame
    /// a second, all for a picture still on screen. The window says when it
    /// is minimised or covered (see `App::sync_shown`).
    pub fn shown(&mut self, shown: bool) {
        if shown != self.shown {
            self.shown = shown;
            log::info!("{}", if shown { "window shown: updates resume" } else { "window hidden: updates may pause" });
            self.backend.focus(shown);
        }
    }

    /// The keyboard focus: the keys still down let go of when it leaves,
    /// and the capture and the shortcuts (the window's) following it. The
    /// clipboard is read when it comes back, by the window, and offered
    /// when the read is in (`local_clipboard`).
    pub fn focus(&mut self, focused: bool, host: &mut Host) {
        if !focused {
            self.release_keys();
            self.down.clear();
        }
        // The capture follows the focus on the next frame.
        host.window.request_redraw();
    }

    /// What was copied here while the window was away, read when it got the
    /// focus back, offered once: `SessionClipboard.read`, which remembers
    /// what it saw whether or not the setting lets it be sent, so that
    /// turning sending on later offers the next copy and not an old one.
    /// `None` is a clipboard with no text, or one that could not be read in
    /// time (see `clipboard/`).
    pub fn local_clipboard(&mut self, text: Option<String>) {
        if let Some(text) = text
            && self.accepts_input()
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
                    // those motions; a warp makes no raw motion), with
                    // barriers on the window's edges so that it cannot be
                    // anywhere else before the warp (see `x11::Fence`).
                    self.restore = host.pointer;
                    host.window.set_cursor_visible(false);
                    let _ = host.window.set_cursor_position(platform::centre(host.window));
                    if self.fence.is_none() {
                        self.fence = Fence::new(host.window);
                    }
                    if let Some(fence) = &mut self.fence {
                        fence.raise();
                    }
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
        match self.capture.take() {
            Some(Capture::Locked) => {
                let _ = host.window.set_cursor_grab(CursorGrabMode::None);
            }
            Some(Capture::Centred) => {
                if let Some(fence) = &mut self.fence {
                    fence.lower();
                }
            }
            None => {}
        }
        host.window.set_cursor_visible(true);
        if let Some(at) = self.restore.take() {
            let _ = host.window.set_cursor_position(at);
        }
    }

    // ---- input from winit --------------------------------------------------

    /// A pointer event from winit, now, that egui will read in the next
    /// frame: the time of the first since the last frame is kept, for the
    /// overlay. One clock read a frame, whether the overlay is on or not.
    pub fn pointer_event(&mut self) {
        self.pointer_since.get_or_insert_with(Instant::now);
    }

    /// The frame just drawn went to the screen at `at`, or was not drawn
    /// (`None`: the window is minimised, or its surface had to be made
    /// again), in which case nothing is measured: a picture nobody could
    /// see has no latency worth the name, and the next one will be timed
    /// from its own frame.
    pub fn presented(&mut self, at: Option<Instant>) {
        if let (Some(ended), Some(at)) = (self.unpresented.take(), at) {
            self.hud.render.record(at.saturating_duration_since(ended));
        }
    }

    /// A key from winit, already known to be ours (see `takes_keys`),
    /// handed over at `since`.
    pub fn key(&mut self, event: &KeyEvent, since: Instant) {
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
                    self.hud.keys.record(since.elapsed());
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

    /// A key from winit that is egui's rather than the far end's: view only,
    /// a prompt, a text field. It goes no further, but `down` is kept true
    /// to it. Only the far end's keys were counted, so a modifier let go of
    /// while input was blocked stayed down for the chord: Ctrl held, View
    /// only on, Ctrl let go, View only off, and the next Alt+D was the
    /// overlay. And a key that went down at the far end before its release
    /// came here (a text field taking the keyboard mid-press) is released
    /// there, which nothing else would do until the focus left.
    pub fn key_elsewhere(&mut self, event: &KeyEvent) {
        let physical = event.physical_key;
        match event.state {
            ElementState::Pressed => {
                if let Some(keysym) = keys::keysym(event) {
                    self.down.insert(physical, keysym);
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
            "settings: send clipboard {}, receive clipboard {}, grab keyboard {}, software cursor {}",
            on_off(settings.clipboard_out),
            on_off(settings.clipboard_in),
            on_off(settings.grab_keyboard),
            on_off(settings.software_cursor)
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
        // Whatever stopped input going to the far end — a prompt arriving,
        // view only, a disconnect — the keys it has down are let go of
        // there, since their releases will now be egui's. View only and the
        // disconnect did this already; a prompt that came up mid-press left
        // the key down at the far end until the focus moved.
        if !self.accepts_input() {
            self.release_keys();
        }

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
        // Taken whether or not there is a desktop for it to be sent over,
        // so that a move from before one was there is not counted later.
        let pointer_since = self.pointer_since.take();
        if let (Some(layout), Some(response)) = (layout, response) {
            self.pointer(ctx, &response, &layout, dt, pointer_since);
            if self.settings.software_cursor {
                self.draw_cursor(ctx, &response);
            } else {
                // Device pixels per desktop pixel: what the shape is
                // resampled by so that its hotspot lands where the pointer is.
                let factor = layout.scale * pixels_per_point;
                self.native_key = (self.hovered && !self.relative && self.connected())
                    .then(|| self.cursor.as_ref().map(|c| Key::new(c.hash, factor)))
                    .flatten();
            }
        }

        self.toolbar(ctx, host);
        // The keyboard is this end's while the pointer is on the toolbar
        // (see `holds_shortcuts`), so what the far end has down is let go
        // of there, as for anything else that takes input away; the
        // releases that follow are `key_elsewhere`'s.
        if self.toolbar_hovered {
            self.release_keys();
        }
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
                debug: self.hud.on,
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
                    Action::Debug(on) => {
                        if on != self.hud.on {
                            self.toggle_hud();
                        }
                    }
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
                } else if !self.settings.software_cursor {
                    "native (off desktop)"
                } else {
                    "software"
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
        // What can be timed at this end, and only that: how long the far
        // end takes to act on input and send back what it did is not
        // something any of the protocols lets this end see.
        let fifth = format!(
            "render {}  pointer {}  keys {}",
            self.hud.render.summary(),
            self.hud.pointer.summary(),
            self.hud.keys.summary()
        );
        vec![first, second, third, fourth, fifth]
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
    /// `since` is when winit handed over the first of the frame's pointer
    /// events; what was sent to the backend for them is timed from it.
    fn pointer(&mut self, ctx: &egui::Context, response: &Response, layout: &Layout, dt: f32, since: Option<Instant>) {
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
        let mut sent = false;
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
                            sent = true;
                        }
                        continue;
                    }
                    _ => continue,
                }
                sent |= self.send_pointer(at, mask);
            }
            sent |= self.send_pointer(at, mask);
        } else if mask != 0 {
            // A button pressed over the desktop and let go elsewhere: the
            // far end is told at the last place it saw.
            let released = events
                .iter()
                .any(|e| matches!(e, egui::Event::PointerButton { pressed: false, .. }));
            if released {
                let (x, y, _) = self.last_pointer;
                sent |= self.send_pointer((x, y), 0);
            }
        }
        if sent && let Some(since) = since {
            self.hud.pointer.record(since.elapsed());
        }
    }

    /// One pointer event, unless it is the one already sent; whether it
    /// was sent.
    fn send_pointer(&mut self, at: (i32, i32), mask: i32) -> bool {
        let fresh = (at.0, at.1, mask) != self.last_pointer;
        if fresh {
            self.backend.pointer(at.0, at.1, mask);
            self.last_pointer = (at.0, at.1, mask);
        }
        fresh
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

    /// Slid down from the top edge, flush with it, while the pointer is at
    /// the edge within the toolbar's own span or on the toolbar itself, and
    /// back up out of sight otherwise. Only its own span, because the rest
    /// of the top edge is the desktop's: a pointer heading for something at
    /// the top of the far end's screen, a menu bar or a tab, had the toolbar
    /// come down over it wherever it touched the edge.
    ///
    /// It can be dragged along the edge, out of the way of what is under
    /// it, by any part of it: the area senses drags beneath its buttons,
    /// which sense only clicks, so egui gives a press on a button the click
    /// and the area the drag (`hit_test`'s "the top Button and the
    /// ScrollArea behind it"). A drag-only widget starts dragging at the
    /// press, so the toolbar moves only once the pointer is decidedly
    /// dragging, which is also when a release stops being a click: a
    /// button is either clicked or dragged, never both.
    fn toolbar(&mut self, ctx: &egui::Context, host: &mut Host) {
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let screen = ctx.content_rect();
        let fullscreen = host.window.fullscreen().is_some();
        let width = self.toolbar_size.x;
        let left = self.placement.left(screen, width);
        let at_edge = pointer.is_some_and(|p| toolbar::reveals(screen, left, width, fullscreen, ctx.pixels_per_point(), p));
        let reveal = self.started.elapsed() < TOOLBAR_REVEAL;
        let show = at_edge || self.toolbar_hovered || self.drag_from.is_some() || reveal || self.disconnecting.is_some();
        let style = ctx.global_style();
        // Square, and flush with the top edge: nothing between it and the
        // edge for the pointer to fall into. Flat, too: a popup's frame
        // without the popup's shadow, so that hidden nothing but the peek
        // `hidden_y` leaves in a window shows, and shown nothing of it lies
        // over the desktop past its edge.
        //
        // Edged on three sides and not the top, which is the window's own
        // edge: a line there would be a rule across the top of the screen
        // and, hidden, all that peeked out. A frame's stroke goes all the
        // way round, so the frame is drawn without one and the three sides
        // are painted over it after, inside its rect as a frame's stroke is,
        // with the margin keeping the stroke's width so that the buttons sit
        // where they did.
        let edge = style.visuals.window_stroke();
        let edge_width = edge.width.round() as i8;
        let frame = Frame::popup(&style)
            .corner_radius(0)
            .shadow(egui::Shadow::NONE)
            .stroke(Stroke::NONE)
            .inner_margin(Margin::symmetric(8 + edge_width, 6 + edge_width));
        let hidden = toolbar::hidden_y(self.toolbar_size.y, fullscreen);
        let y = ctx.animate_value_with_time(Id::new("toolbar-y"), if show { 0.0 } else { hidden }, 0.2);
        if reveal {
            ctx.request_repaint_after(TOOLBAR_REVEAL.saturating_sub(self.started.elapsed()));
        }

        let area = egui::Area::new(Id::new("toolbar"))
            .order(Order::Foreground)
            .fixed_pos(pos2(left, screen.min.y + y))
            // Unconstrained, or egui would keep it on screen: off the top
            // is where it goes to hide.
            .constrain(false)
            .interactable(true)
            .sense(Sense::drag())
            .show(ctx, |ui| {
                let rect = frame
                    .show(ui, |ui| {
                        // A little taller than egui's buttons, which were
                        // made for a menu rather than for a bar at a
                        // screen's edge.
                        ui.spacing_mut().interact_size.y = TOOLBAR_BUTTON;
                        ui.horizontal(|ui| self.toolbar_items(ui, host));
                    })
                    .response
                    .rect;
                let inner = rect.shrink(edge.width / 2.0);
                let sides = vec![
                    pos2(inner.min.x, rect.min.y),
                    inner.left_bottom(),
                    inner.right_bottom(),
                    pos2(inner.max.x, rect.min.y),
                ];
                ui.painter().add(egui::Shape::line(sides, edge));
            });
        let response = &area.response;
        if response.drag_started() {
            self.drag_from = Some(left);
        }
        if let Some(from) = self.drag_from {
            if !response.dragged() {
                self.drag_from = None;
            } else if ctx.input(|i| i.pointer.is_decidedly_dragging()) {
                let moved = response.total_drag_delta().unwrap_or_default().x;
                self.placement.move_to(screen, width, from + moved);
                ctx.set_cursor_icon(CursorIcon::Grabbing);
            }
        } else if response.hovered() && !over_button(ctx, response) {
            // The margins, which are where to take hold of it (the buttons'
            // interact rects meet, so there are no gaps between them), but
            // not the buttons, which are pressed before they are dragged.
            ctx.set_cursor_icon(CursorIcon::Grab);
        }
        self.toolbar_size = response.rect.size();
        // A drag that has run off the toolbar is still the toolbar's, and
        // so is the keyboard while it lasts.
        self.toolbar_hovered = response.contains_pointer() || self.drag_from.is_some();
    }

    /// The toolbar's buttons: Material Symbols and no words, each with its
    /// name and its state in its tooltip. A button with a state shows it in
    /// its icon — one per scale mode, all monitors or one, view only on or
    /// off — and the tooltip says the rest (which monitor or display);
    /// Info and view only are highlighted while they are on.
    fn toolbar_items(&mut self, ui: &mut egui::Ui, host: &mut Host) {
        let connected = self.connected();
        let mode = self.view.mode;
        let scale = ui.add_enabled(connected, icon_only(scale_icon(mode)));
        if scale
            .on_hover_text(format!(
                "Scale: {} — click for {}.\n\
                 Where the desktop overflows the window, the pointer near an edge pans it.",
                mode.label().to_lowercase(),
                mode.next().label().to_lowercase(),
            ))
            .clicked()
        {
            self.view.cycle();
        }

        let facts = &self.snapshot.facts;
        // Several screens while showing them all, one while showing one;
        // which one is the tooltip's to say, since a number on the icon
        // would be the word again.
        if facts.monitors.len() > 1 {
            let count = facts.monitors.len();
            let symbol = if self.monitor.is_some() { icons::MONITOR } else { icons::TV_DISPLAYS };
            if ui
                .add_enabled(connected, icon_only(symbol))
                .on_hover_text(format!(
                    "Monitor: {} — click for {}.\nThe whole desktop, or one of its monitors.",
                    monitor_name(self.monitor, count),
                    monitor_name(next_monitor(self.monitor, count), count),
                ))
                .clicked()
            {
                self.monitor = next_monitor(self.monitor, count);
                self.view.pan = Vec2::ZERO;
            }
        } else if facts.displays.len() > 1 {
            let count = facts.displays.len();
            let next = (facts.display + 1).rem_euclid(count as i32);
            if ui
                .add_enabled(connected, icon_only(icons::MONITOR))
                .on_hover_text(format!(
                    "Display: {} of {count} — click for display {}.\nThe far end sends one display at a \
                     time; a click asks it for the next.",
                    facts.display + 1,
                    next + 1,
                ))
                .clicked()
            {
                self.backend.request_display(next);
            }
        }

        // An eye while only watching, a mouse while in control, so that the
        // icon says which without the highlight's help.
        let view_only = self.view_only;
        let symbol = if view_only { icons::VISIBILITY } else { icons::MOUSE };
        if ui
            .add_enabled(connected, toggle(ui.visuals(), symbol, view_only))
            .on_hover_text(if view_only {
                "View only: on — the mouse, the keyboard and the clipboard stay here. Click to take \
                 control again."
            } else {
                "View only: off — the mouse, the keyboard and the clipboard go to the far end. Click \
                 to watch only."
            })
            .clicked()
        {
            self.set_option("ViewOnly", if view_only { "false" } else { "true" });
        }

        if ui
            .add_enabled(connected, toggle(ui.visuals(), icons::INFO, self.panel.open))
            .on_hover_text("Info: the connection's facts, and the settings it can still be told")
            .clicked()
        {
            self.panel.open = !self.panel.open;
        }

        let fullscreen = host.window.fullscreen().is_some();
        if ui
            .add(icon_only(if fullscreen { icons::FULLSCREEN_EXIT } else { icons::FULLSCREEN }))
            .on_hover_text(if fullscreen {
                "Leave fullscreen: back to a window"
            } else {
                "Fullscreen: fill the screen. The toolbar still slides in at the top edge; its button, \
                 or disconnecting, leaves fullscreen."
            })
            .clicked()
        {
            log::info!("{}", if fullscreen { "leaving fullscreen" } else { "fullscreen" });
            host.window
                .set_fullscreen((!fullscreen).then_some(Fullscreen::Borderless(None)));
        }

        if ui
            .add_enabled(self.disconnecting.is_none() && self.closed.is_none(), icon_only(icons::LINK_OFF))
            .on_hover_text(if self.relative {
                "Disconnect: end the session. Ctrl+Alt held together frees the locked pointer."
            } else {
                "Disconnect: end the session"
            })
            .clicked()
        {
            self.disconnect();
        }
    }

    /// Whether the session's ending was asked for from this side, rather
    /// than the far end's or the library's doing.
    pub fn disconnect_asked(&self) -> bool {
        self.disconnecting.is_some()
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

/// A toolbar icon, at the toolbar's size.
fn icon(symbol: &'static str) -> RichText {
    icons::text(symbol).size(TOOLBAR_ICON)
}

/// The Scale button's icon for a mode: Material Symbols' own fit page
/// width and height for the fit modes, arrows kept within a page as fit
/// keeps the desktop within 1:1; the bare width and height arrows for the
/// fill modes, which stretch the desktop along that axis however far the
/// window asks; and the symbol that reads "1:1" for 1:1.
fn scale_icon(mode: ScaleMode) -> &'static str {
    match mode {
        ScaleMode::FitWidth => icons::FIT_PAGE_WIDTH,
        ScaleMode::FitHeight => icons::FIT_PAGE_HEIGHT,
        ScaleMode::FillWidth => icons::WIDTH,
        ScaleMode::FillHeight => icons::HEIGHT,
        ScaleMode::Original => icons::VIEW_REAL_SIZE,
    }
}

/// Whether the pointer is on one of the toolbar's buttons rather than on
/// the toolbar `area` itself. Both count as hovered, as egui hovers the top
/// button and the area that senses drags behind it alike, so it is told
/// apart by what else is under the pointer on the toolbar's layer: anything
/// there that takes clicks is a button. (On its layer only: the desktop
/// under the toolbar takes clicks too.) Read out first and then looked up,
/// as the snapshot holds the context that `read_response` takes too.
fn over_button(ctx: &egui::Context, area: &egui::Response) -> bool {
    let under: Vec<Id> = ctx.interaction_snapshot(|s| s.contains_pointer.iter().copied().collect());
    under.into_iter().any(|id| {
        id != area.id
            && ctx
                .read_response(id)
                .is_some_and(|r| r.layer_id == area.layer_id && r.sense.senses_click())
    })
}

/// A toolbar button of an icon alone, square, and flat: no frame at rest,
/// egui's while the pointer is on it or pressing it.
fn icon_only(symbol: &'static str) -> egui::Button<'static> {
    egui::Button::new(icon(symbol)).min_size(Vec2::splat(TOOLBAR_BUTTON)).frame_when_inactive(false)
}

/// A toolbar button that is on or off: flat while off, and while on framed
/// at rest too (as `Button::selectable` is), in the selection's colours, so
/// that on reads from across the toolbar without a hover — its icon in the
/// selection's colour, on a fill of the selection's blue at about half its
/// strength rather than all of it, which three buttons on at once made the
/// loudest thing on the screen.
fn toggle(visuals: &egui::Visuals, symbol: &'static str, on: bool) -> egui::Button<'static> {
    let button = icon_only(symbol).selected(on).frame_when_inactive(on);
    if on { button.fill(visuals.selection.bg_fill.gamma_multiply(0.5)) } else { button }
}

/// The Monitor button's next choice after `current` of `count` monitors:
/// the whole desktop (`None`), then each monitor in turn, then the whole
/// desktop again.
fn next_monitor(current: Option<usize>, count: usize) -> Option<usize> {
    match current {
        None if count > 0 => Some(0),
        Some(i) if i + 1 < count => Some(i + 1),
        _ => None,
    }
}

/// A Monitor choice in words, for the tooltip.
fn monitor_name(monitor: Option<usize>, count: usize) -> String {
    match monitor {
        None => format!("all {count}"),
        Some(i) => format!("{} of {count}", i + 1),
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scale_mode_has_its_own_icon() {
        let mut mode = ScaleMode::FitWidth;
        let mut seen = Vec::new();
        for _ in 0..5 {
            let codepoint = scale_icon(mode);
            assert!(!seen.contains(&codepoint), "{} shares its icon", mode.label());
            seen.push(codepoint);
            mode = mode.next();
        }
        assert_eq!(mode, ScaleMode::FitWidth);
    }

    #[test]
    fn the_monitor_button_goes_round_all_then_each() {
        let mut monitor = None;
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(monitor_name(monitor, 3));
            monitor = next_monitor(monitor, 3);
        }
        assert_eq!(seen, ["all 3", "1 of 3", "2 of 3", "3 of 3"]);
        assert_eq!(monitor, None);
        // A monitor gone from under the choice goes back to all of them.
        assert_eq!(next_monitor(Some(5), 2), None);
    }
}
