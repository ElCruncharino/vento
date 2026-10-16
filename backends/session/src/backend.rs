// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! One remote desktop session, whatever protocol is underneath: the
//! interface the window and the probe are written against.
//!
//! It is `Backend.java` with the Android taken out, member for member, because
//! that interface is the one seam eight backends have had to fit and there was
//! no reason to invent a second. What is gone is what a phone put there: the
//! `Bitmap` becomes a slice of `R, G, B, A` bytes, the main-thread listener
//! becomes the [`Mailbox`](crate::events::Mailbox) a session is created with,
//! and the asynchronous `connectionInfo` becomes a call — every backend here
//! answers it from state it keeps beside the connection, including RealVNC's,
//! whose session file asks the core on its own thread and waits.
//!
//! Threads: every method may be called from any thread, and [`read_region`]
//! and [`thumbnail`] are meant to be called from whichever is drawing,
//! concurrently with everything else — they take the read side of a lock the
//! protocol thread holds for a blit at a time, so a pixel fetch never queues
//! behind a socket read. Events arrive through the mailbox, from the protocol
//! thread, and are read from wherever is convenient.
//!
//! [`read_region`]: Backend::read_region
//! [`thumbnail`]: Backend::thumbnail

pub use common::Credentials;

/// One of the far end's screens, as a rectangle in its coordinates.
///
/// Two questions use this shape and they are not the same question — see
/// [`Facts::monitors`] and [`Facts::displays`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    /// What the far end calls this screen, opaque, and only for handing back
    /// in [`Backend::request_layout`]: RFB's screen id, which a server
    /// matches a requested layout against the screens it has. Where the
    /// protocol has no id of its own it is the screen's index in the list.
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Monitor {
    /// The shape every backend hands its layout over in: four ints per
    /// screen, x, y, width, height. One reader for all of them, so that an
    /// empty answer is the same answer whichever library read it off the wire;
    /// a screen with no area is not a screen. Each id is the screen's index
    /// in `flat`, which is also what the phone's JNI layer flattens a layout
    /// back into, so that four ints a screen stays the one shape Java reads.
    pub fn from_flat(flat: &[i32]) -> Vec<Monitor> {
        flat.chunks_exact(4)
            .enumerate()
            .filter(|(_, m)| m[2] > 0 && m[3] > 0)
            .map(|(i, m)| Monitor {
                id: i as u32,
                x: m[0],
                y: m[1],
                width: m[2],
                height: m[3],
            })
            .collect()
    }

    /// The same with the far end's own ids: five ints per screen, id, x, y,
    /// width, height, the id's bits as they came. What a backend whose
    /// protocol numbers its screens hands over, and the shape a layout is
    /// asked for in across the C seam.
    pub fn from_flat5(flat: &[i32]) -> Vec<Monitor> {
        flat.chunks_exact(5)
            .filter(|m| m[3] > 0 && m[4] > 0)
            .map(|m| Monitor {
                id: m[0] as u32,
                x: m[1],
                y: m[2],
                width: m[3],
                height: m[4],
            })
            .collect()
    }

    /// `screens` as [`from_flat5`](Monitor::from_flat5) reads them.
    pub fn flat5(screens: &[Monitor]) -> Vec<i32> {
        screens
            .iter()
            .flat_map(|m| [m.id as i32, m.x, m.y, m.width, m.height])
            .collect()
    }
}

/// Everything about a live session that has to be asked for rather than
/// waited for, in one answer: `Backend.Facts` in Java, field for field.
///
/// A caller must expect every one of these to change while a session runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// Zero until the connection is up.
    pub width: usize,
    pub height: usize,
    /// How the far end's *one* desktop is divided, or empty where it has not
    /// said — which is not the same as one monitor, and is the answer for a
    /// library that never exposes the layout.
    pub monitors: Vec<Monitor>,
    /// The pictures the far end is *not* sending and will send if asked;
    /// empty for every protocol but RustDesk. A menu rather than a map: only
    /// [`display`](Facts::display) of them is the picture.
    pub displays: Vec<Monitor>,
    /// Which of `displays` is on screen, or -1 where there is no choice.
    pub display: i32,
    /// Whether the far end will take a new desktop size right now, as one
    /// screen: false over a desktop of several, which one size would merge.
    pub can_resize: bool,
    /// Whether the far end will take a layout of screens right now, any
    /// number of them, through [`Backend::request_layout`]. Only a protocol
    /// that can say where each screen goes says yes.
    pub can_layout: bool,
    pub view_only: bool,
    /// Whether the far end owns the cursor. Pushed as
    /// [`Event::PointerMode`](crate::events::Event::PointerMode) too, and
    /// here for a screen that attaches to a session that has been running
    /// for an hour.
    pub pointer_relative: bool,
}

