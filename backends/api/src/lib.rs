// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The session API shared by all backends and clients.
//!
//! [`backend::Backend`] is the interface the Linux client and the phone
//! call, whatever the protocol. A session reports what happens through a
//! [`events::Mailbox`], as [`events::Event`]s. [`catalog`] holds each
//! backend's name and options (each entry is its backend's own file under
//! `backends/modules/`, compiled here), and [`options`] the shape of those
//! options; [`kind::Kind`] pairs an entry with a backend's constructor, and
//! [`kind::Connect`] holds a session's parameters. [`credentials`] and
//! [`trust`] are rules every client applies the same way (reading the
//! answer to a prompt, and checking a far end's identity against the pins),
//! shared by the phone and the Linux client and tested against the cases in
//! `tests/vectors.rs`.
//!
//! Each adapter is in its backend's own crate: the protocol crate for RFB,
//! IronRDP, RustDesk and SPICE, and a small crate beside the C library for
//! libvncclient, TigerVNC, FreeRDP and RealVNC, which fill in [`c`], the
//! adapter over the vtable in `vento/backend.h`, behind the `c` feature.
//!
//! Nothing here is platform-specific: what a session needs from the
//! platform (storage directories, video decoders, this machine's name)
//! arrives through [`kind::Connect`].

pub mod backend;
#[cfg(feature = "c")]
pub mod c;
pub mod catalog;
pub mod credentials;
pub mod events;
pub mod kind;
pub mod options;
pub mod rdp;
pub mod trust;

pub use runtime::Credentials;
