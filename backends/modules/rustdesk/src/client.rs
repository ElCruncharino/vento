// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! One RustDesk session: the handshake, the message loop, and everything the
//! app can ask of a connection that is up.
//!
//! The protocol is small — one framed protobuf `Message` with a `oneof` of
//! thirty arms — and most of what their product is lives in arms this app has
//! no seam for. What is here is the seven that make a remote desktop: the login
//! exchange, the picture, the cursor, the clipboard, input in both of its
//! forms, and the timing message that has to be answered or a link saturates.
//!
//! Three things about the far end are not guessable from the `.proto` and each
//! cost a run to find:
//!
//! - **`TestDelay` is echoed unchanged**, and only when the flag in it says the
//!   peer started the exchange. Their server echoes anything with that flag
//!   set, so a client that sets it on a reply gets its own message back and
//!   answers it again — 9701 messages in fifteen seconds, on an idle session.
//! - **A down event carries every modifier held.** Their server *releases*, on
//!   the far end, each modifier not named in the event, so a Ctrl armed for the
//!   letter after it is a Ctrl let go the moment that letter arrives. The same
//!   is true of a mouse button going down, which is what makes Ctrl+click work.
//! - **A Windows peer with more than one session logged in waits**: it
//!   subscribes no video service at all until the client answers with a session
//!   id. Nothing announces the wait — the login succeeds, the desktop simply
//!   never comes.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use protobuf::{Message as _, MessageField};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::framebuffer::{Back, Framebuffer};
use crate::keymap::{self, Key};
use crate::crypto;
use crate::protos::message::{
    CaptureDisplays, Clipboard, ClipboardFormat, ControlKey, DisplayResolution, Hash, KeyEvent,
    ImageQuality, KeyboardMode, LoginRequest, Message, Misc, MouseEvent, OptionMessage, PeerInfo,
    PublicKey, Resolution, SupportedDecoding, SwitchDisplay, TestDelay, VideoFrame, login_response,
    message, misc, option_message::BoolOption, permission_info::Permission, supported_decoding::PreferCodec,
    video_frame,
};
use crate::rendezvous;
use crate::wire::{self, Traffic};
use media::audio::decode::Codec as SoundCodec;
use media::audio::{Format, Outputs, Player};
use runtime::{Handler, Hangup};
use media::video::{self, Codec, Decoded, Decoders, GpuPicture, Hardware};

/// Their default direct-access port, which is `RENDEZVOUS_PORT + 2`.
pub const DIRECT_PORT: u16 = 21118;

/// What we claim to be. Their server compares this against its own to decide
/// which of its compatibility paths to take, so an honest answer is the one
/// that avoids a path nothing here implements.
pub const VERSION: &str = "1.4.9";

/// Their `TestDelay` arrives about once a second whatever the desktop is doing,
/// so silence for this long is a link that has gone. A timeout is fatal rather
/// than retried: a read that gives up part-way through a frame leaves the
/// stream out of step, and there is nothing to resynchronise on.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How often to say something while a question is on somebody's screen, which
/// has to be comfortably under the thirty seconds of silence the peer allows.
const KEEP_ALIVE_EVERY: Duration = Duration::from_secs(8);

/// What the far end is told to send when the session is not on screen. There is
/// no pause message in this protocol, and one frame a second is the nearest
/// thing to one — the picture stays current enough to be worth showing the
/// moment it comes back, at a hundredth of the traffic.
const UNFOCUSED_FPS: i32 = 1;

/// How often the decoder's status line is made again: it is a string a
/// frame otherwise, for an overlay that is read by eye.
const STATUS_EVERY: Duration = Duration::from_millis(500);

pub struct Config {
    /// The peer: `host` or `host:port` for [`Reach::Direct`], and their nine
    /// digits for [`Reach::Id`].
    pub address: String,
    pub reach: Reach,
    pub password: Option<String>,
    pub connect_timeout: Duration,
    /// What the far end calls this phone in its own log and its own screen.
    pub my_name: String,
    /// What this end runs on, as the login says it.
    pub platform: Platform,
    /// The live options' opening values. What they are afterwards is the
    /// session's, since each of them can be changed while it is running.
    pub quality: Quality,
    /// 0 for the far end's own choice.
    pub fps: i32,
    pub codec: PreferredCodec,
    /// Whether the far end locks its screen when this session ends. Sent with
    /// the login and never changed: what it asks for happens after the last
    /// message either end sends.
    pub lock_after: bool,
    /// Where the peer's sound goes, or `None` for a session that never asks
    /// for it. Asked for only where the platform decodes Opus, which is all
    /// their peer sends.
    pub audio: Option<Arc<dyn Outputs>>,
    /// Whether the peer is asked for its sound at login: where the session's
    /// Enable switch starts, which asks again with their option message.
    pub sound: bool,
}

/// How long a stream stays open after its sound was heard: the gap between
/// one sound and the next it rides over, as the other backends' streams do.
/// Their peer goes on sending silence for eight seconds after its desktop
/// falls silent and then nothing (its "zero gate"), so this is also how a
/// session that has stopped sounding stops showing its Sound button, ten
/// seconds after.
const QUIET: Duration = Duration::from_secs(2);

/// What the login tells the peer this end runs on. Not a cosmetic field:
/// their peer keys features on it — a file clipboard only between two
/// Windows or two macOS ends, a printer only for a Windows one, the multiple
/// clipboard formats refused to an Android client older than 1.3.3 — and it
/// is part of how a trusted device is recognised, so each client says what
/// their own client would say on the same machine: `Android` from their
/// Android build, `whoami::platform()` (`Linux`) from their desktop one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Android,
    Linux,
}

impl Platform {
    /// Their spelling, which is `whoami::Platform`'s `Display`.
    pub fn wire(self) -> &'static str {
        match self {
            Platform::Android => "Android",
            Platform::Linux => "Linux",
        }
    }
}

/// The two ways to a peer, which are not two transports of one connection but
/// two different things to have chosen.
///
/// Direct access is a mode the far end switched on, reaching a machine somebody
/// can already address, and it has no key exchange at all. The id path reaches
/// a machine nothing on this network can address, and the encryption comes with
/// it rather than being a second decision: what makes the session verifiable is
/// the peer key the rendezvous server signs.
pub enum Reach {
    Direct,
    Id {
        /// `host` or `host:port`; empty for the public network.
        server: String,
        /// The server's public key, base64, as their `id_ed25519.pub` holds it;
        /// empty for the public network's own, which is a constant in every
        /// build of theirs.
        key: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Low,
    Balanced,
    Best,
}

impl Quality {
    fn wire(self) -> ImageQuality {
        match self {
            Quality::Low => ImageQuality::Low,
            Quality::Balanced => ImageQuality::Balanced,
            Quality::Best => ImageQuality::Best,
        }
    }

    fn of(ordinal: i32) -> Quality {
        match ordinal {
            0 => Quality::Low,
            2 => Quality::Best,
            _ => Quality::Balanced,
        }
    }

    fn ordinal(self) -> i32 {
        match self {
            Quality::Low => 0,
            Quality::Balanced => 1,
            Quality::Best => 2,
        }
    }
}

/// Which codec the far end is asked to encode in.
///
/// A preference rather than an instruction: the peer picks from what it can
/// encode and what this end says it can decode, and [`Auto`](Self::Auto) leaves
/// it to choose — which on their server means whatever hardware encoder that
/// machine has, falling back to VP9, since VP9 is the one codec compiled into
/// every build of theirs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferredCodec {
    Auto,
    Vp9,
    Vp8,
    Av1,
    H264,
    H265,
}

impl PreferredCodec {
    fn wire(self) -> PreferCodec {
        match self {
            PreferredCodec::Auto => PreferCodec::Auto,
            PreferredCodec::Vp9 => PreferCodec::VP9,
            PreferredCodec::Vp8 => PreferCodec::VP8,
            PreferredCodec::Av1 => PreferCodec::AV1,
            PreferredCodec::H264 => PreferCodec::H264,
            PreferredCodec::H265 => PreferCodec::H265,
        }
    }

    /// The codec this asks for, or `None` for [`Auto`](Self::Auto). What it is
    /// for is the check before it is sent: asking for one this end cannot
    /// decode is a session with no picture, and the far end has no way to know.
    fn codec(self) -> Option<Codec> {
        match self {
            PreferredCodec::Auto => None,
            PreferredCodec::Vp9 => Some(Codec::Vp9),
            PreferredCodec::Vp8 => Some(Codec::Vp8),
            PreferredCodec::Av1 => Some(Codec::Av1),
            PreferredCodec::H264 => Some(Codec::H264),
            PreferredCodec::H265 => Some(Codec::H265),
        }
    }

    fn ordinal(self) -> i32 {
        match self {
            PreferredCodec::Auto => 0,
            PreferredCodec::Vp9 => 1,
            PreferredCodec::Vp8 => 2,
            PreferredCodec::Av1 => 3,
            PreferredCodec::H264 => 4,
            PreferredCodec::H265 => 5,
        }
    }

    fn of(ordinal: i32) -> PreferredCodec {
        match ordinal {
            1 => PreferredCodec::Vp9,
            2 => PreferredCodec::Vp8,
            3 => PreferredCodec::Av1,
            4 => PreferredCodec::H264,
            5 => PreferredCodec::H265,
            _ => PreferredCodec::Auto,
        }
    }
}

/// What the handshake settled, and what is left to ask about it.
enum Verified {
    /// The peer proved it holds the key the rendezvous server vouched for, and
    /// both halves of the socket are sealed under a key agreed with it.
    Encrypted([u8; 32]),
    /// Something did not check out, in these words, and the session is in the
    /// clear — which the app has still to agree to.
    Clear(String),
}

/// Everything the connection panel can be told.
#[derive(Clone, Default)]
pub struct Info {
    pub desktop_name: String,
    pub protocol: String,
    pub connection: String,
    pub security: String,
    pub encoding: String,
    /// The round trip their own timing message measures, which is the only
    /// number about the link this protocol offers.
    pub round_trip: String,
    pub platform: String,
    pub display: String,
}

/// The decoder a session is drawing with, and what it was made for: a codec,
/// a picture size and a way of decoding, a change in any of which makes it
/// again.
struct Current {
    codec: Codec,
    size: (usize, usize),
    hardware: Hardware,
    /// What the Encoding row last said of where its pictures come from.
    shown: Option<bool>,
    /// When its status line was last made, `None` before the first.
    status_at: Option<Instant>,
    decoder: Box<dyn video::Decoder>,
}

/// The Encoding row: the codec, and whether its pictures come out of the
/// hardware, where the decoder knows — nothing is said before the first
/// picture, nor by a decoder that never knows.
fn encoding(codec: Codec, hardware: Option<bool>) -> String {
    match hardware {
        Some(true) => format!("{} (hardware)", codec.name()),
        Some(false) => format!("{} (software)", codec.name()),
        None => codec.name().to_string(),
    }
}

