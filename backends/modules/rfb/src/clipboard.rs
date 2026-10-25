// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The extended clipboard: UTF-8 both ways, where plain RFB has Latin-1.
//!
//! Plain `ServerCutText` and `ClientCutText` carry ISO 8859-1 and nothing
//! else, so anything past it — a dash, a euro sign, every script but a few —
//! crossed as `?`. The extension (pseudo-encoding `0xC0A1E5CE`, TigerVNC's and
//! RealVNC's) rides the same two messages with a negative length, and
//! inside them a word of flags: an action and the formats it is about. This
//! client speaks text and only text, as TigerVNC's own viewer does, and
//! follows its `CConnection` turn for turn, since that is the implementation
//! servers are tested against:
//!
//! - The server announces its **caps** first (which actions it takes, and up
//!   to how much of each format it will take unasked). This end answers with
//!   its own: text, every action, and a size of 0 for text, which asks the
//!   server always to *notify* rather than send unasked.
//! - A **notify** says the server's clipboard changed; this end **requests**
//!   the text straight away, as the TigerVNC backend does, and the
//!   **provide** that answers is zlib-compressed and delivered.
//! - Locally copied text is announced with a notify, or provided at once
//!   where the server said it takes that much unasked; a request for it is
//!   answered with a provide, and a **peek** with a notify of whether there
//!   is any.
//!
//! The text is UTF-8 with CRLF line ends on the wire and LF here, which is
//! what TigerVNC converts between, and what RFB's plain messages say lines
//! end in. A server that never announces caps gets the plain messages, as
//! before.
//!
//! Everything here is pure — a message in, a message or a text out — so the
//! encodings are tested byte for byte; `client.rs` holds the state.

use crate::zlib::ZlibStream;
use flate2::{Compress, Compression, FlushCompress};

/// CRLF, and a lone CR, to LF (TigerVNC's `convertLF`, the form text has
/// here and in a plain `ServerCutText`), and every line end to CRLF for a
/// provide (its `convertCRLF`): runtime's, which both RDP backends keep too.
pub use common::text::{to_crlf, to_lf};

/// The pseudo-encoding that says this client speaks the extension.
pub const ENC_EXTENDED_CLIPBOARD: i32 = 0xC0A1_E5CE_u32 as i32;

/// Formats: the low sixteen bits. Only text is spoken here; the rest are
/// RTF, HTML, DIB and files.
pub const UTF8: u32 = 1 << 0;
const FORMAT_MASK: u32 = 0x0000_ffff;

/// Actions: the high byte.
pub const CAPS: u32 = 1 << 24;
pub const REQUEST: u32 = 1 << 25;
pub const PEEK: u32 = 1 << 26;
pub const NOTIFY: u32 = 1 << 27;
pub const PROVIDE: u32 = 1 << 28;
const ACTION_MASK: u32 = 0xff00_0000;

/// One extended clipboard message from the server.
#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    /// What the server takes: its action bits, and for each format it
    /// names, the most it accepts unasked (0: only a notify).
    Caps { flags: u32, utf8_size: Option<u32> },
    /// The server wants these formats of ours.
    Request(u32),
    /// The server asks what we have.
    Peek,
    /// The server's clipboard now has these formats (none: it is empty).
    Notify(u32),
    /// The server's text, or `None` where it provided none this end takes
    /// (another format, too much of it, or not UTF-8).
    Provide(Option<String>),
}

