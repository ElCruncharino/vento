// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The SPICE backend's entry in the catalog: what it is called, where it goes
//! in a picker, how its identities are pinned, and every option it takes.
//! Compiled by `remotedesktop-api`, which names this file by path
//! (`backends/api/src/catalog.rs`), and not by this module's crate: the
//! Linux client's picker and the phone's generated Java (`vento-codegen`)
//! read it without building the backend.

use crate::catalog::About;
use crate::options::{Scope, Spec, choice, described, choices, bool};
use crate::trust::Pinning;

pub static SPICE: About = About {
    id: "spice",
    name: "SPICE",
    short_name: "SPICE",
    description: "Usually used for QEMU/libvirt/Proxmox virtual machines. Supports clipboard and screen resizing if the guest agent is running.",
    // After the RDP clients and RealVNC, and before RustDesk, which arrives
    // in an add-on on the phone.
    order: 35,
    pinning: Pinning::ADDRESS,
    user_has_domain: false,
    saves_identity: false,
    options: OPTIONS,
};

/// The SPICE backend's options.
///
/// The list is short because of the protocol. There is no security to
/// negotiate: a plain port and a TLS port are different ports, so the first
/// row is a fact about the address rather than a preference. There is no
/// colour depth, since a surface is 32-bit and the server compresses it. There
/// is no resize or clipboard switch, because both are handled by the guest
/// agent rather than the protocol.
///
/// Which end owns the pointer is **not** an option either. SPICE has two mouse
/// modes and lets a client ask for one, but the server decides which are
/// available (a guest with a tablet attached offers both, one without offers
/// only server mode), so the live session reports it (`Event::PointerMode`)
/// instead of a form setting it.
pub static OPTIONS: &[Spec] = &[
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
    // Live, like RFB's encoding: the server applies it from the next image it
    // encodes. Automatic leaves the server's default, which for QEMU is GLZ
    // where it helps.
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
    // The playback channel, with the same key and text as TigerVNC's QEMU
    // audio option. On by default, since a silent guest sends nothing. This
    // is the initial state of the session's Enable switch, which opens and
    // closes the channel.
    bool(
        "Audio",
        "Sound",
        "Play audio sent by the server, if it offers any.",
        true,
        Scope::Layered,
        false,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys from the app's option lists before they moved here, in the
    /// same order: keys are stored in connection records and preference
    /// files, so renaming one loses the setting.
    #[test]
    fn the_keys_are_the_ones_records_hold() {
        let keys: Vec<&str> = OPTIONS.iter().map(|s| s.key).collect();
        assert_eq!(keys, ["Security", "Compression", "Audio", "ViewOnly"]);
    }
}