/// One button of theirs, already shifted into the place a mask wants it.
mod mouse {
    pub const TYPE_MOVE: i32 = 0;
    pub const TYPE_DOWN: i32 = 1;
    pub const TYPE_UP: i32 = 2;
    pub const TYPE_WHEEL: i32 = 3;

    pub const LEFT: i32 = 0x01;
    pub const RIGHT: i32 = 0x02;
    pub const MIDDLE: i32 = 0x04;
    pub const BACK: i32 = 0x08;
    pub const FORWARD: i32 = 0x10;
}

/// An RFB button mask, which is what the gesture stack speaks, against theirs.
/// The wheel is not in here: it is four bits of the same mask and becomes an
/// axis rather than a button.
const BUTTONS: [(i32, i32); 5] = [
    (0x01, mouse::LEFT),
    (0x02, mouse::MIDDLE),
    (0x04, mouse::RIGHT),
    (0x80, mouse::BACK),
    (0x100, mouse::FORWARD),
];

/// How far one notch of the wheel scrolls. Their server hands the number
/// straight to the platform's own scroll call, so this is in whatever that
/// calls a line — and one notch is one line on both far ends the rig has.
const WHEEL_NOTCH: i32 = 1;

/// What [`Client::reach`] comes back with: the socket the session runs on, the
/// introduction a peer reached by id still has to be put through (its signed
/// key and the rendezvous key that vouches for it), and the panel's word for
/// the connection.
struct Dialled {
    sock: TcpStream,
    introduction: Option<(Vec<u8>, String)>,
    connection: String,
}

pub struct Client {
    framebuffer: RwLock<Framebuffer>,
    /// Whoever decodes the picture, which is the platform's to say and is not
    /// known here: what the far end is told this end can take is asked of it
    /// whenever the options go out, and a decoder is made from it whenever the
    /// codec or the size changes.
    decoders: Arc<dyn Decoders>,
    traffic: Arc<Traffic>,
    /// Serialized messages on their way to the writer thread. Input is encoded
    /// on whichever thread produced it and written on a thread of its own, so a
    /// stalled socket never reaches the main one.
    out: Mutex<Option<Sender<Vec<u8>>>>,
    /// The sockets, for closing them from another thread: a read blocked on a
    /// socket — or a dial — wakes when it is shut down and there is nothing
    /// else to interrupt it with. Every socket of the rendezvous is held here
    /// while it is the one being waited on, as well as the session's own.
    hangup: Hangup,
    info: Mutex<Info>,
    /// The decoder's status line, for an overlay: made on the protocol
    /// thread, where the decoder is, at most every [`STATUS_EVERY`].
    decoder_status: Mutex<Option<String>>,
    /// What the login names itself as, which for a direct connection is the
    /// address the socket actually went to and for an id is that id.
    login_as: Mutex<String>,
    /// The sizes the far end will take, which is a fixed list rather than RFB's
    /// free choice — empty for a peer that offers none.
    resolutions: Mutex<Vec<(i32, i32)>>,
    /// The far end's displays as rectangles of its whole desktop, in the peer's
    /// own order — empty where it has only one, since there is then nothing to
    /// choose between.
    displays: Mutex<Vec<(i32, i32, i32, i32)>>,
    /// Where the captured display's top left corner sits on the peer's whole
    /// desktop. Input is injected into that desktop rather than into the
    /// picture, so a pointer event on a second screen is off by this much
    /// unless it is added — landing on whatever is at those coordinates on the
    /// primary one. The pair is under one lock because a switch moves both.
    origin: Mutex<(i32, i32)>,
    /// What each held key was sent as, by the id the app ties an up to a down
    /// with.
    keys: Mutex<HashMap<i64, Key>>,
    /// The modifiers held, in the order they went down. Every down event
    /// carries this list.
    modifiers: Mutex<Vec<ControlKey>>,
    /// Where the pointer was last put, so that a button press does not have to
    /// repeat a move that has not changed.
    pointer: Mutex<(i32, i32, i32)>,
    view_only: AtomicBool,
    focused: AtomicBool,
    width: AtomicI32,
    height: AtomicI32,
    display: AtomicI32,
    /// The live options: what a running session has been told, which is where
    /// they live because every one of them can be changed while it is up.
    quality: AtomicI32,
    /// The configured frame rate, which unfocusing lowers and focusing puts
    /// back.
    fps: AtomicI32,
    /// The codec asked for, and the last one the peer was told about. The
    /// second is what keeps a quality change from being a codec renegotiation:
    /// the preference goes out only when it has moved.
    codec: AtomicI32,
    codec_sent: AtomicI32,
    /// The last round trip their timing message measured, in milliseconds.
    round_trip: AtomicU64,
    /// Whether the options ask the peer for its sound: set from the
    /// [`Config`] when the session runs and by the session's switch, read
    /// whenever the options go out. Shared with the switch's call, which
    /// comes on a window's thread.
    sound: Arc<AtomicBool>,
}