impl Facts {
    /// Nothing connected: what a session that is over has to say.
    pub const NONE: Facts = Facts {
        width: 0,
        height: 0,
        monitors: Vec::new(),
        displays: Vec::new(),
        display: -1,
        can_resize: false,
        can_layout: false,
        view_only: false,
        pointer_relative: false,
    };
}

impl Default for Facts {
    fn default() -> Facts {
        Facts::NONE
    }
}

/// What a row of the connection panel *is*, independently of what a backend
/// calls it: `ConnectionFact.Field` in Java.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The desktop's own name for itself. The panel's heading.
    DesktopName,
    /// How the bytes get there: direct TCP, relayed, UDP.
    Connection,
    /// What is protecting them, if anything.
    Security,
    /// Which version of which protocol is being spoken.
    Protocol,
    /// The encoding in use for pixels.
    Encoding,
    /// The measured throughput, however the backend measures it.
    LineSpeed,
    /// What the session has moved, both ways, since it was opened.
    Data,
    /// What the server is sending.
    ServerPixels,
    /// What we asked it for.
    ViewerPixels,
    /// Anything a particular protocol has that this list does not.
    Other,
}

/// One line of what a backend can say about the connection it is running.
///
/// The identity is ours and the wording is theirs: `label` is the backend's
/// own word for the row, which is what goes on screen, and it is the same
/// word the Java backend of the same protocol uses. A backend omits a field
/// it has no concept of and includes one with an empty value when it has the
/// concept but no answer yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionFact {
    pub field: Field,
    pub label: &'static str,
    pub value: String,
    /// Something only worth reading when hunting a problem — the pixel
    /// formats are the case this exists for.
    pub diagnostic: bool,
}

impl ConnectionFact {
    pub fn of(field: Field, label: &'static str, value: &str) -> ConnectionFact {
        ConnectionFact {
            field,
            label,
            value: value.trim().to_string(),
            diagnostic: false,
        }
    }

    pub fn diagnostic(field: Field, label: &'static str, value: &str) -> ConnectionFact {
        ConnectionFact {
            diagnostic: true,
            ..ConnectionFact::of(field, label, value)
        }
    }

    /// [`Field::Data`], from the raw counts. The one row whose wording is
    /// ours rather than the backend's: six clients count in six places, and
    /// the row is only comparable if every backend says it the same way and
    /// rounds it the same way, so they hand over bytes and this decides the
    /// rest. A direction the protocol has no count for is left out rather
    /// than shown as a zero.
    pub fn data(traffic: Traffic) -> ConnectionFact {
        let value = match traffic.sent {
            Some(sent) => format!("{} in · {} out", size(traffic.received), size(sent)),
            None => format!("{} in", size(traffic.received)),
        };
        ConnectionFact::of(Field::Data, "Transferred", &value)
    }
}

/// Bytes as a person reads them: powers of a thousand, and a decimal only
/// where the mantissa is small enough for one to mean anything. SI rather
/// than binary because this is a quantity that crossed a network, which is
/// measured in thousands everywhere else it is quoted.
fn size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    const UNITS: [&str; 4] = ["kB", "MB", "GB", "TB"];
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.5 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value < 9.995 {
        format!("{value:.2} {}", UNITS[unit])
    } else if value < 99.95 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// What the session has moved, in bytes, counted where the protocol meets
/// its transport. `sent` is `None` for a backend that has no count of that
/// direction — RealVNC's core counts one way — rather than a zero that
/// would be a claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Traffic {
    pub received: u64,
    pub sent: Option<u64>,
}

/// A picture, `width × height` pixels of `R, G, B, A` bytes, tightly packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Picture {
    /// From the framebuffer's own words, which are `R, G, B, A` in memory —
    /// the little-endian `0xAABBGGRR` every client here works in.
    pub fn from_words(width: usize, height: usize, words: &[u32]) -> Picture {
        Picture {
            width,
            height,
            rgba: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        }
    }
}

/// The step a thumbnail no bigger than `max_width × max_height` takes: any
/// integer, since every backend here takes the step itself and never sees a
/// float.
pub fn thumbnail_step(width: usize, height: usize, max_width: usize, max_height: usize) -> usize {
    let max_width = max_width.max(1);
    let max_height = max_height.max(1);
    width
        .div_ceil(max_width)
        .max(height.div_ceil(max_height))
        .max(1)
}

