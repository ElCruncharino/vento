// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A RustDesk client, written from scratch rather than wrapping upstream's.
//!
//! Upstream's client cannot be used as a library: it is `src/client.rs` at 4443
//! lines plus an io loop of 2521, tied to RustDesk's config store, UI interface
//! and rendezvous mediator. Only the message definitions and hbb_common's
//! framing can be reused, and this is a client written on top of the
//! definitions (the two `.proto` files `build.rs` compiles).
//!
//! It is the first protocol here defined by a product rather than a published
//! specification: one implementation, nothing to compare it with, and a wire
//! format that changes whenever upstream changes it. Everything here was
//! written against **rustdesk 1.4.9** and tested against a peer of that
//! version, except key exchange version 1 (`crypto.rs`), which is 1.5.0's and
//! was tested against a peer of each.
//!
//! What it supports:
//!
//! | | |
//! |---|---|
//! | Transport | a peer by id, punched or relayed and encrypted; or direct IP access, which is **plaintext in both directions** |
//! | Login | the peer's permanent password, `sha256(sha256(password ++ salt) ++ challenge)` |
//! | Picture | VP9, VP8, AV1, H.264 or H.265 as the platform can decode them; whole frames |
//! | Cursor | shape and hotspot, by id |
//! | Input | absolute pointer, five buttons and a wheel; keys as RustDesk's named keys or as characters |
//! | Clipboard | both ways, text |
//! | Sound | the peer's, Opus, where the platform decodes it |
//! | Live | image quality, frame rate, and the desktop's size out of the list the peer offers |
//!
//! Not supported: the microphone, file transfer, the terminal, tunnelling, 2FA,
//! elevation, privacy mode and virtual displays, which are product features
//! rather than remote desktop. Nor KCP, RustDesk's newer UDP transport, which
//! is never required to reach a peer.
//!
//! Nothing in here decodes video. The client is created with a
//! `runtime::media::video::Decoders` (the phone's MediaCodec in the app,
//! libavcodec in the probe, none in a test), and it tells the far end it can
//! decode whatever that reports. The far end picks the codec from that list,
//! so the same protocol code runs a VP9 session on one platform and an H.265
//! one on another.

// The protocol as an `api::Backend`, with this backend's options read into
// its config, and its info rows. Used by the Linux client's window, and on the
// phone through the JNI layer below.
pub mod backend;

// The library the add-on carries: this backend, registered on the module's
// generated natives class, with the phone's MediaCodec for video and AAudio
// for sound (whose Opus is also decoded by MediaCodec, through it). Android
// only: the rest is the protocol, which builds and tests on the host without
// this.
#[cfg(target_os = "android")]
platform_android::natives!(
    class = c"net/pgaskin/remotedesktop/backend/rustdesk/RustDeskNatives",
    kind = backend::KIND,
    decoders = || Some(std::sync::Arc::new(platform_android::mediacodec::MediaCodec)),
    audio = || Some(std::sync::Arc::new(platform_android::aaudio::AAudio::new())),
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
pub use runtime::media::video::Codec;
pub use runtime::{Credentials, Handler};
pub use error::{Error, Result};
pub use framebuffer::Framebuffer;
