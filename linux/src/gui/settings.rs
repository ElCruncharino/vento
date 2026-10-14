// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The client's own settings: the phone's `AppSettings`, as far as this
//! window has any — one answer for every connection rather than a field of
//! one, and nothing saved, since there is nowhere this client saves.
//!
//! They are drawn in two places with one function: a section of the
//! connection dialog, which is where they are set before a session, and the
//! connection panel, where a session shows what it is running with and
//! takes a change at once. The session owns a copy while it runs and hands
//! it back when it ends, so what was switched mid-session is what the next
//! one starts with.

use egui::{RichText, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Send locally copied text to the far end: `KEY_CLIPBOARD_OUT`.
    pub clipboard_out: bool,
    /// Update the local clipboard with what the far end copies:
    /// `KEY_CLIPBOARD_IN`.
    pub clipboard_in: bool,
    /// In absolute mode, make the far end's cursor shape the window's own
    /// cursor rather than drawing it under the local pointer.
    pub native_cursor: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            clipboard_out: true,
            clipboard_in: true,
            native_cursor: false,
        }
    }
}

impl Settings {
    /// The three as rows of a two-column grid — a label and a box each, the
    /// shape the dialog's and the panel's options are drawn in, with the
    /// label `weak` where the panel's are. True if one was changed this
    /// frame.
    pub fn rows(&mut self, ui: &mut Ui, weak: bool) -> bool {
        let mut changed = false;
        let mut row = |ui: &mut Ui, on: &mut bool, label: &str, summary: &str| {
            let label = if weak { RichText::new(label).weak() } else { RichText::new(label) };
            ui.label(label).on_hover_text(summary);
            changed |= ui.checkbox(on, "").changed();
            ui.end_row();
        };
        row(
            ui,
            &mut self.clipboard_out,
            "Send clipboard",
            "Send locally copied text to the remote machine.",
        );
        row(
            ui,
            &mut self.clipboard_in,
            "Receive clipboard",
            "Update the local clipboard with copied text.",
        );
        row(
            ui,
            &mut self.native_cursor,
            "Native cursor",
            "Use the far end's cursor shape as this window's own pointer, with its hotspot, instead of \
             drawing it under the local one. Only where this end owns the pointer; in relative mode \
             the far end draws its cursor into the picture.",
        );
        changed
    }
}
