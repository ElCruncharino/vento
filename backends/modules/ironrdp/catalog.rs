// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The IronRDP backend's entry in the catalog: its name, its place in a
//! picker, how its identities are pinned, and all its options. Compiled by
//! `remotedesktop-api`, which includes this file by path
//! (`backends/api/src/catalog.rs`), and not by this module's crate, so that
//! the Linux client's picker and the phone's generated Java (`vento-codegen`)
//! can read it without building the backend.

use crate::catalog::About;
use crate::options::{
    Scope, Spec, Choice, VIEW_ONLY, DESKTOP_SIZES, MONITOR_COUNTS, choice, choices, bool,
};
use crate::trust::Pinning;

pub static IRONRDP: About = About {
    id: "rdp",
    name: "RDP (IronRDP)",
    short_name: "IronRDP",
    description: "Rust RDP implementation. Uses less CPU than FreeRDP, but drops more frames",
    order: 25,
    pinning: Pinning::ADDRESS,
    // The prompt has no domain field, so it starts with the domain in the
    // name, as Windows shows it.
    user_has_domain: true,
    saves_identity: false,
    options: OPTIONS,
};

/// The IronRDP backend's options.
///
/// Shorter than [RFB's](crate::catalog::RFB), for a different reason. RFB's
/// list is short because the protocol has few settings; this one is short
/// because RDP negotiates almost everything well (the codec, the
/// compression, the colour depth), and an option that overrides what the
/// two ends already agreed on can only make a session worse.
///
/// Four things are deliberately **not** here:
///
/// - **The keyboard layout.** RDP sends key *positions*, and this crate's
///   `keymap` maps keysyms onto a US layout, so the layout named at connect
///   time has to be the one that table was written for. Offering a choice
///   would allow a layout that turns every letter into a different letter.
/// - **The clipboard.** The channel is open in both directions, but whether
///   to share the local clipboard is a client setting, configured once for
///   every protocol in the client's own settings.
/// - **The domain**, RDP's third credential, which is not a row because it
///   is already part of a field: `DOMAIN\user` and `user@domain` are split
///   out of the user name, the form every RDP client accepts and RDP users
///   type.
/// - **The colour depth**, which three of the VNC backends offer. There it
///   is a wire format the server encodes into; here the picture arrives as
///   RemoteFX or as bitmaps, and the codec has already decided what a pixel
///   costs. 16-bit would apply to only one of those paths, save nothing
///   where the codec is on, and add a conversion in front of a framebuffer
///   that is 32 bits either way.
///
/// The graphics pipeline (MS-RDPEGFX), which Windows 8 and later prefer for
/// sending a desktop and the only path that carries H.264, is the Graphics
/// row's third choice and not its default: RemoteFX is the path every
/// server here has always drawn correctly over, and the pipeline, with its
/// H.264 decoded by the app's own video decoders (the crate's `pipeline`),
/// has been tested against xrdp and Windows 10 but for less long.
pub static OPTIONS: &[Spec] = &[
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
        Some("EGFX is preferred by Windows and supports H.264, RemoteFX is older, and Bitmaps sends raw images."),
        "rfx",
        Scope::Layered,
        false,
        &[
            choice("egfx", "EGFX (experimental)"),
            choice("rfx", "RemoteFX"),
            choice("bitmap", "Bitmaps"),
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
            choice("full", "Everything"),
            choice("balanced", "No animation"),
            choice("plain", "As little as possible"),
        ],
    ),
    // FreeRDP's key, text and default, without "The remote machine": leaving
    // the sound there is a flag in the client info that IronRDP does not let
    // a client set, and "Off" means the server plays it nowhere.
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
    use crate::options::Options;
    use crate::trust::pinned_as;

    /// The keys the phone's lists had when they moved here, in the same
    /// order: keys are stored in connection records and preference files,
    /// so renaming one loses the setting.
    #[test]
    fn the_keys_are_the_ones_records_hold() {
        let keys: Vec<&str> = OPTIONS.iter().map(|s| s.key).collect();
        assert_eq!(
            keys,
            [
                "Nla",
                "DesktopSize",
                "Monitors",
                "Graphics",
                "Experience",
                "Sound",
                "Compression",
                "ViewOnly"
            ]
        );
    }

    /// The graphics default stays RemoteFX: EGFX is offered but not the
    /// default, so an existing connection's picture does not change.
    #[test]
    fn the_graphics_default_is_remotefx() {
        let graphics = OPTIONS.iter().find(|s| s.key == "Graphics").unwrap();
        assert_eq!(graphics.default, "rfx");
        let values: Vec<&str> = graphics.choices.iter().map(|c| c.value).collect();
        assert_eq!(values, ["egfx", "rfx", "bitmap"]);
    }

    #[test]
    fn its_prompt_starts_with_the_domain_and_its_pins_are_the_address() {
        assert_eq!(prompt_user(&IRONRDP, "proto@corp.example"), "corp.example\\proto");
        assert_eq!(prompt_user(&IRONRDP, "proto@hotmail.com"), "MicrosoftAccount\\proto@hotmail.com");
        let pinned = pinned_as(&IRONRDP, "Host:3389", &Options::defaults(OPTIONS));
        assert_eq!(pinned.key, "Host:3389");
        assert!(!pinned.first_sight_silent);
    }
}
