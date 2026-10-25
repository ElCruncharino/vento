// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! 4:2:0 planes to `R, G, B, A` words, which is what a decoded frame costs
//! after the decoder has finished with it.
//!
//! The arithmetic is the same expression twice — once a row at a time and once
//! sixteen pixels at a time — and the vectorised one is used only where it has
//! been shown to agree with the other on this decoder's own output. What is in
//! doubt is never the arithmetic; it is whether the planes are where the
//! decoder's descriptor says they are, and a row of real pixels answers that
//! where an assertion about a column increment does not.
//!
//! What a decoder hands over is a [`Frame`]: the bytes its planes are in and a
//! [`Plane`] saying where in them each one is. That is two shapes of buffer
//! said one way. MediaCodec hands back **one** buffer with all three planes at
//! offsets into it, and the same slice is then given three times; libavcodec
//! allocates **each plane on its own**, and three slices that are not one
//! allocation cannot be made into one without a copy. Everything below reads a
//! plane through its own slice, so neither is a special case.
//!
//! It is a second implementation of what TigerVNC's shim does in C++ for H.264,
//! and deliberately not a shared one: the two are in different languages inside
//! different modules, and what would be shared is thirty lines of arithmetic
//! rather than a decision.

/// Where one plane is in the bytes it was handed over in, and what a row and a
/// column of it cost — MediaCodec's `image-data` descriptor, or libavcodec's
/// data pointer and line size said the same way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Plane {
    pub offset: usize,
    pub col_inc: usize,
    pub row_inc: usize,
    pub horiz_subsampling: usize,
    pub vert_subsampling: usize,
}

/// One decoded picture, as far as anything after the decoder cares: where its
/// three planes are, how much of it to read, and what its numbers mean.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    /// Y, U and V's bytes, each plane's [`Plane::offset`] counted from the
    /// start of its own. The same slice three times for a decoder with one
    /// buffer; U and V are only ever read as one interleaved run when they are.
    pub bytes: [&'a [u8]; 3],
    pub planes: [Plane; 3],
    /// The visible picture, which is the decoder's answer rather than the
    /// session's: the two differ for as long as a resize is on its way.
    pub width: usize,
    pub height: usize,
    /// Out of the bitstream's own description of itself.
    pub bt709: bool,
    pub full_range: bool,
}

impl<'a> Frame<'a> {
    /// A decoder's single buffer, with all three planes in it.
    pub fn one(
        base: &'a [u8],
        planes: [Plane; 3],
        width: usize,
        height: usize,
        bt709: bool,
        full_range: bool,
    ) -> Frame<'a> {
        Frame {
            bytes: [base; 3],
            planes,
            width,
            height,
            bt709,
            full_range,
        }
    }

    /// Whether every sample of the visible picture is inside the bytes it is
    /// said to be in. Every read below is in bounds when the last sample of
    /// each plane is, and a decoder that has just changed resolution is where
    /// that matters.
    pub fn fits(&self) -> bool {
        if self.width == 0 || self.height == 0 {
            return false;
        }
        self.planes.iter().zip(self.bytes).all(|(plane, bytes)| {
            plane.horiz_subsampling != 0
                && plane.vert_subsampling != 0
                && plane.offset
                    + ((self.height - 1) / plane.vert_subsampling) * plane.row_inc
                    + ((self.width - 1) / plane.horiz_subsampling) * plane.col_inc
                    < bytes.len()
        })
    }

    /// U and V in one run of bytes, a byte apart — NV12 or NV21, which is
    /// what a phone's decoder usually hands back. Only ever true of planes in
    /// the same slice, since that is the only case where reading one as a run
    /// of both is reading memory that is there.
    pub fn interleaved(&self) -> bool {
        let (u, v) = (&self.planes[1], &self.planes[2]);
        std::ptr::eq(self.bytes[1], self.bytes[2])
            && u.col_inc == 2
            && v.col_inc == 2
            && u.row_inc == v.row_inc
            && v.offset.abs_diff(u.offset) == 1
    }

    /// The three increments a row conversion needs.
    pub fn layout(&self) -> Layout {
        Layout {
            y_col_inc: self.planes[0].col_inc,
            uv_col_inc: self.planes[1].col_inc,
            horiz_subsampling: self.planes[1].horiz_subsampling,
        }
    }

    /// Where row `row` starts in each plane. In bounds for every row of a
    /// frame that [`fits`](Frame::fits).
    pub fn row(&self, row: usize) -> Row<'a> {
        let at = |i: usize| {
            let plane = &self.planes[i];
            &self.bytes[i][plane.offset + (row / plane.vert_subsampling) * plane.row_inc..]
        };
        Row {
            y: at(0),
            u: at(1),
            v: at(2),
        }
    }
}

