// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A RustDesk client, written rather than bound to.
//!
//! There was nothing to bind to: their client is `src/client.rs` at 4443 lines
//! plus an io loop of 2521, wired into their config store, their UI interface
//! and their rendezvous mediator. What can be taken is `hbb_common` — the
//! message definitions and the framing — and the client written above it, which
//! is what this is.
//!
//! It is the first protocol here that is somebody's product rather than a
//! published specification: one implementation, no second opinion, and a wire
//! that changes when they change it. Everything in here was read against
//! **rustdesk 1.4.9** and driven against a peer of that version.
//!
//! What it speaks:
//!
//! | | |
//! |---|---|
//! | Transport | a peer by id, punched or relayed and encrypted; or direct IP access, which is **plaintext in both directions** |
//! | Login | their permanent password, `sha256(sha256(password ++ salt) ++ challenge)` |
//! | Picture | VP9, VP8, AV1, H.264 or H.265 as the platform can decode them; whole frames |
//! | Cursor | shape and hotspot, by id |
//! | Input | absolute pointer, five buttons and a wheel; keys as their named set or as characters |
//! | Clipboard | both ways, text |
//! | Live | image quality, frame rate, and the desktop's size out of the list the peer offers |
//!
//! What it does not: audio, file transfer, the terminal, tunnelling, 2FA,
//! elevation, privacy mode and virtual displays, which are the product rather
//! than the desktop. Nor KCP, their newer UDP transport, which nothing has to
//! offer for a session to be reached.
//!
//! Nothing in here decodes video. The client is made with a
//! `common::video::Decoders` — the phone's MediaCodec in the app, libavcodec in
//! the probe, nothing at all in a test — and what it tells the far end it can
//! take is whatever that says. Which codecs a session ends up in is the far
//! end's choice out of that list, so the same protocol code is a VP9 session on
//! one platform and an H.265 one on another.

// The protocol as the clients hold it: a `session::Backend`, with this
// backend's options read into its config and its rows — the window's on
// Linux, and the phone's through the one JNI layer below.
pub mod backend;

// The library the add-on carries: this backend, registered on the module's
// generated natives class, with the phone's MediaCodec for the picture.
// Android only — everything else is the protocol, and it builds and tests on
// the host with none of this compiled.
#[cfg(target_os = "android")]
platform_android::natives!(
    class = c"net/pgaskin/remotedesktop/backend/rustdesk/RustDeskNatives",
    kind = backend::KIND,
    decoders = || Some(std::sync::Arc::new(platform_android::mediacodec::MediaCodec)),
);

mod client;
pub mod crypto;
mod error;
mod framebuffer;
mod keymap;
pub mod protos;
pub mod rendezvous;
pub mod wire;

pub use client::{Client, Config, Info, Platform, PreferredCodec, Quality, Reach, VERSION};
pub use common::video::Codec;
pub use common::{Credentials, Handler};
pub use error::{Error, Result};
pub use framebuffer::Framebuffer;
