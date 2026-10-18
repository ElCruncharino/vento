// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a session tells whoever is watching it, and the channel it says it
//! through.
//!
//! An [`Event`] is `runtime::Events` as a value — one variant per method, plus
//! the three the C API has over it (`question`, `security_needed`,
//! `message`) — and a [`Mailbox`] is where a session puts them: a queue
//! behind a mutex, a condition variable for a thread that wants to block on
//! it, and an optional wake hook for one that cannot, which is an egui window
//! asking to be repainted.
//!
//! Damage is the one thing not delivered a message at a time. `damaged` runs
//! hundreds of times a frame on a busy screen, so it marks a rectangle into a
//! list behind the same mutex and sends nothing; `frame_end` turns the list
//! into one [`Event::Frame`], and if an unread `Frame` is still in the queue
//! the two are merged rather than queued — a window that fell behind sees one
//! frame with everything that moved, not a backlog. The merge reaches past
//! whatever else is waiting, a cursor or a bell, since a frame's pixels are
//! read when it is taken, but not past a change of the desktop's size: the
//! rectangles of one size are not rectangles of the next. The events that
//! say what something is now — the cursor's shape, the pointer mode, the far
//! end's clipboard, whether its sound is playing — are kept the same way, the latest unread one of each and
//! no other, so that what waits for a window that stopped reading is a few
//! events, however long it stopped for. A
//! rectangle that repeats one already marked, lies inside one, or makes one
//! rectangle with it is merged into it, so that a window that fell behind
//! reads each pixel once. The list itself is capped: past [`Damage::LIMIT`]
//! rectangles it collapses to their bounding box, since a frame with that
//! many rectangles is a frame that is being redrawn anyway. A channel per rectangle was the alternative,
//! and the cost is not the channel but the allocation and the wake: this
//! sends one of each per frame.
//!
//! The mailbox also keeps the facts the events imply — the size, the state,
//! whether the pointer is relative — so that a backend can answer
//! [`facts`](crate::backend::Backend::facts) without a second copy of them.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub use media::audio::{Sound, Switch};

/// A rectangle of the desktop, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Rect {
    /// How many pixels, wide enough that no rectangle overflows it.
    pub fn area(self) -> u64 {
        self.width as u64 * self.height as u64
    }

    /// The smallest rectangle holding both.
    pub fn union(self, other: Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = (self.x + self.width).max(other.x + other.width);
        let bottom = (self.y + self.height).max(other.y + other.height);
        Rect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }
}

/// The states RealVNC's security prompt carries: `VENTO_IDENTITY_*`, in
/// order, and `Prompt.Trust.Identity` in Java.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identity {
    /// Known, and matches.
    Ok,
    /// Never seen before.
    New,
    /// Seen, but under a different address.
    MatchesAnother,
    /// Seen under this address, and different now: the bad one.
    Changed,
    /// The server offered none.
    Missing,
    /// Configured out of band.
    Preshared,
    /// Apple Remote Desktop, which has no identity of this kind.
    Ard,
}

impl Identity {
    pub fn from_c(value: i32) -> Identity {
        match value {
            0 => Identity::Ok,
            1 => Identity::New,
            2 => Identity::MatchesAnother,
            3 => Identity::Changed,
            4 => Identity::Missing,
            5 => Identity::Preshared,
            _ => Identity::Ard,
        }
    }
}

/// `VENTO_ENCRYPTED` and the two `VENTO_UNENCRYPTED_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encryption {
    Encrypted,
    /// Plain text, and the person should be told.
    UnencryptedWarn,
    /// Plain text, deliberately: a local or already-tunnelled link.
    UnencryptedQuiet,
}

impl Encryption {
    pub fn from_c(value: i32) -> Encryption {
        match value {
            1 => Encryption::UnencryptedWarn,
            2 => Encryption::UnencryptedQuiet,
            _ => Encryption::Encrypted,
        }
    }
}

/// How a message should look: `VENTO_SEVERITY_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Information,
    Question,
    Warning,
    Error,
}

impl Severity {
    pub fn from_c(value: i32) -> Severity {
        match value {
            1 => Severity::Question,
            2 => Severity::Warning,
            3 => Severity::Error,
            _ => Severity::Information,
        }
    }
}

