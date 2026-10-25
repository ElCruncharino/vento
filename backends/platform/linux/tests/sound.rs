// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! libavcodec's AAC and Opus through `runtime::media::audio::decode`, against
//! clips of a known tone: the format that comes out, its pitch and its level.
//! How the clips were made is `fixtures/generate-sound.sh`.
//!
//! Each clip is fed the way a protocol delivers it, one packet per call:
//! AAC as raw frames, the ADTS header cut off as RDP never sends one, and
//! Opus as the packets of its Ogg pages, without the headers.

#![cfg(not(target_os = "android"))]

use runtime::media::audio::Format;
use runtime::media::audio::decode::{Codec, Decoders, opus_head};
use platform_linux::ffmpeg_audio::FFmpegAudio;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// ADTS's frames without their headers: each header is seven bytes (nine
/// with a CRC), and says how long its frame is, header included.
fn adts(bytes: &[u8]) -> Vec<&[u8]> {
    let mut frames = Vec::new();
    let mut at = 0;
    while at + 7 <= bytes.len() {
        let h = &bytes[at..at + 7];
        assert_eq!((h[0], h[1] & 0xf0), (0xff, 0xf0), "an ADTS sync word at {at}");
        let header = if h[1] & 1 == 1 { 7 } else { 9 };
        let len = ((h[3] as usize & 3) << 11) | ((h[4] as usize) << 3) | (h[5] as usize >> 5);
        frames.push(&bytes[at + header..at + len]);
        at += len;
    }
    frames
}

/// Ogg's packets, from its pages' segment tables: a segment of 255 bytes
/// continues a packet, anything shorter ends one. The first two packets of
/// an Opus stream are its identification header and its comments.
fn ogg(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    let mut packet = Vec::new();
    let mut at = 0;
    while at + 27 <= bytes.len() {
        assert_eq!(&bytes[at..at + 4], b"OggS");
        let segments = bytes[at + 26] as usize;
        let table = &bytes[at + 27..at + 27 + segments];
        let mut body = at + 27 + segments;
        for &lacing in table {
            packet.extend_from_slice(&bytes[body..body + lacing as usize]);
            body += lacing as usize;
            if lacing < 255 {
                packets.push(std::mem::take(&mut packet));
            }
        }
        at = body;
    }
    packets
}

/// One channel of interleaved samples.
fn channel(samples: &[i16], channels: usize, which: usize) -> Vec<f64> {
    samples.iter().skip(which).step_by(channels).map(|&s| s as f64 / 32768.0).collect()
}

/// The pitch of a steady tone, from where it crosses zero going up (each
/// crossing placed between its two samples), over the middle of `x`.
fn pitch(x: &[f64], rate: u32) -> f64 {
    let mut crossings = Vec::new();
    for i in 1..x.len() {
        if x[i - 1] < 0.0 && x[i] >= 0.0 {
            crossings.push((i - 1) as f64 + x[i - 1] / (x[i - 1] - x[i]));
        }
    }
    let (first, last) = (crossings[0], crossings[crossings.len() - 1]);
    (crossings.len() - 1) as f64 / ((last - first) / rate as f64)
}

/// The level against a half-scale sine's, whose RMS is 0.5 / √2.
fn level(x: &[f64]) -> f64 {
    let rms = (x.iter().map(|s| s * s).sum::<f64>() / x.len() as f64).sqrt();
    rms / (0.5 / 2f64.sqrt())
}

/// Decodes every packet, checks every packet's format, and returns the
/// samples.
fn decode_all(codec: Codec, format: Format, config: &[u8], packets: &[&[u8]], want: Format) -> Vec<i16> {
    let mut decoder = FFmpegAudio.decoder(codec, format, config).expect("a decoder");
    assert!(!decoder.name().is_empty());
    let mut out = Vec::new();
    for (i, p) in packets.iter().enumerate() {
        match decoder.decode(p, &mut out) {
            Ok(Some(f)) => assert_eq!(f, want, "packet {i}'s format"),
            Ok(None) => {}
            Err(e) => panic!("packet {i}: {e}"),
        }
    }
    out
}

