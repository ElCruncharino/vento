// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The parts of a backend's native half that are about Android rather than
//! about a protocol: [`bitmap`], which writes pixels straight into a Java
//! `Bitmap`, [`callbacks`], which is a session's Java side resolved once and
//! speaking `common::Events`, [`log`], which points the `log` crate at logcat,
//! and [`mediacodec`], which is the phone's video decoder speaking
//! `common::video::Decoders`.
//!
//! It was one crate with the RFB bindings in it, then one crate with both,
//! then a module of `remotedesktop-common`, and the splits changed only where
//! the seams are. What is left in a protocol's own `bindings` module is that
//! protocol's entry points and nothing else: the session they stand in front
//! of is `common::Session`, which is the same for every protocol and for every
//! platform.
//!
//! Two threads reach a session, as they do in the RealVNC backend and for the
//! same reasons: the protocol thread, which owns the connection and delivers
//! every callback, and **whichever thread is drawing**, which calls
//! `nativeReadRegion` straight through so that pixel fetches never queue behind
//! connection work.
//!
//! The handle is a `Box::into_raw`'d session. Java retires it under its own lock
//! before calling `nativeDestroy`, which is the same discipline the RealVNC
//! backend follows — the difference being that here the join in `destroy` can
//! prove the callbacks are finished, because both ends of the boundary are ours.

// Empty on any other target, so that the workspace builds on the host with
// this crate in it and nothing of `jni` behind it.
#![cfg(target_os = "android")]

pub mod bitmap;
pub mod callbacks;
pub mod log;
pub mod mediacodec;

use jni::objects::JString;

/// `null` and the empty string both mean "nothing", because a Java field left
/// blank arrives as one or the other depending on who filled it in.
pub fn string(env: &mut jni::Env, s: &JString) -> Result<Option<String>, jni::errors::Error> {
    if s.is_null() {
        return Ok(None);
    }
    let text = s.try_to_string(env)?;
    Ok(if text.is_empty() { None } else { Some(text) })
}
