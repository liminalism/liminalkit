//! Images prepared once for the size they are drawn at.
//!
//! [`Painter::blit`] resamples on every draw, a float sample and a blend per
//! destination pixel. That is right for something drawn once; it is wrong for
//! a photo in a list row that scrolls at 60 Hz on a Raspberry Pi. A
//! [`PreparedImage`] does the expensive part once — resample to the display
//! size, premultiply, round the corners — and drawing it is then a row copy
//! where the image is opaque and a premultiplied span blend where it is not.
//!
//! Resampling is area-averaging when shrinking (a 1200 px photo at 160 px
//! averages every source pixel rather than sampling four of them and aliasing
//! the rest away) and linear when enlarging, separably, in premultiplied
//! space, so transparent edges never fringe.
//!
//! Memory is the destination size only: `4 × width × height` bytes, plus one
//! intermediate pass of `16 × source height × width` that is freed before
//! returning.

use crate::bitmap::{alpha_of, Bitmap};
use crate::blend::{div255, over_pixel, over_span};
use crate::color::mix;
use crate::kernel::{FillRule, RasterKernel};
use crate::painter::{Painter, Rect};
use crate::path::Path;

/// How a bitmap of one shape fills a rectangle of another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Distort to fill exactly.
    Stretch,
    /// Fill, cropping the overflow evenly from both sides. Photos.
    Cover,
    /// Fit inside, transparent margins where the shapes differ. Logos.
    Contain,
}

/// A premultiplied `0xAARRGGBB` image at its final drawing size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedImage {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
    /// Per row, where the visible pixels are and where the longest opaque
    /// run among them is: drawing skips the transparent margins, copies the
    /// opaque run, and blends only what is left.
    rows: Vec<RowSpan>,
}

impl PreparedImage {
    /// Resample `bitmap` (straight alpha, as decoded) to `width × height`,
    /// with corners rounded to `corner_radius` pixels (0 for square).
    pub fn from_bitmap(
        bitmap: &Bitmap,
        width: u32,
        height: u32,
        fit: Fit,
        corner_radius: f32,
    ) -> PreparedImage {
        let mut pixels = vec![0u32; width as usize * height as usize];
        if bitmap.width > 0 && bitmap.height > 0 && width > 0 && height > 0 {
            let (sw, sh) = (bitmap.width as f64, bitmap.height as f64);
            let (dw, dh) = (f64::from(width), f64::from(height));
            // Source window and destination window, in each one's pixels.
            let (src, dst) = match fit {
                Fit::Stretch => ([0.0, 0.0, sw, sh], [0, 0, width, height]),
                Fit::Cover => {
                    let scale = (dw / sw).max(dh / sh);
                    let (cw, ch) = (dw / scale, dh / scale);
                    (
                        [(sw - cw) / 2.0, (sh - ch) / 2.0, cw, ch],
                        [0, 0, width, height],
                    )
                }
                Fit::Contain => {
                    let scale = (dw / sw).min(dh / sh);
                    let iw = ((sw * scale).round() as u32).clamp(1, width);
                    let ih = ((sh * scale).round() as u32).clamp(1, height);
                    (
                        [0.0, 0.0, sw, sh],
                        [(width - iw) / 2, (height - ih) / 2, iw, ih],
                    )
                }
            };
            resample(bitmap, src, dst, width, &mut pixels);
        }
        let mut image = PreparedImage {
            width,
            height,
            pixels,
            rows: Vec::new(),
        };
        image.round_corners(corner_radius);
        image.index_rows();
        image
    }

    /// Generate an image pixel by pixel: `paint(x, y)` returns straight-alpha
    /// `0xAARRGGBB`. For backgrounds, tiles and placeholders computed once.
    pub fn from_fn(
        width: u32,
        height: u32,
        corner_radius: f32,
        mut paint: impl FnMut(u32, u32) -> u32,
    ) -> PreparedImage {
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height {
            for x in 0..width {
                pixels.push(premultiply(paint(x, y)));
            }
        }
        let mut image = PreparedImage {
            width,
            height,
            pixels,
            rows: Vec::new(),
        };
        image.round_corners(corner_radius);
        image.index_rows();
        image
    }

