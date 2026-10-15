// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop as egui textures, refreshed a damaged rectangle at a time.
//!
//! The textures are allocated once per desktop size and are never re-uploaded
//! whole after that: each frame's coalesced rectangles are read out of the
//! backend with `read_region` into a staging buffer sized for the rectangle
//! and handed to egui as an `ImageDelta::partial`, which egui-wgpu turns into
//! a `write_texture` of just that region. The staging buffers come back once
//! egui has let go of them and are read into again, so that a rectangle is
//! not first filled with black only for `read_region` to overwrite it. This is the same shape as the
//! Android `Mirror` — the damage is already coalesced by the mailbox, and a
//! partial texture write costs what the rectangle costs.
//!
//! A desktop that fits in the device's largest texture is one texture. One
//! that does not — a row of monitors past 16384 pixels, or past 8192 on
//! hardware with that limit — is cut into tiles, a texture each, drawn side by
//! side. A tile's texture holds a row or column of each neighbour's pixels
//! around its own, and only its own are drawn: linear filtering at a seam,
//! which reaches half a pixel past the edge, then blends the same pixels it
//! would have in one texture rather than clamping to the edge, so the seam
//! does not show. The cost is that damage along a seam is written into both
//! tiles. The tiles are drawn as plain meshes rather than egui images, which
//! the tessellator feathers at their edges for anti-aliasing: at a seam that
//! would be a faint line.
//!
//! Filtering follows the scale: nearest at 1:1, where it is exact, and
//! linear elsewhere, where nearest would shimmer. A change of filter is a
//! partial update of one pixel per tile with the new options, which is how
//! egui-wgpu is told to rebuild the sampler without the pixels being sent
//! again.

use super::view::Layout;
use crate::backend::Backend;
use crate::events::Rect;
use egui::emath::GuiRounding;
use egui::{Color32, ColorImage, ImageData, Mesh, Shape, TextureId, TextureOptions, pos2, vec2};
use std::sync::Arc;

/// How many staging buffers are kept for reuse, at most.
const SPARES: usize = 8;

/// One texture's worth of the desktop.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tile {
    id: TextureId,
    /// The desktop pixels drawn from this tile.
    own: Rect,
    /// The desktop pixels its texture holds: `own`, and a row or column of
    /// each neighbour around it where there is one.
    held: Rect,
}

pub struct Desktop {
    tiles: Vec<Tile>,
    width: usize,
    height: usize,
    options: TextureOptions,
    /// What has gone through `read_region` into the textures, for the
    /// debug overlay: rectangles, bytes, and the last one.
    uploads: u64,
    bytes: u64,
    last: Option<Rect>,
    staging: Staging,
}

/// The staging buffers, kept for reuse.
#[derive(Default)]
struct Staging {
    /// Handed to egui with an upload, until it has applied them and
    /// dropped its share.
    lent: Vec<Arc<ColorImage>>,
    /// The ones it is done with, to be read into again.
    spare: Vec<Vec<Color32>>,
}

impl Staging {
    /// A buffer of `n` pixels for `read_region` to write all of. One egui
    /// is done with comes back to be written again: whatever it held is
    /// overwritten, and only a buffer shorter than `n` is lengthened, which
    /// fills just the part it lacked. The spares are the longest few and,
    /// together, no more than `budget` pixels — a desktop's worth, so that
    /// what is kept between frames is bounded by the framebuffer the backend
    /// already holds.
    fn take(&mut self, n: usize, budget: usize) -> Vec<Color32> {
        for image in std::mem::take(&mut self.lent) {
            match Arc::try_unwrap(image) {
                Ok(image) => self.spare.push(image.pixels),
                Err(image) => self.lent.push(image),
            }
        }
        self.spare.sort_by_key(|v| std::cmp::Reverse(v.len()));
        let mut kept = 0;
        self.spare.retain(|v| {
            kept += v.capacity();
            kept <= budget
        });
        self.spare.truncate(SPARES);
        // The shortest that is long enough, or else the longest there is.
        let pick = self.spare.iter().rposition(|v| v.len() >= n).or((!self.spare.is_empty()).then_some(0));
        let mut pixels = pick.map(|i| self.spare.remove(i)).unwrap_or_default();
        if pixels.len() >= n {
            pixels.truncate(n);
        } else {
            pixels.resize(n, Color32::BLACK);
        }
        pixels
    }
}

