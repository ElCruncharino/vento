// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The image half of the display channel: five encodings, two caches, and what
//! a draw's source rectangle does to what comes out.
//!
//! The decoders are theirs and the dispatch is ours, which is the split note 76
//! §2 chose. What is here besides the dispatch is the part a decoder cannot do
//! for itself: which images go in which cache, and the crop and scale that turn
//! a decoded image into the rectangle a draw command names.

use flate2::read::ZlibDecoder;
use shakenfist_spice_compression::{
    ByteBoundedLru, DecompressedImage, GlzDictionary, JpegDecoder, JpegDecoderRsDecoder,
    decompress_glz, decompress_lz, decompress_spice_lz4, quic_decode,
};
use shakenfist_spice_protocol::constants::{IMAGE_FLAGS_CACHE_ME, ImageType};
use shakenfist_spice_protocol::messages::ImageDescriptor;
use std::collections::BTreeSet;
use std::io::Read;
use std::sync::Arc;

/// A decoded image: `R, G, B, A` bytes, row-major, top-down.
///
/// The size is a claim about the pixels rather than a property of them, and
/// the claim comes from the server (a descriptor) or from a decoder of
/// theirs: [`fit`], the one place an image is indexed by its size, checks the
/// two agree first.
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

pub struct Images {
    /// GLZ's cross-image dictionary, which is the one piece of state that is
    /// neither per-image nor per-frame: an image can be a run of back
    /// references into images that came before it, so **display messages are
    /// decoded in wire order and never skipped**. Out of order, `decompress_glz`
    /// waits for an entry that will not arrive, and on a current-thread runtime
    /// that is a hang rather than a stall.
    dictionary: GlzDictionary,
    /// The server's own image cache, which is a different thing: it names an
    /// image the server has sent before and expects this end to have kept. Its
    /// size is what `DISPLAY_INIT` promised, so a client that keeps less than it
    /// said is a client that will miss. Each entry is the image's size and
    /// then its pixels ([`keep`]), since the descriptor that later names it
    /// carries a size of its own and is not where the pixels came from.
    cache: ByteBoundedLru,
    jpeg: Arc<dyn JpegDecoder>,
    /// What has actually arrived, for the panel's encoding row: a list of what
    /// the server chose rather than of what this client can decode.
    seen: BTreeSet<&'static str>,
}

impl Images {
    pub fn new(cache_bytes: usize) -> Images {
        Images {
            dictionary: GlzDictionary::new(),
            cache: ByteBoundedLru::new(cache_bytes),
            // The pure-Rust decoder, which is the only one of theirs that
            // builds for a phone: the others are ImageIO, WIC, VA-API and a
            // vendored libjpeg-turbo.
            jpeg: Arc::new(JpegDecoderRsDecoder::new()),
            seen: BTreeSet::new(),
        }
    }

    /// The image a draw command points at, decoded, cached where the server
    /// asked, and cropped and scaled to the box the draw names.
    ///
    /// `src` is the draw's own source rectangle inside the image, and
    /// `(width, height)` the destination box: SPICE lets an image be smaller
    /// than the box it fills and says how to stretch it, which is the case a
    /// scaled desktop produces.
    pub async fn draw(
        &mut self,
        descriptor: &ImageDescriptor,
        data: &[u8],
        src: (u32, u32, u32, u32),
        width: usize,
        height: usize,
    ) -> Option<Image> {
        let image = self.decode(descriptor, data).await?;
        fit(image, src, width, height)
    }

