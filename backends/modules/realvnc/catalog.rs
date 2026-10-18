// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The RealVNC backend's entry in the catalog: what it is called, where it
//! goes in a picker, how its identities are pinned, and every option it
//! takes. Compiled by `remotedesktop-api`, which names this file by path
//! (`backends/api/src/catalog.rs`), and not by a backend crate (this
//! module has none): the Linux client's picker and the phone's generated Java
//! (`vento-codegen`) read it without building the backend.

use crate::catalog::About;
use crate::options::{Scope, Spec, VIEW_ONLY, BELL, SHARED, choice, choices, bool, integer, when};
use crate::trust::Pinning;

pub static REALVNC: About = About {
    id: "realvnc",
    name: "VNC (RealVNC)",
    // Three of the backends speak RFB, so a badge has to name the
    // implementation as well as the protocol.
    short_name: "RealVNC",
    description: "RealVNC's proprietary client implementation. This is the only one which supports UDP, ZRLE2, and RealVNC's proprietary authentication/encryption. It performs significantly better than the others when connecting to a RealVNC server, especially for typical desktop usage.",
    // After the open VNC and RDP clients: the more capable client against
    // its own server, and the one with a ceiling on its life — 4.9.3's
    // `libvncviewer.so` is 16 KB aligned, but it is somebody else's binary
    // and cannot be rebuilt when the next page size arrives.
    order: 30,
    pinning: Pinning::ADDRESS,
    user_has_domain: false,
    options: OPTIONS,
};

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
pub static OPTIONS: &[Spec] = &[
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{Options, find};

    /// The keys it had when the phone's lists became these, in the same
    /// order: a key is stored in connection records and preference files,
    /// so one renamed is a setting forgotten.
    #[test]
    fn the_keys_are_the_ones_records_hold() {
        let keys: Vec<&str> = OPTIONS.iter().map(|s| s.key).collect();
        assert_eq!(
            keys,
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
    }

    #[test]
    fn the_defaults_are_the_providers_and_a_set_colour_depth_is_remembered() {
        let mut options = Options::defaults(OPTIONS);
        assert_eq!(options.get("Quality"), "Auto");
        assert_eq!(options.get("ColorLevel"), "full");
        assert!(!options.is_set("ColorLevel"));
        assert!(options.bool("Shared"));
        assert_eq!(options.int("KeepAliveInterval", 0), 30);
        options.set("ColorLevel", "rgb111");
        assert!(options.is_set("ColorLevel"));
        let keep = find(OPTIONS, "KeepAliveInterval").unwrap();
        assert!(keep.check("45").is_ok());
        assert!(keep.check("soon").is_err());
    }

    #[test]
    fn the_encoding_is_offered_only_at_a_custom_quality() {
        let gate = find(OPTIONS, "PreferredEncoding").unwrap().gate.unwrap();
        assert_eq!(gate.key, "Quality");
        assert!(gate.open("Custom"));
        assert!(!gate.open("High"));
    }
}