/// Reads one message's body: everything after the length, which the caller
/// has already bounded and read whole.
///
/// `max` bounds the text once inflated, as the length bounds it on the wire:
/// a megabyte of zlib inflates to a gigabyte. A provide past it is dropped,
/// with the stream still in step, since each provide is a zlib stream of its
/// own and the caller read all of this one.
pub fn parse(body: &[u8], max: usize) -> Result<Message, String> {
    let Some(flags) = body.get(..4) else {
        return Err("an extended clipboard message without its flags".into());
    };
    let flags = u32::from_be_bytes(flags.try_into().unwrap());
    let rest = &body[4..];
    if flags & CAPS != 0 {
        // One size per format named, in bit order.
        let mut sizes = rest.chunks_exact(4).map(|c| u32::from_be_bytes(c.try_into().unwrap()));
        let mut utf8_size = None;
        for bit in 0..16 {
            if flags & (1 << bit) != 0 {
                let size = sizes.next().ok_or("extended clipboard caps without their sizes")?;
                if bit == 0 {
                    utf8_size = Some(size);
                }
            }
        }
        return Ok(Message::Caps { flags, utf8_size });
    }
    match flags & ACTION_MASK {
        PROVIDE => Ok(Message::Provide(provided_text(flags, rest, max))),
        REQUEST => Ok(Message::Request(flags & FORMAT_MASK)),
        PEEK => Ok(Message::Peek),
        NOTIFY => Ok(Message::Notify(flags & FORMAT_MASK)),
        other => Err(format!("extended clipboard action {other:#x}")),
    }
}

/// The text out of a provide: inflated, the formats walked in bit order to
/// the first (text is bit 0), cut at its terminating NUL, and refused unless
/// it is UTF-8, as TigerVNC refuses it.
fn provided_text(flags: u32, deflated: &[u8], max: usize) -> Option<String> {
    if flags & UTF8 == 0 {
        log::debug!("extended clipboard: a provide of formats {:#x}, none of them text", flags & FORMAT_MASK);
        return None;
    }
    let mut zlib = ZlibStream::new();
    // The text, its length word, and room for the NUL TigerVNC counts in.
    let inflated = match zlib.inflate(deflated, max + 8) {
        Ok(inflated) => inflated,
        Err(e) => {
            log::warn!("the far end's clipboard did not inflate within {max} bytes ({e}); skipped");
            return None;
        }
    };
    let length = u32::from_be_bytes(inflated.get(..4)?.try_into().unwrap()) as usize;
    if length > max + 1 {
        log::warn!("the far end's clipboard of {length} bytes is over the {max} this client takes; skipped");
        return None;
    }
    let Some(text) = inflated.get(4..4 + length) else {
        log::warn!("the far end's clipboard said {length} bytes and held fewer; skipped");
        return None;
    };
    let text = text.split(|&b| b == 0).next().unwrap_or_default();
    match std::str::from_utf8(text) {
        Ok(text) => Some(to_lf(text)),
        Err(_) => {
            log::warn!("the far end's clipboard was not UTF-8; skipped");
            None
        }
    }
}

/// A message to the server: the `ClientCutText` header with the negative
/// length that marks it extended, the flags, and `payload`.
fn message(flags: u32, payload: &[u8]) -> Vec<u8> {
    let length = -((4 + payload.len()) as i32);
    let mut msg = Vec::with_capacity(12 + payload.len());
    msg.push(6u8);
    msg.extend_from_slice(&[0, 0, 0]);
    msg.extend_from_slice(&length.to_be_bytes());
    msg.extend_from_slice(&flags.to_be_bytes());
    msg.extend_from_slice(payload);
    msg
}

/// Our caps, TigerVNC's: text, every action, and text's size 0, so the
/// server notifies rather than sending what nobody asked for.
pub fn caps() -> Vec<u8> {
    message(UTF8 | CAPS | REQUEST | PEEK | NOTIFY | PROVIDE, &0u32.to_be_bytes())
}

pub fn request() -> Vec<u8> {
    message(REQUEST | UTF8, &[])
}

/// Whether we have text, as the answer to a peek or as an announcement.
pub fn notify(available: bool) -> Vec<u8> {
    message(NOTIFY | if available { UTF8 } else { 0 }, &[])
}

/// The text a provide carries, as TigerVNC writes it: CRLF line ends and a
/// terminating NUL, which its length counts.
pub fn wire_text(text: &str) -> Vec<u8> {
    let mut bytes = to_crlf(text).into_bytes();
    bytes.push(0);
    bytes
}

