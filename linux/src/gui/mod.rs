// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window: one winit event loop, one wgpu surface, egui drawn into it,
//! and either the connection dialog or a session on screen.
//!
//! winit, egui-winit and egui-wgpu are driven directly rather than through
//! eframe. eframe is less code for an ordinary application, but this one
//! needs what eframe wraps away: the window's raw handles for the shortcuts
//! inhibitor (`wayland.rs`), the X11 keyboard grab (`x11.rs`) and the Wayland
//! clipboard (`clipboard/`), device
//! events for the raw motion a captured pointer sends, focus and keyboard events as winit delivers them (with
//! the layout's logical key, which egui's events drop), and a say in when
//! the loop wakes — the backend's mailbox wakes it through an event-loop
//! proxy, and egui's own repaint requests set a deadline it waits for. That
//! is four things eframe would have to be worked around for, against a page
//! of surface setup to do without it.
//!
//! The main thread never waits on a backend: a session is created
//! connecting and asked questions that return at once, and everything it
//! has to say arrives through the mailbox between frames.

mod clipboard;
mod cursor;
mod desktop;
mod dialog;
mod follow;
mod hud;
mod icons;
mod keys;
mod panel;
mod platform;
mod prompts;
mod session;
mod settings;
mod toolbar;
mod view;
mod wayland;
mod x11;

use crate::backends::{Dirs, Kind};
use crate::options::Options;
use clipboard::Clipboard;
use dialog::Dialog;
pub use platform::Platform;
use platform::Shortcuts;
use session::{Session, Teardown};
pub use settings::Settings;
use toolbar::Placement;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{DeviceEvent, DeviceId, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

/// A connection to open, as the command line or the dialog describes one.
#[derive(Clone)]
pub struct Request {
    pub kind: &'static Kind,
    pub address: String,
    pub user: String,
    pub domain: String,
    pub password: Option<String>,
    pub options: Options,
}

/// What the window opens with.
pub struct Launch {
    /// What the command line described, if anything: the dialog is filled
    /// in from it, or — with `connect` — a session is opened on it at once.
    pub request: Option<Request>,
    pub connect: bool,
    pub dirs: Dirs,
    /// The client's own settings, from their flags.
    pub settings: Settings,
    /// The display server to open the window on, where the command line
    /// named one; `None` is winit's choice.
    pub platform: Option<Platform>,
}

/// Why the loop was woken from another thread or from egui.
enum Wake {
    /// Something happened (the backend's mailbox, or egui wanting a frame
    /// at once): repaint.
    Now,
    /// egui wants a frame later: only the deadline is looked at again, in
    /// `about_to_wait`, in case the loop was waiting without one.
    Later,
}

/// What the session needs of the window, for a frame.
pub struct Host<'a> {
    pub window: &'a Window,
    pub winit: &'a mut egui_winit::State,
    pub event_loop: &'a ActiveEventLoop,
    pub focused: bool,
    /// Where the keyboard shortcuts inhibitor is, for the debug overlay.
    pub inhibitor: &'static str,
    /// Which display server the window is on.
    pub platform: Platform,
    /// Where the pointer last was in the window, as the window saw it
    /// (not while the session holds it at the centre).
    pub pointer: Option<PhysicalPosition<f64>>,
    /// The local clipboard, where there is one (see `clipboard/`).
    pub clipboard: Option<&'a Clipboard>,
}