impl Client {
    /// A client that decodes its picture with `decoders`.
    ///
    /// Given here rather than in the [`Config`] because the question it answers
    /// is asked outside [`run`](Client::run): a live option change on whichever
    /// thread made it re-sends what this end can decode, and only the client is
    /// shared with that thread.
    pub fn new(decoders: Arc<dyn Decoders>) -> Client {
        Client {
            framebuffer: RwLock::new(Framebuffer::new()),
            decoders,
            traffic: Arc::new(Traffic::default()),
            out: Mutex::new(None),
            hangup: Hangup::new(),
            info: Mutex::new(Info::default()),
            decoder_status: Mutex::new(None),
            login_as: Mutex::new(String::new()),
            resolutions: Mutex::new(Vec::new()),
            displays: Mutex::new(Vec::new()),
            origin: Mutex::new((0, 0)),
            keys: Mutex::new(HashMap::new()),
            modifiers: Mutex::new(Vec::new()),
            pointer: Mutex::new((-1, -1, 0)),
            view_only: AtomicBool::new(false),
            focused: AtomicBool::new(true),
            width: AtomicI32::new(0),
            height: AtomicI32::new(0),
            display: AtomicI32::new(0),
            quality: AtomicI32::new(Quality::Balanced.ordinal()),
            fps: AtomicI32::new(0),
            codec: AtomicI32::new(PreferredCodec::Auto.ordinal()),
            codec_sent: AtomicI32::new(-1),
            round_trip: AtomicU64::new(0),
            sound: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn framebuffer(&self) -> &RwLock<Framebuffer> {
        &self.framebuffer
    }

    /// Received and sent, in bytes, since the socket was opened.
    pub fn traffic(&self) -> (u64, u64) {
        (
            self.traffic.received.load(Ordering::Relaxed),
            self.traffic.sent.load(Ordering::Relaxed),
        )
    }

    pub fn info(&self) -> Info {
        let mut info = self.info.lock().unwrap().clone();
        let rtt = self.round_trip.load(Ordering::Relaxed);
        info.round_trip = if rtt == 0 {
            String::new()
        } else {
            format!("{rtt} ms")
        };
        info
    }

    /// The picture, where it was left on the GPU (see [`Client::picture`]).
    pub fn gpu_picture(&self) -> Option<Arc<dyn GpuPicture>> {
        self.framebuffer.read().unwrap().gpu()
    }

    /// How the picture is being decoded, as the decoder last said: `None`
    /// before the first frame, and where there is no decoder.
    pub fn decoder_status(&self) -> Option<String> {
        self.decoder_status.lock().unwrap().clone()
    }

    /// Whether the far end offers a size it is not already at.
    ///
    /// Unlike RFB this is a list rather than a free choice, so a request is
    /// answered with the nearest of them — and a peer whose list is the one
    /// mode it is running, which is every Xvfb, offers a control that could
    /// only ever choose what is already on screen.
    pub fn can_resize(&self) -> bool {
        if self.view_only.load(Ordering::Relaxed) {
            return false;
        }
        let (w, h) = (
            self.width.load(Ordering::Relaxed),
            self.height.load(Ordering::Relaxed),
        );
        self.resolutions
            .lock()
            .unwrap()
            .iter()
            .any(|offered| *offered != (w, h))
    }

    /// The far end's displays, four numbers each — x, y, width, height in its
    /// own desktop — and which of them is being captured.
    ///
    /// This protocol sends **one** display at a time, so the list is a choice
    /// rather than a description of the picture: what is on screen is one of
    /// these rectangles and the others are not in it anywhere.
    pub fn displays(&self) -> (Vec<i32>, i32) {
        let displays = self.displays.lock().unwrap();
        let flat = displays
            .iter()
            .flat_map(|(x, y, w, h)| [*x, *y, *w, *h])
            .collect();
        (flat, self.display.load(Ordering::Relaxed))
    }

    /// Ask the far end to capture another of its displays. What arrives back is
    /// a `SwitchDisplay`, so nothing here assumes it worked.
    pub fn request_display(&self, index: i32) {
        if index < 0 || index as usize >= self.displays.lock().unwrap().len() {
            return;
        }
        let mut switch = SwitchDisplay::new();
        switch.display = index;
        let mut misc = Misc::new();
        misc.set_switch_display(switch);
        let mut out = Message::new();
        out.set_misc(misc);
        self.send(out);
    }

    /// Which display this end wants, and a fresh picture of it.
    ///
    /// The pair a client of this version is expected to send once the peer has
    /// said what it is capturing — at the login, and again every time that
    /// changes.
    fn ask_for(&self, display: i32) {
        let mut captured = CaptureDisplays::new();
        captured.set = vec![display];
        let mut misc = Misc::new();
        misc.set_capture_displays(captured);
        let mut out = Message::new();
        out.set_misc(misc);
        self.send(out);
        self.refresh_video();
    }

    /// Ask for a fresh picture, which their server sends as a key frame: what
    /// a decoder that starts mid-stream needs before it can draw anything.
    fn refresh_video(&self) {
        let mut misc = Misc::new();
        misc.set_refresh_video(true);
        let mut out = Message::new();
        out.set_misc(misc);
        self.send(out);
    }

    pub fn close(&self) {
        self.hangup.hang_up();
    }

    // ---- the session ------------------------------------------------------

    /// Connect, log in, and run until the far end or this end stops it. The
    /// calling thread is the protocol thread and every callback arrives on it.
    pub fn run(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        self.quality.store(config.quality.ordinal(), Ordering::Relaxed);
        self.fps.store(config.fps, Ordering::Relaxed);
        self.codec.store(config.codec.ordinal(), Ordering::Relaxed);
        self.sound.store(config.sound && sound_of(config).is_some(), Ordering::Relaxed);
        // No switch past the session, however it ends.
        let _withdraw = Withdraw(sound_of(config));

        // How the socket is obtained is the whole of the difference between the
        // two modes: everything past this point is one session either way.
        let Dialled {
            sock,
            introduction,
            connection,
        } = match self.reach(config) {
            Err(_) if self.hangup.is_hung_up() => return Err(Error::Closed),
            other => other?,
        };
        // Held for the rest of the session, the introduction with it.
        let _holding = match self.hangup.hold(&sock) {
            Err(_) if self.hangup.is_hung_up() => return Err(Error::Closed),
            other => other?,
        };
        let (mut reader, mut writer, _) = wire::split(sock, Arc::clone(&self.traffic))?;
        self.info.lock().unwrap().connection = connection;

        let verified = match introduction {
            Some((signed_id_pk, key)) => {
                reader.set_timeout(Some(config.connect_timeout))?;
                let verified =
                    self.secure(&mut reader, &mut writer, &config.address, &signed_id_pk, &key)?;
                let mut info = self.info.lock().unwrap();
                // Which server vouched is the whole of the trust difference
                // between the two ways of using this path: a key somebody
                // pasted is their own pin, and the public network's is a pin
                // chosen by RustDesk. And a session somebody agreed to have
                // anyway says both halves of what it is.
                info.security = match verified {
                    Verified::Encrypted(_) => format!(
                        "xsalsa20-poly1305, vouched for by {}",
                        if key.trim().is_empty() {
                            "the public network"
                        } else {
                            "your rendezvous server"
                        }
                    ),
                    Verified::Clear(_) => {
                        "None (unencrypted), and the machine was not verified".into()
                    }
                };
                Some(verified)
            }
            None => {
                // Not a footnote and not only in the log: this mode has no key
                // exchange in it at all, and the panel is where a session says
                // what it is protected by.
                self.info.lock().unwrap().security = "None (unencrypted)".into();
                // A direct-access peer expects a client with no key to say so
                // with an empty message, which is the whole of that mode's
                // handshake.
                writer.send(&Message::new())?;
                None
            }
        };

        let (tx, rx) = channel::<Vec<u8>>();
        *self.out.lock().unwrap() = Some(tx);
        let writing = std::thread::Builder::new()
            .name("rustdesk-writer".into())
            .spawn(move || {
                for bytes in rx {
                    if writer.send_bytes(bytes).is_err() {
                        break;
                    }
                }
                writer.close();
            })
            .map_err(Error::Io)?;

        reader.set_timeout(Some(config.connect_timeout))?;
        let outcome = self.session(&mut reader, config, verified, handler);

        // Whether this end asked, read **before** the tidying below closes the
        // socket itself: an error that arrived because somebody pressed
        // Disconnect is not one the screen should be told about, and one that
        // arrived on its own is.
        let asked = self.hangup.is_hung_up();
        // The sender is what keeps the writer thread alive, so it goes before
        // the join; the socket is shut down either way, since an ending that
        // came from the far end has not been through `close`.
        self.out.lock().unwrap().take();
        self.close();
        let _ = writing.join();
        match outcome {
            Err(_) if asked => Err(Error::Closed),
            other => other,
        }
    }

    fn reach(&self, config: &Config) -> Result<Dialled> {
        let (sock, introduction, connection) = match &config.reach {
            Reach::Direct => {
                let addr = resolve(&config.address)?;
                log::info!("connecting to {addr}");
                let sock = self.hangup.dial(addr, config.connect_timeout)?;
                sock.set_nodelay(true)?;
                // What the login says it dialled is this rather than what was
                // typed, which their peer checks against three regular
                // expressions: a v4 literal with an optional port, a v6 one
                // either bare or bracketed **with** a port, or a dotted name
                // with one. A bracketed literal and no port matches none of
                // them, and the refusal is `Offline`.
                *self.login_as.lock().unwrap() = addr.to_string();
                (sock, None, format!("Direct access to {addr}"))
            }
            Reach::Id { server, key } => {
                let server = if server.trim().is_empty() {
                    rendezvous::PUBLIC_SERVER
                } else {
                    server.trim()
                };
                let reached = rendezvous::reach(
                    &self.hangup,
                    &config.address,
                    server,
                    key.trim(),
                    config.connect_timeout,
                )?;
                let connection = reached.how.describe();
                // The id, which is what the peer is expecting to be called here.
                *self.login_as.lock().unwrap() = config.address.clone();
                (
                    reached.sock,
                    Some((reached.signed_id_pk, key.clone())),
                    connection,
                )
            }
        };
        Ok(Dialled {
            sock,
            introduction,
            connection,
        })
    }

    /// The handshake the direct mode has none of, ending with both halves of
    /// the socket sealed — or, where something did not check out, with neither
    /// and with the reason.
    ///
    /// **Nothing is asked here, and that is a deadline rather than a
    /// preference.** The peer gives a handshake **18 seconds** and drops the
    /// connection when it passes, which is less than a person takes to read a
    /// dialog — measured, by leaving one on screen. So the exchange is finished
    /// at machine speed and the question goes to the app afterwards, before the
    /// login, which is the first thing on this wire that is anybody's secret.
    ///
    /// **A failure is a question rather than a refusal or a shrug.** Their own
    /// client falls back to an unencrypted session whenever any step does not
    /// check out — a signature that fails, a key that is not the one the server
    /// vouched for, a peer that sends no `SignedId` at all — and says so only in
    /// its log, which makes a session's own claim about itself worthless.
    /// Refusing outright is the other end of that and deletes a session
    /// somebody may still want.
    fn secure(
        &self,
        reader: &mut wire::Reader,
        writer: &mut wire::Writer,
        peer_id: &str,
        signed_id_pk: &[u8],
        server_key: &str,
    ) -> Result<Verified> {
        let server_key = crypto::server_key(if server_key.trim().is_empty() {
            rendezvous::PUBLIC_KEY
        } else {
            server_key
        })?;
        // One: the server's introduction, which says this id belongs to this
        // long-term key.
        let vouched = if signed_id_pk.is_empty() {
            Err("The rendezvous server did not vouch for a key.".to_string())
        } else {
            crypto::open_id_pk(signed_id_pk, &server_key, peer_id)
                .map_err(|e| format!("The rendezvous server's introduction did not check out: {e}."))
        };
        let peer_key = match vouched {
            Ok(key) => key,
            // Nothing has been read from the peer yet, so what it is waiting
            // for is a client with no key saying so — their empty message.
            Err(why) => return self.in_the_clear(writer, why, false),
        };
        // Two: the peer proving it holds that key, over a box key it made for
        // this connection alone. The question goes to the app between the two,
        // so nothing of ours is sealed to a machine somebody has refused.
        let signed = match reader.message::<Message>()?.union {
            Some(message::Union::SignedId(signed)) => signed,
            _ => {
                return self.in_the_clear(
                    writer,
                    "The machine offered no key of its own.".into(),
                    false,
                );
            }
        };
        let box_key = match crypto::open_id_pk(&signed.id, &peer_key, peer_id) {
            Ok(key) => key,
            // The peer is waiting for a `PublicKey` here, so the way to say
            // there will be no key is an empty one — theirs does the same.
            Err(e) => {
                return self.in_the_clear(
                    writer,
                    format!("The machine's key is not the one it was vouched for: {e}."),
                    true,
                );
            }
        };
        // Three: a key of ours, sealed to theirs. Everything after this frame
        // is encrypted, in both directions, counted separately.
        let (our_box_pk, sealed, key) = crypto::seal_session_key(&box_key)?;
        let mut public_key = PublicKey::new();
        public_key.asymmetric_value = our_box_pk.into();
        public_key.symmetric_value = sealed.into();
        let mut out = Message::new();
        out.set_public_key(public_key);
        writer.send(&out)?;
        writer.set_cipher(Box::new(crypto::Secretbox::new(key)));
        reader.set_cipher(Box::new(crypto::Secretbox::new(key)));
        log::info!("the session is encrypted");
        Ok(Verified::Encrypted(peer_key))
    }

    /// Tell the peer there will be no key, and hand back the reason for the
    /// question the app is about to be asked.
    ///
    /// `expects_public_key` is whether the peer has already offered one: after
    /// its `SignedId` it is waiting for a `PublicKey`, and an empty one is how
    /// their own client declines; before that it is waiting for the empty
    /// message a client with no key of its own sends.
    fn in_the_clear(
        &self,
        writer: &mut wire::Writer,
        why: String,
        expects_public_key: bool,
    ) -> Result<Verified> {
        log::warn!("cannot verify the peer: {why}");
        let mut out = Message::new();
        if expects_public_key {
            out.set_public_key(PublicKey::new());
        }
        writer.send(&out)?;
        Ok(Verified::Clear(why))
    }

    fn session(
        &self,
        reader: &mut wire::Reader,
        config: &Config,
        verified: Option<Verified>,
        handler: &mut dyn Handler,
    ) -> Result<()> {
        // Their server opens with `Hash` before anything is asked of it.
        let hash = loop {
            match reader.message::<Message>()?.union {
                Some(message::Union::Hash(h)) => break h,
                Some(other) => log::info!("before Hash: {}", arm(&other)),
                None => {}
            }
        };
        // Whatever the handshake left to ask, asked here: after it, because the
        // peer times a handshake out under a person reading a dialog, and
        // before the login, because that is what carries the password. The
        // asking blocks this thread, so something else has to keep the socket
        // from going quiet while it does.
        let agreed = match &verified {
            Some(Verified::Encrypted(peer_key)) => {
                let alive = self.keep_alive();
                let answer = handler.trust(&crypto::fingerprint(peer_key));
                alive.stop();
                answer
            }
            Some(Verified::Clear(why)) => {
                let alive = self.keep_alive();
                let answer = handler.unverified(why);
                alive.stop();
                answer
            }
            None => true,
        };
        if !agreed {
            return Err(Error::Cancelled);
        }
        let mut password = config.password.clone().unwrap_or_default();
        self.send_login(config, &hash, &password);
        reader.set_timeout(Some(IDLE_TIMEOUT))?;

        let mut decoder: Option<Current> = None;
        let mut back = Back::new();
        let mut rate = Rate::new();
        let mut cursors: HashMap<u64, Cursor> = HashMap::new();
        let mut connected = false;
        // The peer's sound and the format it last said it is in. A peer
        // sends something at least every second (its timing message), so a
        // check for quiet after every message is often enough.
        let mut sound = sound_of(config).map(Player::new);
        let mut sound_format: Option<Format> = None;
        // Whether the peer lets this session have its sound: their peer says
        // so (a PermissionInfo) only where it does not, as the connection
        // starts and whenever the person over there changes it.
        let mut sound_permitted = true;

        loop {
            let message = match reader.message::<Message>() {
                Ok(m) => m,
                Err(e) if timed_out(&e) => return Err(Error::Silent),
                Err(e) => return Err(Error::Io(e)),
            };
            if let Some(player) = &mut sound {
                // Turned off, at once: what the peer sent before it heard is
                // not worth the second it would take to play out.
                if !self.sound.load(Ordering::Relaxed) {
                    player.close();
                }
                player.close_if_quiet(QUIET);
            }
            let Some(union) = message.union else { continue };
            match union {
                message::Union::LoginResponse(response) => match response.union {
                    Some(login_response::Union::PeerInfo(peer)) => {
                        self.arrived(&peer, handler, &mut back)?;
                        connected = true;
                        // Logged in, the peer may send its sound: an
                        // Enable switch, since the option message that
                        // asked for it at login asks again at any time.
                        if sound_permitted && let Some(audio) = sound_of(config) {
                            self.offer_sound(&audio);
                        }
                    }
                    // Both password errors are answered on the same connection:
                    // their client re-prompts and sends another `LoginRequest`
                    // rather than dialling again, and the peer is waiting for
                    // exactly that.
                    Some(login_response::Union::Error(why))
                        if why == "Wrong Password" || why == "Empty Password" =>
                    {
                        log::info!("the peer wants a password ({why})");
                        match handler.credentials(false).map(|c| c.password) {
                            Some(answer) => {
                                password = answer;
                                self.send_login(config, &hash, &password);
                            }
                            None => return Err(Error::Cancelled),
                        }
                    }
                    Some(login_response::Union::Error(why)) => return Err(Error::Refused(why)),
                    None => {}
                },
                message::Union::VideoFrame(frame) => {
                    if connected {
                        let started = Instant::now();
                        self.picture(frame, &mut decoder, &mut back, handler);
                        rate.frame(started.elapsed(), back.converted());
                        // Every frame, drawn or not: this is what the peer is
                        // waiting for before it captures the next one, so a
                        // frame that changed nothing still has to be answered.
                        let mut misc = Misc::new();
                        misc.set_video_received(true);
                        let mut out = Message::new();
                        out.set_misc(misc);
                        self.send(out);
                    }
                }
                message::Union::CursorData(data) => {
                    let id = data.id;
                    if let Some(cursor) = Cursor::of(data) {
                        cursor.deliver(handler);
                        cursors.insert(id, cursor);
                    }
                }
                message::Union::CursorId(id) => {
                    if let Some(cursor) = cursors.get(&id) {
                        cursor.deliver(handler);
                    }
                }
                message::Union::Clipboard(clip) => self.clipboard_arrived(clip, handler),
                message::Union::MultiClipboards(many) => {
                    // Whichever of them is text; the rest are formats the seam
                    // has no concept of.
                    for clip in many.clipboards {
                        self.clipboard_arrived(clip, handler);
                    }
                }
                message::Union::TestDelay(delay) => {
                    if !delay.from_client {
                        // The round trip comes free with it: `last_delay` is
                        // what the peer measured from the previous exchange, so
                        // a client that echoes promptly is a client that has
                        // been told its own latency and never has to ask.
                        if delay.last_delay > 0 {
                            self.round_trip.store(delay.last_delay as u64, Ordering::Relaxed);
                        }
                        // Echoed **unchanged**. Setting the flag here is what
                        // turns one message a second into a busy loop between
                        // the two ends.
                        let mut out = Message::new();
                        out.set_test_delay(delay);
                        self.send(out);
                    }
                }
                message::Union::MessageBox(box_) => {
                    log::info!("message box: [{}] {} {}", box_.msgtype, box_.title, box_.text);
                }
                message::Union::Misc(misc) => match misc.union {
                    Some(misc::Union::CloseReason(why)) => return Err(Error::Refused(why)),
                    Some(misc::Union::PermissionInfo(permission))
                        if permission.permission.enum_value() == Ok(Permission::Audio) =>
                    {
                        log::info!("the peer {} its sound", if permission.enabled { "allows" } else { "withholds" });
                        sound_permitted = permission.enabled;
                        if let Some(audio) = sound_of(config).filter(|_| connected) {
                            if permission.enabled {
                                self.offer_sound(&audio);
                            } else {
                                audio.controls().withdraw();
                            }
                        }
                    }
                    Some(misc::Union::SwitchDisplay(display)) => {
                        self.switched(&display, handler, &mut back);
                    }
                    // What the Opus that follows was encoded at: one of Opus's
                    // own rates, which their peer resamples its device to.
                    Some(misc::Union::AudioFormat(format)) => {
                        sound_format = Format::new(format.sample_rate, format.channels as usize);
                        log::info!(
                            "the peer sends its sound as Opus at {} Hz in {} channel(s){}",
                            format.sample_rate,
                            format.channels,
                            if sound_format.is_some() { "" } else { ", which is not played" }
                        );
                    }
                    _ => {}
                },
                // Frames that were on their way when the switch was turned
                // off are dropped rather than opening a stream again.
                message::Union::AudioFrame(frame) if self.sound.load(Ordering::Relaxed) => {
                    if let (Some(player), Some(format)) = (&mut sound, sound_format) {
                        player.packet(SoundCodec::Opus, format, &[], &frame.data);
                    }
                }
                _ => {}
            }
        }
    }

    /// Everything that happens once, when the peer has said who it is.
    fn arrived(
        &self,
        peer: &PeerInfo,
        handler: &mut dyn Handler,
        back: &mut Back,
    ) -> Result<()> {
        let current = peer.current_display.max(0) as usize;
        let Some(display) = peer.displays.get(current) else {
            return Err(Error::Protocol("the peer has no display".into()));
        };
        log::info!(
            "{} on {} running {}, display {} of {}, {}x{}",
            peer.hostname,
            peer.platform,
            peer.version,
            current + 1,
            peer.displays.len(),
            display.width,
            display.height
        );
        self.display.store(current as i32, Ordering::Relaxed);
        *self.origin.lock().unwrap() = (display.x, display.y);
        {
            let mut info = self.info.lock().unwrap();
            info.desktop_name = peer.hostname.clone();
            info.protocol = format!("RustDesk {}", peer.version);
            info.platform = peer.platform.clone();
        }
        // A peer with one display is a peer with nothing to choose, and an
        // empty list is what the panel reads as "no row here".
        *self.displays.lock().unwrap() = if peer.displays.len() > 1 {
            peer.displays
                .iter()
                .map(|d| (d.x, d.y, d.width, d.height))
                .collect()
        } else {
            Vec::new()
        };
        self.say_display(current as i32);
        *self.resolutions.lock().unwrap() = peer
            .resolutions
            .resolutions
            .iter()
            .map(|r| (r.width, r.height))
            .collect();

        // A Windows peer with RDP sharing on and more than one session logged
        // in subscribes no video service until it is told which session to
        // capture. Answering with anything other than the one the service is in
        // hands the connection to that session and drops this one, so the
        // answer is the peer's own current one.
        if let Some(sessions) = peer.windows_sessions.as_ref() {
            log::info!(
                "the peer offers {} windows sessions, current {}",
                sessions.sessions.len(),
                sessions.current_sid
            );
            let mut misc = Misc::new();
            misc.set_selected_sid(sessions.current_sid);
            let mut out = Message::new();
            out.set_misc(misc);
            self.send(out);
        }
        // Which display this end wants captured, and a refresh so the first
        // picture is a key frame rather than a wait for whatever the encoder
        // does next.
        self.ask_for(current as i32);

        self.resize(display.width, display.height, back);
        self.width.store(display.width, Ordering::Relaxed);
        self.height.store(display.height, Ordering::Relaxed);
        handler.connected(display.width.max(0) as usize, display.height.max(0) as usize);
        Ok(())
    }

    /// The far end changed what it is capturing, or how big it is.
    ///
    /// One message for both, and the peer sends it whether the change was asked
    /// for or not — a display unplugged over there arrives here as the same
    /// thing a switch does. The sizes it will take come with it, since they are
    /// the new display's rather than the old one's.
    fn switched(
        &self,
        switch: &SwitchDisplay,
        handler: &mut dyn Handler,
        back: &mut Back,
    ) {
        let (width, height) = (switch.width, switch.height);
        // Ahead of the size checks below, both of which return early: where the
        // picture is on the peer's desktop is the one thing here that decides
        // where input lands, and a switch that changed nothing else has still
        // moved it.
        *self.origin.lock().unwrap() = (switch.x, switch.y);
        if self.display.swap(switch.display, Ordering::Relaxed) != switch.display {
            // The capture set and the key frame again, for the same reason the
            // login sends them: the peer has said what it is capturing now, and
            // this end has to say what it wants of it.
            self.ask_for(switch.display);
        }
        self.say_display(switch.display);
        if !switch.resolutions.resolutions.is_empty() {
            *self.resolutions.lock().unwrap() = switch
                .resolutions
                .resolutions
                .iter()
                .map(|r| (r.width, r.height))
                .collect();
        }
        if width <= 0 || height <= 0 {
            return;
        }
        // Both swapped before either is compared: `&&` would leave the height
        // alone whenever the width had changed, which is a framebuffer sized
        // 1024×1440 for a 1024×768 display and a picture that never arrives.
        let was_width = self.width.swap(width, Ordering::Relaxed);
        let was_height = self.height.swap(height, Ordering::Relaxed);
        if was_width == width && was_height == height {
            return;
        }
        log::info!("the desktop is now {width}x{height}");
        self.resize(width, height, back);
        handler.desktop_size(width as usize, height as usize);
    }

    /// The panel's diagnostic line, which says which of the peer's displays the
    /// picture is — and nothing at all where there is only one, since a desktop
    /// having a screen is not news.
    fn say_display(&self, current: i32) {
        let count = self.displays.lock().unwrap().len();
        self.info.lock().unwrap().display = if count > 1 {
            format!("{} of {}", current + 1, count)
        } else {
            String::new()
        };
    }

    fn resize(&self, width: i32, height: i32, back: &mut Back) {
        let (w, h) = (width.max(0) as usize, height.max(0) as usize);
        self.framebuffer.write().unwrap().resize(w, h);
        back.resize(w, h);
    }

    /// One update, which in this protocol is always a whole picture: there are
    /// no damage rectangles anywhere in it, so what moved is found here rather
    /// than read off the wire.
    ///
    /// Where whoever draws the session takes pictures on the GPU (the
    /// platform's decoders say so, every frame), a picture the decoder can
    /// leave there is left there and becomes the framebuffer's picture as it
    /// is, damaging the whole desktop: there is no conversion to find what
    /// moved, and nothing here reads it. The next picture converted on the
    /// processor is then converted whole, its buffer knowing nothing of the
    /// one before.
    fn picture(
        &self,
        frame: VideoFrame,
        decoder: &mut Option<Current>,
        back: &mut Back,
        handler: &mut dyn Handler,
    ) {
        let Some(union) = frame.union else { return };
        let (codec, frames) = match union {
            video_frame::Union::Vp9s(v) => (Codec::Vp9, v.frames),
            video_frame::Union::Vp8s(v) => (Codec::Vp8, v.frames),
            video_frame::Union::Av1s(v) => (Codec::Av1, v.frames),
            video_frame::Union::H264s(v) => (Codec::H264, v.frames),
            video_frame::Union::H265s(v) => (Codec::H265, v.frames),
            // Raw pixels, which their server only sends where nothing can
            // encode; nothing here asks for it and no run has produced one.
            video_frame::Union::Rgb(_) | video_frame::Union::Yuv(_) => {
                log::warn!("the peer sent unencoded pixels, which this client does not read");
                return;
            }
        };
        let (w, h) = (
            self.width.load(Ordering::Relaxed).max(0) as usize,
            self.height.load(Ordering::Relaxed).max(0) as usize,
        );
        if w == 0 || h == 0 || back.pixels.len() != w * h {
            return;
        }
        // The size as well as the codec: a decoder is configured for one
        // picture size, and this protocol changes that under a running session
        // whenever the far end switches display. And the way of decoding,
        // which the platform's decoders say and which can be switched under a
        // running session too: asked every frame, that is the switch taking
        // effect at the next picture, with no message of its own to wait for.
        let hardware = self.decoders.hardware();
        let made_for = |c: &Current| c.codec == codec && c.size == (w, h) && c.hardware == hardware;
        if !decoder.as_ref().is_some_and(made_for) {
            let switched = decoder.as_ref().is_some_and(|c| c.hardware != hardware);
            // Let go of first, so that a hardware decoder's surfaces are back
            // before the next one asks for its own.
            *decoder = None;
            match self.decoders.decoder(codec, w, h) {
                Some(new) => {
                    self.info.lock().unwrap().encoding = encoding(codec, None);
                    *decoder = Some(Current {
                        codec,
                        size: (w, h),
                        hardware,
                        shown: None,
                        status_at: None,
                        decoder: new,
                    });
                    // A new decoder mid-stream has none of the pictures the
                    // next frames refer to. A new codec or size starts with a
                    // key frame of the peer's own accord; a new way of
                    // decoding is this end's doing, and the peer is asked.
                    if switched {
                        log::info!("decoding switched to {}: asking for a key frame", hardware.name());
                        self.refresh_video();
                    }
                }
                None => {
                    self.info.lock().unwrap().encoding = format!("{} (no decoder)", codec.name());
                    *self.decoder_status.lock().unwrap() = None;
                    return;
                }
            }
        }
        let Some(current) = decoder.as_mut() else {
            return;
        };
        back.begin(w, h);
        let gpu = self.decoders.gpu_pictures();
        let mut drew = false;
        let mut held: Option<Arc<dyn GpuPicture>> = None;
        for frame in frames {
            let decoded = if gpu {
                current.decoder.decode_picture(&frame.data, &mut back.pixels, w, h, &mut back.changed)
            } else if current.decoder.decode(&frame.data, &mut back.pixels, w, h, &mut back.changed) {
                Decoded::Converted
            } else {
                Decoded::Nothing
            };
            match decoded {
                Decoded::Gpu(picture) if (picture.width(), picture.height()) == (w, h) => {
                    held = Some(picture);
                    // A picture converted after it in the same update is
                    // converted whole, as one after it in the next is.
                    back.changed.all(w, h);
                }
                // Not the session's size, which is a frame between a resize
                // and the desktop's new size: read back, as a converted one
                // of that size would have been written.
                Decoded::Gpu(picture) => {
                    back.changed.all(w, h);
                    if picture.read(&mut back.pixels, w, h) {
                        drew = true;
                        held = None;
                    }
                }
                Decoded::Converted => {
                    drew = true;
                    held = None;
                }
                Decoded::Nothing => {}
            }
        }
        // A decoder that lost the pictures the stream refers back to — a
        // hardware one that failed mid-stream and went on in software — draws
        // nothing right until the next key frame, which is asked for rather
        // than waited for.
        if current.decoder.take_key_frame_request() {
            log::info!("the decoder asks for a key frame");
            self.refresh_video();
        }
        self.note(current);
        if let Some(picture) = held {
            // Whatever the back buffer holds is not the picture on screen now.
            back.lost();
            if !self.framebuffer.write().unwrap().show_gpu(picture) {
                return;
            }
            handler.damaged(0, 0, w, h);
            handler.frame_end();
            return;
        }
        if !drew {
            return;
        }
        // A frame that repeats the last one — which is what a peer sends to a
        // client whose desktop nobody is touching — costs nothing from here on,
        // and a frame that moved a caret costs the rows the caret is on.
        let Some((dx, dy, dw, dh)) = back.changed.bounds(w) else {
            return;
        };
        // Under a read lock and before the swap takes the picture away: the
        // rows this frame did not convert are still the frame before last.
        if !back.carry(&self.framebuffer.read().unwrap()) {
            return;
        }
        // The write lock is held for a `Vec` swap rather than for a conversion,
        // which is what keeps the drawing thread out of the decoder's way.
        if !self.framebuffer.write().unwrap().swap(&mut back.pixels) {
            back.lost();
            return;
        }
        back.swapped();
        handler.damaged(dx, dy, dw, dh);
        handler.frame_end();
    }

    /// What the decoder now says about itself, where anybody is shown it: the
    /// Encoding row whenever where its pictures come from changed — the
    /// first picture, a fallback — and the status line every
    /// [`STATUS_EVERY`].
    fn note(&self, current: &mut Current) {
        let hardware = current.decoder.hardware();
        if hardware != current.shown {
            current.shown = hardware;
            self.info.lock().unwrap().encoding = encoding(current.codec, hardware);
        }
        let now = Instant::now();
        if current.status_at.is_none_or(|at| now.duration_since(at) >= STATUS_EVERY) {
            current.status_at = Some(now);
            *self.decoder_status.lock().unwrap() = current.decoder.status();
        }
    }

    fn clipboard_arrived(&self, clip: Clipboard, handler: &mut dyn Handler) {
        if clip.format.enum_value_or_default() != ClipboardFormat::Text {
            return;
        }
        let content = if clip.compress {
            zstd::decode_all(&clip.content[..]).unwrap_or_default()
        } else {
            clip.content.to_vec()
        };
        if let Ok(text) = String::from_utf8(content) {
            if !text.is_empty() {
                // The length rather than the text: a clipboard is somebody's
                // password as often as it is a URL.
                log::info!("{} characters of clipboard from the peer", text.chars().count());
                handler.clipboard(&text);
            }
        }
    }

    fn send_login(&self, config: &Config, hash: &Hash, password: &str) {
        let mut login = LoginRequest::new();
        // **The address, not a name.** Their peer refuses a login whose
        // username is neither its own id nor something that parses as an
        // address, and the refusal it sends is `Offline` — which reads like the
        // machine is not there rather than like a field being wrong. Which is
        // why this is the socket's own address rather than what somebody typed:
        // the two differ wherever a default port was filled in.
        login.username = self.login_as.lock().unwrap().clone();
        login.password = login_hash(password, hash).into();
        // Their peer shows this and logs it, so it is the phone rather than
        // anything about this app: a person looking at the far end wants to
        // know which device is on their desktop.
        login.my_id = config.my_name.clone();
        login.my_name = config.my_name.clone();
        login.my_platform = config.platform.wire().into();
        login.version = VERSION.into();
        login.session_id = 1;
        // **The far end paces itself to us.** Without this it captures and
        // sends on its own clock, and a phone that takes longer over a frame
        // than the peer takes to make one falls further behind with every one
        // of them — which is a desktop that drags a second late and gets worse
        // the longer somebody drags it. With it, the peer captures the next
        // frame when this end says it has finished with the last.
        login.video_ack_required = true;
        let mut option = self.options(true);
        // The one option here that is not about the picture, and the only one a
        // session can set and never see the effect of: what it asks for happens
        // after the last message either end sends. Left unset rather than
        // answered No, so a peer configured to lock itself goes on doing so.
        if config.lock_after {
            option.lock_after_session_end = BoolOption::Yes.into();
        }
        login.option = MessageField::some(option);
        let mut out = Message::new();
        out.set_login_request(login);
        self.send(out);
    }

    /// The options a session carries, at login and again whenever one changes.
    ///
    /// The codec preference is in it at login and afterwards **only when it has
    /// moved**, which is what keeps a quality change from being a codec
    /// renegotiation: their peer answers a preference by building a new encoder
    /// and starting again with a key frame, so repeating the one it already has
    /// would make every change of frame rate cost a whole screenful.
    fn options(&self, at_login: bool) -> OptionMessage {
        let mut option = OptionMessage::new();
        option.image_quality = Quality::of(self.quality.load(Ordering::Relaxed)).wire().into();
        let fps = self.fps.load(Ordering::Relaxed);
        option.custom_fps = if self.focused.load(Ordering::Relaxed) {
            fps
        } else {
            UNFOCUSED_FPS
        };
        // The peer's sound where this session plays it, and otherwise not: a
        // desktop that is talking is a stream of Opus a session without
        // sound would pay for and drop. The clipboard has no switch beside it
        // because whether this phone shares one is a question about the
        // phone, answered once for every protocol in the app's own settings.
        option.disable_audio = if self.sound.load(Ordering::Relaxed) {
            BoolOption::No
        } else {
            BoolOption::Yes
        }
        .into();
        let codec = self.codec.load(Ordering::Relaxed);
        let moved = self.codec_sent.swap(codec, Ordering::Relaxed) != codec;
        if at_login || moved {
            option.supported_decoding = MessageField::some(self.decoding(PreferredCodec::of(codec)));
        }
        option
    }

    /// What this end can decode, and which of those it would rather have.
    ///
    /// The abilities are the **platform's**, asked of its [`Decoders`] rather
    /// than declared: their peer sends whatever a client says it can take, and
    /// a claim this end cannot honour is a session with a cursor and no
    /// picture. VP9 is the one every build of theirs can encode, so it is what
    /// a peer with nothing else falls back to and what an empty ability list
    /// would leave.
    fn decoding(&self, prefer: PreferredCodec) -> SupportedDecoding {
        let mut decoding = SupportedDecoding::new();
        decoding.ability_vp9 = self.decoders.decodable(Codec::Vp9) as i32;
        decoding.ability_vp8 = self.decoders.decodable(Codec::Vp8) as i32;
        decoding.ability_av1 = self.decoders.decodable(Codec::Av1) as i32;
        decoding.ability_h264 = self.decoders.decodable(Codec::H264) as i32;
        decoding.ability_h265 = self.decoders.decodable(Codec::H265) as i32;
        // A preference this end cannot decode is not passed on: the far end
        // would honour it exactly, and nothing over there could tell that the
        // picture never arrived.
        let prefer = match prefer.codec() {
            Some(codec) if !self.decoders.decodable(codec) => {
                log::warn!("this end cannot decode {}, leaving the codec to the peer", codec.name());
                PreferredCodec::Auto
            }
            _ => prefer,
        };
        log::info!("codec preference {:?}", prefer);
        decoding.prefer = prefer.wire().into();
        decoding
    }

    /// The session's sound switch as an Enable switch, on as the options
    /// ask: turned, it asks the peer with an option message carrying only
    /// `disable_audio` — every other option left unset, which their peer
    /// leaves as it is — and their peer subscribes the session to its sound
    /// or takes it off. Sent from the switch's thread through the writer.
    fn offer_sound(&self, audio: &Arc<dyn Outputs>) {
        let sound = Arc::clone(&self.sound);
        let out = self.out.lock().unwrap().clone();
        audio.controls().offer_enable(
            self.sound.load(Ordering::Relaxed),
            Box::new(move |on| {
                sound.store(on, Ordering::Relaxed);
                let mut option = OptionMessage::new();
                option.disable_audio = if on { BoolOption::No } else { BoolOption::Yes }.into();
                let mut misc = Misc::new();
                misc.set_option(option);
                let mut message = Message::new();
                message.set_misc(misc);
                if let (Some(out), Ok(bytes)) = (&out, message.write_to_bytes()) {
                    let _ = out.send(bytes);
                }
            }),
        );
    }

    /// A live option change, which is one message and takes effect on the
    /// picture in front of you.
    pub fn set_options(&self, quality: Quality, fps: i32, codec: PreferredCodec) {
        self.quality.store(quality.ordinal(), Ordering::Relaxed);
        self.fps.store(fps, Ordering::Relaxed);
        self.codec.store(codec.ordinal(), Ordering::Relaxed);
        self.send_options();
    }

    fn send_options(&self) {
        let mut misc = Misc::new();
        misc.set_option(self.options(false));
        let mut out = Message::new();
        out.set_misc(misc);
        self.send(out);
    }

    // ---- input ------------------------------------------------------------

    pub fn set_view_only(&self, view_only: bool) {
        self.view_only.store(view_only, Ordering::Relaxed);
        if view_only {
            self.release_all_keys();
        }
    }

    /// Whether the session is on screen. There is no pause message here, so
    /// what this does is ask for one frame a second and ask for a fresh picture
    /// on the way back.
    pub fn set_focused(&self, focused: bool) {
        if self.focused.swap(focused, Ordering::Relaxed) == focused {
            return;
        }
        self.send_options();
        if focused {
            self.refresh_video();
        }
    }

    /// Absolute desktop coordinates and an RFB button mask.
    ///
    /// The seam says where the pointer is and which buttons are held; this
    /// protocol wants a move, a press and a release as separate events, so the
    /// edges are worked out here against the last mask seen. The coordinates
    /// are the picture's and what goes out is the peer's desktop's, which is
    /// the same thing only while the display being captured is at the origin.
    pub fn pointer(&self, x: i32, y: i32, mask: i32) {
        if self.view_only.load(Ordering::Relaxed) {
            return;
        }
        let (ox, oy) = *self.origin.lock().unwrap();
        let (moved, was) = {
            let mut last = self.pointer.lock().unwrap();
            let moved = last.0 != x || last.1 != y;
            let was = last.2;
            *last = (x, y, mask);
            (moved, was)
        };
        if moved {
            self.mouse(mouse::TYPE_MOVE, x + ox, y + oy, false);
        }
        for (ours, theirs) in BUTTONS {
            let down = mask & ours != 0;
            if down != (was & ours != 0) {
                let kind = if down { mouse::TYPE_DOWN } else { mouse::TYPE_UP };
                self.mouse(kind | (theirs << 3), x + ox, y + oy, down);
            }
        }
        // The wheel is four bits of the same mask, and each is a notch rather
        // than something held: the edge is the event and the release is not.
        // Its two numbers are a distance rather than a place, so the display's
        // origin is not added to them.
        let pressed = mask & !was;
        let (mut dx, mut dy) = (0, 0);
        if pressed & 0x08 != 0 {
            dy += WHEEL_NOTCH;
        }
        if pressed & 0x10 != 0 {
            dy -= WHEEL_NOTCH;
        }
        if pressed & 0x20 != 0 {
            dx += WHEEL_NOTCH;
        }
        if pressed & 0x40 != 0 {
            dx -= WHEEL_NOTCH;
        }
        if dx != 0 || dy != 0 {
            self.mouse(mouse::TYPE_WHEEL, dx, dy, false);
        }
    }

    /// One mouse event. A press carries the modifiers held, because their
    /// server releases every modifier a down event does not name.
    fn mouse(&self, mask: i32, x: i32, y: i32, with_modifiers: bool) {
        let mut event = MouseEvent::new();
        event.mask = mask;
        event.x = x;
        event.y = y;
        if with_modifiers {
            event.modifiers = self.held_modifiers(None);
        }
        let mut out = Message::new();
        out.set_mouse_event(event);
        self.send(out);
    }

    pub fn key_down(&self, keysym: u32, key_id: i64) {
        if self.view_only.load(Ordering::Relaxed) {
            return;
        }
        let Some(key) = keymap::key(keysym) else {
            log::info!("nothing in this protocol says keysym {keysym:#x}");
            return;
        };
        if let Some(modifier) = keymap::modifier(keysym) {
            let mut held = self.modifiers.lock().unwrap();
            if !held.contains(&modifier) {
                held.push(modifier);
            }
        }
        self.keys.lock().unwrap().insert(key_id, key);
        self.key(key, true);
    }

    pub fn key_up(&self, key_id: i64) {
        let Some(key) = self.keys.lock().unwrap().remove(&key_id) else {
            return;
        };
        if let Key::Control(control) = key {
            self.modifiers.lock().unwrap().retain(|held| *held != control);
        }
        self.key(key, false);
    }

    /// Let go of every key this session still has down at the far end.
    pub fn release_all_keys(&self) {
        let held: Vec<Key> = {
            let mut keys = self.keys.lock().unwrap();
            let held = keys.values().copied().collect();
            keys.clear();
            held
        };
        self.modifiers.lock().unwrap().clear();
        for key in held {
            self.key(key, false);
        }
    }

    fn key(&self, key: Key, down: bool) {
        let mut event = KeyEvent::new();
        event.down = down;
        event.mode = KeyboardMode::Legacy.into();
        match key {
            Key::Control(control) => {
                event.set_control_key(control);
                if down {
                    event.modifiers = self.held_modifiers(Some(control));
                }
            }
            Key::Char(c) => {
                // `chr` rather than `unicode`: their server can hold one down,
                // which is what makes it a key rather than a piece of typing.
                event.set_chr(c as u32);
                if down {
                    event.modifiers = self.held_modifiers(None);
                }
            }
        }
        let mut out = Message::new();
        out.set_key_event(event);
        self.send(out);
    }

    /// The modifiers held, less the one being pressed: their server compares
    /// the list against what is down on the far end and releases the
    /// difference, and a modifier that names itself is one it would refuse to
    /// press.
    fn held_modifiers(&self, itself: Option<ControlKey>) -> Vec<protobuf::EnumOrUnknown<ControlKey>> {
        let its = itself.and_then(|c| keymap::modifier_of(c));
        self.modifiers
            .lock()
            .unwrap()
            .iter()
            .filter(|held| Some(**held) != its)
            .map(|held| protobuf::EnumOrUnknown::new(*held))
            .collect()
    }

    pub fn clipboard(&self, text: &str) {
        if self.view_only.load(Ordering::Relaxed) {
            return;
        }
        log::info!("{} characters of clipboard to the peer", text.chars().count());
        let mut clip = Clipboard::new();
        clip.content = text.as_bytes().to_vec().into();
        clip.format = ClipboardFormat::Text.into();
        let mut out = Message::new();
        out.set_clipboard(clip);
        self.send(out);
    }

    /// Ask the far end for a desktop this size, out of the sizes it offered.
    ///
    /// Unlike RFB there is no free choice: the peer publishes a list and takes
    /// one of them, so the nearest by area is what a window's size becomes.
    pub fn request_desktop_size(&self, width: i32, height: i32) {
        let offered = self.resolutions.lock().unwrap().clone();
        let wanted = (width as i64) * (height as i64);
        let Some(&(w, h)) = offered.iter().min_by_key(|(ow, oh)| {
            ((*ow as i64) * (*oh as i64) - wanted).abs()
        }) else {
            return;
        };
        log::info!("asked for {width}x{height}, which is {w}x{h} of the sizes offered");
        let mut resolution = Resolution::new();
        resolution.width = w;
        resolution.height = h;
        let mut change = DisplayResolution::new();
        change.display = self.display.load(Ordering::Relaxed);
        change.resolution = MessageField::some(resolution);
        let mut misc = Misc::new();
        misc.set_change_display_resolution(change);
        let mut out = Message::new();
        out.set_misc(misc);
        self.send(out);
    }

    /// Say something harmless every few seconds until told to stop.
    ///
    /// A peer that has heard nothing for **30 seconds** before the login drops
    /// the connection, and a question put to a person can outlast that — the
    /// session was lost at forty, with a dialog still on screen. Their timing
    /// message is the one thing a client may send before it has logged in that
    /// the far end is content to receive, so this is what a keepalive is made of
    /// here.
    fn keep_alive(&self) -> KeepAlive {
        let mut delay = TestDelay::new();
        delay.from_client = true;
        let mut message = Message::new();
        message.set_test_delay(delay);
        let bytes = message.write_to_bytes().unwrap_or_default();
        let out = self.out.lock().unwrap().clone();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("rustdesk-keepalive".into())
            .spawn(move || {
                let Some(out) = out else { return };
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(KEEP_ALIVE_EVERY);
                    if flag.load(Ordering::Relaxed) || out.send(bytes.clone()).is_err() {
                        break;
                    }
                }
            })
            .ok();
        KeepAlive { stop, thread }
    }

    /// Hand a message to the writer thread. Dropped where the session has
    /// ended, which is every input event between a socket going and the screen
    /// being told.
    fn send(&self, message: Message) {
        let bytes = match message.write_to_bytes() {
            Ok(bytes) => bytes,
            Err(e) => {
                log::warn!("encoding a message: {e}");
                return;
            }
        };
        if let Some(out) = self.out.lock().unwrap().as_ref() {
            let _ = out.send(bytes);
        }
    }
}

/// The shape `runtime::Session` runs: the inherent `run` and `close` are the
/// implementation, and this is them written down for a session that does not
/// know which protocol it has.
impl runtime::Client for Client {
    type Config = Config;
    type Error = Error;
    const NAME: &'static str = "rustdesk";

