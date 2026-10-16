// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! One window's part of a session: how the desktop is shown in it, its
//! toolbar, the pointer over it, the far end's cursor as its own, and its
//! share of the debug overlay.
//!
//! A session is one connection, one backend, one desktop texture, one set
//! of keys held at the far end, one capture and one set of prompts, and
//! every window it is shown in draws from those (`session.rs`). What each
//! window has of its own is here: a [`View`] (the scale mode and the pan),
//! which of the far end's monitors it shows, where its toolbar sits and
//! whether the pointer is on it, whether the pointer is over its desktop,
//! the cursor its window was last given, and its own frame counts and
//! latencies. There is one pane today, the main window's; a pane is keyed
//! by its window's egui viewport.
//!
//! A child of the session's module, though a file beside it, so that a
//! pane's frame can read and drive the session's state as it did when the
//! two were one.

use super::super::cursor::{Key, Native};
use super::super::desktop::Desktop;
use super::super::hud::{self, Rate};
use super::super::icons;
use super::super::panel::Snapshot;
use super::super::toolbar::{self, Placement};
use super::super::view::{Layout, ScaleMode, View};
use super::super::Host;
use super::{Capture, LEFT, MIDDLE, NOTCH_POINTS, RIGHT, Session, on_off};
use crate::backend::Monitor;
use egui::{
    Color32, CursorIcon, Frame, Id, LayerId, Margin, Order, PointerButton, Pos2, Rect, Response, RichText, Sense,
    Stroke, Vec2, ViewportId, pos2, vec2,
};
use std::time::{Duration, Instant};
use winit::window::Fullscreen;

/// A pane is known by its window's egui viewport: the root one for the
/// main window.
pub type PaneId = ViewportId;

/// How long the toolbar stays out after the pane opens, so that it is seen
/// once.
const TOOLBAR_REVEAL: Duration = Duration::from_secs(3);
/// The toolbar's buttons' height, in points.
const TOOLBAR_BUTTON: f32 = 28.0;
/// The size of the toolbar's icons, in points: Material Symbols are drawn
/// for 24, and read well down to 18.
const TOOLBAR_ICON: f32 = 20.0;

/// Which of the far end's monitors a pane shows, kept as enough to find it
/// again when the far end's layout changes under it: where it was in the
/// list, and its rectangle. (The protocols that number their screens will
/// add the number, which comes before both.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorKey {
    pub index: usize,
    pub rect: (i32, i32, i32, i32),
}

impl MonitorKey {
    pub fn of(index: usize, monitor: &Monitor) -> MonitorKey {
        MonitorKey {
            index,
            rect: (monitor.x, monitor.y, monitor.width, monitor.height),
        }
    }

    /// The same monitor in `monitors`, the layout as it is now: the one
    /// with the same rectangle wherever it is in the list, since a list
    /// that gained or lost a screen before it has moved it along; failing
    /// that the one at the same place in the list, which is a monitor that
    /// changed size; and failing both, none — it is gone.
    pub fn find(self, monitors: &[Monitor]) -> Option<MonitorKey> {
        let same_rect = monitors
            .iter()
            .position(|m| (m.x, m.y, m.width, m.height) == self.rect);
        let index = same_rect.or((self.index < monitors.len()).then_some(self.index))?;
        Some(MonitorKey::of(index, &monitors[index]))
    }
}

/// Whether `pane` may send pointer events while `owner` holds the pointer:
/// a button pressed in one window and dragged across another is the first
/// window's until it is let go of, and the second sends nothing meanwhile.
pub fn may_send(owner: Option<PaneId>, pane: PaneId) -> bool {
    owner.is_none_or(|o| o == pane)
}

/// Who holds the pointer once `pane` has sent `mask`: the pane, while any
/// button is down; nobody, once every button is up.
pub fn owner_after(pane: PaneId, mask: i32) -> Option<PaneId> {
    (mask != 0).then_some(pane)
}