/// The remote cursor's shape, `R, G, B, A` bytes with alpha from the
/// protocol's mask, and the hotspot as a positive offset into it.
///
/// The alpha is straight, not premultiplied: the phone hands these pixels
/// to `Bitmap.setPixels`, which takes it that way, and where a protocol's
/// alpha is more than a mask the library under it says so — TigerVNC takes
/// `CursorWithAlpha`'s premultiplied pixels off the wire and divides the
/// alpha back out before the session sees them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub width: usize,
    pub height: usize,
    pub hot_x: i32,
    pub hot_y: i32,
    pub rgba: Vec<u8>,
}

/// Where a session is. `Closed` is terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Connecting,
    Connected,
    Closed,
}

/// What a session says, in the order it said it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The connection is up and the desktop is this big. Once per session.
    Connected { width: usize, height: usize },
    /// The desktop changed size mid-session.
    DesktopSize { width: usize, height: usize },
    /// Everything that belongs to one or more updates has arrived, and these
    /// rectangles of the framebuffer have new pixels; the pixels themselves
    /// are read with `read_region`. `ended` is when the first `frame_end` in
    /// it came — the oldest, where an unread frame took in later ones — which
    /// is what a window measures how long its pixels took to show from.
    Frame { rects: Vec<Rect>, ended: Instant },
    /// The pointer's shape, or `None` where the server has hidden it or asked
    /// for the default one, both of which mean "draw your own".
    Cursor(Option<Cursor>),
    /// Which end owns the cursor has changed.
    PointerMode { relative: bool },
    Bell,
    /// What the far end's clipboard now holds.
    Clipboard(String),
    /// Credentials are wanted; answered through `answer_credentials`.
    /// `needs_user` is false for a scheme with no notion of one.
    CredentialsNeeded { needs_user: bool },
    /// A certificate nobody signed, by SHA-256 fingerprint as
    /// `openssl x509 -fingerprint -sha256` prints it; answered through
    /// `answer_trust`.
    TrustNeeded { fingerprint: String },
    /// The far end could not be verified at all, in the words of why;
    /// answered through `answer_trust`.
    Unverified { why: String },
    /// TigerVNC's own certificate dialog, title and text; `answer_trust`.
    Question { title: String, text: String },
    /// RealVNC's security prompt, `trust_needed` and `unverified` as one
    /// question with both answers spelled out; `answer_trust`, or
    /// `answer_security` to remember.
    SecurityNeeded {
        identity: Identity,
        encryption: Encryption,
        signature: String,
        catchphrase: String,
        matching_address: String,
    },
    /// Something RealVNC's core wants shown; `question` is the difference
    /// between a box with one button and one with two. `answer_trust`.
    Message {
        text: String,
        severity: Severity,
        question: bool,
        confirm_label: String,
    },
    /// The session's sound switch and the sound behind it, whenever any of
    /// it changes: what it does and whether it is on — offered by the
    /// backend as soon as the far end may send sound, an Enable switch
    /// where it can be asked to stop and a Mute switch where not — and
    /// whether a stream is playing. What a window shows its sound switch
    /// by, so that a session whose far end sends no sound has none.
    Sound(Sound),
    /// Terminal, and the last event a session sends. `detail` is empty for an
    /// ending nobody needs telling about — the one that was asked for.
    Closed { detail: String },
}

impl Event {
    /// Whether this says how big the desktop is, which a frame's rectangles
    /// are rectangles of.
    fn is_size(&self) -> bool {
        matches!(self, Event::Connected { .. } | Event::DesktopSize { .. })
    }

    /// Whether this says what something is now rather than that something
    /// happened, so that only the latest of its kind is worth reading.
    fn is_state(&self) -> bool {
        matches!(
            self,
            Event::Cursor(_) | Event::PointerMode { .. } | Event::Clipboard(_) | Event::Sound(_)
        )
    }
}

/// The rectangles a frame has marked so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Damage {
    rects: Vec<Rect>,
}

impl Damage {
    /// Past this many, the list is their bounding box. Sixty-four is more
    /// rectangles than a window can usefully upload separately.
    pub const LIMIT: usize = 64;

