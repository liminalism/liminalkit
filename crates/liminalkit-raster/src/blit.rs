//! Drawing a [`Bitmap`] into a [`Painter`]: alpha-composited, optionally
//! resampled to a size other than its own.
//!
//! Icons are decoded once at whatever size the file shipped and drawn at
//! whatever size the layout wants, which is rarely the same size — a dock
//! showing 48px icons from a 256px theme, a thumbnail grid showing 64px
//! previews of a photo. [`ScaleFilter::Bilinear`] interpolates in
//! *premultiplied* space: blending straight alpha at a hard edge (an opaque
//! pixel next to a fully transparent one, whose colour bytes are typically
//! meaningless) pulls that meaningless colour into the result, which shows up
//! as a dark or bright fringe around every icon — the standard
//! un-premultiplied-resize artefact.

use crate::bitmap::{alpha_of, argb, rgb_of, Bitmap};
use crate::color::{blue, green, red};
use crate::painter::{Painter, Rect};

/// How [`Painter::blit`] maps source pixels onto a destination rectangle of a
/// different size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleFilter {
    /// Each destination pixel takes its nearest source pixel: sharp edges,
    /// no new colours introduced. Right for pixel art, and for the common
    /// case where the destination already matches the bitmap's own size.
    Nearest,
    /// Bilinear interpolation of the four nearest source pixels.
    Bilinear,
}

impl Painter<'_> {
    /// Draw `bitmap` into `dest`, alpha-composited over what is already
    /// there. `dest`'s size need not match the bitmap's — `filter` decides
    /// how the mismatch is resampled — and at a 1:1 scale both filters
    /// reproduce the source pixels exactly.
    pub fn blit(&mut self, bitmap: &Bitmap, dest: Rect, filter: ScaleFilter) {
        if bitmap.width == 0 || bitmap.height == 0 || dest.is_empty() {
            return;
        }
        let visible = dest.intersect(self.clip());
        if visible.is_empty() {
            return;
        }
        let scale_x = bitmap.width as f32 / dest.w as f32;
        let scale_y = bitmap.height as f32 / dest.h as f32;
        for y in visible.y..visible.bottom() {
            // The centre of destination row `y` maps to source row `v`; at a
            // 1:1 scale this lands exactly on an integer source row.
            let v = ((y - dest.y) as f32 + 0.5) * scale_y - 0.5;
            for x in visible.x..visible.right() {
                let u = ((x - dest.x) as f32 + 0.5) * scale_x - 0.5;
                let pixel = match filter {
                    ScaleFilter::Nearest => sample_nearest(bitmap, u, v),
                    ScaleFilter::Bilinear => sample_bilinear(bitmap, u, v),
                };
                self.blend(x, y, rgb_of(pixel), alpha_of(pixel));
            }
        }
    }
}

fn sample_nearest(bitmap: &Bitmap, u: f32, v: f32) -> u32 {
    let x = (u.round() as i32).clamp(0, bitmap.width as i32 - 1) as u32;
    let y = (v.round() as i32).clamp(0, bitmap.height as i32 - 1) as u32;
    bitmap.pixel(x, y)
}

fn sample_bilinear(bitmap: &Bitmap, u: f32, v: f32) -> u32 {
    let x0f = u.floor();
    let y0f = v.floor();
    let tx = u - x0f;
    let ty = v - y0f;
    let max_x = bitmap.width as i32 - 1;
    let max_y = bitmap.height as i32 - 1;
    let x0 = (x0f as i32).clamp(0, max_x) as u32;
    let x1 = (x0f as i32 + 1).clamp(0, max_x) as u32;
    let y0 = (y0f as i32).clamp(0, max_y) as u32;
    let y1 = (y0f as i32 + 1).clamp(0, max_y) as u32;

    let top = lerp4(
        premultiplied(bitmap.pixel(x0, y0)),
        premultiplied(bitmap.pixel(x1, y0)),
        tx,
    );
    let bottom = lerp4(
        premultiplied(bitmap.pixel(x0, y1)),
        premultiplied(bitmap.pixel(x1, y1)),
        tx,
    );
    let [a, r, g, b] = lerp4(top, bottom, ty);
    unpremultiply(a, r, g, b)
}

