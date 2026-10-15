// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! X11's `CLIPBOARD`, read and served on a connection of its own, by one
//! thread that never blocks on another client.
//!
//! Its own connection rather than winit's, because a selection is a
//! conversation of events (`SelectionNotify`, `SelectionRequest`,
//! `PropertyNotify`) and winit owns its connection's event queue; requests
//! that only the server answers (an atom, a property) are waited for, which
//! the server does at once. Everything that waits on another client is a
//! state with a deadline, worked through by one loop that polls the
//! connection and the window's bell:
//!
//! - **Reading**: `TARGETS` is asked for first, and the best text target in
//!   it (`text.rs`) after; an owner that refuses `TARGETS` is asked for
//!   `UTF8_STRING` and then `STRING` straight out. What arrives is decoded
//!   by the type it came as, and an `INCR` transfer is received chunk by
//!   chunk. The whole of it has one deadline ([`READ_DEADLINE`]), after which
//!   the read is answered with nothing and a late answer is ignored.
//! - **Owning**: the selection is taken with a real timestamp (got from a
//!   property change on our window, as the ICCCM asks) and every target in
//!   `text.rs` is served, plus `TARGETS` and `TIMESTAMP`; `MULTIPLE` is not.
//!   Text up to [`ONE_PROPERTY`] (or the server's largest request, if
//!   smaller) goes in one property and anything longer by `INCR`, in steps
//!   of the same size, each transfer dropped if its requestor stops
//!   deleting for [`SEND_IDLE`]. A `SelectionClear` ends the ownership.
//! - **Exiting** while owning hands the text to a clipboard manager, where
//!   one holds `CLIPBOARD_MANAGER`, by asking it to convert `SAVE_TARGETS`
//!   and serving it meanwhile, for up to [`HANDOFF`].

use super::text::{self, MAX_TEXT};
use super::{Asked, Command, HANDOFF, Link, READ_DEADLINE, Why, fold, poll};
use std::collections::HashMap;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Arc;
use std::time::{Duration, Instant};
use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property,
    SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;
use x11rb::{CURRENT_TIME, NONE};

/// The most text put in one property; more goes by `INCR`.
const ONE_PROPERTY: usize = 1 << 20;

/// How long an `INCR` transfer to a requestor may wait for it to take the
/// last chunk before it is dropped.
const SEND_IDLE: Duration = Duration::from_secs(10);

/// The atoms this side uses, by name; the text formats are in `text.rs`.
const NAMES: &[&str] = &[
    "CLIPBOARD",
    "TARGETS",
    "TIMESTAMP",
    "INCR",
    "CLIPBOARD_MANAGER",
    "SAVE_TARGETS",
    // Our window's properties: where conversions land, and the one changed
    // to learn the server's time.
    "_VENTO_CLIPBOARD",
    "_VENTO_CLIPBOARD_TIME",
];

struct Atoms {
    by_name: HashMap<&'static str, Atom>,
    by_atom: HashMap<Atom, &'static str>,
}

impl Atoms {
    fn get(&self, name: &str) -> Atom {
        self.by_name[name]
    }

    /// The name of one of ours, or `None` for an atom we never interned.
    fn name(&self, atom: Atom) -> Option<&'static str> {
        self.by_atom.get(&atom).copied()
    }
}

/// Where a read is.
enum Step {
    /// `TARGETS` asked for.
    Targets,
    /// A target asked for straight out, an owner having refused `TARGETS`;
    /// the index into [`BLIND`] of the one asked.
    Blind(usize),
    /// The chosen target asked for.
    Data,
    /// An `INCR` transfer coming in, as this type.
    Incr { type_: Atom, buf: Vec<u8> },
}

/// What an owner that does not answer `TARGETS` is asked for, in turn.
const BLIND: &[&str] = &["UTF8_STRING", "STRING"];

struct Reading {
    why: Why,
    deadline: Instant,
    step: Step,
    /// The target last asked for: an answer for another is a late one, to a
    /// read given up on.
    asked: Atom,
}

