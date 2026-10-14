// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window: one winit event loop, one wgpu surface, egui drawn into it,
//! and either the connection dialog or a session on screen.
//!
//! winit, egui-winit and egui-wgpu are driven directly rather than through
//! eframe. eframe is less code for an ordinary application, but this one
//! needs what eframe wraps away: the window's raw handles for the shortcuts
//! inhibitor (`wayland.rs`), device events for the raw motion a locked
//! pointer sends, focus and keyboard events as winit delivers them (with
//! the layout's logical key, which egui's events drop), and a say in when
//! the loop wakes — the backend's mailbox wakes it through an event-loop
//! proxy, and egui's own repaint requests set a deadline it waits for. That
//! is four things eframe would have to be worked around for, against a page
//! of surface setup to do without it.
//!
//! The main thread never waits on a backend: a session is created
//! connecting and asked questions that return at once, and everything it
//! has to say arrives through the mailbox between frames.

mod desktop;
mod dialog;
mod follow;
mod keys;
mod panel;
mod prompts;
mod session;
mod view;
mod wayland;

use crate::backends::{Dirs, Kind};
use crate::options::Options;
use dialog::Dialog;
use session::Session;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use wayland::Shortcuts;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
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
}

/// Something happened on another thread; repaint.
struct Wake;

/// What the session needs of the window, for a frame.
pub struct Host<'a> {
    pub window: &'a Window,
    pub winit: &'a mut egui_winit::State,
    pub focused: bool,
}

/// Runs the window until it is closed; the process's exit status.
pub fn run(launch: Launch) -> i32 {
    let event_loop = match EventLoop::<Wake>::with_user_event().build() {
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
        proxy,
        window: None,
        gpu: None,
        egui: egui::Context::default(),
        winit: None,
        screen: Screen::Dialog(Dialog::new()),
        shortcuts: None,
        focused: false,
        repaint_at: Arc::new(Mutex::new(None)),
        exit_code: 0,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("the event loop failed: {e}");
        return 1;
    }
    app.exit_code
}

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

