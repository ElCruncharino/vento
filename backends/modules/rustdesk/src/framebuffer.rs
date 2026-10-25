// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop's pixels: the picture shared by two threads, and the buffer the
//! next picture is decoded into.

use std::sync::Arc;

use runtime::media::video::{Changes, GpuPicture};

/// A framebuffer of `R, G, B, A` words, so a read is a row copy into an Android
/// `ARGB_8888` bitmap.
///
/// Written by the protocol thread under a write lock and read by the drawing
/// thread under a read lock, and the write is a **pointer swap**: this protocol
/// has no damage rectangles, so a key frame is a whole 1920×1200 picture, and
/// converting it in place under the lock would block the drawing thread for
/// the whole conversion, a third of a frame's time at 30 fps. The decoder
/// converts into its own buffer and [`swap`](Self::swap) installs it, holding
/// the lock only for a `Vec` swap. [`Back`] handles the cost of that buffer
/// when a frame converts only part of itself.
///
/// Alternatively the picture is on the GPU ([`show_gpu`](Self::show_gpu)),
/// when a window that draws there asked for it to stay there: the pixels here
/// are then out of date until read, and the first read
/// ([`catch_up`](Self::catch_up)) converts the picture into them, once.
pub struct Framebuffer {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
    gpu: Option<Arc<dyn GpuPicture>>,
    behind: bool,
}

impl Framebuffer {
    pub fn new() -> Framebuffer {
        Framebuffer {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            gpu: None,
            behind: false,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Nothing is preserved: a desktop that has changed size is repainted from
    /// a key frame.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels.resize(width * height, 0xff00_0000);
        self.gpu = None;
        self.behind = false;
    }

    /// Installs `next` as the picture, returning the previous buffer for the
    /// decoder to draw the following frame into.
    ///
    /// A mismatched size is refused rather than resized: the caller resizes
    /// when the far end reports a desktop change, and a frame arriving in
    /// between is one frame of the old size.
    pub fn swap(&mut self, next: &mut Vec<u32>) -> bool {
        if next.len() != self.width * self.height || self.width == 0 {
            return false;
        }
        std::mem::swap(&mut self.pixels, next);
        self.gpu = None;
        self.behind = false;
        true
    }

    /// Installs a picture left on the GPU as the picture. Refused, as a swap
    /// is, if it is not this desktop's size.
    pub fn show_gpu(&mut self, picture: Arc<dyn GpuPicture>) -> bool {
        if (picture.width(), picture.height()) != (self.width, self.height) || self.width == 0 {
            return false;
        }
        self.gpu = Some(picture);
        self.behind = true;
        true
    }

    /// The picture, if it is on the GPU.
    pub fn gpu(&self) -> Option<Arc<dyn GpuPicture>> {
        self.gpu.clone()
    }

    /// Whether the pixels here are older than a picture on the GPU, so a read
    /// has to [`catch_up`](Self::catch_up) first.
    pub fn behind(&self) -> bool {
        self.behind
    }

    /// Reads the picture on the GPU back into the pixels here, if they are
    /// out of date: leaving it on the GPU avoids this readback unless the
    /// pixels are read. If it cannot be read the pixels stay as they were,
    /// rather than retrying on every read.
    pub fn catch_up(&mut self) {
        if !std::mem::take(&mut self.behind) {
            return;
        }
        if let Some(picture) = &self.gpu
            && !picture.read(&mut self.pixels, self.width, self.height)
        {
            log::warn!("the picture on the GPU could not be read back");
        }
    }

    /// Copies the rows selected by `behind` from the picture here into `dst`,
    /// the buffer the next [`swap`](Self::swap) will take. Returns false, with
    /// nothing copied, if `dst` is not this desktop's size.
    ///
    /// A frame that converts only part of itself needs this to fill in the
    /// rest of its buffer: see [`Back`].
    pub fn carry_rows(&self, dst: &mut [u32], behind: impl Fn(usize) -> bool) -> bool {
        if self.width == 0 || dst.len() != self.width * self.height {
            return false;
        }
        for row in 0..self.height {
            if !behind(row) {
                continue;
            }
            let at = row * self.width;
            dst[at..at + self.width].copy_from_slice(&self.pixels[at..at + self.width]);
        }
        true
    }

    /// Copies a desktop rectangle out as `R, G, B, A` bytes, one row at a time,
    /// into a destination with its own stride. False if there is nothing to
    /// read or the rectangle is not wholly inside the desktop, since a partial
    /// result would show as a half-drawn tile.
    pub fn read_rgba(
        &self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        dst: &mut [u8],
        dst_stride: usize,
    ) -> bool {
        if self.width == 0 || w == 0 || h == 0 {
            return false;
        }
        if x + w > self.width || y + h > self.height {
            return false;
        }
        if dst_stride < w * 4 || dst.len() < (h - 1) * dst_stride + w * 4 {
            return false;
        }
        for row in 0..h {
            let from = (y + row) * self.width + x;
            let src = &self.pixels[from..from + w];
            let to = row * dst_stride;
            // The word order is the bitmap's by construction, so this is a
            // reinterpretation rather than a conversion.
            let (_, bytes, _) = unsafe { src.align_to::<u8>() };
            dst[to..to + w * 4].copy_from_slice(bytes);
        }
        true
    }

    /// The whole desktop at `1/step`, nearest neighbour: the home screen's
    /// thumbnail.
    pub fn thumbnail(&self, step: usize) -> Option<(usize, usize, Vec<u32>)> {
        if self.width == 0 || step == 0 {
            return None;
        }
        let tw = self.width.div_ceil(step);
        let th = self.height.div_ceil(step);
        let mut out = Vec::with_capacity(tw * th);
        for row in 0..th {
            let sy = (row * step).min(self.height - 1);
            for col in 0..tw {
                let sx = (col * step).min(self.width - 1);
                out.push(self.pixels[sy * self.width + sx]);
            }
        }
        Some((tw, th, out))
    }
}

impl Default for Framebuffer {
    fn default() -> Framebuffer {
        Framebuffer::new()
    }
}

/// The buffer a frame is decoded into, and the rows the last two frames wrote
/// to it.
///
/// [`Framebuffer::swap`] takes this buffer and returns the one that was on
/// screen, so a buffer just returned holds the frame **before** last. That
/// does not matter while every frame is converted whole, but once a frame
/// converts only the rows that changed, a row it does not touch is two
/// pictures out of date rather than one.
///
/// So the rows the *last* frame converted are copied from the picture now on
/// screen before the swap takes it; every other row was left alone by both of
/// the last two frames, and is therefore already correct.
pub struct Back {
    pub pixels: Vec<u32>,
    /// The rows this frame converted, and the rows the last one did.
    pub changed: Changes,
    carried: Changes,
    /// Whether these pixels have lost track of the picture on screen. Set when
    /// a frame did not go on screen, and cleared by converting the next one
    /// whole, since an unknown buffer cannot be repaired row by row.
    stale: bool,
    /// Rows converted by the last frame that went on screen, which measures
    /// the frame's cost for the session's cost log.
    converted: usize,
}

impl Back {
    pub fn new() -> Back {
        Back {
            pixels: Vec::new(),
            changed: Changes::new(),
            carried: Changes::new(),
            stale: true,
            converted: 0,
        }
    }

