// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! An RFB (VNC) client, implemented here rather than bound from a library.
//!
//! Writing one was chosen over using a crate: RFB is small, and the encodings
//! and pseudo-encodings this app needs are a short, specific list. All of it
//! is here (no async runtime, no protocol framework), and the parts that are
//! easy to get subtly wrong (the DES key's reversed bits, ZRLE's persistent
//! zlib stream, Hextile's colours carrying between tiles, an overlapping
//! `CopyRect`) have tests rather than comments claiming they are right.
//!
//! What it supports:
//!
//! | | |
//! |---|---|
//! | Versions | 3.3, 3.7, 3.8 |
//! | Security | None, VncAuth, VeNCrypt (X509None, X509Vnc, X509Plain) |
//! | Encodings | Raw, CopyRect, RRE, Hextile, ZRLE |
//! | Pseudo | Cursor, DesktopSize, ExtendedDesktopSize, LastRect, compression level, ExtendedMouseButtons, PointerTypeChange, ExtendedClipboard |
//! | Pointer | absolute, or relative where the server asks for it |
//! | Clipboard | both ways: UTF-8 text with the extended clipboard, Latin-1 without |
//!
//! Not supported, with the reason: **anonymous TLS** (VeNCrypt's 257–259,
//! which encrypt the connection but prove nothing about the far end; see
//! `security`) and **Tight** (it needs a JPEG decoder, and ZRLE is what
//! TigerVNC serves well on a LAN).
//!
//! Everything works in one pixel format, [`PixelFormat::NATIVE`], chosen so a
//! framebuffer word is already an Android `ARGB_8888` pixel and a region read
//! is a row copy.

/// The highest version this client supports. Returned across JNI by
/// `version()`, the simplest call that exercises the whole native path.
pub const PROTOCOL_VERSION: &str = "RFB 3.8";

// The protocol as an `api::Backend`, with this backend's options read into
// its config, and its info rows. The Linux client uses it directly, Android
// through the shared JNI layer below.
pub mod backend;

// Android only: the library the app loads, registering this backend on the
// module's generated natives class. Everything else is the protocol, which
// builds, tests and runs an example on the host without this.
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
pub use runtime::{Credentials, Handler};
pub use decode::{ENC_HEXTILE, ENC_RAW, ENC_RRE, ENC_ZRLE, encoding_name};
pub use error::{Error, Result};
pub use framebuffer::Framebuffer;
pub use pixel::PixelFormat;
pub use security::Security;
