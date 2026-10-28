#![allow(clippy::too_many_arguments)]
//! Anti-aliased shapes and alpha fills for [`Painter`], built on the coverage
//! kernel so a circle's edge is rendered exactly like a glyph's.

use crate::kernel::{FillRule, RasterKernel};
use crate::painter::{Painter, Rect};
use crate::path::Path;

impl Painter<'_> {
    /// Fill a rectangle with a colour at a constant opacity (0..=255).
    ///
    /// Row spans through the coverage kernel rather than a per-pixel blend:
    /// a panel-sized tint at 2× is a million pixels, and the per-pixel path
    /// with its clip test was the single most expensive thing in a frame.
    pub fn blend_rect(&mut self, rect: Rect, color: u32, alpha: u8) {
        if alpha == 255 {
            self.fill_rect(rect, color);
            return;
        }
        let visible = rect.intersect(self.clip());
        if visible.is_empty() || alpha == 0 {
            return;
        }
        thread_local! {
            static SPAN: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        SPAN.with(|span| {
            let mut span = span.borrow_mut();
            let width = visible.w as usize;
            if span.len() < width || span.first().copied() != Some(alpha) {
                span.clear();
                span.resize(width.max(64), alpha);
            }
            for y in visible.y..visible.bottom() {
                self.blend_coverage_row(visible.x, y, color, &span[..width]);
            }
        });
    }

    /// One-pixel horizontal hairline covering `[x, x + width)`.
    pub fn hline(&mut self, x: i32, y: i32, width: i32, color: u32) {
        self.fill_rect(Rect::new(x, y, width, 1), color);
    }

    /// One-pixel vertical hairline covering `[y, y + height)`.
    pub fn vline(&mut self, x: i32, y: i32, height: i32, color: u32) {
        self.fill_rect(Rect::new(x, y, 1, height), color);
    }

    /// Fill a path with `color` at `alpha` (0..=255 multiplies the coverage).
    ///
    /// The kernel needs a scratch; callers that draw many shapes per frame
    /// should keep one and reuse it.
    pub fn fill_path(&mut self, kernel: &mut RasterKernel, path: &mut Path, color: u32, alpha: u8) {
        self.fill_path_rule(kernel, path, FillRule::NonZero, color, alpha);
    }

    pub fn fill_path_rule(
        &mut self,
        kernel: &mut RasterKernel,
        path: &mut Path,
        rule: FillRule,
        color: u32,
        alpha: u8,
    ) {
        if alpha == 0 {
            return;
        }
        let clip = self.clip();
        // The kernel rasterizes into a `width × height` device area starting at
        // the origin; the clip's right/bottom bound that area, and rows above
        // or left of the clip are cut by `blend_coverage_row` itself.
        let width = clip.right().max(0) as usize;
        let height = clip.bottom().max(0) as usize;
        if width == 0 || height == 0 {
            return;
        }
        let mut scaled: Vec<u8> = Vec::new();
        path.fill_with(kernel, width, height, rule, |y, x0, x1, cov| {
            let y = y as i32;
            if y < clip.y {
                return;
            }
            let span = &cov[x0..=x1];
            if alpha == 255 {
                self.blend_coverage_row(x0 as i32, y, color, span);
            } else {
                scaled.clear();
                scaled.extend(
                    span.iter()
                        .map(|&c| ((u32::from(c) * u32::from(alpha) + 127) / 255) as u8),
                );
                self.blend_coverage_row(x0 as i32, y, color, &scaled);
            }
        });
    }

    /// Anti-aliased filled circle.
    pub fn fill_circle_aa(
        &mut self,
        kernel: &mut RasterKernel,
        cx: f32,
        cy: f32,
        r: f32,
        color: u32,
        alpha: u8,
    ) {
        let mut path = Path::new();
        path.circle(cx, cy, r);
        self.fill_path(kernel, &mut path, color, alpha);
    }

    /// Anti-aliased ring of outer radius `r` and stroke `width`.
    pub fn stroke_circle_aa(
        &mut self,
        kernel: &mut RasterKernel,
        cx: f32,
        cy: f32,
        r: f32,
        width: f32,
        color: u32,
        alpha: u8,
    ) {
        let mut path = Path::new();
        path.ring(cx, cy, r, width);
        self.fill_path(kernel, &mut path, color, alpha);
    }

    /// Anti-aliased rounded rectangle.
    pub fn fill_rounded_rect_aa(
        &mut self,
        kernel: &mut RasterKernel,
        rect: Rect,
        radius: f32,
        color: u32,
        alpha: u8,
    ) {
        let mut path = Path::new();
        path.rounded_rect(
            rect.x as f32,
            rect.y as f32,
            rect.w as f32,
            rect.h as f32,
            radius,
        );
        self.fill_path(kernel, &mut path, color, alpha);
    }

    /// Anti-aliased polyline stroke (butt caps, unjoined segments).
    pub fn stroke_polyline_aa(
        &mut self,
        kernel: &mut RasterKernel,
        points: &[[f32; 2]],
        width: f32,
        color: u32,
        alpha: u8,
    ) {
        let mut path = Path::new();
        path.stroke_polyline(points, width);
        self.fill_path(kernel, &mut path, color, alpha);
    }

    /// Anti-aliased line segment.
    pub fn line_aa(
        &mut self,
        kernel: &mut RasterKernel,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        width: f32,
        color: u32,
        alpha: u8,
    ) {
        self.stroke_polyline_aa(kernel, &[[x0, y0], [x1, y1]], width, color, alpha);
    }

    /// A cheap soft shadow: `spread` one-pixel rings outside `rect`, each
    /// fainter than the last, shifted down by `offset_y`. Only the rings are
    /// blended — the interior is about to be covered by the panel — so the
    /// cost is the perimeter, not the area.
    pub fn shadow_rect(&mut self, rect: Rect, offset_y: i32, spread: i32, color: u32, alpha: u8) {
        if spread <= 0 || alpha == 0 {
            return;
        }
        let total = (spread * (spread + 1) / 2).max(1) as u32;
        for i in 1..=spread {
            let a = (u32::from(alpha) * (spread - i + 1) as u32 / total).min(255) as u8;
            if a == 0 {
                continue;
            }
            let r = Rect::new(
                rect.x - i,
                rect.y - i + offset_y,
                rect.w + 2 * i,
                rect.h + 2 * i,
            );
            let inner = Rect::new(
                rect.x - i + 1,
                rect.y - i + offset_y + 1,
                rect.w + 2 * i - 2,
                rect.h + 2 * i - 2,
            );
            // Top and bottom rows of the ring, then the side columns between.
            self.blend_rect(Rect::new(r.x, r.y, r.w, 1), color, a);
            self.blend_rect(Rect::new(r.x, r.bottom() - 1, r.w, 1), color, a);
            self.blend_rect(Rect::new(r.x, inner.y, 1, inner.h), color, a);
            self.blend_rect(Rect::new(r.right() - 1, inner.y, 1, inner.h), color, a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::painter::WindowBuffer;

    #[test]
    fn a_filled_circle_respects_the_clip() {
        let mut buffer = WindowBuffer::new(40, 40);
        let mut painter = Painter::new(&mut buffer);
        let mut kernel = RasterKernel::new();
        painter.clear(0x000000);
        painter.push_clip(Rect::new(0, 0, 20, 40));
        painter.fill_circle_aa(&mut kernel, 20.0, 20.0, 10.0, 0xffffff, 255);
        painter.pop_clip();
        // Nothing right of x = 20 was touched.
        for y in 0..40 {
            for x in 20..40 {
                assert_eq!(
                    buffer.pixels[y * 40 + x],
                    0,
                    "({x},{y}) painted outside clip"
                );
            }
        }
        // The centre column just inside the clip is fully lit.
        assert_eq!(buffer.pixels[20 * 40 + 19], 0xffffff);
    }

    #[test]
    fn alpha_scales_coverage() {
        let mut buffer = WindowBuffer::new(10, 10);
        let mut painter = Painter::new(&mut buffer);
        let mut kernel = RasterKernel::new();
        painter.clear(0x000000);
        let mut p = Path::new();
        p.rect(0.0, 0.0, 10.0, 10.0);
        painter.fill_path(&mut kernel, &mut p, 0xffffff, 128);
        let v = buffer.pixels[55] & 0xff;
        assert!((v as i32 - 128).abs() <= 1, "{v}");
    }

    #[test]
    fn blend_rect_is_clipped_and_alpha_ends_are_exact() {
        let mut buffer = WindowBuffer::new(4, 4);
        let mut painter = Painter::new(&mut buffer);
        painter.clear(0x000000);
        painter.blend_rect(Rect::new(-2, -2, 4, 4), 0xffffff, 255);
        painter.blend_rect(Rect::new(2, 2, 10, 10), 0xffffff, 0);
        assert_eq!(buffer.pixels[0], 0xffffff);
        assert_eq!(buffer.pixels[15], 0);
    }
}
