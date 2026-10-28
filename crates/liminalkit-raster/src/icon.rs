//! Vector icons and other shapes, rasterized once into coverage masks and
//! tinted when drawn.
//!
//! An [`Icon`] is path data in a square view box — the form every filled
//! icon set publishes. [`Icon::rasterize`] turns it into a [`Mask`] of
//! coverage bytes at one pixel size, through the same exact-area kernel that
//! renders text, so icons and type share an edge. [`Mask::rounded_rect`]
//! does the same for pills, buttons and chips, which would otherwise be
//! rasterized from a path on every frame they appear in. Drawing a mask is a
//! coverage blend per row ([`Painter::draw_mask`]) in whatever colour the
//! moment needs: one mask serves the pressed and unpressed button, light and
//! dark.
//!
//! The mask is the caller's to keep, keyed however it likes (usually by shape
//! and pixel size, rebuilt when the scale changes) — the toolkit keeps no
//! hidden caches, in keeping with its immediate-mode, caller-owns-state rule.

use crate::kernel::{FillRule, RasterKernel};
use crate::painter::Painter;
use crate::path::Path;
use crate::svg::{SvgPathError, SvgTransform};

/// SVG path data in a `view_box × view_box` square.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Icon {
    pub view_box: f32,
    /// One or more path data strings, filled together as one path.
    pub paths: &'static [&'static str],
    pub rule: FillRule,
}

impl Icon {
    pub const fn new(view_box: f32, paths: &'static [&'static str]) -> Icon {
        Icon {
            view_box,
            paths,
            rule: FillRule::NonZero,
        }
    }

    pub const fn even_odd(self) -> Icon {
        Icon {
            rule: FillRule::EvenOdd,
            ..self
        }
    }

    /// Rasterize at `size × size` pixels.
    pub fn rasterize(&self, kernel: &mut RasterKernel, size: u32) -> Result<Mask, SvgPathError> {
        let mut path = Path::new();
        let transform = SvgTransform::fit(self.view_box, size as f32, 0.0, 0.0);
        for d in self.paths {
            path.append_svg(d, transform)?;
        }
        Ok(Mask::fill(kernel, &mut path, size, size, self.rule, 255))
    }
}

/// Coverage only, `width × height`, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mask {
    width: u32,
    height: u32,
    coverage: Vec<u8>,
}

impl Mask {
    /// Coverage of `path` over a `width × height` area at the origin,
    /// multiplied by `opacity` (255 for none) — so a translucent shape is
    /// drawn with the same span blend as an opaque one.
    pub fn fill(
        kernel: &mut RasterKernel,
        path: &mut Path,
        width: u32,
        height: u32,
        rule: FillRule,
        opacity: u8,
    ) -> Mask {
        let w = width as usize;
        let mut coverage = vec![0u8; w * height as usize];
        path.fill_with(kernel, w, height as usize, rule, |y, x0, x1, row| {
            let out = &mut coverage[y * w + x0..=y * w + x1];
            if opacity == 255 {
                out.copy_from_slice(&row[x0..=x1]);
            } else {
                for (o, &c) in out.iter_mut().zip(&row[x0..=x1]) {
                    *o = crate::blend::div255(u32::from(c) * u32::from(opacity) + 127) as u8;
                }
            }
        });
        Mask {
            width,
            height,
            coverage,
        }
    }

