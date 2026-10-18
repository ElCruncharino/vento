// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hardware decoding off, which is the default, opens nothing: after every
//! clip has been decoded with it off, no device has been asked for and
//! libva is not in the process. A binary of its own, since a
//! test of `hardware.rs` beside it would open the device this says is not.

#![cfg(not(target_os = "android"))]

use media::video::{Changes, Decoders};
use platform_linux::ffmpeg::{FFmpeg, Hardware, device_opened};

mod clips;
use clips::{annex_b, fixture, h264_aud, h265_aud, ivf};

#[test]
fn off_opens_no_device_and_loads_no_library() {
    let ffmpeg = FFmpeg::default();
    assert_eq!(ffmpeg.hardware(), Hardware::Off);
    let clips = [
        ("h264.h264", media::video::Codec::H264),
        ("h264-high10.h264", media::video::Codec::H264),
        ("h265-main10.h265", media::video::Codec::H265),
        ("vp8.ivf", media::video::Codec::Vp8),
        ("vp9.ivf", media::video::Codec::Vp9),
        ("av1.ivf", media::video::Codec::Av1),
    ];
    for (name, codec) in clips {
        let bytes = fixture(name);
        let frames = match codec {
            media::video::Codec::H264 => annex_b(&bytes, h264_aud),
            media::video::Codec::H265 => annex_b(&bytes, h265_aud),
            _ => ivf(&bytes),
        };
        let mut decoder = ffmpeg.decoder(codec, 128, 128).expect("a decoder");
        let mut out = vec![0u32; 128 * 128];
        let mut changed = Changes::new();
        for frame in frames {
            changed.reset(128);
            assert!(decoder.decode(frame, &mut out, 128, 128, &mut changed), "{name}");
        }
        assert_eq!(decoder.hardware(), Some(false), "{name}");
        assert!(decoder.status().unwrap().contains(" sw "), "{name}");
    }
    assert_eq!(device_opened(Hardware::Vaapi), None);
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    for library in ["libva.so", "libva-drm.so"] {
        assert!(!maps.contains(library), "{library} is loaded with hardware decoding off");
    }
}