    /// Nothing is preserved, and the next frame is converted whole.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.pixels.clear();
        self.pixels.resize(width * height, 0xff00_0000);
        self.stale = true;
    }

    /// Starts an update: nothing has changed yet, unless these pixels have
    /// lost track of the picture on screen, in which case all of it has.
    pub fn begin(&mut self, width: usize, height: usize) {
        self.changed.reset(height);
        self.converted = 0;
        if self.stale {
            self.changed.all(width, height);
        }
    }

    /// Updates the rows this frame did not convert from the picture on screen:
    /// every row the last frame converted and this one did not. False if that
    /// picture is not this buffer's size, and the buffer is then marked stale
    /// rather than left half correct.
    pub fn carry(&mut self, from: &Framebuffer) -> bool {
        let Back {
            pixels,
            changed,
            carried,
            stale,
            ..
        } = self;
        if !from.carry_rows(pixels, |row| carried.row(row) && !changed.row(row)) {
            *stale = true;
            return false;
        }
        true
    }

    /// The picture went on screen: the rows this frame converted are the ones
    /// the next frame will have to copy.
    pub fn swapped(&mut self) {
        self.converted = self.changed.count();
        std::mem::swap(&mut self.changed, &mut self.carried);
        self.stale = false;
    }

    /// How many rows the last frame put on screen, or zero for a frame that
    /// did not go on screen.
    pub fn converted(&self) -> usize {
        self.converted
    }

    /// The picture did not go on screen, so the next frame is converted whole.
    pub fn lost(&mut self) {
        self.stale = true;
    }
}

