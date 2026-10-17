// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What each backend can be told, described well enough to offer without
//! knowing what it means: `BackendOption` in Java, and the eight lists
//! themselves, which are the one copy. The phone's are generated from these
//! (`vento-codegen`, as a generated source of each backend's module), in the
//! phone's words; where a desktop needs other words — a phone vibrates for a
//! bell, a window cannot — a row says so beside them
//! ([`Spec::desktop_summary`], [`Choice::desktop_label`]).
//!
//! The values are strings, because that is what the backends' own
//! configuration layers take; [`Kind`] says how to ask for one, not how to
//! store it. A `-o key=value` on the command line and a row in a dialog are
//! the same thing here — a key checked against a backend's list, a value
//! checked against the kind — which is what keeps both of them generic.
//!
//! The lists are `static` and the descriptions are `&'static str`, so a
//! dialog can hold a reference to a row without owning anything; the
//! per-backend helpers that turn a value into what a library wants (a
//! compression level, an encodings string) are in that backend's adapter,
//! beside the call that takes it.

use crate::kind::Platform;
use std::collections::{BTreeMap, BTreeSet};

/// Which control to offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `"true"` / `"false"`.
    Bool,
    /// One of the choices.
    Choice,
    /// A decimal integer.
    Int,
    /// Free text.
    Text,
}

/// Whose setting it is: the connection's, the backend's, or the connection's
/// with the backend's answer as its default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Connection,
    Global,
    Layered,
}

/// A value and what to call it on screen. `summary` is why you would pick
/// this one over the others, where that needs saying; `requires` is the key
/// of a `Bool` option this choice depends on, so a choice whose switch is off
/// is not offered at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub value: &'static str,
    /// What to call it, as the phone words it.
    pub label: &'static str,
    /// What a desktop calls it where the phone's word is the phone's — "this
    /// phone" — or `None`. See [`Choice::label_on`].
    pub desktop_label: Option<&'static str>,
    pub summary: Option<&'static str>,
    pub requires: Option<&'static str>,
}

impl Choice {
    /// What to call it in the client it is shown by.
    pub fn label_on(&self, platform: Platform) -> &'static str {
        match platform {
            Platform::Android => self.label,
            Platform::Linux => self.desktop_label.unwrap_or(self.label),
        }
    }
}

/// What another option must say for this one to be offered at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gate {
    pub key: &'static str,
    pub values: &'static [&'static str],
}

impl Gate {
    pub fn open(&self, value: &str) -> bool {
        self.values.contains(&value)
    }
}

/// One setting a backend understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The backend's own name for it; the map key.
    pub key: &'static str,
    /// One line, for a preference title.
    pub label: &'static str,
    /// The longer explanation, as the phone words it.
    pub summary: Option<&'static str>,
    /// The same explanation where a desktop needs other words — a phone
    /// vibrates for a bell and a window cannot — or `None` where the phone's
    /// words do for both. See [`Spec::summary_on`].
    pub desktop_summary: Option<&'static str>,
    pub kind: Kind,
    /// For `Choice`, in display order; empty otherwise.
    pub choices: &'static [Choice],
    /// What the backend does when nothing is set.
    pub default: &'static str,
    pub scope: Scope,
    /// Whether `set_option` does anything mid-session.
    pub live: bool,
    pub gate: Option<Gate>,
}

impl Spec {
    /// The explanation in the words of the client it is shown by.
    pub fn summary_on(&self, platform: Platform) -> Option<&'static str> {
        match platform {
            Platform::Android => self.summary,
            Platform::Linux => self.desktop_summary.or(self.summary),
        }
    }

    /// For a `Choice`: what to show for `value` (a `&'static str` from the
    /// list) in the client it is shown by, or the value itself when it is not
    /// one of the choices.
    pub fn label_for<'a>(&self, value: &'a str, platform: Platform) -> &'a str {
        self.choices
            .iter()
            .find(|c| c.value == value)
            .map(|c| c.label_on(platform))
            .unwrap_or(value)
    }

    /// Whether `value` is something this option takes, in words when not.
    pub fn check(&self, value: &str) -> Result<(), String> {
        match self.kind {
            Kind::Bool => match value {
                "true" | "false" => Ok(()),
                _ => Err(format!("{}: true or false, not {value:?}", self.key)),
            },
            Kind::Int => value
                .parse::<i64>()
                .map(|_| ())
                .map_err(|_| format!("{}: a whole number, not {value:?}", self.key)),
            Kind::Choice => {
                if self.choices.iter().any(|c| c.value == value) {
                    Ok(())
                } else {
                    let values: Vec<&str> = self.choices.iter().map(|c| c.value).collect();
                    Err(format!(
                        "{}: one of {}, not {value:?}",
                        self.key,
                        values.join(", ")
                    ))
                }
            }
            Kind::Text => Ok(()),
        }
    }

    /// For a `Choice`: the choice `value` names, where it requires a switch
    /// that `on` says is off — a value left standing after its switch went,
    /// or given without it. Such a value is not offered, and is not what
    /// the backend would do either (TigerVNC's `Encoding=h264` without
    /// `H264` negotiates something else), so it must not stay selected.
    pub fn stranded(&self, value: &str, on: impl Fn(&str) -> bool) -> Option<&'static Choice> {
        let choices: &'static [Choice] = self.choices;
        choices
            .iter()
            .find(|c| c.value == value)
            .filter(|c| c.requires.is_some_and(|key| !on(key)))
    }

    /// What a stranded choice falls back to: the option's default, as if it
    /// had never been picked, or the first choice on offer where the default
    /// is stranded too (no list here has one that is).
    pub fn fallback(&self, on: impl Fn(&str) -> bool) -> &'static str {
        if self.stranded(self.default, &on).is_none() {
            return self.default;
        }
        let choices: &'static [Choice] = self.choices;
        choices
            .iter()
            .find(|c| c.requires.is_none_or(&on))
            .map_or(self.default, |c| c.value)
    }
}

