//! A software painter over a plain `u32` buffer: default XRGB or opt-in premultiplied ARGB.
//!
//! Deliberately small: rectangles, rules, text and a clip stack. A cashier
//! screen is a handful of panels and a lot of legible type, and every pixel of
//! it should be predictable at 3 a.m. when someone is trying to work out why
//! the change looked wrong.
//!
//! The clip stack is what makes a scrolling region possible at all. Every
//! write goes through it — fills, blends, single pixels — so there is no way
//! to draw outside the region you were given by forgetting to check, and a
//! nested clip intersects rather than replaces, so a child can never escape
//! its parent.

/// The window's pixels, row-major. [`Painter::new`] writes XRGB;
/// [`Painter::new_premultiplied`] writes premultiplied ARGB with coverage in the top byte.
/// The presenter must interpret the buffer with the same mode as its painter.
#[derive(Debug, Clone)]
pub struct WindowBuffer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl WindowBuffer {
    pub fn new(width: u32, height: u32) -> WindowBuffer {
        WindowBuffer {
            width,
            height,
            pixels: vec![0; width as usize * height as usize],
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.pixels.resize(width as usize * height as usize, 0);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn right(self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(self) -> i32 {
        self.y + self.h
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn inset(self, amount: i32) -> Rect {
        Rect::new(
            self.x + amount,
            self.y + amount,
            (self.w - amount * 2).max(0),
            (self.h - amount * 2).max(0),
        )
    }

    /// Split a horizontal strip off the top, returning it and the remainder.
    pub fn split_top(self, height: i32) -> (Rect, Rect) {
        let height = height.min(self.h);
        (
            Rect::new(self.x, self.y, self.w, height),
            Rect::new(self.x, self.y + height, self.w, self.h - height),
        )
    }

    /// Split a horizontal strip off the bottom.
    pub fn split_bottom(self, height: i32) -> (Rect, Rect) {
        let height = height.min(self.h);
        (
            Rect::new(self.x, self.bottom() - height, self.w, height),
            Rect::new(self.x, self.y, self.w, self.h - height),
        )
    }

    /// Split a vertical strip off the right.
    pub fn split_right(self, width: i32) -> (Rect, Rect) {
        let width = width.min(self.w);
        (
            Rect::new(self.right() - width, self.y, width, self.h),
            Rect::new(self.x, self.y, self.w - width, self.h),
        )
    }

    /// Split a vertical strip off the left.
    pub fn split_left(self, width: i32) -> (Rect, Rect) {
        let width = width.min(self.w);
        (
            Rect::new(self.x, self.y, width, self.h),
            Rect::new(self.x + width, self.y, self.w - width, self.h),
        )
    }

    /// The overlap of two rectangles, empty if they do not touch.
    pub fn intersect(self, other: Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Rect::new(x, y, (right - x).max(0), (bottom - y).max(0))
    }

    pub fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// Move by an offset — how a scrolled row finds its place on screen.
    pub fn translate(self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
}

pub struct Painter<'a> {
    buffer: &'a mut WindowBuffer,
    /// Opt-in premultiplied ARGB storage. RGB drawing APIs still take ordinary RGB colours.
    premultiplied: bool,
    /// The region drawing is confined to. Every write goes through it, so a
    /// scrolled row half outside its viewport is cut rather than drawn over
    /// the panel below.
    clip: Rect,
    /// Clips pushed and not yet popped. A stack rather than a single value
    /// because a table inside a panel inside a screen nests three deep, and
    /// each level has to restore exactly what it found.
    /// Ordinary widget trees nest only a few clips. Keep those frames inline
    /// so constructing a painter for every redraw does not allocate. The
    /// overflow vector preserves arbitrary nesting for custom controls and
    /// remains empty in the normal path.
    saved_inline: [Rect; INLINE_CLIP_DEPTH],
    saved_overflow: Vec<Rect>,
    saved_depth: usize,
}

const INLINE_CLIP_DEPTH: usize = 8;

impl<'a> Painter<'a> {
    pub fn new(buffer: &'a mut WindowBuffer) -> Painter<'a> {
        let clip = Rect::new(0, 0, buffer.width as i32, buffer.height as i32);
        Painter {
            buffer,
            premultiplied: false,
            clip,
            saved_inline: [Rect::default(); INLINE_CLIP_DEPTH],
            saved_overflow: Vec::new(),
            saved_depth: 0,
        }
    }

    /// Paint premultiplied `0xAARRGGBB` for an alpha-capable native presenter.
    /// RGB fills are opaque; use `erase_rect` for genuinely transparent ground.
    pub fn new_premultiplied(buffer: &'a mut WindowBuffer) -> Painter<'a> {
        Painter {
            premultiplied: true,
            ..Self::new(buffer)
        }
    }

    /// Replace pixels with transparent black, including any previously painted objects.
    pub fn erase_rect(&mut self, rect: Rect) {
        self.fill_raw(rect, 0);
    }

    /// Confine drawing to the intersection of `rect` and the current clip.
    ///
    /// Intersection, not replacement: a child cannot draw outside its parent
    /// by asking for a larger area, which is the property that makes nesting
    /// safe to reason about.
    pub fn push_clip(&mut self, rect: Rect) {
        if self.saved_depth < INLINE_CLIP_DEPTH {
            self.saved_inline[self.saved_depth] = self.clip;
        } else {
            self.saved_overflow.push(self.clip);
        }
        self.saved_depth += 1;
        self.clip = self.clip.intersect(rect);
    }

    pub fn pop_clip(&mut self) {
        if self.saved_depth == 0 {
            return;
        }
        self.saved_depth -= 1;
        self.clip = if self.saved_depth < INLINE_CLIP_DEPTH {
            self.saved_inline[self.saved_depth]
        } else {
            self.saved_overflow
                .pop()
                .expect("overflow clip has a matching push")
        };
    }

    /// The region currently being drawn into.
    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// Whether anything drawn in `rect` could be visible. Worth asking before
    /// laying out a row: a table of a thousand deliveries draws thirty.
    pub fn is_visible(&self, rect: Rect) -> bool {
        !self.clip.intersect(rect).is_empty()
    }

    pub fn width(&self) -> i32 {
        self.buffer.width as i32
    }

    pub fn height(&self) -> i32 {
        self.buffer.height as i32
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width(), self.height())
    }

    /// Fill the whole clip with one colour.
    ///
    /// Goes through the clip like everything else. It used to fill the raw
    /// buffer, which meant any widget that reached for the obvious way to
    /// paint its own background escaped the region it had been given — and
    /// the escape was silent, because clearing to the screen colour looks
    /// like nothing happened until two panels overlap.
    pub fn clear(&mut self, color: u32) {
        self.fill_rect(self.clip, color);
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, color: u32) {
        if self.clip.contains(x, y) {
            self.buffer.pixels[y as usize * self.buffer.width as usize + x as usize] =
                if self.premultiplied {
                    color | 0xff00_0000
                } else {
                    color
                };
        }
    }

    /// Blend a colour over what is there, by coverage.
    ///
    /// This is how anti-aliased text lands on a panel: the glyph's coverage
    /// byte becomes the alpha.
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, color: u32, alpha: u8) {
        if alpha == 0 {
            return;
        }
        if alpha == 255 {
            self.put(x, y, color);
            return;
        }
        if !self.clip.contains(x, y) {
            return;
        }
        let index = y as usize * self.buffer.width as usize + x as usize;
        self.buffer.pixels[index] = if self.premultiplied {
            crate::blend::blend_premultiplied(self.buffer.pixels[index], color, alpha)
        } else {
            blend_channels(self.buffer.pixels[index], color, alpha)
        };
    }

    /// The visible part of a `len`-pixel run starting at `(x, y)`: how far into
    /// the run visibility begins, and the buffer pixels the visible part
    /// covers. `None` when none of it is visible. For span-wise drawing in
    /// other modules (images, masks) without a per-pixel clip test.
    pub(crate) fn clipped_run(
        &mut self,
        x: i32,
        y: i32,
        len: usize,
    ) -> Option<(usize, &mut [u32])> {
        if y < self.clip.y || y >= self.clip.bottom() {
            return None;
        }
        let start = (self.clip.x - x).max(0);
        let end = (self.clip.right() - x).min(len as i32);
        if end <= start {
            return None;
        }
        let left = y as usize * self.buffer.width as usize + (x + start) as usize;
        let count = (end - start) as usize;
        Some((start as usize, &mut self.buffer.pixels[left..left + count]))
    }

    /// Whether this painter stores premultiplied alpha in the top byte.
    pub(crate) fn is_premultiplied(&self) -> bool {
        self.premultiplied
    }

    /// Blend one colour along a row, weighted per pixel by a coverage byte.
    ///
    /// This is how a glyph lands on a panel, and it is the hottest path in the
    /// client. It used to be `blend` called once per pixel, which meant a
    /// rectangle test, two bounds checks and a branch for every antialiased
    /// pixel of every letter of every label of every frame. Clipping a whole
    /// row at once turns all of that into two comparisons, and what is left is
    /// a straight-line span the vector kernels can take four or eight pixels
    /// at a time.
    ///
    /// `coverage` runs left to right from `x`. Anything outside the clip is
    /// cut from the ends rather than tested per pixel.
    pub fn blend_coverage_row(&mut self, x: i32, y: i32, color: u32, coverage: &[u8]) {
        if y < self.clip.y || y >= self.clip.bottom() {
            return;
        }
        let start = (self.clip.x - x).max(0);
        let end = (self.clip.right() - x).min(coverage.len() as i32);
        if end <= start {
            return;
        }
        let (start, end) = (start as usize, end as usize);
        // The clip is only ever intersected, never replaced, so it is still
        // inside the buffer and these indices are too.
        let left = y as usize * self.buffer.width as usize + (x + start as i32) as usize;
        let destination = &mut self.buffer.pixels[left..left + (end - start)];
        if self.premultiplied {
            for (pixel, &coverage) in destination.iter_mut().zip(&coverage[start..end]) {
                *pixel = crate::blend::blend_premultiplied(*pixel, color, coverage);
            }
        } else {
            crate::blend::blend_coverage_span(destination, color, &coverage[start..end]);
        }
    }

    pub fn fill_rect(&mut self, rect: Rect, color: u32) {
        let color = if self.premultiplied {
            color | 0xff00_0000
        } else {
            color
        };
        self.fill_raw(rect, color);
    }

    fn fill_raw(&mut self, rect: Rect, color: u32) {
        let visible = rect.intersect(self.clip);
        if visible.is_empty() {
            return;
        }
        let width = self.buffer.width as usize;
        // A rectangle that spans the buffer's full width — a screen cleared, a
        // panel across the top, a selected row in a table that fills the
        // window — is one contiguous run. Filling it in one call rather than
        // one per row hands the whole thing to a single wide store loop
        // instead of re-entering it six hundred times.
        if visible.x == 0 && visible.w as usize == width {
            let top = visible.y as usize * width;
            let bottom = visible.bottom() as usize * width;
            self.buffer.pixels[top..bottom].fill(color);
            return;
        }
        for y in visible.y..visible.bottom() {
            let row = y as usize * width;
            self.buffer.pixels[row + visible.x as usize..row + visible.right() as usize]
                .fill(color);
        }
    }

    /// Fill a rectangle with rounded corners using horizontal spans.
    ///
    /// A card needs at most `radius * 2` extra one-pixel fills, no heap
    /// allocation, and no off-screen texture. That keeps a softer visual
    /// language inexpensive on the same Raspberry Pi that owns the buffer.
    pub fn fill_rounded_rect(&mut self, rect: Rect, radius: i32, color: u32) {
        if rect.is_empty() {
            return;
        }
        let radius = radius.clamp(0, rect.w.min(rect.h) / 2);
        if radius == 0 {
            self.fill_rect(rect, color);
            return;
        }

        self.fill_rect(
            Rect::new(rect.x + radius, rect.y, rect.w - radius * 2, rect.h),
            color,
        );
        self.fill_rect(
            Rect::new(rect.x, rect.y + radius, rect.w, rect.h - radius * 2),
            color,
        );

        let radius = f64::from(radius);
        let radius_squared = radius * radius;
        for row in 0..radius as i32 {
            let distance = radius - f64::from(row) - 0.5;
            let half_span = (radius_squared - distance * distance).max(0.0).sqrt() as i32;
            let inset = radius as i32 - half_span;
            let width = (rect.w - inset * 2).max(0);
            self.fill_rect(Rect::new(rect.x + inset, rect.y + row, width, 1), color);
            self.fill_rect(
                Rect::new(rect.x + inset, rect.bottom() - row - 1, width, 1),
                color,
            );
        }
    }

    /// A filled rounded surface with its border drawn underneath.
    pub fn rounded_rect(&mut self, rect: Rect, radius: i32, border: i32, fill: u32, edge: u32) {
        let border = border.max(0);
        if border == 0 {
            self.fill_rounded_rect(rect, radius, fill);
            return;
        }
        self.fill_rounded_rect(rect, radius, edge);
        let inner = rect.inset(border);
        if !inner.is_empty() {
            self.fill_rounded_rect(inner, (radius - border).max(0), fill);
        }
    }

    pub fn stroke_rect(&mut self, rect: Rect, thickness: i32, color: u32) {
        if rect.w <= 0 || rect.h <= 0 {
            return;
        }
        let t = thickness.min(rect.w).min(rect.h);
        self.fill_rect(Rect::new(rect.x, rect.y, rect.w, t), color);
        self.fill_rect(Rect::new(rect.x, rect.bottom() - t, rect.w, t), color);
        self.fill_rect(Rect::new(rect.x, rect.y, t, rect.h), color);
        self.fill_rect(Rect::new(rect.right() - t, rect.y, t, rect.h), color);
    }

    pub fn horizontal_rule(&mut self, x: i32, y: i32, width: i32, color: u32) {
        self.fill_rect(Rect::new(x, y, width, 1), color);
    }
}

/// Integer blend in sRGB space, for one pixel at a time.
///
/// The arithmetic lives in `pos-simd` rather than here, because
/// [`Painter::blend_coverage_row`] does the same sum four or eight pixels at a
/// time and the two must agree exactly. A second copy of these weights in this
/// file is how a vector path and a scalar path start disagreeing by one step
/// on the pixels at the end of a row.
///
/// The weights are `alpha + 1` and `256 - (alpha + 1)`, summing to 256 to
/// match the `>> 8`. Weighting by `alpha` and `255 - alpha` sums to 255, which
/// loses a step: blending a colour over *itself* returns one less than it
/// started with, so an antialiased glyph drawn in the panel's own colour grows
/// a faint dark fringe. Every edge of every letter, on every panel.
#[inline]
fn blend_channels(destination: u32, source: u32, alpha: u8) -> u32 {
    crate::blend::blend_channels(destination, source, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplied_glyph_coverage_keeps_alpha_and_composes_without_a_fringe() {
        let coverage = [0, 1, 64, 128, 254, 255];
        let mut buffer = WindowBuffer::new(6, 2);
        {
            let mut painter = Painter::new_premultiplied(&mut buffer);
            painter.blend_coverage_row(0, 0, 0x00ff_ffff, &coverage);
            for (x, alpha) in coverage.into_iter().enumerate() {
                painter.blend(x as i32, 1, 0x00ff_ffff, alpha);
            }
        }
        for (x, alpha) in coverage.into_iter().enumerate() {
            let expected = u32::from(alpha) * 0x0101_0101;
            assert_eq!(buffer.pixels[x], expected);
            assert_eq!(
                buffer.pixels[6 + x],
                expected,
                "scalar and glyph spans agree"
            );
        }
        Painter::new_premultiplied(&mut buffer).blend(3, 0, 0xff0000, 128);
        assert_eq!(
            buffer.pixels[3], 0xc0c0_4040,
            "source-over accumulates premultiplied RGB and alpha"
        );
    }

    #[test]
    fn premultiplied_fills_are_opaque_and_erasure_replaces_objects_with_zero() {
        let mut buffer = WindowBuffer::new(4, 3);
        {
            let mut painter = Painter::new_premultiplied(&mut buffer);
            painter.clear(0x123456);
            painter.push_clip(Rect::new(1, 1, 2, 1));
            painter.erase_rect(Rect::new(-10, -10, 30, 30));
            painter.pop_clip();
            painter.put(0, 0, 0xabcdef);
        }
        assert_eq!(buffer.pixels[0], 0xffab_cdef);
        assert_eq!(buffer.pixels[5..7], [0, 0]);
        assert_eq!(buffer.pixels[4], 0xff12_3456);
        assert_eq!(buffer.pixels[7], 0xff12_3456);
        assert!(buffer.pixels[8..].iter().all(|pixel| *pixel == 0xff12_3456));
    }

    #[test]
    fn premultiplied_antialiased_paths_have_real_coverage_not_a_colour_key() {
        let mut buffer = WindowBuffer::new(12, 12);
        let mut kernel = crate::RasterKernel::new();
        Painter::new_premultiplied(&mut buffer).fill_circle_aa(&mut kernel, 6.0, 6.0, 4.3, 0, 255);
        assert!(
            buffer.pixels.iter().any(|pixel| *pixel == 0xff00_0000),
            "black objects are opaque, not treated as a transparency key"
        );
        assert!(buffer.pixels.iter().any(|pixel| {
            let alpha = pixel >> 24;
            alpha > 0 && alpha < 255
        }));
        assert_eq!(buffer.pixels[0], 0);
        assert!(buffer.pixels.iter().all(|pixel| pixel & 0x00ff_ffff == 0));
    }

    #[test]
    fn a_filled_rectangle_covers_exactly_its_bounds() {
        let mut buffer = WindowBuffer::new(8, 8);
        let mut painter = Painter::new(&mut buffer);
        painter.fill_rect(Rect::new(2, 3, 3, 2), 0x00ff_0000);
        for y in 0..8 {
            for x in 0..8 {
                let inside = (2..5).contains(&x) && (3..5).contains(&y);
                let expected = if inside { 0x00ff_0000 } else { 0 };
                assert_eq!(buffer.pixels[y * 8 + x], expected, "({x}, {y})");
            }
        }
    }

    #[test]
    fn a_rounded_surface_leaves_its_corners_and_keeps_its_border() {
        let mut buffer = WindowBuffer::new(12, 12);
        Painter::new(&mut buffer).rounded_rect(
            Rect::new(1, 1, 10, 10),
            4,
            1,
            0x0011_2233,
            0x00aa_bbcc,
        );
        let pixel = |x: usize, y: usize| buffer.pixels[y * 12 + x];
        assert_eq!(pixel(1, 1), 0, "the square corner stays transparent");
        assert_eq!(pixel(5, 1), 0x00aa_bbcc, "the top edge is the border");
        assert_eq!(pixel(5, 2), 0x0011_2233, "the inside is the fill");
        assert_eq!(pixel(6, 6), 0x0011_2233, "the centre is filled");
    }

    #[test]
    fn drawing_outside_the_buffer_is_clipped_rather_than_wrapped() {
        let mut buffer = WindowBuffer::new(4, 4);
        {
            // Entirely outside, in both directions: nothing at all.
            let mut painter = Painter::new(&mut buffer);
            painter.fill_rect(Rect::new(-10, -10, 5, 5), 0x00ff_ffff);
            painter.fill_rect(Rect::new(100, 100, 5, 5), 0x00ff_ffff);
            painter.put(-1, 0, 0x00ff_0000);
            painter.put(0, 99, 0x00ff_0000);
        }
        assert!(buffer.pixels.iter().all(|&pixel| pixel == 0));

        // Straddling the corner: the overlap is drawn and nothing wraps to the
        // far side.
        Painter::new(&mut buffer).fill_rect(Rect::new(-2, -2, 4, 4), 0x00ff_ffff);
        assert_eq!(buffer.pixels[0], 0x00ff_ffff);
        assert_eq!(buffer.pixels[1], 0x00ff_ffff);
        assert_eq!(buffer.pixels[2], 0, "clipped, not wrapped");
        assert_eq!(buffer.pixels[15], 0);
    }

    #[test]
    fn blending_a_colour_over_itself_changes_nothing() {
        // Antialiased text on a panel of its own colour. Weights summing to
        // 255 rather than 256 lose a step here, and every glyph edge grows a
        // dark fringe — the kind of artifact that gets blamed on the shaper.
        for colour in [0x0000_0000, 0x0012_3456, 0x00ff_ffff, 0x0080_8080] {
            for alpha in 1..=254u8 {
                assert_eq!(
                    blend_channels(colour, colour, alpha),
                    colour,
                    "{colour:#08x} over itself at alpha {alpha}"
                );
            }
        }
    }

    #[test]
    fn a_channel_never_bleeds_into_its_neighbour() {
        // The two-channels-at-once trick is only safe while each product
        // stays inside its byte lane.
        let blended = blend_channels(0x00ff_00ff, 0x0000_ff00, 128);
        assert!(blended <= 0x00ff_ffff, "{blended:#08x} left the lanes");
        assert_eq!(blend_channels(0x0000_0000, 0x00ff_ffff, 255), 0x00ff_ffff);
        assert_eq!(blend_channels(0x00ff_ffff, 0x0000_0000, 255), 0x0000_0000);
    }

    #[test]
    fn clearing_respects_the_clip() {
        // A widget painting its own background is the most ordinary thing a
        // widget does, and `clear` is the obvious way to reach for it.
        let mut buffer = WindowBuffer::new(6, 6);
        let mut painter = Painter::new(&mut buffer);
        painter.push_clip(Rect::new(1, 1, 2, 2));
        painter.clear(0x00ff_ffff);
        painter.pop_clip();

        let inked = buffer.pixels.iter().filter(|&&pixel| pixel != 0).count();
        assert_eq!(inked, 4, "clear escaped its region");

        // With no clip pushed it still means the whole window.
        Painter::new(&mut buffer).clear(0x0011_2233);
        assert!(buffer.pixels.iter().all(|&pixel| pixel == 0x0011_2233));
    }

    #[test]
    fn blending_at_half_coverage_lands_halfway() {
        let mut buffer = WindowBuffer::new(2, 1);
        let mut painter = Painter::new(&mut buffer);
        painter.fill_rect(Rect::new(0, 0, 2, 1), 0x0000_0000);
        painter.blend(0, 0, 0x00ff_ffff, 128);
        let blended = buffer.pixels[0] & 0xff;
        assert!((126..=130).contains(&blended), "got {blended}");
    }

    #[test]
    fn full_and_zero_coverage_short_circuit_correctly() {
        let mut buffer = WindowBuffer::new(2, 1);
        let mut painter = Painter::new(&mut buffer);
        painter.blend(0, 0, 0x00ff_ffff, 255);
        painter.blend(1, 0, 0x00ff_ffff, 0);
        assert_eq!(buffer.pixels[0], 0x00ff_ffff);
        assert_eq!(buffer.pixels[1], 0);
    }

    #[test]
    fn a_blended_row_lands_exactly_where_pixel_by_pixel_blending_would() {
        // The whole point of the row form is that it is the same picture. If
        // it ever is not, every glyph on every screen is subtly wrong and the
        // committed screenshots are the only thing that would notice.
        let coverage: Vec<u8> = (0..40u16).map(|column| (column * 7 % 256) as u8).collect();

        let mut by_row = WindowBuffer::new(48, 4);
        let mut by_pixel = WindowBuffer::new(48, 4);
        for start in [-10i32, -1, 0, 3, 40, 47] {
            let mut painter = Painter::new(&mut by_row);
            painter.blend_coverage_row(start, 1, 0x00ff_8811, &coverage);

            let mut painter = Painter::new(&mut by_pixel);
            for (offset, &cover) in coverage.iter().enumerate() {
                painter.blend(start + offset as i32, 1, 0x00ff_8811, cover);
            }
        }
        assert_eq!(by_row.pixels, by_pixel.pixels);
    }

    #[test]
    fn a_blended_row_is_cut_by_the_clip_rather_than_wrapped() {
        // A label that starts left of its column and runs past the right of
        // it. Both ends have to be cut, and nothing may appear on the row
        // above or below by running off the end of this one.
        let mut buffer = WindowBuffer::new(10, 3);
        let mut painter = Painter::new(&mut buffer);
        painter.push_clip(Rect::new(3, 1, 4, 1));
        painter.blend_coverage_row(-5, 1, 0x00ff_ffff, &[255; 30]);
        painter.blend_coverage_row(-5, 0, 0x00ff_ffff, &[255; 30]);
        painter.blend_coverage_row(-5, 2, 0x00ff_ffff, &[255; 30]);
        painter.pop_clip();

        for y in 0..3 {
            for x in 0..10 {
                let inside = y == 1 && (3..7).contains(&x);
                let expected = if inside { 0x00ff_ffff } else { 0 };
                assert_eq!(buffer.pixels[y * 10 + x], expected, "({x}, {y})");
            }
        }
    }

    #[test]
    fn a_blended_row_entirely_outside_the_buffer_draws_nothing() {
        let mut buffer = WindowBuffer::new(8, 2);
        let mut painter = Painter::new(&mut buffer);
        painter.blend_coverage_row(-100, 0, 0x00ff_ffff, &[255; 20]);
        painter.blend_coverage_row(100, 0, 0x00ff_ffff, &[255; 20]);
        painter.blend_coverage_row(0, -1, 0x00ff_ffff, &[255; 20]);
        painter.blend_coverage_row(0, 2, 0x00ff_ffff, &[255; 20]);
        painter.blend_coverage_row(0, 0, 0x00ff_ffff, &[]);
        assert!(buffer.pixels.iter().all(|&pixel| pixel == 0));
    }

    #[test]
    fn a_full_width_fill_covers_the_same_pixels_as_a_row_at_a_time_one() {
        // `fill_rect` takes a shortcut when a rectangle spans the buffer, and
        // a shortcut that got the last row wrong would show as a one-pixel
        // band nobody looks at directly.
        let mut whole = WindowBuffer::new(9, 6);
        Painter::new(&mut whole).fill_rect(Rect::new(0, 2, 9, 3), 0x0012_3456);

        let mut expected = WindowBuffer::new(9, 6);
        for y in 2..5 {
            for x in 0..9 {
                expected.pixels[y * 9 + x] = 0x0012_3456;
            }
        }
        assert_eq!(whole.pixels, expected.pixels);

        // And it must still be the clip that decides, not the rectangle asked
        // for: a full-width rectangle inside a narrower clip is not full width.
        let mut clipped = WindowBuffer::new(9, 6);
        let mut painter = Painter::new(&mut clipped);
        painter.push_clip(Rect::new(2, 0, 4, 6));
        painter.fill_rect(Rect::new(0, 0, 9, 6), 0x00ff_ffff);
        painter.pop_clip();
        let inked = clipped.pixels.iter().filter(|&&pixel| pixel != 0).count();
        assert_eq!(inked, 4 * 6, "the full-width shortcut escaped the clip");
    }

    #[test]
    fn splitting_a_rectangle_partitions_it_exactly() {
        let rect = Rect::new(10, 20, 100, 50);
        let (top, rest) = rect.split_top(15);
        assert_eq!(top, Rect::new(10, 20, 100, 15));
        assert_eq!(rest, Rect::new(10, 35, 100, 35));
        assert_eq!(top.h + rest.h, rect.h);

        let (right, left) = rect.split_right(30);
        assert_eq!(right, Rect::new(80, 20, 30, 50));
        assert_eq!(left, Rect::new(10, 20, 70, 50));
        assert_eq!(right.w + left.w, rect.w);
    }

    #[test]
    fn splitting_more_than_there_is_takes_everything() {
        let rect = Rect::new(0, 0, 10, 10);
        let (top, rest) = rect.split_top(999);
        assert_eq!(top.h, 10);
        assert_eq!(rest.h, 0);
    }

    #[test]
    fn a_stroked_rectangle_draws_its_border_and_not_its_middle() {
        let mut buffer = WindowBuffer::new(8, 8);
        let mut painter = Painter::new(&mut buffer);
        painter.stroke_rect(Rect::new(1, 1, 6, 6), 1, 0x00ff_ffff);
        let pixel = |x: usize, y: usize| buffer.pixels[y * 8 + x];
        assert_eq!(pixel(1, 1), 0x00ff_ffff, "top-left corner");
        assert_eq!(pixel(6, 1), 0x00ff_ffff, "top-right corner");
        assert_eq!(pixel(1, 6), 0x00ff_ffff, "bottom-left corner");
        assert_eq!(pixel(3, 3), 0, "the middle is untouched");
    }

    #[test]
    fn resizing_keeps_the_buffer_consistent_with_its_dimensions() {
        let mut buffer = WindowBuffer::new(4, 4);
        buffer.resize(10, 3);
        assert_eq!(buffer.pixels.len(), 30);
        assert_eq!((buffer.width, buffer.height), (10, 3));
    }

    #[test]
    fn a_clip_cuts_what_falls_outside_it() {
        // The scrolling case: a row that starts above its viewport must be
        // cut at the top edge, not drawn over the header.
        let mut buffer = WindowBuffer::new(10, 10);
        let mut painter = Painter::new(&mut buffer);
        painter.push_clip(Rect::new(2, 2, 6, 6));
        painter.fill_rect(Rect::new(0, 0, 10, 10), 0x00ff_ffff);
        painter.pop_clip();

        for y in 0..10 {
            for x in 0..10 {
                let inside = (2..8).contains(&x) && (2..8).contains(&y);
                let expected = if inside { 0x00ff_ffff } else { 0 };
                assert_eq!(buffer.pixels[y * 10 + x], expected, "({x}, {y})");
            }
        }
    }

    #[test]
    fn a_nested_clip_cannot_escape_its_parent() {
        // The property that makes nesting safe: a child asking for more than
        // its parent allows gets the intersection, not what it asked for.
        let mut buffer = WindowBuffer::new(10, 10);
        let mut painter = Painter::new(&mut buffer);
        painter.push_clip(Rect::new(2, 2, 4, 4));
        painter.push_clip(Rect::new(0, 0, 10, 10));
        assert_eq!(painter.clip(), Rect::new(2, 2, 4, 4));
        painter.fill_rect(Rect::new(0, 0, 10, 10), 0x00ff_ffff);
        painter.pop_clip();
        painter.pop_clip();

        assert_eq!(buffer.pixels[0], 0, "the parent's bound still held");
        assert_eq!(buffer.pixels[2 * 10 + 2], 0x00ff_ffff);
    }

    #[test]
    fn popping_restores_exactly_what_was_there() {
        let mut buffer = WindowBuffer::new(10, 10);
        let mut painter = Painter::new(&mut buffer);
        let original = painter.clip();
        painter.push_clip(Rect::new(1, 1, 2, 2));
        painter.push_clip(Rect::new(1, 1, 1, 1));
        painter.pop_clip();
        assert_eq!(painter.clip(), Rect::new(1, 1, 2, 2));
        painter.pop_clip();
        assert_eq!(painter.clip(), original);
    }

    #[test]
    fn ordinary_clip_nesting_stays_inline_and_deep_nesting_still_restores() {
        let mut buffer = WindowBuffer::new(64, 64);
        let mut painter = Painter::new(&mut buffer);
        let original = painter.clip();

        for inset in 0..INLINE_CLIP_DEPTH {
            let inset = inset as i32;
            painter.push_clip(Rect::new(inset, inset, 64 - inset * 2, 64 - inset * 2));
        }
        assert_eq!(painter.saved_overflow.capacity(), 0);

        for inset in INLINE_CLIP_DEPTH..INLINE_CLIP_DEPTH + 3 {
            let inset = inset as i32;
            painter.push_clip(Rect::new(inset, inset, 64 - inset * 2, 64 - inset * 2));
        }
        assert_eq!(painter.saved_overflow.len(), 3);

        for _ in 0..INLINE_CLIP_DEPTH + 3 {
            painter.pop_clip();
        }
        assert_eq!(painter.clip(), original);
        assert_eq!(painter.saved_depth, 0);
        assert!(painter.saved_overflow.is_empty());
    }

    #[test]
    fn popping_more_than_was_pushed_leaves_the_clip_alone() {
        // A drawing bug should not become a panic in the middle of service.
        let mut buffer = WindowBuffer::new(4, 4);
        let mut painter = Painter::new(&mut buffer);
        painter.pop_clip();
        painter.pop_clip();
        assert_eq!(painter.clip(), Rect::new(0, 0, 4, 4));
        painter.fill_rect(Rect::new(0, 0, 4, 4), 0x00ff_ffff);
        assert!(buffer.pixels.iter().all(|&pixel| pixel == 0x00ff_ffff));
    }

    #[test]
    fn a_clip_bounds_blending_as_well_as_filling() {
        // Text is drawn with `blend`, so a clip that only bound `fill_rect`
        // would cut the row's background and let its letters through.
        let mut buffer = WindowBuffer::new(6, 6);
        let mut painter = Painter::new(&mut buffer);
        painter.push_clip(Rect::new(2, 2, 2, 2));
        for y in 0..6 {
            for x in 0..6 {
                painter.blend(x, y, 0x00ff_ffff, 128);
                painter.blend(x, y, 0x00ff_ffff, 255);
            }
        }
        painter.pop_clip();

        let inked = buffer.pixels.iter().filter(|&&pixel| pixel != 0).count();
        assert_eq!(inked, 4, "only the clipped 2×2 should have ink");
    }

    #[test]
    fn visibility_answers_before_anything_is_drawn() {
        let mut buffer = WindowBuffer::new(10, 10);
        let mut painter = Painter::new(&mut buffer);
        painter.push_clip(Rect::new(0, 4, 10, 4));
        // A table asks this per row so it can skip laying out what is
        // scrolled off; it must not skip a row that is only half visible.
        assert!(!painter.is_visible(Rect::new(0, 0, 10, 4)), "wholly above");
        assert!(painter.is_visible(Rect::new(0, 2, 10, 4)), "straddling");
        assert!(painter.is_visible(Rect::new(0, 4, 10, 1)), "inside");
        assert!(!painter.is_visible(Rect::new(0, 8, 10, 4)), "wholly below");
    }

    #[test]
    fn intersecting_disjoint_rectangles_is_empty_rather_than_negative() {
        let a = Rect::new(0, 0, 5, 5);
        let b = Rect::new(50, 50, 5, 5);
        let overlap = a.intersect(b);
        assert!(overlap.is_empty());
        assert!(overlap.w >= 0 && overlap.h >= 0, "{overlap:?}");
    }

    #[test]
    fn every_split_partitions_exactly() {
        let rect = Rect::new(10, 20, 100, 50);

        let (bottom, rest) = rect.split_bottom(15);
        assert_eq!(bottom, Rect::new(10, 55, 100, 15));
        assert_eq!(rest, Rect::new(10, 20, 100, 35));
        assert_eq!(bottom.h + rest.h, rect.h);

        let (left, rest) = rect.split_left(30);
        assert_eq!(left, Rect::new(10, 20, 30, 50));
        assert_eq!(rest, Rect::new(40, 20, 70, 50));
        assert_eq!(left.w + rest.w, rect.w);
    }

    #[test]
    fn translating_moves_a_rectangle_without_resizing_it() {
        let rect = Rect::new(5, 5, 20, 10);
        let moved = rect.translate(-3, 7);
        assert_eq!(moved, Rect::new(2, 12, 20, 10));
        assert_eq!((moved.w, moved.h), (rect.w, rect.h));
    }
}
