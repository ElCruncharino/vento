// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Wayland's selection, read and served through `wl_data_device` on winit's
//! connection, from a thread of its own with an event queue of its own.
//!
//! winit's connection because the selection belongs to the client with the
//! keyboard focus: only that client is told what the selection is, and only
//! with the serial of one of its own input events may it set one. So the
//! window's `wl_display` is borrowed (as `wayland.rs` borrows it for the
//! shortcuts inhibitor), and this thread binds a seat, a data device on it
//! and a keyboard and pointer of its own for the serials. Its queue is read
//! the way libwayland has threads share a connection — prepare, poll, read
//! — which winit's loop does too, so neither starves the other.
//!
//! - **Reading**: a `wl_display.sync` first, so that a selection the
//!   compositor sent just before the focus (it comes "immediately before")
//!   has been dispatched here; then the selection's offer is asked for the
//!   best text type it has (`text.rs`) into a non-blocking pipe, read as it
//!   fills — modelled on xwlrvnc's `PendingReceive`. It is given up if the
//!   source writes nothing for [`READ_IDLE`], or has not finished within
//!   [`READ_DEADLINE`] of being asked, or passes [`MAX_TEXT`]; a source that
//!   is a stopped process holds the pipe open and is one of those. Our own
//!   text comes back through the same pipe from our own source.
//! - **Owning**: a `wl_data_source` offering every type in `text.rs`, its
//!   `send`s written as the pipes take them (a reader that stops for
//!   [`SEND_IDLE`] is dropped), until the compositor cancels it.
//!
//! Wayland has no clipboard manager convention; what this process owns is
//! gone when it exits, unless the compositor or a tool keeps it.