/// One row of a [`Frame`]: each plane's bytes from that row's first sample on.
#[derive(Clone, Copy)]
pub struct Row<'a> {
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
}

/// The three increments a row conversion needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub y_col_inc: usize,
    pub uv_col_inc: usize,
    pub horiz_subsampling: usize,
}

/// Y, then the four chroma terms, in 1/256ths, and what Y is offset by.
#[derive(Clone, Copy)]
pub struct Coefficients {
    pub y: i32,
    pub rv: i32,
    pub gu: i32,
    pub gv: i32,
    pub bu: i32,
    pub y_offset: i32,
}

pub fn coefficients(bt709: bool, full_range: bool) -> Coefficients {
    let [y, rv, gu, gv, bu, y_offset] = match (full_range, bt709) {
        (true, true) => [256, 403, -48, -120, 475, 0],
        (true, false) => [256, 359, -88, -183, 454, 0],
        (false, true) => [298, 459, -55, -136, 541, -16],
        (false, false) => [298, 409, -100, -208, 516, -16],
    };
    Coefficients {
        y,
        rv,
        gu,
        gv,
        bu,
        y_offset,
    }
}

#[inline]
fn clamp(v: i32) -> u32 {
    v.clamp(0, 255) as u32
}

/// One row, scalar, from `col` to the end of it.
///
/// The chroma terms are computed once per chroma sample rather than once per
/// pixel, and the subsampling is a countdown rather than a division: a division
/// inside this loop, which is the obvious way to write it, cost 29 ms of a
/// decoder thread for one 1920×1200 frame in the C++ this is a second copy of —
/// more than the hardware decoder spends on the whole picture.
pub fn row_scalar(row: &Row, l: &Layout, out: &mut [u32], col: usize, w: usize, c: &Coefficients) {
    let mut yi = col * l.y_col_inc;
    let mut ui = (col / l.horiz_subsampling) * l.uv_col_inc;
    let mut vi = ui;
    let mut chroma = 0usize;
    let (mut rv, mut gu, mut bu) = (0i32, 0i32, 0i32);
    for out in out.iter_mut().take(w).skip(col) {
        if chroma == 0 {
            let uu = row.u[ui] as i32 - 128;
            let vv = row.v[vi] as i32 - 128;
            rv = c.rv * vv + 128;
            gu = c.gu * uu + c.gv * vv + 128;
            bu = c.bu * uu + 128;
            ui += l.uv_col_inc;
            vi += l.uv_col_inc;
            chroma = l.horiz_subsampling;
        }
        chroma -= 1;
        let yy = (row.y[yi] as i32 + c.y_offset) * c.y;
        yi += l.y_col_inc;
        // Red at shift 0, which is the framebuffer's word order and so the byte
        // order an Android `ARGB_8888` bitmap reads.
        *out = clamp((yy + rv) >> 8)
            | (clamp((yy + gu) >> 8) << 8)
            | (clamp((yy + bu) >> 8) << 16)
            | 0xff00_0000;
    }
}

#[cfg(target_arch = "aarch64")]
mod neon {
    use super::{Coefficients, Layout, Row};
    use std::arch::aarch64::*;

