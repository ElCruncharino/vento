// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! `auto`, per stream: VAAPI where it takes the stream, software where it
//! cannot (no device, or no decoder for the profile), with VAAPI's reason
//! alone.
//!
//! The same conventions as `hardware.rs`: what a machine without the device
//! cannot show is skipped with a note, `VENTO_TEST_HW=require` makes a
//! missing VAAPI device a failure, and `VENTO_TEST_HW=absent` is a run with
//! `/dev/dri` or libva hidden.

#![cfg(not(target_os = "android"))]

use common::video::Decoder as _;
use platform_linux::ffmpeg::{Decoder, Hardware};

mod clips;
use clips::{H, W, difference, frames, pictures};

fn mode() -> String {
    std::env::var("VENTO_TEST_HW").unwrap_or_default()
}

/// The clip decoded in `auto` and in software, picture by picture the same;
/// the decoder after it.
fn auto(name: &str) -> Decoder {
    let (codec, frames) = frames(name);
    let mut sw = Decoder::with_hardware(codec, W, H, Hardware::Off).unwrap();
    let mut hw = Decoder::with_hardware(codec, W, H, Hardware::Auto).unwrap();
    let want = pictures(&mut sw, &frames);
    let got = pictures(&mut hw, &frames);
    eprintln!("{name}: {}", hw.status().unwrap());
    for (i, (want, got)) in want.iter().zip(&got).enumerate() {
        let d = difference(want, got);
        assert!(d <= 1, "{name}: picture {i} is up to {d} levels from software's");
    }
    assert!(!hw.take_key_frame_request(), "{name}: a key frame asked for");
    hw
}

#[test]
fn auto_is_vaapi_then_software() {
    // 1. A stream VAAPI takes: decoded on it.
    let vaapi = {
        let hw = auto("h264.h264");
        if hw.method() == Hardware::Vaapi {
            assert_eq!(hw.hardware(), Some(true));
            assert_eq!(hw.software_reason(), None);
            true
        } else {
            assert!(mode() != "require", "VENTO_TEST_HW=require and VAAPI did not take H.264");
            let why = hw.software_reason().unwrap_or_default();
            assert!(why.starts_with("vaapi: no "), "{why}");
            eprintln!("auto: no VAAPI here ({why}), so only the steps after it");
            false
        }
    };
    if mode() == "absent" {
        assert!(!vaapi, "VENTO_TEST_HW=absent and VAAPI took H.264");
    }

    // 2. A stream VAAPI has no decoder for (iHD and H.264 High 10):
    // software, in the same context, the reason VAAPI's alone.
    let hw = auto("h264-high10.h264");
    let why = hw.software_reason().unwrap_or_default();
    if vaapi {
        assert_eq!(why, "vaapi: H.264 High 10 is not supported");
    } else {
        assert!(why.starts_with("vaapi: no ") && !why.contains("; "), "{why}");
    }
    assert_eq!(hw.hardware(), Some(false));
    assert!(hw.status().unwrap().contains(&why));

    // 3. AV1 that VAAPI has no decoder for (iHD and 4:4:4), which `av1`
    // cannot decode in software: dav1d, with VAAPI's reason.
    let hw = auto("av1-444.ivf");
    let why = hw.software_reason().unwrap_or_default();
    assert!(why.starts_with("vaapi: ") && !why.contains("; "), "{why}");
    assert_eq!(hw.name(), "libdav1d");
}
