// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! An RFB (VNC) client, written rather than bound to.
//!
//! A crate was weighed against writing one and writing won: RFB is small, and
//! the encodings and pseudo-encodings this app needs are a short and specific
//! list. The whole of it is here — no async runtime, no protocol framework —
//! and the parts that are easy to get subtly wrong (the DES key's reversed
//! bits, ZRLE's persistent zlib stream, Hextile's colours carrying between
//! tiles, an overlapping `CopyRect`) have tests rather than comments claiming
//! they are right.
//!
//! What it speaks:
//!
//! | | |
//! |---|---|
//! | Versions | 3.3, 3.7, 3.8 |
//! | Security | None, VncAuth, VeNCrypt (X509None, X509Vnc, X509Plain) |
//! | Encodings | Raw, CopyRect, RRE, Hextile, ZRLE |
//! | Pseudo | Cursor, DesktopSize, LastRect, compression level, ExtendedMouseButtons, PointerTypeChange, ExtendedClipboard |
//! | Pointer | absolute, or relative where the server asks for it |
//! | Clipboard | both ways: UTF-8 text with the extended clipboard, Latin-1 without |
//!
//! What it does not, and where each is answered: **anonymous TLS** (VeNCrypt's
//! 257–259, which encrypt the line and prove nothing about the far end — see
//! `security`), **Tight** (a JPEG decoder, and ZRLE is what TigerVNC serves a
//! LAN well with), and **ExtendedDesktopSize** (resizing the desktop from here,
//! which nothing in the app asks for).
//!
//! Everything works in one pixel format, [`PixelFormat::NATIVE`], chosen so
//! that a framebuffer word is already an Android `ARGB_8888` pixel and a region
//! read is a row copy.

/// The highest version this client speaks. Answered across JNI as the one call
/// that proves the whole native path.
pub const PROTOCOL_VERSION: &str = "RFB 3.8";

// The protocol as the clients hold it: a `session::Backend`, with this
// backend's options read into its config and its rows — the window's on
// Linux, and the phone's through the one JNI layer below.
pub mod backend;

// The library the app loads: this backend, registered on the module's
// generated natives class. Android only — everything else is the protocol, and
// it builds, tests and runs an example on the host with none of this compiled.
#[cfg(target_os = "android")]
platform_android::natives!(
    class = c"net/pgaskin/remotedesktop/backend/rfb/RfbNatives",
    kind = backend::KIND,
);

mod client;
mod clipboard;
mod decode;
mod error;
mod framebuffer;
mod pixel;
mod proto;
mod security;
mod transport;
mod zlib;

pub use client::{Client, Config, Info};
pub use common::{Credentials, Handler};
pub use decode::{ENC_HEXTILE, ENC_RAW, ENC_RRE, ENC_ZRLE, encoding_name};
pub use error::{Error, Result};
pub use framebuffer::Framebuffer;
pub use pixel::PixelFormat;
pub use security::Security;