impl Desktop {
    /// Black textures for a desktop `width × height`, waiting for its first
    /// frame: as many as the device's largest texture makes it.
    pub fn new(ctx: &egui::Context, width: usize, height: usize) -> Desktop {
        let options = TextureOptions::LINEAR;
        let max = ctx.input(|i| i.max_texture_side);
        let tiles: Vec<Tile> = grid(width, height, max)
            .into_iter()
            .map(|(own, held)| {
                let image = ColorImage::filled([held.width, held.height], Color32::BLACK);
                let id = ctx
                    .tex_manager()
                    .write()
                    .alloc("desktop".to_string(), ImageData::Color(image.into()), options);
                Tile { id, own, held }
            })
            .collect();
        if tiles.len() > 1 {
            log::info!(
                "a {width}×{height} desktop is past this device's {max}-pixel textures: {} tiles",
                tiles.len()
            );
        }
        Desktop {
            tiles,
            width,
            height,
            options,
            uploads: 0,
            bytes: 0,
            last: None,
            staging: Staging::default(),
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// How many rectangles have been read into the textures, how many bytes
    /// that was, and the last of them.
    pub fn stats(&self) -> (u64, u64, Option<Rect>) {
        (self.uploads, self.bytes, self.last)
    }

    /// Read `rects` out of the backend and into the textures. A rectangle
    /// past the edge is clipped; one the backend refuses is skipped and will
    /// be marked again by the next frame that touches it.
    pub fn update(&mut self, ctx: &egui::Context, backend: &dyn Backend, rects: &[Rect]) {
        let whole = Rect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        };
        for rect in rects {
            let Some(rect) = overlap(*rect, whole) else {
                continue;
            };
            for tile in 0..self.tiles.len() {
                if let Some(part) = overlap(rect, self.tiles[tile].held) {
                    self.upload(ctx, backend, tile, part);
                }
            }
        }
    }

    /// Read `rect`, which is inside the tile's `held`, into its texture.
    fn upload(&mut self, ctx: &egui::Context, backend: &dyn Backend, tile: usize, rect: Rect) {
        let Rect { x, y, width: w, height: h } = rect;
        // The staging buffer is the image's own pixel vector, filled
        // through its bytes: `Color32` is four bytes `R, G, B, A`, which
        // is what `read_region` writes, so nothing is converted.
        let mut pixels = self.staging.take(w * h, self.width * self.height);
        let bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut pixels);
        if !backend.read_region(x, y, w, h, bytes, w * 4, 0, 0) {
            self.staging.spare.push(pixels);
            return;
        }
        let image = Arc::new(ColorImage::new([w, h], pixels));
        self.staging.lent.push(Arc::clone(&image));
        let Tile { id, held, .. } = self.tiles[tile];
        ctx.tex_manager().write().set(
            id,
            egui::epaint::ImageDelta::partial([x - held.x, y - held.y], image, self.options),
        );
        self.uploads += 1;
        self.bytes += (w * h * 4) as u64;
        self.last = Some(rect);
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
        // One pixel per tile, re-read, carrying the new options: the
        // samplers change and the rest of each texture stays where it is.
        for tile in 0..self.tiles.len() {
            let held = self.tiles[tile].held;
            let corner = Rect {
                x: held.x,
                y: held.y,
                width: 1,
                height: 1,
            };
            self.upload(ctx, backend, tile, corner);
        }
    }