/// The text we own, and the time we took it at.
struct Owned {
    text: Arc<String>,
    time: u32,
}

/// One `INCR` transfer to a requestor.
struct Sending {
    requestor: Window,
    property: Atom,
    type_: Atom,
    data: Vec<u8>,
    offset: usize,
    /// Whether the zero-length chunk that ends it has been written.
    ended: bool,
    progress_at: Instant,
}

pub struct Worker {
    conn: XCBConnection,
    window: Window,
    atoms: Atoms,
    link: Link,
    /// The text, waiting for the timestamp to take the selection with.
    claiming: Option<String>,
    owned: Option<Owned>,
    reading: Option<Reading>,
    /// A read asked for while another was going, started once it ends.
    queued: Option<Asked>,
    sending: Vec<Sending>,
    /// The most bytes put in one property.
    chunk: usize,
    /// Exiting, and waiting until then for a clipboard manager.
    handing_off: Option<Instant>,
}

impl Worker {
    /// Connects and makes the window the conversations need, or `None` (with
    /// the reason in the log) where the X server cannot be reached.
    pub fn new(link: Link) -> Option<Worker> {
        let (conn, screen) = XCBConnection::connect(None)
            .map_err(|e| log::warn!("clipboard: cannot connect to the X server ({e}); the clipboard stays in the window"))
            .ok()?;
        let fail = |e: &dyn std::fmt::Display| log::warn!("clipboard: X11 setup failed ({e}); the clipboard stays in the window");
        let root = conn.setup().roots[screen].root;
        let window = conn.generate_id().map_err(|e| fail(&e)).ok()?;
        // Unmapped, input-only: a requestor and owner, nothing on screen.
        // PropertyChange for the INCR chunks coming in and the timestamp.
        conn.create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(|e| fail(&e))
        .ok()?;
        let names: Vec<&'static str> = NAMES.iter().chain(text::READ_ORDER.iter()).copied().collect();
        let cookies: Vec<_> = names
            .iter()
            .map(|n| conn.intern_atom(false, n.as_bytes()))
            .collect::<Result<_, _>>()
            .map_err(|e| fail(&e))
            .ok()?;
        let mut atoms = Atoms {
            by_name: HashMap::new(),
            by_atom: HashMap::new(),
        };
        for (name, cookie) in names.into_iter().zip(cookies) {
            let atom = cookie.reply().map_err(|e| fail(&e)).ok()?.atom;
            atoms.by_name.insert(name, atom);
            atoms.by_atom.insert(atom, name);
        }
        let chunk = ONE_PROPERTY.min(conn.maximum_request_bytes().saturating_sub(64)).max(4096);
        conn.flush().map_err(|e| fail(&e)).ok()?;
        Some(Worker {
            conn,
            window,
            atoms,
            link,
            claiming: None,
            owned: None,
            reading: None,
            queued: None,
            sending: Vec::new(),
            chunk,
            handing_off: None,
        })
    }

