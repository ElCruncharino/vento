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
//!   [`READ_IDLE`] is given up sooner. A read is asked for with its
//!   reason ([`Why`]): the focus coming back, whose answer the session
//!   offers to the far end, or a Ctrl+V, whose answer is pasted. One asked
//!   for while another is going is started once that one ends, and takes
//!   over its reasons, so only the newest text is given, for every reason
//!   it was asked.
//! - **Setting** the text makes this process the owner, served from the
//!   same thread in every format in `text.rs`, until another client takes
//!   it over.
//!
//! egui keeps a clipboard of its own inside egui-winit (a `String`, with
//! the feature off), which its Ctrl+V would paste into a text field. That
//! is the clipboard as of the last read, stale if something copied since
//! without the focus moving (a clipboard sync tool, a script, a hotkey), so
//! the window takes the paste command itself ([`is_paste`]), reads, and
//! hands egui the paste when the text is in. The `String` is still kept in
//! step with every read and every set.
//!
//! The X11 side is `x11.rs`, on a connection of its own; the Wayland side
//! is `wayland.rs`, on winit's connection with an event queue of its own,
//! since a Wayland selection may only be set by the client with the focus.

mod text;
mod wayland;
mod x11;

use super::DisplayServer;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
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

/// Why a read was asked for: to offer the text to the far end (the
/// focus-in read), to paste it into a text field (Ctrl+V), or both. A read
/// asked for while another is going is folded into the next one, keeping
/// every reason, and so are answers the window has not yet taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Why {
    pub offer: bool,
    pub paste: bool,
}

impl Why {
    pub const OFFER: Why = Why {
        offer: true,
        paste: false,
    };
    pub const PASTE: Why = Why {
        offer: false,
        paste: true,
    };

    fn and(self, other: Why) -> Why {
        Why {
            offer: self.offer || other.offer,
            paste: self.paste || other.paste,
        }
    }
}

/// A read asked for and not yet started: why, and when it was asked, which
/// is what its deadline runs from.
#[derive(Clone, Copy)]
struct Asked {
    why: Why,
    at: Instant,
}

impl Asked {
    /// Two reads folded into one: both reasons, and the later time, so that
    /// the newest asker is given its whole deadline.
    fn and(self, other: Asked) -> Asked {
        Asked {
            why: self.why.and(other.why),
            at: self.at.max(other.at),
        }
    }
}

/// `queued` with `asked` folded in.
fn fold(queued: Option<Asked>, asked: Asked) -> Option<Asked> {
    Some(queued.map_or(asked, |q| q.and(asked)))
}

/// What the window asks of the clipboard's thread.
enum Command {
    /// Read the clipboard.
    Read(Asked),
    /// Own the clipboard, with this text.
    Set(String),
    /// Hand the text to a clipboard manager if there is one, and stop.
    Quit,
}

/// The latest answer, and whom to wake for it.
struct Answer {
    slot: Mutex<Option<(Option<String>, Why)>>,
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

    /// The newest text, for every reason not yet taken.
    fn answer(&self, why: Why, text: Option<String>) {
        let mut slot = self.answer.slot.lock().unwrap();
        let why = slot.take().map_or(why, |(_, untaken)| why.and(untaken));
        *slot = Some((text, why));
        drop(slot);
        (self.answer.wake)();
    }
}