const fn choice(value: &'static str, label: &'static str) -> Choice {
    Choice {
        value,
        label,
        desktop_label: None,
        summary: None,
        requires: None,
    }
}

const fn described(value: &'static str, label: &'static str, summary: &'static str) -> Choice {
    Choice {
        value,
        label,
        desktop_label: None,
        summary: Some(summary),
        requires: None,
    }
}

const fn requiring(value: &'static str, label: &'static str, requires: &'static str) -> Choice {
    Choice {
        value,
        label,
        desktop_label: None,
        summary: None,
        requires: Some(requires),
    }
}

// Eight fields of one record set at once; the shorthands below are what
// the lists actually call.
#[allow(clippy::too_many_arguments)]
const fn spec(
    key: &'static str,
    label: &'static str,
    summary: Option<&'static str>,
    kind: Kind,
    choices: &'static [Choice],
    default: &'static str,
    scope: Scope,
    live: bool,
) -> Spec {
    Spec {
        key,
        label,
        summary,
        desktop_summary: None,
        kind,
        choices,
        default,
        scope,
        live,
        gate: None,
    }
}

const fn bool(
    key: &'static str,
    label: &'static str,
    summary: &'static str,
    default: bool,
    scope: Scope,
    live: bool,
) -> Spec {
    spec(
        key,
        label,
        Some(summary),
        Kind::Bool,
        &[],
        if default { "true" } else { "false" },
        scope,
        live,
    )
}

const fn integer(
    key: &'static str,
    label: &'static str,
    summary: &'static str,
    default: &'static str,
    scope: Scope,
    live: bool,
) -> Spec {
    spec(key, label, Some(summary), Kind::Int, &[], default, scope, live)
}

const fn text(
    key: &'static str,
    label: &'static str,
    summary: &'static str,
    default: &'static str,
    scope: Scope,
    live: bool,
) -> Spec {
    spec(key, label, Some(summary), Kind::Text, &[], default, scope, live)
}

const fn choices(
    key: &'static str,
    label: &'static str,
    summary: Option<&'static str>,
    default: &'static str,
    scope: Scope,
    live: bool,
    choices: &'static [Choice],
) -> Spec {
    spec(key, label, summary, Kind::Choice, choices, default, scope, live)
}