    pub fn run(mut self) {
        let fd: RawFd = self.conn.as_raw_fd();
        let bell = self.link.bell.as_raw_fd();
        loop {
            // Events first and until there are none, since a reply waited
            // for may have brought some in behind it, and a poll of the
            // socket would not see those.
            loop {
                match self.conn.poll_for_event() {
                    Ok(Some(event)) => self.event(event),
                    Ok(None) => break,
                    Err(e) => {
                        log::warn!("clipboard: the X connection failed ({e}); the clipboard stops");
                        return;
                    }
                }
            }
            for command in self.link.commands() {
                self.command(command);
            }
            let now = Instant::now();
            self.expire(now);
            if self.handing_off.is_some_and(|until| until <= now) || (self.handing_off.is_some() && self.owned.is_none()) {
                return;
            }
            if self.conn.flush().is_err() {
                return;
            }
            // Something done above may have queued events in turn.
            match self.conn.poll_for_event() {
                Ok(Some(event)) => {
                    self.event(event);
                    continue;
                }
                Ok(None) => {}
                Err(_) => return,
            }
            let mut fds = [
                libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: bell,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let wait = self.next_deadline().map(|at| at.saturating_duration_since(Instant::now()));
            poll(&mut fds, wait);
            if fds[0].revents & (libc::POLLERR | libc::POLLHUP) != 0 {
                log::warn!("clipboard: the X connection closed; the clipboard stops");
                return;
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let reading = self.reading.as_ref().map(|r| r.deadline);
        let sending = self.sending.iter().map(|s| s.progress_at + SEND_IDLE).min();
        [reading, sending, self.handing_off].into_iter().flatten().min()
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Read(asked) => {
                if self.reading.is_some() {
                    self.queued = fold(self.queued, asked);
                } else {
                    self.start_read(asked);
                }
            }
            Command::Set(text) => {
                // The time to take the selection at is the server's, and
                // a property change is how a client learns it.
                self.claiming = Some(text);
                let stamp = self.atoms.get("_VENTO_CLIPBOARD_TIME");
                let _ = self
                    .conn
                    .change_property8(PropMode::APPEND, self.window, stamp, AtomEnum::INTEGER, &[]);
            }
            Command::Quit => self.hand_off(),
        }
    }

    // ---- reading ----------------------------------------------------------

    fn start_read(&mut self, asked: Asked) {
        let clipboard = self.atoms.get("CLIPBOARD");
        // Our own text needs no conversation.
        if let Some(text) = self.claiming.clone().or_else(|| self.owned.as_ref().map(|o| o.text.to_string())) {
            self.link.answer(asked.why, Some(text));
            return;
        }
        let owner = self.conn.get_selection_owner(clipboard).ok().and_then(|c| c.reply().ok()).map(|r| r.owner);
        if owner.is_none_or(|o| o == NONE) {
            log::debug!("clipboard: no owner, nothing to read");
            self.link.answer(asked.why, None);
            return;
        }
        self.reading = Some(Reading {
            why: asked.why,
            deadline: asked.at + READ_DEADLINE,
            step: Step::Targets,
            asked: NONE,
        });
        self.convert(self.atoms.get("TARGETS"));
    }

    fn convert(&mut self, target: Atom) {
        if let Some(reading) = &mut self.reading {
            reading.asked = target;
        }
        let _ = self.conn.convert_selection(
            self.window,
            self.atoms.get("CLIPBOARD"),
            target,
            self.atoms.get("_VENTO_CLIPBOARD"),
            CURRENT_TIME,
        );
    }

    /// Ends the read in hand with `text`, and starts the one queued.
    fn finish(&mut self, text: Option<String>) {
        let Some(reading) = self.reading.take() else {
            return;
        };
        let _ = self.conn.delete_property(self.window, self.atoms.get("_VENTO_CLIPBOARD"));
        // A read asked for meanwhile supersedes this one: the window wants
        // what the clipboard holds now, and only that answer is taken.
        // Its reasons go with it.
        if let Some(queued) = self.queued.take() {
            self.start_read(Asked {
                why: queued.why.and(reading.why),
                ..queued
            });
        } else {
            self.link.answer(reading.why, text);
        }
    }

    /// The property a conversion left on our window, whole, and deleted:
    /// its type and its bytes, or `None` where there was none or it was
    /// over [`MAX_TEXT`].
    fn take_property(&self) -> Option<(Atom, Vec<u8>)> {
        let property = self.atoms.get("_VENTO_CLIPBOARD");
        let length = (MAX_TEXT / 4 + 1) as u32;
        let reply = self
            .conn
            .get_property(true, self.window, property, AtomEnum::ANY, 0, length)
            .ok()?
            .reply()
            .ok()?;
        if reply.bytes_after > 0 {
            log::warn!("clipboard: the owner's text is over {MAX_TEXT} bytes; not read");
            let _ = self.conn.delete_property(self.window, property);
            return None;
        }
        if reply.type_ == NONE {
            return None;
        }
        Some((reply.type_, reply.value))
    }

    fn selection_notify(&mut self, e: SelectionNotifyEvent) {
        if e.requestor != self.window || e.selection != self.atoms.get("CLIPBOARD") {
            return;
        }
        let Some(reading) = self.reading.as_ref().filter(|r| r.asked == e.target) else {
            // The answer to a read given up on. Its property goes too, unless
            // another read is under way, whose answer may be in it by now.
            if self.reading.is_none() {
                let _ = self.conn.delete_property(self.window, self.atoms.get("_VENTO_CLIPBOARD"));
            }
            return;
        };
        let refused = e.property == NONE;
        let step = match reading.step {
            Step::Targets => Step::Targets,
            Step::Blind(i) => Step::Blind(i),
            Step::Data => Step::Data,
            // Chunks come as property changes, not as this.
            Step::Incr { .. } => return,
        };
        match step {
            Step::Targets if refused => self.blind(0),
            Step::Targets => {
                let offered: Vec<&str> = match self.take_property() {
                    Some((_, bytes)) => bytes
                        .chunks_exact(4)
                        .filter_map(|c| self.atoms.name(u32::from_ne_bytes(c.try_into().unwrap())))
                        .collect(),
                    None => Vec::new(),
                };
                match text::best(offered.iter().copied()) {
                    Some(target) => {
                        log::debug!("clipboard: reading it as {target}");
                        self.reading.as_mut().unwrap().step = Step::Data;
                        self.convert(self.atoms.get(target));
                    }
                    None => {
                        log::debug!("clipboard: the owner offers no text");
                        self.finish(None);
                    }
                }
            }
            Step::Blind(i) if refused => self.blind(i + 1),
            Step::Blind(_) | Step::Data if refused => self.finish(None),
            Step::Blind(_) | Step::Data => match self.take_property() {
                Some((type_, bytes)) if type_ == self.atoms.get("INCR") => {
                    // Deleting it (done by the read) is what starts the
                    // chunks coming.
                    self.reading.as_mut().unwrap().step = Step::Incr {
                        type_: NONE,
                        buf: Vec::new(),
                    };
                    log::debug!(
                        "clipboard: an INCR transfer of about {} bytes",
                        bytes.get(..4).map_or(0, |b| u32::from_ne_bytes(b.try_into().unwrap()))
                    );
                }
                Some((type_, bytes)) => {
                    let text = self.decode(type_, &bytes);
                    self.finish(Some(text));
                }
                None => self.finish(None),
            },
            Step::Incr { .. } => unreachable!(),
        }
    }

    /// Asks an owner that would not say its targets for `BLIND[i]`.
    fn blind(&mut self, i: usize) {
        match BLIND.get(i) {
            Some(target) => {
                self.reading.as_mut().unwrap().step = Step::Blind(i);
                self.convert(self.atoms.get(target));
            }
            None => {
                log::debug!("clipboard: the owner refused every text target");
                self.finish(None);
            }
        }
    }

    /// By the type it came as; STRING is among the atoms interned (it is
    /// predefined, and interning it gives the same atom).
    fn decode(&self, type_: Atom, bytes: &[u8]) -> String {
        text::decode(self.atoms.name(type_).unwrap_or(""), bytes)
    }

    /// The next chunk of an `INCR` transfer into our window.
    fn incoming_chunk(&mut self) {
        let Some((type_, bytes)) = self.take_property() else {
            return;
        };
        let Some(Reading {
            step: Step::Incr { type_: t, buf }, ..
        }) = &mut self.reading
        else {
            return;
        };
        if bytes.is_empty() {
            let (t, buf) = (*t, std::mem::take(buf));
            let text = self.decode(t, &buf);
            self.finish(Some(text));
            return;
        }
        *t = type_;
        buf.extend_from_slice(&bytes);
        if buf.len() > MAX_TEXT {
            log::warn!("clipboard: the owner's text is over {MAX_TEXT} bytes; not read");
            self.finish(None);
        }
    }

    // ---- owning -----------------------------------------------------------

    /// The timestamp has come: take the selection with it.
    fn claim(&mut self, time: u32) {
        let Some(text) = self.claiming.take() else {
            return;
        };
        let clipboard = self.atoms.get("CLIPBOARD");
        let _ = self.conn.set_selection_owner(self.window, clipboard, time);
        let owner = self.conn.get_selection_owner(clipboard).ok().and_then(|c| c.reply().ok()).map(|r| r.owner);
        if owner == Some(self.window) {
            log::debug!("clipboard: owned, {} bytes", text.len());
            self.owned = Some(Owned {
                text: Arc::new(text),
                time,
            });
        } else {
            log::warn!("clipboard: the X server did not give us the clipboard");
            self.owned = None;
        }
    }

    fn selection_request(&mut self, e: SelectionRequestEvent) {
        // An obsolete requestor names no property; the target is its name.
        let property = if e.property == NONE { e.target } else { e.property };
        let delivered = self.convert_for(&e, property);
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: e.time,
            requestor: e.requestor,
            selection: e.selection,
            target: e.target,
            property: if delivered { property } else { NONE },
        };
        let _ = self.conn.send_event(false, e.requestor, EventMask::NO_EVENT, notify);
    }

    /// Puts what `e` asks for on its requestor's `property`, or says no.
    fn convert_for(&mut self, e: &SelectionRequestEvent, property: Atom) -> bool {
        let Some(owned) = &self.owned else {
            return false;
        };
        // A request from before we took it is for somebody else's.
        if e.selection != self.atoms.get("CLIPBOARD") || (e.time != CURRENT_TIME && e.time < owned.time) {
            return false;
        }
        let (requestor, target) = (e.requestor, e.target);
        if target == self.atoms.get("TARGETS") {
            let mut targets = vec![self.atoms.get("TARGETS"), self.atoms.get("TIMESTAMP")];
            targets.extend(text::SERVED.iter().map(|(name, _, _)| self.atoms.get(name)));
            return self
                .conn
                .change_property32(PropMode::REPLACE, requestor, property, AtomEnum::ATOM, &targets)
                .is_ok();
        }
        if target == self.atoms.get("TIMESTAMP") {
            return self
                .conn
                .change_property32(PropMode::REPLACE, requestor, property, AtomEnum::INTEGER, &[owned.time])
                .is_ok();
        }
        let Some((type_name, encoding)) = self.atoms.name(target).and_then(text::served) else {
            log::debug!("clipboard: refused a conversion to atom {target}");
            return false;
        };
        let type_ = self.atoms.get(type_name);
        let data = text::encode(&owned.text, encoding);
        if data.len() <= self.chunk {
            return self.conn.change_property8(PropMode::REPLACE, requestor, property, type_, &data).is_ok();
        }
        // INCR: the size first, then a chunk each time the requestor deletes
        // the property, which it needs our PropertyChange on its window for.
        let _ = self.conn.change_window_attributes(
            requestor,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        );
        let incr = self.atoms.get("INCR");
        let ok = self
            .conn
            .change_property32(PropMode::REPLACE, requestor, property, incr, &[data.len() as u32])
            .is_ok();
        if ok {
            log::debug!("clipboard: serving {} bytes by INCR", data.len());
            self.sending.retain(|s| !(s.requestor == requestor && s.property == property));
            self.sending.push(Sending {
                requestor,
                property,
                type_,
                data,
                offset: 0,
                ended: false,
                progress_at: Instant::now(),
            });
        }
        ok
    }

    /// A requestor deleted a property: the next chunk of its transfer.
    fn outgoing_chunk(&mut self, window: Window, atom: Atom) {
        let Some(i) = self.sending.iter().position(|s| s.requestor == window && s.property == atom) else {
            return;
        };
        let s = &mut self.sending[i];
        if s.ended {
            let s = self.sending.remove(i);
            self.forget(s.requestor);
            return;
        }
        let end = (s.offset + self.chunk).min(s.data.len());
        let chunk = &s.data[s.offset..end];
        let _ = self.conn.change_property8(PropMode::REPLACE, s.requestor, s.property, s.type_, chunk);
        s.ended = chunk.is_empty();
        s.offset = end;
        s.progress_at = Instant::now();
    }

    /// No longer listening to a requestor's properties.
    fn forget(&self, requestor: Window) {
        if !self.sending.iter().any(|s| s.requestor == requestor) {
            let _ = self.conn.change_window_attributes(
                requestor,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
            );
        }
    }

    /// Exiting: hand the text to a clipboard manager if one runs, and keep
    /// serving until it has it (see `run`).
    fn hand_off(&mut self) {
        let manager = self.atoms.get("CLIPBOARD_MANAGER");
        let has_manager = self
            .conn
            .get_selection_owner(manager)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.owner != NONE);
        let deadline = Instant::now();
        match &self.owned {
            Some(owned) if has_manager => {
                log::debug!("clipboard: handing the text to the clipboard manager");
                let _ = self.conn.convert_selection(
                    self.window,
                    manager,
                    self.atoms.get("SAVE_TARGETS"),
                    self.atoms.get("_VENTO_CLIPBOARD"),
                    owned.time,
                );
                self.handing_off = Some(deadline + HANDOFF);
            }
            _ => self.handing_off = Some(deadline),
        }
    }