    /// An opaque linear gradient from `from` to `to` (`0xRRGGBB`), top to
    /// bottom or left to right.
    pub fn linear_gradient(
        width: u32,
        height: u32,
        corner_radius: f32,
        from: u32,
        to: u32,
        vertical: bool,
    ) -> PreparedImage {
        let span = if vertical { height } else { width }
            .saturating_sub(1)
            .max(1) as f32;
        PreparedImage::from_fn(width, height, corner_radius, |x, y| {
            let t = if vertical { y } else { x } as f32 / span;
            0xff00_0000 | mix(from, to, t)
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Premultiplied `0xAARRGGBB`.
    pub fn pixel(&self, x: u32, y: u32) -> u32 {
        self.pixels[(y * self.width + x) as usize]
    }

    pub fn byte_size(&self) -> usize {
        self.pixels.len() * 4
    }

    fn round_corners(&mut self, radius: f32) {
        let radius = radius.min(self.width.min(self.height) as f32 / 2.0);
        if radius <= 0.0 || self.pixels.is_empty() {
            return;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let mut coverage = vec![0u8; w * h];
        let mut path = Path::new();
        path.rounded_rect(0.0, 0.0, w as f32, h as f32, radius);
        path.fill_with(
            &mut RasterKernel::new(),
            w,
            h,
            FillRule::NonZero,
            |y, x0, x1, row| {
                coverage[y * w + x0..=y * w + x1].copy_from_slice(&row[x0..=x1]);
            },
        );
        for (pixel, &c) in self.pixels.iter_mut().zip(&coverage) {
            *pixel = scale_premultiplied(*pixel, c);
        }
    }

    fn index_rows(&mut self) {
        let w = self.width as usize;
        self.rows = (0..self.height as usize)
            .map(|y| RowSpan::of(&self.pixels[y * w..(y + 1) * w]))
            .collect();
    }
}

/// One row's shape, in pixels from the row's start: visible in
/// `[start, end)`, opaque in `[opaque_start, opaque_end)` (which may be empty).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowSpan {
    start: usize,
    end: usize,
    opaque_start: usize,
    opaque_end: usize,
}

impl RowSpan {
    fn of(row: &[u32]) -> RowSpan {
        let visible = |p: &u32| p >> 24 != 0;
        let start = row.iter().position(visible).unwrap_or(row.len());
        let end = row.iter().rposition(visible).map_or(start, |i| i + 1);
        let (mut best, mut run) = ((start, start), None::<usize>);
        for (i, p) in row.iter().enumerate().take(end).skip(start) {
            if p >> 24 == 0xff {
                let from = *run.get_or_insert(i);
                if i + 1 - from > best.1 - best.0 {
                    best = (from, i + 1);
                }
            } else {
                run = None;
            }
        }
        RowSpan {
            start,
            end,
            opaque_start: best.0,
            opaque_end: best.1,
        }
    }
}

impl Painter<'_> {
    /// Draw a prepared image with its top-left corner at `(x, y)`.
    pub fn draw_image(&mut self, image: &PreparedImage, x: i32, y: i32) {
        self.draw_image_faded(image, x, y, 255);
    }

    /// Draw at `opacity` (255 opaque), for fades and disabled states.
    pub fn draw_image_faded(&mut self, image: &PreparedImage, x: i32, y: i32, opacity: u8) {
        if opacity == 0 {
            return;
        }
        let xrgb = !self.is_premultiplied();
        let w = image.width as usize;
        for (row, span) in image.rows.iter().enumerate() {
            if span.start == span.end {
                continue;
            }
            let Some((skip, dest)) = self.clipped_run(x, y + row as i32, w) else {
                continue;
            };
            let source = &image.pixels[row * w..(row + 1) * w];
            // Part `[from, to)` of the row, intersected with what is visible.
            let visible = skip..skip + dest.len();
            let mut part = |from: usize, to: usize, opaque: bool| {
                let (from, to) = (from.max(visible.start), to.min(visible.end));
                if from >= to {
                    return;
                }
                let (src, dst) = (&source[from..to], &mut dest[from - skip..to - skip]);
                if opacity != 255 {
                    for (d, &s) in dst.iter_mut().zip(src) {
                        *d = over_pixel(*d, scale_premultiplied(s, opacity), xrgb);
                    }
                } else if opaque {
                    if xrgb {
                        // XRGB keeps its top byte clear.
                        for (d, &s) in dst.iter_mut().zip(src) {
                            *d = s & 0x00ff_ffff;
                        }
                    } else {
                        dst.copy_from_slice(src);
                    }
                } else {
                    over_span(dst, src, xrgb);
                }
            };
            part(span.start, span.opaque_start, false);
            part(span.opaque_start, span.opaque_end, true);
            part(span.opaque_end, span.end, false);
        }
    }

    /// Fill `rect` with a linear gradient between two `0xRRGGBB` colours,
    /// computed per row (vertical) or once per column and copied (horizontal):
    /// a full-screen gradient costs about what a full-screen fill does.
    pub fn fill_gradient(&mut self, rect: Rect, from: u32, to: u32, vertical: bool) {
        let premultiplied = self.is_premultiplied();
        let opaque = |c: u32| if premultiplied { c | 0xff00_0000 } else { c };
        let w = rect.w.max(0) as usize;
        if vertical {
            let span = (rect.h - 1).max(1) as f32;
            for row in 0..rect.h {
                let color = opaque(mix(from, to, row as f32 / span));
                if let Some((_, dest)) = self.clipped_run(rect.x, rect.y + row, w) {
                    dest.fill(color);
                }
            }
        } else {
            let span = (rect.w - 1).max(1) as f32;
            let colors: Vec<u32> = (0..rect.w)
                .map(|col| opaque(mix(from, to, col as f32 / span)))
                .collect();
            for row in 0..rect.h {
                if let Some((skip, dest)) = self.clipped_run(rect.x, rect.y + row, w) {
                    let n = dest.len();
                    dest.copy_from_slice(&colors[skip..skip + n]);
                }
            }
        }
    }
}

#[inline]
fn scale_premultiplied(pixel: u32, by: u8) -> u32 {
    if by == 255 {
        return pixel;
    }
    let by = u32::from(by);
    let channel = |shift: u32| div255(((pixel >> shift) & 0xff) * by + 127) << shift;
    channel(24) | channel(16) | channel(8) | channel(0)
}

#[inline]
fn premultiply(pixel: u32) -> u32 {
    let a = u32::from(alpha_of(pixel));
    if a == 255 {
        return pixel;
    }
    let channel = |shift: u32| div255(((pixel >> shift) & 0xff) * a + 127) << shift;
    (a << 24) | channel(16) | channel(8) | channel(0)
}

/// Contributions of source pixels to one destination pixel along one axis.
struct Taps {
    first: usize,
    weights: Vec<f32>,
}

/// Weights for `count` destination pixels drawn from `[start, start + len)`
/// of a `limit`-pixel source axis: box (area) when shrinking, tent (linear)
/// when enlarging. Each set sums to one.
fn taps(start: f64, len: f64, count: usize, limit: usize) -> Vec<Taps> {
    let scale = len / count as f64;
    let last = limit as f64;
    (0..count)
        .map(|i| {
            let mut out = if scale >= 1.0 {
                let a = (start + i as f64 * scale).clamp(0.0, last);
                let b = (a + scale).clamp(0.0, last);
                let first = a.floor() as usize;
                let end = (b.ceil() as usize).min(limit).max(first + 1);
                let weights = (first..end)
                    .map(|j| ((b.min(j as f64 + 1.0) - a.max(j as f64)).max(0.0)) as f32)
                    .collect();
                Taps { first, weights }
            } else {
                let centre = (start + (i as f64 + 0.5) * scale - 0.5).clamp(0.0, last - 1.0);
                let first = centre.floor() as usize;
                let t = (centre - first as f64) as f32;
                if first + 1 < limit {
                    Taps {
                        first,
                        weights: vec![1.0 - t, t],
                    }
                } else {
                    Taps {
                        first,
                        weights: vec![1.0],
                    }
                }
            };
            let sum: f32 = out.weights.iter().sum();
            if sum > 0.0 {
                out.weights.iter_mut().for_each(|w| *w /= sum);
            }
            out
        })
        .collect()
}

/// Separable resample of `src` window `[x, y, w, h]` into `dst` window
/// `[x, y, w, h]` of an output `stride` pixels wide.
fn resample(bitmap: &Bitmap, src: [f64; 4], dst: [u32; 4], stride: u32, out: &mut [u32]) {
    let [dx, dy, dw, dh] = dst.map(|v| v as usize);
    let (sw, sh) = (bitmap.width as usize, bitmap.height as usize);
    let columns = taps(src[0], src[2], dw, sw);
    let rows = taps(src[1], src[3], dh, sh);
    let row_first = rows.first().map_or(0, |t| t.first);
    let row_end = rows
        .iter()
        .map(|t| t.first + t.weights.len())
        .max()
        .unwrap_or(0);

    // Horizontal pass over only the source rows the vertical pass will read.
    let mut horizontal = vec![[0f32; 4]; (row_end - row_first) * dw];
    for sy in row_first..row_end {
        let line = &bitmap.pixels[sy * sw..(sy + 1) * sw];
        for (x, tap) in columns.iter().enumerate() {
            let mut acc = [0f32; 4];
            for (k, &weight) in tap.weights.iter().enumerate() {
                let p = premultiply(line[tap.first + k]);
                for (c, shift) in [24, 16, 8, 0].into_iter().enumerate() {
                    acc[c] += ((p >> shift) & 0xff) as f32 * weight;
                }
            }
            horizontal[(sy - row_first) * dw + x] = acc;
        }
    }
    for (y, tap) in rows.iter().enumerate() {
        for x in 0..dw {
            let mut acc = [0f32; 4];
            for (k, &weight) in tap.weights.iter().enumerate() {
                let sample = horizontal[(tap.first + k - row_first) * dw + x];
                for c in 0..4 {
                    acc[c] += sample[c] * weight;
                }
            }
            let byte = |v: f32| (v.round().clamp(0.0, 255.0)) as u32;
            let alpha = byte(acc[0]);
            // Premultiplied colour can never exceed its alpha.
            let channel = |v: f32| byte(v).min(alpha);
            out[(dy + y) * stride as usize + dx + x] =
                (alpha << 24) | (channel(acc[1]) << 16) | (channel(acc[2]) << 8) | channel(acc[3]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitmap::argb;
    use crate::painter::WindowBuffer;

    fn checker(size: u32) -> Bitmap {
        let mut b = Bitmap::new(size, size);
        for y in 0..size {
            for x in 0..size {
                b.pixels[(y * size + x) as usize] = if (x + y) % 2 == 0 {
                    argb(255, 0xffffff)
                } else {
                    argb(255, 0x000000)
                };
            }
        }
        b
    }

    #[test]
    fn shrinking_averages_instead_of_aliasing() {
        // A one-pixel checkerboard shrunk 8x is grey everywhere, not a moiré
        // of whichever pixels happened to be sampled.
        let image = PreparedImage::from_bitmap(&checker(64), 8, 8, Fit::Stretch, 0.0);
        for y in 0..8 {
            for x in 0..8 {
                let p = image.pixel(x, y);
                assert_eq!(p >> 24, 0xff);
                let grey = (p >> 16) & 0xff;
                assert!((126..=129).contains(&grey), "{grey:#x} at {x},{y}");
            }
        }
    }

    #[test]
    fn same_size_is_exact_and_opaque_rows_copy() {
        let source = checker(4);
        let image = PreparedImage::from_bitmap(&source, 4, 4, Fit::Stretch, 0.0);
        for (p, s) in image.pixels.iter().zip(&source.pixels) {
            assert_eq!(p, s);
        }
        let mut buffer = WindowBuffer::new(6, 6);
        Painter::new(&mut buffer).draw_image(&image, 1, 1);
        assert_eq!(buffer.pixels[6 + 1], 0xffffff);
        assert_eq!(buffer.pixels[6 + 2], 0x000000);
        assert_eq!(buffer.pixels[0], 0, "outside the image");
        assert!(
            buffer.pixels.iter().all(|p| p >> 24 == 0),
            "XRGB stays XRGB"
        );
    }

    #[test]
    fn cover_crops_and_contain_pads() {
        // 4x2: left half red, right half blue.
        let mut wide = Bitmap::new(4, 2);
        for y in 0..2 {
            for x in 0..4 {
                wide.pixels[y * 4 + x] = argb(255, if x < 2 { 0xff0000 } else { 0x0000ff });
            }
        }
        let cover = PreparedImage::from_bitmap(&wide, 2, 2, Fit::Cover, 0.0);
        // The middle two columns: one red, one blue.
        assert_eq!(cover.pixel(0, 0) & 0xffffff, 0xff0000);
        assert_eq!(cover.pixel(1, 0) & 0xffffff, 0x0000ff);
        let contain = PreparedImage::from_bitmap(&wide, 4, 4, Fit::Contain, 0.0);
        assert_eq!(contain.pixel(0, 0) >> 24, 0, "padding is transparent");
        assert_eq!(contain.pixel(0, 1) >> 24, 0xff);
        assert_eq!(
            contain.rows[0].start, contain.rows[0].end,
            "padding row is empty"
        );
        assert_eq!(
            (contain.rows[1].opaque_start, contain.rows[1].opaque_end),
            (0, 4)
        );
    }

    #[test]
    fn rounded_corners_are_transparent_and_blend_over_the_ground() {
        let image = PreparedImage::linear_gradient(40, 40, 12.0, 0xff0000, 0x0000ff, true);
        assert_eq!(image.pixel(0, 0) >> 24, 0, "corner cut");
        assert_eq!(image.pixel(20, 0) >> 24, 0xff, "edge kept");
        assert_eq!(image.pixel(20, 0) & 0xffffff, 0xff0000);
        assert_eq!(image.pixel(20, 39) & 0xffffff, 0x0000ff);
        let mut buffer = WindowBuffer::new(40, 40);
        let mut painter = Painter::new(&mut buffer);
        painter.clear(0x00ff00);
        painter.draw_image(&image, 0, 0);
        assert_eq!(
            buffer.pixels[0], 0x00ff00,
            "ground shows through the corner"
        );
        // A partly covered corner pixel mixes rather than overwrites.
        let edge = buffer.pixels[40 * 3 + 3];
        assert!(edge & 0x00ff00 != 0 && edge & 0xff0000 != 0, "{edge:#x}");
    }

    #[test]
    fn row_spans_find_the_visible_extent_and_the_longest_opaque_run() {
        let a = |alpha: u32| alpha << 24;
        let row = [
            a(0),
            a(0),
            a(40),
            a(255),
            a(255),
            a(9),
            a(255),
            a(255),
            a(255),
            a(1),
            a(0),
        ];
        assert_eq!(
            RowSpan::of(&row),
            RowSpan {
                start: 2,
                end: 10,
                opaque_start: 6,
                opaque_end: 9
            }
        );
        let empty = RowSpan::of(&[0, 0]);
        assert_eq!(empty.start, empty.end);
        let translucent = RowSpan::of(&[a(5), a(6)]);
        assert_eq!(translucent.opaque_start, translucent.opaque_end);
    }

    /// The span-wise draw against the per-pixel definition, for every
    /// clipping of a card-like image: transparent margin, translucent shadow,
    /// anti-aliased edge, opaque interior.
    #[test]
    fn drawing_by_spans_equals_drawing_pixel_by_pixel() {
        let card = card_like(40, 30);
        for xrgb in [true, false] {
            for (x, y) in [(0, 0), (-7, -3), (13, 5), (25, 20), (-39, 0), (60, 60)] {
                for opacity in [255u8, 128] {
                    let mut ground = WindowBuffer::new(48, 40);
                    for (i, p) in ground.pixels.iter_mut().enumerate() {
                        *p = (i as u32).wrapping_mul(0x9e37_79b9)
                            & if xrgb { 0x00ff_ffff } else { u32::MAX };
                    }
                    let mut expected = ground.clone();
                    for row in 0..card.height() as i32 {
                        for col in 0..card.width() as i32 {
                            let (px, py) = (x + col, y + row);
                            if px < 0 || py < 0 || px >= 48 || py >= 40 {
                                continue;
                            }
                            let s =
                                scale_premultiplied(card.pixel(col as u32, row as u32), opacity);
                            let d = &mut expected.pixels[(py * 48 + px) as usize];
                            *d = over_pixel(*d, s, xrgb);
                        }
                    }
                    let mut painter = if xrgb {
                        Painter::new(&mut ground)
                    } else {
                        Painter::new_premultiplied(&mut ground)
                    };
                    painter.draw_image_faded(&card, x, y, opacity);
                    assert_eq!(
                        ground.pixels, expected.pixels,
                        "at {x},{y} xrgb={xrgb} opacity={opacity}"
                    );
                }
            }
        }
    }

    fn card_like(width: u32, height: u32) -> PreparedImage {
        PreparedImage::from_fn(width, height, 8.0, |x, y| {
            let margin = x.min(y).min(width - 1 - x).min(height - 1 - y);
            match margin {
                0 => 0,
                1..=3 => (margin * 30) << 24,
                _ => 0xff00_0000 | (x * 6) << 16 | (y * 8) << 8 | 0x40,
            }
        })
    }

    #[test]
    fn faded_and_premultiplied_drawing() {
        let image = PreparedImage::from_fn(2, 1, 0.0, |_, _| argb(255, 0xffffff));
        let mut buffer = WindowBuffer::new(2, 1);
        Painter::new(&mut buffer).draw_image_faded(&image, 0, 0, 128);
        assert_eq!(buffer.pixels[0], 0x808080);
        let mut buffer = WindowBuffer::new(2, 1);
        Painter::new_premultiplied(&mut buffer).draw_image_faded(&image, 0, 0, 128);
        assert_eq!(buffer.pixels[0], 0x8080_8080);
    }

    #[test]
    fn gradients_fill_through_the_clip() {
        let mut buffer = WindowBuffer::new(4, 4);
        let mut painter = Painter::new(&mut buffer);
        painter.push_clip(Rect::new(1, 0, 2, 4));
        painter.fill_gradient(Rect::new(0, 0, 4, 4), 0x000000, 0xffffff, false);
        painter.fill_gradient(Rect::new(0, 0, 4, 4), 0x000000, 0x0000ff, true);
        painter.pop_clip();
        assert_eq!(buffer.pixels[0], 0, "clipped");
        assert_eq!(buffer.pixels[1], 0x000000);
        assert_eq!(buffer.pixels[3 * 4 + 1], 0x0000ff);
    }
}