    /// Mark `rect`. A rectangle is merged with one already marked wherever
    /// their bounding box is no bigger than the two of them apart — which
    /// takes in a repeat, one inside another, and two halves of one — so
    /// that no pixel is read twice for the sake of a list, and the merge is
    /// tried again with what it made.
    pub fn add(&mut self, rect: Rect) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let mut rect = rect;
        while let Some(i) = self.rects.iter().position(|&r| r.union(rect).area() <= r.area() + rect.area()) {
            rect = rect.union(self.rects.swap_remove(i));
        }
        if self.rects.len() >= Damage::LIMIT {
            let all = self.rects.drain(..).fold(rect, Rect::union);
            self.rects.push(all);
        } else {
            self.rects.push(rect);
        }
    }

    /// Everything marked, one list into the other.
    pub fn extend(&mut self, rects: Vec<Rect>) {
        for rect in rects {
            self.add(rect);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub fn take(&mut self) -> Vec<Rect> {
        std::mem::take(&mut self.rects)
    }
}

struct Inner {
    queue: VecDeque<Event>,
    damage: Damage,
    frames: usize,
    width: usize,
    height: usize,
    state: State,
    pointer_relative: bool,
    /// `AcceptBell`, for the backends that have it; see
    /// [`Mailbox::accept_bell`].
    accept_bell: bool,
}

/// Where a session's events go, and what they have said so far.
pub struct Mailbox {
    inner: Mutex<Inner>,
    ready: Condvar,
    wake: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Mailbox {
    /// `wake` is called, on the protocol thread, after every event is queued:
    /// a window hands over its `request_repaint`. It must not block.
    pub fn new(wake: Option<Box<dyn Fn() + Send + Sync>>) -> Arc<Mailbox> {
        Arc::new(Mailbox {
            inner: Mutex::new(Inner {
                queue: VecDeque::new(),
                damage: Damage::default(),
                frames: 0,
                width: 0,
                height: 0,
                state: State::Connecting,
                pointer_relative: false,
                accept_bell: true,
            }),
            ready: Condvar::new(),
            wake,
        })
    }

    fn push(&self, event: Event) {
        {
            let mut inner = self.inner.lock().unwrap();
            match &event {
                Event::Connected { width, height } => {
                    inner.width = *width;
                    inner.height = *height;
                    inner.state = State::Connected;
                }
                Event::DesktopSize { width, height } => {
                    inner.width = *width;
                    inner.height = *height;
                }
                Event::PointerMode { relative } => inner.pointer_relative = *relative,
                Event::Closed { .. } => inner.state = State::Closed,
                _ => {}
            }
            // An unread event that only says what something is now is
            // superseded by the next of its kind: it goes, and the new one
            // takes its turn at the back. There is at most one to find,
            // since every one before it went the same way.
            if event.is_state()
                && let Some(i) = inner.queue.iter().position(|e| std::mem::discriminant(e) == std::mem::discriminant(&event))
            {
                inner.queue.remove(i);
            }
            inner.queue.push_back(event);
        }
        self.notify();
    }

    fn notify(&self) {
        self.ready.notify_all();
        if let Some(wake) = &self.wake {
            wake();
        }
    }

    /// The next event, if one is waiting.
    pub fn take(&self) -> Option<Event> {
        self.inner.lock().unwrap().queue.pop_front()
    }

    /// Blocks until an event is waiting or `timeout` has passed; whether one
    /// is. For a thread with nothing better to do, which a window is not.
    pub fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut inner = self.inner.lock().unwrap();
        while inner.queue.is_empty() {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            inner = self.ready.wait_timeout(inner, deadline - now).unwrap().0;
        }
        true
    }

    /// The desktop's size, as the last event said it; zero until connected.
    pub fn size(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap();
        (inner.width, inner.height)
    }

    pub fn state(&self) -> State {
        self.inner.lock().unwrap().state
    }

    pub fn connected(&self) -> bool {
        self.state() == State::Connected
    }

    pub fn pointer_relative(&self) -> bool {
        self.inner.lock().unwrap().pointer_relative
    }

    /// How many `frame_end`s have arrived, counting the ones with nothing in
    /// them.
    pub fn frames(&self) -> usize {
        self.inner.lock().unwrap().frames
    }

    // ---- what a session calls, from its protocol thread -------------------

    pub fn on_connected(&self, width: usize, height: usize) {
        self.push(Event::Connected { width, height });
    }

    pub fn on_desktop_size(&self, width: usize, height: usize) {
        self.push(Event::DesktopSize { width, height });
    }

    /// A mark, not a message: see the module comment.
    pub fn on_damaged(&self, x: usize, y: usize, width: usize, height: usize) {
        self.inner.lock().unwrap().damage.add(Rect {
            x,
            y,
            width,
            height,
        });
    }

    pub fn on_frame_end(&self) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.frames += 1;
            if inner.damage.is_empty() {
                return;
            }
            let rects = inner.damage.take();
            // The unread Frame since the desktop last changed size, wherever
            // it is in the queue: its rectangles are read when it is taken,
            // so taking these with it reads the same pixels, only sooner,
            // and the events in between are about other things. A change of
            // size is the one thing a rectangle cannot be moved across.
            let unread = inner
                .queue
                .iter_mut()
                .rev()
                .take_while(|e| !e.is_size())
                .find_map(|e| match e {
                    Event::Frame { rects, .. } => Some(rects),
                    _ => None,
                });
            // The unread frame keeps when it ended: its pixels have waited
            // since then, and still have.
            match unread {
                Some(previous) => {
                    let mut merged = Damage::default();
                    merged.extend(std::mem::take(previous));
                    merged.extend(rects);
                    *previous = merged.take();
                }
                _ => inner.queue.push_back(Event::Frame {
                    rects,
                    ended: Instant::now(),
                }),
            }
        }
        self.notify();
    }

    /// `pixels` in the framebuffer's own word order, which is `R, G, B, A`
    /// in memory; an empty cursor is the default one.
    pub fn on_cursor(&self, pixels: &[u32], width: usize, height: usize, hot_x: i32, hot_y: i32) {
        let cursor = if width == 0 || height == 0 || pixels.len() < width * height {
            None
        } else {
            Some(Cursor {
                width,
                height,
                hot_x,
                hot_y,
                rgba: pixels[..width * height]
                    .iter()
                    .flat_map(|p| p.to_le_bytes())
                    .collect(),
            })
        };
        self.push(Event::Cursor(cursor));
    }

    pub fn on_pointer_mode(&self, relative: bool) {
        self.push(Event::PointerMode { relative });
    }

    /// Whether a bell from the far end is passed on at all. Off, it never
    /// reaches the queue, which is the Java backends' `onBell` returning
    /// early when `AcceptBell` is off — done once here, for every backend,
    /// rather than in each adapter, the library ones included.
    pub fn accept_bell(&self, accept: bool) {
        self.inner.lock().unwrap().accept_bell = accept;
    }

    pub fn on_bell(&self) {
        if !self.inner.lock().unwrap().accept_bell {
            return;
        }
        self.push(Event::Bell);
    }

    /// The session's sound, from its audio controls (see
    /// [`Kind::start`](crate::kind::Kind::start)). Dropped once the session
    /// has closed: a backend that closes its last stream, or withdraws its
    /// switch, while it tears down must not say anything after the event
    /// that is the last.
    pub fn on_sound(&self, sound: Sound) {
        if self.state() == State::Closed {
            return;
        }
        self.push(Event::Sound(sound));
    }

    pub fn on_clipboard(&self, text: &str) {
        self.push(Event::Clipboard(text.to_string()));
    }

    pub fn on_credentials_needed(&self, needs_user: bool) {
        self.push(Event::CredentialsNeeded { needs_user });
    }

    pub fn on_trust_needed(&self, fingerprint: &str) {
        self.push(Event::TrustNeeded {
            fingerprint: fingerprint.to_string(),
        });
    }

    pub fn on_unverified(&self, why: &str) {
        self.push(Event::Unverified {
            why: why.to_string(),
        });
    }

    pub fn on_question(&self, title: &str, text: &str) {
        self.push(Event::Question {
            title: title.to_string(),
            text: text.to_string(),
        });
    }

    pub fn on_security_needed(
        &self,
        identity: Identity,
        encryption: Encryption,
        signature: &str,
        catchphrase: &str,
        matching_address: &str,
    ) {
        self.push(Event::SecurityNeeded {
            identity,
            encryption,
            signature: signature.to_string(),
            catchphrase: catchphrase.to_string(),
            matching_address: matching_address.to_string(),
        });
    }

    pub fn on_message(&self, text: &str, severity: Severity, question: bool, confirm_label: &str) {
        self.push(Event::Message {
            text: text.to_string(),
            severity,
            question,
            confirm_label: confirm_label.to_string(),
        });
    }

    pub fn on_closed(&self, detail: &str) {
        self.push(Event::Closed {
            detail: detail.to_string(),
        });
    }
}