    /// Every image encoding this client understands, dispatched on the
    /// descriptor's type. The uncompressed one is ours because nobody
    /// compresses it; the rest are theirs.
    async fn decode(&mut self, descriptor: &ImageDescriptor, data: &[u8]) -> Option<Image> {
        let kind = ImageType::from_u8(descriptor.image_type);
        self.seen.insert(name(descriptor.image_type));
        // QUIC and LZ4 are handed the descriptor's size and allocate by it, so
        // a size no desktop could be is refused before they are asked.
        let pixels = (descriptor.width as usize).checked_mul(descriptor.height as usize);
        if pixels.is_none_or(|n| n > common::MAX_DESKTOP_PIXELS) {
            log::warn!("display: a {}x{} image", descriptor.width, descriptor.height);
            return None;
        }
        let decoded: Option<DecompressedImage> = match kind {
            Some(ImageType::Pixmap) => pixmap(data, descriptor.image_id),
            // The four-byte prefix is the compressed size, which the message
            // framing has already told us.
            Some(ImageType::LzRgb) => decompress_lz(data.get(4..)?).ok(),
            Some(ImageType::GlzRgb) => decompress_glz(data.get(4..)?, &self.dictionary).await.ok(),
            Some(ImageType::ZlibGlzRgb) => {
                // A GLZ image with zlib over it: the uncompressed size, the
                // compressed size, and then the stream.
                let compressed = u32::from_le_bytes(data.get(4..8)?.try_into().ok()?) as usize;
                let zlib = data.get(8..8 + compressed.min(data.len() - 8))?;
                let mut glz = Vec::new();
                ZlibDecoder::new(zlib).read_to_end(&mut glz).ok()?;
                decompress_glz(&glz, &self.dictionary).await.ok()
            }
            // The prefix is the compressed size here as well, whatever the
            // reference client's comment says: what follows it is the
            // `top_down` byte and the bitmap format, and a decoder handed the
            // size instead reads the low byte of a length as an orientation.
            Some(ImageType::Lz4) => decompress_spice_lz4(
                data.get(4..)?,
                descriptor.width as usize,
                descriptor.height as usize,
            ),
            Some(ImageType::Quic) => quic_decode(data.get(4..)?, descriptor.width, descriptor.height)
                .map(|pixels| {
                    DecompressedImage::new(descriptor.width, descriptor.height, pixels, descriptor.image_id)
                }),
            Some(ImageType::Jpeg) => {
                let jpeg = data.get(4..)?;
                let decoded = self.jpeg.decode(jpeg)?;
                Some(DecompressedImage::new(
                    decoded.width,
                    decoded.height,
                    decoded.rgba,
                    descriptor.image_id,
                ))
            }
            // A JPEG and its alpha channel, which is a *separate* LZ stream
            // rather than a fourth channel in the JPEG — and is what this server
            // sends for every photographic image once it has decided the link is
            // slow, since a QXL surface is 32 bits deep and so always has an
            // alpha to carry. A client that decodes plain JPEG and not this one
            // shows the desktop with a hole where the picture is.
            Some(ImageType::JpegAlpha) => {
                let jpeg_size = u32::from_le_bytes(data.get(1..5)?.try_into().ok()?) as usize;
                let data_size = u32::from_le_bytes(data.get(5..9)?.try_into().ok()?) as usize;
                let body = data.get(9..9 + data_size.min(data.len() - 9))?;
                let decoded = self.jpeg.decode(body.get(..jpeg_size)?)?;
                let mut pixels = decoded.rgba;
                // A failed alpha is opaque rather than nothing: the colour is
                // already decoded and every draw but a blend ignores the
                // channel anyway.
                if !lz_alpha(body.get(jpeg_size..)?, &mut pixels) {
                    log::warn!("display: a JPEG's alpha channel did not decode");
                }
                Some(DecompressedImage::new(
                    decoded.width,
                    decoded.height,
                    pixels,
                    descriptor.image_id,
                ))
            }
            // The image at the size it was kept, whatever this descriptor
            // says: a server that names a cached pixel as 100×100 has its
            // pixel drawn over the box, rather than a hundred rows read past
            // it.
            Some(ImageType::FromCache) | Some(ImageType::FromCacheLossless) => {
                return kept(&mut self.cache, descriptor.image_id);
            }
            other => {
                log::warn!("display: image type {other:?} is not decoded here");
                None
            }
        };
        let Some(decoded) = decoded else {
            // A decoder that says no is a picture with a hole in it, and the
            // hole is silent: worth one line whatever the encoding.
            log::warn!("display: a {} image did not decode", name(descriptor.image_type));
            return None;
        };

        // A GLZ image is always kept, whether or not the server asked: the
        // dictionary is what the *next* one may refer back to. The window it
        // names is how far back that can reach, so anything older goes.
        if matches!(kind, Some(ImageType::GlzRgb) | Some(ImageType::ZlibGlzRgb)) {
            self.dictionary
                .insert(decoded.image_id, decoded.pixels.clone());
            if decoded.win_head_dist > 0 {
                self.dictionary
                    .evict_older_than(decoded.image_id.saturating_sub(decoded.win_head_dist as u64));
            }
        } else if descriptor.flags & IMAGE_FLAGS_CACHE_ME != 0 {
            keep(&mut self.cache, &decoded);
        }

        Some(Image {
            width: decoded.width as usize,
            height: decoded.height as usize,
            pixels: decoded.pixels,
        })
    }