    /// The same row, sixteen pixels at a time. Returns how many it did, which
    /// is a multiple of sixteen and may be none — the tail, and every layout
    /// not covered here, goes back through the scalar loop.
    ///
    /// Sixteen rather than eight because that is what makes both chroma loads
    /// whole: eight chroma samples is one `vld1_u8` where the planes are
    /// separate and one `vld2_u8` where they are interleaved, and neither reads
    /// a byte the sixteen luma pixels do not account for.
    pub fn row(row: &Row, l: &Layout, out: &mut [u32], w: usize, c: &Coefficients) -> usize {
        if l.y_col_inc != 1 || l.horiz_subsampling != 2 {
            return 0;
        }
        // NV12 has U first and NV21 V first; one load covers both planes either
        // way, and which half is which is the only difference. Interleaved is a
        // claim about addresses, and the load below reads both through the
        // slice of whichever comes first.
        let (u, v) = (row.u.as_ptr(), row.v.as_ptr());
        let u_first = v == u.wrapping_add(1);
        let interleaved = l.uv_col_inc == 2 && (u_first || u == v.wrapping_add(1));
        if l.uv_col_inc != 1 && !interleaved {
            return 0;
        }
        let pair = if u_first { row.u } else { row.v };

        // How far the blocks read, checked here rather than promised by the
        // caller: a slice that ends early is the scalar loop's to index, which
        // panics where this would read past it.
        let end = w / 16 * 16;
        let chroma = if interleaved {
            pair.len() >= end
        } else {
            row.u.len() >= end / 2 && row.v.len() >= end / 2
        };
        if end == 0 || row.y.len() < end || out.len() < end || !chroma {
            return 0;
        }

        // SAFETY: every load is of bytes below `end` in its own slice (or
        // `end / 2` for a separate chroma plane), and every store of words below
        // `end` in `out`, which the lengths above cover.
        unsafe {
            let y_off = vdupq_n_s16(c.y_offset as i16);
            let cy = vdup_n_s16(c.y as i16);
            let crv = vdup_n_s16(c.rv as i16);
            let cgu = vdup_n_s16(c.gu as i16);
            let cgv = vdup_n_s16(c.gv as i16);
            let cbu = vdup_n_s16(c.bu as i16);
            let half = vdupq_n_s32(128);
            let opaque = vdup_n_u8(0xff);
            let mid = vdupq_n_s16(128);

            let mut col = 0usize;
            while col < end {
                let y_ptr = row.y.as_ptr().add(col);

                let (ub, vb) = if interleaved {
                    let both = vld2_u8(pair.as_ptr().add(col));
                    if u_first {
                        (both.0, both.1)
                    } else {
                        (both.1, both.0)
                    }
                } else {
                    (
                        vld1_u8(row.u.as_ptr().add(col / 2)),
                        vld1_u8(row.v.as_ptr().add(col / 2)),
                    )
                };
                let uu = vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(ub)), mid);
                let vv = vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(vb)), mid);

                // Eight chroma terms, then each duplicated onto the two luma
                // pixels it covers, which is what the zip below does.
                let mut rv = [vdupq_n_s32(0); 2];
                let mut gu = [vdupq_n_s32(0); 2];
                let mut bu = [vdupq_n_s32(0); 2];
                for i in 0..2 {
                    let uh = if i == 0 {
                        vget_low_s16(uu)
                    } else {
                        vget_high_s16(uu)
                    };
                    let vh = if i == 0 {
                        vget_low_s16(vv)
                    } else {
                        vget_high_s16(vv)
                    };
                    rv[i] = vaddq_s32(vmull_s16(vh, crv), half);
                    gu[i] = vaddq_s32(vaddq_s32(vmull_s16(uh, cgu), vmull_s16(vh, cgv)), half);
                    bu[i] = vaddq_s32(vmull_s16(uh, cbu), half);
                }

                let yraw = vld1q_u8(y_ptr);
                let ylo = vaddq_s16(
                    vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(yraw))),
                    y_off,
                );
                let yhi = vaddq_s16(
                    vreinterpretq_s16_u16(vmovl_u8(vget_high_u8(yraw))),
                    y_off,
                );
                let yy = [
                    vmull_s16(vget_low_s16(ylo), cy),
                    vmull_s16(vget_high_s16(ylo), cy),
                    vmull_s16(vget_low_s16(yhi), cy),
                    vmull_s16(vget_high_s16(yhi), cy),
                ];

                for block in 0..2 {
                    let r = (vzip1q_s32(rv[block], rv[block]), vzip2q_s32(rv[block], rv[block]));
                    let g = (vzip1q_s32(gu[block], gu[block]), vzip2q_s32(gu[block], gu[block]));
                    let b = (vzip1q_s32(bu[block], bu[block]), vzip2q_s32(bu[block], bu[block]));
                    // `vqshrun` saturates the negative side to zero and
                    // `vqmovn` the high side to 255, so the scalar clamp is two
                    // instructions here.
                    let pack = |lo: int32x4_t, hi: int32x4_t| {
                        vqmovn_u16(vcombine_u16(
                            vqshrun_n_s32::<8>(vaddq_s32(yy[block * 2], lo)),
                            vqshrun_n_s32::<8>(vaddq_s32(yy[block * 2 + 1], hi)),
                        ))
                    };
                    let px = uint8x8x4_t(
                        pack(r.0, r.1),
                        pack(g.0, g.1),
                        pack(b.0, b.1),
                        opaque,
                    );
                    vst4_u8(out.as_mut_ptr().add(col + block * 8) as *mut u8, px);
                }
                col += 16;
            }
            col
        }
    }
}