/// `[alpha, r, g, b]` as `0..=255` floats, colour scaled by alpha — the space
/// in which interpolation does not invent colour from a transparent pixel's
/// unused bytes.
fn premultiplied(pixel: u32) -> [f32; 4] {
    let a = f32::from(alpha_of(pixel));
    let rgb = rgb_of(pixel);
    let scale = a / 255.0;
    [
        a,
        f32::from(red(rgb)) * scale,
        f32::from(green(rgb)) * scale,
        f32::from(blue(rgb)) * scale,
    ]
}

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut out = [0.0; 4];
    for i in 0..4 {
        out[i] = a[i] + (b[i] - a[i]) * t;
    }
    out
}

fn unpremultiply(a: f32, r: f32, g: f32, b: f32) -> u32 {
    let alpha = a.round().clamp(0.0, 255.0) as u8;
    if alpha == 0 {
        // No colour survives at zero coverage; picking one would show up as
        // a border tint the moment this pixel is ever blended at non-zero
        // alpha by a future resample.
        return 0;
    }
    let scale = 255.0 / a;
    let channel = |c: f32| (c * scale).round().clamp(0.0, 255.0) as u32;
    argb(alpha, (channel(r) << 16) | (channel(g) << 8) | channel(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::painter::WindowBuffer;

    fn checker(size: u32) -> Bitmap {
        let mut bitmap = Bitmap::new(size, size);
        for y in 0..size {
            for x in 0..size {
                let on = (x + y) % 2 == 0;
                let colour = if on { 0x00ff_0000 } else { 0x0000_00ff };
                bitmap.pixels[(y * size + x) as usize] = argb(255, colour);
            }
        }
        bitmap
    }

    #[test]
    fn a_one_to_one_blit_reproduces_the_source_exactly() {
        let bitmap = checker(4);
        for filter in [ScaleFilter::Nearest, ScaleFilter::Bilinear] {
            let mut buffer = WindowBuffer::new(4, 4);
            let mut painter = Painter::new(&mut buffer);
            painter.blit(&bitmap, Rect::new(0, 0, 4, 4), filter);
            for y in 0..4 {
                for x in 0..4 {
                    assert_eq!(
                        buffer.pixels[y * 4 + x],
                        rgb_of(bitmap.pixel(x as u32, y as u32)),
                        "{filter:?} at ({x},{y})"
                    );
                }
            }
        }
    }

    #[test]
    fn nearest_scaling_keeps_hard_edges_on_pixel_art() {
        // A 2x2 checkerboard blown up 4x should stay four crisp blocks, not
        // blur into a gradient — the reason nearest exists at all.
        let bitmap = checker(2);
        let mut buffer = WindowBuffer::new(8, 8);
        let mut painter = Painter::new(&mut buffer);
        painter.blit(&bitmap, Rect::new(0, 0, 8, 8), ScaleFilter::Nearest);
        // Every pixel in the top-left quadrant matches the source's (0,0).
        let expected = rgb_of(bitmap.pixel(0, 0));
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(buffer.pixels[y * 8 + x], expected, "({x},{y})");
            }
        }
        assert_ne!(
            expected,
            rgb_of(bitmap.pixel(1, 0)),
            "the two colours differ"
        );
    }

    #[test]
    fn opaque_alpha_composites_like_an_ordinary_fill() {
        let mut bitmap = Bitmap::new(1, 1);
        bitmap.pixels[0] = argb(255, 0x00ab_cdef);
        let mut buffer = WindowBuffer::new(2, 2);
        let mut painter = Painter::new(&mut buffer);
        painter.clear(0x0011_2233);
        painter.blit(&bitmap, Rect::new(0, 0, 2, 2), ScaleFilter::Nearest);
        assert!(buffer.pixels.iter().all(|&p| p == 0x00ab_cdef));
    }

    #[test]
    fn half_alpha_blends_toward_the_background_by_half() {
        let mut bitmap = Bitmap::new(1, 1);
        bitmap.pixels[0] = argb(128, 0x00ff_ffff);
        let mut buffer = WindowBuffer::new(1, 1);
        let mut painter = Painter::new(&mut buffer);
        painter.clear(0x0000_0000);
        painter.blit(&bitmap, Rect::new(0, 0, 1, 1), ScaleFilter::Nearest);
        let channel = buffer.pixels[0] & 0xff;
        assert!((126..=130).contains(&channel), "got {channel}");
    }

    #[test]
    fn a_fully_transparent_source_leaves_the_background_untouched() {
        let bitmap = Bitmap::new(3, 3); // alpha 0 everywhere
        let mut buffer = WindowBuffer::new(3, 3);
        let mut painter = Painter::new(&mut buffer);
        painter.clear(0x0042_4242);
        painter.blit(&bitmap, Rect::new(0, 0, 3, 3), ScaleFilter::Bilinear);
        assert!(buffer.pixels.iter().all(|&p| p == 0x0042_4242));
    }

    #[test]
    fn bilinear_sampling_keeps_full_saturation_next_to_a_transparent_neighbour() {
        // A red opaque pixel beside a transparent pixel whose stored colour
        // is black — a realistic PNG, where fully transparent pixels are
        // often left at (0,0,0). Interpolating straight (non-premultiplied)
        // RGB would dim the red in proportion to closeness to the
        // transparent neighbour, and that dimmed value would then be
        // composited a *second* time by its own alpha — the classic dark
        // fringe. Premultiplied interpolation keeps the colour fully
        // saturated here and lets alpha alone carry the fade.
        let mut bitmap = Bitmap::new(2, 1);
        bitmap.pixels[0] = argb(255, 0x00ff_0000); // opaque red
        bitmap.pixels[1] = argb(0, 0x0000_0000); // transparent, black

        for u in [0.1f32, 0.3, 0.5, 0.7, 0.9] {
            let sample = sample_bilinear(&bitmap, u, 0.0);
            assert_eq!(rgb_of(sample), 0x00ff_0000, "u={u}: {sample:#010x}");
            let alpha = alpha_of(sample);
            assert!(
                alpha > 0 && alpha < 255,
                "u={u}: alpha {alpha} is not a partial fade"
            );
        }
    }

    #[test]
    fn blitting_off_the_edge_of_the_buffer_is_clipped_not_wrapped() {
        let bitmap = checker(4);
        let mut buffer = WindowBuffer::new(4, 4);
        let mut painter = Painter::new(&mut buffer);
        painter.blit(&bitmap, Rect::new(-2, -2, 4, 4), ScaleFilter::Nearest);
        // Only the bottom-right 2x2 of the buffer was touched.
        for y in 0..4 {
            for x in 0..4 {
                let touched = x < 2 && y < 2;
                let painted = buffer.pixels[y * 4 + x] != 0;
                assert_eq!(painted, touched, "({x},{y})");
            }
        }
    }

    #[test]
    fn a_zero_sized_bitmap_or_destination_does_not_panic() {
        let empty = Bitmap::new(0, 0);
        let mut buffer = WindowBuffer::new(4, 4);
        let mut painter = Painter::new(&mut buffer);
        painter.blit(&empty, Rect::new(0, 0, 4, 4), ScaleFilter::Nearest);
        let real = checker(2);
        painter.blit(&real, Rect::new(0, 0, 0, 0), ScaleFilter::Bilinear);
        assert!(buffer.pixels.iter().all(|&p| p == 0));
    }
}
