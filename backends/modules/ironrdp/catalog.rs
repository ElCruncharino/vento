// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The IronRDP backend's entry in the catalog: what it is called, where it
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

pub static IRONRDP: About = About {
    id: "rdp",
    name: "RDP (IronRDP)",
    short_name: "IronRDP",
    description: "Rust RDP implementation. Uses less CPU than FreeRDP, but drops more frames and does not properly support EGFX",
    order: 25,
    pinning: Pinning::ADDRESS,
    // The prompt has no box for a domain, so it starts with it in the
    // name, as Windows shows it.
    user_has_domain: true,
    saves_identity: false,
    options: OPTIONS,
};

/// What the IronRDP backend can be told.
///
/// Shorter than [RFB's](crate::catalog::RFB), and shorter for a different
/// reason. RFB's list is short because the protocol has little to say; this
/// one is short because almost everything RDP negotiates is negotiated *well*
/// — the codec, the compression, the colour depth — and an option that
/// re-decides what the two ends already agreed on is a way to make a session
/// worse.
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
    // FreeRDP's key, words and default, less the remote machine: leaving the
    // sound there is a flag in the client info that IronRDP does not let a
    // client set, and "Off" is the server playing it nowhere.
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

    #[test]
    fn its_prompt_starts_with_the_domain_and_its_pins_are_the_address() {
        assert_eq!(prompt_user(&IRONRDP, "proto@corp.example"), "corp.example\\proto");
        let pinned = pinned_as(&IRONRDP, "Host:3389", &Options::defaults(OPTIONS));
        assert_eq!(pinned.key, "Host:3389");
        assert!(!pinned.first_sight_silent);
    }
}