    fn run(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        self.run(config, handler)
    }

    fn close(&self) {
        self.close();
    }

    fn closed_on_request(error: &Error) -> bool {
        matches!(error, Error::Closed)
    }
}

/// The thread behind [`Client::keep_alive`], stopped by hand rather than by
/// being dropped: it is stopped at a point where the answer has arrived and the
/// next thing to happen is the login, and a `Drop` that joins a sleeping thread
/// would hold that up for as long as the sleep has left.
struct KeepAlive {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl KeepAlive {
    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        drop(self.thread);
    }
}

/// What a session is costing, logged once every [`Rate::EVERY`] frames.
///
/// This protocol has no damage rectangles, so a frame arrives whole whatever
/// moved in it, and what a session costs is how much of one is worth
/// converting. That and where the time goes is the one thing worth having out
/// of a running session, and it is one line a minute rather than anything a
/// frame pays for.
struct Rate {
    frames: u64,
    since: Instant,
    working: Duration,
    rows: u64,
}

impl Rate {
    const EVERY: u64 = 300;

    fn new() -> Rate {
        Rate {
            frames: 0,
            since: Instant::now(),
            working: Duration::ZERO,
            rows: 0,
        }
    }

    fn frame(&mut self, took: Duration, rows: usize) {
        self.frames += 1;
        self.working += took;
        self.rows += rows as u64;
        if self.frames < Rate::EVERY {
            return;
        }
        let elapsed = self.since.elapsed().as_secs_f64().max(0.001);
        log::info!(
            "{} frames in {:.1}s — {:.1} a second, {:.1} ms of decode and conversion each, {:.0} rows converted each, {:.0}% of this thread",
            self.frames,
            elapsed,
            self.frames as f64 / elapsed,
            self.working.as_secs_f64() * 1000.0 / self.frames as f64,
            self.rows as f64 / self.frames as f64,
            self.working.as_secs_f64() * 100.0 / elapsed,
        );
        self.frames = 0;
        self.working = Duration::ZERO;
        self.rows = 0;
        self.since = Instant::now();
    }
}

/// A cursor shape, kept because their protocol sends the pixels once and refers
/// to them by id from then on.
struct Cursor {
    pixels: Vec<u32>,
    width: usize,
    height: usize,
    hot_x: i32,
    hot_y: i32,
}

impl Cursor {
    fn of(data: crate::protos::message::CursorData) -> Option<Cursor> {
        let (width, height) = (data.width.max(0) as usize, data.height.max(0) as usize);
        if width == 0 || height == 0 {
            return None;
        }
        // Their cursors are zstd, as their clipboard is.
        let rgba = zstd::decode_all(&data.colors[..]).unwrap_or_default();
        if rgba.len() < width * height * 4 {
            log::warn!("a {width}x{height} cursor arrived as {} bytes", rgba.len());
            return None;
        }
        let pixels = rgba
            .chunks_exact(4)
            .take(width * height)
            .map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]]))
            .collect();
        Some(Cursor {
            pixels,
            width,
            height,
            hot_x: data.hotx,
            hot_y: data.hoty,
        })
    }

    fn deliver(&self, handler: &mut dyn Handler) {
        handler.cursor(&self.pixels, self.width, self.height, self.hot_x, self.hot_y);
    }
}