/// One remote desktop connection, whatever protocol is underneath.
///
/// Created connecting — there is no `connect()`, since nothing here needs a
/// backend to exist before it has somewhere to go — and over when the
/// mailbox says `Closed`. `destroy` ends it and waits for its thread; it is
/// `&self` so that a shared handle can be retired from anywhere, and
/// idempotent so that it can be retired twice.
pub trait Backend: Send + Sync {
    /// Absolute desktop coordinates and an RFB button mask.
    fn pointer(&self, x: i32, y: i32, button_mask: i32);

    /// The same event where the far end owns the cursor: how far it moved
    /// rather than where it now is. Only meaningful while
    /// [`Facts::pointer_relative`]; every other backend ignores it.
    fn pointer_relative(&self, dx: i32, dy: i32, button_mask: i32) {
        let _ = (dx, dy, button_mask);
    }

    /// `keysym` is an X11 keysym; `key_id` is a stable identity for the
    /// physical key, which is what ties an up to its down.
    fn key_down(&self, keysym: u32, key_id: i64);

    fn key_up(&self, key_id: i64);

    /// Let go of every key this session still has down at the far end.
    fn release_all_keys(&self);

    /// Whether the session can be seen: false while its window is minimised
    /// or hidden (wholly covered on X11, suspended by the compositor on
    /// Wayland), true again when it is shown. Backends may pause updates
    /// while it is not. Not the keyboard focus: a window on screen without
    /// it stays live. (The name is the Android app's, where the session's
    /// screen going to the background is the same thing.)
    fn focus(&self, focused: bool);

    fn set_view_only(&self, view_only: bool);

    /// What this end has copied, offered to the far end.
    fn clipboard(&self, text: &str);

    /// Whether a clipboard offered now has somewhere to go at the far end,
    /// where the session can know: `Some(false)` is the backend saying it
    /// would go nowhere, as SPICE does with no guest agent that takes one.
    /// `None`, the default, is a protocol that carries the clipboard as a
    /// message and is not told what became of it. Not in `Backend.java`,
    /// whose app has no use for it; the probe's status does.
    fn carries_clipboard(&self) -> Option<bool> {
        None
    }

    /// Ask the far end to make the desktop this size. A request: what
    /// actually happened arrives as a desktop size, or as nothing at all.
    fn request_desktop_size(&self, width: u32, height: u32) {
        let _ = (width, height);
    }

    /// Ask the far end to make the desktop `width × height`, divided into
    /// `screens` — a request, like [`request_desktop_size`], whose answer is
    /// a desktop size and a new [`Facts::monitors`], or nothing at all.
    ///
    /// A screen's id is one [`Facts::monitors`] gave out, handed back so
    /// that the far end changes the screen it has rather than replacing it;
    /// an id it does not have, or one already used earlier in the list, is a
    /// new screen, and the backend gives it an id of its own. The default,
    /// for a protocol with no layout to send ([`Facts::can_layout`] false),
    /// asks for the one size where there is one screen and nothing
    /// otherwise.
    ///
    /// [`request_desktop_size`]: Backend::request_desktop_size
    fn request_layout(&self, width: u32, height: u32, screens: &[Monitor]) {
        if screens.len() == 1 {
            self.request_desktop_size(width, height);
        }
    }

    /// Ask the far end to send another of its displays.
    fn request_display(&self, index: i32) {
        let _ = index;
    }

    /// Change one of the settings this backend describes on the running
    /// session. Only meaningful for an option that is live; anything else is
    /// remembered for the next connection at best.
    fn set_option(&self, key: &str, value: &str);

    fn facts(&self) -> Facts;

    /// Whatever the backend can say about the live connection, in display
    /// order, with the labels its Java twin uses — or empty when nothing is
    /// connected.
    fn info(&self) -> Vec<ConnectionFact>;

    /// `None` while there is nothing to count.
    fn traffic(&self) -> Option<Traffic>;

    /// Copy the desktop rectangle `(x, y, width, height)` into `dst` at
    /// `(dst_x, dst_y)`, 1:1, where `dst` is rows of `R, G, B, A` bytes
    /// `dst_stride` bytes apart. The rest of `dst` is left as it was, which
    /// is what lets a caller hold a picture bigger than any one read and
    /// refresh the parts of it that changed. False if there is nothing to
    /// read yet, or if the rectangle does not fit either buffer, in which
    /// case `dst` is untouched.
    #[allow(clippy::too_many_arguments)]
    fn read_region(
        &self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        dst: &mut [u8],
        dst_stride: usize,
        dst_x: usize,
        dst_y: usize,
    ) -> bool;