/// A mailbox as the `Events` a `runtime::Session` is started with.
pub struct Feed(pub Arc<Mailbox>);

impl runtime::Events for Feed {
    fn connected(&self, width: usize, height: usize) {
        self.0.on_connected(width, height);
    }

    fn desktop_size(&self, width: usize, height: usize) {
        self.0.on_desktop_size(width, height);
    }

    fn damaged(&self, x: usize, y: usize, width: usize, height: usize) {
        self.0.on_damaged(x, y, width, height);
    }

    fn frame_end(&self) {
        self.0.on_frame_end();
    }

    fn cursor(&self, pixels: &[u32], width: usize, height: usize, hot_x: i32, hot_y: i32) {
        self.0.on_cursor(pixels, width, height, hot_x, hot_y);
    }

    fn pointer_mode(&self, relative: bool) {
        self.0.on_pointer_mode(relative);
    }

    fn bell(&self) {
        self.0.on_bell();
    }

    fn clipboard(&self, text: &str) {
        self.0.on_clipboard(text);
    }

    fn credentials_needed(&self, needs_user: bool) {
        self.0.on_credentials_needed(needs_user);
    }

    fn trust_needed(&self, fingerprint: &str) {
        self.0.on_trust_needed(fingerprint);
    }

    fn unverified(&self, why: &str) {
        self.0.on_unverified(why);
    }

