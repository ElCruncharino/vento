// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Why a session ended, in the text the app shows.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The socket, at any point.
    Io(std::io::Error),
    /// The server refused before authentication, usually "too many attempts".
    Refused(String),
    /// The credentials were wrong.
    AuthFailed(String),
    /// The password prompt was cancelled.
    Cancelled,
    /// The server's certificate was not the one pinned for this address, or a
    /// new one was not accepted.
    Untrusted,
    /// The server needs something this client does not implement.
    Unsupported(String),
    /// The server sent bytes that violate the protocol.
    Protocol(String),
    /// The entered text is not a valid address, with a message saying why.
    Address(String),
    /// `Client::close` was called; the only normal ending.
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
                std::io::ErrorKind::UnexpectedEof => write!(f, "The server closed the connection."),
                _ => write!(f, "{e}"),
            },
            Error::Refused(why) if why.is_empty() => write!(f, "The server refused the connection."),
            Error::Refused(why) => write!(f, "{why}"),
            Error::AuthFailed(why) => write!(f, "{why}"),
            Error::Cancelled => write!(f, "Cancelled."),
            Error::Untrusted => write!(f, "The server's certificate was not accepted."),
            Error::Unsupported(what) => write!(f, "Unsupported: {what}"),
            Error::Protocol(what) => write!(f, "The server is not making sense: {what}"),
            Error::Address(why) => write!(f, "{why}"),
            Error::Closed => write!(f, "Disconnected"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}
