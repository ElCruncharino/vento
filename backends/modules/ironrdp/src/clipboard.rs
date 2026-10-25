// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The clipboard channel, text only.
//!
//! RFB sends Latin-1 text in a single message. CLIPRDR uses delayed
//! rendering: the copying end announces the *formats* it can produce, and the
//! other end requests one of them when the user pastes. So each direction
//! takes two exchanges rather than one, and the state between them is kept
//! here.
//!
//! Everything in this file runs on two threads. The [`CliprdrBackend`] half is
//! called from inside the protocol thread's decode; [`Clipboard::set_local`] is
//! called from whichever thread noticed a local copy. Neither may encode
//! anything (the channel object that encodes is owned by the active stage),
//! so both queue a [`Want`] for the protocol loop to pick up.

use ironrdp_cliprdr::backend::CliprdrBackend;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
};
use ironrdp_core::impl_as_any;

use runtime::text::{to_crlf, to_lf};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// The clipboard size limit in either direction, in bytes of the text as
/// held on this side (UTF-8, LF line ends): the limit every backend applies
/// to the far end's clipboard, and the one FreeRDP's C uses as
/// `VENTO_MAX_CLIPBOARD`. The phone reads its own clipboard up to 1 Mi
/// characters (`SessionClipboard`), which beyond ASCII is more bytes than
/// this; such a copy is not offered.
const MAX_CLIPBOARD: usize = runtime::MAX_CLIPBOARD;

/// A message only the channel can encode, queued for the protocol thread.
#[derive(Debug)]
pub enum Want {
    /// Announce the formats this end holds.
    ///
    /// Also the channel's handshake: the client half stays in its
    /// initialisation state until a format list has been sent *and*
    /// acknowledged, and a paste before that is refused. So this is sent even
    /// when the list is empty, meaning "nothing here".
    Advertise,
    /// Request the remote's copied data, in a format it offered.
    Paste(ClipboardFormatId),
    /// Answer the remote's request for this end's data.
    Send(ClipboardFormatId),
}

/// A session's clipboard state.
#[derive(Debug, Default)]
pub struct Clipboard {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// What this end last copied, or none if it is not being shared.
    ours: Option<String>,
    /// The format requested from the remote, so a response can be decoded
    /// accordingly: a `FormatDataResponse` carries bytes and no format.
    pending: Option<ClipboardFormatId>,
    wants: VecDeque<Want>,
    incoming: VecDeque<String>,
}

impl Clipboard {
    /// Any thread: this end copied something.
    pub fn set_local(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        if text.len() > MAX_CLIPBOARD {
            log::warn!(
                "this end's clipboard is over the {MAX_CLIPBOARD} bytes it offers; not offered"
            );
            return;
        }
        let mut state = self.state.lock().unwrap();
        if state.ours.as_deref() == Some(text) {
            return;
        }
        state.ours = Some(text.to_string());
        state.wants.push_back(Want::Advertise);
    }

    /// The formats this end can produce. Two rather than one so that a server
    /// that asks for `CF_TEXT` gets it: the remote picks one of the offered
    /// formats, and offering only Unicode would make pasting fail on older
    /// software.
    pub fn formats(&self) -> Vec<ClipboardFormat> {
        if self.state.lock().unwrap().ours.is_none() {
            return Vec::new();
        }
        vec![
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
            ClipboardFormat::new(ClipboardFormatId::CF_TEXT),
        ]
    }

    /// This end's text in the requested format, with the line endings a
    /// Windows application expects. `None` means there is nothing to give,
    /// which is sent as an error response rather than empty data.
    pub fn data(&self, format: ClipboardFormatId) -> Option<Vec<u8>> {
        let text = to_crlf(self.state.lock().unwrap().ours.as_deref()?);
        match format {
            ClipboardFormatId::CF_UNICODETEXT => Some(utf16le(&text)),
            ClipboardFormatId::CF_TEXT => Some(latin1(&text)),
            _ => None,
        }
    }

    pub fn take_wants(&self) -> Vec<Want> {
        self.state.lock().unwrap().wants.drain(..).collect()
    }

    pub fn take_incoming(&self) -> Vec<String> {
        self.state.lock().unwrap().incoming.drain(..).collect()
    }

    /// The channel's handle on this, for the connector to hold.
    pub fn backend(self: &Arc<Self>) -> Box<dyn CliprdrBackend> {
        Box::new(Channel {
            shared: Arc::clone(self),
        })
    }
}

/// The callbacks the channel makes, all on the protocol thread, none of
/// them allowed to encode anything.
#[derive(Debug)]
struct Channel {
    shared: Arc<Clipboard>,
}

impl_as_any!(Channel);

impl Channel {
    fn push(&self, want: Want) {
        self.shared.state.lock().unwrap().wants.push_back(want);
    }
}

impl CliprdrBackend for Channel {
    /// Where transferred files would go. Nothing here transfers a file, and the
    /// path is sent regardless during the channel's opening exchange.
    fn temporary_directory(&self) -> &str {
        "."
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        // The alternative is a fixed 32-byte ASCII name per format, as in
        // CLIPRDR before this flag existed. Asking for the newer form costs
        // nothing, and a server without it says so.
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
    }

    fn on_ready(&mut self) {
        log::debug!("the clipboard channel is up");
    }

    fn on_request_format_list(&mut self) {
        self.push(Want::Advertise);
    }