use super::text::{self, Encoding, MAX_TEXT};
use super::{Command, Link, READ_DEADLINE, READ_IDLE, poll};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wayland_client::backend::Backend;
use wayland_client::protocol::{
    wl_callback, wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source, wl_display, wl_keyboard,
    wl_pointer, wl_registry, wl_seat,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use winit::window::Window;

/// How long a paste into another client may stall before it is dropped.
const SEND_IDLE: Duration = Duration::from_secs(10);

/// The mime types an offer carries, as its `offer` events name them.
type Mimes = Mutex<Vec<String>>;

/// A read coming in through a pipe.
struct Receiving {
    seq: u64,
    fd: OwnedFd,
    mime: &'static str,
    buf: Vec<u8>,
    /// When the window asked, so the deadline covers the sync as well.
    since: Instant,
    progress_at: Instant,
}

/// Our text going out to a client that pastes it.
struct Sending {
    fd: OwnedFd,
    data: Vec<u8>,
    offset: usize,
    progress_at: Instant,
}

/// Everything the queue's events change.
struct State {
    link: Link,
    qh: QueueHandle<State>,
    display: wl_display::WlDisplay,
    seat: Option<wl_seat::WlSeat>,
    manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    device: Option<wl_data_device::WlDataDevice>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// The latest serial of an input event of ours, which setting the
    /// selection needs.
    serial: Option<u32>,
    /// The selection's offer, as the compositor last said.
    selection: Option<wl_data_offer::WlDataOffer>,
    /// A drag's offer, kept only to be destroyed.
    drag: Option<wl_data_offer::WlDataOffer>,
    /// Our source, while it is the selection.
    own: Option<wl_data_source::WlDataSource>,
    /// The read waiting for its sync.
    syncing: Option<(u64, Instant)>,
    receiving: Option<Receiving>,
    /// A read asked for while another was going, started once it ends.
    queued: Option<(u64, Instant)>,
    sending: Vec<Sending>,
}

pub struct Worker {
    queue: EventQueue<State>,
    state: State,
}

/// A pipe, both ends non-blocking and close-on-exec: (read, write).
fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors pipe2 writes.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both were just opened and are owned by nobody else.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn set_nonblocking(fd: &OwnedFd) {
    // SAFETY: fcntl on a descriptor we own.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

impl Worker {
    pub fn new(window: &Window, link: Link) -> Option<Worker> {
        let display = match window.display_handle().ok()?.as_raw() {
            RawDisplayHandle::Wayland(h) => h.display.as_ptr(),
            _ => return None,
        };
        // SAFETY: winit's live display, which outlives this worker: the
        // window stops it (`Clipboard`'s drop) before the event loop goes.
        // Foreign, so the connection is never closed from here.
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        let queue = conn.new_event_queue::<State>();
        let qh = queue.handle();
        let display = conn.display();
        display.get_registry(&qh, ());
        let state = State {
            link,
            qh,
            display,
            seat: None,
            manager: None,
            device: None,
            keyboard: None,
            pointer: None,
            serial: None,
            selection: None,
            drag: None,
            own: None,
            syncing: None,
            receiving: None,
            queued: None,
            sending: Vec::new(),
        };
        Some(Worker { queue, state })
    }

    /// The globals, and then the seat's capabilities and the device: two
    /// round trips, on this thread rather than the window's.
    fn bind(&mut self) -> bool {
        for _ in 0..2 {
            if let Err(e) = self.queue.roundtrip(&mut self.state) {
                log::warn!("clipboard: the Wayland registry failed ({e}); the clipboard stays in the window");
                return false;
            }
        }
        if self.state.device.is_none() {
            log::warn!("clipboard: the compositor has no wl_data_device_manager or seat; the clipboard stays in the window");
            return false;
        }
        true
    }

    pub fn run(mut self) {
        if !self.bind() {
            return;
        }
        let bell = self.state.link.bell.as_raw_fd();
        loop {
            if let Err(e) = self.queue.dispatch_pending(&mut self.state) {
                log::warn!("clipboard: the Wayland connection failed ({e}); the clipboard stops");
                return;
            }
            for command in self.state.link.commands() {
                match command {
                    Command::Read(seq) => self.state.read(seq, Instant::now()),
                    Command::Set(text) => self.state.set(text),
                    Command::Quit => {
                        self.state.quit();
                        let _ = self.queue.flush();
                        return;
                    }
                }
            }
            let now = Instant::now();
            self.state.pump_receive(now);
            self.state.pump_sends(now);
            let _ = self.queue.flush();
            let Some(guard) = self.queue.prepare_read() else {
                // Events came in meanwhile; dispatch them first.
                continue;
            };
            let mut fds = vec![
                libc::pollfd {
                    fd: guard.connection_fd().as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: bell,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            if let Some(r) = &self.state.receiving {
                fds.push(libc::pollfd {
                    fd: r.fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
            for s in &self.state.sending {
                fds.push(libc::pollfd {
                    fd: s.fd.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                });
            }
            let wait = self.state.next_deadline().map(|at| at.saturating_duration_since(Instant::now()));
            poll(&mut fds, wait);
            if fds[0].revents & libc::POLLIN != 0 {
                // WouldBlock is another reader having taken the bytes.
                if let Err(e) = guard.read()
                    && !matches!(&e, wayland_client::backend::WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock)
                {
                    log::warn!("clipboard: reading from the compositor failed ({e}); the clipboard stops");
                    return;
                }
            } else {
                drop(guard);
            }
        }
    }
}

impl State {
    fn next_deadline(&self) -> Option<Instant> {
        let reading = self.syncing.map(|(_, since)| since + READ_DEADLINE);
        let receiving = self
            .receiving
            .as_ref()
            .map(|r| (r.since + READ_DEADLINE).min(r.progress_at + READ_IDLE));
        let sending = self.sending.iter().map(|s| s.progress_at + SEND_IDLE).min();
        [reading, receiving, sending].into_iter().flatten().min()
    }

    // ---- reading ----------------------------------------------------------

    fn read(&mut self, seq: u64, since: Instant) {
        if self.syncing.is_some() || self.receiving.is_some() {
            self.queued = Some((seq, since));
            return;
        }
        self.display.sync(&self.qh, seq);
        self.syncing = Some((seq, since));
    }

    /// The sync is back: everything the compositor sent before the window
    /// asked has been dispatched, the selection included.
    fn synced(&mut self, seq: u64) {
        let Some((asked, since)) = self.syncing.take() else {
            return;
        };
        if asked != seq {
            return;
        }
        if let Some((seq, since)) = self.queued.take() {
            return self.read(seq, since);
        }
        let Some(offer) = &self.selection else {
            log::debug!("clipboard: no selection, nothing to read");
            return self.link.answer(seq, None);
        };
        let mimes = offer.data::<Mimes>().map(|m| m.lock().unwrap().clone()).unwrap_or_default();
        let Some(mime) = text::best(mimes.iter().map(String::as_str)) else {
            log::debug!("clipboard: the selection offers no text ({mimes:?})");
            return self.link.answer(seq, None);
        };
        let (r, w) = match pipe() {
            Ok(pair) => pair,
            Err(e) => {
                log::warn!("clipboard: no pipe to read it through ({e})");
                return self.link.answer(seq, None);
            }
        };
        log::debug!("clipboard: reading it as {mime}");
        offer.receive(mime.to_string(), w.as_fd());
        // The request holds its own copy of the write end; ours closes so
        // that the source's closing it is the end of the text.
        drop(w);
        let now = Instant::now();
        self.receiving = Some(Receiving {
            seq,
            fd: r,
            mime,
            buf: Vec::new(),
            since,
            progress_at: now,
        });
    }

    /// Whatever the pipe holds now, and the end of the read if it is over.
    fn pump_receive(&mut self, now: Instant) {
        if let Some((_, since)) = self.syncing
            && since + READ_DEADLINE <= now
        {
            // A compositor that does not answer a sync is not one to read
            // from either; the late answer finds nothing waiting.
            let (seq, _) = self.syncing.take().unwrap();
            log::warn!("clipboard: the compositor did not answer in time; nothing read");
            self.link.answer(seq, None);
            return self.next();
        }
        let Some(r) = &mut self.receiving else {
            return;
        };
        let mut chunk = [0u8; 64 * 1024];
        let end = loop {
            // SAFETY: reading at most `chunk.len()` bytes into `chunk`.
            let n = unsafe { libc::read(r.fd.as_raw_fd(), chunk.as_mut_ptr().cast(), chunk.len()) };
            if n > 0 {
                r.buf.extend_from_slice(&chunk[..n as usize]);
                r.progress_at = now;
                if r.buf.len() > MAX_TEXT {
                    break Some(Err(format!("the text is over {MAX_TEXT} bytes")));
                }
                continue;
            }
            if n == 0 {
                break Some(Ok(()));
            }
            match std::io::Error::last_os_error().kind() {
                std::io::ErrorKind::Interrupted => continue,
                std::io::ErrorKind::WouldBlock => {}
                _ => break Some(Ok(())),
            }
            if now >= r.since + READ_DEADLINE {
                break Some(Err(format!("the source did not finish within {} s", READ_DEADLINE.as_secs_f32())));
            }
            if now >= r.progress_at + READ_IDLE {
                break Some(Err(format!("the source wrote nothing for {} s", READ_IDLE.as_secs_f32())));
            }
            break None;
        };
        let Some(end) = end else {
            return;
        };
        let r = self.receiving.take().unwrap();
        match end {
            Ok(()) => self.link.answer(r.seq, Some(text::decode(r.mime, &r.buf))),
            Err(why) => {
                log::warn!("clipboard: gave up reading it after {} bytes: {why}", r.buf.len());
                self.link.answer(r.seq, None);
            }
        }
        self.next();
    }

    /// The read asked for while one was going, now that it is over.
    fn next(&mut self) {
        if let Some((seq, since)) = self.queued.take() {
            self.read(seq, since);
        }
    }

    // ---- owning -----------------------------------------------------------

    fn set(&mut self, text: String) {
        let (Some(manager), Some(device)) = (&self.manager, &self.device) else {
            return;
        };
        let Some(serial) = self.serial else {
            log::warn!("clipboard: no input event of ours yet to set the selection with");
            return;
        };
        let source = manager.create_data_source(&self.qh, Arc::new(text));
        for (name, _, _) in text::SERVED {
            source.offer(name.to_string());
        }
        device.set_selection(Some(&source), serial);
        log::debug!("clipboard: offered, {} bytes", source.data::<Arc<String>>().map_or(0, |t| t.len()));
        if let Some(old) = self.own.replace(source) {
            old.destroy();
        }
    }

    fn pump_sends(&mut self, now: Instant) {
        self.sending.retain_mut(|s| {
            while s.offset < s.data.len() {
                let rest = &s.data[s.offset..];
                // SAFETY: writing at most `rest.len()` bytes from `rest`.
                let n = unsafe { libc::write(s.fd.as_raw_fd(), rest.as_ptr().cast(), rest.len()) };
                if n > 0 {
                    s.offset += n as usize;
                    s.progress_at = now;
                    continue;
                }
                if n == 0 {
                    return false;
                }
                match std::io::Error::last_os_error().kind() {
                    std::io::ErrorKind::Interrupted => continue,
                    // Full: on, unless it has stayed full too long.
                    std::io::ErrorKind::WouldBlock => return now < s.progress_at + SEND_IDLE,
                    // The reader went away (EPIPE), or worse.
                    _ => return false,
                }
            }
            false
        });
    }

    fn quit(&mut self) {
        if let Some(source) = self.own.take() {
            source.destroy();
        }
        for offer in [self.selection.take(), self.drag.take()].into_iter().flatten() {
            offer.destroy();
        }
        if let Some(device) = self.device.take() {
            device.release();
        }
        if let Some(keyboard) = self.keyboard.take() {
            keyboard.release();
        }
        if let Some(pointer) = self.pointer.take() {
            pointer.release();
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut State,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_data_device_manager" if state.manager.is_none() => {
                    state.manager = Some(registry.bind(name, version.min(3), qh, ()));
                }
                // The first seat, as for the shortcuts inhibitor.
                "wl_seat" if state.seat.is_none() => {
                    state.seat = Some(registry.bind(name, version.min(5), qh, ()));
                }
                _ => {}
            }
            if let (Some(manager), Some(seat), None) = (&state.manager, &state.seat, &state.device) {
                state.device = Some(manager.get_data_device(seat, qh, ()));
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut State,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
        }
    }
}

/// Only the serials: winit has the keyboard and the pointer themselves.
impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut State,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        match event {
            wl_keyboard::Event::Enter { serial, .. } | wl_keyboard::Event::Key { serial, .. } => {
                state.serial = Some(serial);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut State,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wl_pointer::Event::Button { serial, .. } = event {
            state.serial = Some(serial);
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    fn event(
        state: &mut State,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        match event {
            wl_data_device::Event::Selection { id } => {
                if let Some(old) = std::mem::replace(&mut state.selection, id) {
                    old.destroy();
                }
            }
            wl_data_device::Event::Enter { id, .. } => {
                if let Some(old) = std::mem::replace(&mut state.drag, id) {
                    old.destroy();
                }
            }
            wl_data_device::Event::Leave | wl_data_device::Event::Drop => {
                if let Some(old) = state.drag.take() {
                    old.destroy();
                }
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(State, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, Mimes::default()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, Mimes> for State {
    fn event(
        _: &mut State,
        _: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        mimes: &Mimes,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            mimes.lock().unwrap().push(mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, Arc<String>> for State {
    fn event(
        state: &mut State,
        source: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        text: &Arc<String>,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                let encoding = text::served(&mime_type).map_or(Encoding::Utf8, |(_, e)| e);
                set_nonblocking(&fd);
                state.sending.push(Sending {
                    fd,
                    data: text::encode(text, encoding),
                    offset: 0,
                    progress_at: Instant::now(),
                });
            }
            wl_data_source::Event::Cancelled => {
                if state.own.as_ref() == Some(source) {
                    log::debug!("clipboard: another client owns it now");
                    state.own = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, u64> for State {
    fn event(
        state: &mut State,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        seq: &u64,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.synced(*seq);
        }
    }
}

wayland_client::delegate_noop!(State: ignore wl_data_device_manager::WlDataDeviceManager);