/// `sha256(sha256(password ++ salt) ++ challenge)`. The inner hash is what a
/// client stores, so a saved password is already salted per peer — which is
/// worth knowing and is not what this app does, since it has one password store
/// for six protocols.
fn login_hash(password: &str, hash: &Hash) -> Vec<u8> {
    if password.is_empty() {
        return Vec::new();
    }
    let stored = Sha256::new()
        .chain_update(password.as_bytes())
        .chain_update(hash.salt.as_bytes())
        .finalize();
    Sha256::new()
        .chain_update(stored)
        .chain_update(hash.challenge.as_bytes())
        .finalize()
        .to_vec()
}

/// `host` or `host:port`, plus the bracketed IPv6 form. The direct mode's
/// address only; an id is not one of these and never reaches here.
fn resolve(address: &str) -> Result<SocketAddr> {
    let (host, port) = runtime::address::split(address, DIRECT_PORT, runtime::address::Ports::Plain)
        .map_err(Error::Address)?;
    (host.as_str(), port)
        .to_socket_addrs()
        .map_err(Error::Io)?
        .next()
        .ok_or_else(|| Error::Protocol(format!("{address} resolves to nothing")))
}

fn timed_out(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn arm(union: &message::Union) -> &'static str {
    match union {
        message::Union::SignedId(_) => "SignedId",
        message::Union::PublicKey(_) => "PublicKey",
        message::Union::TestDelay(_) => "TestDelay",
        message::Union::VideoFrame(_) => "VideoFrame",
        message::Union::LoginResponse(_) => "LoginResponse",
        message::Union::Hash(_) => "Hash",
        message::Union::CursorData(_) => "CursorData",
        message::Union::CursorId(_) => "CursorId",
        message::Union::CursorPosition(_) => "CursorPosition",
        message::Union::Clipboard(_) => "Clipboard",
        message::Union::Misc(_) => "Misc",
        message::Union::MessageBox(_) => "MessageBox",
        _ => "something else",
    }
}

