// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The TigerVNC backend's entry in the catalog: what it is called, where it
//! goes in a picker, how its identities are pinned, and every option it
//! takes. Compiled by `remotedesktop-api`, which names this file by path
//! (`backends/api/src/catalog.rs`), and not by this module's crate: the
//! phone's generated Java (`vento-codegen`) reads it without building the
//! backend.

use crate::catalog::About;
use crate::options::{
    Scope, Spec, VIEW_ONLY, BELL, SHARED, ANONYMOUS_TLS, COMPRESSION_LEVELS, choice, requiring,
    choices, bool,
};
use crate::trust::Pinning;

pub static TIGERVNC: About = About {
    id: "tigervnc",
    name: "VNC (TigerVNC)",
    short_name: "TigerVNC",
    description: "Fastest implementation, dropping the fewest frames, but using 2-5x the CPU usage. This the only VNC backend which supports H.264 encoding, which is the most bandwidth-efficient for large screen updates, where supported by the server. It also supports QEMU audio.",
    // After libvncclient, which came first.
    order: 17,
    pinning: Pinning::ADDRESS,
    user_has_domain: false,
    options: OPTIONS,
};

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
pub static OPTIONS: &[Spec] = &[
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
    // way — so against anything else this row changes nothing at all. Where
    // a server has it this is where the session's Enable switch starts: the
    // extension has a disable and an enable, so the sound can be asked for
    // later in a session that began without it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::Options;

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
                "Audio",
                "ViewOnly",
                "Shared",
                "AnonymousTLS",
                "H264",
                "AcceptBell"
            ]
        );
    }

    #[test]
    fn h264_falls_back_to_the_default_once_its_switch_is_off() {
        let h264 = OPTIONS[0].choices.iter().find(|c| c.value == "h264").unwrap();
        assert_eq!(h264.requires, Some("H264"));
        let mut options = Options::defaults(OPTIONS);
        options.set("H264", "true");
        options.set("Encoding", "h264");
        assert!(options.settle(OPTIONS).is_empty(), "its switch is on");
        assert_eq!(options.check_requirements(OPTIONS), Ok(()));
        options.set("H264", "false");
        assert_eq!(options.check_requirements(OPTIONS), Err("Encoding=h264 needs H264=true".to_string()));
        assert_eq!(options.settle(OPTIONS), ["Encoding"]);
        assert_eq!(options.get("Encoding"), "tight");
        assert!(!options.is_set("Encoding"), "a fallback is nobody's choice");
        assert!(options.settle(OPTIONS).is_empty(), "and settled stays settled");
        // Any other choice stands without the switch.
        options.set("Encoding", "zrle");
        assert!(options.settle(OPTIONS).is_empty());
        assert_eq!(options.get("Encoding"), "zrle");
    }
}