    /// Draw the layout's `source` where it lands: each tile's share of it.
    pub fn paint(&self, painter: &egui::Painter, layout: &Layout, pixels_per_point: f32) {
        // On device pixels, as egui puts an image's rectangle, so that at 1:1
        // a desktop pixel is a device pixel and nearest is exact.
        let onto = layout.rect.round_to_pixels(pixels_per_point);
        for (tile, rect, uv) in pieces(&self.tiles, layout.source, onto) {
            let mut mesh = Mesh::with_texture(self.tiles[tile].id);
            mesh.add_rect_with_uv(rect, uv, Color32::WHITE);
            painter.add(Shape::mesh(mesh));
        }
    }

    pub fn free(self, ctx: &egui::Context) {
        let textures = ctx.tex_manager();
        let mut textures = textures.write();
        for tile in self.tiles {
            textures.free(tile.id);
        }
    }
}

/// Where two rectangles overlap, if they do.
fn overlap(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    (x < right && y < bottom).then(|| Rect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

/// One axis of `len` pixels cut for textures at most `max` long, as `[own
/// start, own end, held start, held end]`: one span with nothing held past
/// it where the axis fits, and otherwise spans of `max - 2` that each hold a
/// pixel either side, which keeps every texture within `max`.
fn spans(len: usize, max: usize) -> Vec<[usize; 4]> {
    if len <= max {
        return vec![[0, len, 0, len]];
    }
    let step = max.saturating_sub(2).max(1);
    (0..len)
        .step_by(step)
        .map(|start| {
            let end = (start + step).min(len);
            [start, end, start.saturating_sub(1), (end + 1).min(len)]
        })
        .collect()
}

/// The tiles of a desktop `width × height`, as `(own, held)`, row by row.
fn grid(width: usize, height: usize, max: usize) -> Vec<(Rect, Rect)> {
    let mut tiles = Vec::new();
    for [y0, y1, hy0, hy1] in spans(height, max) {
        for [x0, x1, hx0, hx1] in spans(width, max) {
            let own = Rect {
                x: x0,
                y: y0,
                width: x1 - x0,
                height: y1 - y0,
            };
            let held = Rect {
                x: hx0,
                y: hy0,
                width: hx1 - hx0,
                height: hy1 - hy0,
            };
            tiles.push((own, held));
        }
    }
    tiles
}

/// What to draw for `source` (desktop pixels) shown in `onto` (points): for
/// each tile with a share of it, the tile, where that share lands, and the
/// share as UV in the tile's texture. A seam's position is computed the same
/// way from either side, so neighbours meet without a gap or an overlap.
fn pieces(tiles: &[Tile], source: egui::Rect, onto: egui::Rect) -> Vec<(usize, egui::Rect, egui::Rect)> {
    if source.width() <= 0.0 || source.height() <= 0.0 {
        return Vec::new();
    }
    let scale = onto.size() / source.size();
    let at = |d: egui::Pos2| {
        pos2(
            if d.x >= source.max.x { onto.max.x } else { onto.min.x + (d.x - source.min.x) * scale.x },
            if d.y >= source.max.y { onto.max.y } else { onto.min.y + (d.y - source.min.y) * scale.y },
        )
    };
    let float = |r: Rect| egui::Rect::from_min_size(pos2(r.x as f32, r.y as f32), vec2(r.width as f32, r.height as f32));
    let mut out = Vec::new();
    for (i, tile) in tiles.iter().enumerate() {
        let share = float(tile.own).intersect(source);
        if share.width() <= 0.0 || share.height() <= 0.0 {
            continue;
        }
        let held = float(tile.held);
        let uv = egui::Rect::from_min_max(
            ((share.min - held.min) / held.size()).to_pos2(),
            ((share.max - held.min) / held.size()).to_pos2(),
        );
        out.push((i, egui::Rect::from_min_max(at(share.min), at(share.max)), uv));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: usize, y: usize, w: usize, h: usize) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn tiles(width: usize, height: usize, max: usize) -> Vec<Tile> {
        grid(width, height, max)
            .into_iter()
            .map(|(own, held)| Tile {
                id: TextureId::default(),
                own,
                held,
            })
            .collect()
    }

    fn frect(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(pos2(x, y), vec2(w, h))
    }

    #[test]
    fn a_staging_buffer_comes_back_once_egui_lets_go_of_it() {
        let mut staging = Staging::default();
        let mut pixels = staging.take(4, 100);
        assert_eq!(pixels, [Color32::BLACK; 4]);
        pixels[0] = Color32::RED;
        let image = Arc::new(ColorImage::new([2, 2], pixels));
        staging.lent.push(Arc::clone(&image));
        // Still held, as egui holds a delta until it is drawn: a new one.
        let other = staging.take(4, 100);
        assert_eq!(other[0], Color32::BLACK);
        drop(image);
        // Let go of: the same pixels, not filled again.
        let again = staging.take(4, 100);
        assert_eq!(again[0], Color32::RED);
        assert!(staging.lent.is_empty());
    }

    #[test]
    fn a_staging_buffer_is_the_shortest_that_fits_and_the_spares_are_bounded() {
        let mut staging = Staging::default();
        for n in [10, 40, 20] {
            staging.spare.push(vec![Color32::RED; n]);
        }
        // Fifteen: the twenty, shortened, with nothing filled.
        let pixels = staging.take(15, 1000);
        assert_eq!(pixels, [Color32::RED; 15]);
        // Fifty: the longest, lengthened by what it lacked.
        let pixels = staging.take(50, 1000);
        assert_eq!(pixels[..40], [Color32::RED; 40]);
        assert_eq!(pixels[40..], [Color32::BLACK; 10]);
        // A budget of fewer pixels than the spares hold drops the shorter.
        staging.spare = vec![vec![Color32::RED; 60], vec![Color32::RED; 30]];
        let _ = staging.take(1, 70);
        assert!(staging.spare.is_empty());
        staging.spare = (0..SPARES + 3).map(|_| vec![Color32::RED; 1]).collect();
        let _ = staging.take(1, 1000);
        assert_eq!(staging.spare.len(), SPARES - 1);
    }

    #[test]
    fn a_desktop_that_fits_is_one_texture_with_nothing_held_past_it() {
        assert_eq!(grid(1920, 1200, 8192), [(rect(0, 0, 1920, 1200), rect(0, 0, 1920, 1200))]);
        assert_eq!(grid(8192, 8192, 8192).len(), 1);
    }

    #[test]
    fn a_wide_desktop_is_cut_into_tiles_that_hold_a_pixel_of_each_neighbour() {
        let grid = grid(17000, 1200, 8192);
        assert_eq!(
            grid,
            [
                (rect(0, 0, 8190, 1200), rect(0, 0, 8191, 1200)),
                (rect(8190, 0, 8190, 1200), rect(8189, 0, 8192, 1200)),
                (rect(16380, 0, 620, 1200), rect(16379, 0, 621, 1200)),
            ]
        );
        // Every texture is within the limit, and the own parts cover the
        // desktop exactly once.
        assert!(grid.iter().all(|(_, held)| held.width <= 8192 && held.height <= 8192));
        assert_eq!(grid.iter().map(|(own, _)| own.width).sum::<usize>(), 17000);
        // Both ways at once: a grid, row by row.
        let grid = super::grid(20000, 9000, 8192);
        assert_eq!(grid.len(), 3 * 2);
        assert_eq!(grid[4].0, rect(8190, 8190, 8190, 810));
        assert_eq!(grid[4].1, rect(8189, 8189, 8192, 811));
    }

    #[test]
    fn damage_along_a_seam_goes_into_both_tiles() {
        let tiles = tiles(17000, 1200, 8192);
        let damage = rect(8180, 10, 20, 5);
        let parts: Vec<Option<Rect>> = tiles.iter().map(|t| overlap(damage, t.held)).collect();
        assert_eq!(parts, [Some(rect(8180, 10, 11, 5)), Some(rect(8189, 10, 11, 5)), None]);
        assert_eq!(overlap(rect(0, 0, 5, 5), rect(5, 0, 5, 5)), None);
    }

    #[test]
    fn one_texture_is_drawn_as_before_and_a_monitor_is_its_share() {
        let tiles = tiles(3200, 800, 8192);
        // The whole desktop: all of the texture, all of the rectangle.
        let pieces = pieces(&tiles, frect(0.0, 0.0, 3200.0, 800.0), frect(10.0, 20.0, 1600.0, 400.0));
        assert_eq!(pieces, [(0, frect(10.0, 20.0, 1600.0, 400.0), frect(0.0, 0.0, 1.0, 1.0))]);
        // One monitor of it: that monitor's share of the texture.
        let pieces = pieces_of(&tiles, frect(1920.0, 0.0, 1280.0, 800.0));
        assert_eq!(pieces, [(0, frect(0.0, 0.0, 1280.0, 800.0), egui::Rect::from_min_max(pos2(0.6, 0.0), pos2(1.0, 1.0)))]);
    }

    fn pieces_of(tiles: &[Tile], source: egui::Rect) -> Vec<(usize, egui::Rect, egui::Rect)> {
        pieces(tiles, source, frect(0.0, 0.0, source.width(), source.height()))
    }

    #[test]
    fn tiles_meet_at_their_seams_and_draw_only_their_own_pixels() {
        let tiles = tiles(17000, 1200, 8192);
        // Shown at a third of its size, as fit width might.
        let onto = frect(0.0, 0.0, 17000.0 / 3.0, 400.0);
        let pieces = pieces(&tiles, frect(0.0, 0.0, 17000.0, 1200.0), onto);
        assert_eq!(pieces.len(), 3);
        // Each seam is the same number from both sides, and the last edge is
        // the rectangle's own.
        assert_eq!(pieces[0].1.max.x, pieces[1].1.min.x);
        assert_eq!(pieces[1].1.max.x, pieces[2].1.min.x);
        assert_eq!(pieces[0].1.min.x, 0.0);
        assert_eq!(pieces[2].1.max.x, onto.max.x);
        // A tile's UV covers its own pixels, inside the border it holds: the
        // middle one starts a pixel in and ends a pixel short.
        let (_, _, uv) = pieces[1];
        assert_eq!(uv.min.x, 1.0 / 8192.0);
        assert_eq!(uv.max.x, 8191.0 / 8192.0);
        assert_eq!((uv.min.y, uv.max.y), (0.0, 1.0));
        assert_eq!(pieces[0].2.max.x, 8190.0 / 8191.0);
        assert_eq!(pieces[2].2.min.x, 1.0 / 621.0);
    }

    #[test]
    fn a_monitor_inside_one_tile_draws_that_tile_alone_and_one_across_a_seam_both() {
        let tiles = tiles(17000, 1200, 8192);
        let inside = pieces_of(&tiles, frect(10000.0, 0.0, 1920.0, 1200.0));
        assert_eq!(inside.len(), 1);
        assert_eq!(inside[0].0, 1);
        assert_eq!(inside[0].1, frect(0.0, 0.0, 1920.0, 1200.0));
        let across = pieces_of(&tiles, frect(7680.0, 0.0, 1920.0, 1200.0));
        assert_eq!(across.iter().map(|p| p.0).collect::<Vec<_>>(), [0, 1]);
        // The seam at desktop x 8190 is 510 points in.
        assert_eq!(across[0].1, frect(0.0, 0.0, 510.0, 1200.0));
        assert_eq!(across[1].1, frect(510.0, 0.0, 1410.0, 1200.0));
    }
}
