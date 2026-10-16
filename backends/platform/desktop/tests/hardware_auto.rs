// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! `auto`, per stream: VAAPI where it takes the stream, and then nothing of
//! Vulkan is loaded; Vulkan where VAAPI cannot (no device, or no decoder for
//! the profile); software after both, with every device tried named in the
//! reason. A binary of its own, since whether libvulkan has been loaded is
//! the process's, and `hardware.rs` beside it loads it.
//!
//! The same conventions as `hardware.rs`: what a machine without the devices
//! cannot show is skipped with a note, `VENTO_TEST_HW=require` makes a
//! missing VAAPI device a failure, and `VENTO_TEST_HW=absent` is a run with
//! `/dev/dri` or libva hidden. `VENTO_TEST_VULKAN=1` says a Vulkan device
//! that decodes is expected (on Mesa's ANV, with `ANV_DEBUG=video-decode`).

#![cfg(not(target_os = "android"))]

use common::video::Decoder as _;
use platform_desktop::ffmpeg::{Decoder, Hardware, device_opened};

mod clips;
use clips::{H, W, difference, frames, pictures};

fn mode() -> String {
    std::env::var("VENTO_TEST_HW").unwrap_or_default()
}

fn loaded(library: &str) -> bool {
    std::fs::read_to_string("/proc/self/maps")
        .unwrap()
        .contains(library)
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

/// One test, in order, since the first step's point is that nothing has
/// loaded libvulkan yet.
#[test]
fn auto_is_vaapi_then_vulkan_then_software() {
    // 1. A stream VAAPI takes: decoded on it, and Vulkan never looked at.
    let vaapi = {
        let hw = auto("h264.h264");
        if hw.method() == Hardware::Vaapi {
            assert_eq!(hw.hardware(), Some(true));
            assert_eq!(hw.software_reason(), None);
            assert_eq!(device_opened(Hardware::Vulkan), None);
            assert!(!loaded("libvulkan"), "auto loaded libvulkan for a stream VAAPI took");
            true
        } else {
            assert!(mode() != "require", "VENTO_TEST_HW=require and VAAPI did not take H.264");
            let why = hw.software_reason().unwrap_or_default();
            assert!(why.starts_with("vaapi: no "), "{why}");
            eprintln!("auto: no VAAPI here ({why}), so only the steps after it");
            false
        }
    };

    // 2. A stream VAAPI has no decoder for (iHD and H.264 High 10): Vulkan
    // is tried next, and software after it, the reason naming both.
    let hw = auto("h264-high10.h264");
    let why = hw.software_reason().unwrap_or_default();
    if vaapi {
        assert!(
            why.starts_with("vaapi: H.264 High 10 is not supported; vulkan: "),
            "{why}"
        );
    } else {
        assert!(why.starts_with("vaapi: no ") && why.contains("; vulkan: "), "{why}");
    }
    // Software, though it may be in the Vulkan device's decoder, which
    // libavcodec decodes the stream in software in when the driver has no
    // decoder for the profile.
    assert_eq!(hw.hardware(), Some(false));
    assert!(hw.status().unwrap().contains(&why));

    // 3. AV1 that VAAPI has no decoder for (iHD and 4:4:4), which `av1`
    // cannot decode in software: Vulkan, and then dav1d.
    let hw = auto("av1-444.ivf");
    let why = hw.software_reason().unwrap_or_default();
    assert!(why.contains("; vulkan: "), "{why}");
    assert_eq!(hw.name(), "libdav1d");

    // 4. Without VAAPI, a stream the Vulkan device has: on Vulkan.
    if !vaapi && std::env::var_os("VENTO_TEST_VULKAN").is_some() {
        let hw = auto("vp9.ivf");
        let why = hw.software_reason().unwrap_or_default();
        if platform_desktop::ffmpeg::vulkan_device().is_some() {
            assert_eq!(hw.method(), Hardware::Vulkan, "{why}");
            assert_eq!(hw.hardware(), Some(true));
        } else {
            eprintln!("auto: no Vulkan device decodes here ({why})");
        }
    }

    // With no GPU to open (`/dev/dri` hidden), what is left of Vulkan is a
    // CPU's, which is not opened, and nothing of it stays loaded.
    if mode() == "absent" && platform_desktop::ffmpeg::vulkan_device().is_none() {
        let hw = auto("vp9.ivf");
        let why = hw.software_reason().unwrap_or_default();
        assert!(why.starts_with("vaapi: no "), "{why}");
        assert!(
            why.ends_with("; vulkan: no video decode device")
                || why.contains("; vulkan: no libvulkan"),
            "{why}"
        );
        assert_eq!(device_opened(Hardware::Vulkan), Some(false));
        assert!(!loaded("libvulkan_lvp"), "lavapipe is still loaded");
    }
}
