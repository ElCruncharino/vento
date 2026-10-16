// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The hardware decoders against the software ones, on the clips `decode.rs`
//! decodes: each picture the same within a level, whether the hardware's
//! came out as NV12, P010 or XV30; the profile the device has no decoder for
//! decoded in software with the reason said; one device for every decoder;
//! and a hardware failure mid-stream a software decoder and a key frame
//! asked for.
//!
//! **Skipped where there is no VAAPI device** — each test then passes with a
//! note on stderr — since a machine that builds this need not have a GPU, and
//! a clip a device has no decoder for is compared all the same, in software,
//! with a note. `VENTO_TEST_HW=require` makes both failures instead, for a
//! machine known to have the device and the profiles (Intel's iHD has every
//! clip here but High 10). `VENTO_TEST_HW=absent` is for a run with
//! `/dev/dri` or libva hidden (`bwrap --dev-bind / / --tmpfs /dev/dri ...`),
//! and asserts the opposite: every decoder software, saying why.

#![cfg(not(target_os = "android"))]

use common::video::{Changes, Codec, Decoded, Decoder as _, fourcc};
use platform_desktop::ffmpeg::{Decoder, Hardware};

mod clips;
use clips::{H, W, annex_b, difference, frames, h264_aud, h265_aud, ivf, pictures};

fn mode() -> String {
    std::env::var("VENTO_TEST_HW").unwrap_or_default()
}

/// Whether a VAAPI decoder can be had here; a note, or with `require` a
/// failure, where not.
fn vaapi(test: &str) -> bool {
    let decoder = Decoder::with_hardware(Codec::H264, W, H, Hardware::Vaapi).expect("a decoder");
    if decoder.method() == Hardware::Vaapi {
        return true;
    }
    let why = decoder.software_reason().unwrap_or_default();
    assert!(
        mode() != "require",
        "{test}: VENTO_TEST_HW=require and there is no VAAPI decoder: {why}"
    );
    eprintln!("{test}: skipped, no VAAPI here ({why})");
    false
}

/// The clip on VAAPI and in software, picture by picture. `hardware` is
/// whether the device is expected to take it (on iHD); where it is not, the
/// reason is expected to name `reason`.
fn compare(name: &str, hardware: bool, reason: &str) {
    if vaapi(name) {
        compare_on(Hardware::Vaapi, name, hardware, reason);
    }
}

fn compare_on(method: Hardware, name: &str, hardware: bool, reason: &str) {
    let (codec, frames) = frames(name);
    let mut sw = Decoder::with_hardware(codec, W, H, Hardware::Off).expect("a software decoder");
    let mut hw = Decoder::with_hardware(codec, W, H, method).expect("a decoder");
    let want = pictures(&mut sw, &frames);
    let got = pictures(&mut hw, &frames);
    let status = hw.status().unwrap();
    eprintln!("{name}: {status}");
    let on_hardware = hw.hardware() == Some(true);
    if on_hardware != hardware {
        let why = hw.software_reason().unwrap_or_default();
        // `require` is iHD's profiles: Vulkan's are the driver's to have
        // (Fedora's Mesa has no H.264 or H.265), and only noted.
        assert!(
            mode() != "require" || method != Hardware::Vaapi,
            "{name}: on hardware {on_hardware}, expected {hardware} ({why})"
        );
        eprintln!("{name}: on hardware {on_hardware}, expected {hardware}: {why}");
    }
    if !on_hardware && !hardware {
        let why = hw.software_reason().unwrap_or_default();
        assert!(why.contains(reason), "{name}: the reason is {why:?}");
        assert!(status.contains(" sw ") && status.contains(reason), "{status}");
    }
    if on_hardware {
        let how = if method == Hardware::Vulkan { "copy" } else { "map" };
        assert!(
            status.starts_with(&format!("{} {} {how} ", codec.name(), method.name())),
            "{status}"
        );
        assert_eq!(hw.software_reason(), None);
    }
    assert!(!hw.take_key_frame_request(), "{name}: nothing failed");
    for (i, (want, got)) in want.iter().zip(&got).enumerate() {
        let d = difference(want, got);
        assert!(d <= 1, "{name}: picture {i} is up to {d} levels from software's");
    }
}

#[test]
fn h264() {
    compare("h264.h264", true, "");
}

