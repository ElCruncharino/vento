// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The FreeRDP backend's entry in the catalog: what it is called, where it
//! goes in a picker, how its identities are pinned, and every option it
//! takes. Compiled by `remotedesktop-api`, which names this file by path
//! (`backends/api/src/catalog.rs`), and not by this module's crate: the
//! phone's generated Java (`vento-codegen`) reads it without building the
//! backend.

use crate::catalog::About;
use crate::options::{
    Scope, Spec, Choice, VIEW_ONLY, DESKTOP_SIZES, MONITOR_COUNTS, choice, choices, bool,
};
use crate::trust::Pinning;

pub static FREERDP: About = About {
    id: "freerdp",
    name: "RDP (FreeRDP)",
    short_name: "FreeRDP",
    description: "The most robust RDP implementation. Support EGFX with hardware-accelerated H.264 video, and supports audio. This is the best one to use with a Windows RDP server.",
    // Ahead of the other RDP client: it is the one that reaches a Windows
    // desktop, and the one whose picture can be a video.
    order: 20,
    pinning: Pinning::ADDRESS,
    // The prompt has no box for a domain, so it starts with it in the
    // name, as Windows shows it.
    user_has_domain: true,
    options: OPTIONS,
};

/// What the FreeRDP backend can be told.
///
/// It began as the other RDP backend's rows copied so the two would be
/// comparable, and two of these are things that client cannot be asked at
/// all: **EGFX**, the graphics pipeline, which is how a machine since Windows
/// 8 would rather send a desktop and the only path that carries H.264; and the
/// **interface size**, which is the only answer the phone has to a desktop
/// whose text is sized for a monitor and is being read on it. The **sound**
/// began here too, and the other has it now, all but the remote machine.
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
pub static OPTIONS: &[Spec] = &[
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::prompt_user;
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
    }

    #[test]
    fn its_prompt_starts_with_the_domain_and_its_sound_is_this_machine() {
        assert_eq!(prompt_user(&FREERDP, "CORP\\proto"), "CORP\\proto");
        assert_eq!(prompt_user(&FREERDP, " proto "), "proto");
        let sound = find(OPTIONS, "Sound").unwrap();
        assert_eq!(sound.label_for("local", Platform::Android), "This phone");
        assert_eq!(sound.label_for("local", Platform::Linux), "This computer");
    }
}