    fn on_process_negotiated_capabilities(&mut self, capabilities: ClipboardGeneralCapabilityFlags) {
        log::debug!("clipboard capabilities: {capabilities:?}");
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        // Unicode first: `CF_TEXT` is in the server's code page, which is
        // unknown here, and every server that offers one offers the other.
        let Some(format) = [
            ClipboardFormatId::CF_UNICODETEXT,
            ClipboardFormatId::CF_TEXT,
        ]
        .into_iter()
        .find(|id| available_formats.iter().any(|f| f.id == *id)) else {
            log::debug!("the remote copied something that is not text");
            return;
        };
        let mut state = self.shared.state.lock().unwrap();
        state.pending = Some(format);
        state.wants.push_back(Want::Paste(format));
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.push(Want::Send(request.format));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        let format = self.shared.state.lock().unwrap().pending.take();
        if response.is_error() {
            log::debug!("the remote could not produce its clipboard");
            return;
        }
        let text = match format {
            Some(ClipboardFormatId::CF_UNICODETEXT) => from_utf16le(response.data()),
            Some(ClipboardFormatId::CF_TEXT) => from_latin1(response.data()),
            // A response with no pending request.
            _ => return,
        };
        let text = to_lf(&text);
        if text.is_empty() {
            return;
        }
        if text.len() > MAX_CLIPBOARD {
            log::warn!(
                "the far end's clipboard is over the {MAX_CLIPBOARD} bytes this end takes; skipped"
            );
            return;
        }
        let mut state = self.shared.state.lock().unwrap();
        // Stored as this end's already, so that the local copy this triggers
        // is not announced straight back to the remote.
        state.ours = Some(text.clone());
        state.incoming.push_back(text);
    }

    fn on_file_contents_request(&mut self, _request: FileContentsRequest) {}

    fn on_file_contents_response(&mut self, _response: FileContentsResponse<'_>) {}

    fn on_lock(&mut self, _data_id: LockDataId) {}

    fn on_unlock(&mut self, _data_id: LockDataId) {}
}

// ---- the two encodings ------------------------------------------------------

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16()
        // NUL-terminated, because a Windows application reads it as a C
        // string and would otherwise paste whatever follows in the buffer.
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn from_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Text for `CF_TEXT`, in the server's code page, which is unknown here:
/// sent as Latin-1, with `?` for anything above U+00FF.
fn latin1(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| if (c as u32) < 0x100 { c as u8 } else { b'?' })
        .chain(std::iter::once(0))
        .collect()
}

fn from_latin1(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round trips, including the terminator that is written and not read back.
    #[test]
    fn text_survives_both_encodings() {
        for text in ["hello", "héllo\nwörld", "", "tabs\tand\r\nnewlines"] {
            assert_eq!(from_utf16le(&utf16le(text)), text);
        }
        assert_eq!(from_latin1(&latin1("héllo")), "héllo");
        // Outside the low 256 code points, which Latin-1 cannot represent.
        assert_eq!(from_latin1(&latin1("日本")), "??");
    }

    /// The far end gets CRLF and this side LF, as FreeRDP's backend does.
    #[test]
    fn line_endings_are_crlf_there_and_lf_here() {
        let clip = Arc::new(Clipboard::default());
        clip.set_local("a\nb\r\nc");
        assert_eq!(
            clip.data(ClipboardFormatId::CF_UNICODETEXT),
            Some(utf16le("a\r\nb\r\nc"))
        );
        let mut channel = clip.backend();
        channel.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
        channel.on_format_data_response(FormatDataResponse::new_data(utf16le("x\r\ny\rz")));
        assert_eq!(clip.take_incoming(), ["x\ny\nz"]);
    }

    /// Both directions apply the shared limit in bytes, not characters: a
    /// text of two-byte characters exceeds it at half the count.
    #[test]
    fn the_bound_is_bytes_both_ways() {
        let clip = Arc::new(Clipboard::default());
        let at = "x".repeat(MAX_CLIPBOARD);
        let over = "é".repeat(MAX_CLIPBOARD / 2 + 1);
        clip.set_local(&over);
        assert!(clip.formats().is_empty());
        clip.set_local(&at);
        assert_eq!(clip.formats().len(), 2);

        let mut channel = clip.backend();
        for (text, arrives) in [(&over, false), (&at, true)] {
            channel.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
            channel.on_format_data_response(FormatDataResponse::new_data(utf16le(text)));
            assert_eq!(clip.take_incoming().len(), usize::from(arrives));
        }
    }

    /// A copy is announced once; the same text again is not.
    #[test]
    fn only_a_change_is_announced() {
        let clip = Clipboard::default();
        clip.set_local("hello");
        clip.set_local("hello");
        assert_eq!(clip.take_wants().len(), 1);
        assert_eq!(clip.formats().len(), 2);
        assert_eq!(
            clip.data(ClipboardFormatId::CF_UNICODETEXT),
            Some(utf16le("hello"))
        );
    }

    /// With nothing to offer, the list is empty rather than absent, since the
    /// channel does not open until one has been sent.
    #[test]
    fn an_empty_clipboard_still_has_a_format_list() {
        let clip = Arc::new(Clipboard::default());
        let mut channel = clip.backend();
        channel.on_request_format_list();
        assert_eq!(clip.take_wants().len(), 1);
        assert!(clip.formats().is_empty());
        assert_eq!(clip.data(ClipboardFormatId::CF_UNICODETEXT), None);
    }
}