pub struct Pane {
    pub id: PaneId,
    view: View,
    /// Which of the far end's monitors is on screen, or `None` for the
    /// whole desktop.
    monitor: Option<MonitorKey>,
    /// Where the toolbar sits along the top edge: where the last session
    /// left it, then wherever it is dragged.
    pub placement: Placement,
    /// Where the toolbar's left edge was when the drag under way began.
    drag_from: Option<f32>,
    /// The toolbar's size as last drawn, which is where it would be when
    /// out, and so the span of the edge that brings it out.
    toolbar_size: Vec2,
    /// The pointer is on the toolbar: it stays out, and the keyboard is
    /// this end's meanwhile.
    pub toolbar_hovered: bool,
    /// When the pane opened, which is how long its toolbar stays out first.
    opened: Instant,
    /// Whether the pointer was over the desktop in the last frame.
    hovered: bool,
    /// The far end's cursor as this window's own.
    pub native: Native,
    /// The shape the window's cursor should be this frame, if the far
    /// end's: the software cursor setting off, absolute mode, the pointer
    /// over the desktop, and a shape to show.
    pub native_key: Option<Key>,
    /// Frames drawn, and their rate, for the overlay.
    frames: u64,
    fps: Rate,
    /// The desktop's upload count at the last frame, for the per-frame
    /// figure.
    uploads_seen: u64,
    /// When the oldest frame taken but not yet presented here ended at the
    /// backend: the start of the overlay's render latency.
    pub unpresented: Option<Instant>,
    /// When winit handed over the first pointer event egui has not yet
    /// read, which the next frame turns into the pointer event the backend
    /// is given: the start of the overlay's pointer latency.
    pub pointer_since: Option<Instant>,
}

impl Pane {
    pub fn new(id: PaneId, placement: Placement) -> Pane {
        Pane {
            id,
            view: View::default(),
            monitor: None,
            placement,
            drag_from: None,
            toolbar_size: vec2(0.0, 40.0),
            toolbar_hovered: false,
            opened: Instant::now(),
            hovered: false,
            native: Native::default(),
            native_key: None,
            frames: 0,
            fps: Rate::default(),
            uploads_seen: 0,
            unpresented: None,
            pointer_since: None,
        }
    }

    fn main(&self) -> bool {
        self.id == ViewportId::ROOT
    }

    /// The far end's layout was replaced (a new desktop size): the pane
    /// goes back to the whole desktop.
    pub fn desktop_changed(&mut self) {
        self.monitor = None;
    }

    /// This pane's frame: the desktop, the pointer over it, the toolbar,
    /// and the overlay — and in the main window the panel and the prompts,
    /// which are only ever there.
    pub fn frame(&mut self, s: &mut Session, root: &mut egui::Ui, host: &mut Host) {
        let ctx = &root.ctx().clone();
        s.sync_capture(host);

        if self.main() {
            let title = s.title();
            if title != title_set(ctx) {
                host.window.set_title(&title);
                ctx.data_mut(|d| d.insert_temp(Id::new("title"), title));
            }
        }

        let pixels_per_point = ctx.pixels_per_point();
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let mut layout = None;
        let mut response = None;
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(Color32::from_gray(24)))
            .show(root, |ui| {
                let window = ui.max_rect();
                let Some((width, height)) = s.desktop.as_ref().map(Desktop::size) else {
                    s.waiting(ui);
                    return;
                };
                let source = self.source_rect(&s.snapshot, width, height);
                let l = self.view.layout(source, window, pixels_per_point);
                let exact = (l.scale * pixels_per_point - 1.0).abs() < 1e-3;
                let desktop = s.desktop.as_mut().expect("checked above");
                desktop.set_exact(ctx, &*s.backend, exact);
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
            self.pointer(s, ctx, &response, &layout, dt, pointer_since);
            if s.settings.software_cursor {
                draw_cursor(s, ctx, &response);
            } else {
                // Device pixels per desktop pixel: what the shape is
                // resampled by so that its hotspot lands where the pointer is.
                let factor = layout.scale * pixels_per_point;
                self.native_key = (self.hovered && !s.relative && s.connected())
                    .then(|| s.cursor.as_ref().map(|c| Key::new(c.hash, factor)))
                    .flatten();
            }
        }

        self.toolbar(s, ctx, host);
        // The keyboard is this end's while the pointer is on the toolbar
        // (see `holds_shortcuts`), so what the far end has down is let go
        // of there, as for anything else that takes input away; the
        // releases that follow are `key_elsewhere`'s.
        if self.toolbar_hovered {
            s.release_keys();
        }
        if self.main() {
            s.panel(ctx, host);
        }
        if s.hud.on {
            let lines = self.hud_lines(s, host, layout.as_ref(), pixels_per_point);
            hud::Hud::draw(ctx, &lines);
        }
        if self.main() {
            s.prompt(ctx);
        }
    }