#[test]
fn h264_bt601_full_range() {
    compare("h264-bt601-full.h264", true, "");
}

/// iHD has no H.264 High 10 decoder, so libavcodec asks again without VAAPI,
/// and the stream is software's with the profile named.
#[test]
fn h264_high10_is_software() {
    compare("h264-high10.h264", false, "vaapi: H.264 High 10 is not supported");
}

#[test]
fn h265() {
    compare("h265.h265", true, "");
}

/// P010 off the device, ten bits at the top of each sample's two.
#[test]
fn h265_main10() {
    compare("h265-main10.h265", true, "");
}

/// XV30 off the device, packed.
#[test]
fn h265_444_ten_bits() {
    compare("h265-444-10bit.h265", true, "");
}

#[test]
fn vp8() {
    compare("vp8.ivf", true, "");
}

#[test]
fn vp9() {
    compare("vp9.ivf", true, "");
}

#[test]
fn vp9_untagged() {
    compare("vp9-untagged.ivf", true, "");
}

#[test]
fn vp9_444_ten_bits() {
    compare("vp9-444-10bit.ivf", true, "");
}

/// libavcodec's own `av1`, which is what decodes on a device, against dav1d.
#[test]
fn av1() {
    compare("av1.ivf", true, "");
}

/// iHD has no AV1 decoder but profile 0's, and `av1` decodes nothing in
/// software: the decoder is reopened as dav1d on the first frame, which is
/// given again, so no key frame needs asking for.
#[test]
fn av1_444_is_dav1d() {
    compare("av1-444.ivf", false, "vaapi: AV1 High is not supported");
}

#[test]
fn av1_untagged() {
    compare("av1-untagged.ivf", true, "");
}

/// A clip decoded on VAAPI with its pictures held: each one exported as the
/// two layers its format has, and read back the same picture to the bit as
/// a decode that mapped it; every picture of one decoder in the same set of
/// surfaces, and another decoder's in another.
fn held(name: &str, layers: [u32; 2]) {
    if !vaapi(name) {
        return;
    }
    let (codec, frames) = frames(name);
    let mut mapped = Decoder::with_hardware(codec, W, H, Hardware::Vaapi).unwrap();
    let mut holding = Decoder::with_hardware(codec, W, H, Hardware::Vaapi).unwrap();
    let want = pictures(&mut mapped, &frames);
    let mut pools = Vec::new();
    for (i, frame) in frames.iter().enumerate() {
        let mut out = vec![0u32; W * H];
        let mut changed = Changes::new();
        changed.reset(H);
        let picture = match holding.decode_picture(frame, &mut out, W, H, &mut changed) {
            Decoded::Gpu(picture) => picture,
            Decoded::Converted => panic!("{name}: picture {i} was converted, not held"),
            Decoded::Nothing => panic!("{name}: no picture {i}"),
        };
        assert_eq!(out, vec![0u32; W * H], "{name}: a held picture writes nothing");
        assert_eq!((picture.width(), picture.height()), (W, H));
        let dmabuf = picture.export().unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            dmabuf.layers.iter().map(|l| l.fourcc).collect::<Vec<_>>(),
            layers,
            "{name}: the layers' formats"
        );
        let (y, uv) = (&dmabuf.layers[0], &dmabuf.layers[1]);
        assert!(y.width as usize >= W && y.height as usize >= H, "{name}: Y is {}x{}", y.width, y.height);
        assert_eq!((uv.width, uv.height), (y.width.div_ceil(2), y.height.div_ceil(2)));
        assert!(y.pitch >= y.width && uv.pitch >= uv.width, "{name}: pitches");
        let mut read = vec![0u32; W * H];
        assert!(picture.read(&mut read, W, H), "{name}: picture {i} reads back");
        assert_eq!(difference(&read, &want[i]), 0, "{name}: picture {i} read back");
        pools.push(picture.surface().0);
    }
    assert!(pools.windows(2).all(|p| p[0] == p[1]), "{name}: pools {pools:?}");
    assert_eq!(holding.hardware(), Some(true));
    let status = holding.status().unwrap();
    assert!(status.starts_with(&format!("{} vaapi dmabuf ", codec.name())), "{status}");

    let mut another = Decoder::with_hardware(codec, W, H, Hardware::Vaapi).unwrap();
    let mut out = vec![0u32; W * H];
    let mut changed = Changes::new();
    changed.reset(H);
    let Decoded::Gpu(picture) = another.decode_picture(&frames[0], &mut out, W, H, &mut changed) else {
        panic!("{name}: the other decoder held nothing");
    };
    assert_ne!(picture.surface().0, pools[0], "{name}: another decoder's surfaces");
}

