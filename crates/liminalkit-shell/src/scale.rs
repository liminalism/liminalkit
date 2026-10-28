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
}