/// The middle 0.2 s of each channel: 440 Hz, at the level it was encoded
/// at, and the channels alike.
fn assert_tone(samples: &[i16], format: Format, what: &str) {
    let frames = samples.len() / format.channels;
    let middle = format.rate as usize / 20..frames - format.rate as usize / 20;
    assert!(frames as f64 / format.rate as f64 > 0.25, "{what}: only {frames} frames");
    let first = channel(samples, format.channels, 0);
    for c in 0..format.channels {
        let x = channel(samples, format.channels, c);
        let x = &x[middle.clone()];
        let hz = pitch(x, format.rate);
        let l = level(x);
        assert!((hz - 440.0).abs() < 0.5, "{what}: channel {c} at {hz:.2} Hz");
        assert!((l - 1.0).abs() < 0.05, "{what}: channel {c} at {l:.4} of the source");
        let apart = x.iter().zip(&first[middle.clone()]).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
        assert!(apart < 0.01, "{what}: channel {c} differs from the first by {apart}");
    }
}

#[test]
fn both_codecs_are_here() {
    assert!(FFmpegAudio.decodable(Codec::Aac));
    assert!(FFmpegAudio.decodable(Codec::Opus));
}

#[test]
fn rdp_aac_is_a_440_hz_tone() {
    let bytes = fixture("aac-44100-stereo.aac");
    let frames = adts(&bytes);
    assert!(frames.len() > 10);
    let format = Format::new(44_100, 2).unwrap();
    // No setup, as RDP's sound gives none: AAC LC from the format alone.
    let samples = decode_all(Codec::Aac, format, &[], &frames, format);
    assert_tone(&samples, format, "AAC");
    // The same with the setup an encoder writes, which is the one made up.
    let again = decode_all(Codec::Aac, format, &[0x12, 0x10], &frames, format);
    assert_eq!(samples, again);
}

#[test]
fn opus_is_a_440_hz_tone_in_stereo_and_mono() {
    for (name, channels) in [("opus-48000-stereo.opus", 2), ("opus-48000-mono.opus", 1)] {
        let packets = ogg(&fixture(name));
        let (head, audio) = (&packets[0], &packets[2..]);
        assert!(head.starts_with(b"OpusHead"));
        let audio: Vec<&[u8]> = audio.iter().map(|p| p.as_slice()).collect();
        let format = Format::new(48_000, channels).unwrap();

        // With the stream's own header, whose pre-skip libavcodec drops.
        let with_head = decode_all(Codec::Opus, format, head, &audio, format);
        assert_tone(&with_head, format, name);
        // With none, as RDP's sound sends it: everything decoded, the
        // encoder's lookahead included.
        let without = decode_all(Codec::Opus, format, &[], &audio, format);
        assert_tone(&without, format, name);
        let skip = u16::from_le_bytes([head[10], head[11]]) as usize;
        assert_eq!(without.len() - with_head.len(), skip * channels, "{name}: the pre-skip");
        // The header made up for that pre-skip is the stream's own, but for
        // the input rate the encoder was told.
        let made = opus_head(format, skip as u16).unwrap();
        assert_eq!(made[..12], head[..12]);
    }
}

#[test]
fn a_broken_packet_is_an_error_and_the_next_decodes() {
    let bytes = fixture("aac-44100-stereo.aac");
    let frames = adts(&bytes);
    let format = Format::new(44_100, 2).unwrap();
    let mut decoder = FFmpegAudio.decoder(Codec::Aac, format, &[]).unwrap();
    let mut out = Vec::new();
    for f in &frames[..3] {
        decoder.decode(f, &mut out).unwrap();
    }
    let before = out.len();
    let garbage = vec![0xa5u8; 300];
    match decoder.decode(&garbage, &mut out) {
        Err(_) | Ok(None) => assert_eq!(out.len(), before, "nothing from garbage"),
        Ok(Some(_)) => {} // a decoder that conceals is free to
    }
    let after = out.len();
    assert_eq!(decoder.decode(frames[3], &mut out).unwrap(), Some(format));
    assert_eq!(out.len() - after, 1024 * 2);

    // A setup no format has is refused at the open, not at the first packet.
    let odd = Format::new(44_000, 2).unwrap();
    assert!(FFmpegAudio.decoder(Codec::Aac, odd, &[]).is_err());
}