#[test]
fn a_held_h264_picture_is_nv12() {
    held("h264.h264", [fourcc::R8, fourcc::GR88]);
}

#[test]
fn a_held_vp9_picture_is_nv12() {
    held("vp9.ivf", [fourcc::R8, fourcc::GR88]);
}

#[test]
fn a_held_av1_picture_is_nv12() {
    held("av1.ivf", [fourcc::R8, fourcc::GR88]);
}

#[test]
fn a_held_main10_picture_is_p010() {
    held("h265-main10.h265", [fourcc::R16, fourcc::GR1616]);
}

/// What cannot be held is converted as ever: software's pictures, and
/// VAAPI's in XV30.
#[test]
fn what_cannot_be_held_is_converted() {
    let try_one = |name: &str, hardware: Hardware| {
        let (codec, frames) = frames(name);
        let mut decoder = Decoder::with_hardware(codec, W, H, hardware).unwrap();
        let mut out = vec![0u32; W * H];
        let mut changed = Changes::new();
        changed.reset(H);
        changed.all(W, H);
        assert!(
            matches!(decoder.decode_picture(&frames[0], &mut out, W, H, &mut changed), Decoded::Converted),
            "{name} on {hardware:?}"
        );
        assert_ne!(out, vec![0u32; W * H]);
    };
    try_one("h264.h264", Hardware::Off);
    if vaapi("what_cannot_be_held_is_converted") {
        try_one("h265-444-10bit.h265", Hardware::Vaapi);
    }
}

/// A held picture whose wait fails is a fallback as a failed map is: the
/// packet decoded again in software, a key frame asked for, and converted.
#[test]
fn a_held_picture_that_fails_falls_back_to_software() {
    if !vaapi("a_held_picture_that_fails_falls_back_to_software") {
        return;
    }
    let (codec, frames) = frames("h264.h264");
    let mut decoder = Decoder::with_hardware(codec, W, H, Hardware::Vaapi).unwrap();
    let mut out = vec![0u32; W * H];
    let mut decode = |decoder: &mut Decoder, frame: &[u8]| {
        let mut changed = Changes::new();
        changed.reset(H);
        changed.all(W, H);
        decoder.decode_picture(frame, &mut out, W, H, &mut changed)
    };
    assert!(matches!(decode(&mut decoder, &frames[0]), Decoded::Gpu(_)));
    decoder.fail_next();
    // A P-frame, with nothing before it in software to refer to: no picture
    // until the key frame that is asked for, and that one converted.
    assert!(!matches!(decode(&mut decoder, &frames[1]), Decoded::Gpu(_)));
    assert!(decoder.take_key_frame_request());
    assert!(matches!(decode(&mut decoder, &frames[0]), Decoded::Converted));
    assert_eq!(decoder.hardware(), Some(false));
    assert!(decoder.software_reason().unwrap().contains("vaapi: sync failed"));
}

/// The render nodes this process has open: one per VAAPI device.
fn render_nodes() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .filter(|target| target.to_string_lossy().starts_with("/dev/dri/renderD"))
        .count()
}

/// VAAPI's pictures read both ways, as an image the driver copies into and
/// mapped in place, on alternate pictures until one is measured the faster:
/// every picture the same as software's either way, and a verdict at the
/// end of it.
#[test]
fn mapped_either_way_the_picture_is_the_same() {
    if !vaapi("mapped_either_way_the_picture_is_the_same") {
        return;
    }
    // The clip over and over (each time from its key frame), for enough
    // pictures to try both ways eight times after the first two, and one
    // more, whose decode is when the last trial's unmap is counted.
    let (codec, frames) = frames("h264.h264");
    let frames: Vec<Vec<u8>> = frames.iter().cycle().take(19).cloned().collect();
    let mut sw = Decoder::with_hardware(codec, W, H, Hardware::Off).unwrap();
    let mut hw = Decoder::with_hardware(codec, W, H, Hardware::Vaapi).unwrap();
    let want = pictures(&mut sw, &frames);
    let got = pictures(&mut hw, &frames);
    assert_eq!(hw.hardware(), Some(true));
    for (i, (want, got)) in want.iter().zip(&got).enumerate() {
        let d = difference(want, got);
        assert!(d <= 1, "picture {i} is up to {d} levels from software's");
    }
    let verdict = hw.maps_in_place();
    eprintln!("VAAPI maps in place: {verdict:?}");
    assert!(verdict.is_some(), "no verdict after {} pictures", frames.len());
}

