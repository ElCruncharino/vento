// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The local clipboard, owned here rather than by egui-winit, read and
//! served on a thread of its own so that the window never waits on it.
//!
//! egui-winit's clipboard (arboard on X11, smithay-clipboard on Wayland) is
//! read on the main thread and waits for the owner: arboard tried six
//! formats one after the other with four seconds each, so an owner that
//! never answered froze the window for 24 s on every focus-in, and
//! smithay's read had no limit at all. Neither looked at what an owner
//! offers before asking (a Latin-1-only owner gave nothing), arboard
//! refused text that was not valid UTF-8, and it advertised formats it then
//! refused to convert. So egui-winit is built without its clipboard, and
//! this is the clipboard instead:
//!
//! - A **read** is asked for and answered later: the answer comes back
//!   through [`Clipboard::take`], with the event loop woken for it. One
//!   deadline bounds it ([`READ_DEADLINE`]), after which it is given up and
//!   answered with nothing; on Wayland a source that goes quiet for
//!   [`READ_IDLE`] is given up sooner. A read asked for while one is going
//!   is started once that one ends, and only its answer is given.
//! - **Setting** the text makes this process the owner, served from the
//!   same thread in every format in `text.rs`, until another client takes
//!   it over.
//!
//! egui keeps a clipboard of its own inside egui-winit (a `String`, with
//! the feature off), which is what its Ctrl+V pastes into a text field; the
//! window keeps it in step with every read and every set.
//!
//! The X11 side is `x11.rs`, on a connection of its own; the Wayland side
//! is `wayland.rs`, on winit's connection with an event queue of its own,
//! since a Wayland selection may only be set by the client with the focus.

mod text;
mod wayland;
mod x11;

use super::Platform;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use winit::window::Window;

/// The longest a read of the local clipboard may take, from being asked
/// for to its last byte. An owner on the same machine answers in
/// milliseconds; one that has not in this time is hung, or is a stopped
/// process, and the window is not made to wait for it.
pub const READ_DEADLINE: Duration = Duration::from_secs(2);

/// Wayland: the longest a source may write nothing before the read is
/// given up, within [`READ_DEADLINE`].
pub const READ_IDLE: Duration = Duration::from_secs(1);

/// How long, when the window closes owning the clipboard, a clipboard
/// manager is given to take a copy (X11's `SAVE_TARGETS`).
const HANDOFF: Duration = Duration::from_secs(2);

/// What the window asks of the clipboard's thread.
enum Command {
    /// Read the clipboard; the answer is tagged with this number.
    Read(u64),
    /// Own the clipboard, with this text.
    Set(String),
    /// Hand the text to a clipboard manager if there is one, and stop.
    Quit,
}

/// The latest answer, and whom to wake for it.
struct Answer {
    slot: Mutex<Option<(u64, Option<String>)>>,
    wake: Box<dyn Fn() + Send + Sync>,
}

/// The thread's side: its commands, the socket the window rings when there
/// are some, and where its answers go.
struct Link {
    commands: Receiver<Command>,
    bell: UnixStream,
    answer: Arc<Answer>,
}

impl Link {
    /// Every command waiting, the bell drained with them.
    fn commands(&self) -> Vec<Command> {
        use std::io::Read;
        let mut buf = [0u8; 64];
        while matches!((&self.bell).read(&mut buf), Ok(n) if n > 0) {}
        self.commands.try_iter().collect()
    }

    fn answer(&self, seq: u64, text: Option<String>) {
        *self.answer.slot.lock().unwrap() = Some((seq, text));
        (self.answer.wake)();
    }
}

pub struct Clipboard {
    commands: Sender<Command>,
    bell: UnixStream,
    /// The number of the latest read asked for: only its answer is taken.
    latest: AtomicU64,
    answer: Arc<Answer>,
    thread: Option<JoinHandle<()>>,
    /// Sent when the thread is done, so that the window waits for it with a
    /// limit rather than joining it.
    done: Receiver<()>,
}

