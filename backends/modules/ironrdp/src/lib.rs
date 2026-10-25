// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! An RDP client built on IronRDP rather than written from scratch.
//!
//! The opposite choice from `rfb`: RFB is small enough that writing it was the
//! only way to be sure of the encodings, while RDP is too large to implement
//! again. IronRDP provides the connection sequence, the decoders and the input
//! encoding; this crate provides the structure around them (threads, the
//! framebuffer, the trust decision, the keysym map), deliberately the same as
//! `rfb::Client`'s, since both implement `Backend`.
//!
//! Supported:
//!
//! | | |
//! |---|---|
//! | Security | TLS, with or without CredSSP (NLA); never the legacy RC4 one |
//! | Graphics | the graphics pipeline (MS-RDPEGFX) with its H.264 decoded by the platform (`pipeline`), or fast-path and slow-path bitmaps, RemoteFX, NSCodec: whatever IronRDP decodes |
//! | Pointer | the server's own shape and hotspot, as a bitmap drawn locally |
//! | Input | fast-path, scancodes from a US layout with Unicode for the rest |
//! | Trust | the certificate pinned per address, as `rfb` does it (`runtime::tls::pinning`) |
//! | Clipboard | text both ways over CLIPRDR (`clipboard`) |
//! | Sound | the server's, over rdpsnd: PCM, and AAC or Opus where the platform decodes them (`sound`) |
//!
//! Not supported: **the microphone**, **drive and printer redirection**, and
//! **files on the clipboard**. The initial desktop size is requested at
//! connect time, which is RDP's own model and the reverse of RFB's; later
//! resizes go over the display control channel.

/// The protocol this client speaks, reported as the backend's version, the
/// call that proves the whole native path is wired up.
pub const PROTOCOL_VERSION: &str = "RDP 5.x (IronRDP)";

// The protocol as an `api::Backend`, with this backend's options read into
// its config and its info rows: used by the Linux window directly and by the
// phone through the JNI layer below.
pub mod backend;

// The library the app loads: this backend, registered on the module's
// generated natives class, with MediaCodec for the graphics pipeline's H.264
// and AAudio for the server's sound (and MediaCodec for its AAC, through the
// same object). Android only; everything else is the protocol, which builds,
// tests and runs an example on the host without this.
#[cfg(target_os = "android")]
platform_android::natives!(
    class = c"net/pgaskin/remotedesktop/backend/ironrdp/IronRdpNatives",
    kind = backend::KIND,
    decoders = || Some(std::sync::Arc::new(platform_android::mediacodec::MediaCodec)),
    audio = || Some(std::sync::Arc::new(platform_android::aaudio::AAudio::new())),
);

mod client;
mod clipboard;
mod error;
mod framebuffer;
mod keymap;
mod pipeline;
mod sound;
mod tls;

pub use client::{Client, Config, Experience, Graphics, Info, Nla, Platform, TouchPhase, compression, experience, graphics};
pub use runtime::{Credentials, Handler};
pub use error::{Error, Result};
pub use framebuffer::Framebuffer;
pub use keymap::{Key, scancode};
