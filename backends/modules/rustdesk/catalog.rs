// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The RustDesk backend's entry in the catalog: what it is called, where it
//! goes in a picker, how its identities are pinned, and every option it
//! takes. Compiled by `remotedesktop-api`, which names this file by path
//! (`backends/api/src/catalog.rs`), and not by this module's crate: the
//! Linux client's picker and the phone's generated Java (`vento-codegen`)
//! read it without building the backend.

use crate::catalog::About;
use crate::options::{Scope, Spec, VIEW_ONLY, choice, described, choices, bool, text, when};
use crate::trust::{Network, Pinning};

/// The rendezvous server an empty `RendezvousServer` means, which names the
/// public network in a pin's key.
const PUBLIC_NETWORK: &str = "rs-ny.rustdesk.com";

pub static RUSTDESK: About = About {
    id: "rustdesk",
    name: "RustDesk",
    short_name: "RustDesk",
    description: "RustDesk backend supporting direct or rendezvous connections. Uses hardware-accelerated video codecs.",
    // Last: a third protocol, and the only one that arrives in an add-on on
    // the phone.
    order: 40,
    // The id and the network it means something on. An id is handed out
    // by a rendezvous server and re-issued when a machine's configuration
    // is wiped, so the same digits on two networks are two machines, and
    // one entry under the bare id would have them overwriting each other's
    // identity. A first key is taken without asking, since it arrives
    // signed by the rendezvous server for the id that was dialled and a
    // question would have nothing to check it by.
    pinning: Pinning {
        network: Some(Network {
            option: "RendezvousServer",
            default: PUBLIC_NETWORK,
        }),
        first_sight_silent: true,
    },
    user_has_domain: false,
    saves_identity: false,
    options: OPTIONS,
};

/// What the RustDesk backend can be told.
///
/// A short list, and the reason is the protocol rather than restraint: no
/// security to choose, since each way to a machine has exactly one answer to
/// that, and no colour depth, since the picture is a video stream. The codec
/// is the nearest thing to an encoding here and behaves nothing like one — it
/// is a preference between whole video codecs, answered by whichever of them
/// that machine has an encoder for.
///
/// The first three rows are how a machine is reached, and they are here rather
/// than beside the address because only one protocol here has more than one
/// answer to it. The server and its key are [`Scope::Layered`]: somebody
/// running their own rendezvous server has every machine on it, so the answer
/// belongs to the backend with a connection able to disagree.
///
/// What is **not** here and could have been: the peer's other displays. This
/// protocol captures one at a time and switches with a message, so a chooser
/// is a row whose contents depend on the connection that is running — a live
/// fact rather than a static description, and it is in the connection panel
/// with the desktop size, which is there for the same reason.
pub static OPTIONS: &[Spec] = &[
    // The first row of the connection, because it says what the box above it
    // holds and because the two are not one transport with a switch on it: one
    // reaches a machine somebody can already address and encrypts nothing, and
    // the other reaches a machine nothing on this network can address and is
    // encrypted because of how it got there.
    choices(
        "ConnectBy",
        "Connect by",
        None,
        "id",
        Scope::Connection,
        false,
        &[
            described("id", "ID", "Connect by ID via a rendezvous server. Encrypted."),
            described("address", "Address", "Connect to a machine by IP. Not encrypted."),
        ],
    ),
    when(
        text(
            "RendezvousServer",
            "Rendezvous server",
            "Leave empty for the default public rendezvous server.",
            "",
            Scope::Layered,
            false,
        ),
        "ConnectBy",
        &["id"],
    ),
    when(
        text(
            "RendezvousKey",
            "Rendezvous server key",
            "Public key for the rendezvous server.",
            "",
            Scope::Layered,
            false,
        ),
        "ConnectBy",
        &["id"],
    ),
    // Both of these are live in the way RFB's encoding is: their option
    // message is read on the next frame the peer encodes.
    choices(
        "Quality",
        "Quality",
        Some("Control the video bitrate."),
        "balanced",
        Scope::Layered,
        true,
        &[
            choice("low", "Low"),
            choice("balanced", "Balanced"),
            choice("best", "Best"),
        ],
    ),
    choices(
        "FrameRate",
        "Frame rate",
        Some("Limits the maximum FPS."),
        "auto",
        Scope::Layered,
        true,
        &[
            choice("auto", "Automatic"),
            choice("10", "10"),
            choice("20", "20"),
            choice("30", "30"),
            choice("60", "60"),
        ],
    ),
    // Live like the two above it, and the one that is not free: the far end
    // answers a codec it is not already using by building an encoder and
    // starting again with a key frame.
    choices(
        "Codec",
        "Codec",
        Some("Preferred codec, if supported by the server."),
        "auto",
        Scope::Layered,
        true,
        &[
            choice("auto", "Automatic"),
            choice("vp9", "VP9"),
            choice("vp8", "VP8"),
            choice("av1", "AV1"),
            choice("h264", "H.264"),
            choice("h265", "H.265"),
        ],
    ),
    // Their own client's switch, on as theirs is: the peer sends Opus while
    // its desktop makes a sound, and nothing once it has been silent for
    // eight seconds. What the login asks, and where the session's Enable
    // switch starts, which asks again with their option message. "The remote
    // machine" rather than "the server", which here is the rendezvous one.
    bool(
        "Audio",
        "Sound",
        "Play the remote machine's audio.",
        true,
        Scope::Layered,
        false,
    ),
    bool(
        "LockAfterSession",
        "Lock when the session ends",
        "Lock the remote screen on disconnecting.",
        false,
        Scope::Layered,
        false,
    ),
    VIEW_ONLY,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{Options, find};
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
                "ConnectBy",
                "RendezvousServer",
                "RendezvousKey",
                "Quality",
                "FrameRate",
                "Codec",
                "Audio",
                "LockAfterSession",
                "ViewOnly"
            ]
        );
    }

    #[test]
    fn the_rendezvous_rows_are_for_an_id_and_name_the_pins_network() {
        assert_eq!(find(OPTIONS, "RendezvousKey").unwrap().gate.unwrap().values, ["id"]);
        assert!(find(OPTIONS, "RendezvousServer").unwrap().check("anything").is_ok());
        let mut o = Options::defaults(OPTIONS);
        let id = pinned_as(&RUSTDESK, "123456789", &o);
        assert_eq!(id.key, "123456789@rs-ny.rustdesk.com");
        assert!(id.first_sight_silent);
        o.set("RendezvousServer", " hbbs.example ");
        assert_eq!(pinned_as(&RUSTDESK, "123456789", &o).key, "123456789@hbbs.example");
    }
}
