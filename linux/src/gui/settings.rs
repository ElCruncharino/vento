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

use crate::backends::Hardware;
use egui::{RichText, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Send locally copied text to the far end: `KEY_CLIPBOARD_OUT`.
    pub clipboard_out: bool,
    /// Update the local clipboard with what the far end copies:
    /// `KEY_CLIPBOARD_IN`.
    pub clipboard_in: bool,
    /// In absolute mode, draw the far end's cursor shape into the picture
    /// under the local pointer, with the OS cursor hidden over the desktop,
    /// as the phone does, rather than making it the window's own cursor.
    pub software_cursor: bool,
    /// Hold the window manager's or the compositor's shortcuts off while a
    /// session has the focus and takes input — the X11 keyboard grab, or
    /// Wayland's shortcuts inhibitor — so that Alt+Tab and the rest reach
    /// the far end. Off, neither is ever taken and they stay this
    /// desktop's.
    pub grab_keyboard: bool,
    /// Decode video on the GPU where it can be, through VAAPI (`vaapi`, or
    /// `auto`, which is the same). Off, the default, no device is opened and
    /// libva is not loaded. Like the rest, not saved: a run that wants it
    /// asks for it (`--hw-decode`) or switches it on here.
    pub hw_decode: Hardware,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            clipboard_out: true,
            clipboard_in: true,
            software_cursor: false,
            grab_keyboard: true,
            hw_decode: Hardware::Off,
        }
    }
}

impl Settings {
    /// The five as rows of a two-column grid — a label and a box each, the
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
        // With the clipboard's, as the other switch about what crosses
        // between the two desktops: here, the keyboard's chords.
        row(
            ui,
            &mut self.grab_keyboard,
            "Grab keyboard",
            "Send the window manager's shortcuts (Alt+Tab, Super and the rest) to the remote machine \
             while the session has the focus and takes input. Off, they stay this desktop's.",
        );
        row(
            ui,
            &mut self.software_cursor,
            "Software cursor",
            "Draw the far end's cursor shape into the picture under the pointer, hiding this desktop's \
             own, instead of making it this window's pointer. Only where this end owns the pointer; in \
             relative mode the far end draws its cursor into the picture either way.",
        );
        // A box like the others, on meaning `auto`; `vaapi` from the command
        // line, which decodes the same, is kept until the box is switched
        // off. Which backends use it is said in the summary, which grows as
        // more of them decode through it.
        let label = if weak { RichText::new("Hardware decoding").weak() } else { RichText::new("Hardware decoding") };
        ui.label(label).on_hover_text(
            "Decode video on the GPU (VAAPI) where the device takes the stream, and in software where it \
             does not. RustDesk and TigerVNC's H.264 switch at once (at the next picture the far end sends \
             whole), and RustDesk's VAAPI pictures are drawn from the GPU without being read back where the \
             window's Vulkan device can; FreeRDP's H.264 takes it at the next connection; the other \
             backends decode as their libraries do and do not use this.",
        );
        let mut on = self.hw_decode != Hardware::Off;
        if ui.checkbox(&mut on, "").changed() {
            self.hw_decode = if on { Hardware::Auto } else { Hardware::Off };
            changed = true;
        }
        ui.end_row();
        changed
    }
}
