// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Log lines on stderr, written by `cpp/vento_log.c`, the same C the backends'
//! libraries link for their `vento_log`: `HH:MM:SS.mmm L tag: text`, the time
//! local and to the millisecond. The client's logger writes its lines through
//! [`write_line`], and [`route_ffmpeg`] sends FFmpeg's there too, so every log
//! line the client writes has one layout and one clock.

use std::ffi::c_char;

unsafe extern "C" {
    fn vento_log_line(level: c_char, tag: *const c_char, tag_len: usize, text: *const c_char, text_len: usize);
    fn vento_video_log_to_stderr();
}

/// Writes one line: `level` is its letter (`W`, `I`, `D` or `T`), `tag` names
/// what wrote it. A newline in `text` is written as it is.
pub fn write_line(level: u8, tag: &str, text: &str) {
    // SAFETY: both pointers are valid for their lengths for the call, and the
    // C reads them as bytes, with no terminator.
    unsafe {
        vento_log_line(
            level as c_char,
            tag.as_ptr().cast(),
            tag.len(),
            text.as_ptr().cast(),
            text.len(),
        )
    }
}

/// Sends FFmpeg's log lines through [`write_line`] under the tag `ffmpeg`
/// rather than to stderr in FFmpeg's own layout. Its level stays at info.
pub fn route_ffmpeg() {
    // SAFETY: sets a function pointer in libavutil; callable at any time.
    unsafe { vento_video_log_to_stderr() }
}
