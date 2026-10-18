// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The libvncclient backend's entry in the catalog: what it is called, where
//! it goes in a picker, how its identities are pinned, and every option it
//! takes. Compiled by `remotedesktop-api`, which names this file by path
//! (`backends/api/src/catalog.rs`), and not by this module's crate: the
//! Linux client's picker and the phone's generated Java (`vento-codegen`)
//! read it without building the backend.

use crate::catalog::About;
use crate::options::{
    Scope, Spec, VIEW_ONLY, BELL, SHARED, ANONYMOUS_TLS, COMPRESSION_LEVELS, choice, choices,
};
use crate::trust::Pinning;

pub static LIBVNC: About = About {
    id: "libvnc",
    // The project is LibVNCServer and the library is libvncclient; what
    // matters is that this row and the two beside it are the same protocol
    // with a different implementation behind it.
    name: "VNC (LibVNC)",
    short_name: "LibVNC",
    description: "Usually the best option for typical usage. Has the lowest bandwidth usage for small incremental updates, and is the most CPU efficient.",
    // After our own client, which is the one that will still be here.
    order: 15,
    pinning: Pinning::ADDRESS,
    user_has_domain: false,
    options: OPTIONS,
};

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

    /// The keys it had when the phone's lists became these, in the same
    /// order: a key is stored in connection records and preference files,
    /// so one renamed is a setting forgotten.
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