    /// Everything the display channel is decoding, in the panel's words.
    pub fn encodings(&self) -> Vec<&'static str> {
        self.seen.iter().copied().collect()
    }

    /// `INVAL_ALL_PIXMAPS` and `RESET`: the server is saying that what this end
    /// remembers is no longer what it thinks this end remembers.
    pub fn forget(&mut self) {
        self.dictionary.clear();
        self.cache.clear();
    }
}

/// Into the server's cache, as the image's width and height and then its
/// pixels: one entry, so the size is evicted with the pixels it describes.
fn keep(cache: &mut ByteBoundedLru, image: &DecompressedImage) {
    let mut entry = Vec::with_capacity(8 + image.pixels.len());
    entry.extend_from_slice(&image.width.to_le_bytes());
    entry.extend_from_slice(&image.height.to_le_bytes());
    entry.extend_from_slice(&image.pixels);
    cache.insert(image.image_id, entry);
}

/// What [`keep`] put there, or nothing for an id this end did not keep.
fn kept(cache: &mut ByteBoundedLru, id: u64) -> Option<Image> {
    let entry = cache.get(&id)?;
    let (size, pixels) = entry.split_at_checked(8)?;
    Some(Image {
        width: u32::from_le_bytes(size[0..4].try_into().ok()?) as usize,
        height: u32::from_le_bytes(size[4..8].try_into().ok()?) as usize,
        pixels: pixels.to_vec(),
    })
}

/// A 32-bit SPICE bitmap, which is the uncompressed case: a header of its own
/// and then rows that are bottom-up unless the flag says otherwise.
fn pixmap(data: &[u8], id: u64) -> Option<DecompressedImage> {
    if data.len() < 18 {
        return None;
    }
    let format = data[0];
    let flags = data[1];
    let width = u32::from_le_bytes(data[2..6].try_into().ok()?) as usize;
    let height = u32::from_le_bytes(data[6..10].try_into().ok()?) as usize;
    let stride = u32::from_le_bytes(data[10..14].try_into().ok()?) as usize;
    let top_down = flags & 0x04 != 0;
    // 8 is BGRX and 9 is BGRA; the paletted and 16-bit formats belong to
    // guests whose driver predates a 32-bit surface, and QEMU's does not.
    if format != 8 && format != 9 {
        log::warn!("display: a {format}-format bitmap is not 32-bit");
        return None;
    }
    let rows = &data[18..];
    // Checked, as a 32-bit phone's `usize` is the fields' own width.
    let row = width.checked_mul(4)?;
    if width == 0 || height == 0 || stride < row || stride.checked_mul(height)? > rows.len() {
        return None;
    }
    let mut pixels = vec![0u8; row.checked_mul(height)?];
    for y in 0..height {
        let source = if top_down { y } else { height - 1 - y };
        let src = &rows[source * stride..source * stride + width * 4];
        let dst = &mut pixels[y * width * 4..(y + 1) * width * 4];
        for (out, px) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            out[0] = px[2];
            out[1] = px[1];
            out[2] = px[0];
            out[3] = if format == 9 { px[3] } else { 0xff };
        }
    }
    Some(DecompressedImage::new(
        width as u32,
        height as u32,
        pixels,
        id,
    ))
}