/// Whether the two agree on this decoder's first row, which is what decides
/// that the vectorised one may be used at all. One row of one frame, once per
/// layout.
pub fn vectorised_agrees(row: &Row, l: &Layout, w: usize, c: &Coefficients) -> bool {
    let mut scalar = vec![0u32; w];
    let mut vector = vec![0u32; w];
    row_scalar(row, l, &mut scalar, 0, w, c);
    let done = row_vectorised(row, l, &mut vector, w, c);
    if done == 0 {
        return false;
    }
    row_scalar(row, l, &mut vector, done, w, c);
    scalar == vector
}

/// The vectorised row where there is one, and nothing done where there is not.
pub fn row_vectorised(row: &Row, l: &Layout, out: &mut [u32], w: usize, c: &Coefficients) -> usize {
    #[cfg(target_arch = "aarch64")]
    {
        neon::row(row, l, out, w, c)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = (row, l, out, w, c);
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A planar and a semi-planar buffer of the same picture, so that a wrong
    /// column increment shows up as a wrong colour rather than as nothing.
    fn planes(w: usize, h: usize, interleaved: bool) -> (Vec<u8>, Layout, [usize; 3]) {
        let mut base = vec![0u8; w * h + w * h / 2 + 16];
        for (i, luma) in base[..w * h].iter_mut().enumerate() {
            *luma = (i * 7 % 256) as u8;
        }
        let chroma = w * h;
        for i in 0..w * h / 4 {
            if interleaved {
                base[chroma + i * 2] = (i * 11 % 256) as u8;
                base[chroma + i * 2 + 1] = (i * 13 % 256) as u8;
            } else {
                base[chroma + i] = (i * 11 % 256) as u8;
                base[chroma + w * h / 4 + i] = (i * 13 % 256) as u8;
            }
        }
        if interleaved {
            (
                base,
                Layout {
                    y_col_inc: 1,
                    uv_col_inc: 2,
                    horiz_subsampling: 2,
                },
                [0, chroma, chroma + 1],
            )
        } else {
            (
                base,
                Layout {
                    y_col_inc: 1,
                    uv_col_inc: 1,
                    horiz_subsampling: 2,
                },
                [0, chroma, chroma + w * h / 4],
            )
        }
    }

    fn row_of<'a>(base: &'a [u8], at: [usize; 3]) -> Row<'a> {
        Row {
            y: &base[at[0]..],
            u: &base[at[1]..],
            v: &base[at[2]..],
        }
    }

    /// Only where there is a second path to agree with: everywhere else the
    /// scalar loop is the whole of the conversion, and a decoder checks this
    /// again on its own first row of every layout.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn the_two_paths_agree() {
        let c = coefficients(true, false);
        for interleaved in [false, true] {
            let (base, layout, at) = planes(64, 4, interleaved);
            // A width that is not a multiple of sixteen, so the tail is
            // exercised as well as the block.
            assert!(vectorised_agrees(&row_of(&base, at), &layout, 61, &c));
        }
    }

    /// Black and white through the full-range coefficients, which is the one
    /// case the arithmetic can be checked against by hand.
    #[test]
    fn the_ends_of_the_range() {
        let c = coefficients(true, true);
        let base = [0u8, 255, 128, 128, 128, 128];
        let layout = Layout {
            y_col_inc: 1,
            uv_col_inc: 1,
            horiz_subsampling: 2,
        };
        let mut out = [0u32; 2];
        row_scalar(&row_of(&base, [0, 2, 4]), &layout, &mut out, 0, 2, &c);
        assert_eq!(out[0], 0xff00_0000);
        assert_eq!(out[1], 0xffff_ffff);
    }

    /// One buffer and three are the same picture: the planes copied out into
    /// allocations of their own, the way libavcodec hands them back, convert
    /// to the words they did in place — and are never taken for interleaved,
    /// whatever their offsets say.
    #[test]
    fn three_slices_are_one_frame() {
        let (w, h) = (32, 4);
        for interleaved in [false, true] {
            let (base, _, at) = planes(w, h, interleaved);
            let chroma = |offset: usize, col_inc: usize| Plane {
                offset,
                col_inc,
                row_inc: if interleaved { w } else { w / 2 },
                horiz_subsampling: 2,
                vert_subsampling: 2,
            };
            let inc = if interleaved { 2 } else { 1 };
            let luma = Plane {
                offset: 0,
                col_inc: 1,
                row_inc: w,
                horiz_subsampling: 1,
                vert_subsampling: 1,
            };
            let one = Frame::one(
                &base,
                [luma, chroma(at[1], inc), chroma(at[2], inc)],
                w,
                h,
                true,
                false,
            );
            assert!(one.fits());
            assert_eq!(one.interleaved(), interleaved);

            let y = base[..w * h].to_vec();
            let u = base[at[1]..].to_vec();
            let v = base[at[2]..].to_vec();
            let three = Frame {
                bytes: [&y, &u, &v],
                planes: [luma, chroma(0, inc), chroma(0, inc)],
                ..one
            };
            assert!(three.fits());
            assert!(!three.interleaved());

            let c = coefficients(one.bt709, one.full_range);
            for row in 0..h {
                let mut a = vec![0u32; w];
                let mut b = vec![0u32; w];
                row_scalar(&one.row(row), &one.layout(), &mut a, 0, w, &c);
                row_scalar(&three.row(row), &three.layout(), &mut b, 0, w, &c);
                assert_eq!(a, b, "row {row}");
            }
        }
    }

    /// A plane that ends before its last sample is a frame that does not fit,
    /// rather than an index out of bounds somewhere in a row.
    #[test]
    fn a_short_plane_does_not_fit() {
        let (base, _, at) = planes(16, 2, false);
        let luma = Plane {
            offset: 0,
            col_inc: 1,
            row_inc: 16,
            horiz_subsampling: 1,
            vert_subsampling: 1,
        };
        let chroma = Plane {
            offset: 0,
            col_inc: 1,
            row_inc: 8,
            horiz_subsampling: 2,
            vert_subsampling: 2,
        };
        let frame = Frame {
            bytes: [
                &base[..32],
                &base[at[1]..at[1] + 8],
                &base[at[2]..at[2] + 7],
            ],
            planes: [luma, chroma, chroma],
            width: 16,
            height: 2,
            bt709: true,
            full_range: false,
        };
        assert!(!frame.fits());
        let long_enough = Frame {
            bytes: [frame.bytes[0], frame.bytes[1], frame.bytes[1]],
            ..frame
        };
        assert!(long_enough.fits());
    }
}
