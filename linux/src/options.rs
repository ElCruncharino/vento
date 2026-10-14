// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What each backend can be told, described well enough to offer without
//! knowing what it means: `BackendOption` in Java, and the eight lists the
//! `*Provider` classes hold, ported row for row.
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
    pub label: &'static str,
    pub summary: Option<&'static str>,
    pub requires: Option<&'static str>,
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
    /// The longer explanation.
    pub summary: Option<&'static str>,
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
    /// For a `Choice`: what to show for `value` (a `&'static str` from the
    /// list), or the value itself when it is not one of the choices.
    pub fn label_for<'a>(&self, value: &'a str) -> &'a str {
        self.choices
            .iter()
            .find(|c| c.value == value)
            .map(|c| c.label)
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
}

const fn choice(value: &'static str, label: &'static str) -> Choice {
    Choice {
        value,
        label,
        summary: None,
        requires: None,
    }
}

const fn described(value: &'static str, label: &'static str, summary: &'static str) -> Choice {
    Choice {
        value,
        label,
        summary: Some(summary),
        requires: None,
    }
}

const fn requiring(value: &'static str, label: &'static str, requires: &'static str) -> Choice {
    Choice {
        value,
        label,
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

const BELL: Spec = bool(
    "AcceptBell",
    "Bell",
    "Vibrate when the remote servers rings the bell.",
    true,
    Scope::Global,
    false,
);

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

/// `RfbProvider`.
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

/// `LibVncProvider`.
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

/// `TigerVncProvider`.
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

/// `IronRdpProvider`.
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

/// `FreeRdpProvider`.
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
            choice("local", "This computer"),
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

/// `SpiceProvider`.
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
    bool(
        "ViewOnly",
        "View only",
        "Do not send input events.",
        false,
        Scope::Connection,
        true,
    ),
];

/// `RustDeskProvider`.
pub static RUSTDESK: &[Spec] = &[
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

/// `RealVncProvider`.
pub static REALVNC: &[Spec] = &[
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
        assert_eq!(encoding.label_for("rre"), "RRE");
        assert_eq!(encoding.label_for("nonsense"), "nonsense");
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

    /// Every list has the keys its Java twin has, in the same order, with the
    /// same defaults — the probe that the port is a port.
    #[test]
    fn the_lists_are_the_providers_lists() {
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
        assert_eq!(keys(SPICE), ["Security", "Compression", "ViewOnly"]);
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