/// The alpha channel of a `JPEG_ALPHA` image, written into `pixels` in place.
///
/// It is LZ again, and the same control bytes as `decompress_lz` — but with
/// **one-byte pixels**: an alpha channel is one byte each, so a literal run is
/// that many bytes and a back reference counts in bytes rather than in
/// triplets. Their decoder assumes three, which is why this is here rather than
/// a second call into theirs; everything else about the stream, header
/// included, is the same.
///
/// False for a stream that is not one, in which case the caller keeps the
/// colour and an opaque alpha.
fn lz_alpha(data: &[u8], pixels: &mut [u8]) -> bool {
    /// A control byte below this is a literal run of `ctrl + 1` pixels.
    const MAX_COPY: u8 = 32;
    const HEADER: usize = 28;

    if data.len() < HEADER || &data[0..4] != b"  ZL" {
        return false;
    }
    // Big-endian, unlike everything else on this wire: magic, version, three
    // bytes of padding, the type, and then the shape.
    let width = u32::from_be_bytes(data[12..16].try_into().unwrap()) as usize;
    let height = u32::from_be_bytes(data[16..20].try_into().unwrap()) as usize;
    let top_down = u32::from_be_bytes(data[24..28].try_into().unwrap()) != 0;
    let count = width.saturating_mul(height);
    if count == 0 || count * 4 > pixels.len() {
        return false;
    }

    let mut alpha = vec![0u8; count];
    let mut at = HEADER;
    let mut out = 0usize;
    while out < count && at < data.len() {
        let ctrl = data[at];
        at += 1;
        if ctrl < MAX_COPY {
            let run = (ctrl as usize + 1).min(count - out).min(data.len() - at);
            alpha[out..out + run].copy_from_slice(&data[at..at + run]);
            at += run;
            out += run;
            continue;
        }
        let mut length = (ctrl >> 5) as usize;
        let mut offset = ((ctrl & 0x1f) as usize) << 8;
        if length == 7 {
            // A run longer than the three bits hold, as bytes that add up and
            // end with one below 255.
            while at < data.len() {
                let more = data[at];
                at += 1;
                length += more as usize;
                if more != 255 {
                    break;
                }
            }
        }
        if at >= data.len() {
            break;
        }
        let low = data[at] as usize;
        at += 1;
        offset += low;
        if low == 255 && offset - low == 31 << 8 {
            if at + 2 > data.len() {
                break;
            }
            offset = ((data[at] as usize) << 8) | data[at + 1] as usize;
            offset += 8191;
            at += 2;
        }
        offset += 1;
        if offset > out {
            return false;
        }
        for _ in 0..length {
            if out >= count {
                break;
            }
            alpha[out] = alpha[out - offset];
            out += 1;
        }
    }

    for y in 0..height {
        let row = if top_down { y } else { height - 1 - y };
        for x in 0..width {
            pixels[(y * width + x) * 4 + 3] = alpha[row * width + x];
        }
    }
    true
}

/// The draw's source rectangle out of the image, at the size of the box it goes
/// in — nearest neighbour, which is what SPICE's own default scale mode is.
///
/// The ordinary case is that neither happens: the image is exactly the
/// rectangle, and this hands it straight back.
///
/// Nothing for an image whose pixels are not the size it claims, and for a
/// box bigger than any desktop could be ([`common::MAX_DESKTOP_PIXELS`]):
/// both sizes are the server's word, and this is where they are indexed by.
fn fit(image: Image, src: (u32, u32, u32, u32), width: usize, height: usize) -> Option<Image> {
    let claimed = image.width.checked_mul(image.height).and_then(|n| n.checked_mul(4));
    if claimed != Some(image.pixels.len()) {
        log::warn!(
            "display: a {}x{} image of {} bytes",
            image.width,
            image.height,
            image.pixels.len()
        );
        return None;
    }
    let (left, top, right, bottom) = (
        (src.0 as usize).min(image.width),
        (src.1 as usize).min(image.height),
        (src.2 as usize).min(image.width),
        (src.3 as usize).min(image.height),
    );
    let (crop_w, crop_h) = (right.saturating_sub(left), bottom.saturating_sub(top));
    if crop_w == 0 || crop_h == 0 {
        return Some(image);
    }
    if left == 0 && top == 0 && crop_w == image.width && crop_h == image.height
        && (width == 0 || height == 0 || (width == image.width && height == image.height))
    {
        return Some(image);
    }
    let (width, height) = if width == 0 || height == 0 {
        (crop_w, crop_h)
    } else {
        (width, height)
    };
    let count = width.checked_mul(height).filter(|&n| n <= common::MAX_DESKTOP_PIXELS);
    let Some(count) = count else {
        log::warn!("display: a draw into a {width}x{height} box");
        return None;
    };
    let mut pixels = vec![0u8; count * 4];
    for y in 0..height {
        let sy = top + y * crop_h / height;
        for x in 0..width {
            let sx = left + x * crop_w / width;
            let from = (sy * image.width + sx) * 4;
            let to = (y * width + x) * 4;
            pixels[to..to + 4].copy_from_slice(&image.pixels[from..from + 4]);
        }
    }
    Some(Image {
        width,
        height,
        pixels,
    })
}

