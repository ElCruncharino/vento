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
    // The sentence that writes itself: the others show you a desktop, and
    // this one shows you a machine.
    description: "Usually used for QEMU/libvirt/Proxmox virtual machines. Supports clipboard and screen resizing if the guest agent is running.",
    // After the RDP clients and before the one that arrives in an add-on on
    // the phone.
    order: 35,
    pinning: Pinning::ADDRESS,
    user_has_domain: false,
    saves_identity: false,
    options: OPTIONS,
};

/// What the SPICE backend can be told.
///
/// A short list, and the reason is the protocol rather than restraint. There
/// is no security to negotiate — a plain port and a TLS port are different
/// ports, which is why the first row is a fact about the address rather than a
/// preference. There is no colour depth, since a surface is 32-bit and the
/// server compresses it. And there is no resize and no clipboard switch,
/// because both of those live in the guest's own agent rather than in the
/// protocol.
///
/// What is **not** here and could have been: which end owns the pointer. SPICE
/// has two mouse modes and lets a client ask for one, but which are on offer
/// is the far end's answer — a guest with a tablet attached offers both and
/// one without offers neither — so it is a fact about a live session and is
/// announced through the seam rather than chosen in a form.
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
    // Live in the way RFB's encoding is: the message is read on the next image
    // the server encodes. Automatic is what a hypervisor does without being
    // asked, which for QEMU is GLZ where it helps.
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
    // The playback channel, TigerVNC's key and words for QEMU's other way of
    // sending a guest's sound: on unless turned off, since a guest that
    // plays nothing sends nothing. Where the session's Enable switch starts:
    // the channel is opened and closed with the switch.
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

    /// The keys it had when the phone's lists became these, in the same
    /// order: a key is stored in connection records and preference files,
    /// so one renamed is a setting forgotten.
    #[test]
    fn the_keys_are_the_ones_records_hold() {
        let keys: Vec<&str> = OPTIONS.iter().map(|s| s.key).collect();
        assert_eq!(keys, ["Security", "Compression", "Audio", "ViewOnly"]);
    }
}
