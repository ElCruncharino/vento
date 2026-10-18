// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The far end's sound where it arrives compressed: which codecs there are,
//! what a decoder promises, and the few bytes of setup each codec needs that
//! a protocol does not send.
//!
//! As with [`video::Decoders`](crate::video::Decoders), a protocol asks for a
//! decoder and does not know whose it is: what it holds is the session's
//! [`Outputs`](super::Outputs), whose [`decoders`](super::Outputs::decoders)
//! are the platform's — libavcodec on the Linux client (built with the
//! client's own FFmpeg), MediaCodec on the phone — so that RDP's AAC is
//! decoded by the same code on both, and by none of FreeRDP's.
//!
//! What comes out is what a [`Stream`](super::Stream) takes, signed 16-bit
//! samples interleaved, whatever the decoder works in: both libraries decode
//! AAC and Opus to floating point, and the conversion is the platform's half
//! rather than the protocol's.

use super::Format;

/// Which codec a stream's packets are in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// MPEG-4 AAC, as raw access units — one frame a packet, with no ADTS
    /// header — which is how RDP's sound sends it (`WAVE_FORMAT_AAC_MS`).
    Aac,
    /// Opus packets, as RDP's sound (`WAVE_FORMAT_OPUS`) and RustDesk send
    /// them: one a packet, 48 kHz, mono or stereo.
    Opus,
}

impl Codec {
    pub const ALL: [Codec; 2] = [Codec::Aac, Codec::Opus];

    pub fn name(self) -> &'static str {
        match self {
            Codec::Aac => "AAC",
            Codec::Opus => "Opus",
        }
    }
}

/// One stream's decoder. Made, fed and dropped on whichever thread the
/// protocol's sound arrives on, one call at a time.
pub trait Decoder: Send {
    /// One packet, as the far end sent it: its samples appended to `out`,
    /// signed 16-bit and interleaved, in the format returned. `None`, with
    /// nothing appended, where the packet made no samples yet — a decoder
    /// that holds a packet or two back has them on a later call — and an
    /// error where it could not be decoded, which drops that packet and
    /// leaves the decoder ready for the next.
    ///
    /// The format is the decoder's answer, not the caller's guess: an AAC
    /// stream with SBR comes out at twice the rate its setup names, and Opus
    /// always at 48 kHz.
    fn decode(&mut self, packet: &[u8], out: &mut Vec<i16>) -> Result<Option<Format>, String>;

    /// The decoder itself, for a log line: `aac (libavcodec)`,
    /// `c2.android.aac.decoder`.
    fn name(&self) -> String;
}

/// What decodes compressed sound here.
pub trait Decoders: Send + Sync {
    /// Whether this codec can be decoded here at all: what a protocol offers
    /// the far end, which then sends only what it was offered. Cheap to ask
    /// again.
    fn decodable(&self, codec: Codec) -> bool;

    /// A decoder for a stream of `format`, set up by `config` — AAC's
    /// AudioSpecificConfig or Opus's identification header, as a protocol
    /// that sends one has it — or, where `config` is empty, by the format
    /// alone: AAC LC, and Opus as RFC 7845 describes a stream with no
    /// pre-skip, which is what RDP's sound sends without either.
    fn decoder(&self, codec: Codec, format: Format, config: &[u8]) -> Result<Box<dyn Decoder>, String>;
}

/// The setup a decoder is given: `config` where the protocol had one, else
/// the one [`Decoders::decoder`] says the format stands for. `None` for a
/// format no such setup describes (AAC at a rate it has no index for, Opus
/// in more than two channels).
pub fn config(codec: Codec, format: Format, config: &[u8]) -> Option<Vec<u8>> {
    if !config.is_empty() {
        return Some(config.to_vec());
    }
    match codec {
        Codec::Aac => aac_config(format).map(|c| c.to_vec()),
        Codec::Opus => opus_head(format, 0).map(|c| c.to_vec()),
    }
}

/// The AAC sampling frequencies an AudioSpecificConfig can name by index
/// (ISO/IEC 14496-3, 1.6.3.4).
const AAC_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000, 7_350,
];

/// The two-byte AudioSpecificConfig of AAC LC at `format`: what RDP's
/// sound leaves out, since its format says no more than a rate and a
/// channel count, and what both libraries need before a raw frame means
/// anything (libavcodec guesses one from its context's numbers, MediaCodec
/// will not start without it). Five bits of object type (2, LC), four of
/// rate index, four of channel configuration (one to six channels, then
/// eight for 7.1), and three zero bits.
pub fn aac_config(format: Format) -> Option<[u8; 2]> {
    let rate = AAC_RATES.iter().position(|&r| r == format.rate)? as u16;
    let channels = match format.channels {
        1..=6 => format.channels as u16,
        8 => 7,
        _ => return None,
    };
    let bits = (2u16 << 11) | (rate << 7) | (channels << 3);
    Some(bits.to_be_bytes())
}

