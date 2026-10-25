// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The parts of a backend's native half that are about Android rather than
//! about a protocol: [`backend`], which is any `api::Backend` as the
//! natives `NativeBackend.java` calls, [`bitmap`], which writes pixels
//! straight into a Java `Bitmap`, [`log`], which points the `log` crate at
//! logcat, [`mediacodec`], which is the phone's video decoder speaking
//! `runtime::media::video::Decoders`, [`aaudio`], which is its sound output
//! speaking `runtime::media::audio::Outputs`, and [`mediacodec_audio`], its AAC
//! and Opus decoders — and [`surface`], the JNI table itself, which
//! is plain data and builds everywhere, because the code generator that
//! writes each module's natives class runs on the host.
//!
//! It was one crate with the RFB bindings in it, then one crate with every
//! protocol's bindings beside it, and is now the one JNI layer: what a Rust
//! backend's library adds to its protocol is its `api::Backend` adapter and
//! one [`natives!`] line.
//!
//! Two threads reach a session, as they do in the RealVNC backend and for the
//! same reasons: the protocol thread, which owns the connection and delivers
//! every event, and **whichever thread is drawing**, which calls `readRegion`
//! straight through so that pixel fetches never queue behind connection work.

#[cfg(target_os = "android")]
pub mod aaudio;
#[cfg(target_os = "android")]
pub mod backend;
#[cfg(target_os = "android")]
pub mod bitmap;
#[cfg(target_os = "android")]
pub mod log;
#[cfg(target_os = "android")]
pub mod mediacodec;
#[cfg(target_os = "android")]
pub mod mediacodec_audio;
pub mod surface;

#[cfg(target_os = "android")]
use jni::objects::JString;

/// `null` and the empty string both mean "nothing", because a Java field left
/// blank arrives as one or the other depending on who filled it in.
#[cfg(target_os = "android")]
pub fn string(env: &mut jni::Env, s: &JString) -> Result<Option<String>, jni::errors::Error> {
    if s.is_null() {
        return Ok(None);
    }
    let text = s.try_to_string(env)?;
    Ok(if text.is_empty() { None } else { Some(text) })
}
