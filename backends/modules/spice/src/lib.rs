// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A SPICE client: ryll's protocol and decoder crates under this crate's own
//! session layer.
//!
//! This split differs from the other protocols here, where RFB is implemented
//! from scratch and RDP uses existing libraries. SPICE's hard parts are the
//! image codecs (QUIC is its own lossless codec, and GLZ has a dictionary
//! shared across a whole connection) and its easy part is the state machine,
//! so the codecs come from `shakenfist-spice-{protocol,compression}` in
//! `third_party/ryll` and everything above them is here.
//!
//! What it speaks:
//!
//! | | |
//! |---|---|
//! | Transport | four TCP connections (five with sound), plain or TLS on a port of its own, pinned rather than verified |
//! | Auth | a ticket encrypted to a key the server makes per connection |
//! | Picture | draw commands carrying QUIC, GLZ, LZ, LZ4, JPEG or a raw bitmap, and MJPEG video streams |
//! | Cursor | shape and hotspot, on a channel of its own |
//! | Input | absolute or relative pointer, five buttons and a wheel; keys as AT scancodes |
//! | Live | the image compression the server encodes with |
//! | Agent | the clipboard both ways and a desktop size asked for, where the guest runs `spice-vdagent` |
//! | Sound | the playback channel, PCM or Opus, on the session's output (`playback`) |
//!
//! What it does not support, and why: **the microphone** (the record
//! channel), which no backend here sends; **USB redirection**, **WebDAV** and
//! **smartcard**, which are not part of a remote desktop; **file transfer**,
//! which the agent carries but the app does not handle; and **H.264 streams**,
//! whose decoder would be the platform's (a `runtime::media::video::Decoders`,
//! as RustDesk uses) and is not implemented.
//!
//! It is not the reference implementation: the decoders' output was checked
//! pixel for pixel against `remote-viewer`'s picture.

// The client as an `api::Backend`, with its option handling and connection
// panel rows: used directly on Linux, and through the JNI layer below on
// Android.
pub mod backend;

// The library the app loads: this backend, registered on the module's
// generated natives class, with AAudio for the guest's sound (and MediaCodec
// for its Opus, through the same object). Android only; the rest builds and
// tests on the host without it.
#[cfg(target_os = "android")]
platform_android::natives!(
    class = c"net/pgaskin/remotedesktop/backend/spice/SpiceNatives",
    kind = backend::KIND,
    audio = || Some(std::sync::Arc::new(platform_android::aaudio::AAudio::new())),
);

mod agent;
mod channel;
mod client;
mod cursor;
mod display;
mod error;
mod framebuffer;
mod images;
mod keymap;
mod playback;
mod tls;

pub use client::{Client, Config, Info, VERSION, compression_of};
pub use runtime::{Credentials, Handler};
pub use error::{Error, Result};
pub use framebuffer::Framebuffer;

/// What the channels' tests feed messages through: a handler that ignores
/// everything, since those tests only check that a message is handled without
/// failing, and a runtime to wait on the async ones.
#[cfg(test)]
mod testing {
    use runtime::{Credentials, Handler};

    pub struct Nothing;

    impl Handler for Nothing {
        fn connected(&mut self, _width: usize, _height: usize) {}
        fn desktop_size(&mut self, _width: usize, _height: usize) {}
        fn damaged(&mut self, _x: usize, _y: usize, _width: usize, _height: usize) {}
        fn frame_end(&mut self) {}
        fn cursor(&mut self, _p: &[u32], _w: usize, _h: usize, _hx: i32, _hy: i32) {}
        fn clipboard(&mut self, _text: &str) {}
        fn credentials(&mut self, _needs_user: bool) -> Option<Credentials> {
            None
        }
        fn trust(&mut self, _fingerprint: &str) -> bool {
            false
        }
    }

    pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }
}
