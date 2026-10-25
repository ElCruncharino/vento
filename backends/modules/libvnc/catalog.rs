// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The libvncclient backend's catalog entry: its name, its position in a
//! picker, how its identities are pinned, and its options. Compiled by
//! `remotedesktop-api`, which includes this file by path
//! (`backends/api/src/catalog.rs`), and not by this module's crate, so the
//! Linux client's picker and the phone's generated Java (`vento-codegen`)
//! can read it without building the backend.

use crate::catalog::About;
use crate::options::{
    Scope, Spec, VIEW_ONLY, BELL, SHARED, ANONYMOUS_TLS, COMPRESSION_LEVELS, choice, choices,
};
use crate::trust::Pinning;

pub static LIBVNC: About = About {
    id: "libvnc",
    // The project is LibVNCServer and the library is libvncclient; the name
    // shows that this row and the VNC rows beside it are the same protocol
    // with different implementations.
    name: "VNC (LibVNC)",
    short_name: "LibVNC",
    description: "Usually the best option for typical usage. Has the lowest bandwidth usage for small incremental updates, and is the most CPU efficient.",
    // After our own RFB client, the one meant to stay.
    order: 15,
    pinning: Pinning::ADDRESS,
    user_has_domain: false,
    saves_identity: false,
    options: OPTIONS,
};

/// The libvncclient backend's options.
///
/// The list follows the RFB one and differs where the two clients differ:
/// **Tight is here**, with a JPEG quality option, and **there is no
/// encryption choice**. This client does VeNCrypt, but it takes whatever the
/// server offers and has no call to prefer, require or refuse it.
///
/// The one encryption row here is a different question: whether a tunnel
/// without a certificate may be used at all. It decides between asking the
/// user and ending the session, not between encrypted and unencrypted, and
/// the TigerVNC backend has the same row for the same reason.
///
/// The colour row matches the RealVNC and TigerVNC backends' row, and it is
/// the only one of the three that cannot change mid-session: libvncclient
/// sends a pixel format and reallocates around it without waiting for the
/// updates in flight to drain, so a change would decode those at the wrong
/// width.
pub static OPTIONS: &[Spec] = &[
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys the phone's lists had before they moved here, in the same
    /// order: keys are stored in connection records and preference files,
    /// so renaming one loses the setting.
    #[test]
    fn the_keys_are_the_ones_records_hold() {
        let keys: Vec<&str> = OPTIONS.iter().map(|s| s.key).collect();
        assert_eq!(
            keys,
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
    }
}
