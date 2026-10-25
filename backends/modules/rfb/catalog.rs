// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The RFB backend's catalog entry: its name, its place in a picker, how its
//! identities are pinned, and its options. Compiled by `remotedesktop-api`,
//! which includes this file by path (`backends/api/src/catalog.rs`), not by
//! this module's crate, so the Linux client's picker and the Android generated
//! Java (`vento-codegen`) read it without building the backend.

use crate::catalog::About;
use crate::options::{Scope, Spec, VIEW_ONLY, BELL, SHARED, COMPRESSION_LEVELS, choice, choices};
use crate::trust::Pinning;

pub static RFB: About = About {
    id: "rfb",
    // Named for the implementation rather than for being the default: there
    // is more than one VNC client here, and the implementation is the only
    // thing these names need to distinguish.
    name: "VNC (Rust)",
    short_name: "Rust",
    description: "Rust RFB implementation. Does not support Tight encoding, and typically uses more bandwith and performs worse on slow networks. Allows remote pointer position updates, where supported by the server.",
    // First in a picker: it is ours, and it will be kept.
    order: 10,
    pinning: Pinning::ADDRESS,
    user_has_domain: false,
    saves_identity: false,
    options: OPTIONS,
};

/// The RFB backend's options.
///
/// Deliberately shorter than [RealVNC's](crate::catalog::REALVNC). That list
/// is long because it is a selection from RealVNC's parameter registry, most
/// of which exists for their own transports and identity system; this one is
/// everything the protocol offers, and an option that can't be described is
/// worse than no option (the same rule, from the other side).
///
/// Two of these **apply live, unlike RealVNC's**, because of the protocol:
/// `SetEncodings` may be sent at any point in a session, so changing the
/// encoding or the compression affects the current picture. RealVNC's
/// equivalent needs a *changed* quality and carries the group with it.
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
    // The two below are per connection with the backend's value as their
    // default, since the best encoding and compression depend mostly on the
    // link the device is on. There is no colour option: the RFB client works in
    // one pixel format on purpose, which makes a region read a row copy, and a
    // reduced format would put a conversion back in the path for every decoder.
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

    /// The keys the Android lists had when they moved here, in the same
    /// order: keys are stored in connection records and preference files,
    /// so renaming one loses the setting.
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
