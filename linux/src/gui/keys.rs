// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A winit key as the far end wants it: an X11 keysym, and an identity for
//! the physical key that ties a release to its press.
//!
//! The keysym comes from the *logical* key, which is what the layout says
//! the key means — a `z` on a German keyboard is `y` at the far end too,
//! which is what somebody typing expects. A character is its Latin-1 keysym
//! where it has one and the Unicode keysym (`0x01000000 | codepoint`)
//! otherwise, which is `xkeysym`'s `from_char`; a named key is looked up in
//! a table of the ones a desktop has a use for. Modifiers are told apart by
//! side, since the far end's applications can be, and the number pad by
//! location, since `KP_1` and `1` are different keys to the same
//! applications.
//!
//! The identity is per physical key rather than per keysym, because the
//! keysym of a key can change while it is held — Shift released before the
//! letter is — and the release must still find its press. `KeyIds` hands out
//! a small integer per `PhysicalKey` as they are first seen, which is stable
//! for the life of the process and is all the backends ask of it.

use std::collections::HashMap;
use winit::event::KeyEvent;
use winit::keyboard::{Key, KeyLocation, NamedKey, PhysicalKey};
use xkeysym::Keysym;

/// The keysym for a key event, or `None` for a key with no keysym worth
/// sending (a dead key, or something winit could not name).
pub fn keysym(event: &KeyEvent) -> Option<u32> {
    let right = event.location == KeyLocation::Right;
    let numpad = event.location == KeyLocation::Numpad;
    let sym = match &event.logical_key {
        Key::Character(text) => {
            let c = text.chars().next()?;
            if numpad {
                keypad(c)
            } else {
                Keysym::from_char(c)
            }
        }
        Key::Named(named) => named_keysym(*named, right, numpad)?,
        Key::Dead(_) | Key::Unidentified(_) => return None,
    };
    (sym != Keysym::NoSymbol).then_some(sym.raw())
}

/// The number pad's own keysyms for the characters it types.
fn keypad(c: char) -> Keysym {
    match c {
        '0'..='9' => Keysym::new(Keysym::KP_0.raw() + (c as u32 - '0' as u32)),
        '.' => Keysym::KP_Decimal,
        ',' => Keysym::KP_Separator,
        '+' => Keysym::KP_Add,
        '-' => Keysym::KP_Subtract,
        '*' => Keysym::KP_Multiply,
        '/' => Keysym::KP_Divide,
        '=' => Keysym::KP_Equal,
        other => Keysym::from_char(other),
    }
}