/// Runs the window until it is closed; the process's exit status.
pub fn run(launch: Launch) -> i32 {
    let mut builder = EventLoop::<Wake>::with_user_event();
    // Fully qualified: both traits have a `with_any_thread` too.
    match launch.platform {
        Some(Platform::X11) => {
            winit::platform::x11::EventLoopBuilderExtX11::with_x11(&mut builder);
        }
        Some(Platform::Wayland) => {
            winit::platform::wayland::EventLoopBuilderExtWayland::with_wayland(&mut builder);
        }
        _ => {}
    }
    let event_loop = match builder.build() {
        Ok(event_loop) => event_loop,
        Err(e) => {
            eprintln!("cannot open a window: {e}");
            return 1;
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App {
        launch: Some(launch),
        dirs: Dirs::default(),
        settings: Settings::default(),
        placement: Placement::default(),
        proxy,
        window: None,
        gpu: None,
        egui: egui::Context::default(),
        winit: None,
        screen: Screen::Dialog(Dialog::new()),
        shortcuts: None,
        clipboard: None,
        platform: Platform::Other,
        pointer: None,
        focused: false,
        occluded: false,
        shown: true,
        repaint_at: Arc::new(Mutex::new(None)),
        teardowns: Vec::new(),
        launched: false,
        exit_code: 0,
    };
    let ran = event_loop.run_app(&mut app);
    let exit_code = app.exit_code;
    // The window goes first, and then whatever backend is still being
    // destroyed is given a moment: a process that exits under a protocol
    // thread takes the C libraries' exit handlers down with it, but a window
    // that stays up while it waits is the freeze `Teardown` exists to avoid.
    // Hidden rather than only dropped, because on X11 dropping it queues the
    // destroy without sending it, and the window stayed on screen until the
    // process was gone. (Wayland has no hiding; there it stays up for the
    // wait, which is the rare case of a backend that would not let go.)
    let teardowns = std::mem::take(&mut app.teardowns);
    if teardowns.iter().any(|t| !t.is_finished())
        && let Some(window) = &app.window
    {
        window.set_visible(false);
    }
    drop(app);
    let deadline = Instant::now() + TEARDOWN_PATIENCE;
    for teardown in teardowns {
        if !teardown.wait_until(deadline) {
            log::warn!("a backend was still being destroyed {TEARDOWN_PATIENCE:?} after the window closed; exiting anyway");
            break;
        }
    }
    if let Err(e) = ran {
        eprintln!("the event loop failed: {e}");
        return 1;
    }
    exit_code
}

/// How long the process waits, once the window has closed, for a backend
/// that is still being destroyed.
const TEARDOWN_PATIENCE: Duration = Duration::from_secs(2);

struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: egui_wgpu::Renderer,
}

enum Screen {
    Dialog(Dialog),
    Session(Box<Session>),
}

/// What the window does once a session has ended.
#[derive(Debug, PartialEq, Eq)]
enum After {
    /// Back to the dialog, filled in, with the reason if there is one.
    Dialog,
    /// Close the window and exit, as closing it would.
    Quit,
}

/// `vento <backend> <address> --connect` is a session and nothing else, so
/// disconnecting it is the end of the program; any other ending of it —
/// the far end hanging up, an error — goes back to the dialog so that the
/// reason stays on screen, and every session after it is the dialog's.
/// `launched`: the session was the one `--connect` opened; `asked`: its
/// ending was a disconnect from this side (the toolbar's, or the Cancel
/// while connecting).
fn after_session(launched: bool, asked: bool) -> After {
    if launched && asked { After::Quit } else { After::Dialog }
}

struct App {
    launch: Option<Launch>,
    dirs: Dirs,
    /// The settings a session starts with, and what the dialog edits; a
    /// session that changed them hands them back when it ends.
    settings: Settings,
    /// Where the toolbar sits, kept from one session to the next.
    placement: Placement,
    proxy: EventLoopProxy<Wake>,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    egui: egui::Context,
    winit: Option<egui_winit::State>,
    screen: Screen,
    shortcuts: Option<Shortcuts>,
    /// The local clipboard: read when the window gets the focus, set when
    /// the far end or egui copies (see `clipboard/`).
    clipboard: Option<Clipboard>,
    /// Which display server the window is on, once it is open.
    platform: Platform,
    /// The last pointer position that was not swallowed (see `centred`).
    pointer: Option<PhysicalPosition<f64>>,
    focused: bool,
    /// X11: the window is wholly covered, as its visibility events say.
    occluded: bool,
    /// Whether the window could be seen when last looked, which is what the
    /// session is told (see `sync_shown`).
    shown: bool,
    /// When egui last asked to be repainted by.
    repaint_at: Arc<Mutex<Option<Instant>>>,
    /// The backends of sessions that have ended, still being destroyed.
    teardowns: Vec<Teardown>,
    /// The session on screen is the one `--connect` opened, which quits
    /// the window when it is disconnected (see `after_session`). Cleared
    /// when it ends, so a session started later from the dialog does not.
    launched: bool,
    exit_code: i32,
}

impl App {
    fn open(&mut self, event_loop: &ActiveEventLoop) {
        let attributes = Window::default_attributes()
            .with_title("Vento")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(e) => {
                eprintln!("cannot create the window: {e}");
                self.exit_code = 1;
                event_loop.exit();
                return;
            }
        };
        let gpu = match pollster::block_on(Gpu::new(Arc::clone(&window))) {
            Ok(gpu) => gpu,
            Err(e) => {
                eprintln!("cannot set up the GPU: {e}");
                self.exit_code = 1;
                event_loop.exit();
                return;
            }
        };
        // The toolbar's icons, Material Symbols, as a font family of their
        // own (see icons.rs).
        self.egui.add_font(icons::font_insert());
        // No animations of egui's own: a dropdown, a tooltip or a window
        // is there the frame it is asked for, at full opacity, and gone
        // the frame it is closed, and a collapsing section opens at once.
        // A fade is a frame or two of something half-drawn over a desktop
        // that is itself moving, and costs repaints at a time when the
        // remote picture is what should be getting them. Every fade and
        // fold in egui takes its length from `animation_time`, which an
        // `Area`'s fade-in and a `Window`'s fade-out read as instant at
        // zero; a programmatic scroll has its own, `scroll_animation`.
        // The toolbar's slide is not egui's — it names its own time — and
        // stays.
        self.egui.all_styles_mut(|style| {
            style.animation_time = 0.0;
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        let state = egui_winit::State::new(
            self.egui.clone(),
            egui::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            None,
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        {
            // egui's repaint requests become a deadline the loop waits for;
            // the proxy wakes it in case it is already waiting. A request
            // for later must not draw at once: one made from inside a frame
            // (the panel's once-a-second poll is one) would otherwise draw
            // the next frame straight away, and that one ask again — a loop
            // that a Wayland compositor's frame callbacks slow to the
            // refresh rate and X11 does not slow at all.
            let repaint_at = Arc::clone(&self.repaint_at);
            let proxy = self.proxy.clone();
            self.egui.set_request_repaint_callback(move |info| {
                let at = Instant::now() + info.delay;
                let mut slot = repaint_at.lock().unwrap();
                if slot.is_none_or(|t| at < t) {
                    *slot = Some(at);
                }
                let _ = proxy.send_event(if info.delay.is_zero() { Wake::Now } else { Wake::Later });
            });
        }
        self.platform = Platform::of(&window);
        log::info!("display server: {}", self.platform.name());
        if self.platform == Platform::X11 {
            // winit turns XI2's raw events off on every focus-out, and that
            // includes the ones our own keyboard grab causes, which are
            // filtered out below as not a focus change at all; relative
            // mode needs them back. The session drops motion it is not
            // capturing, so having them unfocused costs nothing.
            event_loop.listen_device_events(DeviceEvents::Always);
        }
        let proxy = self.proxy.clone();
        let wake: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            let _ = proxy.send_event(Wake::Now);
        });
        self.clipboard = Clipboard::new(&window, self.platform, wake);
        self.shortcuts = Shortcuts::new(&window);
        match &self.shortcuts {
            Some(Shortcuts::Wayland(_)) => log::info!("Wayland: looking for the keyboard shortcuts inhibitor"),
            Some(Shortcuts::X11(_)) => {
                log::info!(
                    "X11: the keyboard is grabbed while a session has focus and takes input, and not over the \
                     toolbar, unless the grab keyboard setting is off"
                )
            }
            None => log::info!("no shortcuts inhibitor on this display server"),
        }
        self.window = Some(window);
        self.gpu = Some(gpu);
        self.winit = Some(state);

        if let Some(launch) = self.launch.take() {
            self.dirs = launch.dirs;
            self.settings = launch.settings;
            if let Some(request) = launch.request {
                if launch.connect {
                    self.connect(request);
                    self.launched = matches!(self.screen, Screen::Session(_));
                } else {
                    self.screen = Screen::Dialog(Dialog::from_request(&request));
                }
            }
        }
        self.window.as_ref().unwrap().request_redraw();
    }

    /// Starts a session, or shows why one could not be started.
    fn connect(&mut self, request: Request) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        let proxy = self.proxy.clone();
        let wake: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            let _ = proxy.send_event(Wake::Now);
        });
        match Session::start(
            request.clone(),
            &self.dirs,
            self.settings,
            self.placement,
            (size.width, size.height),
            wake,
        ) {
            Ok(mut session) => {
                if !self.shown {
                    session.shown(false);
                }
                self.screen = Screen::Session(Box::new(session));
            }
            Err(e) => {
                let mut dialog = Dialog::from_request(&request);
                dialog.notice = Some(format!("Could not connect: {e}"));
                self.screen = Screen::Dialog(dialog);
            }
        }
        window.request_redraw();
    }

    fn session(&mut self) -> Option<&mut Session> {
        match &mut self.screen {
            Screen::Session(session) => Some(session),
            Screen::Dialog(_) => None,
        }
    }

    /// Keep the shortcuts inhibitor in step with the focus and the session:
    /// held while the window has the focus and the session takes input,
    /// released otherwise. Not merely connected, as it was: in view only, or
    /// under a prompt, the keys go nowhere at the far end, and the grab kept
    /// Alt+Tab and the rest from the window manager too. Released while the
    /// pointer is on the toolbar as well, and never taken with the grab
    /// keyboard setting off (`Session::holds_shortcuts`). This
    /// runs after every frame and on focus changes, but the grab and the
    /// inhibitor act only when what is wanted changes, so a pointer resting
    /// on the toolbar costs nothing.
    fn sync_shortcuts(&mut self) {
        let takes_input = matches!(&self.screen, Screen::Session(s) if s.holds_shortcuts());
        if let Some(shortcuts) = &mut self.shortcuts {
            shortcuts.pump();
            if self.focused && takes_input {
                shortcuts.inhibit();
                // A refused X grab is asked for again when due, without
                // waiting for input to wake the loop.
                if let Some(at) = shortcuts.retry_at() {
                    let mut slot = self.repaint_at.lock().unwrap();
                    if slot.is_none_or(|t| at < t) {
                        *slot = Some(at);
                    }
                }
            } else {
                shortcuts.release();
            }
        }
    }

    /// Tell the session whether the window can be seen, which is what lets
    /// a backend pause: not while it merely lacks the keyboard focus, which
    /// leaves the picture on screen, but while it is minimised or wholly
    /// covered. `minimized` is winit's `is_minimized`, asked by the caller.
    ///
    /// What can be told depends on the display server. X11 says both: the
    /// window manager's `_NET_WM_STATE_HIDDEN`, read on every frame (a
    /// minimised session still runs one a second, for the poll, so it is
    /// seen within a second, or at once with the focus-out a minimise
    /// usually brings), and the server's visibility events, which winit
    /// reports as `Occluded` — under a compositing manager the window is
    /// never reported covered, and then only minimising pauses. Wayland
    /// says neither: xdg-toplevel has no minimised state, winit 0.30 does
    /// not pass on its `suspended` state, and it reports no `Occluded`
    /// there; a hidden window's frame callbacks stop, and so do its frames,
    /// but the backend is not told and keeps receiving.
    fn sync_shown(&mut self, minimized: Option<bool>) {
        let shown = !self.occluded && minimized != Some(true);
        if shown != self.shown {
            self.shown = shown;
            if let Screen::Session(session) = &mut self.screen {
                session.shown(shown);
            }
        }
    }

    /// The inhibitor's state in a word, for the session's overlay.
    fn inhibitor(&self) -> &'static str {
        self.shortcuts.as_ref().map(Shortcuts::state).unwrap_or("none")
    }

    fn draw(&mut self, event_loop: &ActiveEventLoop) {
        let inhibitor = self.inhibitor();
        let Some(minimized) = self.window.as_ref().map(|w| w.is_minimized()) else {
            return;
        };
        self.sync_shown(minimized);
        let (Some(window), Some(gpu), Some(state)) = (self.window.clone(), self.gpu.as_mut(), self.winit.as_mut())
        else {
            return;
        };
        *self.repaint_at.lock().unwrap() = None;
        if let Some(shortcuts) = &mut self.shortcuts {
            shortcuts.pump();
        }

        // A read of the local clipboard that has come back: egui's own
        // clipboard follows it, so that Ctrl+V in a text field pastes what
        // is really there, and the session offers it to the far end.
        if let Some(text) = self.clipboard.as_ref().and_then(Clipboard::take) {
            if let Some(text) = &text {
                state.set_clipboard_text(text.clone());
            }
            if let Screen::Session(session) = &mut self.screen {
                session.local_clipboard(text);
            }
        }
        let raw_input = state.take_egui_input(&window);
        let mut next: Option<Request> = None;
        let mut ended: Option<(String, bool)> = None;
        let mut host = Host {
            window: &window,
            winit: state,
            event_loop,
            focused: self.focused,
            inhibitor,
            platform: self.platform,
            pointer: self.pointer,
            clipboard: self.clipboard.as_ref(),
        };
        let egui = self.egui.clone();
        let settings = &mut self.settings;
        let mut output = egui.run_ui(raw_input, |ui| match &mut self.screen {
            Screen::Dialog(dialog) => {
                next = dialog.ui(ui, settings);
            }
            Screen::Session(session) => {
                session.frame(ui, &mut host);
                if let Some(detail) = &session.closed {
                    ended = Some((detail.clone(), session.disconnect_asked()));
                }
            }
        });
        // What egui copied (a text field's selection) becomes the local
        // clipboard; egui-winit still keeps it as its own as well.
        if let Some(clipboard) = &self.clipboard {
            for command in &output.platform_output.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    clipboard.set(text.clone());
                }
            }
        }
        // The window's cursor: egui-winit's icon, and the far end's shape
        // over it where the session wants that — put back before, applied
        // after, so the two never fight (see `cursor.rs`).
        let icon = output.platform_output.cursor_icon;
        if let Screen::Session(session) = &mut self.screen {
            session.cursor_before(&mut host);
        }
        host.winit.handle_platform_output(&window, output.platform_output);
        if let Screen::Session(session) = &mut self.screen {
            session.cursor_after(&mut host, icon);
        }

        let pixels_per_point = output.pixels_per_point;
        let paint_jobs = egui.tessellate(output.shapes, pixels_per_point);
        let descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point,
        };
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                gpu.renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        // A minimised window shows nothing, so nothing is rendered into it:
        // the frame has still run (the events taken, the poll made) and the
        // textures still get their updates above, but there is no present
        // until the window is back, which its first expose asks a redraw
        // for. Only X11 says; elsewhere this is `None` and every frame is
        // presented as before.
        let frame = if minimized == Some(true) {
            None
        } else {
            match gpu.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    gpu.surface.configure(&gpu.device, &gpu.config);
                    window.request_redraw();
                    None
                }
                _ => None,
            }
        };
        let mut presented = None;
        if let Some(frame) = frame {
            let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("egui") });
            let commands = gpu
                .renderer
                .update_buffers(&gpu.device, &gpu.queue, &mut encoder, &paint_jobs, &descriptor);
            {
                let mut pass = encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("egui"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    })
                    .forget_lifetime();
                gpu.renderer.render(&mut pass, &paint_jobs, &descriptor);
            }
            gpu.queue.submit(commands.into_iter().chain(std::iter::once(encoder.finish())));
            window.pre_present_notify();
            gpu.queue.present(frame);
            presented = Some(Instant::now());
        }
        // The overlay's render latency ends here: the frame's pixels have
        // been submitted and the surface handed to the compositor, which
        // is as far as this end can follow them.
        if let Screen::Session(session) = &mut self.screen {
            session.presented(presented);
        }
        for id in &output.textures_delta.free {
            gpu.renderer.free_texture(id);
        }
        // Applied above; epaint asserts that nobody drops one unapplied.
        output.textures_delta.clear();

        // After the frame, so that the session's last picture was drawn.
        if let Some((detail, asked)) = ended {
            match after_session(self.launched, asked) {
                After::Quit => {
                    log::info!("the session --connect opened was disconnected; quitting");
                    self.close(event_loop);
                    return;
                }
                After::Dialog => self.end_session(&detail, event_loop),
            }
        }
        if let Some(request) = next {
            self.connect(request);
        }
        self.sync_shortcuts();
    }

    /// Back to the dialog, filled in with what was connected to, and told
    /// why it ended where there is a why.
    fn end_session(&mut self, detail: &str, event_loop: &ActiveEventLoop) {
        let inhibitor = self.inhibitor();
        let (Some(window), Some(state)) = (self.window.clone(), self.winit.as_mut()) else {
            return;
        };
        let screen = std::mem::replace(&mut self.screen, Screen::Dialog(Dialog::new()));
        if let Screen::Session(session) = screen {
            let mut dialog = Dialog::from_request(&session.request);
            if !detail.is_empty() {
                dialog.notice = Some(format!("Disconnected: {detail}"));
            }
            let mut host = Host {
                window: &window,
                winit: state,
                event_loop,
                focused: self.focused,
                inhibitor,
                platform: self.platform,
                pointer: self.pointer,
                clipboard: self.clipboard.as_ref(),
            };
            self.settings = session.settings;
            self.placement = session.placement;
            let teardown = session.shutdown(&self.egui, &mut host);
            self.reap(teardown);
            self.screen = Screen::Dialog(dialog);
        }
        self.launched = false;
        window.set_title("Vento");
        self.egui.data_mut(|d| d.remove_temp::<String>(egui::Id::new("title")));
        window.request_redraw();
    }

    fn close(&mut self, event_loop: &ActiveEventLoop) {
        let inhibitor = self.inhibitor();
        if let (Some(window), Some(state)) = (self.window.clone(), self.winit.as_mut()) {
            let screen = std::mem::replace(&mut self.screen, Screen::Dialog(Dialog::new()));
            if let Screen::Session(session) = screen {
                let mut host = Host {
                    window: &window,
                    winit: state,
                    event_loop,
                    focused: self.focused,
                    inhibitor,
                    platform: self.platform,
                    pointer: self.pointer,
                    clipboard: self.clipboard.as_ref(),
                };
                let teardown = session.shutdown(&self.egui, &mut host);
                self.reap(teardown);
            }
        }
        self.shortcuts = None;
        event_loop.exit();
    }

    /// Keeps `teardown` until it has finished, and forgets the ones that have.
    fn reap(&mut self, teardown: Teardown) {
        self.teardowns.retain(|t| !t.is_finished());
        self.teardowns.push(teardown);
    }
}

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            self.open(event_loop);
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, wake: Wake) {
        if let Wake::Now = wake
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let (Some(window), Some(state)) = (self.window.clone(), self.winit.as_mut()) else {
            return;
        };
        match &event {
            WindowEvent::CloseRequested => {
                self.close(event_loop);
                return;
            }
            WindowEvent::RedrawRequested => {
                self.draw(event_loop);
                return;
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu
                    && size.width > 0
                    && size.height > 0
                {
                    gpu.config.width = size.width;
                    gpu.config.height = size.height;
                    gpu.surface.configure(&gpu.device, &gpu.config);
                }
                if let Screen::Session(session) = &mut self.screen {
                    session.resized();
                }
            }
            WindowEvent::Moved(_) => {
                if let Screen::Session(session) = &mut self.screen {
                    session.refence();
                }
            }
            WindowEvent::Focused(focused) => {
                // On X11 the keyboard grab makes focus events of its own,
                // which are not the focus moving (see `x11.rs`): neither
                // the session nor egui hears of them.
                if !*focused && self.shortcuts.as_ref().is_some_and(|s| s.focus_still_ours(&window)) {
                    log::debug!("X11: a focus-out from our own keyboard grab; still focused");
                    return;
                }
                if *focused == self.focused {
                    // winit repeats itself around a grab.
                    return;
                }
                self.focused = *focused;
                let inhibitor = self.shortcuts.as_ref().map(Shortcuts::state).unwrap_or("none");
                let mut host = Host {
                    window: &window,
                    winit: state,
                    event_loop,
                    focused: *focused,
                    inhibitor,
                    platform: self.platform,
                    pointer: self.pointer,
                    clipboard: self.clipboard.as_ref(),
                };
                if let Screen::Session(session) = &mut self.screen {
                    session.focus(*focused, &mut host);
                }
                // What was copied elsewhere while the window was away: read
                // on the clipboard's thread, and handled when it comes back
                // (see `draw`), dialog or session.
                if *focused && let Some(clipboard) = &self.clipboard {
                    clipboard.read();
                }
                self.sync_shortcuts();
                // A minimise usually takes the focus with it: seen now
                // rather than at the next frame.
                self.sync_shown(window.is_minimized());
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = *occluded;
                self.sync_shown(window.is_minimized());
            }
            WindowEvent::KeyboardInput {
                event: key,
                is_synthetic,
                ..
            } if !is_synthetic => {
                let since = Instant::now();
                let egui = self.egui.clone();
                if let Screen::Session(session) = &mut self.screen {
                    if session.takes_keys(&egui) {
                        session.key(key, since);
                        window.request_redraw();
                        return;
                    }
                    // egui's, but the session still hears of it: a key
                    // going up here is no longer down for its chord.
                    session.key_elsewhere(key);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                // X11's capture: the pointer is hidden and held at the
                // centre, and every move away from it is undone. The far
                // end gets raw motion, which a warp does not make.
                if let Screen::Session(s) = &self.screen
                    && s.centred()
                {
                    let c = platform::centre(&window);
                    if position.x.round() as i32 != c.x || position.y.round() as i32 != c.y {
                        let _ = window.set_cursor_position(c);
                    }
                    return;
                }
                self.pointer = Some(*position);
                if let Screen::Session(session) = &mut self.screen {
                    session.pointer_event();
                }
            }
            WindowEvent::CursorLeft { .. } | WindowEvent::CursorEntered { .. }
                if matches!(&self.screen, Screen::Session(s) if s.centred()) =>
            {
                let _ = window.set_cursor_position(platform::centre(&window));
                return;
            }
            WindowEvent::MouseInput { state: pressed, button, .. } => {
                if let Screen::Session(session) = &mut self.screen
                    && session.takes_raw_mouse()
                {
                    session.mouse_button(*button, *pressed);
                    return;
                }
                if let Screen::Session(session) = &mut self.screen {
                    session.pointer_event();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Screen::Session(session) = &mut self.screen
                    && session.takes_raw_mouse()
                {
                    session.mouse_wheel(*delta);
                    return;
                }
                if let Screen::Session(session) = &mut self.screen {
                    session.pointer_event();
                }
            }
            _ => {}
        }
        let response = self.winit.as_mut().unwrap().on_window_event(&window, &event);
        if response.repaint {
            window.request_redraw();
        }
    }

    /// The clipboard goes before the event loop does: on Wayland its
    /// thread is on winit's connection, which ends with the loop, and on
    /// X11 it may have a clipboard manager to hand the text to.
    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.clipboard = None;
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && let Some(session) = self.session()
        {
            session.mouse_motion(delta.0, delta.1);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let at = *self.repaint_at.lock().unwrap();
        match at {
            Some(at) if at <= Instant::now() => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

impl Gpu {
    async fn new(window: Arc<Window>) -> Result<Gpu, String> {
        // With the display handle, so the Vulkan instance enables the
        // surface extension this windowing system needs.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(Arc::clone(&window)),
        ));
        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(|e| format!("surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("adapter: {e}"))?;
        // Everything the adapter can do rather than WebGPU's defaults, whose
        // 8192-pixel textures are half of what most hardware has; a desktop
        // past even the adapter's limit is tiled (see `desktop.rs`).
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("vento"),
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("device: {e}"))?;
        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        // A non-sRGB format, since egui does its own gamma; the first one
        // the surface offers otherwise.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .or_else(|| caps.formats.first().copied())
            .ok_or("the surface offers no format")?;
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("the surface is not supported by the adapter")?;
        config.format = format;
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&device, &config);
        let renderer = egui_wgpu::Renderer::new(
            &device,
            format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: false,
                predictable_texture_filtering: false,
            },
        );
        log::info!("wgpu: {} ({:?})", adapter.get_info().name, adapter.get_info().backend);
        Ok(Gpu {
            surface,
            device,
            queue,
            config,
            renderer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_disconnected_command_line_session_quits() {
        assert_eq!(after_session(true, true), After::Quit);
        // The far end ended it: the reason is shown, whoever started it.
        assert_eq!(after_session(true, false), After::Dialog);
        // Started from the dialog: back to it either way.
        assert_eq!(after_session(false, true), After::Dialog);
        assert_eq!(after_session(false, false), After::Dialog);
    }
}