    /// An anti-aliased rounded rectangle filling `width × height`.
    pub fn rounded_rect(
        kernel: &mut RasterKernel,
        width: u32,
        height: u32,
        radius: f32,
        opacity: u8,
    ) -> Mask {
        let mut path = Path::new();
        path.rounded_rect(0.0, 0.0, width as f32, height as f32, radius);
        Mask::fill(kernel, &mut path, width, height, FillRule::NonZero, opacity)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The side of a square mask (an icon's size).
    pub fn size(&self) -> u32 {
        self.width
    }

    pub fn coverage(&self) -> &[u8] {
        &self.coverage
    }

    pub fn byte_size(&self) -> usize {
        self.coverage.len()
    }
}

impl Painter<'_> {
    /// Draw a mask in `color` with its top-left corner at `(x, y)`.
    pub fn draw_mask(&mut self, mask: &Mask, x: i32, y: i32, color: u32) {
        let w = mask.width as usize;
        if w == 0 {
            return;
        }
        for (row, span) in mask.coverage.chunks_exact(w).enumerate() {
            self.blend_coverage_row(x, y + row as i32, color, span);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::painter::{Rect, WindowBuffer};

    const SQUARE: Icon = Icon::new(24.0, &["M4 4h16v16H4z"]);
    /// A square with a square hole, as two subpaths.
    const FRAME: Icon = Icon::new(24.0, &["M4 4h16v16H4z", "M8 8h8v8H8z"]).even_odd();

    #[test]
    fn a_mask_scales_with_its_size_and_tints_when_drawn() {
        let mut kernel = RasterKernel::new();
        let small = SQUARE.rasterize(&mut kernel, 24).unwrap();
        let large = SQUARE.rasterize(&mut kernel, 48).unwrap();
        let sum = |m: &Mask| m.coverage().iter().map(|&c| u32::from(c)).sum::<u32>() / 255;
        assert_eq!(sum(&small), 256);
        assert_eq!(sum(&large), 1024);

        let mut buffer = WindowBuffer::new(30, 30);
        Painter::new(&mut buffer).draw_mask(&small, 3, 3, 0xff0000);
        assert_eq!(buffer.pixels[(3 + 10) * 30 + 3 + 10], 0xff0000);
        assert_eq!(buffer.pixels[(3 + 1) * 30 + 3 + 1], 0, "outside the glyph");
    }

    #[test]
    fn even_odd_cuts_holes_and_bad_data_is_an_error() {
        let mut kernel = RasterKernel::new();
        let frame = FRAME.rasterize(&mut kernel, 24).unwrap();
        assert_eq!(frame.coverage()[12 * 24 + 12], 0, "the hole");
        assert_eq!(frame.coverage()[5 * 24 + 5], 255, "the frame");
        let broken = Icon::new(24.0, &["L1 1"]);
        assert!(broken.rasterize(&mut kernel, 24).is_err());
    }

    #[test]
    fn a_mask_drawn_off_the_edge_is_clipped() {
        let mut kernel = RasterKernel::new();
        let mask = SQUARE.rasterize(&mut kernel, 24).unwrap();
        let mut buffer = WindowBuffer::new(10, 10);
        Painter::new(&mut buffer).draw_mask(&mask, -8, -8, 0x00ff00);
        assert_eq!(buffer.pixels[0], 0x00ff00);
        Painter::new(&mut buffer).draw_mask(&mask, 50, 50, 0x0000ff);
    }

    /// A cached pill must look exactly like the one rasterized per frame:
    /// `fill_rounded_rect_aa` blends coverage scaled by alpha through the same
    /// span kernel.
    #[test]
    fn a_cached_rounded_rect_draws_what_the_per_frame_one_did() {
        let mut kernel = RasterKernel::new();
        for (alpha, color) in [(255u8, 0xb0174f), (46, 0xffffff), (70, 0x000000)] {
            let rect = Rect::new(5, 3, 70, 24);
            let ground = |buffer: &mut WindowBuffer| {
                for (i, p) in buffer.pixels.iter_mut().enumerate() {
                    *p = (i as u32).wrapping_mul(2_654_435_761) & 0x00ff_ffff;
                }
            };
            let mut direct = WindowBuffer::new(80, 30);
            ground(&mut direct);
            Painter::new(&mut direct).fill_rounded_rect_aa(&mut kernel, rect, 12.0, color, alpha);
            let mut cached = WindowBuffer::new(80, 30);
            ground(&mut cached);
            let mask = Mask::rounded_rect(&mut kernel, 70, 24, 12.0, alpha);
            Painter::new(&mut cached).draw_mask(&mask, 5, 3, color);
            assert_eq!(direct.pixels, cached.pixels, "alpha {alpha}");
        }
    }
}