fn named_keysym(key: NamedKey, right: bool, numpad: bool) -> Option<Keysym> {
    let side = |left: Keysym, right_sym: Keysym| if right { right_sym } else { left };
    Some(match key {
        NamedKey::Alt => side(Keysym::Alt_L, Keysym::Alt_R),
        NamedKey::AltGraph => Keysym::ISO_Level3_Shift,
        NamedKey::CapsLock => Keysym::Caps_Lock,
        NamedKey::Control => side(Keysym::Control_L, Keysym::Control_R),
        NamedKey::NumLock => Keysym::Num_Lock,
        NamedKey::ScrollLock => Keysym::Scroll_Lock,
        NamedKey::Shift => side(Keysym::Shift_L, Keysym::Shift_R),
        NamedKey::Meta | NamedKey::Super => side(Keysym::Super_L, Keysym::Super_R),
        NamedKey::Hyper => side(Keysym::Hyper_L, Keysym::Hyper_R),
        NamedKey::Enter => {
            if numpad {
                Keysym::KP_Enter
            } else {
                Keysym::Return
            }
        }
        NamedKey::Tab => Keysym::Tab,
        NamedKey::Space => Keysym::space,
        NamedKey::ArrowDown => Keysym::Down,
        NamedKey::ArrowLeft => Keysym::Left,
        NamedKey::ArrowRight => Keysym::Right,
        NamedKey::ArrowUp => Keysym::Up,
        NamedKey::End => Keysym::End,
        NamedKey::Home => Keysym::Home,
        NamedKey::PageDown => Keysym::Page_Down,
        NamedKey::PageUp => Keysym::Page_Up,
        NamedKey::Backspace => Keysym::BackSpace,
        NamedKey::Clear => Keysym::Clear,
        NamedKey::Copy => Keysym::XF86_Copy,
        NamedKey::Cut => Keysym::XF86_Cut,
        NamedKey::Paste => Keysym::XF86_Paste,
        NamedKey::Delete => Keysym::Delete,
        NamedKey::Insert => Keysym::Insert,
        NamedKey::Undo => Keysym::Undo,
        NamedKey::Redo => Keysym::Redo,
        NamedKey::Cancel => Keysym::Cancel,
        NamedKey::ContextMenu => Keysym::Menu,
        NamedKey::Escape => Keysym::Escape,
        NamedKey::Execute => Keysym::Execute,
        NamedKey::Find => Keysym::Find,
        NamedKey::Help => Keysym::Help,
        NamedKey::Pause => Keysym::Pause,
        NamedKey::Select => Keysym::Select,
        NamedKey::PrintScreen => Keysym::Print,
        NamedKey::Compose => Keysym::Multi_key,
        NamedKey::Convert => Keysym::Henkan,
        NamedKey::NonConvert => Keysym::Muhenkan,
        NamedKey::ModeChange => Keysym::Mode_switch,
        NamedKey::HangulMode => Keysym::Hangul,
        NamedKey::HanjaMode => Keysym::Hangul_Hanja,
        NamedKey::Eisu => Keysym::Eisu_toggle,
        NamedKey::Hankaku => Keysym::Hankaku,
        NamedKey::Hiragana => Keysym::Hiragana,
        NamedKey::HiraganaKatakana | NamedKey::KanaMode => Keysym::Hiragana_Katakana,
        NamedKey::Katakana => Keysym::Katakana,
        NamedKey::Romaji => Keysym::Romaji,
        NamedKey::Zenkaku => Keysym::Zenkaku,
        NamedKey::ZenkakuHankaku => Keysym::Zenkaku_Hankaku,
        NamedKey::AudioVolumeDown => Keysym::XF86_AudioLowerVolume,
        NamedKey::AudioVolumeUp => Keysym::XF86_AudioRaiseVolume,
        NamedKey::AudioVolumeMute => Keysym::XF86_AudioMute,
        NamedKey::MediaPlayPause | NamedKey::MediaPlay => Keysym::XF86_AudioPlay,
        NamedKey::MediaPause => Keysym::XF86_AudioPause,
        NamedKey::MediaStop => Keysym::XF86_AudioStop,
        NamedKey::MediaTrackNext => Keysym::XF86_AudioNext,
        NamedKey::MediaTrackPrevious => Keysym::XF86_AudioPrev,
        NamedKey::BrowserBack => Keysym::XF86_Back,
        NamedKey::BrowserForward => Keysym::XF86_Forward,
        NamedKey::BrowserHome => Keysym::XF86_HomePage,
        NamedKey::BrowserRefresh => Keysym::XF86_Refresh,
        NamedKey::BrowserSearch => Keysym::XF86_Search,
        NamedKey::BrowserStop => Keysym::XF86_Stop,
        NamedKey::BrowserFavorites => Keysym::XF86_Favorites,
        NamedKey::LaunchMail => Keysym::XF86_Mail,
        NamedKey::Eject => Keysym::XF86_Eject,
        NamedKey::Power => Keysym::XF86_PowerOff,
        NamedKey::Standby => Keysym::XF86_Sleep,
        NamedKey::WakeUp => Keysym::XF86_WakeUp,
        NamedKey::F1 => Keysym::F1,
        NamedKey::F2 => Keysym::F2,
        NamedKey::F3 => Keysym::F3,
        NamedKey::F4 => Keysym::F4,
        NamedKey::F5 => Keysym::F5,
        NamedKey::F6 => Keysym::F6,
        NamedKey::F7 => Keysym::F7,
        NamedKey::F8 => Keysym::F8,
        NamedKey::F9 => Keysym::F9,
        NamedKey::F10 => Keysym::F10,
        NamedKey::F11 => Keysym::F11,
        NamedKey::F12 => Keysym::F12,
        NamedKey::F13 => Keysym::F13,
        NamedKey::F14 => Keysym::F14,
        NamedKey::F15 => Keysym::F15,
        NamedKey::F16 => Keysym::F16,
        NamedKey::F17 => Keysym::F17,
        NamedKey::F18 => Keysym::F18,
        NamedKey::F19 => Keysym::F19,
        NamedKey::F20 => Keysym::F20,
        NamedKey::F21 => Keysym::F21,
        NamedKey::F22 => Keysym::F22,
        NamedKey::F23 => Keysym::F23,
        NamedKey::F24 => Keysym::F24,
        _ => return None,
    })
}

/// A stable small integer per physical key, handed out as keys are first
/// seen. Only ever grows, by one entry per distinct key on the keyboard.
#[derive(Default)]
pub struct KeyIds {
    ids: HashMap<PhysicalKey, i64>,
}

impl KeyIds {
    pub fn id(&mut self, key: PhysicalKey) -> i64 {
        let next = self.ids.len() as i64 + 1;
        *self.ids.entry(key).or_insert(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;

    #[test]
    fn characters_are_latin_1_or_unicode_keysyms() {
        assert_eq!(Keysym::from_char('a').raw(), 0x61);
        assert_eq!(Keysym::from_char('é').raw(), 0xe9);
        assert_eq!(Keysym::from_char('€').raw(), 0x20ac);
        // A character with a legacy keysym gets that; one without gets the
        // Unicode keysym.
        assert_eq!(Keysym::from_char('я').raw(), Keysym::Cyrillic_ya.raw());
        assert_eq!(Keysym::from_char('😀').raw(), 0x0101_f600);
        assert_eq!(keypad('7').raw(), Keysym::KP_7.raw());
        assert_eq!(keypad('.').raw(), Keysym::KP_Decimal.raw());
    }

    #[test]
    fn named_keys_tell_their_side_and_pad_apart() {
        assert_eq!(named_keysym(NamedKey::Shift, false, false), Some(Keysym::Shift_L));
        assert_eq!(named_keysym(NamedKey::Shift, true, false), Some(Keysym::Shift_R));
        assert_eq!(named_keysym(NamedKey::Enter, false, true), Some(Keysym::KP_Enter));
        assert_eq!(named_keysym(NamedKey::Enter, false, false), Some(Keysym::Return));
        assert_eq!(named_keysym(NamedKey::PageDown, false, false), Some(Keysym::Page_Down));
        assert_eq!(named_keysym(NamedKey::TVPower, false, false), None);
    }

    #[test]
    fn key_ids_are_stable_per_physical_key() {
        let mut ids = KeyIds::default();
        let a = ids.id(PhysicalKey::Code(KeyCode::KeyA));
        let b = ids.id(PhysicalKey::Code(KeyCode::KeyB));
        assert_ne!(a, b);
        assert_eq!(ids.id(PhysicalKey::Code(KeyCode::KeyA)), a);
        assert!(a > 0 && b > 0);
    }
}
