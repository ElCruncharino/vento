// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A SPICE client: their protocol and their decoders, our session.
//!
//! The split is note 76 §2's, and it is neither of the two this repository had
//! before. RFB is written and RDP is bound; SPICE's hard parts are the image
//! codecs — QUIC is its own lossless codec and GLZ has a dictionary shared
//! across a whole connection — and its easy part is the state machine, so the
//! codecs are `shakenfist-spice-{protocol,compression}` at `third_party/ryll`
//! and everything above them is here.
//!
//! What it speaks:
//!
//! | | |
//! |---|---|
//! | Transport | four TCP connections, plain or TLS on a port of its own, pinned rather than verified |
//! | Auth | a ticket encrypted to a key the server makes per connection |
//! | Picture | draw commands carrying QUIC, GLZ, LZ, LZ4, JPEG or a raw bitmap, and MJPEG video streams |
//! | Cursor | shape and hotspot, on a channel of its own |
//! | Input | absolute or relative pointer, five buttons and a wheel; keys as AT scancodes |
//! | Live | the image compression the server encodes with |
//! | Agent | the clipboard both ways and a desktop size asked for, where the guest runs `spice-vdagent` |
//!
//! What it does not, and why: **audio**, which is one sink in this whole app
//! and it is inside FreeRDP's tree; **USB redirection**, **WebDAV** and
//! **smartcard**, which are not a desktop; **file transfer**, which the agent
//! carries and a phone has nowhere to put; and **H.264 streams**, whose decoder
//! would be the platform's — a `common::video::Decoders`, as RustDesk's is —
//! and is a stage of its own.
//!
//! It is not the reference implementation and does not have to be: what a
//! decoder produces was compared with `remote-viewer`'s own picture, pixel for
//! pixel, before any of this was written.

/// The JNI half: the entry points the Java in this module declares, in front
/// of a `common::Session`. Android only — everything above is the protocol,
/// and it builds and tests on the host with none of this compiled.
#[cfg(target_os = "android")]
mod bindings;

mod agent;
mod channel;
mod client;
mod cursor;
mod display;
mod error;
mod framebuffer;
mod images;
mod keymap;
mod tls;

pub use client::{Client, Config, Info, VERSION, compression_of};
pub use common::{Credentials, Handler};
pub use error::{Error, Result};
pub use framebuffer::Framebuffer;

/// What the channels' tests feed a message through: a handler that takes
/// everything and keeps none of it, since what those tests ask is whether a
/// message is survived, and a runtime to wait on the async ones with.
#[cfg(test)]
mod testing {
    use common::{Credentials, Handler};

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
