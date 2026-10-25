// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a protocol crate is, to a session.

use crate::handler::Handler;
use std::fmt::Display;

/// One protocol's client: a connection that runs to completion on the calling
/// thread and can be closed from any other.
///
/// Every protocol here has the same shape around whatever it speaks (a
/// `Client::new()`, a `run` that blocks for the life of the session and
/// delivers every [`Handler`] call on the thread it was called from, and a
/// `close` that makes `run` return) because all of them have to fit
/// `Backend`. This trait is that shape, so that a [`Runner`](crate::Runner)
/// can be written once.
///
/// `Send + Sync + 'static` because a session shares the client between the
/// protocol thread and the holder of the handle: pointer events, region
/// reads and `close` all arrive from other threads.
pub trait Client: Send + Sync + 'static {
    /// What a session is given before it starts.
    type Config: Send + 'static;

    /// Why a session ended, in words a screen can show.
    type Error: Display;

    /// The protocol, for the name of the thread it runs on.
    const NAME: &'static str;

    /// Connect and run until the session ends. Blocks; every [`Handler`] call
    /// happens on this thread.
    fn run(&self, config: &Self::Config, handler: &mut dyn Handler) -> Result<(), Self::Error>;

    /// End the session from another thread. `run` returns soon after; what it
    /// returns is up to the protocol.
    fn close(&self);

    /// Whether `error` is the ending [`close`](Client::close) asked for, which
    /// the screen already knows about and shows no message for. The default
    /// says no, and a protocol whose `run` reports a requested close as an
    /// error overrides it so that the close arrives with an empty detail.
    fn closed_on_request(error: &Self::Error) -> bool {
        let _ = error;
        false
    }
}