impl Clipboard {
    /// The clipboard for the display server `window` is on, or `None` where
    /// there is none to be had (the log says why). `wake` is called from the
    /// clipboard's thread whenever an answer is ready.
    pub fn new(window: &Window, platform: Platform, wake: Box<dyn Fn() + Send + Sync>) -> Option<Clipboard> {
        let (tx, rx) = channel();
        let (ours, theirs) = match UnixStream::pair() {
            Ok(pair) => pair,
            Err(e) => {
                log::warn!("clipboard: no socket pair for its thread ({e}); the clipboard stays in the window");
                return None;
            }
        };
        let _ = ours.set_nonblocking(true);
        let _ = theirs.set_nonblocking(true);
        let answer = Arc::new(Answer {
            slot: Mutex::new(None),
            wake,
        });
        let link = Link {
            commands: rx,
            bell: theirs,
            answer: Arc::clone(&answer),
        };
        let (done_tx, done) = channel();
        // Set up on its own thread too, so that not even the first round
        // trips are the window's; a setup that fails says so in the log,
        // and reads then go unanswered, leaving egui's own clipboard.
        let body: Box<dyn FnOnce() + Send> = match platform {
            Platform::X11 => Box::new(move || {
                if let Some(worker) = x11::Worker::new(link) {
                    worker.run();
                }
            }),
            Platform::Wayland => {
                let worker = wayland::Worker::new(window, link)?;
                Box::new(move || worker.run())
            }
            Platform::Other => return None,
        };
        let thread = std::thread::Builder::new()
            .name("vento-clipboard".into())
            .spawn(move || {
                body();
                let _ = done_tx.send(());
            })
            .map_err(|e| log::warn!("clipboard: cannot start its thread ({e})"))
            .ok()?;
        Some(Clipboard {
            commands: tx,
            bell: ours,
            latest: AtomicU64::new(0),
            answer,
            thread: Some(thread),
            done,
        })
    }

    fn send(&self, command: Command) {
        use std::io::Write;
        if self.commands.send(command).is_ok() {
            // Full only if the thread has a backlog of rings to read, in
            // which case it will see this command with them.
            let _ = (&self.bell).write(&[1]);
        }
    }

    /// Ask for what the clipboard holds; [`take`](Self::take) has it later.
    pub fn read(&self) {
        let seq = self.latest.fetch_add(1, Ordering::Relaxed) + 1;
        self.send(Command::Read(seq));
    }

    /// Make this process the clipboard's owner, holding `text`.
    pub fn set(&self, text: String) {
        self.send(Command::Set(text));
    }

    /// The answer to the latest read, once: `Some(None)` where there was no
    /// text, or it could not be had in time.
    pub fn take(&self) -> Option<Option<String>> {
        let mut slot = self.answer.slot.lock().unwrap();
        match slot.take() {
            Some((seq, text)) if seq == self.latest.load(Ordering::Relaxed) => Some(text),
            _ => None,
        }
    }
}

impl Drop for Clipboard {
    /// The thread is told to stop and given [`HANDOFF`] and a little more,
    /// for a clipboard manager to copy what this process owns. On Wayland
    /// the connection it uses is winit's, so this must happen while the
    /// event loop is still there (see `App::exiting`).
    fn drop(&mut self) {
        self.send(Command::Quit);
        if self.done.recv_timeout(HANDOFF + Duration::from_millis(500)).is_ok()
            && let Some(thread) = self.thread.take()
        {
            let _ = thread.join();
        }
    }
}

/// Polls `fds` for up to `timeout` (forever with `None`), going round an
/// interrupted call; the revents are left in `fds`.
fn poll(fds: &mut [libc::pollfd], timeout: Option<Duration>) {
    // Rounded up, so that a deadline is not polled for again just short of it.
    let ms = timeout.map_or(-1, |t| t.as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as i32);
    // SAFETY: `fds` is a live slice of pollfd for the length passed.
    let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
    if n < 0 {
        for fd in fds.iter_mut() {
            fd.revents = 0;
        }
    }
}