/// Held by the tests that count render nodes and by the one that opens the
/// Vulkan device, which is one more render node: the count is the process's.
static RENDER_NODES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Every decoder that asks for VAAPI decodes on the same device: a second
/// decoder (and a tenth) opens no render node of its own, and the device
/// outlives the decoders.
#[test]
fn one_device_for_every_decoder() {
    if vaapi("one_device_for_every_decoder") {
        one_device_on(Hardware::Vaapi, "h264.h264");
    }
}

fn one_device_on(method: Hardware, name: &str) {
    let _counting = RENDER_NODES.lock().unwrap_or_else(|e| e.into_inner());
    let (codec, frames) = frames(name);
    let mut first = Decoder::with_hardware(codec, W, H, method).unwrap();
    pictures(&mut first, &frames);
    let open = render_nodes();
    assert!(open >= 1, "no render node open with a {} decoder", method.name());
    let mut more: Vec<Decoder> = (0..10)
        .map(|_| Decoder::with_hardware(codec, W, H, method).unwrap())
        .collect();
    for decoder in &mut more {
        pictures(decoder, &frames);
        assert_eq!(decoder.hardware(), Some(true));
    }
    assert_eq!(render_nodes(), open, "a decoder opened a device of its own");
    drop(more);
    drop(first);
    assert_eq!(platform_desktop::ffmpeg::device_opened(method), Some(true));
    assert_eq!(render_nodes(), open, "the device closed with its decoders");
}

/// A hardware picture that cannot be read mid-stream: the decoder is software
/// from then on, says so once through `take_key_frame_request`, and the next
/// key frame is a whole, right picture.
#[test]
fn a_failure_mid_stream_falls_back_to_software() {
    if vaapi("a_failure_mid_stream_falls_back_to_software") {
        fallback_on(Hardware::Vaapi, "h264.h264");
    }
}

fn fallback_on(method: Hardware, name: &str) {
    let (codec, frames) = frames(name);
    let mut sw = Decoder::with_hardware(codec, W, H, Hardware::Off).unwrap();
    let want = pictures(&mut sw, &frames[..1]);
    let mut hw = Decoder::with_hardware(codec, W, H, method).unwrap();
    pictures(&mut hw, &frames[..1]);
    assert_eq!(hw.hardware(), Some(true));

    hw.fail_next();
    let mut out = vec![0u32; W * H];
    let mut changed = Changes::new();
    changed.reset(H);
    // The frame after the key frame refers to a picture the new decoder never
    // saw: whether anything comes out of it is libavcodec's business.
    let _ = hw.decode(&frames[1], &mut out, W, H, &mut changed);
    assert!(hw.take_key_frame_request(), "the fallback asked for no key frame");
    assert!(!hw.take_key_frame_request(), "asked twice");
    assert_eq!(hw.method(), Hardware::Off);
    assert_eq!(hw.hardware(), Some(false));
    let why = hw.software_reason().unwrap_or_default();
    // Vulkan's images are copied, not mapped.
    let failed = if method == Hardware::Vulkan { "copy failed" } else { "map failed" };
    assert!(why.starts_with(&format!("{}: {failed}", method.name())), "{why}");
    let status = hw.status().unwrap();
    assert!(
        status.starts_with(&format!("{} sw ", codec.name())) && status.contains(failed),
        "{status}"
    );

    // The key frame again, as the far end would send it when asked.
    let got = pictures(&mut hw, &frames[..1]);
    assert_eq!(hw.hardware(), Some(false));
    assert!(difference(&want[0], &got[0]) <= 1);
}

