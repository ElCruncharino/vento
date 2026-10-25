// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! libavcodec through `runtime::media::video`, against clips whose picture is
//! known: the colour that comes out, which rows a frame says it moved, and that
//! the rows it does not name are left alone. How the clips were made, and what
//! is in them, is `fixtures/generate.sh`. Each clip is fed one encoded frame
//! per call, as `clips` cuts it.

#![cfg(not(target_os = "android"))]

use std::ops::Range;

use runtime::media::video::{Changes, Codec, Decoder, Decoders};
use platform_linux::ffmpeg::FFmpeg;

mod clips;
use clips::{annex_b, fixture, frames, h264_aud, h265_aud, ivf, pictures, planes_pictures};

const W: usize = 128;
const H: usize = 128;
const BACKGROUND: u32 = 0x3366cc;
const BAND: u32 = 0xe0a030;
/// The rows frame 1 paints, and frame 2 repeats.
const BAND_ROWS: Range<usize> = 16..32;
/// What no decode writes unless it means to.
const UNTOUCHED: u32 = 0x0012_3456;

/// How far a pixel may be from the colour it was encoded as, per channel: the
/// encoders are lossy and the round trip through 8-bit YUV is not exact. Two
/// tolerances, because the mean over a flat area is much closer than any one
/// pixel, and is what tells the right matrix from the wrong one: those are
/// four to six levels apart on these colours.
const PIXEL: i32 = 10;
const MEAN: f64 = 2.5;

fn channels(word: u32) -> [i32; 3] {
    // Red at shift 0, as the framebuffer has it.
    [word & 0xff, (word >> 8) & 0xff, (word >> 16) & 0xff].map(|c| c as i32)
}

fn rgb(colour: u32) -> [i32; 3] {
    [(colour >> 16) & 0xff, (colour >> 8) & 0xff, colour & 0xff].map(|c| c as i32)
}

/// Every pixel of these rows near `colour`, and their mean nearer.
fn assert_colour(
    out: &[u32],
    width: usize,
    rows: Range<usize>,
    cols: Range<usize>,
    colour: u32,
    what: &str,
) {
    let want = rgb(colour);
    let mut sum = [0f64; 3];
    let mut n = 0f64;
    for row in rows.clone() {
        for col in cols.clone() {
            let got = channels(out[row * width + col]);
            for c in 0..3 {
                assert!(
                    (got[c] - want[c]).abs() <= PIXEL,
                    "{what}: ({col}, {row}) is {got:?}, wanted about {want:?}"
                );
                sum[c] += got[c] as f64;
            }
            n += 1.0;
        }
    }
    let mean = sum.map(|s| s / n);
    for c in 0..3 {
        assert!(
            (mean[c] - want[c] as f64).abs() <= MEAN,
            "{what}: rows {rows:?} average {mean:?}, wanted {want:?}"
        );
    }
}

/// The contract every decode is held to: a row `changed` names was written,
/// and a row it does not name was not touched.
fn assert_only_changed_rows_written(out: &[u32], changed: &Changes, what: &str) {
    for row in 0..H {
        let line = &out[row * W..row * W + W];
        if changed.row(row) {
            assert!(
                line.iter().all(|p| *p != UNTOUCHED),
                "{what}: row {row} is marked and was not written"
            );
        } else {
            assert!(
                line.iter().all(|p| *p == UNTOUCHED),
                "{what}: row {row} is not marked and was written"
            );
        }
    }
}

fn decode(
    decoder: &mut dyn Decoder,
    frame: &[u8],
    out: &mut [u32],
    changed: &mut Changes,
    what: &str,
) {
    assert!(
        decoder.decode(frame, out, W, H, changed),
        "{what}: no picture came out"
    );
}

/// The three frames of a clip, and everything they are meant to show.
fn clip(codec: Codec, frames: &[&[u8]]) {
    let name = codec.name();
    assert_eq!(frames.len(), 3, "{name}: the clip is three frames");
    assert!(
        FFmpeg::default().decodable(codec),
        "{name}: this libavcodec cannot decode it"
    );
    let mut decoder = FFmpeg::default().decoder(codec, W, H).expect("a decoder");
    let mut out = vec![UNTOUCHED; W * H];
    let mut changed = Changes::new();

    // The first frame is all of it, since there is nothing to compare it with.
    changed.reset(H);
    decode(decoder.as_mut(), frames[0], &mut out, &mut changed, name);
    assert_eq!(changed.bounds(W), Some((0, 0, W, H)), "{name}: frame 0");
    assert_colour(&out, W, 0..H, 0..W, BACKGROUND, name);

    // The second moves the band, and whatever around it the encoder chose to
    // touch up inside the same block, but nothing in the bottom half, which
    // no block of the band's reaches; and whatever it does not name is left as
    // it was.
    out.fill(UNTOUCHED);
    changed.reset(H);
    decode(decoder.as_mut(), frames[1], &mut out, &mut changed, name);
    assert_only_changed_rows_written(&out, &changed, name);
    for row in BAND_ROWS {
        assert!(changed.row(row), "{name}: frame 1 did not mark row {row}");
    }
    for row in H / 2..H {
        assert!(
            !changed.row(row),
            "{name}: frame 1 marked row {row}, which did not move"
        );
    }
    assert_colour(
        &out,
        W,
        BAND_ROWS.start + 2..BAND_ROWS.end - 2,
        0..W,
        BAND,
        name,
    );

    // The third repeats the second. A row marked on the way in is converted
    // whether it moved or not (which is how a caller whose buffer is out of
    // step asks for it), and still nothing unmarked is written.
    out.fill(UNTOUCHED);
    changed.reset(H);
    changed.mark(H - 4, 0, W);
    decode(decoder.as_mut(), frames[2], &mut out, &mut changed, name);
    assert_only_changed_rows_written(&out, &changed, name);
    assert_colour(&out, W, H - 4..H - 3, 0..W, BACKGROUND, name);
}

