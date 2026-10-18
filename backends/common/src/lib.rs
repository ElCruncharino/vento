// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The neutral layer: what a protocol client is written against, and what a
//! platform is written to, with neither knowing the other.
//!
//! A protocol crate implements [`Client`] and calls a [`Handler`]. A platform
//! implements [`Events`] — the outward, non-blocking half of the same
//! conversation — and gets a [`Session`] that runs the client on a thread of
//! its own, with a [`Bridge`] in between that turns the two questions a
//! protocol can ask (credentials, trust) into an event and a wait on a
//! [`Slot`]. The shape of `Events` is the shape of the Java `Callbacks`
//! interface, and is meant to be the shape of a C ABI too: that is the one
//! place this crate is opinionated, because it is the one seam a second
//! platform has to fit.
//!
//! A protocol whose picture is video also gets a [`video::Decoders`] from
//! whoever started it, and does not know whose: the phone's hardware, or
//! libavcodec on a desktop. What is done with a decoder's planes afterwards —
//! finding what moved, and converting only that — is [`video`]'s and the same
//! for both.
//!
//! A protocol that reaches TLS reaches it through [`tls`]: the certificate
//! pinned rather than verified, and one connection read and written by two
//! threads.
//!
//! What is not here: anything with `jni` in it, which is
//! `remotedesktop-platform-android`.

pub mod address;
pub mod audio;
pub mod client;
pub mod cursor;
pub mod events;
pub mod handler;
pub mod hangup;
pub mod session;
pub mod slot;
pub mod tls;
pub mod video;

pub use client::Client;
pub use events::Events;
pub use handler::{Credentials, Handler};
pub use hangup::Hangup;
pub use session::{Bridge, Session};
pub use slot::Slot;