/// Without a device (or without libva), asking for VAAPI is software, with
/// the reason; only meaningful in a run that hid them, which says so.
#[test]
fn without_a_device_it_is_software() {
    if mode() != "absent" {
        eprintln!("without_a_device_it_is_software: skipped, VENTO_TEST_HW is not absent");
        return;
    }
    let (codec, frames) = frames("h264.h264");
    let mut hw = Decoder::with_hardware(codec, W, H, Hardware::Vaapi).unwrap();
    assert_eq!(hw.method(), Hardware::Off);
    let why = hw.software_reason().unwrap_or_default();
    assert!(
        why.starts_with("vaapi: no device") || why.starts_with("vaapi: no libva"),
        "{why}"
    );
    pictures(&mut hw, &frames);
    assert_eq!(hw.hardware(), Some(false));
    let status = hw.status().unwrap();
    eprintln!("without a device: {status}");
    assert!(status.starts_with("H.264 sw ") && status.contains(&why), "{status}");
    assert_eq!(platform_desktop::ffmpeg::device_opened(Hardware::Vaapi), Some(false));
}

/// With `/dev/dri` hidden the only Vulkan device left is a CPU's (lavapipe),
/// which is not one to decode on: asking for Vulkan is software, with the
/// reason, no device is opened, and nothing of lavapipe stays loaded from
/// the look at the devices.
#[test]
fn without_a_gpu_vulkan_opens_no_cpu_device() {
    if mode() != "absent" || render_nodes_present() {
        eprintln!("without_a_gpu_vulkan_opens_no_cpu_device: skipped, /dev/dri is not hidden");
        return;
    }
    let (codec, frames) = frames("vp9.ivf");
    let mut hw = Decoder::with_hardware(codec, W, H, Hardware::Vulkan).unwrap();
    assert_eq!(hw.method(), Hardware::Off);
    let why = hw.software_reason().unwrap_or_default();
    assert!(
        why == "vulkan: no video decode device" || why.starts_with("vulkan: no libvulkan"),
        "{why}"
    );
    pictures(&mut hw, &frames);
    assert_eq!(hw.hardware(), Some(false));
    assert_eq!(platform_desktop::ffmpeg::device_opened(Hardware::Vulkan), Some(false));
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    assert!(!maps.contains("libvulkan_lvp"), "lavapipe is still loaded");
}

/// Whether there is any DRM render node to open at all.
fn render_nodes_present() -> bool {
    std::fs::read_dir("/dev/dri")
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
        })
        .unwrap_or(false)
}

/// Vulkan Video, which only a run that asks for it tries
/// (`VENTO_TEST_VULKAN=1`, and on Mesa's ANV `ANV_DEBUG=video-decode`): what
/// the device has a decoder for comes out as software's does, and what it has
/// not is software, with a note; one device for every decoder, and a failure
/// mid-stream software, as on VAAPI.
#[test]
fn vulkan() {
    if std::env::var_os("VENTO_TEST_VULKAN").is_none() {
        eprintln!("vulkan: skipped, VENTO_TEST_VULKAN is not set");
        return;
    }
    let _counting = RENDER_NODES.lock().unwrap_or_else(|e| e.into_inner());
    let probe = Decoder::with_hardware(Codec::Av1, W, H, Hardware::Vulkan).unwrap();
    if probe.method() != Hardware::Vulkan {
        eprintln!("vulkan: no device ({:?})", probe.software_reason());
        return;
    }
    // The device chosen is a GPU's, not lavapipe's (`llvmpipe (LLVM ...)`),
    // which the loader may well list first.
    let device = platform_desktop::ffmpeg::vulkan_device().expect("a Vulkan device chosen");
    eprintln!("vulkan: on {device}");
    assert!(!device.contains("llvmpipe"), "{device}");
    for name in [
        "h264.h264",
        "h265.h265",
        "h265-main10.h265",
        "vp9.ivf",
        "vp9-444-10bit.ivf",
        "av1.ivf",
    ] {
        compare_on(Hardware::Vulkan, name, true, "");
    }
    drop(_counting);
    one_device_on(Hardware::Vulkan, "av1.ivf");
    fallback_on(Hardware::Vulkan, "av1.ivf");
}

