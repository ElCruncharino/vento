// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rows of `R, G, B, A` as a binary PPM, which is the one picture format that
//! needs no library and that every viewer and `magick` reads. What the C
//! probe writes, written again here so the two can be compared byte for
//! byte.

use crate::backend::Picture;
use std::io::{self, Write};
use std::path::Path;

/// `P6`, then the pixels with their alpha dropped.
pub fn encode(picture: &Picture) -> Vec<u8> {
    let mut out = format!("P6\n{} {}\n255\n", picture.width, picture.height).into_bytes();
    out.reserve(picture.width * picture.height * 3);
    for px in picture.rgba.chunks_exact(4) {
        out.extend_from_slice(&px[..3]);
    }
    out
}

pub fn write(path: &Path, picture: &Picture) -> io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(&encode(picture))?;
    file.flush()
}

/// `FILE.ppm` to `FILE-thumb.ppm`, as the C probe names its thumbnail — and
/// `FILE` to `FILE-thumb` where there is no extension to go after.
pub fn thumbnail_path(out: &Path) -> std::path::PathBuf {
    let name = out.file_name().and_then(|n| n.to_str()).unwrap_or("out.ppm");
    let thumb = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}-thumb.{ext}"),
        _ => format!("{name}-thumb"),
    };
    out.with_file_name(thumb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_and_the_pixels_are_what_the_c_probe_writes() {
        let picture = Picture {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 0],
        };
        assert_eq!(encode(&picture), b"P6\n2 1\n255\n\x01\x02\x03\x04\x05\x06");
    }

    #[test]
    fn the_thumbnail_goes_beside_the_picture() {
        assert_eq!(
            thumbnail_path(Path::new("/x/shot.ppm")),
            Path::new("/x/shot-thumb.ppm")
        );
        assert_eq!(thumbnail_path(Path::new("shot")), Path::new("shot-thumb"));
        assert_eq!(
            thumbnail_path(Path::new("/a.b/shot")),
            Path::new("/a.b/shot-thumb")
        );
    }
}
