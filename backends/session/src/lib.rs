// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! One session on any backend, the way every client holds one.
//!
//! [`backend::Backend`] is the seam: what a window or a phone calls, whatever
//! protocol is underneath. A session says what happens through a
//! [`events::Mailbox`], as [`events::Event`]s. [`catalog`] is what each
//! backend is called and can be told — each entry its backend's own file
//! under `backends/modules/`, compiled here — and [`options`] the shape of
//! what it can be told; [`kind::Kind`] puts an entry beside a backend's
//! constructor, and [`kind::Connect`] is what a session is started with.
//! [`credentials`] and [`trust`] are the rules every client keeps alike —
//! how a prompt's answer is read, and how a far end's identity meets the
//! pins — applied here for the phone and the Linux client both, and held to
//! their cases by `backends/vectors/`.
//!
//! The adapters themselves are each in its backend's own crate: the protocol
//! crate for RFB, IronRDP, RustDesk and SPICE, and a small crate beside the C
//! library for libvncclient, TigerVNC and FreeRDP, which fill in [`c`] — the
//! one adapter over `vento/backend.h`'s vtable, behind the `c` feature.
//!
//! This was the Linux client's own `src/`, until the phone wanted the same
//! thing: nothing in it knows a platform, and what a session needs of one —
//! where to keep things, what decodes video, what this machine is called —
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

pub use common::Credentials;