/// The same option, offered only where `key` says one of `values`.
const fn when(spec: Spec, key: &'static str, values: &'static [&'static str]) -> Spec {
    Spec {
        gate: Some(Gate { key, values }),
        ..spec
    }
}

const VIEW_ONLY: Spec = bool(
    "ViewOnly",
    "View only",
    "Do not send input or clipboard events.",
    false,
    Scope::Connection,
    true,
);

const BELL: Spec = Spec {
    desktop_summary: Some("Show a bell in the window's title when the remote server rings the bell."),
    ..bool(
        "AcceptBell",
        "Bell",
        "Vibrate when the remote servers rings the bell.",
        true,
        Scope::Global,
        false,
    )
};

const SHARED: Spec = bool(
    "Shared",
    "Shared",
    "Do not ask the server to disconnect other clients.",
    true,
    Scope::Connection,
    false,
);

const ANONYMOUS_TLS: Spec = bool(
    "AnonymousTLS",
    "Unverified encryption",
    "Ask before using encryption without a peer certificate.",
    true,
    Scope::Connection,
    false,
);

const COMPRESSION_LEVELS: &[Choice] = &[
    choice("auto", "Automatic"),
    choice("1", "1 (fastest)"),
    choice("3", "3"),
    choice("6", "6 (balanced)"),
    choice("9", "9 (smallest)"),
];

const DESKTOP_SIZES: &[Choice] = &[
    choice("remote", "Do not change"),
    choice("device", "Match local device"),
    choice("1920x1200", "1920 × 1200"),
    choice("1920x1080", "1920 × 1080"),
    choice("1600x1000", "1600 × 1000"),
    choice("1366x768", "1366 × 768"),
    choice("1280x800", "1280 × 800"),
    choice("2560x1440", "2560 × 1440"),
];

const MONITOR_COUNTS: &[Choice] = &[
    choice("1", "One"),
    choice("2", "Two"),
    choice("3", "Three"),
];

/// What the RFB backend can be told.
///
/// Deliberately shorter than [`REALVNC`]. That list is long because it is a
/// selection from somebody else's parameter registry, most of which exists for
/// their own transports and identity system; this one is the whole of what the
/// protocol has to offer, and an option nobody can describe is worse than no
/// option (the same rule, from the other end).
///
/// Two of these are **live in a way RealVNC's are not**, and that is the
/// protocol rather than a nicety: `SetEncodings` may be sent at any point in a
/// session, so changing the encoding or the compression acts on the picture in
/// front of you. RealVNC's equivalent needs a *changed* quality and carries
/// the group with it.
pub static RFB: &[Spec] = &[
    choices(
        "Security",
        "Encryption",
        Some("TLS (VeNCrypt) extension support."),
        "prefer",
        Scope::Connection,
        false,
        &[
            choice("prefer", "Where the server offers it"),
            choice("require", "Required"),
            choice("plain", "Never"),
        ],
    ),
    // The two below are the connection's with the backend's as their default,
    // since which encoding and how hard to compress is mostly a fact about the
    // link the phone is on. There is no colour row: the RFB client works in one
    // pixel format on purpose, which is what makes a region read a row copy,
    // and a reduced one would put a conversion back in the path for every
    // decoder it has.
    choices(
        "Encoding",
        "Encoding",
        Some("Preferred image encoding, overriding the server's selection if not Automatic."),
        "auto",
        Scope::Layered,
        true,
        &[
            choice("auto", "Automatic"),
            choice("zrle", "ZRLE"),
            choice("hextile", "Hextile"),
            choice("rre", "RRE"),
            choice("raw", "Raw (uncompressed)"),
        ],
    ),
    choices(
        "Compression",
        "Compression",
        Some("Image compression level, where supported by the encoding."),
        "auto",
        Scope::Layered,
        true,
        COMPRESSION_LEVELS,
    ),
    VIEW_ONLY,
    SHARED,
    BELL,
];

/// What the libvncclient backend can be told.
///
/// The option list is the same shape as the RFB one and differs where the two
/// clients differ, which is the point of having both: **Tight is here**, with
/// a JPEG quality beside it, and **the encryption choice is not** — this
/// client does VeNCrypt, but it takes whatever the server offers and there is
/// no call to prefer, require or refuse it.
///
/// The one row about encryption here asks something else: whether a tunnel
/// with no certificate in it may be taken at all. That is not a preference
/// between encrypted and not, it is whether a question gets asked or the
/// session ends, and the same row is on the TigerVNC backend for the same
/// reason.
///
/// The colour row is the same question the RealVNC and TigerVNC backends ask,
/// asked in the same words, and it is the only one of the three that cannot
/// change mid-session: libvncclient sends a pixel format and reallocates
/// around it with no point at which nothing is in flight, so a change here
/// would be decoding whatever is already on its way at the wrong width.
pub static LIBVNC: &[Spec] = &[
    choices(
        "Encoding",
        "Encoding",
        Some("Preferred image encoding, overriding the server's selection if not Automatic."),
        "auto",
        Scope::Layered,
        true,
        &[
            choice("auto", "Automatic"),
            choice("tight", "Tight"),
            choice("zrle", "ZRLE"),
            choice("trle", "TRLE"),
            choice("ultra", "Ultra"),
            choice("hextile", "Hextile"),
            choice("rre", "RRE"),
            choice("raw", "Raw (uncompressed)"),
        ],
    ),
    choices(
        "Compression",
        "Compression",
        Some("Image compression level, where supported by the encoding."),
        "auto",
        Scope::Layered,
        true,
        COMPRESSION_LEVELS,
    ),
    choices(
        "Quality",
        "Picture quality",
        Some("JPEG image quality, for Tight encoding."),
        "lossless",
        Scope::Layered,
        true,
        &[
            choice("lossless", "Lossless"),
            choice("9", "9 (best)"),
            choice("7", "7"),
            choice("5", "5 (balanced)"),
            choice("3", "3"),
            choice("0", "0 (smallest)"),
        ],
    ),
    // The same key and values as the RealVNC backend's colour row.
    choices(
        "ColorLevel",
        "Colour depth",
        Some("Pixel color depth. Takes effect on the next connection."),
        "full",
        Scope::Layered,
        false,
        &[
            choice("full", "Full colour (24-bit)"),
            choice("rgb222", "6-bit (rgb222)"),
            choice("rgb111", "3-bit (rgb111)"),
        ],
    ),
    VIEW_ONLY,
    SHARED,
    ANONYMOUS_TLS,
    BELL,
];

/// What the TigerVNC backend can be told.
///
/// The option list is the libvncclient one with a different meaning under the
/// encoding row: TigerVNC's client always offers every encoding it can decode
/// and names one of them preferred, so this is a hint rather than a
/// restriction and there is no "automatic" to choose — the server chooses
/// either way.
///
/// Where there *is* an automatic to offer is the picture quality and the
/// colour depth, and only here: this is the one client in the repository that
/// measures its own throughput, so it is the one that can move a setting on
/// somebody's behalf. Both rules are the viewer's own, thresholds included.
pub static TIGERVNC: &[Spec] = &[
    choices(
        "Encoding",
        "Encoding",
        Some("Preferred image encoding. The server may still choose to use a different one."),
        "tight",
        Scope::Layered,
        true,
        &[
            choice("tight", "Tight"),
            requiring("h264", "H.264", "H264"),
            choice("zrle", "ZRLE"),
            choice("hextile", "Hextile"),
            choice("rre", "RRE"),
            choice("raw", "Raw (uncompressed)"),
        ],
    ),
    choices(
        "Compression",
        "Compression",
        Some("Image compression level, where supported by the encoding."),
        "auto",
        Scope::Layered,
        true,
        COMPRESSION_LEVELS,
    ),
    choices(
        "Quality",
        "Picture quality",
        Some("JPEG image quality, for Tight encoding."),
        "lossless",
        Scope::Layered,
        true,
        &[
            choice("auto", "Automatic"),
            choice("lossless", "Lossless"),
            choice("9", "9 (best)"),
            choice("7", "7"),
            choice("5", "5 (balanced)"),
            choice("3", "3"),
            choice("0", "0 (smallest)"),
        ],
    ),
    // The same key and values as the RealVNC backend's colour row.
    choices(
        "ColorLevel",
        "Colour depth",
        Some("Pixel color depth."),
        "full",
        Scope::Layered,
        true,
        &[
            choice("auto", "Automatic"),
            choice("full", "Full colour (24-bit)"),
            choice("rgb222", "6-bit (rgb222)"),
            choice("rgb111", "3-bit (rgb111)"),
        ],
    ),
    // Their viewer's own parameter. QEMU's audio extension, which only
    // servers that speak it offer — QEMU's own, and the ones built the same
    // way — so against anything else this row changes nothing at all.
    bool(
        "Audio",
        "Sound",
        "Play audio sent by the server, if it offers any.",
        true,
        Scope::Layered,
        false,
    ),
    VIEW_ONLY,
    SHARED,
    ANONYMOUS_TLS,
    bool(
        "H264",
        "Offer H.264 encoding",
        "Allow the server to use H.264 encoding, with local hardware-accelerated decoding.",
        false,
        Scope::Global,
        true,
    ),
    BELL,
];

/// What the IronRDP backend can be told.
///
/// Shorter than [`RFB`], and shorter for a different reason. RFB's list is
/// short because the protocol has little to say; this one is short because
/// almost everything RDP negotiates is negotiated *well* — the codec, the
/// compression, the colour depth — and an option that re-decides what the two
/// ends already agreed on is a way to make a session worse.
///
/// Five things are deliberately **not** here:
///
/// - **The keyboard layout.** RDP sends key *positions*, and `rdp::keymap`
///   maps keysyms onto a US layout, so the layout named at connect time has to
///   be the one that table was written for. Offering a choice would let
///   someone pick a layout that turns every letter into a different letter.
/// - **The clipboard.** The channel is open in both directions, but whether to
///   share the local clipboard is a question about the client and is
///   answered once for every protocol, in the client's own settings.
/// - **The domain**, RDP's third credential, which is not a row because it is
///   already a field: `DOMAIN\user` and `user@domain` are split out of the
///   user name, which is what every RDP client takes and what anyone who has
///   one will type.
/// - **The colour depth**, which the three VNC backends do offer. There it is
///   a wire format the server encodes into; here the picture arrives as
///   RemoteFX or as bitmaps, and the codec has already decided what a pixel
///   costs — 16-bit would apply to one of those two paths, save nothing where
///   the codec is on, and put a conversion in front of a framebuffer that is
///   32 bits either way.
/// - **The graphics pipeline (MS-RDPEGFX)**, which is how a machine since
///   Windows 8 would rather send a desktop and the only path that carries
///   H.264. It was built, driven against both servers and taken out again,
///   because IronRDP's implementation was then incomplete and, where it was
///   complete, wrong. Nothing in the library set the capability flag a server
///   needs before it will open the channel; the client received RFX
///   Progressive and discarded it, though a finished decoder for it sat unused
///   one crate away; and the ClearCodec that Windows sends for most of an
///   interface was rejected outright — fifteen ClearCodec and twelve
///   progressive failures in one short session, which is a desktop with solid
///   rectangles of the wrong colour through it. Patched, all of that got as
///   far as working against xrdp and no further. The library has since grown
///   all three missing pieces — the flag and both decoders, and a session that
///   composites what the pipeline draws — but upstream's own client still
///   ships with the flag off, and none of it has been driven against a Windows
///   host from here since. Until it has, offering it would be the same bet as
///   before: a choice known to be right on one server and not known to be
///   right on the one the protocol belongs to.
pub static IRONRDP: &[Spec] = &[
    choices(
        "Nla",
        "NLA",
        Some("Network Level Authentication authenticates using CredSSP before the session starts rather than using the server-side login screen."),
        "prefer",
        Scope::Connection,
        false,
        &[
            choice("prefer", "Where the server offers it"),
            choice("require", "Required"),
            choice("off", "Never"),
        ],
    ),
    choices(
        "DesktopSize",
        "Desktop size",
        Some("Preferred screen resolution."),
        "1920x1200",
        Scope::Connection,
        false,
        DESKTOP_SIZES,
    ),
    choices(
        "Monitors",
        "Monitors",
        Some("Simulate multiple monitors."),
        "1",
        Scope::Connection,
        false,
        MONITOR_COUNTS,
    ),
    choices(
        "Graphics",
        "Graphics",
        Some("EGFX is not supported, RemoteFX is older, and Bitmap sends raw images."),
        "rfx",
        Scope::Layered,
        false,
        &[choice("rfx", "RemoteFX"), choice("bitmap", "Bitmaps")],
    ),
    choices(
        "Experience",
        "Visual effects",
        Some("Controls the amount of animation and visual effects, if supported by the server."),
        "balanced",
        Scope::Layered,
        false,
        &[
            choice("full", "Everything"),
            choice("balanced", "No animation"),
            choice("plain", "As little as possible"),
        ],
    ),
    bool(
        "Compression",
        "Compression",
        "Enable connection-level compression. Uses less bandwidth, but more CPU.",
        true,
        Scope::Layered,
        false,
    ),
    VIEW_ONLY,
];

/// What the FreeRDP backend can be told.
///
/// It began as the other RDP backend's rows copied so the two would be
/// comparable, and three of these are now things that client cannot be asked
/// at all: **EGFX**, the graphics pipeline, which is how a machine since
/// Windows 8 would rather send a desktop and the only path that carries H.264;
/// the **sound**, which is the one whole subsystem no backend behind this seam
/// has ever had; and the **interface size**, which is the only answer the phone
/// has to a desktop whose text is sized for a monitor and is being read on
/// it.
///
/// What is deliberately **not** here:
///
/// - **The keyboard layout.** Every key goes as a position against a US table,
///   and the phone's own layout has already turned a press into a character
///   before the backend sees it — so the layout named at connect time is what
///   turns those positions back into the characters that were typed, and
///   naming another one makes every letter a different letter.
/// - **The codecs one at a time.** Progressive and planar cost nothing and are
///   decoded in software either way; the two H.264 switches are one switch,
///   since the pair also selects every capability version above 8.1. What is
///   left is video or no video, which is a value on the picture row.
/// - **Small cache and thin client.** Both are a phone telling a server to
///   expect less of it. The cache they shrink is the server's, the memory it
///   costs is the server's, and the one part that reaches the picture asks for
///   a cheaper H.264 profile on a phone that decodes in hardware.
/// - **Drives, printers and smartcards.** A printer needs CUPS and a smartcard
///   needs PC/SC, neither of which is in this build; a drive needs a POSIX
///   path, and what a person can grant on Android is a document tree with no
///   path at all.
/// - **The clipboard** (a question about the client, answered once for every
///   protocol in the client's own settings), **the domain** (already a field,
///   split out of the user name) and **the colour depth** (the codec has
///   decided what a pixel costs long before it reaches a framebuffer that is
///   32 bits either way).
pub static FREERDP: &[Spec] = &[
    choices(
        "Nla",
        "NLA",
        Some("Network Level Authentication authenticates using CredSSP before the session starts rather than using the server-side login screen."),
        "prefer",
        Scope::Connection,
        false,
        &[
            choice("prefer", "If supported by the server"),
            choice("require", "Required"),
            choice("off", "Never"),
        ],
    ),
    bool(
        "LegacyTls",
        "Old TLS",
        "Allow TLS 1.0 and 1.1 and their weaker keys, for a server that has nothing newer, such as Windows 7 or Server 2008 R2 without updates.",
        false,
        Scope::Connection,
        false,
    ),
    choices(
        "DesktopSize",
        "Desktop size",
        Some("Preferred screen resolution."),
        "1920x1200",
        Scope::Connection,
        false,
        DESKTOP_SIZES,
    ),
    choices(
        "Scale",
        "Display scale",
        Some("Supported on Windows 8.1 and later."),
        "100",
        Scope::Connection,
        false,
        &[
            choice("100", "100%"),
            choice("140", "140%"),
            choice("180", "180%"),
        ],
    ),
    choices(
        "Monitors",
        "Monitors",
        Some("Simulate multiple monitors."),
        "1",
        Scope::Connection,
        false,
        MONITOR_COUNTS,
    ),
    choices(
        "Graphics",
        "Graphics",
        Some("EGFX is the newest and most advanced codec, RemoteFX is older, and Bitmap sends raw images."),
        "gfx",
        Scope::Layered,
        false,
        &[
            choice("gfx", "EGFX"),
            choice("gfx-novideo", "EGFX, no video"),
            choice("rfx", "RemoteFX"),
            choice("bitmap", "Bitmap"),
        ],
    ),
    choices(
        "Experience",
        "Visual effects",
        Some("Controls the amount of animation and visual effects, if supported by the server."),
        "balanced",
        Scope::Layered,
        false,
        &[
            choice("full", "Full"),
            choice("balanced", "Balanced"),
            choice("plain", "Limited"),
        ],
    ),
    choices(
        "Sound",
        "Sound",
        Some("Controls where sound is played, if supported by the server."),
        "off",
        Scope::Layered,
        false,
        &[
            choice("off", "Off"),
            Choice {
                desktop_label: Some("This computer"),
                ..choice("local", "This phone")
            },
            choice("remote", "The remote machine"),
        ],
    ),
    bool(
        "Compression",
        "Compression",
        "Enable connection-level compression. Uses less bandwidth, but more CPU.",
        true,
        Scope::Layered,
        false,
    ),
    VIEW_ONLY,
];

/// What the SPICE backend can be told.
///
/// A short list, and the reason is the protocol rather than restraint. There
/// is no security to negotiate — a plain port and a TLS port are different
/// ports, which is why the first row is a fact about the address rather than a
/// preference. There is no colour depth, since a surface is 32-bit and the
/// server compresses it. And there is no resize and no clipboard switch,
/// because both of those live in the guest's own agent rather than in the
/// protocol.
///
/// What is **not** here and could have been: which end owns the pointer. SPICE
/// has two mouse modes and lets a client ask for one, but which are on offer
/// is the far end's answer — a guest with a tablet attached offers both and
/// one without offers neither — so it is a fact about a live session and is
/// announced through the seam rather than chosen in a form.
pub static SPICE: &[Spec] = &[
    choices(
        "Security",
        "Encryption",
        None,
        "plain",
        Scope::Connection,
        false,
        &[
            described("plain", "None", "Plaintext."),
            described("tls", "TLS", "Encrypted."),
        ],
    ),
    // Live in the way RFB's encoding is: the message is read on the next image
    // the server encodes. Automatic is what a hypervisor does without being
    // asked, which for QEMU is GLZ where it helps.
    choices(
        "Compression",
        "Image compression",
        Some("Preferred image compression method."),
        "",
        Scope::Layered,
        true,
        &[
            choice("", "Automatic"),
            choice("auto-glz", "GLZ, if better"),
            choice("auto-lz", "LZ, if better"),
            choice("quic", "QUIC"),
            choice("glz", "GLZ"),
            choice("lz", "LZ"),
            choice("lz4", "LZ4"),
            choice("off", "None"),
        ],
    ),
    // The playback channel, TigerVNC's key and words for QEMU's other way of
    // sending a guest's sound: on unless turned off, since a guest that
    // plays nothing sends nothing.
    bool(
        "Audio",
        "Sound",
        "Play audio sent by the server, if it offers any.",
        true,
        Scope::Layered,
        false,
    ),
    bool(
        "ViewOnly",
        "View only",
        "Do not send input events.",
        false,
        Scope::Connection,
        true,
    ),
];

/// What the RustDesk backend can be told.
///
/// A short list, and the reason is the protocol rather than restraint: no
/// security to choose, since each way to a machine has exactly one answer to
/// that, and no colour depth, since the picture is a video stream. The codec
/// is the nearest thing to an encoding here and behaves nothing like one — it
/// is a preference between whole video codecs, answered by whichever of them
/// that machine has an encoder for.
///
/// The first three rows are how a machine is reached, and they are here rather
/// than beside the address because only one protocol here has more than one
/// answer to it. The server and its key are [`Scope::Layered`]: somebody
/// running their own rendezvous server has every machine on it, so the answer
/// belongs to the backend with a connection able to disagree.
///
/// What is **not** here and could have been: the peer's other displays. This
/// protocol captures one at a time and switches with a message, so a chooser
/// is a row whose contents depend on the connection that is running — a live
/// fact rather than a static description, and it is in the connection panel
/// with the desktop size, which is there for the same reason.
pub static RUSTDESK: &[Spec] = &[
    // The first row of the connection, because it says what the box above it
    // holds and because the two are not one transport with a switch on it: one
    // reaches a machine somebody can already address and encrypts nothing, and
    // the other reaches a machine nothing on this network can address and is
    // encrypted because of how it got there.
    choices(
        "ConnectBy",
        "Connect by",
        None,
        "id",
        Scope::Connection,
        false,
        &[
            described("id", "ID", "Connect by ID via a rendezvous server. Encrypted."),
            described("address", "Address", "Connect to a machine by IP. Not encrypted."),
        ],
    ),
    when(
        text(
            "RendezvousServer",
            "Rendezvous server",
            "Leave empty for the default public rendezvous server.",
            "",
            Scope::Layered,
            false,
        ),
        "ConnectBy",
        &["id"],
    ),
    when(
        text(
            "RendezvousKey",
            "Rendezvous server key",
            "Public key for the rendezvous server.",
            "",
            Scope::Layered,
            false,
        ),
        "ConnectBy",
        &["id"],
    ),
    // Both of these are live in the way RFB's encoding is: their option
    // message is read on the next frame the peer encodes.
    choices(
        "Quality",
        "Quality",
        Some("Control the video bitrate."),
        "balanced",
        Scope::Layered,
        true,
        &[
            choice("low", "Low"),
            choice("balanced", "Balanced"),
            choice("best", "Best"),
        ],
    ),
    choices(
        "FrameRate",
        "Frame rate",
        Some("Limits the maximum FPS."),
        "auto",
        Scope::Layered,
        true,
        &[
            choice("auto", "Automatic"),
            choice("10", "10"),
            choice("20", "20"),
            choice("30", "30"),
            choice("60", "60"),
        ],
    ),
    // Live like the two above it, and the one that is not free: the far end
    // answers a codec it is not already using by building an encoder and
    // starting again with a key frame.
    choices(
        "Codec",
        "Codec",
        Some("Preferred codec, if supported by the server."),
        "auto",
        Scope::Layered,
        true,
        &[
            choice("auto", "Automatic"),
            choice("vp9", "VP9"),
            choice("vp8", "VP8"),
            choice("av1", "AV1"),
            choice("h264", "H.264"),
            choice("h265", "H.265"),
        ],
    ),
    bool(
        "LockAfterSession",
        "Lock when the session ends",
        "Lock the remote screen on disconnecting.",
        false,
        Scope::Layered,
        false,
    ),
    VIEW_ONLY,
];

/// Which of RealVNC's parameters the clients offer.
///
/// Every name here is one of two things: a parameter recovered from the
/// binary's own registry, or one the original itself sets on `createSession`.
/// The registry holds a great many more — the whole underscore-prefixed
/// `_Hosted*` / `_Udp*` / `_Opus*` transport surface — and none is offered
/// until there is a reason to, because an option nobody can describe is worse
/// than no option.
///
/// [`Scope::Connection`] versus [`Scope::Global`] is a question about the
/// setting, not about the protocol: does one server want a different answer
/// from another? Quality does, encryption policy does not.
pub static REALVNC: &[Spec] = &[
    // Auto, not the core's own default of High: Auto follows the measured line
    // speed, and pinning High on every connection is deciding for a network
    // nobody has seen yet.
    choices(
        "Quality",
        "Picture quality",
        Some("Encoding and color depth preset."),
        "Auto",
        Scope::Layered,
        true,
        &[
            choice("Auto", "Automatic"),
            choice("High", "High"),
            choice("Medium", "Medium"),
            choice("Low", "Low"),
            choice("Custom", "Custom"),
        ],
    ),
    // The one row every other RFB backend has and this one did not. All eight
    // values of the enum the core registers, in the order it registers them,
    // rather than a guess or a subset — including the two that are lossy,
    // since the quality dial that would otherwise be how lossiness is asked
    // for is by definition out of the way here.
    //
    // Only a Custom quality reaches it, which is why the row is not
    // offered at any other. The picture policy builds its own list of
    // encodings for each of the three fixed classes and consults this
    // parameter only for the fourth, and that is the library rather than an
    // interface decision: it was read out of the class switch and driven both
    // ways — Automatic, High and Low all sent ZRLE with Hextile asked for, and
    // Custom sent Hextile.
    when(
        choices(
            "PreferredEncoding",
            "Encoding",
            Some("Preferred image encoding. The server may still choose to use a different one. Only used when quality is set to Custom"),
            "ZRLE2",
            Scope::Layered,
            true,
            &[
                choice("JPEG", "JPEG (lossy)"),
                choice("Zlib", "Zlib"),
                choice("ZRLE", "ZRLE"),
                choice("ZRLE2", "ZRLE2"),
                choice("TRLE", "TRLE"),
                choice("Hextile", "Hextile"),
                choice("Raw", "Raw (uncompressed)"),
                choice("JRLE", "JRLE (lossy)"),
            ],
        ),
        "Quality",
        &["Custom"],
    ),
    // The colour depth, offered where the core reads it and nowhere else.
    //
    // Low and Custom are the qualities that consult it. The picture policy
    // resolves a quality to one of four classes and picks the pixel format
    // from that: High and Medium ask for full colour whatever this says, Low
    // and Custom use it, and Automatic is whichever class the measured line
    // speed puts it in. Their own published reference says the parameter is
    // ignored unless the quality is Custom, and the library disagrees with the
    // documentation — the Low case is real, and was read out of the picture
    // policy and then driven: at High a connection with `rgb111` asks for
    // 24-bit colour, at Low and at Custom it asks for 3-bit.
    //
    // The default is full colour where the original forces `rgb222` at every
    // quality but Custom. That difference is smaller than it looked when it
    // was made: at High and Medium the core asks for full colour whatever
    // either of us says, so what it actually buys is a Low quality that keeps
    // its colour, and an Automatic one that keeps it on a slow line.
    when(
        choices(
            "ColorLevel",
            "Colour depth",
            Some("Pixel color depth. Only used when quality is set to Low or Custom."),
            "full",
            Scope::Layered,
            true,
            &[
                choice("full", "Full colour (24-bit)"),
                choice("rgb222", "64 colours (rgb222)"),
                choice("rgb111", "8 colours (rgb111)"),
            ],
        ),
        "Quality",
        &["Low", "Custom"],
    ),
    VIEW_ONLY,
    SHARED,
    bool(
        "AutoReconnect",
        "Reconnect automatically",
        "Do not ask before re-establishing dropped connections.",
        true,
        Scope::Connection,
        true,
    ),
    bool(
        "EnableUdpRfb",
        "Enable UDP",
        "Enable RealVNC's proprietary SCTP/RTP-based UDP protocol, falling back to TCP if the server doesn't respond within 500ms.",
        true,
        Scope::Connection,
        false,
    ),
    choices(
        "Encryption",
        "Encryption",
        Some("Whether to encrypt connections."),
        "PreferOn",
        Scope::Global,
        false,
        &[
            choice("AlwaysOn", "Required"),
            choice("PreferOn", "If supported by the server"),
            choice("PreferOff", "Only if required by the server"),
            choice("Server", "Let the server decide"),
        ],
    ),
    bool(
        "WarnUnencrypted",
        "Warn when unencrypted",
        "Show a warning when connecting to a server which doesn't support encryption.",
        true,
        Scope::Global,
        false,
    ),
    bool(
        "DotWhenNoCursor",
        "Dot for an invisible cursor",
        "Show a dot when the server hides the cursor.",
        true,
        Scope::Global,
        false,
    ),
    BELL,
    // The three clipboard parameters this core has are deliberately not rows.
    // Whether the local clipboard goes to a remote machine and comes back is a
    // question about the client, asked once in the client's own settings and
    // enforced there — the core's copies would be a second answer to it, in a
    // second screen, one layer down. Left at their defaults, which is on, so
    // the client's answer is the only one.
    integer(
        "KeepAliveInterval",
        "Keep-alive interval",
        "Seconds between keep-alives on an idle connection.",
        "30",
        Scope::Global,
        false,
    ),
    integer(
        "KeepAliveResponseTimeout",
        "Keep-alive timeout",
        "Seconds to wait for an answer before giving up.",
        "30",
        Scope::Global,
        false,
    ),
];

/// A backend's settings as it was built with: every key from its list at its
/// default, and whatever was set over that. Remembers which keys were set,
/// because one backend (RealVNC) treats a colour depth somebody chose
/// differently from one that is merely the default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    values: BTreeMap<String, String>,
    set: BTreeSet<String>,
}

