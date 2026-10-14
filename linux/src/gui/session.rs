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
//! once a second, which is what names the window and fills the panel.
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
//! Android app does. In relative mode the far end draws its own cursor into
//! the picture and owns the motion, so the local pointer is locked and
//! hidden while the window has focus and its raw motion is sent as deltas;
//! Ctrl+Alt held together lets it go, and a click on the desktop takes it
//! back.

use super::desktop::Desktop;
use super::keys::{self, KeyIds};
use super::panel::{self, Snapshot};
use super::prompts::{self, Answer, Form, Prompt};
use super::view::{Layout, View};
use super::{Host, Request};
use crate::backend::Backend;
use crate::backends::{Connect, Dirs};
use crate::events::{Cursor, Event, Mailbox, State};
use egui::{
    Align2, Color32, CursorIcon, Frame, Id, LayerId, Order, PointerButton, Pos2, Rect, Response, Sense,
    TextureHandle, TextureOptions, Vec2, pos2, vec2,
};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use winit::keyboard::PhysicalKey;
use xkeysym::Keysym;
use winit::window::CursorGrabMode;

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

/// The far end's cursor, uploaded once per shape.
struct CursorShape {
    texture: TextureHandle,
    hot: Vec2,
    size: Vec2,
}

pub struct Session {
    pub request: Request,
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
    panel_open: bool,
    prompts: VecDeque<Prompt>,
    form: Form,
    cursor: Option<CursorShape>,
    /// What the far end said about who owns the pointer.
    relative: bool,
    /// Whether the local pointer is locked to the window right now.
    captured: bool,
    /// The escape chord was used; a click on the desktop ends it.
    escaped: bool,
    key_ids: KeyIds,
    /// The physical keys this session has down at the far end, and the
    /// keysym each went down as.
    held: HashMap<PhysicalKey, u32>,
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
        Ok(Session {
            request,
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
            panel_open: false,
            prompts: VecDeque::new(),
            form: Form::default(),
            cursor: None,
            relative: false,
            captured: false,
            escaped: false,
            key_ids: KeyIds::default(),
            held: HashMap::new(),
            last_pointer: (-1, -1, 0),
            wheel: Vec2::ZERO,
            motion_rest: (0.0, 0.0),
            captured_mask: 0,
            view_only: false,
            clipboard_seen: None,
            bell_until: None,
            title,
            disconnecting: None,
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
    /// which is only while the pointer is locked.
    pub fn takes_raw_mouse(&self) -> bool {
        self.captured && self.accepts_input()
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
                    log::debug!("clipboard from the far end: {} bytes", text.len());
                    if self.clipboard_seen.as_deref() != Some(text.as_str()) {
                        host.winit.set_clipboard_text(text.clone());
                        self.clipboard_seen = Some(text);
                    }
                }
                Event::Closed { detail } => {
                    // An ending that was asked for needs no telling, whatever
                    // words the library closed with.
                    self.closed = Some(if self.disconnecting.is_some() { String::new() } else { detail });
                }
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
            let image = egui::ColorImage::from_rgba_premultiplied([c.width, c.height], &c.rgba);
            CursorShape {
                texture: ctx.load_texture("cursor", image, TextureOptions::NEAREST),
                hot: vec2(c.hot_x as f32, c.hot_y as f32),
                size: vec2(c.width as f32, c.height as f32),
            }
        });
    }

    /// The facts and the info, once a second: the window's title and the
    /// panel come from here, and so does the monitor list.
    fn poll(&mut self, ctx: &egui::Context) {
        if !self.connected() {
            return;
        }
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= Duration::from_secs(1));
        if !due {
            if let Some(last) = self.last_poll {
                ctx.request_repaint_after(Duration::from_secs(1).saturating_sub(last.elapsed()));
            }
            return;
        }
        self.last_poll = Some(Instant::now());
        self.snapshot = Snapshot {
            facts: self.backend.facts(),
            info: self.backend.info(),
            traffic: self.backend.traffic(),
        };
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    // ---- focus, capture and the clipboard ----------------------------------

    pub fn focus(&mut self, focused: bool, host: &mut Host) {
        self.backend.focus(focused);
        if focused {
            // What was copied here while the window was away, offered once.
            if self.accepts_input()
                && let Some(text) = host.winit.clipboard_text()
                && !text.is_empty()
                && self.clipboard_seen.as_deref() != Some(text.as_str())
            {
                log::debug!("clipboard to the far end: {} bytes", text.len());
                self.backend.clipboard(&text);
                self.clipboard_seen = Some(text);
            }
        } else {
            self.release_keys();
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

    /// Lock or free the pointer to match what the mode, the focus and the
    /// escape chord say.
    fn sync_capture(&mut self, host: &mut Host) {
        let want = self.relative && host.focused && !self.escaped && self.accepts_input();
        if want == self.captured {
            return;
        }
        if want {
            match host.window.set_cursor_grab(CursorGrabMode::Locked) {
                Ok(()) => {
                    host.window.set_cursor_visible(false);
                    self.captured = true;
                    self.motion_rest = (0.0, 0.0);
                    log::info!("pointer locked (Ctrl+Alt releases it)");
                }
                Err(e) => log::warn!("pointer lock refused: {e}"),
            }
        } else {
            let _ = host.window.set_cursor_grab(CursorGrabMode::None);
            host.window.set_cursor_visible(true);
            self.captured = false;
            if self.captured_mask != 0 {
                self.captured_mask = 0;
                self.backend.pointer_relative(0, 0, 0);
            }
            log::info!("pointer released");
        }
    }

    // ---- input from winit --------------------------------------------------

    /// A key from winit, already known to be ours (see `takes_keys`).
    pub fn key(&mut self, event: &KeyEvent) {
        let physical = event.physical_key;
        match event.state {
            ElementState::Pressed => {
                if let Some(keysym) = keys::keysym(event) {
                    let id = self.key_ids.id(physical);
                    self.held.insert(physical, keysym);
                    log::debug!("key down {keysym:#x} ({:?}) as {id}", event.logical_key);
                    self.backend.key_down(keysym, id);
                }
                if self.captured && self.chord_held() {
                    // Let go of the lock, and of the chord's keys at the far
                    // end, which otherwise stay down there.
                    self.escaped = true;
                    self.release_keys();
                }
            }
            ElementState::Released => {
                if self.held.remove(&physical).is_some() {
                    self.backend.key_up(self.key_ids.id(physical));
                }
            }
        }
    }

    /// Ctrl and Alt, either side, held together. By keysym rather than by
    /// key code, so that a keyboard whose codes are not the usual ones (a
    /// virtual one, say) still has the chord.
    fn chord_held(&self) -> bool {
        let down = |a: Keysym, b: Keysym| self.held.values().any(|&k| k == a.raw() || k == b.raw());
        down(Keysym::Control_L, Keysym::Control_R) && down(Keysym::Alt_L, Keysym::Alt_R)
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

    /// One frame: the events, the poll, the desktop, the chrome.
    pub fn frame(&mut self, root: &mut egui::Ui, host: &mut Host) {
        let ctx = &root.ctx().clone();
        self.drain(ctx, host);
        self.poll(ctx);
        if let Some(since) = self.disconnecting
            && self.closed.is_none()
            && since.elapsed() > DISCONNECT_PATIENCE
        {
            log::warn!("no close within {DISCONNECT_PATIENCE:?} of the disconnect; giving up on it");
            self.closed = Some(String::new());
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
                ui.painter()
                    .image(desktop.id(), l.rect, l.uv(width as f32, height as f32), Color32::WHITE);
                response = Some(ui.interact(window, Id::new("desktop"), Sense::click_and_drag()));
                layout = Some(l);
            });
        if let (Some(layout), Some(response)) = (layout, response) {
            self.pointer(ctx, &response, &layout, dt);
            self.draw_cursor(ctx, &response);
        }

        self.toolbar(ctx);
        if self.panel_open {
            let state = self.state_line();
            let title = self.request.address.clone();
            panel::show(ctx, &mut self.panel_open, &self.snapshot, &title, &state);
        }
        self.prompt(ctx);
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

        if over && !self.captured && let Some(pos) = pos {
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
    fn toolbar(&mut self, ctx: &egui::Context) {
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
                    ui.horizontal(|ui| self.toolbar_items(ui));
                });
            });
        self.toolbar_height = area.response.rect.height();
        self.toolbar_hovered = area.response.contains_pointer();
    }

    fn toolbar_items(&mut self, ui: &mut egui::Ui) {
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

        let mut view_only = self.view_only;
        if ui
            .add_enabled(connected, egui::Button::new(if view_only { "View only: on" } else { "View only: off" }))
            .on_hover_text("Watch without sending the mouse, the keyboard or the clipboard")
            .clicked()
        {
            view_only = !view_only;
        }
        if view_only != self.view_only {
            self.view_only = view_only;
            self.backend.set_view_only(view_only);
            if view_only {
                self.release_keys();
            }
        }

        if ui
            .add_enabled(connected, egui::Button::new("Info"))
            .on_hover_text("The connection's facts")
            .clicked()
        {
            self.panel_open = !self.panel_open;
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
                Answer::Security { accept, remember } => self.backend.answer_security(accept, remember),
            }
        }
    }

    // ---- the end -----------------------------------------------------------

    /// Ends the session and releases the backend, and the texture with it.
    pub fn shutdown(mut self, ctx: &egui::Context, host: &mut Host) {
        self.release_keys();
        if self.captured {
            let _ = host.window.set_cursor_grab(CursorGrabMode::None);
            host.window.set_cursor_visible(true);
        }
        if let Some(desktop) = self.desktop.take() {
            desktop.free(ctx);
        }
        if self.closed.is_none() {
            self.backend.disconnect();
        }
        self.backend.destroy();
    }
}