impl Default for Back {
    fn default() -> Back {
        Back::new()
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A picture on the GPU that reads back as one colour, counting reads.
    pub struct Flat {
        pub size: (usize, usize),
        pub colour: u32,
        pub reads: std::sync::atomic::AtomicUsize,
    }

    impl GpuPicture for Flat {
        fn width(&self) -> usize {
            self.size.0
        }
        fn height(&self) -> usize {
            self.size.1
        }
        fn surface(&self) -> (u64, u32) {
            (1, 1)
        }
        fn colour(&self) -> (bool, bool) {
            (false, false)
        }
        fn platform(&self) -> &dyn std::any::Any {
            self
        }
        fn read(&self, out: &mut [u32], width: usize, height: usize) -> bool {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            out[..width * height].fill(self.colour);
            true
        }
    }

    pub fn flat(width: usize, height: usize, colour: u32) -> Arc<Flat> {
        Arc::new(Flat {
            size: (width, height),
            colour,
            reads: Default::default(),
        })
    }

    #[test]
    fn a_picture_on_the_gpu_is_read_back_once_and_only_when_read() {
        let mut fb = Framebuffer::new();
        fb.resize(2, 2);
        let picture = flat(2, 2, 7);
        assert!(fb.show_gpu(picture.clone()));
        assert!(fb.behind());
        assert!(fb.gpu().is_some());
        assert_eq!(picture.reads.load(std::sync::atomic::Ordering::Relaxed), 0);
        fb.catch_up();
        fb.catch_up();
        assert_eq!(picture.reads.load(std::sync::atomic::Ordering::Relaxed), 1);
        let mut out = [0u8; 16];
        assert!(fb.read_rgba(0, 0, 2, 2, &mut out, 8));
        assert_eq!(u32::from_le_bytes(out[0..4].try_into().unwrap()), 7);
        // A picture of another size is refused, as a swap would be.
        assert!(!fb.show_gpu(flat(3, 2, 1)));
        // A picture converted on the CPU replaces it.
        let mut next = vec![1u32, 2, 3, 4];
        assert!(fb.swap(&mut next));
        assert!(fb.gpu().is_none() && !fb.behind());
        fb.catch_up();
        assert!(fb.read_rgba(0, 0, 2, 2, &mut out, 8));
        assert_eq!(u32::from_le_bytes(out[0..4].try_into().unwrap()), 1);
    }

    #[test]
    fn a_swapped_frame_reads_back() {
        let mut fb = Framebuffer::new();
        fb.resize(2, 2);
        let mut next = vec![1u32, 2, 3, 4];
        assert!(fb.swap(&mut next));
        // The old buffer is returned, so the next frame needs no allocation.
        assert_eq!(next.len(), 4);
        let mut out = [0u8; 16];
        assert!(fb.read_rgba(0, 0, 2, 2, &mut out, 8));
        assert_eq!(u32::from_le_bytes(out[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(out[12..16].try_into().unwrap()), 4);
    }

    #[test]
    fn a_frame_of_the_wrong_size_is_refused() {
        let mut fb = Framebuffer::new();
        fb.resize(2, 2);
        let mut next = vec![0u32; 3];
        assert!(!fb.swap(&mut next));
    }

    const W: usize = 4;
    const H: usize = 4;

    /// One update of a picture with one colour per row: the rows in `moved`
    /// are converted and the rest are left alone, as a decoder does.
    fn frame(fb: &mut Framebuffer, back: &mut Back, picture: &[u32; H], moved: &[usize]) {
        back.begin(W, H);
        for &row in moved {
            back.changed.mark(row, 0, W);
        }
        for (row, colour) in picture.iter().enumerate() {
            if back.changed.row(row) {
                back.pixels[row * W..row * W + W].fill(*colour);
            }
        }
        assert!(back.carry(fb));
        assert!(fb.swap(&mut back.pixels));
        back.swapped();
    }

    fn on_screen(fb: &Framebuffer) -> [u32; H] {
        std::array::from_fn(|row| fb.pixels[row * W])
    }

    /// The key requirement of converting part of a frame: the buffer returned
    /// by a swap holds the frame *before* last, so a row the new frame does not
    /// convert has to be copied from the picture that has just gone on screen.
    #[test]
    fn a_row_that_is_not_converted_comes_forward() {
        let mut fb = Framebuffer::new();
        fb.resize(W, H);
        let mut back = Back::new();
        back.resize(W, H);

        let mut picture = [1u32; H];
        frame(&mut fb, &mut back, &picture, &[]); // the first frame is converted whole
        assert_eq!(on_screen(&fb), [1, 1, 1, 1]);

        picture[1] = 2;
        frame(&mut fb, &mut back, &picture, &[1]);
        assert_eq!(on_screen(&fb), [1, 2, 1, 1]);

        // Row 1 is the one tested: it changed in the last frame and not in
        // this one, so the buffer being drawn into still holds its older value.
        picture[2] = 3;
        frame(&mut fb, &mut back, &picture, &[2]);
        assert_eq!(on_screen(&fb), [1, 2, 3, 1]);
    }

    #[test]
    fn a_frame_that_did_not_go_on_screen_converts_the_next_one_whole() {
        let mut back = Back::new();
        back.resize(W, H);
        back.begin(W, H);
        back.swapped();

        back.lost();
        back.begin(W, H);
        assert!((0..H).all(|row| back.changed.row(row)));
    }

    #[test]
    fn read_outside_is_refused() {
        let mut fb = Framebuffer::new();
        fb.resize(4, 3);
        let mut out = [0u8; 64];
        assert!(!fb.read_rgba(3, 0, 2, 1, &mut out, 8));
    }

    #[test]
    fn thumbnail_halves() {
        let mut fb = Framebuffer::new();
        fb.resize(4, 4);
        let mut next = vec![0u32; 16];
        next[0..4].copy_from_slice(&[1, 2, 3, 4]);
        assert!(fb.swap(&mut next));
        let (w, h, px) = fb.thumbnail(2).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(&px[0..2], &[1, 3]);
    }
}
