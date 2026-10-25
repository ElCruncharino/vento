// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Why a session ended, as the message the app shows.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The socket, at any point.
    Io(std::io::Error),
    /// The peer refused the login, in its own words.
    Refused(String),
    /// A password or identity prompt was cancelled or declined.
    Cancelled,
    /// The peer sent something malformed or unexpected.
    Protocol(String),
    /// The address is invalid; the message says why.
    Address(String),
    /// The peer stopped sending anything. RustDesk's server sends a
    /// `TestDelay` about once a second regardless of activity, so silence for
    /// this long means the connection is gone rather than the desktop idle.
    Silent,
    /// `Client::close` was called; the only ordinary ending.
    Closed,
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // A dropped connection is the common case and the message is shown
            // to the user, so it says what happened rather than naming an
            // errno.
            Error::Io(e) => match e.kind() {
                std::io::ErrorKind::ConnectionRefused => {
                    write!(f, "Nothing is listening at that address.")
                }
                std::io::ErrorKind::TimedOut => write!(f, "The connection timed out."),
                std::io::ErrorKind::UnexpectedEof => write!(f, "The peer closed the connection."),
                _ => write!(f, "{e}"),
            },
            Error::Refused(why) if why.is_empty() => write!(f, "The peer refused the connection."),
            Error::Refused(why) => write!(f, "{why}"),
            Error::Cancelled => write!(f, "Cancelled."),
            Error::Protocol(what) => write!(f, "The peer is not making sense: {what}"),
            Error::Address(why) => write!(f, "{why}"),
            Error::Silent => write!(f, "The connection stopped responding."),
            Error::Closed => write!(f, "Disconnected"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}
