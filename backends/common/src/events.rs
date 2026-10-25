// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The outward half of a session: what a platform is told, and nothing it is
//! asked.

/// What a [`Session`](crate::Session) tells the platform, fire-and-forget.
///
/// This is [`Handler`](crate::Handler) with the blocking taken out: every
/// question becomes an event that says a question is pending, and the answer
/// arrives later through the session — which is how a dialog on another thread
/// has to work, and how a C caller would want it too. The shape is the Java
/// `*Native.Callbacks` interface's, method for method, because that interface
/// is the one platform this has had to fit so far and there was no reason to
/// invent a second.
///
/// Every method is called from the protocol thread, through a shared
/// reference, and must not block on anything the protocol thread is waiting
/// for — or, behind the C API, from a thread the C library runs beside it
/// (FreeRDP's graphics pipeline paints on its dynamic channels' thread:
/// `vento/backend.h`). `Send + Sync` because the session's thread calls these and whoever
/// created the session holds the other end.
pub trait Events: Send + Sync {
    fn connected(&self, width: usize, height: usize);
    fn desktop_size(&self, width: usize, height: usize);
    fn damaged(&self, x: usize, y: usize, width: usize, height: usize);
    fn frame_end(&self);
    /// `pixels` is the framebuffer's own word order (`R, G, B, A` in memory);
    /// whatever the platform wants instead, it converts on its side of this
    /// call, where it already has to copy. See [`crate::cursor`] for the two
    /// things every platform so far has wanted.
    fn cursor(&self, pixels: &[u32], width: usize, height: usize, hot_x: i32, hot_y: i32);
    fn pointer_mode(&self, relative: bool);
    fn bell(&self);
    fn clipboard(&self, text: &str);
    /// Credentials are wanted; answered through
    /// [`Session::answer_credentials`](crate::Session::answer_credentials).
    fn credentials_needed(&self, needs_user: bool);
    /// A trust decision is wanted; answered through
    /// [`Session::answer_trust`](crate::Session::answer_trust).
    fn trust_needed(&self, fingerprint: &str);
    /// The far end could not be verified and the session wants to know
    /// whether to go on. Answered through the same
    /// [`Session::answer_trust`](crate::Session::answer_trust), since a yes
    /// and a no mean the same things to the protocol thread.
    fn unverified(&self, why: &str);
    /// The session is over and its thread is about to end. `detail` is empty
    /// for an ending nobody needs telling about, and otherwise says why in the
    /// words the error chose.
    fn closed(&self, detail: &str);
}