/// What the panel calls each encoding.
fn name(image_type: u8) -> &'static str {
    match image_type {
        0 => "Bitmap",
        1 => "QUIC",
        100 => "LZ (paletted)",
        101 => "LZ",
        102 => "GLZ",
        103 | 106 => "Cached",
        104 => "Surface",
        105 => "JPEG",
        107 => "GLZ (zlib)",
        108 => "JPEG with alpha",
        109 => "LZ4",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: usize, height: usize, f: impl Fn(usize, usize) -> [u8; 4]) -> Image {
        let mut pixels = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&f(x, y));
            }
        }
        Image {
            width,
            height,
            pixels,
        }
    }

    /// A bottom-up bitmap is the default, and getting it the wrong way up is
    /// a picture that looks nearly right — which is why it has a test.
    #[test]
    fn a_bitmap_without_the_flag_is_bottom_up() {
        let mut data = vec![8u8, 0]; // format 32-bit BGRX, no flags
        data.extend_from_slice(&1u32.to_le_bytes()); // width
        data.extend_from_slice(&2u32.to_le_bytes()); // height
        data.extend_from_slice(&4u32.to_le_bytes()); // stride
        data.extend_from_slice(&0u32.to_le_bytes()); // palette
        data.extend_from_slice(&[0x11, 0x22, 0x33, 0xff]); // bottom row, BGRX
        data.extend_from_slice(&[0x44, 0x55, 0x66, 0xff]); // top row

        let decoded = pixmap(&data, 1).unwrap();
        assert_eq!(&decoded.pixels[0..4], &[0x66, 0x55, 0x44, 0xff], "top row");
        assert_eq!(&decoded.pixels[4..8], &[0x33, 0x22, 0x11, 0xff], "bottom");
    }

    /// The one that matters for a picture rather than for a crash: a bitmap
    /// whose stride is wider than its pixels, which is every bitmap a server
    /// pads to four bytes.
    #[test]
    fn a_padded_stride_is_not_read_as_pixels() {
        let mut data = vec![8u8, 0x04]; // top-down
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes()); // one pixel, eight bytes
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&[1, 2, 3, 0xff, 0, 0, 0, 0]);
        data.extend_from_slice(&[4, 5, 6, 0xff, 0, 0, 0, 0]);

        let decoded = pixmap(&data, 1).unwrap();
        assert_eq!(&decoded.pixels[0..4], &[3, 2, 1, 0xff]);
        assert_eq!(&decoded.pixels[4..8], &[6, 5, 4, 0xff]);
    }

    #[test]
    fn a_short_bitmap_is_refused_rather_than_read_past() {
        let mut data = vec![8u8, 0x04];
        data.extend_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(&16u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&[0; 16]); // one row where four were promised
        assert!(pixmap(&data, 1).is_none());
    }

    /// The ordinary case has to cost nothing: the whole image into a box its
    /// own size is the same pixels back.
    #[test]
    fn a_whole_image_into_its_own_box_is_untouched() {
        let source = image(2, 2, |x, y| [x as u8, y as u8, 0, 0xff]);
        let fitted = fit(source, (0, 0, 2, 2), 2, 2).unwrap();
        assert_eq!(fitted.width, 2);
        assert_eq!(&fitted.pixels[0..4], &[0, 0, 0, 0xff]);
    }

    #[test]
    fn a_source_rectangle_crops() {
        let source = image(4, 4, |x, y| [x as u8, y as u8, 0, 0xff]);
        let fitted = fit(source, (1, 2, 3, 4), 2, 2).unwrap();
        assert_eq!((fitted.width, fitted.height), (2, 2));
        assert_eq!(&fitted.pixels[0..4], &[1, 2, 0, 0xff], "top left is (1,2)");
        assert_eq!(&fitted.pixels[12..16], &[2, 3, 0, 0xff], "and (2,3)");
    }

    /// A one-pixel top-down BGRA bitmap, and the descriptor that asks for it
    /// to be kept as `id`.
    fn cached_pixel(id: u64) -> (ImageDescriptor, Vec<u8>) {
        let mut data = vec![9u8, 0x04];
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        let descriptor = ImageDescriptor {
            image_id: id,
            image_type: ImageType::Pixmap as u8,
            flags: IMAGE_FLAGS_CACHE_ME,
            width: 1,
            height: 1,
        };
        (descriptor, data)
    }

    /// A cached image is the size it was when it was kept. The descriptor
    /// that names it carries a size too, and a server that says 100×100 for
    /// a pixel it cached is a server whose word is not what the pixels are:
    /// what is drawn is the pixel, stretched over the box, rather than a
    /// read a hundred rows past it.
    #[test]
    fn a_cached_image_is_its_own_size_whatever_the_descriptor_says() {
        use crate::testing::block_on;
        let mut images = Images::new(1 << 20);
        let (keep, data) = cached_pixel(7);
        assert!(block_on(images.draw(&keep, &data, (0, 0, 1, 1), 1, 1)).is_some());
        for kind in [ImageType::FromCache, ImageType::FromCacheLossless] {
            let named = ImageDescriptor {
                image_id: 7,
                image_type: kind as u8,
                flags: 0,
                width: 100,
                height: 100,
            };
            let image = block_on(images.draw(&named, &[], (0, 0, 50, 50), 50, 50)).unwrap();
            assert_eq!((image.width, image.height), (50, 50));
            assert_eq!(image.pixels.len(), 50 * 50 * 4);
            assert_eq!(&image.pixels[..4], &[0x33, 0x22, 0x11, 0x44]);
        }
    }

    /// A draw's box is two 32-bit fields of the server's, and a box no
    /// desktop could hold is refused before an image that size is made.
    #[test]
    fn a_box_bigger_than_any_desktop_is_refused() {
        use crate::testing::block_on;
        let mut images = Images::new(1 << 20);
        let (keep, data) = cached_pixel(7);
        assert!(block_on(images.draw(&keep, &data, (0, 0, 1, 1), 100_000, 100_000)).is_none());
        let (keep, data) = cached_pixel(8);
        assert!(block_on(images.draw(&keep, &data, (0, 0, 1, 1), 1 << 33, 1 << 33)).is_none());
    }

    /// An image whose pixels are not the size it says is refused by the one
    /// function that indexes it by that size.
    #[test]
    fn an_image_that_is_not_its_own_size_is_refused() {
        let image = Image {
            width: 100,
            height: 100,
            pixels: vec![0; 4],
        };
        assert!(fit(image, (0, 0, 50, 50), 50, 50).is_none());
    }

    /// An image smaller than its box is stretched rather than left in a corner
    /// with the rest of the box untouched.
    #[test]
    fn an_image_smaller_than_its_box_is_scaled() {
        let source = image(2, 1, |x, _| [x as u8 * 10, 0, 0, 0xff]);
        let fitted = fit(source, (0, 0, 2, 1), 4, 2).unwrap();
        assert_eq!((fitted.width, fitted.height), (4, 2));
        let at = |i: usize| fitted.pixels[i * 4];
        assert_eq!([at(0), at(1), at(2), at(3)], [0, 0, 10, 10]);
        assert_eq!(at(4), 0, "and the second row is the first one again");
    }
}
