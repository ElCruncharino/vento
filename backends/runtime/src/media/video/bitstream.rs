// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a decoder can tell from a frame's first bytes without decoding it.

use super::Codec;

/// Whether `frame` is one a decoder that has seen nothing yet can decode: a
/// key frame that carries what it is decoded with. For a decoder that has
/// to start over in the middle of a stream (MediaCodec, given a frame larger
/// than its input buffers, is stopped and configured again with larger
/// ones), it is the difference between feeding the frame and asking the far
/// end for another.
///
/// H.264 and H.265: an access unit with a sequence parameter set and an IDR
/// (H.265: IRAP) slice in it, which is how every encoder tested sends a key
/// frame. VP8 and VP9: the frame header's key frame flag. AV1 is never
/// one here, since its key frame is a field deep in a frame header, behind a
/// sequence header that decides how to read it; a caller that gets false
/// asks for a key frame, which costs a round trip rather than a picture.
pub fn starts_stream(codec: Codec, frame: &[u8]) -> bool {
    match codec {
        Codec::H264 => {
            let (mut sps, mut idr) = (false, false);
            for nal in nal_units(frame) {
                match nal & 0x1f {
                    7 => sps = true,
                    5 => idr = true,
                    _ => {}
                }
            }
            sps && idr
        }
        Codec::H265 => {
            let (mut sps, mut irap) = (false, false);
            for nal in nal_units(frame) {
                match (nal >> 1) & 0x3f {
                    33 => sps = true,
                    16..=21 => irap = true,
                    _ => {}
                }
            }
            sps && irap
        }
        // RFC 6386 9.1: the frame tag's lowest bit is 0 for a key frame,
        // whose start code follows the three-byte tag.
        Codec::Vp8 => frame.len() >= 10 && frame[0] & 1 == 0 && frame[3..6] == [0x9d, 0x01, 0x2a],
        // VP9's uncompressed header: frame marker (2 bits, 0b10), profile (2
        // bits, and a reserved one for profile 3), show_existing_frame, then
        // frame_type, which is 0 for a key frame.
        Codec::Vp9 => {
            let Some(&b) = frame.first() else { return false };
            if b >> 6 != 0b10 {
                return false;
            }
            let profile = (((b >> 4) & 1) << 1) | ((b >> 5) & 1);
            let at = if profile == 3 { 2 } else { 3 };
            let show_existing = (b >> at) & 1;
            let frame_type = (b >> (at - 1)) & 1;
            show_existing == 0 && frame_type == 0
        }
        Codec::Av1 => false,
    }
}

/// The first byte of each NAL unit of an Annex B stream: what follows every
/// three-byte start code (a four-byte one ends in the same three).
fn nal_units(stream: &[u8]) -> impl Iterator<Item = u8> + '_ {
    stream.windows(4).filter(|w| w[..3] == [0, 0, 1]).map(|w| w[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SC: [u8; 4] = [0, 0, 0, 1];

    fn annex_b(nals: &[&[u8]]) -> Vec<u8> {
        nals.iter().flat_map(|n| SC.iter().chain(n.iter()).copied()).collect()
    }

    #[test]
    fn an_h264_key_frame_is_one_with_its_parameter_sets() {
        // SPS (0x67), PPS (0x68), IDR slice (0x65): what TigerVNC's and
        // Windows' encoders send as a key frame.
        let key = annex_b(&[&[0x67, 0x64, 0x00, 0x28], &[0x68, 0xee, 0x3c], &[0x65, 0x88, 0x84, 0x00]]);
        assert!(starts_stream(Codec::H264, &key));
        // An IDR without the parameter sets, and a P slice (0x41) with them.
        assert!(!starts_stream(Codec::H264, &annex_b(&[&[0x65, 0x88, 0x84]])));
        let p = annex_b(&[&[0x67, 0x64, 0x00], &[0x68, 0xee], &[0x41, 0x9a, 0x00]]);
        assert!(!starts_stream(Codec::H264, &p));
        // Three-byte start codes count the same.
        let short: Vec<u8> = [&[0, 0, 1, 0x67, 0x64][..], &[0, 0, 1, 0x65, 0x88]].concat();
        assert!(starts_stream(Codec::H264, &short));
        assert!(!starts_stream(Codec::H264, &[]));
    }

    #[test]
    fn an_h265_key_frame_is_one_with_its_parameter_sets() {
        // VPS (32), SPS (33), PPS (34), IDR_W_RADL (19); the type is the six
        // bits after the forbidden bit.
        let key = annex_b(&[&[0x40, 0x01], &[0x42, 0x01], &[0x44, 0x01], &[0x26, 0x01, 0xaf]]);
        assert!(starts_stream(Codec::H265, &key));
        // TRAIL_R (1) with the parameter sets is not one.
        let trail = annex_b(&[&[0x40, 0x01], &[0x42, 0x01], &[0x44, 0x01], &[0x02, 0x01, 0xd0]]);
        assert!(!starts_stream(Codec::H265, &trail));
    }

    #[test]
    fn vp8_and_vp9_say_so_in_their_first_bytes() {
        let vp8_key = [0x50, 0x42, 0x00, 0x9d, 0x01, 0x2a, 0x80, 0x07, 0x38, 0x04];
        assert!(starts_stream(Codec::Vp8, &vp8_key));
        let vp8_inter = [0x51, 0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert!(!starts_stream(Codec::Vp8, &vp8_inter));
        // Profile 0: 10 0 0 0 (show_existing) 0 (key) ... and 1 for inter.
        assert!(starts_stream(Codec::Vp9, &[0x82, 0x49, 0x83]));
        assert!(!starts_stream(Codec::Vp9, &[0x86, 0x00]));
        // Profile 1 (low bit set) key, and profile 3's reserved bit.
        assert!(starts_stream(Codec::Vp9, &[0xa2, 0x49]));
        assert!(starts_stream(Codec::Vp9, &[0xb0, 0x49]));
        assert!(!starts_stream(Codec::Vp9, &[0xb2, 0x49]));
        // show_existing_frame.
        assert!(!starts_stream(Codec::Vp9, &[0x88]));
        assert!(!starts_stream(Codec::Av1, &[0x12, 0x00, 0x0a]));
    }
}