    // ---- the loop ---------------------------------------------------------

    fn event(&mut self, event: Event) {
        match event {
            Event::SelectionNotify(e) if self.handing_off.is_some() && e.selection == self.atoms.get("CLIPBOARD_MANAGER") => {
                log::debug!("clipboard: the clipboard manager has the text");
                self.handing_off = Some(Instant::now());
            }
            Event::SelectionNotify(e) => self.selection_notify(e),
            Event::SelectionRequest(e) => self.selection_request(e),
            Event::SelectionClear(e) if e.selection == self.atoms.get("CLIPBOARD") => {
                if self.owned.as_ref().is_some_and(|o| e.time >= o.time || e.time == CURRENT_TIME) {
                    log::debug!("clipboard: another client owns it now");
                    self.owned = None;
                }
            }
            Event::PropertyNotify(e) if e.window == self.window => {
                if e.atom == self.atoms.get("_VENTO_CLIPBOARD_TIME") {
                    self.claim(e.time);
                } else if e.atom == self.atoms.get("_VENTO_CLIPBOARD")
                    && e.state == Property::NEW_VALUE
                    && matches!(self.reading, Some(Reading { step: Step::Incr { .. }, .. }))
                {
                    self.incoming_chunk();
                }
            }
            Event::PropertyNotify(e) if e.state == Property::DELETE => self.outgoing_chunk(e.window, e.atom),
            // A requestor gone mid-transfer, mostly; nothing to be done.
            Event::Error(e) => log::debug!("clipboard: X error {:?}", e.error_kind),
            _ => {}
        }
    }

    fn expire(&mut self, now: Instant) {
        if self.reading.as_ref().is_some_and(|r| r.deadline <= now) {
            log::warn!(
                "clipboard: the owner did not answer within {} s; nothing read",
                READ_DEADLINE.as_secs_f32()
            );
            self.finish(None);
        }
        let stalled: Vec<Window> = self
            .sending
            .iter()
            .filter(|s| s.progress_at + SEND_IDLE <= now)
            .map(|s| s.requestor)
            .collect();
        if !stalled.is_empty() {
            log::debug!("clipboard: dropped {} INCR transfers whose requestors stopped", stalled.len());
            self.sending.retain(|s| s.progress_at + SEND_IDLE > now);
            for requestor in stalled {
                self.forget(requestor);
            }
        }
    }
}