    fn closed(&self, detail: &str) {
        self.0.on_closed(detail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A frame's rectangles, when it is one: when it ended is the one
    /// thing a test cannot spell out.
    fn frame(event: Option<Event>) -> Option<Vec<Rect>> {
        match event {
            Some(Event::Frame { rects, .. }) => Some(rects),
            _ => None,
        }
    }

    #[test]
    fn a_merged_frame_ended_when_its_oldest_part_did() {
        let mailbox = Mailbox::new(None);
        let before = Instant::now();
        mailbox.on_damaged(0, 0, 1, 1);
        mailbox.on_frame_end();
        let first = Instant::now();
        std::thread::sleep(Duration::from_millis(5));
        mailbox.on_damaged(2, 2, 1, 1);
        mailbox.on_frame_end();
        let Some(Event::Frame { rects, ended }) = mailbox.take() else {
            panic!("a frame");
        };
        assert_eq!(rects.len(), 2);
        assert!(before <= ended && ended <= first, "the first frame_end's time, not the second's");
    }

    fn rect(x: usize, y: usize, w: usize, h: usize) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn damage_collapses_to_a_bounding_box_past_the_limit() {
        let mut damage = Damage::default();
        // Apart, so that none of them merges with its neighbour.
        for i in 0..Damage::LIMIT {
            damage.add(rect(i * 20, 0, 10, 10));
        }
        assert_eq!(damage.rects.len(), Damage::LIMIT);
        damage.add(rect(0, 500, 1, 1));
        let rects = damage.take();
        assert_eq!(rects, vec![rect(0, 0, (Damage::LIMIT - 1) * 20 + 10, 501)]);
        assert!(damage.is_empty());
    }

    #[test]
    fn damage_drops_repeats_and_rectangles_inside_others() {
        let mut damage = Damage::default();
        damage.add(rect(10, 10, 100, 100));
        damage.add(rect(10, 10, 100, 100));
        damage.add(rect(20, 20, 5, 5));
        assert_eq!(damage.rects, [rect(10, 10, 100, 100)]);
        // A bigger one takes in the one it covers.
        damage.add(rect(0, 0, 200, 200));
        assert_eq!(damage.take(), [rect(0, 0, 200, 200)]);
    }

    #[test]
    fn damage_merges_where_the_union_costs_nothing_and_not_elsewhere() {
        let mut damage = Damage::default();
        // Two halves of one row, and then two that overlap by half: each
        // pair's bounding box is no more than the pair.
        damage.add(rect(0, 0, 50, 10));
        damage.add(rect(50, 0, 50, 10));
        assert_eq!(damage.rects, [rect(0, 0, 100, 10)]);
        damage.add(rect(0, 100, 10, 10));
        damage.add(rect(5, 100, 10, 10));
        assert_eq!(damage.rects, [rect(0, 0, 100, 10), rect(0, 100, 15, 10)]);
        // Diagonal neighbours would read the corners they do not cover.
        let mut damage = Damage::default();
        damage.add(rect(0, 0, 10, 10));
        damage.add(rect(10, 10, 10, 10));
        assert_eq!(damage.rects.len(), 2);
        // A merge is tried again with what it made: the third joins the two
        // into one, and that one swallows the first's neighbour.
        damage.add(rect(0, 10, 10, 10));
        damage.add(rect(10, 0, 10, 10));
        assert_eq!(damage.take(), [rect(0, 0, 20, 20)]);
    }

    #[test]
    fn a_full_screen_update_marked_tile_by_tile_is_one_rectangle() {
        // The shape an encoder's tiles give: 64-pixel squares, a frame's
        // worth twice over, as a server that sends the screen twice does.
        let mut damage = Damage::default();
        for _ in 0..2 {
            for y in (0..1200).step_by(64) {
                for x in (0..1920).step_by(64) {
                    damage.add(rect(x, y, 64.min(1920 - x), 64.min(1200 - y)));
                }
            }
        }
        assert_eq!(damage.take(), [rect(0, 0, 1920, 1200)]);
    }

    #[test]
    fn an_empty_rectangle_is_not_damage() {
        let mut damage = Damage::default();
        damage.add(rect(5, 5, 0, 10));
        assert!(damage.is_empty());
    }

    #[test]
    fn a_frame_is_one_event_and_an_unread_one_absorbs_the_next() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&wakes);
        let mailbox = Mailbox::new(Some(Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        })));
        mailbox.on_damaged(0, 0, 10, 10);
        mailbox.on_damaged(20, 20, 5, 5);
        mailbox.on_frame_end();
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        mailbox.on_damaged(100, 100, 1, 1);
        mailbox.on_frame_end();
        // Nothing marked: a frame with nothing in it is counted and not sent.
        mailbox.on_frame_end();
        assert_eq!(mailbox.frames(), 3);
        assert_eq!(
            frame(mailbox.take()),
            Some(vec![rect(0, 0, 10, 10), rect(20, 20, 5, 5), rect(100, 100, 1, 1)])
        );
        assert_eq!(mailbox.take(), None);
    }

    #[test]
    fn an_unread_frame_absorbs_the_next_past_other_events() {
        let mailbox = Mailbox::new(None);
        mailbox.on_damaged(0, 0, 1, 1);
        mailbox.on_frame_end();
        mailbox.on_bell();
        mailbox.on_damaged(2, 2, 1, 1);
        mailbox.on_frame_end();
        assert_eq!(frame(mailbox.take()), Some(vec![rect(0, 0, 1, 1), rect(2, 2, 1, 1)]));
        assert_eq!(mailbox.take(), Some(Event::Bell));
        assert_eq!(mailbox.take(), None);
    }

    #[test]
    fn a_bell_not_accepted_is_never_queued() {
        let mailbox = Mailbox::new(None);
        mailbox.accept_bell(false);
        mailbox.on_bell();
        assert_eq!(mailbox.take(), None);
        mailbox.accept_bell(true);
        mailbox.on_bell();
        assert_eq!(mailbox.take(), Some(Event::Bell));
    }

    #[test]
    fn the_sound_is_the_latest_and_never_after_the_end() {
        let mailbox = Mailbox::new(None);
        let playing = |playing: bool| Sound {
            switch: Some(Switch::Mute),
            enabled: true,
            muted: false,
            playing,
        };
        mailbox.on_sound(playing(true));
        mailbox.on_bell();
        mailbox.on_sound(playing(false));
        mailbox.on_sound(playing(true));
        assert_eq!(mailbox.take(), Some(Event::Bell));
        assert_eq!(mailbox.take(), Some(Event::Sound(playing(true))));
        mailbox.on_closed("");
        mailbox.on_sound(Sound::default());
        assert_eq!(mailbox.take(), Some(Event::Closed { detail: String::new() }));
        assert_eq!(mailbox.take(), None);
    }

    #[test]
    fn a_change_of_size_keeps_the_frames_either_side_of_it_apart() {
        let mailbox = Mailbox::new(None);
        mailbox.on_connected(100, 100);
        mailbox.on_damaged(90, 90, 10, 10);
        mailbox.on_frame_end();
        mailbox.on_desktop_size(50, 50);
        mailbox.on_damaged(0, 0, 5, 5);
        mailbox.on_frame_end();
        mailbox.on_bell();
        mailbox.on_damaged(10, 10, 5, 5);
        mailbox.on_frame_end();
        assert_eq!(mailbox.take(), Some(Event::Connected { width: 100, height: 100 }));
        assert_eq!(frame(mailbox.take()), Some(vec![rect(90, 90, 10, 10)]));
        assert_eq!(mailbox.take(), Some(Event::DesktopSize { width: 50, height: 50 }));
        assert_eq!(frame(mailbox.take()), Some(vec![rect(0, 0, 5, 5), rect(10, 10, 5, 5)]));
        assert_eq!(mailbox.take(), Some(Event::Bell));
        assert_eq!(mailbox.take(), None);
    }

    #[test]
    fn a_window_that_stops_reading_finds_a_few_events_waiting() {
        // The far end's cursor changing shape between every two frames, 25
        // times a second for thirty seconds, with nobody reading: what used
        // to queue 750 frames and 750 cursors.
        let mailbox = Mailbox::new(None);
        mailbox.on_connected(1920, 1200);
        for i in 0..750 {
            mailbox.on_damaged(i, 0, 1, 1);
            mailbox.on_frame_end();
            let pixel = if i % 2 == 0 { 0xff00_00ff } else { 0xffff_0000 };
            mailbox.on_cursor(&[pixel], 1, 1, 0, 0);
            mailbox.on_clipboard(&format!("copy {i}"));
            mailbox.on_pointer_mode(i % 2 == 0);
        }
        let waiting: Vec<Event> = std::iter::from_fn(|| mailbox.take()).collect();
        assert_eq!(waiting.len(), 5, "{waiting:?}");
        assert_eq!(waiting[0], Event::Connected { width: 1920, height: 1200 });
        // One frame with every column in it, merged as it went.
        assert_eq!(frame(Some(waiting[1].clone())), Some(vec![rect(0, 0, 750, 1)]));
        // And the latest of each of the others, in the order they were last
        // said.
        assert!(matches!(&waiting[2], Event::Cursor(Some(c)) if c.rgba == [0x00, 0x00, 0xff, 0xff]));
        assert_eq!(waiting[3], Event::Clipboard("copy 749".to_string()));
        assert_eq!(waiting[4], Event::PointerMode { relative: false });
    }

    #[test]
    fn the_facts_follow_the_events() {
        let mailbox = Mailbox::new(None);
        assert_eq!(mailbox.state(), State::Connecting);
        mailbox.on_connected(800, 600);
        assert_eq!(mailbox.size(), (800, 600));
        assert!(mailbox.connected());
        mailbox.on_desktop_size(1024, 768);
        mailbox.on_pointer_mode(true);
        assert_eq!(mailbox.size(), (1024, 768));
        assert!(mailbox.pointer_relative());
        mailbox.on_closed("gone");
        assert_eq!(mailbox.state(), State::Closed);
        assert!(mailbox.wait(Duration::from_millis(1)));
        assert_eq!(mailbox.take(), Some(Event::Connected { width: 800, height: 600 }));
    }

    #[test]
    fn a_cursor_crosses_as_bytes_and_an_empty_one_as_none() {
        let mailbox = Mailbox::new(None);
        mailbox.on_cursor(&[0x4433_2211], 1, 1, 0, 0);
        let Some(Event::Cursor(Some(cursor))) = mailbox.take() else {
            panic!("a cursor");
        };
        assert_eq!(cursor.rgba, [0x11, 0x22, 0x33, 0x44]);
        mailbox.on_cursor(&[], 0, 0, 0, 0);
        assert_eq!(mailbox.take(), Some(Event::Cursor(None)));
    }
}
