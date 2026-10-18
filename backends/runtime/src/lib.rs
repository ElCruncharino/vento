// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The neutral layer: what a protocol client is written against, and what a
//! platform is written to, with neither knowing the other.
//!
//! A protocol crate implements [`Client`] and calls a [`Handler`]. A platform
//! implements [`Callbacks`] — the outward, non-blocking half of the same
//! conversation — and gets a [`Runner`] that runs the client on a thread of
//! its own, with a [`Bridge`] in between that turns the two questions a
//! protocol can ask (credentials, trust) into an event and a wait on a
//! [`Slot`]. The shape of `Callbacks` is the shape of the C API's
//! `vento_callbacks` and of the Java seam's `NativeBackend` `on…` methods:
//! that is the one place this crate is opinionated, because it is the one
//! seam every platform has to fit.
//!
//! A protocol that reaches TLS reaches it through [`tls`]: the certificate
//! pinned rather than verified, and one connection read and written by two
//! threads.
//!
//! What is not here: the picture and the sound, which are
//! `remotedesktop-media`'s (`media::video`, `media::audio`) and need nothing
//! of this crate; and anything with `jni` in it, which is
//! `remotedesktop-platform-android`.

pub mod address;
pub mod callbacks;
pub mod client;
pub mod cursor;
pub mod handler;
pub mod hangup;
pub mod runner;
pub mod slot;
pub mod tls;

pub use callbacks::Callbacks;
pub use client::Client;
pub use handler::{Credentials, Handler};
pub use hangup::Hangup;
pub use runner::{Bridge, Runner};
pub use slot::Slot;

/// The most pixels a server may say its desktop has, for a backend whose
/// framebuffer is its own to allocate (RFB's and SPICE's). Not a judgement
/// about what a client can draw — the Linux client tiles a desktop past its
/// GPU's texture limit, and this is three times the largest desktop measured
/// against, with an 8K screen under it — but about what a pair of fields off
/// the wire may cost before the claim is treated as a protocol error: a
/// framebuffer is four bytes a pixel, 128 MiB at this bound, and is allocated
/// the moment the size arrives.
pub const MAX_DESKTOP_PIXELS: usize = 32 << 20;
