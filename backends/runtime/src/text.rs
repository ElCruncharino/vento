// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Line ends in a clipboard's text: LF on this side of the seam, and CRLF on
//! the wire of a protocol whose far end is Windows' idea of text (RDP's, and
//! TigerVNC's extended clipboard). The C session files keep the same rule in
//! `vento/text.h` (`vento_text_to_lf`, `vento_text_to_crlf`), and both are
//! held to the same cases.

/// CRLF, and a lone CR, to LF: what text is everywhere above a protocol.
pub fn to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Every line end, whichever it was, to CRLF: what a Windows application
/// puts on a clipboard and expects to find there.
pub fn to_crlf(text: &str) -> String {
    to_lf(text).replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One line end in, one line end out, whichever end started it and
    /// however many times the text has crossed. `vento/text.h`'s cases.
    #[test]
    fn line_ends_are_normalised_once() {
        assert_eq!(to_lf("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert_eq!(to_lf("\r\r\n\n"), "\n\n\n");
        assert_eq!(to_crlf("a\nb\r\nc\rd"), "a\r\nb\r\nc\r\nd");
        assert_eq!(to_crlf("\r\r\n\n"), "\r\n\r\n\r\n");
        assert_eq!(to_crlf(&to_crlf("a\nb")), "a\r\nb");
        assert_eq!(to_lf(&to_crlf("héllo\nwörld\n")), "héllo\nwörld\n");
        assert_eq!(to_lf(""), "");
        assert_eq!(to_crlf("no line end"), "no line end");
    }
}