    /// The whole desktop at `1/step`, nearest neighbour, or `None` if there
    /// is nothing to show yet. [`thumbnail_step`] turns a bound into a step.
    fn thumbnail(&self, step: usize) -> Option<Picture>;

    /// The answer to a credentials event. `None` cancels, which ends the
    /// session. An empty user name means the one the session was created
    /// with, as the Java prompts fill it in.
    fn answer_credentials(&self, credentials: Option<Credentials>);

    /// The answer to a trust, unverified, question, security or message
    /// event. A no ends the session, except to a message that was not a
    /// question, where it is the only answer there is.
    fn answer_trust(&self, accept: bool);

    /// [`answer_trust`](Backend::answer_trust) for a security event, with the
    /// one thing that question has over the others: whether to remember the
    /// identity. Only RealVNC has a remember to carry; everyone else answers
    /// the plain way.
    fn answer_security(&self, accept: bool, remember: bool) {
        let _ = remember;
        self.answer_trust(accept);
    }

    /// An identity the library asked to have remembered, for the caller to
    /// keep and hand back to the next session: RealVNC's opaque blob, which
    /// its core sends after a security prompt answered with *remember*. The
    /// caller decides whether to keep it — the window does, the probe never
    /// asks.
    fn saved_identity(&self) -> Option<String> {
        None
    }

    /// Ask for a graceful disconnect. `Closed` follows.
    fn disconnect(&self);

    /// Ends the session, waits for its thread, and releases whatever the
    /// native side is holding. Nothing but `destroy` may be called afterwards.
    fn destroy(&self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_the_way_the_java_row_reads() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1000), "1.00 kB");
        assert_eq!(size(12_345), "12.3 kB");
        assert_eq!(size(123_456), "123 kB");
        assert_eq!(size(999_500), "1.00 MB");
        assert_eq!(size(2_500_000_000), "2.50 GB");
    }

    #[test]
    fn the_data_row_leaves_out_a_direction_nobody_counted() {
        let both = ConnectionFact::data(Traffic {
            received: 1500,
            sent: Some(20),
        });
        assert_eq!(both.value, "1.50 kB in · 20 B out");
        assert_eq!(both.label, "Transferred");
        let one = ConnectionFact::data(Traffic {
            received: 1500,
            sent: None,
        });
        assert_eq!(one.value, "1.50 kB in");
    }

    #[test]
    fn monitors_without_area_are_not_monitors() {
        let flat = [0, 0, 1920, 1200, 1920, 0, 0, 1200, 1920, 0, 1280, 800, 7];
        let monitors = Monitor::from_flat(&flat);
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[1].x, 1920);
        assert_eq!(monitors[1].width, 1280);
        // An id is the place in the list the library gave, gaps and all.
        assert_eq!(monitors[0].id, 0);
        assert_eq!(monitors[1].id, 2);
    }

    #[test]
    fn five_ints_a_screen_carry_the_far_ends_own_ids() {
        let screens = [
            Monitor {
                id: 0x8000_0001,
                x: 0,
                y: 0,
                width: 1280,
                height: 800,
            },
            Monitor {
                id: 7,
                x: 1280,
                y: 0,
                width: 1024,
                height: 768,
            },
        ];
        let flat = Monitor::flat5(&screens);
        assert_eq!(flat.len(), 10);
        assert_eq!(Monitor::from_flat5(&flat), screens);
        // And a screen with no area is dropped, as in four ints.
        assert_eq!(Monitor::from_flat5(&[3, 0, 0, 0, 800, 9]), []);
    }

    #[test]
    fn a_thumbnail_step_fits_both_bounds() {
        assert_eq!(thumbnail_step(1920, 1200, 240, 240), 8);
        assert_eq!(thumbnail_step(1920, 1200, 1000, 100), 12);
        assert_eq!(thumbnail_step(100, 100, 1000, 1000), 1);
        assert_eq!(thumbnail_step(0, 0, 0, 0), 1);
    }

    /// `0xAABBGGRR` as a little-endian word is `R, G, B, A` in memory.
    #[test]
    fn words_come_out_in_their_memory_order() {
        let p = Picture::from_words(2, 1, &[0x4433_2211, 0]);
        assert_eq!(p.rgba, [0x11, 0x22, 0x33, 0x44, 0, 0, 0, 0]);
    }
}
