//! Logical-to-physical scaling.
//!
//! Layout is written in logical pixels (the numbers in a design mockup) and
//! multiplied by the window's scale factor at draw time. Cursor positions
//! arrive physical and stay physical: hit-testing compares against the same
//! scaled rects that were drawn, so nothing is converted back.

/// The window's scale factor, with the conversions layout code needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale(pub f32);

impl Scale {
    pub const ONE: Scale = Scale(1.0);

    pub fn new(factor: f64) -> Scale {
        Scale(factor as f32)
    }

    /// Logical pixels to whole physical pixels, rounded to nearest.
    #[inline]
    pub fn px(self, logical: i32) -> i32 {
        (logical as f32 * self.0).round() as i32
    }

    /// Fractional logical pixels to whole physical pixels.
    #[inline]
    pub fn pxf(self, logical: f32) -> i32 {
        (logical * self.0).round() as i32
    }

    /// Logical to physical without rounding, for geometry the AA path draws.
    #[inline]
    pub fn f(self, logical: f32) -> f32 {
        logical * self.0
    }

    /// A font size in logical pixels to a physical em size.
    #[inline]
    pub fn font(self, logical: f32) -> f32 {
        logical * self.0
    }

    /// A rectangle in logical pixels to device pixels. Each edge is rounded
    /// on its own, so two logical rectangles that share an edge still share
    /// it after scaling, with no gap or overlap between them.
    #[inline]
    pub fn rect(self, logical: liminalkit_raster::Rect) -> liminalkit_raster::Rect {
        let x = self.px(logical.x);
        let y = self.px(logical.y);
        liminalkit_raster::Rect::new(
            x,
            y,
            self.px(logical.right()) - x,
            self.px(logical.bottom()) - y,
        )
    }

    /// Physical back to logical, for the rare thing that reports up.
    #[inline]
    pub fn logical(self, physical: i32) -> f32 {
        physical as f32 / self.0
    }

    /// A pixel is at least one device pixel, whatever the scale.
    #[inline]
    pub fn hairline(self) -> i32 {
        self.px(1).max(1)
    }
}

impl Default for Scale {
    fn default() -> Scale {
        Scale::ONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_rounds_to_nearest_and_never_loses_the_hairline() {
        let s = Scale(1.5);
        assert_eq!(s.px(62), 93);
        assert_eq!(s.px(1), 2);
        assert_eq!(Scale(0.75).hairline(), 1);
        assert_eq!(Scale(2.0).px(288), 576);
        assert_eq!(Scale::ONE.px(17), 17);
    }

    #[test]
    fn scaled_rectangles_that_shared_an_edge_still_share_it() {
        use liminalkit_raster::Rect;
        let s = Scale(1.5);
        let top = s.rect(Rect::new(0, 0, 100, 33));
        let bottom = s.rect(Rect::new(0, 33, 100, 33));
        assert_eq!(top.bottom(), bottom.y);
        assert_eq!(top, Rect::new(0, 0, 150, 50));
        assert_eq!(bottom, Rect::new(0, 50, 150, 49));
        assert_eq!(
            Scale::ONE.rect(Rect::new(3, 4, 5, 6)),
            Rect::new(3, 4, 5, 6)
        );
    }
}
