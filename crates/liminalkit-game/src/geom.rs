//! Rectangles and colours in floating point, for things that move.
//!
//! The rest of liminalkit lays out in whole device pixels. Game UI does not:
//! a card sliding in, a popover following a projected point and a colour
//! fading out all sit between pixels for most of their lives, and rounding
//! them every frame would make them crawl. These types keep the fractions
//! and leave rounding to whoever draws.
//!
//! EXPERIMENTAL: the API is new.

/// An axis-aligned rectangle: origin at the top left, `x` to the right and
/// `y` down, in whatever unit the caller works in (usually pixels).
///
/// A rectangle with no area, or with a coordinate that is not finite, is
/// [empty](Self::is_empty); operations treat it as covering nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl RectF {
    /// A rectangle from its top-left corner and its size.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> RectF {
        RectF { x, y, w, h }
    }

    /// The right edge, `x + w`.
    pub fn right(self) -> f32 {
        self.x + self.w
    }

    /// The bottom edge, `y + h`.
    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    /// The middle point, `[x, y]`.
    pub fn centre(self) -> [f32; 2] {
        [self.x + self.w * 0.5, self.y + self.h * 0.5]
    }

    /// Whether `p` is inside. Half-open: the left and top edges are inside,
    /// the right and bottom edges are not, so two rectangles that share an
    /// edge never both claim a point on it.
    pub fn contains(self, p: [f32; 2]) -> bool {
        !self.is_empty()
            && p[0] >= self.x
            && p[0] < self.right()
            && p[1] >= self.y
            && p[1] < self.bottom()
    }

    /// Whether it covers nothing: no width, no height, or a coordinate that
    /// is not finite.
    pub fn is_empty(self) -> bool {
        // Written as a negation so that NaN, which fails every comparison,
        // counts as empty.
        !(self.w > 0.0
            && self.h > 0.0
            && self.x.is_finite()
            && self.y.is_finite()
            && self.w.is_finite()
            && self.h.is_finite())
    }

    /// The part covered by both, or the zero rectangle
    /// ([`RectF::default()`]) when they do not overlap.
    pub fn intersect(self, o: RectF) -> RectF {
        if self.is_empty() || o.is_empty() {
            return RectF::default();
        }
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        if x1 > x0 && y1 > y0 {
            RectF::new(x0, y0, x1 - x0, y1 - y0)
        } else {
            RectF::default()
        }
    }

    /// The smallest rectangle covering both. An empty operand covers
    /// nothing, so it is ignored rather than stretching the result towards
    /// its origin.
    pub fn union(self, o: RectF) -> RectF {
        if o.is_empty() {
            return self;
        }
        if self.is_empty() {
            return o;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = self.right().max(o.right());
        let y1 = self.bottom().max(o.bottom());
        RectF::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Grown by `by` on every side; a negative `by` shrinks it. Shrinking
    /// past nothing leaves a zero-sized rectangle at the centre rather than
    /// one with a negative size.
    pub fn inflate(self, by: f32) -> RectF {
        let [cx, cy] = self.centre();
        let w = self.w + 2.0 * by;
        let h = self.h + 2.0 * by;
        let (x, w) = if w < 0.0 { (cx, 0.0) } else { (self.x - by, w) };
        let (y, h) = if h < 0.0 { (cy, 0.0) } else { (self.y - by, h) };
        RectF::new(x, y, w, h)
    }

    /// Moved by `dx` across and `dy` down.
    pub fn translate(self, dx: f32, dy: f32) -> RectF {
        RectF::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    /// The area both cover; zero when they do not overlap.
    pub fn overlap_area(self, o: RectF) -> f32 {
        let both = self.intersect(o);
        if both.is_empty() {
            0.0
        } else {
            both.w * both.h
        }
    }
}

/// Straight (not premultiplied) 0xAARRGGBB, sRGB-encoded.
///
/// Straight because that is what people write down and what a fade changes:
/// fading a colour out lowers the alpha byte and leaves the colour alone.
/// Use [`premultiplied`](Self::premultiplied) when handing it to a blender.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Argb(pub u32);

impl Argb {
    /// Nothing at all: every channel zero.
    pub const TRANSPARENT: Argb = Argb(0x0000_0000);
    /// Opaque white.
    pub const WHITE: Argb = Argb(0xFFFF_FFFF);
    /// Opaque black.
    pub const BLACK: Argb = Argb(0xFF00_0000);

    /// An opaque colour from `0xRRGGBB`. Anything above the low 24 bits is
    /// ignored.
    pub const fn rgb(rgb: u32) -> Argb {
        Argb(0xFF00_0000 | (rgb & 0x00FF_FFFF))
    }

    /// The same colour with its alpha replaced.
    pub const fn with_alpha(self, alpha: u8) -> Argb {
        Argb((self.0 & 0x00FF_FFFF) | ((alpha as u32) << 24))
    }

    /// The alpha byte: 0 is transparent, 255 opaque.
    pub fn alpha(self) -> u8 {
        (self.0 >> 24) as u8
    }

    /// The four encoded bytes, `[a, r, g, b]`.
    pub fn channels(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    /// A colour from four encoded bytes, `[a, r, g, b]`.
    pub fn from_channels(argb: [u8; 4]) -> Argb {
        Argb(u32::from_be_bytes(argb))
    }

    /// Straight `[r, g, b, a]`, each 0 to 1. The colour channels stay
    /// sRGB-encoded: this is a change of scale, not of colour space.
    pub fn to_f32(self) -> [f32; 4] {
        let [a, r, g, b] = self.channels();
        [r, g, b, a].map(|c| f32::from(c) / 255.0)
    }

    /// `[r * a, g * a, b * a, a]`, each 0 to 1, for a blender that wants
    /// premultiplied colour.
    pub fn premultiplied(self) -> [f32; 4] {
        let [r, g, b, a] = self.to_f32();
        [r * a, g * a, b * a, a]
    }

    /// From straight `[r, g, b, a]`, each 0 to 1. Each channel is clamped
    /// into range (NaN counts as 0) and rounded to the nearest byte, so a
    /// round trip through [`to_f32`](Self::to_f32) is exact.
    pub fn from_f32(straight: [f32; 4]) -> Argb {
        let [r, g, b, a] = straight.map(unit_to_byte);
        Argb::from_channels([a, r, g, b])
    }
}

/// 0..1 to the nearest byte, clamped; NaN is 0.
fn unit_to_byte(c: f32) -> u8 {
    if c.is_nan() {
        return 0;
    }
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_and_centre_come_from_origin_and_size() {
        let r = RectF::new(10.0, 20.0, 30.0, 40.0);
        assert_eq!((r.right(), r.bottom()), (40.0, 60.0));
        assert_eq!(r.centre(), [25.0, 40.0]);
    }

    #[test]
    fn containment_is_half_open() {
        let r = RectF::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains([0.0, 0.0]));
        assert!(r.contains([9.99, 9.99]));
        assert!(!r.contains([10.0, 5.0]));
        assert!(!r.contains([5.0, 10.0]));
        assert!(!r.contains([-0.01, 5.0]));
        assert!(!RectF::new(0.0, 0.0, 0.0, 10.0).contains([0.0, 0.0]));
    }

    #[test]
    fn a_rectangle_with_no_area_or_a_nan_is_empty() {
        assert!(RectF::default().is_empty());
        assert!(RectF::new(0.0, 0.0, -1.0, 5.0).is_empty());
        assert!(RectF::new(0.0, 0.0, 5.0, 0.0).is_empty());
        assert!(RectF::new(f32::NAN, 0.0, 5.0, 5.0).is_empty());
        assert!(RectF::new(0.0, 0.0, f32::INFINITY, 5.0).is_empty());
        assert!(!RectF::new(-5.0, -5.0, 1.0, 1.0).is_empty());
    }

    #[test]
    fn intersection_is_the_shared_part_or_nothing() {
        let a = RectF::new(0.0, 0.0, 10.0, 10.0);
        let b = RectF::new(5.0, 6.0, 10.0, 10.0);
        assert_eq!(a.intersect(b), RectF::new(5.0, 6.0, 5.0, 4.0));
        assert_eq!(b.intersect(a), RectF::new(5.0, 6.0, 5.0, 4.0));
        let far = RectF::new(20.0, 20.0, 1.0, 1.0);
        assert_eq!(a.intersect(far), RectF::default());
        // Touching along an edge shares no area.
        let touching = RectF::new(10.0, 0.0, 5.0, 5.0);
        assert!(a.intersect(touching).is_empty());
    }

    #[test]
    fn union_covers_both_and_ignores_an_empty_operand() {
        let a = RectF::new(0.0, 0.0, 10.0, 10.0);
        let b = RectF::new(20.0, -5.0, 5.0, 5.0);
        assert_eq!(a.union(b), RectF::new(0.0, -5.0, 25.0, 15.0));
        let nothing = RectF::new(100.0, 100.0, 0.0, 0.0);
        assert_eq!(a.union(nothing), a);
        assert_eq!(nothing.union(a), a);
    }

    #[test]
    fn inflating_grows_every_side_and_shrinking_stops_at_nothing() {
        let r = RectF::new(10.0, 10.0, 20.0, 10.0);
        assert_eq!(r.inflate(2.0), RectF::new(8.0, 8.0, 24.0, 14.0));
        assert_eq!(r.inflate(-2.0), RectF::new(12.0, 12.0, 16.0, 6.0));
        let gone = r.inflate(-6.0);
        assert_eq!(gone, RectF::new(16.0, 15.0, 8.0, 0.0));
        assert!(gone.is_empty());
    }

    #[test]
    fn translation_moves_without_resizing() {
        let r = RectF::new(1.0, 2.0, 3.0, 4.0).translate(10.0, -2.0);
        assert_eq!(r, RectF::new(11.0, 0.0, 3.0, 4.0));
    }

    #[test]
    fn overlap_area_is_the_shared_area() {
        let a = RectF::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(a.overlap_area(RectF::new(5.0, 5.0, 10.0, 10.0)), 25.0);
        assert_eq!(a.overlap_area(RectF::new(50.0, 5.0, 10.0, 10.0)), 0.0);
        assert_eq!(a.overlap_area(a), 100.0);
    }

    #[test]
    fn colours_are_built_from_rgb_and_alpha() {
        assert_eq!(Argb::rgb(0x12_3456), Argb(0xFF12_3456));
        assert_eq!(Argb::rgb(0xAB12_3456), Argb(0xFF12_3456));
        assert_eq!(Argb::rgb(0x12_3456).with_alpha(0x80), Argb(0x8012_3456));
        assert_eq!(Argb(0x8012_3456).alpha(), 0x80);
        assert_eq!(Argb::WHITE.with_alpha(0), Argb(0x00FF_FFFF));
        assert_eq!(Argb(0x8012_3456).channels(), [0x80, 0x12, 0x34, 0x56]);
        assert_eq!(
            Argb::from_channels([0x80, 0x12, 0x34, 0x56]),
            Argb(0x8012_3456)
        );
    }

    #[test]
    fn colours_convert_to_unit_floats_straight_and_premultiplied() {
        assert_eq!(Argb::WHITE.to_f32(), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(Argb::BLACK.to_f32(), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(Argb::TRANSPARENT.to_f32(), [0.0; 4]);
        let half_red = Argb(0x80FF_0000);
        let [r, g, b, a] = half_red.to_f32();
        assert_eq!((r, g, b), (1.0, 0.0, 0.0));
        assert!((a - 128.0 / 255.0).abs() < 1e-6);
        let [pr, pg, pb, pa] = half_red.premultiplied();
        assert!((pr - a).abs() < 1e-6);
        assert_eq!((pg, pb, pa), (0.0, 0.0, a));
    }

    #[test]
    fn unit_floats_clamp_and_round_to_the_nearest_byte() {
        for value in [0u32, 0xFFFF_FFFF, 0x8012_3456, 0x01FE_7F80] {
            assert_eq!(Argb::from_f32(Argb(value).to_f32()), Argb(value));
        }
        assert_eq!(
            Argb::from_f32([2.0, -1.0, f32::NAN, 1.0]),
            Argb(0xFFFF_0000)
        );
        // 0.5 * 255 = 127.5 rounds away from zero.
        assert_eq!(Argb::from_f32([0.5, 0.0, 0.0, 1.0]), Argb(0xFF80_0000));
        assert_eq!(
            Argb::from_f32([0.0, 0.0, 0.0, 0.499 / 255.0]),
            Argb(0x0000_0000)
        );
    }
}
