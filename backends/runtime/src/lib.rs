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
//! A protocol that reaches TLS reaches it through [`tls`]: the certificate
//! pinned rather than verified, and one connection read and written by two
//! threads.
//!
//! The picture and the sound a session carries are [`media`]'s: the video
//! decoders and sound outputs a platform supplies by implementing its two
//! traits ([`media::video::Decoders`], [`media::audio::Outputs`]), and what is
//! done with their planes and samples, the same for every protocol. It and the
//! conversation above use nothing of each other.
//!
//! What is not here: anything with `jni` in it, which is
//! `remotedesktop-platform-android`.

pub mod address;
pub mod client;
pub mod cursor;
pub mod events;
pub mod handler;
pub mod hangup;
pub mod media;
pub mod session;
pub mod slot;
pub mod text;
pub mod tls;

pub use client::Client;
pub use events::Events;
pub use handler::{Credentials, Handler};
pub use hangup::Hangup;
pub use session::{Bridge, Session};
pub use slot::Slot;

/// The most pixels a server may say its desktop has, for a backend whose
/// framebuffer is its own to allocate (RFB's and SPICE's). Not a judgement
/// about what a client can draw — this is three times the largest desktop
/// measured against, with an 8K screen under it — but about what a pair of
/// fields off the wire may cost before the claim is treated as a protocol
/// error: a framebuffer is four bytes a pixel, 128 MiB at this bound, and is
/// allocated the moment the size arrives.
pub const MAX_DESKTOP_PIXELS: usize = 32 << 20;

/// The widest and tallest cursor a backend takes from the far end, which is
/// libvncclient's bound (TigerVNC's viewer holds to 256). A cursor's size is
/// two fields of the server's with no desktop to be checked against, and its
/// pixels are allocated by them, so past this the shape is refused before
/// anything is: at the bound it is 4 MiB, where two 16-bit fields would allow
/// 17 GB. SPICE holds its own to a tighter 256, the largest a guest draws.
pub const MAX_CURSOR_SIDE: usize = 1024;

/// The most of the far end's clipboard a backend takes, in bytes of text as
/// it arrives (after any compression the protocol puts over it, and as the
/// UTF-8 it becomes where the protocol's text is UTF-16, as RDP's is). A
/// clipboard past it is skipped rather than cut short, since a part of a text
/// pasted here reads as the whole of it, and rather than refused, since a
/// large copy at the far end is somebody's ordinary afternoon and not a reason
/// to end the session. The two RDP backends hold what they offer the far end
/// to it too, and FreeRDP's C has it as `VENTO_MAX_CLIPBOARD` (`vento/text.h`),
/// which `api` holds equal to this.
pub const MAX_CLIPBOARD: usize = 1 << 20;