struct App {
    launch: Option<Launch>,
    dirs: Dirs,
    proxy: EventLoopProxy<Wake>,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    egui: egui::Context,
    winit: Option<egui_winit::State>,
    screen: Screen,
    shortcuts: Option<Shortcuts>,
    focused: bool,
    /// When egui last asked to be repainted by.
    repaint_at: Arc<Mutex<Option<Instant>>>,
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
            // the proxy wakes it in case it is already waiting.
            let repaint_at = Arc::clone(&self.repaint_at);
            let proxy = self.proxy.clone();
            self.egui.set_request_repaint_callback(move |info| {
                let at = Instant::now() + info.delay;
                let mut slot = repaint_at.lock().unwrap();
                if slot.is_none_or(|t| at < t) {
                    *slot = Some(at);
                }
                let _ = proxy.send_event(Wake);
            });
        }
        self.shortcuts = Shortcuts::new(&window);
        match &self.shortcuts {
            Some(_) => log::info!("Wayland: looking for the keyboard shortcuts inhibitor"),
            None => log::info!("not a Wayland window: no keyboard shortcuts inhibitor"),
        }
        self.window = Some(window);
        self.gpu = Some(gpu);
        self.winit = Some(state);

        if let Some(launch) = self.launch.take() {
            self.dirs = launch.dirs;
            if let Some(request) = launch.request {
                if launch.connect {
                    self.connect(request);
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
            let _ = proxy.send_event(Wake);
        });
        match Session::start(request.clone(), &self.dirs, (size.width, size.height), wake) {
            Ok(session) => {
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
    /// held while both, released otherwise.
    fn sync_shortcuts(&mut self) {
        let connected = matches!(&self.screen, Screen::Session(s) if s.connected());
        if let Some(shortcuts) = &mut self.shortcuts {
            shortcuts.pump();
            if self.focused && connected {
                shortcuts.inhibit();
            } else {
                shortcuts.release();
            }
        }
    }

    fn draw(&mut self) {
        let (Some(window), Some(gpu), Some(state)) = (self.window.clone(), self.gpu.as_mut(), self.winit.as_mut())
        else {
            return;
        };
        *self.repaint_at.lock().unwrap() = None;
        if let Some(shortcuts) = &mut self.shortcuts {
            shortcuts.pump();
        }

        let raw_input = state.take_egui_input(&window);
        let mut next: Option<Request> = None;
        let mut ended: Option<String> = None;
        let mut host = Host {
            window: &window,
            winit: state,
            focused: self.focused,
        };
        let egui = self.egui.clone();
        let mut output = egui.run_ui(raw_input, |ui| match &mut self.screen {
            Screen::Dialog(dialog) => {
                next = dialog.ui(ui);
            }
            Screen::Session(session) => {
                session.frame(ui, &mut host);
                if let Some(detail) = &session.closed {
                    ended = Some(detail.clone());
                }
            }
        });
        host.winit.handle_platform_output(&window, output.platform_output);

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
        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                window.request_redraw();
                None
            }
            _ => None,
        };
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
        }
        for id in &output.textures_delta.free {
            gpu.renderer.free_texture(id);
        }
        // Applied above; epaint asserts that nobody drops one unapplied.
        output.textures_delta.clear();

        // After the frame, so that the session's last picture was drawn.
        if let Some(detail) = ended {
            self.end_session(&detail);
        }
        if let Some(request) = next {
            self.connect(request);
        }
        self.sync_shortcuts();
    }

    /// Back to the dialog, filled in with what was connected to, and told
    /// why it ended where there is a why.
    fn end_session(&mut self, detail: &str) {
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
                focused: self.focused,
            };
            session.shutdown(&self.egui, &mut host);
            self.screen = Screen::Dialog(dialog);
        }
        window.set_title("Vento");
        self.egui.data_mut(|d| d.remove_temp::<String>(egui::Id::new("title")));
        window.request_redraw();
    }

    fn close(&mut self, event_loop: &ActiveEventLoop) {
        if let (Some(window), Some(state)) = (self.window.clone(), self.winit.as_mut()) {
            let screen = std::mem::replace(&mut self.screen, Screen::Dialog(Dialog::new()));
            if let Screen::Session(session) = screen {
                let mut host = Host {
                    window: &window,
                    winit: state,
                    focused: self.focused,
                };
                session.shutdown(&self.egui, &mut host);
            }
        }
        self.shortcuts = None;
        event_loop.exit();
    }
}

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            self.open(event_loop);
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, _: Wake) {
        if let Some(window) = &self.window {
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
                self.draw();
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
            WindowEvent::Focused(focused) => {
                self.focused = *focused;
                let mut host = Host {
                    window: &window,
                    winit: state,
                    focused: *focused,
                };
                if let Screen::Session(session) = &mut self.screen {
                    session.focus(*focused, &mut host);
                }
                self.sync_shortcuts();
            }
            WindowEvent::KeyboardInput {
                event: key,
                is_synthetic,
                ..
            } if !is_synthetic => {
                let egui = self.egui.clone();
                if let Screen::Session(session) = &mut self.screen
                    && session.takes_keys(&egui)
                {
                    session.key(key);
                    window.request_redraw();
                    return;
                }
            }
            WindowEvent::MouseInput { state: pressed, button, .. } => {
                if let Screen::Session(session) = &mut self.screen
                    && session.takes_raw_mouse()
                {
                    session.mouse_button(*button, *pressed);
                    return;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Screen::Session(session) = &mut self.screen
                    && session.takes_raw_mouse()
                {
                    session.mouse_wheel(*delta);
                    return;
                }
            }
            _ => {}
        }
        let response = self.winit.as_mut().unwrap().on_window_event(&window, &event);
        if response.repaint {
            window.request_redraw();
        }
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
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("vento"),
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
