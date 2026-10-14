// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop as one egui texture, refreshed a damaged rectangle at a time.
//!
//! The texture is allocated once per desktop size and is never re-uploaded
//! whole after that: each frame's coalesced rectangles are read out of the
//! backend with `read_region` into a staging buffer sized for the rectangle
//! and handed to egui as an `ImageDelta::partial`, which egui-wgpu turns into
//! a `write_texture` of just that region. This is the same shape as the
//! Android `Mirror`, without the tiling — the damage is already coalesced by
//! the mailbox, and a partial texture write costs what the rectangle costs.
//!
//! Filtering follows the scale: nearest at 1:1, where it is exact, and
//! linear elsewhere, where nearest would shimmer. A change of filter is a
//! partial update of one pixel with the new options, which is how egui-wgpu
//! is told to rebuild the sampler without the pixels being sent again.

use crate::backend::Backend;
use crate::events::Rect;
use egui::{Color32, ColorImage, ImageData, TextureId, TextureOptions};

pub struct Desktop {
    id: TextureId,
    width: usize,
    height: usize,
    options: TextureOptions,
    /// What has gone through `read_region` into the texture, for the
    /// debug overlay: rectangles, bytes, and the last one.
    uploads: u64,
    bytes: u64,
    last: Option<Rect>,
}

impl Desktop {
    /// A black texture `width × height`, waiting for its first frame.
    pub fn new(ctx: &egui::Context, width: usize, height: usize) -> Desktop {
        let options = TextureOptions::LINEAR;
        let image = ColorImage::filled([width, height], Color32::BLACK);
        let id = ctx
            .tex_manager()
            .write()
            .alloc("desktop".to_string(), ImageData::Color(image.into()), options);
        Desktop {
            id,
            width,
            height,
            options,
            uploads: 0,
            bytes: 0,
            last: None,
        }
    }

    pub fn id(&self) -> TextureId {
        self.id
    }

    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// How many rectangles have been read into the texture, how many bytes
    /// that was, and the last of them.
    pub fn stats(&self) -> (u64, u64, Option<Rect>) {
        (self.uploads, self.bytes, self.last)
    }

    /// Read `rects` out of the backend and into the texture. A rectangle
    /// past the edge is clipped; one the backend refuses is skipped and will
    /// be marked again by the next frame that touches it.
    pub fn update(&mut self, ctx: &egui::Context, backend: &dyn Backend, rects: &[Rect]) {
        for rect in rects {
            let x = rect.x.min(self.width);
            let y = rect.y.min(self.height);
            let w = rect.width.min(self.width - x);
            let h = rect.height.min(self.height - y);
            if w == 0 || h == 0 {
                continue;
            }
            // The staging buffer is the image's own pixel vector, filled
            // through its bytes: `Color32` is four bytes `R, G, B, A`, which
            // is what `read_region` writes, so nothing is converted.
            let mut pixels = vec![Color32::BLACK; w * h];
            let bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut pixels);
            if !backend.read_region(x, y, w, h, bytes, w * 4, 0, 0) {
                continue;
            }
            let image = ColorImage::new([w, h], pixels);
            ctx.tex_manager()
                .write()
                .set(self.id, egui::epaint::ImageDelta::partial([x, y], image, self.options));
            self.uploads += 1;
            self.bytes += (w * h * 4) as u64;
            self.last = Some(Rect {
                x,
                y,
                width: w,
                height: h,
            });
        }
    }

    /// Nearest where a desktop pixel is a device pixel, linear otherwise.
    pub fn set_exact(&mut self, ctx: &egui::Context, backend: &dyn Backend, exact: bool) {
        let wanted = if exact {
            TextureOptions::NEAREST
        } else {
            TextureOptions::LINEAR
        };
        if wanted == self.options {
            return;
        }
        self.options = wanted;
        // One pixel, re-read, carrying the new options: the sampler changes
        // and the rest of the texture stays where it is.
        self.update(
            ctx,
            backend,
            &[Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }],
        );
    }

    pub fn free(self, ctx: &egui::Context) {
        ctx.tex_manager().write().free(self.id);
    }
}
