// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The RFB backend's entry in the catalog: what it is called, where it goes
//! in a picker, how its identities are pinned, and every option it takes.
//! Compiled by `remotedesktop-api`, which names this file by path
//! (`backends/api/src/catalog.rs`), and not by this module's crate: the
//! phone's generated Java (`vento-codegen`) reads it without building the
//! backend.

use crate::catalog::About;
use crate::options::{Scope, Spec, VIEW_ONLY, BELL, SHARED, COMPRESSION_LEVELS, choice, choices};
use crate::trust::Pinning;

pub static RFB: About = About {
    id: "rfb",
    // Named for what it is rather than for being the default one: there is
    // more than one VNC client here, and which implementation a connection
    // is on is the only thing these names have to tell apart.
    name: "VNC (Rust)",
    short_name: "Rust",
    description: "Rust RFB implementation. Does not support Tight encoding, and typically uses more bandwith and performs worse on slow networks. Allows remote pointer position updates, where supported by the server.",
    // First in a picker: it is ours, and it is the one that will still be
    // here.
    order: 10,
    pinning: Pinning::ADDRESS,
    user_has_domain: false,
    saves_identity: false,
    options: OPTIONS,
};

/// What the RFB backend can be told.
///
/// Deliberately shorter than [RealVNC's](crate::catalog::REALVNC). That list
/// is long because it is a selection from somebody else's parameter registry,
/// most of which exists for their own transports and identity system; this
/// one is the whole of what the protocol has to offer, and an option nobody
/// can describe is worse than no option (the same rule, from the other end).
///
/// Two of these are **live in a way RealVNC's are not**, and that is the
/// protocol rather than a nicety: `SetEncodings` may be sent at any point in a
/// session, so changing the encoding or the compression acts on the picture in
/// front of you. RealVNC's equivalent needs a *changed* quality and carries
/// the group with it.
pub static OPTIONS: &[Spec] = &[
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::Platform;
    use crate::options::find;

    /// The keys it had when the phone's lists became these, in the same
    /// order: a key is stored in connection records and preference files,
    /// so one renamed is a setting forgotten.
    #[test]
    fn the_keys_are_the_ones_records_hold() {
        let keys: Vec<&str> = OPTIONS.iter().map(|s| s.key).collect();
        assert_eq!(
            keys,
            [
                "Security",
                "Encoding",
                "Compression",
                "ViewOnly",
                "Shared",
                "AcceptBell"
            ]
        );
    }

    #[test]
    fn the_encodings_are_the_ones_this_client_decodes() {
        let encoding = find(OPTIONS, "Encoding").unwrap();
        assert!(encoding.check("zrle").is_ok());
        assert!(encoding.check("tight").unwrap_err().contains("one of"));
        assert_eq!(encoding.label_for("rre", Platform::Linux), "RRE");
        assert!(find(OPTIONS, "Quality").is_err());
    }
}