/// What a frame costs, software against VAAPI (or the methods
/// `VENTO_BENCH_HW` names, comma-separated: `off,vaapi,vulkan`), on the clips
/// `VENTO_BENCH` names (comma-separated Annex B `.h264`/`.h265` at
/// `VENTO_BENCH_SIZE`, or IVF, which says its own size): the decoder's own time (decode, and on
/// hardware the wait for it and the map) as `status` has it, the whole of
/// `decode` with every row converted, and the CPU the process spent. Run
/// with `--release --ignored --nocapture`, since the conversion is Rust's.
/// `VENTO_BENCH_FRAMES` decodes only the first so many, and
/// `VENTO_BENCH_GAP_MS` waits that long before each, untimed, as a quiet
/// desktop does between updates (and a GPU idles, clocking down).
#[test]
#[ignore]
fn bench() {
    let Ok(clips) = std::env::var("VENTO_BENCH") else {
        eprintln!("bench: VENTO_BENCH names no clips");
        return;
    };
    let cpu = || {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        let fields: Vec<&str> = stat.rsplit(')').next().unwrap().split_whitespace().collect();
        // utime and stime, in ticks of a hundredth of a second.
        (fields[11].parse::<f64>().unwrap() + fields[12].parse::<f64>().unwrap()) * 10.0
    };
    let methods: Vec<Hardware> = std::env::var("VENTO_BENCH_HW")
        .unwrap_or("off,vaapi".into())
        .split(',')
        .map(|name| Hardware::from_name(name).unwrap_or_else(|| panic!("{name}: which method?")))
        .collect();
    let number = |var: &str| std::env::var(var).ok().map(|v| v.parse::<u64>().unwrap());
    let gap = std::time::Duration::from_millis(number("VENTO_BENCH_GAP_MS").unwrap_or(0));
    let limit = number("VENTO_BENCH_FRAMES").map_or(usize::MAX, |n| n as usize);
    for path in clips.split(',') {
        let bytes = std::fs::read(path).unwrap();
        let (codec, frames, (w, h)) = if path.ends_with(".ivf") {
            let size = (
                u16::from_le_bytes([bytes[12], bytes[13]]) as usize,
                u16::from_le_bytes([bytes[14], bytes[15]]) as usize,
            );
            let codec = if &bytes[8..12] == b"AV01" { Codec::Av1 } else { Codec::Vp9 };
            (codec, ivf(&bytes), size)
        } else {
            let size = std::env::var("VENTO_BENCH_SIZE").unwrap_or("1920x1080".into());
            let (w, h) = size.split_once('x').unwrap();
            let size = (w.parse().unwrap(), h.parse().unwrap());
            if path.ends_with(".h265") {
                (Codec::H265, annex_b(&bytes, h265_aud), size)
            } else {
                (Codec::H264, annex_b(&bytes, h264_aud), size)
            }
        };
        for &method in &methods {
            let mut decoder = Decoder::with_hardware(codec, w, h, method).unwrap();
            let mut out = vec![0u32; w * h];
            let mut changed = Changes::new();
            let (mut wall, mut worst, mut pictures) = (0f64, 0f64, 0usize);
            let cpu0 = cpu();
            let frames = &frames[..frames.len().min(limit)];
            for (i, frame) in frames.iter().enumerate() {
                std::thread::sleep(gap);
                changed.reset(h);
                for row in 0..h {
                    changed.mark(row, 0, w);
                }
                let t = std::time::Instant::now();
                pictures += usize::from(decoder.decode(frame, &mut out, w, h, &mut changed));
                let dt = t.elapsed().as_secs_f64() * 1000.0;
                if i > 0 {
                    wall += dt;
                    worst = worst.max(dt);
                }
            }
            let n = (frames.len() - 1) as f64;
            let map = decoder.timing().and_then(|(_, map)| map);
            let in_place = match decoder.maps_in_place() {
                Some(true) => ", in place",
                Some(false) => ", as an image",
                None => "",
            };
            eprintln!(
                "{path} {}: {pictures}/{} pictures; {} (of it map {}{in_place}); decode+convert \
                 {:.2} ms/frame (worst {worst:.2}); CPU {:.2} ms/frame",
                method.name(),
                frames.len(),
                decoder.status().unwrap(),
                map.map_or("-".into(), |m| format!("{m:.2} ms")),
                wall / n,
                (cpu() - cpu0) / frames.len() as f64,
            );
        }
    }
}
