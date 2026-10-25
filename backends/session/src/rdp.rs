// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the two RDP backends read alike out of their options: the desktop
//! size to ask for and how many monitors. RDP has no "whatever it already
//! is": the size is in the client's connection data, so something has to go
//! there.

use crate::options::Options;

/// US English, and not a choice: the scancode tables the phone sends are a
/// US layout, so the layout the server is told to use has to be the same
/// one.
pub const KEYBOARD_LAYOUT: u32 = 0x0409;

/// What to ask for when nothing better is known: the first connection to a
/// machine under `remote`, and anything unparseable.
pub const FALLBACK_SIZE: (u32, u32) = (1920, 1200);

/// `1920x1200` as its two numbers, or none.
pub fn parse_size(value: &str) -> Option<(u32, u32)> {
    let (w, h) = value.split_once('x')?;
    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// The desktop size to ask this connection's server for. `remote` means
/// "whatever it was last time", which is a note a client keeps per address
/// where it keeps one, and the fallback where it does not. `device` is the
/// screen the session will be drawn on, where the caller knows it.
pub fn desktop_size(value: &str, screen: Option<(u32, u32)>) -> (u32, u32) {
    match value {
        "remote" => FALLBACK_SIZE,
        "device" => screen
            .filter(|&(w, h)| (200..=4096).contains(&w) && (200..=4096).contains(&h))
            .unwrap_or(FALLBACK_SIZE),
        _ => parse_size(value).unwrap_or(FALLBACK_SIZE),
    }
}

/// How many monitors a connection asks for, which is a number in a choice
/// list and so is only a number if nothing has been typed in by hand.
pub fn monitor_count(options: &Options) -> u8 {
    options.int("Monitors", 1).clamp(1, 16) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_size_option_is_read_as_the_provider_reads_it() {
        assert_eq!(desktop_size("1366x768", None), (1366, 768));
        assert_eq!(desktop_size("remote", Some((800, 600))), FALLBACK_SIZE);
        assert_eq!(desktop_size("device", Some((2560, 1440))), (2560, 1440));
        assert_eq!(desktop_size("device", Some((100, 100))), FALLBACK_SIZE);
        assert_eq!(desktop_size("device", None), FALLBACK_SIZE);
        assert_eq!(desktop_size("nonsense", None), FALLBACK_SIZE);
        assert_eq!(parse_size("0x5"), None);
    }
}
