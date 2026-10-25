// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The fixtures, and the clips in them cut into frames the way a protocol
//! delivers them (one encoded frame per call), so IVF is walked by its frame
//! headers and Annex B is cut at its access unit delimiters, rather than
//! handed to a demuxer that would do framing a protocol does not. Shared
//! by the test binaries that decode them.

#![allow(dead_code)]

use runtime::media::video::{Changes, Codec, Decoder, Output};

/// The fixtures' size.
pub const W: usize = 128;
pub const H: usize = 128;

pub fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// IVF's frames: a 32-byte file header (whose own length is at byte 6), then
/// per frame a 4-byte size, an 8-byte timestamp and the frame.
pub fn ivf(bytes: &[u8]) -> Vec<&[u8]> {
    assert_eq!(&bytes[..4], b"DKIF");
    let mut at = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    let mut frames = Vec::new();
    while at + 12 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        at += 12;
        frames.push(&bytes[at..at + size]);
        at += size;
    }
    frames
}

/// An Annex B stream cut into access units at each delimiter, start code
/// included, which is what one of RustDesk's H.264 or H.265 frames is.
pub fn annex_b(bytes: &[u8], is_delimiter: fn(u8) -> bool) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 < bytes.len() {
        if bytes[i..i + 3] == [0, 0, 1] {
            // A four-byte start code begins at the zero before.
            let code = if i > 0 && bytes[i - 1] == 0 { i - 1 } else { i };
            if is_delimiter(bytes[i + 3]) {
                starts.push(code);
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    assert!(!starts.is_empty(), "no access unit delimiters");
    starts.push(bytes.len());
    starts.windows(2).map(|w| &bytes[w[0]..w[1]]).collect()
}

pub fn h264_aud(header: u8) -> bool {
    header & 0x1f == 9
}

pub fn h265_aud(header: u8) -> bool {
    (header >> 1) & 0x3f == 35
}

/// A fixture's codec, which its name says, and its frames.
pub fn frames(name: &str) -> (Codec, Vec<Vec<u8>>) {
    let bytes = fixture(name);
    let (codec, frames) = match name.rsplit('.').next() {
        Some("ivf") if name.starts_with("vp8") => (Codec::Vp8, ivf(&bytes)),
        Some("ivf") if name.starts_with("vp9") => (Codec::Vp9, ivf(&bytes)),
        Some("ivf") => (Codec::Av1, ivf(&bytes)),
        Some("h264") => (Codec::H264, annex_b(&bytes, h264_aud)),
        Some("h265") => (Codec::H265, annex_b(&bytes, h265_aud)),
        _ => panic!("{name}: which codec?"),
    };
    (codec, frames.into_iter().map(<[u8]>::to_vec).collect())
}

/// Every frame of the clip decoded whole, each picture as it came out.
pub fn pictures(decoder: &mut dyn Decoder, frames: &[Vec<u8>]) -> Vec<Vec<u32>> {
    let mut out = Vec::new();
    for frame in frames {
        let mut picture = vec![0u32; W * H];
        let mut changed = Changes::new();
        changed.reset(H);
        for row in 0..H {
            changed.mark(row, 0, W);
        }
        assert!(
            decoder.decode(frame, &mut picture, W, H, &mut changed),
            "no picture came out"
        );
        out.push(picture);
    }
    out
}

/// The largest difference of any channel of any pixel.
pub fn difference(a: &[u32], b: &[u32]) -> i32 {
    a.iter()
        .zip(b)
        .flat_map(|(a, b)| {
            (0..3).map(move |c| ((a >> (8 * c)) & 0xff) as i32 - ((b >> (8 * c)) & 0xff) as i32)
        })
        .map(i32::abs)
        .max()
        .unwrap_or(0)
}

/// Every frame of the clip decoded to its planes, each picture converted
/// whole afterwards as [`pictures`] has the decoder convert it: what
/// FreeRDP's subsystem is handed, made comparable.
pub fn planes_pictures(decoder: &mut dyn Decoder, frames: &[Vec<u8>]) -> Vec<Vec<u32>> {
    let mut output = Output::new();
    let mut out = Vec::new();
    for frame in frames {
        let mut picture = vec![0u32; W * H];
        let mut changed = Changes::new();
        changed.reset(H);
        for row in 0..H {
            changed.mark(row, 0, W);
        }
        let mut handed = 0;
        assert!(
            decoder.decode_planes(frame, &mut |planes| {
                handed += 1;
                output.write(planes, &mut picture, W, H, &mut changed);
            }),
            "no planes came out"
        );
        assert_eq!(handed, 1, "one picture a frame");
        out.push(picture);
    }
    out
}
