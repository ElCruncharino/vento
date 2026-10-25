// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A decoded picture copied into I420 — three 8-bit planes of its own, Y at
//! full size and U and V at half — for whoever converts a picture itself
//! rather than having [`Output`](super::Output) do it: FreeRDP, whose
//! AVC444 makes one picture of two before converting, and which was handed
//! I420 by every decoder it ships.
//!
//! A copy, because what a decoder hands over is lent for one call: an
//! FFmpeg frame is reused by the next decode and a MediaCodec buffer is
//! released straight after. Whatever layout the decoder left the planes in
//! — NV12's interleaved chroma off a GPU, MediaCodec's one buffer with three
//! planes at offsets — comes out the same, and is read once, here, in code
//! that is optimised in a debug build too (it is in this crate for that).

use super::convert::Frame;

/// Bytes on each row and rows below a plane past what the picture covers:
/// FreeRDP converts sixteen samples at a time, and the planes it was handed
/// before came from libavcodec, whose planes have that much and more.
pub const MARGIN: usize = 32;

/// One plane: its own memory, its rows 64-byte aligned as libavcodec's are.
#[derive(Default)]
struct Plane {
    buf: Vec<u8>,
    at: usize,
    stride: usize,
}

impl Plane {
    /// Room for `width` by `height` samples and the margin, zeroed where it
    /// had to grow; what an earlier picture left inside is overwritten by
    /// this one's before anything reads it.
    fn fit(&mut self, width: usize, height: usize) {
        let stride = (width + MARGIN).next_multiple_of(64);
        let len = stride * (height + MARGIN) + 64;
        if self.stride != stride || self.buf.len() < len {
            self.buf = vec![0; len];
            self.at = self.buf.as_ptr().align_offset(64);
            self.stride = stride;
        }
    }
}

/// The last picture [`copy`](I420::copy) was given, kept for its reader
/// until the next.
#[derive(Default)]
pub struct I420 {
    planes: [Plane; 3],
    width: usize,
    height: usize,
}

impl I420 {
    pub fn new() -> I420 {
        I420::default()
    }

    /// `frame` into the planes. False, with the planes as they were, for a
    /// picture that is not 4:2:0, or whose planes are not where it says.
    pub fn copy(&mut self, frame: &Frame<'_>) -> bool {
        let (width, height) = (frame.width, frame.height);
        let luma = &frame.planes[0];
        if !frame.fits()
            || luma.horiz_subsampling != 1
            || luma.vert_subsampling != 1
            || frame.planes[1..]
                .iter()
                .any(|p| p.horiz_subsampling != 2 || p.vert_subsampling != 2)
        {
            return false;
        }
        let chroma = (width.div_ceil(2), height.div_ceil(2));
        for (i, plane) in self.planes.iter_mut().enumerate() {
            let (w, h) = if i == 0 { (width, height) } else { chroma };
            let from = &frame.planes[i];
            plane.fit(w, h);
            for row in 0..h {
                let src = &frame.bytes[i][from.offset + row * from.row_inc..];
                let at = plane.at + row * plane.stride;
                let dst = &mut plane.buf[at..at + w];
                if from.col_inc == 1 {
                    dst.copy_from_slice(&src[..w]);
                } else {
                    for (sample, from) in dst.iter_mut().zip(src.iter().step_by(from.col_inc)) {
                        *sample = *from;
                    }
                }
            }
        }
        self.width = width;
        self.height = height;
        true
    }

    /// The picture's size, in samples of Y.
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Each plane's first sample and the bytes from one row to the next,
    /// valid until the next [`copy`](I420::copy) or until this is dropped.
    pub fn planes(&self) -> [(*const u8, usize); 3] {
        self.planes
            .each_ref()
            .map(|p| (p.buf[p.at..].as_ptr(), p.stride))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::video::convert::Plane as Layout;

    fn at(offset: usize, col_inc: usize, row_inc: usize, sub: usize) -> Layout {
        Layout {
            offset,
            col_inc,
            row_inc,
            horiz_subsampling: sub,
            vert_subsampling: sub,
        }
    }

    fn row(i420: &I420, plane: usize, row: usize, n: usize) -> Vec<u8> {
        let (data, stride) = i420.planes()[plane];
        unsafe { std::slice::from_raw_parts(data.add(row * stride), n) }.to_vec()
    }

    /// NV12 as a GPU hands it back: chroma interleaved, rows padded.
    #[test]
    fn nv12_is_deinterleaved() {
        let mut bytes: Vec<u8> = (10..22).collect();
        bytes.extend([100, 200, 101, 201, 102, 202, 103, 203]);
        let frame = Frame::one(
            &bytes,
            [at(0, 1, 4, 1), at(12, 2, 4, 2), at(13, 2, 4, 2)],
            3,
            3,
            false,
            false,
        );
        let mut i420 = I420::new();
        assert!(i420.copy(&frame));
        assert_eq!(i420.size(), (3, 3));
        for (data, stride) in i420.planes() {
            assert!((data as usize).is_multiple_of(64));
            assert!(stride >= 3 + MARGIN && stride.is_multiple_of(64));
        }
        assert_eq!(row(&i420, 0, 0, 3), [10, 11, 12]);
        assert_eq!(row(&i420, 0, 2, 3), [18, 19, 20]);
        assert_eq!(row(&i420, 1, 1, 2), [102, 103]);
        assert_eq!(row(&i420, 2, 0, 2), [200, 201]);
    }

    /// Three planes of their own, as libavcodec's software decoders make.
    #[test]
    fn planar_is_copied_row_by_row() {
        let y: Vec<u8> = (0..16).collect();
        let (u, v) = ([50, 51, 52, 53], [60, 61, 62, 63]);
        let frame = Frame {
            bytes: [&y, &u, &v],
            planes: [at(0, 1, 4, 1), at(0, 1, 2, 2), at(0, 1, 2, 2)],
            width: 4,
            height: 4,
            bt709: false,
            full_range: false,
        };
        let mut i420 = I420::new();
        assert!(i420.copy(&frame));
        assert_eq!(row(&i420, 0, 3, 4), [12, 13, 14, 15]);
        assert_eq!(row(&i420, 1, 1, 2), [52, 53]);
        assert_eq!(row(&i420, 2, 0, 2), [60, 61]);
    }

    #[test]
    fn what_is_not_420_is_refused() {
        let bytes = [0u8; 27];
        let full = at(0, 1, 3, 1);
        let frame = Frame::one(&bytes, [full, full, full], 3, 3, false, false);
        assert!(!I420::new().copy(&frame));
        // Planes said to be past the end of their bytes.
        let frame = Frame::one(
            &bytes[..10],
            [at(0, 1, 4, 1), at(12, 2, 4, 2), at(13, 2, 4, 2)],
            3,
            3,
            false,
            false,
        );
        assert!(!I420::new().copy(&frame));
    }
}