impl Options {
    pub fn defaults(specs: &[Spec]) -> Options {
        Options {
            values: specs
                .iter()
                .map(|s| (s.key.to_string(), s.default.to_string()))
                .collect(),
            set: BTreeSet::new(),
        }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.values.insert(key.to_string(), value.to_string());
        self.set.insert(key.to_string());
    }

    /// The value, or empty where there is none — which is what every reader
    /// here expects of an option nobody has answered.
    pub fn get(&self, key: &str) -> &str {
        self.values.get(key).map(String::as_str).unwrap_or("")
    }

    /// Whether somebody set this, as opposed to it being the default.
    pub fn is_set(&self, key: &str) -> bool {
        self.set.contains(key)
    }

    /// Every choice whose required switch is off, put back to what
    /// [`Spec::fallback`] says; the keys that changed, for a live session to
    /// pass on. A fallback is nobody's choice, so it is not counted as set.
    /// The dialog and the live panel both keep to this whenever a switch
    /// moves.
    pub fn settle(&mut self, specs: &[Spec]) -> Vec<&'static str> {
        let mut changed = Vec::new();
        for spec in specs {
            if spec.stranded(self.get(spec.key), |key| self.bool(key)).is_some() {
                let value = spec.fallback(|key| self.bool(key));
                self.values.insert(spec.key.to_string(), value.to_string());
                self.set.remove(spec.key);
                changed.push(spec.key);
            }
        }
        changed
    }

    /// The first choice given without its switch, in words: the command line
    /// refuses `-o Encoding=h264` without `-o H264=true` rather than guess
    /// which of the two was meant.
    pub fn check_requirements(&self, specs: &[Spec]) -> Result<(), String> {
        for spec in specs {
            if let Some(choice) = spec.stranded(self.get(spec.key), |key| self.bool(key)) {
                let needs = choice.requires.unwrap_or_default();
                return Err(format!("{}={} needs {needs}=true", spec.key, choice.value));
            }
        }
        Ok(())
    }

    /// `Boolean.parseBoolean`: true for `"true"` in any case, false for
    /// everything else including nothing.
    pub fn bool(&self, key: &str) -> bool {
        self.get(key).eq_ignore_ascii_case("true")
    }

    /// A whole number, or `default` where it is not one.
    pub fn int(&self, key: &str, default: i64) -> i64 {
        self.get(key).trim().parse().unwrap_or(default)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

/// `key=value`, as `-o` takes it. The first `=` splits, so a value may hold
/// one; a missing one is an error in words.
pub fn parse_assignment(text: &str) -> Result<(&str, &str), String> {
    match text.split_once('=') {
        Some((key, value)) if !key.trim().is_empty() => Ok((key.trim(), value)),
        _ => Err(format!("{text:?} is not key=value")),
    }
}

/// The spec for `key` in `specs`, in words when there is none.
pub fn find<'a>(specs: &'a [Spec], key: &str) -> Result<&'a Spec, String> {
    specs.iter().find(|s| s.key == key).ok_or_else(|| {
        let keys: Vec<&str> = specs.iter().map(|s| s.key).collect();
        format!("no option {key:?}; the options are {}", keys.join(", "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignments_split_at_the_first_equals() {
        assert_eq!(parse_assignment("Encoding=zrle"), Ok(("Encoding", "zrle")));
        assert_eq!(parse_assignment("Key=a=b"), Ok(("Key", "a=b")));
        assert_eq!(parse_assignment("Compression="), Ok(("Compression", "")));
        assert!(parse_assignment("=x").is_err());
        assert!(parse_assignment("novalue").is_err());
    }

    #[test]
    fn a_value_is_checked_against_its_kind() {
        let encoding = find(RFB, "Encoding").unwrap();
        assert!(encoding.check("zrle").is_ok());
        assert!(encoding.check("tight").unwrap_err().contains("one of"));
        assert_eq!(encoding.label_for("rre", Platform::Linux), "RRE");
        assert_eq!(encoding.label_for("nonsense", Platform::Linux), "nonsense");
        let sound = find(FREERDP, "Sound").unwrap();
        assert_eq!(sound.label_for("local", Platform::Android), "This phone");
        assert_eq!(sound.label_for("local", Platform::Linux), "This computer");
        let bell = find(RFB, "AcceptBell").unwrap();
        assert!(bell.summary_on(Platform::Android).unwrap().starts_with("Vibrate"));
        assert!(bell.summary_on(Platform::Linux).unwrap().starts_with("Show a bell"));
        let shared = find(RFB, "Shared").unwrap();
        assert!(shared.check("true").is_ok());
        assert!(shared.check("yes").is_err());
        let keep = find(REALVNC, "KeepAliveInterval").unwrap();
        assert!(keep.check("45").is_ok());
        assert!(keep.check("soon").is_err());
        assert!(find(RFB, "Quality").is_err());
        assert!(find(RUSTDESK, "RendezvousServer").unwrap().check("anything").is_ok());
    }

    #[test]
    fn defaults_are_the_providers_and_overrides_are_remembered() {
        let mut options = Options::defaults(REALVNC);
        assert_eq!(options.get("Quality"), "Auto");
        assert_eq!(options.get("ColorLevel"), "full");
        assert!(!options.is_set("ColorLevel"));
        assert!(options.bool("Shared"));
        assert_eq!(options.int("KeepAliveInterval", 0), 30);
        options.set("ColorLevel", "rgb111");
        assert!(options.is_set("ColorLevel"));
        assert_eq!(options.get("ColorLevel"), "rgb111");
        assert_eq!(options.get("Nothing"), "");
        assert!(!options.bool("Nothing"));
        assert!(options.int("Quality", 7) == 7);
    }

    #[test]
    fn gates_and_requirements_came_across() {
        let encoding = find(REALVNC, "PreferredEncoding").unwrap();
        let gate = encoding.gate.unwrap();
        assert_eq!(gate.key, "Quality");
        assert!(gate.open("Custom"));
        assert!(!gate.open("High"));
        let h264 = TIGERVNC[0].choices.iter().find(|c| c.value == "h264").unwrap();
        assert_eq!(h264.requires, Some("H264"));
        assert_eq!(find(RUSTDESK, "RendezvousKey").unwrap().gate.unwrap().values, ["id"]);
    }

    #[test]
    fn a_choice_whose_switch_is_off_falls_back_to_the_default() {
        let mut options = Options::defaults(TIGERVNC);
        options.set("H264", "true");
        options.set("Encoding", "h264");
        assert!(options.settle(TIGERVNC).is_empty(), "its switch is on");
        assert_eq!(options.check_requirements(TIGERVNC), Ok(()));
        options.set("H264", "false");
        assert_eq!(options.check_requirements(TIGERVNC), Err("Encoding=h264 needs H264=true".to_string()));
        assert_eq!(options.settle(TIGERVNC), ["Encoding"]);
        assert_eq!(options.get("Encoding"), "tight");
        assert!(!options.is_set("Encoding"), "a fallback is nobody's choice");
        assert!(options.settle(TIGERVNC).is_empty(), "and settled stays settled");
        // Any other choice stands without the switch.
        options.set("Encoding", "zrle");
        assert!(options.settle(TIGERVNC).is_empty());
        assert_eq!(options.get("Encoding"), "zrle");
        // Nothing in any list has a default that needs a switch.
        for specs in [RFB, LIBVNC, TIGERVNC, IRONRDP, FREERDP, SPICE, RUSTDESK, REALVNC] {
            for spec in specs {
                assert!(spec.stranded(spec.default, |_| false).is_none(), "{}", spec.key);
            }
        }
    }

    /// Every list has the keys it had when the phone's lists became these, in
    /// the same order: a key is stored in connection records and preference
    /// files, so one renamed is a setting forgotten.
    #[test]
    fn the_keys_are_the_ones_records_hold() {
        let keys = |specs: &[Spec]| specs.iter().map(|s| s.key).collect::<Vec<_>>();
        assert_eq!(
            keys(RFB),
            ["Security", "Encoding", "Compression", "ViewOnly", "Shared", "AcceptBell"]
        );
        assert_eq!(
            keys(LIBVNC),
            [
                "Encoding",
                "Compression",
                "Quality",
                "ColorLevel",
                "ViewOnly",
                "Shared",
                "AnonymousTLS",
                "AcceptBell"
            ]
        );
        assert_eq!(
            keys(TIGERVNC),
            [
                "Encoding",
                "Compression",
                "Quality",
                "ColorLevel",
                "Audio",
                "ViewOnly",
                "Shared",
                "AnonymousTLS",
                "H264",
                "AcceptBell"
            ]
        );
        assert_eq!(
            keys(IRONRDP),
            [
                "Nla",
                "DesktopSize",
                "Monitors",
                "Graphics",
                "Experience",
                "Compression",
                "ViewOnly"
            ]
        );
        assert_eq!(
            keys(FREERDP),
            [
                "Nla",
                "LegacyTls",
                "DesktopSize",
                "Scale",
                "Monitors",
                "Graphics",
                "Experience",
                "Sound",
                "Compression",
                "ViewOnly"
            ]
        );
        assert_eq!(keys(SPICE), ["Security", "Compression", "Audio", "ViewOnly"]);
        assert_eq!(
            keys(RUSTDESK),
            [
                "ConnectBy",
                "RendezvousServer",
                "RendezvousKey",
                "Quality",
                "FrameRate",
                "Codec",
                "LockAfterSession",
                "ViewOnly"
            ]
        );
        assert_eq!(
            keys(REALVNC),
            [
                "Quality",
                "PreferredEncoding",
                "ColorLevel",
                "ViewOnly",
                "Shared",
                "AutoReconnect",
                "EnableUdpRfb",
                "Encryption",
                "WarnUnencrypted",
                "DotWhenNoCursor",
                "AcceptBell",
                "KeepAliveInterval",
                "KeepAliveResponseTimeout"
            ]
        );
        for specs in [RFB, LIBVNC, TIGERVNC, IRONRDP, FREERDP, SPICE, RUSTDESK, REALVNC] {
            for spec in specs {
                assert!(spec.check(spec.default).is_ok(), "{} default", spec.key);
            }
        }
    }
}