#[test]
fn vp9() {
    clip(Codec::Vp9, &ivf(&fixture("vp9.ivf")));
}

#[test]
fn vp8() {
    clip(Codec::Vp8, &ivf(&fixture("vp8.ivf")));
}

#[test]
fn av1() {
    clip(Codec::Av1, &ivf(&fixture("av1.ivf")));
}

/// What a RustDesk peer sends: BT.601 limited range with nothing in the
/// stream saying so. Taken for BT.709, the background comes out seven levels
/// short in red and four over in blue, and the rig's red warning bar a dozen
/// or more too light.
#[test]
fn vp9_untagged_is_bt601() {
    clip(Codec::Vp9, &ivf(&fixture("vp9-untagged.ivf")));
}

/// 4:4:4, which VAAPI leaves to software (see `hardware.rs`).
#[test]
fn av1_444() {
    clip(Codec::Av1, &ivf(&fixture("av1-444.ivf")));
}

#[test]
fn av1_untagged_is_bt601() {
    clip(Codec::Av1, &ivf(&fixture("av1-untagged.ivf")));
}

#[test]
fn h264() {
    clip(Codec::H264, &annex_b(&fixture("h264.h264"), h264_aud));
}

/// Planes, for FreeRDP's subsystem: one picture a frame, and each the
/// picture `decode` converts, exactly, once converted the same way.
#[test]
fn h264_planes_are_its_pictures() {
    let (codec, frames) = frames("h264.h264");
    let mut converted = FFmpeg::default().decoder(codec, W, H).expect("a decoder");
    let mut planes = FFmpeg::default().decoder(codec, W, H).expect("a decoder");
    let want = pictures(converted.as_mut(), &frames);
    let got = planes_pictures(planes.as_mut(), &frames);
    assert_eq!(want.len(), got.len());
    for (i, (want, got)) in want.iter().zip(&got).enumerate() {
        assert!(want == got, "picture {i} differs");
    }
}

#[test]
fn h265() {
    clip(Codec::H265, &annex_b(&fixture("h265.h265"), h265_aud));
}

/// The same picture as BT.601 full range, which is a long way from BT.709
/// limited, so a decoder that ignored the stream's tags fails on colour.
#[test]
fn h264_bt601_full_range() {
    clip(
        Codec::H264,
        &annex_b(&fixture("h264-bt601-full.h264"), h264_aud),
    );
}

/// Ten bits per sample, narrowed to eight on the way: a peer's encoder can be
/// set to send High 10, and refusing it would leave the session with no
/// picture from then on.
#[test]
fn h264_high10() {
    clip(
        Codec::H264,
        &annex_b(&fixture("h264-high10.h264"), h264_aud),
    );
}

/// The same for 4:4:4, whose chroma is not subsampled.
#[test]
fn vp9_444_ten_bits() {
    clip(Codec::Vp9, &ivf(&fixture("vp9-444-10bit.ivf")));
}

/// H.265 Main 10 and Main 4:4:4 10, narrowed the same way: what hardware
/// decodes these into (P010, XV30) is `hardware.rs`'s.
#[test]
fn h265_main10() {
    clip(
        Codec::H265,
        &annex_b(&fixture("h265-main10.h265"), h265_aud),
    );
}

#[test]
fn h265_444_ten_bits() {
    clip(
        Codec::H265,
        &annex_b(&fixture("h265-444-10bit.h265"), h265_aud),
    );
}

/// A picture that is not the session's size, which is every frame between a
/// resize being asked for and arriving: what the two have in common is
/// written, and nothing outside either.
#[test]
fn a_picture_of_another_size_writes_what_overlaps() {
    let bytes = fixture("vp9.ivf");
    let frames = ivf(&bytes);
    for (width, height) in [(W + 16, H - 16), (W - 16, H + 16)] {
        let what = format!("a {width}x{height} session");
        let mut decoder = FFmpeg::default()
            .decoder(Codec::Vp9, width, height)
            .expect("a decoder");
        let mut out = vec![UNTOUCHED; width * height];
        let mut changed = Changes::new();
        changed.reset(height);
        assert!(
            decoder.decode(frames[0], &mut out, width, height, &mut changed),
            "{what}"
        );
        let (w, h) = (width.min(W), height.min(H));
        assert_colour(&out, width, 0..h, 0..w, BACKGROUND, &what);
        for row in 0..height {
            for col in 0..width {
                if row >= h || col >= w {
                    assert_eq!(out[row * width + col], UNTOUCHED, "{what}: ({col}, {row})");
                }
            }
        }
    }
}

/// Garbage is refused rather than taken for a picture, and the decoder still
/// decodes afterwards.
#[test]
fn garbage_is_no_picture() {
    let bytes = fixture("h264.h264");
    let frames = annex_b(&bytes, h264_aud);
    let mut decoder = FFmpeg::default().decoder(Codec::H264, W, H).expect("a decoder");
    let mut out = vec![UNTOUCHED; W * H];
    let mut changed = Changes::new();
    changed.reset(H);
    assert!(!decoder.decode(
        &[0, 0, 0, 1, 0x65, 0xff, 0xff],
        &mut out,
        W,
        H,
        &mut changed
    ));
    assert!(out.iter().all(|p| *p == UNTOUCHED));
    decode(
        decoder.as_mut(),
        frames[0],
        &mut out,
        &mut changed,
        "after garbage",
    );
    assert_colour(&out, W, 0..H, 0..W, BACKGROUND, "after garbage");
}