pub struct Clipboard {
    commands: Sender<Command>,
    bell: UnixStream,
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
    pub fn new(window: &Window, display_server: DisplayServer, wake: Box<dyn Fn() + Send + Sync>) -> Option<Clipboard> {
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
        let body: Box<dyn FnOnce() + Send> = match display_server {
            DisplayServer::X11 => Box::new(move || {
                if let Some(worker) = x11::Worker::new(link) {
                    worker.run();
                }
            }),
            DisplayServer::Wayland => {
                let worker = wayland::Worker::new(window, link)?;
                Box::new(move || worker.run())
            }
            DisplayServer::Other => return None,
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
    pub fn read(&self, why: Why) {
        self.send(Command::Read(Asked { why, at: Instant::now() }));
    }

    /// Make this process the clipboard's owner, holding `text`.
    pub fn set(&self, text: String) {
        self.send(Command::Set(text));
    }

    /// The latest answer, once, with every reason it answers: a text of
    /// `None` where there was none, or it could not be had in time.
    pub fn take(&self) -> Option<(Option<String>, Why)> {
        self.answer.slot.lock().unwrap().take()
    }
}

impl Clipboard {
    /// The thread is told to stop and given [`HANDOFF`] and a little more,
    /// for a clipboard manager to copy what this process owns: whether it
    /// stopped in that time. On Wayland the connection it uses is winit's,
    /// so this must happen while the event loop is still there (see
    /// `App::exiting`), and a thread that did not stop is still on that
    /// connection: the window then keeps it open to the end of the process
    /// (see `run`), since winit closes it with the last window and the
    /// thread would go on into a freed `wl_display`.
    pub fn stop(mut self) -> bool {
        self.quit()
    }

    fn quit(&mut self) -> bool {
        let Some(thread) = self.thread.take() else {
            return true;
        };
        self.send(Command::Quit);
        if self.done.recv_timeout(HANDOFF + Duration::from_millis(500)).is_ok() {
            let _ = thread.join();
            true
        } else {
            false
        }
    }
}

impl Drop for Clipboard {
    /// [`stop`](Clipboard::stop), where nothing asks how it went: a
    /// clipboard replaced while the event loop runs, whose connection
    /// stays open anyway.
    fn drop(&mut self) {
        self.quit();
    }
}

/// Whether a key press is egui-winit's paste command, which the window
/// takes for itself (see `App::window_event`): Ctrl+V by the layout's
/// letter, or by the key where a V would be on a layout whose letters are
/// not Latin, or the Paste key. egui-winit's own rule, which keeps the
/// layout's letter where it has one, so Ctrl+K on Dvorak's V key is not a
/// paste.
pub fn is_paste(ctrl: bool, logical: &Key, physical: PhysicalKey) -> bool {
    match logical {
        Key::Named(NamedKey::Paste) => true,
        _ if !ctrl => false,
        Key::Character(c) if c.eq_ignore_ascii_case("v") => true,
        Key::Character(c) if c.is_ascii() => false,
        _ => physical == PhysicalKey::Code(KeyCode::KeyV),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn letter(c: &str) -> Key {
        Key::Character(c.into())
    }

    #[test]
    fn paste_is_ctrl_v_by_the_layout_s_letter() {
        let v = PhysicalKey::Code(KeyCode::KeyV);
        let k = PhysicalKey::Code(KeyCode::KeyK);
        assert!(is_paste(true, &letter("v"), v));
        assert!(is_paste(true, &letter("V"), v));
        assert!(!is_paste(false, &letter("v"), v));
        // Dvorak: the V key is a K, and Ctrl+K is not a paste; its V is.
        assert!(!is_paste(true, &letter("k"), v));
        assert!(is_paste(true, &letter("v"), k));
        // Cyrillic: no Latin letter, so the key where V is.
        assert!(is_paste(true, &letter("\u{43c}"), v));
        assert!(!is_paste(true, &letter("\u{43c}"), k));
        assert!(is_paste(false, &Key::Named(NamedKey::Paste), k));
    }

    #[test]
    fn folded_reads_keep_every_reason_and_the_later_time() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(5);
        let a = Asked { why: Why::OFFER, at: t0 };
        let b = Asked { why: Why::PASTE, at: t1 };
        let f = fold(Some(a), b).unwrap();
        assert_eq!(f.why, Why { offer: true, paste: true });
        assert_eq!(f.at, t1);
        assert_eq!(fold(None, a).unwrap().why, Why::OFFER);
    }
}
