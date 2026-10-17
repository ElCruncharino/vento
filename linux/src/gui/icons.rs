// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The toolbar's icons: Google's Material Symbols, Outlined and unfilled,
//! as a font family of their own, and the codepoints of the symbols the
//! toolbar draws.
//!
//! The font is `assets/material-symbols/MaterialSymbolsOutlined-Regular.ttf`,
//! Material Symbols Outlined 2.975 from google/material-design-icons at
//! commit 49d4db35df873165d6bd6ba09b063c7dafbac2f4 (2026-10-09), Apache
//! License 2.0 (`LICENSE` beside it). That repository ships Material Symbols
//! only as a variable font, and egui can set none of a variable font's axes,
//! so what is here is a static instance of its
//! `variablefont/MaterialSymbolsOutlined[FILL,GRAD,opsz,wght].ttf`, made with
//! fontTools at Google Fonts' defaults but unfilled:
//!
//! ```sh
//! fonttools varLib.instancer --static --update-name-table \
//!     -o MaterialSymbolsOutlined-Regular.ttf \
//!     'MaterialSymbolsOutlined[FILL,GRAD,opsz,wght].ttf' FILL=0 GRAD=0 opsz=24 wght=400
//! ```
//!
//! `MaterialSymbolsOutlined.codepoints` is that commit's codepoints file,
//! renamed, where a symbol's name finds its codepoint; the tests check every
//! symbol below against it and against the font. The whole font is kept,
//! not the toolbar's few glyphs, so that a new button needs only a line
//! here.

use egui::{FontData, FontFamily, RichText};
use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};

/// The font itself, built into the binary.
const FONT: &[u8] = include_bytes!("../../assets/material-symbols/MaterialSymbolsOutlined-Regular.ttf");

/// The font family's name in egui.
const FAMILY: &str = "material-symbols-outlined";

/// The font as a family of its own and nothing else's. Not a fallback of
/// the proportional family: there any character egui's own fonts lack would
/// come out in it, and the rest of the window keeps the fonts it had.
pub fn font_insert() -> FontInsert {
    let mut data = FontData::from_static(FONT);
    // The symbols sit a little high on egui's baseline; this centres them
    // on a button's text line, as egui_material_icons did.
    data.tweak.y_offset_factor = 0.05;
    FontInsert::new(
        FAMILY,
        data,
        vec![InsertFontFamily { family: FontFamily::Name(FAMILY.into()), priority: FontPriority::Highest }],
    )
}

/// A symbol as text in the symbols' family, at the size the caller gives it.
pub fn text(symbol: &'static str) -> RichText {
    RichText::new(symbol).family(FontFamily::Name(FAMILY.into()))
}

macro_rules! symbols {
    ($($constant:ident = $name:literal $codepoint:literal;)*) => {
        $(
            #[doc = concat!("Material Symbols' `", $name, "`.")]
            pub const $constant: &str = $codepoint;
        )*

        /// Each symbol's name and codepoint, for the tests.
        #[cfg(test)]
        const ALL: &[(&str, &str)] = &[$(($name, $constant)),*];
    };
}

symbols! {
    FIT_PAGE_HEIGHT = "fit_page_height" "\u{f397}";
    FIT_PAGE_WIDTH = "fit_page_width" "\u{f396}";
    FULLSCREEN = "fullscreen" "\u{e5d0}";
    FULLSCREEN_EXIT = "fullscreen_exit" "\u{e5d1}";
    HEIGHT = "height" "\u{ea16}";
    INFO = "info" "\u{e88e}";
    LINK_OFF = "link_off" "\u{e16f}";
    MONITOR = "monitor" "\u{ef5b}";
    MOUSE = "mouse" "\u{e323}";
    OPEN_IN_NEW = "open_in_new" "\u{e89e}";
    PIP_EXIT = "pip_exit" "\u{f70d}";
    TV_DISPLAYS = "tv_displays" "\u{f3ec}";
    VIEW_REAL_SIZE = "view_real_size" "\u{f4c2}";
    VISIBILITY = "visibility" "\u{e8f4}";
    VOLUME_OFF = "volume_off" "\u{e04f}";
    VOLUME_UP = "volume_up" "\u{e050}";
    WIDTH = "width" "\u{f730}";
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODEPOINTS: &str = include_str!("../../assets/material-symbols/MaterialSymbolsOutlined.codepoints");

    #[test]
    fn every_symbol_is_the_codepoints_files() {
        for &(name, symbol) in ALL {
            let listed = CODEPOINTS
                .lines()
                .filter_map(|line| line.split_once(' '))
                .find(|&(n, _)| n == name)
                .map(|(_, hex)| char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap());
            assert_eq!(listed, symbol.chars().next(), "{name}");
        }
    }

    #[test]
    fn every_symbol_is_in_the_font() {
        let face = ttf_parser::Face::parse(FONT, 0).unwrap();
        // A static instance: no axis left for egui to miss.
        assert!(!face.is_variable());
        for &(name, symbol) in ALL {
            let c = symbol.chars().next().unwrap();
            assert!(face.glyph_index(c).is_some(), "{name} is not in the font");
        }
    }
}