    /// The desktop pixels on screen: one monitor's, or all of them. A
    /// monitor that has moved in the far end's list is followed there, and
    /// one that has gone leaves the whole desktop.
    fn source_rect(&mut self, snapshot: &Snapshot, width: usize, height: usize) -> Rect {
        let whole = Rect::from_min_size(Pos2::ZERO, vec2(width as f32, height as f32));
        let Some(key) = self.monitor else {
            return whole;
        };
        self.monitor = key.find(&snapshot.facts.monitors);
        match self.monitor {
            Some(MonitorKey { rect: (x, y, w, h), .. }) => {
                let r = Rect::from_min_size(pos2(x as f32, y as f32), vec2(w as f32, h as f32)).intersect(whole);
                if r.width() >= 1.0 && r.height() >= 1.0 {
                    r
                } else {
                    whole
                }
            }
            None => whole,
        }
    }

    /// The pointer over the desktop: the edge pan, and in absolute mode the
    /// position, the buttons and the wheel.
    /// `since` is when winit handed over the first of the frame's pointer
    /// events; what was sent to the backend for them is timed from it.
    fn pointer(
        &mut self,
        s: &mut Session,
        ctx: &egui::Context,
        response: &Response,
        layout: &Layout,
        dt: f32,
        since: Option<Instant>,
    ) {
        let over = response.hovered() || response.dragged() || response.is_pointer_button_down_on();
        let pos = ctx.input(|i| i.pointer.latest_pos());

        if over && !s.captured() && let Some(pos) = pos {
            let velocity = layout.edge_velocity(pos);
            if velocity != Vec2::ZERO {
                self.view.pan += velocity * dt;
                ctx.request_repaint();
            }
        }

        if response.clicked() || response.drag_started() {
            // A click on the desktop takes the pointer back after the chord.
            s.escaped = false;
        }
        if !s.accepts_input() || s.relative || !may_send(s.pointer_owner, self.id) {
            return;
        }

        // The frame's events in order rather than the state at its end,
        // so that a press and a release in the same frame are both sent,
        // and in that order.
        let events = ctx.input(|i| i.events.clone());
        let (_, _, mut mask) = s.last_pointer;
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
                        for bit in s.notches(notches) {
                            s.backend.pointer(at.0, at.1, mask | bit);
                            s.backend.pointer(at.0, at.1, mask);
                            sent = true;
                        }
                        continue;
                    }
                    _ => continue,
                }
                sent |= s.send_pointer(at, mask);
            }
            sent |= s.send_pointer(at, mask);
        } else if mask != 0 {
            // A button pressed over the desktop and let go elsewhere: the
            // far end is told at the last place it saw.
            let released = events
                .iter()
                .any(|e| matches!(e, egui::Event::PointerButton { pressed: false, .. }));
            if released {
                let (x, y, _) = s.last_pointer;
                sent |= s.send_pointer((x, y), 0);
            }
        }
        s.pointer_owner = owner_after(self.id, s.last_pointer.2);
        if sent && let Some(since) = since {
            s.hud.pointer.record(since.elapsed());
        }
    }

    // ---- the toolbar -------------------------------------------------------

    /// Slid down from the top edge, flush with it, while the pointer is at
    /// the edge within the toolbar's own span or on the toolbar itself, and
    /// back up otherwise, to a peek of its lower edge in a window or with
    /// the pointer on fullscreen's top row, and out of sight in fullscreen
    /// with the pointer anywhere else. Only its own span, because the rest
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
    fn toolbar(&mut self, s: &mut Session, ctx: &egui::Context, host: &mut Host) {
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let screen = ctx.content_rect();
        let fullscreen = host.window.fullscreen().is_some();
        let width = self.toolbar_size.x;
        let left = self.placement.left(screen, width);
        let at_edge = pointer.is_some_and(|p| toolbar::reveals(screen, left, width, fullscreen, ctx.pixels_per_point(), p));
        let reveal = self.opened.elapsed() < TOOLBAR_REVEAL;
        let show = at_edge || self.toolbar_hovered || self.drag_from.is_some() || reveal || s.disconnecting.is_some();
        let style = ctx.global_style();
        // Square, and flush with the top edge: nothing between it and the
        // edge for the pointer to fall into. Flat, too: a popup's frame
        // without the popup's shadow, so that hidden nothing but the peek
        // `hidden_y` leaves shows, and shown nothing of it lies
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
        // Hidden, it peeks in a window always and in fullscreen while the
        // pointer is on the screen's top row anywhere along it; the slide
        // to and from the peek is the same slide, the same 0.2 s.
        let peeks = toolbar::peeks(screen, fullscreen, ctx.pixels_per_point(), pointer);
        let hidden = toolbar::hidden_y(self.toolbar_size.y, peeks);
        let y = ctx.animate_value_with_time(Id::new("toolbar-y"), if show { 0.0 } else { hidden }, 0.2);
        if reveal {
            ctx.request_repaint_after(TOOLBAR_REVEAL.saturating_sub(self.opened.elapsed()));
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
                        ui.horizontal(|ui| self.toolbar_items(s, ui, host));
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
    fn toolbar_items(&mut self, s: &mut Session, ui: &mut egui::Ui, host: &mut Host) {
        let connected = s.connected();
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

        let facts = &s.snapshot.facts;
        // Several screens while showing them all, one while showing one;
        // which one is the tooltip's to say, since a number on the icon
        // would be the word again.
        if facts.monitors.len() > 1 {
            let count = facts.monitors.len();
            let current = self.monitor.map(|k| k.index);
            let symbol = if current.is_some() { icons::MONITOR } else { icons::TV_DISPLAYS };
            if ui
                .add_enabled(connected, icon_only(symbol))
                .on_hover_text(format!(
                    "Monitor: {} — click for {}.\nThe whole desktop, or one of its monitors.",
                    monitor_name(current, count),
                    monitor_name(next_monitor(current, count), count),
                ))
                .clicked()
            {
                self.monitor = next_monitor(current, count).map(|i| MonitorKey::of(i, &facts.monitors[i]));
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
                s.backend.request_display(next);
            }
        }

        // An eye while only watching, a mouse while in control, so that the
        // icon says which without the highlight's help.
        let view_only = s.view_only;
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
            s.set_option("ViewOnly", if view_only { "false" } else { "true" });
        }

        if ui
            .add_enabled(connected, toggle(ui.visuals(), icons::INFO, s.panel.open))
            .on_hover_text("Info: the connection's facts, and the settings it can still be told")
            .clicked()
        {
            s.panel.open = !s.panel.open;
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
            .add_enabled(s.disconnecting.is_none() && s.closed.is_none(), icon_only(icons::LINK_OFF))
            .on_hover_text(if s.relative {
                "Disconnect: end the session. Ctrl+Alt held together frees the locked pointer."
            } else {
                "Disconnect: end the session"
            })
            .clicked()
        {
            s.disconnect();
        }
    }

    // ---- the debug overlay -------------------------------------------------

    /// `SessionHud.draw`'s lines, in its order where they apply here: the
    /// state and the pixel path, the cursor and the view, the physical
    /// mouse and keyboard, and the things this window has that the phone
    /// has not. The session's, with this pane's frame rate, uploads, view
    /// and hover in them.
    fn hud_lines(&mut self, s: &mut Session, host: &Host, layout: Option<&Layout>, pixels_per_point: f32) -> Vec<String> {
        let now = Instant::now();
        self.frames += 1;
        let fps = self.fps.sample(self.frames, now);
        let damage_rate = s.hud.damage.sample(s.damage, now);
        let (uploads, bytes, last) = s.desktop.as_ref().map(Desktop::stats).unwrap_or((0, 0, None));
        let per_frame = uploads.saturating_sub(self.uploads_seen);
        self.uploads_seen = uploads;
        let read_rate = s.hud.bytes.sample(bytes, now);
        let (width, height) = s.desktop.as_ref().map(Desktop::size).unwrap_or((0, 0));
        // `SessionClipboard.summary`: what the far end would be given.
        let clip = match &s.clipboard_seen {
            Some(text) if s.settings.clipboard_out => format!("{}c", text.chars().count()),
            _ => "-".to_string(),
        };
        let state = format!("{:?}", s.mailbox.state()).to_ascii_lowercase();
        let first = format!(
            "state {state}  desktop {width}x{height}  fps {fps}  dmg {} ({damage_rate}/s)  tex {per_frame}/frame  rd {}  last {}  clip {clip}",
            s.damage,
            hud::bytes_per_second(read_rate),
            last.map(|r| format!("{},{} {}x{}", r.x, r.y, r.width, r.height))
                .unwrap_or_else(|| "-".to_string()),
        );

        let (x, y, mask) = s.last_pointer;
        let cursor = if s.relative {
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

        let mouse = match s.capture {
            Some((_, Capture::Locked)) => "captured (locked)",
            Some((_, Capture::Centred)) => "captured (centred)",
            None if self.hovered => "hover",
            None => "-",
        };
        // Every whole pixel of raw motion sent so far in relative mode, so
        // that motion counted twice shows as twice the distance.
        let sent = if s.relative {
            format!("  Δ {},{}", s.motion_sent.0, s.motion_sent.1)
        } else {
            String::new()
        };
        let third = format!(
            "mouse {mouse}{sent}  btn {}  keys {} held{}",
            hud::buttons_name(if s.captured() { s.captured_mask } else { mask }),
            s.held.len(),
            if s.view_only { "   VIEW ONLY" } else { "" }
        );

        let follow = if !s.follower.enabled() {
            "off".to_string()
        } else {
            match s.follower.followed() {
                Some((w, h)) => format!("{w}x{h}"),
                None => "waiting".to_string(),
            }
        };
        let shape = match &s.cursor {
            None => "none".to_string(),
            Some(c) => {
                let how = if self.native.shown().is_some() {
                    "native"
                } else if !s.settings.software_cursor {
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
            on_off(s.settings.clipboard_out),
            on_off(s.settings.clipboard_in)
        );
        // What can be timed at this end, and only that: how long the far
        // end takes to act on input and send back what it did is not
        // something any of the protocols lets this end see.
        let fifth = format!(
            "render {}  pointer {}  keys {}",
            s.hud.render.summary(),
            s.hud.pointer.summary(),
            s.hud.keys.summary()
        );
        vec![first, second, third, fourth, fifth]
    }
}

fn title_set(ctx: &egui::Context) -> String {
    ctx.data(|d| d.get_temp::<String>(Id::new("title"))).unwrap_or_default()
}

/// The far end's cursor under the local pointer, in absolute mode.
fn draw_cursor(s: &Session, ctx: &egui::Context, response: &Response) {
    if s.relative || !s.connected() {
        return;
    }
    let Some(cursor) = &s.cursor else {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(x: i32, y: i32, width: i32, height: i32) -> Monitor {
        Monitor { x, y, width, height }
    }

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

    #[test]
    fn a_monitor_is_found_by_its_rectangle_first() {
        let two = [monitor(0, 0, 1920, 1080), monitor(1920, 0, 1280, 1024)];
        let right = MonitorKey::of(1, &two[1]);
        // Unchanged: itself.
        assert_eq!(right.find(&two), Some(right));
        // A screen added before it moves it along the list; it is followed
        // there, by its rectangle, rather than staying at its old place.
        let three = [monitor(-1280, 0, 1280, 1024), two[0], two[1]];
        assert_eq!(right.find(&three), Some(MonitorKey::of(2, &three[2])));
        // And one taken away before it.
        let left = MonitorKey::of(0, &two[0]);
        assert_eq!(left.find(&three), Some(MonitorKey::of(1, &three[1])));
    }

    #[test]
    fn a_monitor_that_changed_shape_is_found_by_its_place_and_a_gone_one_not_at_all() {
        let two = [monitor(0, 0, 1920, 1080), monitor(1920, 0, 1280, 1024)];
        let right = MonitorKey::of(1, &two[1]);
        // Resized where it is: the same place in the list, its new rectangle
        // kept from now on.
        let resized = [two[0], monitor(1920, 0, 1600, 900)];
        let found = right.find(&resized).unwrap();
        assert_eq!(found, MonitorKey { index: 1, rect: (1920, 0, 1600, 900) });
        // Gone, with nothing of its shape and nothing at its place.
        assert_eq!(right.find(&two[..1]), None);
        assert_eq!(right.find(&[]), None);
    }

    #[test]
    fn a_drag_belongs_to_the_pane_it_was_pressed_in() {
        let main = ViewportId::ROOT;
        let other = ViewportId::from_hash_of("pop-out");
        // Nobody holds the pointer: anyone may send.
        assert!(may_send(None, main));
        assert!(may_send(None, other));
        // A press in the main window makes it the holder; the other window
        // sends nothing while the drag lasts, even with the pointer over it.
        let owner = owner_after(main, super::super::LEFT);
        assert_eq!(owner, Some(main));
        assert!(may_send(owner, main));
        assert!(!may_send(owner, other));
        // Every button up: nobody again.
        assert_eq!(owner_after(main, 0), None);
        assert!(may_send(owner_after(main, 0), other));
        // Moving with nothing pressed holds nothing.
        assert_eq!(owner_after(other, 0), None);
    }
}
