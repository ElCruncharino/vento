// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What each of the eight backends is, before anything is connected: its
//! name, why somebody would pick it, where it goes in a picker, and what it
//! can be told. One list for every client — the Linux client's picker reads
//! it, and each of the phone's `BackendProvider`s answers from a class
//! `vento-codegen` writes out of it — so that a backend is described once.

use crate::options::{self, Spec};

/// One backend, described.
#[derive(Debug, PartialEq, Eq)]
pub struct About {
    /// What a connection record stores and a command line names it by, so it
    /// must not change once shipped.
    pub id: &'static str,
    /// For a picker and a title.
    pub name: &'static str,
    /// A word or two, for a badge with no room for more. Two backends may
    /// speak one protocol, so this sometimes names the implementation.
    pub short_name: &'static str,
    /// Why somebody would choose this one, in a sentence or two: what it does
    /// that the others do not, and what that costs.
    pub description: &'static str,
    /// Where in a picker, lowest first.
    pub order: i32,
    pub options: &'static [Spec],
}

pub static RFB: About = About {
    id: "rfb",
    // Named for what it is rather than for being the default one: there is
    // more than one VNC client here, and which implementation a connection
    // is on is the only thing these names have to tell apart.
    name: "VNC (Rust)",
    short_name: "Rust",
    description: "Rust RFB implementation. Does not support Tight encoding, and typically uses more bandwith and performs worse on slow networks. Allows remote pointer position updates, where supported by the server.",
    // First in a picker: it is ours, and it is the one that will still be
    // here.
    order: 10,
    options: options::RFB,
};

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
    options: options::LIBVNC,
};

pub static TIGERVNC: About = About {
    id: "tigervnc",
    name: "VNC (TigerVNC)",
    short_name: "TigerVNC",
    description: "Fastest implementation, dropping the fewest frames, but using 2-5x the CPU usage. This the only VNC backend which supports H.264 encoding, which is the most bandwidth-efficient for large screen updates, where supported by the server. It also supports QEMU audio.",
    // After libvncclient, which came first.
    order: 17,
    options: options::TIGERVNC,
};

pub static FREERDP: About = About {
    id: "freerdp",
    name: "RDP (FreeRDP)",
    short_name: "FreeRDP",
    description: "The most robust RDP implementation. Support EGFX with hardware-accelerated H.264 video, and supports audio. This is the best one to use with a Windows RDP server.",
    // Ahead of the other RDP client: it is the one that reaches a Windows
    // desktop, and the one whose picture can be a video.
    order: 20,
    options: options::FREERDP,
};

pub static IRONRDP: About = About {
    id: "rdp",
    name: "RDP (IronRDP)",
    short_name: "IronRDP",
    description: "Rust RDP implementation. Uses less CPU than FreeRDP, but drops more frames and does not properly support EGFX",
    order: 25,
    options: options::IRONRDP,
};

pub static REALVNC: About = About {
    id: "realvnc",
    name: "VNC (RealVNC)",
    // Three of the backends speak RFB, so a badge has to name the
    // implementation as well as the protocol.
    short_name: "RealVNC",
    description: "RealVNC's proprietary client implementation. This is the only one which supports UDP, ZRLE2, and RealVNC's proprietary authentication/encryption. It performs significantly better than the others when connecting to a RealVNC server, especially for typical desktop usage.",
    // After the open VNC and RDP clients: the more capable client against
    // its own server, and the one with a ceiling on its life — 4.9.3's
    // `libvncviewer.so` is 16 KB aligned, but it is somebody else's binary
    // and cannot be rebuilt when the next page size arrives.
    order: 30,
    options: options::REALVNC,
};

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
    options: options::SPICE,
};

pub static RUSTDESK: About = About {
    id: "rustdesk",
    name: "RustDesk",
    short_name: "RustDesk",
    description: "RustDesk backend supporting direct or rendezvous connections. Uses hardware-accelerated video codecs.",
    // Last: a third protocol, and the only one that arrives in an add-on on
    // the phone.
    order: 40,
    options: options::RUSTDESK,
};

/// All eight, in the picker's order.
pub static ALL: [&About; 8] = [&RFB, &LIBVNC, &TIGERVNC, &FREERDP, &IRONRDP, &REALVNC, &SPICE, &RUSTDESK];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::Options;

    #[test]
    fn every_backend_has_a_distinct_id_and_place_and_defaults_that_pass_its_own_checks() {
        for (i, about) in ALL.iter().enumerate() {
            for other in &ALL[i + 1..] {
                assert_ne!(about.id, other.id);
                assert!(about.order < other.order, "{} before {}", about.id, other.id);
            }
            let options = Options::defaults(about.options);
            for spec in about.options {
                assert!(spec.check(options.get(spec.key)).is_ok(), "{}/{}", about.id, spec.key);
            }
        }
    }
}
