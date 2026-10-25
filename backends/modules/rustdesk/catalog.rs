// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The RustDesk backend's catalog entry: its name, its position in a picker,
//! how its identities are pinned, and its options. Compiled by
//! `remotedesktop-api`, which includes this file by path
//! (`backends/api/src/catalog.rs`), and not by this module's crate, so the
//! Linux client's picker and the phone's generated Java (`vento-codegen`)
//! can read it without building the backend.

use crate::catalog::About;
use crate::options::{Scope, Spec, VIEW_ONLY, choice, described, choices, bool, text, when};
use crate::trust::{Network, Pinning};

/// The rendezvous server used when `RendezvousServer` is empty; it names the
/// public network in a pin's key.
const PUBLIC_NETWORK: &str = "rs-ny.rustdesk.com";

pub static RUSTDESK: About = About {
    id: "rustdesk",
    name: "RustDesk",
    short_name: "RustDesk",
    description: "RustDesk backend supporting direct or rendezvous connections. Uses hardware-accelerated video codecs.",
    // Last, after the VNC, RDP and SPICE backends; on the phone it comes in an
    // add-on.
    order: 40,
    // Pinned by the id and its network. An id is assigned by a rendezvous
    // server and re-issued when a machine's configuration is wiped, so the
    // same digits on two networks are two machines, and one entry under the
    // bare id would let them overwrite each other's identity. A first key is
    // accepted without asking, since it arrives signed by the rendezvous
    // server for the dialled id and the user would have nothing to check it
    // against.
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

/// The RustDesk backend's options.
///
/// The list is short because of the protocol: there is no security choice,
/// since each way of reaching a machine has exactly one, and no colour depth,
/// since the picture is a video stream. The codec is the closest thing to an
/// encoding here but works differently: it is a preference between video
/// codecs, honoured only if the machine has an encoder for it.
///
/// The first three rows say how a machine is reached. They are here rather
/// than next to the address because only this protocol has more than one way.
/// The server and its key are [`Scope::Layered`]: a self-hosted rendezvous
/// server usually serves all of a user's machines, so the setting belongs to
/// the backend, with a connection able to override it.
///
/// Deliberately **not** here: the peer's other displays. This protocol
/// captures one at a time and switches with a message, so a display chooser
/// depends on the running connection. It is in the connection panel, with the
/// desktop size, for the same reason.
pub static OPTIONS: &[Spec] = &[
    // The connection's first row, because it says what the address field
    // above it holds, and because the two choices are different transports
    // rather than one with a switch: one reaches a machine that is already
    // addressable and encrypts nothing, and the other reaches a machine that
    // may not be addressable from this network, encrypted as part of the
    // rendezvous.
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
    // Both of these are live, like RFB's encoding: the option message takes
    // effect on the next frame the peer encodes.
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
    // Live like the two above, but not free: switching to a codec the far end
    // is not already using makes it build a new encoder and restart with a key
    // frame.
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
    // The same switch as RustDesk's own client, on by default as there: the
    // peer sends Opus while its desktop makes sound, and nothing once it has
    // been silent for eight seconds. It sets what the login requests and the
    // initial state of the session's Enable switch, which changes it with
    // the option message. "The remote machine" rather than "the server",
    // which here would mean the rendezvous server.
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

    /// The keys the phone's lists had before they moved here, in the same
    /// order: keys are stored in connection records and preference files,
    /// so renaming one loses the setting.
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