/// Withdraws the session's sound switch when the session ends, whichever
/// way it ends.
struct Withdraw(Option<Arc<dyn Outputs>>);

impl Drop for Withdraw {
    fn drop(&mut self) {
        if let Some(audio) = &self.0 {
            audio.controls().withdraw();
        }
    }
}

/// The session's sound output, where the config has one and its platform
/// decodes Opus: the only codec their peer encodes its sound in, so an
/// output without the decoder is no sound at all.
fn sound_of(config: &Config) -> Option<Arc<dyn Outputs>> {
    config
        .audio
        .clone()
        .filter(|audio| audio.decoders().is_some_and(|d| d.decodable(SoundCodec::Opus)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Their formula, against the one value a run has confirmed end to end: the
    /// rig's password, salt and challenge, and the digest their server
    /// accepted.
    #[test]
    fn the_password_is_two_rounds() {
        let mut hash = Hash::new();
        hash.salt = "0123456789abcdef0123456789abcdef".into();
        hash.challenge = "abcdef".into();
        let digest = login_hash("PASSword1", &hash);
        assert_eq!(digest.len(), 32);
        // The inner hash is what a client would store, and the outer one is
        // what goes on the wire — so the same password against a second
        // challenge is a different answer.
        let mut second = hash.clone();
        second.challenge = "fedcba".into();
        assert_ne!(digest, login_hash("PASSword1", &second));
        assert!(login_hash("", &hash).is_empty());
    }

    /// The rig's second screen: 1024×768 at 1920,665 on a desktop whose first
    /// display is 1920×1440. A tap in the middle of the picture is a tap on
    /// that screen, and every coordinate on the wire is the desktop's — so a
    /// client that sends the picture's own numbers moves the cursor on the
    /// first display instead, which is what this pins.
    /// A handler that takes everything and does nothing with it.
    struct Nothing;
    impl Handler for Nothing {
        fn connected(&mut self, _width: usize, _height: usize) {}
        fn desktop_size(&mut self, _width: usize, _height: usize) {}
        fn damaged(&mut self, _x: usize, _y: usize, _width: usize, _height: usize) {}
        fn frame_end(&mut self) {}
        fn cursor(&mut self, _p: &[u32], _w: usize, _h: usize, _hx: i32, _hy: i32) {}
        fn clipboard(&mut self, _text: &str) {}
        fn credentials(&mut self, _needs_user: bool) -> Option<runtime::Credentials> {
            None
        }
        fn trust(&mut self, _fingerprint: &str) -> bool {
            false
        }
    }

    #[test]
    fn a_pointer_is_where_the_captured_display_is() {
        let client = Client::new(Arc::new(video::NoDecoders));
        let (out, sent) = channel();
        *client.out.lock().unwrap() = Some(out);
        let events = || -> Vec<MouseEvent> {
            sent.try_iter()
                .filter_map(|bytes| match Message::parse_from_bytes(&bytes).unwrap().union {
                    Some(message::Union::MouseEvent(event)) => Some(event),
                    _ => None,
                })
                .collect()
        };

        // The origin comes off the switch message rather than the list the
        // login carried, since that is the one a peer sends when a screen it
        // was not asked about moves.
        let mut switch = SwitchDisplay::new();
        switch.display = 1;
        switch.x = 1920;
        switch.y = 665;
        switch.width = 1024;
        switch.height = 768;
        client.switched(&switch, &mut Nothing, &mut Back::new());

        client.pointer(512, 384, 0x01);
        let moved_and_pressed = events();
        assert_eq!(moved_and_pressed.len(), 2);
        assert_eq!(moved_and_pressed[0].mask, mouse::TYPE_MOVE);
        assert_eq!((moved_and_pressed[0].x, moved_and_pressed[0].y), (2432, 1049));
        assert_eq!(
            moved_and_pressed[1].mask,
            mouse::TYPE_DOWN | (mouse::LEFT << 3)
        );
        assert_eq!((moved_and_pressed[1].x, moved_and_pressed[1].y), (2432, 1049));

        // The wheel's two numbers are how far rather than where.
        client.pointer(512, 384, 0x08);
        let released_and_scrolled = events();
        assert_eq!(released_and_scrolled.len(), 2);
        assert_eq!(
            (released_and_scrolled[0].x, released_and_scrolled[0].y),
            (2432, 1049)
        );
        assert_eq!(released_and_scrolled[1].mask, mouse::TYPE_WHEEL);
        assert_eq!(
            (released_and_scrolled[1].x, released_and_scrolled[1].y),
            (0, WHEEL_NOTCH)
        );
    }

    /// An output that decodes Opus where told to, and plays nothing.
    struct Sounding(Arc<media::audio::Controls>, bool);

    impl Outputs for Sounding {
        fn controls(&self) -> &Arc<media::audio::Controls> {
            &self.0
        }

        fn device(&self, _queue: Arc<media::audio::Queue>) -> std::result::Result<Box<dyn media::audio::Device>, String> {
            Err("not in a test".into())
        }

        fn decoders(&self) -> Option<&dyn media::audio::decode::Decoders> {
            self.1.then_some(self as _)
        }
    }

    impl media::audio::decode::Decoders for Sounding {
        fn decodable(&self, codec: SoundCodec) -> bool {
            codec == SoundCodec::Opus
        }

        fn decoder(
            &self,
            _codec: SoundCodec,
            _format: Format,
            _config: &[u8],
        ) -> std::result::Result<Box<dyn media::audio::decode::Decoder>, String> {
            Err("not in a test".into())
        }
    }

    #[test]
    fn the_peers_sound_is_asked_for_only_where_its_opus_is_decoded() {
        let config = |audio: Option<Arc<dyn Outputs>>| Config {
            address: String::new(),
            reach: Reach::Direct,
            password: None,
            connect_timeout: Duration::from_secs(1),
            my_name: String::new(),
            platform: Platform::Linux,
            quality: Quality::Balanced,
            fps: 0,
            codec: PreferredCodec::Auto,
            lock_after: false,
            audio,
            sound: true,
        };
        let controls = Arc::new(media::audio::Controls::new());
        assert!(sound_of(&config(None)).is_none());
        assert!(sound_of(&config(Some(Arc::new(Sounding(Arc::clone(&controls), false))))).is_none());
        assert!(sound_of(&config(Some(Arc::new(Sounding(controls, true))))).is_some());

        let client = Client::new(Arc::new(video::NoDecoders));
        assert_eq!(client.options(true).disable_audio.enum_value(), Ok(BoolOption::Yes));
        client.sound.store(true, Ordering::Relaxed);
        assert_eq!(client.options(false).disable_audio.enum_value(), Ok(BoolOption::No));
    }

    #[test]
    fn the_switch_asks_the_peer_with_the_audio_option_alone() {
        let controls = Arc::new(media::audio::Controls::new());
        let audio: Arc<dyn Outputs> = Arc::new(Sounding(Arc::clone(&controls), true));
        let client = Client::new(Arc::new(video::NoDecoders));
        let (out, sent) = channel::<Vec<u8>>();
        *client.out.lock().unwrap() = Some(out);
        client.sound.store(true, Ordering::Relaxed);
        client.offer_sound(&audio);
        let sound = controls.sound();
        assert_eq!((sound.switch, sound.enabled), (Some(media::audio::Switch::Enable), true));

        let asked = |sent: &std::sync::mpsc::Receiver<Vec<u8>>| {
            let message = Message::parse_from_bytes(&sent.try_recv().unwrap()).unwrap();
            let Some(message::Union::Misc(misc)) = message.union else { panic!("not a misc") };
            let Some(misc::Union::Option(option)) = misc.union else { panic!("not an option") };
            // Nothing else in it, so the peer changes nothing else.
            assert_eq!(option.image_quality.enum_value(), Ok(ImageQuality::NotSet));
            assert_eq!(option.custom_fps, 0);
            assert!(option.supported_decoding.is_none());
            option.disable_audio.enum_value().unwrap()
        };
        assert!(controls.set_enabled(false));
        assert_eq!(asked(&sent), BoolOption::Yes);
        assert!(!client.sound.load(Ordering::Relaxed));
        // The next options, a quality change's, say the same.
        assert_eq!(client.options(false).disable_audio.enum_value(), Ok(BoolOption::Yes));
        assert!(controls.set_enabled(true));
        assert_eq!(asked(&sent), BoolOption::No);
        assert!(sent.try_recv().is_err());
    }

    #[test]
    fn a_port_is_theirs_unless_one_is_given() {
        assert_eq!(resolve("127.0.0.1").unwrap().port(), DIRECT_PORT);
        assert_eq!(resolve("127.0.0.1:5900").unwrap().port(), 5900);
        // The bracketed form with no port is the case a `contains(':')` test
        // for "has a port already" gets wrong, since every literal has one.
        assert_eq!(resolve("[::1]").unwrap().port(), DIRECT_PORT);
        assert!(resolve("::1").is_err());
    }

    #[test]
    fn the_encoding_row_says_where_the_pictures_come_from_once_known() {
        assert_eq!(encoding(Codec::Av1, Some(false)), "AV1 (software)");
        assert_eq!(encoding(Codec::H264, Some(true)), "H.264 (hardware)");
        assert_eq!(encoding(Codec::Vp9, None), "VP9");
    }

    /// Decoders whose way of decoding can be switched, as a desktop's can,
    /// and which count what they make; each decoder draws a word per frame,
    /// says it is hardware when made so, and asks for a key frame when told
    /// to.
    #[derive(Default)]
    struct Switchable {
        hardware: Mutex<Hardware>,
        made: AtomicI32,
        lose_references: Arc<AtomicBool>,
        /// The window takes pictures on the GPU, which each decoder then
        /// hands back as a flat one.
        gpu: AtomicBool,
    }

    struct Drawing {
        hardware: bool,
        lose_references: Arc<AtomicBool>,
    }

    impl video::Decoder for Drawing {
        fn decode(
            &mut self,
            _data: &[u8],
            out: &mut [u32],
            _width: usize,
            _height: usize,
            changed: &mut video::Changes,
        ) -> bool {
            out[0] = out[0].wrapping_add(1);
            changed.mark(0, 0, 1);
            true
        }

        fn hardware(&self) -> Option<bool> {
            Some(self.hardware)
        }

        fn status(&self) -> Option<String> {
            Some(if self.hardware { "H.264 vaapi" } else { "H.264 sw" }.into())
        }

        fn take_key_frame_request(&mut self) -> bool {
            self.lose_references.swap(false, Ordering::Relaxed)
        }

        fn decode_picture(
            &mut self,
            _data: &[u8],
            _out: &mut [u32],
            width: usize,
            height: usize,
            _changed: &mut video::Changes,
        ) -> Decoded {
            Decoded::Gpu(crate::framebuffer::tests::flat(width, height, 9))
        }
    }

    impl Decoders for Switchable {
        fn decodable(&self, _codec: Codec) -> bool {
            true
        }

        fn decoder(&self, _codec: Codec, _width: usize, _height: usize) -> Option<Box<dyn video::Decoder>> {
            self.made.fetch_add(1, Ordering::Relaxed);
            Some(Box::new(Drawing {
                hardware: *self.hardware.lock().unwrap() != Hardware::Off,
                lose_references: Arc::clone(&self.lose_references),
            }))
        }

        fn hardware(&self) -> Hardware {
            *self.hardware.lock().unwrap()
        }

        fn gpu_pictures(&self) -> bool {
            self.gpu.load(Ordering::Relaxed)
        }
    }

    /// Hardware decoding switched on under a running session is a new
    /// decoder at the next picture and a key frame asked for, with the
    /// Encoding row and the status line following it; a decoder that loses
    /// its references asks for one too; and nothing else asks.
    #[test]
    fn a_switch_of_decoding_is_a_new_decoder_and_a_key_frame() {
        let decoders = Arc::new(Switchable::default());
        let client = Client::new(Arc::clone(&decoders) as Arc<dyn Decoders>);
        let (out, sent) = channel();
        *client.out.lock().unwrap() = Some(out);
        let refreshes = || {
            sent.try_iter()
                .filter(|bytes| match Message::parse_from_bytes(bytes).unwrap().union {
                    Some(message::Union::Misc(misc)) => misc.refresh_video(),
                    _ => false,
                })
                .count()
        };
        let mut back = Back::new();
        client.width.store(4, Ordering::Relaxed);
        client.height.store(2, Ordering::Relaxed);
        client.resize(4, 2, &mut back);
        let frame = || {
            let mut one = crate::protos::message::EncodedVideoFrame::new();
            one.data = vec![0; 4];
            let mut frames = crate::protos::message::EncodedVideoFrames::new();
            frames.frames.push(one);
            let mut frame = VideoFrame::new();
            frame.set_h264s(frames);
            frame
        };
        let mut decoder = None;

        client.picture(frame(), &mut decoder, &mut back, &mut Nothing);
        assert_eq!(decoders.made.load(Ordering::Relaxed), 1);
        assert_eq!(client.info().encoding, "H.264 (software)");
        assert_eq!(client.decoder_status().as_deref(), Some("H.264 sw"));
        assert_eq!(refreshes(), 0, "a first decoder waits for the peer's own key frame");

        client.picture(frame(), &mut decoder, &mut back, &mut Nothing);
        assert_eq!(decoders.made.load(Ordering::Relaxed), 1);

        *decoders.hardware.lock().unwrap() = Hardware::Auto;
        client.picture(frame(), &mut decoder, &mut back, &mut Nothing);
        assert_eq!(decoders.made.load(Ordering::Relaxed), 2);
        assert_eq!(refreshes(), 1);
        assert_eq!(client.info().encoding, "H.264 (hardware)");
        assert_eq!(client.decoder_status().as_deref(), Some("H.264 vaapi"));

        decoders.lose_references.store(true, Ordering::Relaxed);
        client.picture(frame(), &mut decoder, &mut back, &mut Nothing);
        assert_eq!(decoders.made.load(Ordering::Relaxed), 2);
        assert_eq!(refreshes(), 1);
        client.picture(frame(), &mut decoder, &mut back, &mut Nothing);
        assert_eq!(refreshes(), 0);
    }

    /// A handler that keeps the damage it is told of.
    #[derive(Default)]
    struct Damage(Vec<(usize, usize, usize, usize)>);
    impl Handler for Damage {
        fn connected(&mut self, _width: usize, _height: usize) {}
        fn desktop_size(&mut self, _width: usize, _height: usize) {}
        fn damaged(&mut self, x: usize, y: usize, width: usize, height: usize) {
            self.0.push((x, y, width, height));
        }
        fn frame_end(&mut self) {}
        fn cursor(&mut self, _p: &[u32], _w: usize, _h: usize, _hx: i32, _hy: i32) {}
        fn clipboard(&mut self, _text: &str) {}
        fn credentials(&mut self, _needs_user: bool) -> Option<runtime::Credentials> {
            None
        }
        fn trust(&mut self, _fingerprint: &str) -> bool {
            false
        }
    }

    /// With the window taking pictures on the GPU, the decoder's picture is
    /// the framebuffer's as it is, the whole desktop damaged and nothing
    /// converted until somebody reads it; and once it takes them no more,
    /// the next picture is converted whole, with no new decoder or key
    /// frame for the change.
    #[test]
    fn a_picture_on_the_gpu_is_the_picture_until_one_is_converted() {
        let decoders = Arc::new(Switchable::default());
        let client = Client::new(Arc::clone(&decoders) as Arc<dyn Decoders>);
        let mut back = Back::new();
        client.width.store(4, Ordering::Relaxed);
        client.height.store(2, Ordering::Relaxed);
        client.resize(4, 2, &mut back);
        let frame = || {
            let mut one = crate::protos::message::EncodedVideoFrame::new();
            one.data = vec![0; 4];
            let mut frames = crate::protos::message::EncodedVideoFrames::new();
            frames.frames.push(one);
            let mut frame = VideoFrame::new();
            frame.set_h264s(frames);
            frame
        };
        let words = |client: &Client| {
            let mut out = [0u8; 32];
            assert!(client.framebuffer().read().unwrap().read_rgba(0, 0, 4, 2, &mut out, 16));
            out.chunks_exact(4)
                .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
                .collect::<Vec<_>>()
        };
        let mut decoder = None;
        let mut damage = Damage::default();

        client.picture(frame(), &mut decoder, &mut back, &mut damage);
        assert!(client.gpu_picture().is_none());
        assert_eq!(words(&client)[0], 0xff00_0001);

        decoders.gpu.store(true, Ordering::Relaxed);
        damage.0.clear();
        client.picture(frame(), &mut decoder, &mut back, &mut damage);
        assert_eq!(decoders.made.load(Ordering::Relaxed), 1);
        assert!(client.gpu_picture().is_some());
        assert_eq!(damage.0, [(0, 0, 4, 2)]);
        assert!(client.framebuffer().read().unwrap().behind());
        client.framebuffer().write().unwrap().catch_up();
        assert_eq!(words(&client), [9; 8]);

        decoders.gpu.store(false, Ordering::Relaxed);
        damage.0.clear();
        client.picture(frame(), &mut decoder, &mut back, &mut damage);
        assert_eq!(decoders.made.load(Ordering::Relaxed), 1);
        assert!(client.gpu_picture().is_none());
        assert_eq!(damage.0, [(0, 0, 4, 2)], "converted whole");
    }
}
