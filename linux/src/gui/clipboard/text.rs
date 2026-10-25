// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The clipboard's text formats, in one table for both display servers, so
//! that what is advertised and what is served cannot disagree, and what is
//! read is decoded by the type it actually came as.
//!
//! X11 names its formats by target atom and Wayland by mime type, and the
//! names overlap: Xwayland and the X bridges put `UTF8_STRING` and `STRING`
//! on the Wayland side, and X clients ask for `text/plain;charset=utf-8`. So
//! one name list does for both. Modelled on xwlrvnc's `bridge/x11/targets.rs`
//! (same author, MIT), which bridges the two.

/// The most text read from the local clipboard, in bytes. Every backend's far
/// end takes less (RFB a megabyte, Xvnc 256 KiB in), so this only keeps a
/// runaway owner from filling memory.
pub const MAX_TEXT: usize = 16 << 20;

/// What text can come as, best first: the order a read picks from what the
/// owner offers. UTF-8 by name first, then Latin-1, which is exact for what
/// it holds, then the two whose encoding is the owner's guess.
pub const READ_ORDER: &[&str] = &[
    "UTF8_STRING",
    "text/plain;charset=utf-8",
    "text/plain;charset=UTF-8",
    "STRING",
    "TEXT",
    "text/plain",
];

/// How a served format is encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    /// ISO 8859-1, `?` for what it lacks: X11's `STRING` by definition.
    Latin1,
}

/// What this end serves its text as, X11 and Wayland alike: each name, the
/// property type an X11 requestor is given it as, and the encoding.
/// `TEXT` is not a type ("it will never be the returned type", says the
/// ICCCM), so it is delivered as the `UTF8_STRING` it is; a mime type is
/// delivered as itself. Plain `text/plain` is served as UTF-8, which is what
/// every current toolkit means by it.
pub const SERVED: &[(&str, &str, Encoding)] = &[
    ("UTF8_STRING", "UTF8_STRING", Encoding::Utf8),
    ("text/plain;charset=utf-8", "text/plain;charset=utf-8", Encoding::Utf8),
    ("text/plain;charset=UTF-8", "text/plain;charset=UTF-8", Encoding::Utf8),
    ("text/plain", "text/plain", Encoding::Utf8),
    ("TEXT", "UTF8_STRING", Encoding::Utf8),
    ("STRING", "STRING", Encoding::Latin1),
];

/// The name to ask an owner for, out of what it offers.
pub fn best<'a>(offered: impl IntoIterator<Item = &'a str> + Clone) -> Option<&'static str> {
    READ_ORDER.iter().copied().find(|want| offered.clone().into_iter().any(|o| o == *want))
}

/// What one served name is delivered as.
pub fn served(name: &str) -> Option<(&'static str, Encoding)> {
    SERVED.iter().find(|(n, _, _)| *n == name).map(|(_, t, e)| (*t, *e))
}

/// The text as `encoding` puts it on the wire.
pub fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Latin1 => text.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).collect(),
    }
}

/// The text out of what an owner delivered, by the type it says it is,
/// which need not be the one asked for: xclip answers a `UTF8_STRING`
/// request with a `STRING` property when that is what it was given.
///
/// `STRING` is Latin-1 and maps byte for byte. Everything else is taken as
/// UTF-8, and where it is not valid UTF-8 it is read as Latin-1 instead,
/// whole. Not replaced with U+FFFD: TigerVNC's servers refuse a clipboard
/// with U+FFFD in it as invalid (`isValidUTF8` counts it as a decoding
/// failure), so the whole copy would be lost at an Xvnc far end, where
/// Latin-1 gives every far end a character for every byte. Text that is
/// not UTF-8 under a UTF-8 name is also nearly always an older program's
/// Latin-1, which this reads exactly, and which a Latin-1 far end (plain
/// RFB) is given back byte for byte.
pub fn decode(type_name: &str, bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) if type_name != "STRING" => text.to_string(),
        _ => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_prefers_utf_8_then_latin_1() {
        assert_eq!(best(["TARGETS", "STRING", "UTF8_STRING"]), Some("UTF8_STRING"));
        assert_eq!(best(["text/plain", "TEXT", "STRING"]), Some("STRING"));
        assert_eq!(best(["text/plain;charset=utf-8", "text/plain"]), Some("text/plain;charset=utf-8"));
        assert_eq!(best(["text/plain"]), Some("text/plain"));
        assert_eq!(best(["TARGETS", "image/png", "text/html"]), None);
    }

    /// What arboard advertised and refused: every name served converts, and
    /// `TEXT` is delivered as a real type.
    #[test]
    fn every_served_name_converts() {
        for (name, _, _) in SERVED {
            assert!(served(name).is_some(), "{name}");
        }
        assert_eq!(served("TEXT"), Some(("UTF8_STRING", Encoding::Utf8)));
        assert_eq!(served("STRING"), Some(("STRING", Encoding::Latin1)));
        assert_eq!(served("text/plain"), Some(("text/plain", Encoding::Utf8)));
        assert_eq!(served("MULTIPLE"), None);
        // And what a read would choose is among them, so the two ends of
        // two vento windows understand each other.
        assert!(READ_ORDER.iter().all(|n| served(n).is_some()));
    }

    #[test]
    fn latin_1_has_a_question_mark_for_what_it_lacks() {
        assert_eq!(encode("caf\u{e9} \u{2014} \u{1f600}", Encoding::Latin1), b"caf\xe9 ? ?");
        assert_eq!(encode("caf\u{e9}", Encoding::Utf8), "caf\u{e9}".as_bytes());
    }

    /// The two repros: a Latin-1-only owner, and bad UTF-8 under its name.
    #[test]
    fn what_arrives_is_decoded_by_its_type() {
        assert_eq!(decode("STRING", b"caf\xe9 latin1-only"), "caf\u{e9} latin1-only");
        assert_eq!(decode("UTF8_STRING", b"bad\xff utf8"), "bad\u{ff} utf8");
        assert_eq!(decode("text/plain", b"caf\xe9"), "caf\u{e9}");
        // STRING is Latin-1 even where the bytes happen to be UTF-8 too.
        assert_eq!(decode("STRING", "\u{e9}".as_bytes()), "\u{c3}\u{a9}");
        assert_eq!(decode("text/plain;charset=utf-8", "caf\u{e9}".as_bytes()), "caf\u{e9}");
        assert_eq!(decode("UTF8_STRING", "\u{1f600}".as_bytes()), "\u{1f600}");
    }
}