/// The 19-byte identification header of an Opus stream at `format`, mono or
/// stereo (channel mapping family 0), with `pre_skip` samples at 48 kHz to
/// drop from the start (RFC 7845, 5.1). The rate in it is only what the
/// encoder was given: Opus decodes at 48 kHz whatever it says.
pub fn opus_head(format: Format, pre_skip: u16) -> Option<[u8; 19]> {
    if !(1..=2).contains(&format.channels) {
        return None;
    }
    let mut head = [0u8; 19];
    head[..8].copy_from_slice(b"OpusHead");
    head[8] = 1;
    head[9] = format.channels as u8;
    head[10..12].copy_from_slice(&pre_skip.to_le_bytes());
    head[12..16].copy_from_slice(&format.rate.to_le_bytes());
    // Output gain 0 dB and mapping family 0 are the zeros left.
    Some(head)
}

/// The pre-skip an identification header asks for, in 48 kHz samples: what
/// MediaCodec wants told separately (as `csd-1`, in nanoseconds).
pub fn opus_pre_skip(head: &[u8]) -> Option<u16> {
    (head.len() >= 19 && head.starts_with(b"OpusHead")).then(|| u16::from_le_bytes([head[10], head[11]]))
}

/// Converts `frames` frames of `channels` planes of 32-bit floats (what
/// libavcodec's AAC and Opus decoders produce, and MediaCodec's where it is
/// asked for floats) into interleaved samples appended to `out`: clipped at
/// full scale rather than wrapped, and rounded to the nearest.
pub fn planar_f32_to_s16(planes: &[&[f32]], frames: usize, out: &mut Vec<i16>) {
    out.reserve(frames * planes.len());
    for i in 0..frames {
        for plane in planes {
            out.push(f32_to_s16(plane[i]));
        }
    }
}

pub fn f32_to_s16(s: f32) -> i16 {
    let scaled = (s * 32768.0).round();
    if scaled.is_nan() {
        0
    } else {
        scaled.clamp(i16::MIN as f32, i16::MAX as f32) as i16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aac_lc_configs_are_what_encoders_write() {
        // ffmpeg's and every MP4 muxer's for AAC LC 44.1 kHz stereo and
        // 48 kHz mono.
        assert_eq!(aac_config(Format::new(44_100, 2).unwrap()), Some([0x12, 0x10]));
        assert_eq!(aac_config(Format::new(48_000, 1).unwrap()), Some([0x11, 0x88]));
        assert_eq!(aac_config(Format::new(48_000, 8).unwrap()), Some([0x11, 0xb8]));
        assert_eq!(aac_config(Format::new(44_000, 2).unwrap()), None, "no index for it");
        assert_eq!(aac_config(Format::new(48_000, 7).unwrap()), None);
    }

    #[test]
    fn opus_heads_are_rfc_7845s() {
        let head = opus_head(Format::new(48_000, 2).unwrap(), 312).unwrap();
        assert_eq!(&head[..8], b"OpusHead");
        assert_eq!(head[8..12], [1, 2, 0x38, 0x01]);
        assert_eq!(head[12..16], 48_000u32.to_le_bytes());
        assert_eq!(head[16..], [0, 0, 0]);
        assert_eq!(opus_pre_skip(&head), Some(312));
        assert_eq!(opus_pre_skip(b"OpusTags"), None);
        assert!(opus_head(Format::new(48_000, 3).unwrap(), 0).is_none());
    }

    #[test]
    fn a_config_given_is_kept() {
        let f = Format::new(44_100, 2).unwrap();
        assert_eq!(config(Codec::Aac, f, &[0x13, 0x10]), Some(vec![0x13, 0x10]));
        assert_eq!(config(Codec::Aac, f, &[]), Some(vec![0x12, 0x10]));
        assert_eq!(config(Codec::Opus, f, &[]).unwrap().len(), 19);
    }

    #[test]
    fn floats_become_samples_interleaved_and_clipped() {
        let left = [0.0f32, 0.5, 1.0, -1.0, 2.0];
        let right = [-0.5f32, 0.25, -2.0, f32::NAN, 1.0 / 32768.0];
        let mut out = vec![7];
        planar_f32_to_s16(&[&left, &right], 5, &mut out);
        assert_eq!(out, [7, 0, -16384, 16384, 8192, 32767, -32768, -32768, 0, 32767, 1]);
    }
}