/// A provide of `wire` (see [`wire_text`]): its length and its bytes in one
/// zlib stream of their own, sync-flushed, which is what TigerVNC's
/// `ZlibOutStream::flush` leaves and its reader expects.
pub fn provide(wire: &[u8]) -> Vec<u8> {
    let mut plain = Vec::with_capacity(4 + wire.len());
    plain.extend_from_slice(&(wire.len() as u32).to_be_bytes());
    plain.extend_from_slice(wire);
    let mut deflate = Compress::new(Compression::default(), true);
    let mut out = Vec::with_capacity(plain.len() / 2 + 64);
    loop {
        let consumed = deflate.total_in() as usize;
        deflate
            .compress_vec(&plain[consumed..], &mut out, FlushCompress::Sync)
            .expect("deflating into memory");
        // zlib's word for a finished flush: all the input taken and room
        // left over in the output. Otherwise there is more to come out.
        if deflate.total_in() as usize == plain.len() && out.len() < out.capacity() {
            break;
        }
        out.reserve(out.capacity().max(64));
    }
    message(PROVIDE | UTF8, &out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Decompress, FlushDecompress};

    /// A server message as it arrives: the flags and their payload, which
    /// is what `parse` is handed once the length has been read.
    fn body(flags: u32, payload: &[u8]) -> Vec<u8> {
        let mut b = flags.to_be_bytes().to_vec();
        b.extend_from_slice(payload);
        b
    }

    /// What a server's provide carries, deflated as Xvnc deflates it.
    fn deflated(formats: &[&[u8]]) -> Vec<u8> {
        let mut plain = Vec::new();
        for f in formats {
            plain.extend_from_slice(&(f.len() as u32).to_be_bytes());
            plain.extend_from_slice(f);
        }
        let mut deflate = Compress::new(Compression::default(), true);
        let mut out = Vec::with_capacity(plain.len() + 1024);
        deflate.compress_vec(&plain, &mut out, FlushCompress::Sync).unwrap();
        out
    }

    #[test]
    fn the_pseudo_encoding_is_tigervncs() {
        assert_eq!(ENC_EXTENDED_CLIPBOARD as u32, 0xC0A1E5CE);
    }

    /// Our caps, byte for byte: a length of −8, the flags, and text's size.
    #[test]
    fn caps_say_text_every_action_and_notify_only() {
        assert_eq!(
            caps(),
            vec![6, 0, 0, 0, 0xff, 0xff, 0xff, 0xf8, 0x1f, 0, 0, 1, 0, 0, 0, 0]
        );
    }

    #[test]
    fn request_and_notify_are_a_flags_word_alone() {
        assert_eq!(request(), vec![6, 0, 0, 0, 0xff, 0xff, 0xff, 0xfc, 0x02, 0, 0, 1]);
        assert_eq!(notify(true), vec![6, 0, 0, 0, 0xff, 0xff, 0xff, 0xfc, 0x08, 0, 0, 1]);
        assert_eq!(notify(false), vec![6, 0, 0, 0, 0xff, 0xff, 0xff, 0xfc, 0x08, 0, 0, 0]);
    }

    /// Xvnc's caps: text, RTF and HTML with their sizes, every action. Only
    /// text's size is kept, and a size for each named format must be there.
    #[test]
    fn server_caps_are_read_with_text_s_size() {
        let flags = UTF8 | 2 | 4 | CAPS | REQUEST | PEEK | NOTIFY | PROVIDE;
        let sizes: Vec<u8> = [10u32 << 20, 2 << 20, 2 << 20].iter().flat_map(|s| s.to_be_bytes()).collect();
        assert_eq!(
            parse(&body(flags, &sizes), 1 << 20),
            Ok(Message::Caps {
                flags,
                utf8_size: Some(10 << 20)
            })
        );
        assert!(parse(&body(flags, &sizes[..8]), 1 << 20).is_err());
        assert!(parse(&[0, 0], 1 << 20).is_err());
    }

    #[test]
    fn the_small_actions_parse() {
        assert_eq!(parse(&body(NOTIFY | UTF8, &[]), 64), Ok(Message::Notify(UTF8)));
        assert_eq!(parse(&body(NOTIFY, &[]), 64), Ok(Message::Notify(0)));
        assert_eq!(parse(&body(REQUEST | UTF8, &[]), 64), Ok(Message::Request(UTF8)));
        assert_eq!(parse(&body(PEEK, &[]), 64), Ok(Message::Peek));
        assert!(parse(&body(1 << 29, &[]), 64).is_err());
    }

    /// The far end's text: inflated, the NUL dropped, CRLF made LF, and the
    /// formats after text (here HTML) left alone.
    #[test]
    fn a_provide_is_inflated_to_text() {
        let text = "caf\u{e9} \u{2014} x\r\nline two\0".as_bytes();
        let b = body(PROVIDE | UTF8 | 4, &deflated(&[text, b"<p>html</p>"]));
        assert_eq!(
            parse(&b, 1 << 20),
            Ok(Message::Provide(Some("caf\u{e9} \u{2014} x\nline two".into())))
        );
    }

    #[test]
    fn a_provide_this_end_cannot_take_delivers_nothing() {
        // Not text.
        let b = body(PROVIDE | 4, &deflated(&[b"<p>html</p>"]));
        assert_eq!(parse(&b, 1 << 20), Ok(Message::Provide(None)));
        // Not UTF-8.
        let b = body(PROVIDE | UTF8, &deflated(&[b"bad \xff\0"]));
        assert_eq!(parse(&b, 1 << 20), Ok(Message::Provide(None)));
        // Over the limit once inflated, however small on the wire.
        let big = vec![b'x'; 4096];
        let b = body(PROVIDE | UTF8, &deflated(&[&big]));
        assert!(b.len() < 200);
        assert_eq!(parse(&b, 1024), Ok(Message::Provide(None)));
        // Not zlib at all.
        assert_eq!(parse(&body(PROVIDE | UTF8, b"nonsense"), 1024), Ok(Message::Provide(None)));
    }

    /// Our provide, read back the way Xvnc reads it: the negative length
    /// covers the flags and the stream, which inflates to the length word,
    /// the text with CRLF line ends and the NUL.
    #[test]
    fn a_provide_is_deflated_as_tigervnc_reads_it() {
        let text = "caf\u{e9} \u{2014} utf8 \u{1f600}\nnext\r\nlast";
        let msg = provide(&wire_text(text));
        assert_eq!(&msg[..4], &[6, 0, 0, 0]);
        let length = i32::from_be_bytes(msg[4..8].try_into().unwrap());
        assert_eq!(-length as usize, msg.len() - 8);
        assert_eq!(u32::from_be_bytes(msg[8..12].try_into().unwrap()), PROVIDE | UTF8);
        let mut inflate = Decompress::new(true);
        let mut plain = Vec::with_capacity(1024);
        inflate.decompress_vec(&msg[12..], &mut plain, FlushDecompress::Sync).unwrap();
        let expected = "caf\u{e9} \u{2014} utf8 \u{1f600}\r\nnext\r\nlast\0".as_bytes();
        assert_eq!(&plain[..4], &(expected.len() as u32).to_be_bytes());
        assert_eq!(&plain[4..], expected);
        // And it is what our own parser makes of a provide.
        assert_eq!(
            parse(&msg[8..], 1 << 20),
            Ok(Message::Provide(Some("caf\u{e9} \u{2014} utf8 \u{1f600}\nnext\nlast".into())))
        );
    }

    /// A text bigger than one deflate output buffer still goes in whole.
    #[test]
    fn a_large_provide_is_deflated_whole() {
        let text: String = (0..200_000).map(|i| char::from(b'a' + (i * 7 % 26) as u8)).collect();
        let msg = provide(&wire_text(&text));
        assert_eq!(parse(&msg[8..], 1 << 20), Ok(Message::Provide(Some(text))));
    }

    #[test]
    fn line_ends_convert_as_tigervnc_converts_them() {
        assert_eq!(to_lf("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert_eq!(to_crlf("a\nb\r\nc\rd"), "a\r\nb\r\nc\r\nd");
    }
}
