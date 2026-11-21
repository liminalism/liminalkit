//! Placing a popover beside a point: a label over a unit, a tooltip on a
//! map marker, a callout at a projected 3D position.
//!
//! [`place`] is a function: give it the point, the popover's size, the
//! screen and anything it must keep clear of, and it says where the popover
//! goes, which side of the point it took, where a leader line should meet it,
//! and whether it fits. The one thing that must persist between frames to
//! keep a popover from flickering between sides is the side it took last
//! time, and the caller holds that and passes it back. A point that moves
//! by a pixel or so every frame (a projection of a swaying object) can be
//! steadied with [`deadband`].
//!
//! EXPERIMENTAL: the API is new, and the scoring has not been tuned on a
//! screen.

use crate::geom::RectF;

/// Which side of the point the popover sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Above the point; its bottom edge faces the point.
    Above,
    /// Below the point; its top edge faces the point.
    Below,
    /// Left of the point; its right edge faces the point.
    Left,
    /// Right of the point; its left edge faces the point.
    Right,
}

impl Side {
    /// Whether the popover is offset vertically (above or below), so that it
    /// slides horizontally to stay on screen.
    fn is_vertical(self) -> bool {
        matches!(self, Side::Above | Side::Below)
    }
}

/// What to place, where.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchoring<'r> {
    /// The point the popover belongs to, in screen space.
    pub point: [f32; 2],
    /// The popover's `[width, height]`.
    pub size: [f32; 2],
    /// The space between the point and the popover's facing edge.
    pub gap: f32,
    /// The space kept between the popover and the edges of `bounds`.
    pub margin: f32,
    /// Where the popover may go: the screen, or the part of it given to
    /// popovers.
    pub bounds: RectF,
    /// Places to keep clear of: the HUD, a minimap, other popovers already
    /// placed this frame.
    pub reserved: &'r [RectF],
    /// The sides to try, most preferred first.
    pub order: [Side; 4],
    /// How much worse, in the score's pixels, the side used last time may be
    /// than the best side before the popover moves to the best one. Keeps a
    /// popover near an edge from flipping back and forth as its point
    /// wavers.
    pub hysteresis: f32,
}

impl<'r> Anchoring<'r> {
    /// A popover of `size` at `point` within `bounds`: 8 px from the point
    /// and from the edges, nothing reserved, preferring above, then below,
    /// right and left, with 16 px of hysteresis.
    pub fn new(point: [f32; 2], size: [f32; 2], bounds: RectF) -> Anchoring<'r> {
        Anchoring {
            point,
            size,
            gap: 8.0,
            margin: 8.0,
            bounds,
            reserved: &[],
            order: [Side::Above, Side::Below, Side::Right, Side::Left],
            hysteresis: 16.0,
        }
    }
}

/// Where a popover went.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchored {
    /// The popover's rectangle, in screen space.
    pub rect: RectF,
    /// The side of the point it is on. Pass it back to the next
    /// [`place`] as `previous`.
    pub side: Side,
    /// The point on the popover's edge facing the anchor point that is
    /// nearest the anchor point, kept a few pixels from the corners: where a
    /// leader line or a pointer triangle meets the popover.
    pub tail: [f32; 2],
    /// False if the popover could not be placed entirely inside `bounds`
    /// (less the margin) or still overlaps a reserved rectangle. It is
    /// placed as well as it can be either way; the caller decides whether to
    /// draw it, shrink it or drop it.
    pub fits: bool,
}

/// How far a leader line's end is kept from the popover's corners, where
/// they are rounded.
const TAIL_INSET: f32 = 6.0;

/// Below this, in pixels, two edges are taken to coincide.
const EPSILON: f32 = 1e-3;

/// Place a popover beside `a.point`.
///
/// Each side is tried in turn. Its candidate is centred on the point across
/// the side (horizontally for above and below, vertically for left and
/// right) and set `gap` away from it, then slid across the side to stay
/// inside the bounds less the margin, as far as it can. Its score, in
/// pixels, is what then still sticks out of those bounds, summed over both
/// axes, plus, for each reserved rectangle, the square root of the area it
/// overlaps: the side of a square of the same area, so that both terms are
/// lengths and a small overlap costs a little and a large one a lot. The
/// lowest score wins, ties going to the earlier side in `a.order`.
///
/// Sliding before scoring means that a popover above a point near the right
/// edge of the screen is scored as it will be shown, slid left, and is not
/// passed over for another side because a centred popover would not have
/// fitted.
///
/// If `previous` is given and its score is no more than `a.hysteresis`
/// worse than the best, the popover stays on that side.
///
/// The chosen rectangle is then clamped into the bounds along the side's
/// axis too, when it fits there (a popover that cannot fit above or below
/// covers the point rather than leave the screen), and finally slid across
/// the side to the nearest position that overlaps reserved rectangles
/// least, staying in bounds.
pub fn place(a: &Anchoring<'_>, previous: Option<Side>) -> Anchored {
    let inner = a.bounds.inflate(-a.margin);
    let mut best: Option<(Side, f32)> = None;
    for side in a.order {
        let score = score(a, inner, side);
        if best.is_none_or(|(_, best)| score < best) {
            best = Some((side, score));
        }
    }
    let (mut side, best_score) = best.unwrap_or((Side::Above, f32::INFINITY));
    if let Some(previous) = previous {
        if score(a, inner, previous) <= best_score + a.hysteresis.max(0.0) {
            side = previous;
        }
    }

    let rect = slide_across(candidate(a, side), side, inner);
    let rect = clamp_along(rect, side, inner);
    let rect = avoid(rect, side, inner, a.reserved);
    let fits = within(rect, inner) && penalty(rect, a.reserved) == 0.0;
    Anchored {
        rect,
        side,
        tail: tail(rect, side, a.point),
        fits,
    }
}

/// Keep `previous`'s origin if `next` is within `px` of it (straight-line
/// distance), so that a popover whose point jitters by a pixel or so does
/// not shimmer. The size always comes from `next`. With no previous, or a
/// larger move, `next` as it is.
pub fn deadband(previous: Option<RectF>, next: RectF, px: f32) -> RectF {
    match previous {
        Some(previous) if (previous.x - next.x).hypot(previous.y - next.y) < px => {
            RectF::new(previous.x, previous.y, next.w, next.h)
        }
        _ => next,
    }
}

/// `v` limited to `lo..=hi`, never panicking: NaN stays NaN, and if `lo`
/// is above `hi` the result is `hi`.
fn limit(v: f32, lo: f32, hi: f32) -> f32 {
    v.max(lo).min(hi)
}

/// The popover's size, with nonsense taken as nothing.
fn size(a: &Anchoring<'_>) -> [f32; 2] {
    a.size.map(|v| v.max(0.0))
}

/// The rectangle centred across `side` and `gap` away from the point.
fn candidate(a: &Anchoring<'_>, side: Side) -> RectF {
    let [px, py] = a.point;
    let [w, h] = size(a);
    let (x, y) = match side {
        Side::Above => (px - w / 2.0, py - a.gap - h),
        Side::Below => (px - w / 2.0, py + a.gap),
        Side::Left => (px - a.gap - w, py - h / 2.0),
        Side::Right => (px + a.gap, py - h / 2.0),
    };
    RectF::new(x, y, w, h)
}

/// Where a span of `len` starting at `start` goes to lie within `lo..hi`:
/// moved as little as possible if it fits, centred on the range if not.
fn slide_into(start: f32, len: f32, lo: f32, hi: f32) -> f32 {
    if len <= hi - lo {
        limit(start, lo, hi - len)
    } else {
        lo + (hi - lo - len) / 2.0
    }
}

/// Slid across the side (the axis it is not offset along) into `inner`.
fn slide_across(rect: RectF, side: Side, inner: RectF) -> RectF {
    if side.is_vertical() {
        let x = slide_into(rect.x, rect.w, inner.x, inner.right());
        RectF::new(x, rect.y, rect.w, rect.h)
    } else {
        let y = slide_into(rect.y, rect.h, inner.y, inner.bottom());
        RectF::new(rect.x, y, rect.w, rect.h)
    }
}

/// Clamped along the side's own axis into `inner`, if it fits there.
fn clamp_along(rect: RectF, side: Side, inner: RectF) -> RectF {
    if side.is_vertical() {
        if rect.h <= inner.h {
            let y = limit(rect.y, inner.y, inner.bottom() - rect.h);
            return RectF::new(rect.x, y, rect.w, rect.h);
        }
    } else if rect.w <= inner.w {
        let x = limit(rect.x, inner.x, inner.right() - rect.w);
        return RectF::new(x, rect.y, rect.w, rect.h);
    }
    rect
}

/// Pixels of `rect` outside `inner`, summed over both axes and both ends.
fn overflow(rect: RectF, inner: RectF) -> f32 {
    (inner.x - rect.x).max(0.0)
        + (rect.right() - inner.right()).max(0.0)
        + (inner.y - rect.y).max(0.0)
        + (rect.bottom() - inner.bottom()).max(0.0)
}

/// Overlap with reserved rectangles, as lengths: the square root of each
/// overlapping area, summed.
fn penalty(rect: RectF, reserved: &[RectF]) -> f32 {
    reserved
        .iter()
        .map(|other| rect.overlap_area(*other).sqrt())
        .sum()
}

fn score(a: &Anchoring<'_>, inner: RectF, side: Side) -> f32 {
    let rect = slide_across(candidate(a, side), side, inner);
    let score = overflow(rect, inner) + penalty(rect, a.reserved);
    if score.is_nan() {
        f32::INFINITY
    } else {
        score
    }
}

/// Slid across the side to the in-bounds position nearest where it is that
/// overlaps the reserved rectangles least. The positions worth trying are
/// where it is and those just clear of each reserved rectangle on either
/// side, since overlap only changes its shape at those edges.
fn avoid(rect: RectF, side: Side, inner: RectF, reserved: &[RectF]) -> RectF {
    let current = penalty(rect, reserved);
    if current == 0.0 {
        return rect;
    }
    let vertical = side.is_vertical();
    let (start, len, lo, hi) = if vertical {
        (rect.x, rect.w, inner.x, inner.right())
    } else {
        (rect.y, rect.h, inner.y, inner.bottom())
    };
    let moved = |to: f32| {
        if vertical {
            RectF::new(to, rect.y, rect.w, rect.h)
        } else {
            RectF::new(rect.x, to, rect.w, rect.h)
        }
    };
    let mut best = (current, 0.0_f32, start);
    for other in reserved {
        let (other_start, other_end) = if vertical {
            (other.x, other.right())
        } else {
            (other.y, other.bottom())
        };
        for to in [other_start - len, other_end] {
            let to = slide_into(to, len, lo, hi);
            let cost = penalty(moved(to), reserved);
            let distance = (to - start).abs();
            if cost < best.0 || (cost == best.0 && distance < best.1) {
                best = (cost, distance, to);
            }
        }
    }
    moved(best.2)
}

/// Whether `rect` lies inside `inner`, give or take rounding.
fn within(rect: RectF, inner: RectF) -> bool {
    rect.x >= inner.x - EPSILON
        && rect.y >= inner.y - EPSILON
        && rect.right() <= inner.right() + EPSILON
        && rect.bottom() <= inner.bottom() + EPSILON
}

/// The point on the edge facing the anchor nearest to it, at least
/// [`TAIL_INSET`] from the corners (or at the middle of a shorter edge).
fn tail(rect: RectF, side: Side, point: [f32; 2]) -> [f32; 2] {
    let along = |p: f32, start: f32, len: f32| {
        let inset = TAIL_INSET.min(len / 2.0);
        limit(p, start + inset, start + len - inset)
    };
    match side {
        Side::Above => [along(point[0], rect.x, rect.w), rect.bottom()],
        Side::Below => [along(point[0], rect.x, rect.w), rect.y],
        Side::Left => [rect.right(), along(point[1], rect.y, rect.h)],
        Side::Right => [rect.x, along(point[1], rect.y, rect.h)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: RectF = RectF::new(0.0, 0.0, 1000.0, 1000.0);

    fn anchoring(point: [f32; 2]) -> Anchoring<'static> {
        Anchoring {
            gap: 10.0,
            margin: 0.0,
            ..Anchoring::new(point, [200.0, 100.0], SCREEN)
        }
    }

    #[test]
    fn the_preferred_side_is_used_when_it_fits() {
        let placed = place(&anchoring([500.0, 500.0]), None);
        assert_eq!(placed.side, Side::Above);
        assert_eq!(placed.rect, RectF::new(400.0, 390.0, 200.0, 100.0));
        assert!(placed.fits);
        assert_eq!(placed.tail, [500.0, 490.0]);
    }

    #[test]
    fn it_flips_away_from_each_edge() {
        use Side::{Above, Below, Left, Right};
        let cases = [
            ([500.0, 20.0], [Above, Below, Left, Right], Below),
            ([500.0, 980.0], [Below, Above, Left, Right], Above),
            ([20.0, 500.0], [Left, Right, Above, Below], Right),
            ([980.0, 500.0], [Right, Left, Above, Below], Left),
        ];
        for (point, order, expected) in cases {
            let placed = place(
                &Anchoring {
                    order,
                    ..anchoring(point)
                },
                None,
            );
            assert_eq!(placed.side, expected, "at {point:?}");
            assert!(placed.fits, "at {point:?}");
            assert!(!placed.rect.contains(point), "clear of the point");
        }
        let below = place(&anchoring([500.0, 20.0]), None);
        assert_eq!(below.rect, RectF::new(400.0, 30.0, 200.0, 100.0));
        assert_eq!(below.tail, [500.0, 30.0]);
    }

    #[test]
    fn near_a_side_edge_it_slides_instead_of_changing_side() {
        let placed = place(&anchoring([998.0, 500.0]), None);
        assert_eq!(placed.side, Side::Above);
        assert_eq!(placed.rect.x, 800.0);
        assert!(placed.fits);
        // The tail stays on the facing edge, short of the corner.
        assert_eq!(placed.tail, [994.0, 490.0]);

        let with_margin = place(
            &Anchoring {
                margin: 8.0,
                ..anchoring([998.0, 500.0])
            },
            None,
        );
        assert_eq!(with_margin.rect.x, 792.0);
    }

    #[test]
    fn hysteresis_keeps_the_previous_side_within_its_band() {
        // Above sticks out 5 px at the top, below fits: below is best, but
        // above is within 16 px of it.
        let near = anchoring([500.0, 105.0]);
        assert_eq!(place(&near, None).side, Side::Below);
        let kept = place(&near, Some(Side::Above));
        assert_eq!(kept.side, Side::Above);
        assert_eq!(kept.rect.y, 0.0, "clamped into the screen");
        assert!(kept.fits);
        // 30 px out is past the band: it moves.
        let far = anchoring([500.0, 80.0]);
        assert_eq!(place(&far, Some(Side::Above)).side, Side::Below);
        // A previous side that is as good as any is kept too.
        let open = anchoring([500.0, 500.0]);
        assert_eq!(place(&open, Some(Side::Left)).side, Side::Left);
        let strict = Anchoring {
            hysteresis: 0.0,
            ..near
        };
        assert_eq!(place(&strict, Some(Side::Above)).side, Side::Below);
    }

    #[test]
    fn a_reserved_rectangle_sends_it_to_another_side() {
        let hud = [RectF::new(300.0, 300.0, 400.0, 190.0)];
        let placed = place(
            &Anchoring {
                reserved: &hud,
                ..anchoring([500.0, 500.0])
            },
            None,
        );
        assert_eq!(placed.side, Side::Below);
        assert!(placed.fits);
        assert_eq!(placed.rect.overlap_area(hud[0]), 0.0);
    }

    #[test]
    fn a_reserved_rectangle_it_cannot_avoid_by_side_is_slid_clear_of() {
        let reserved = [
            // Over the left part of the spot above the point.
            RectF::new(380.0, 380.0, 60.0, 120.0),
            // Everything below the point.
            RectF::new(0.0, 505.0, 1000.0, 495.0),
        ];
        let placed = place(
            &Anchoring {
                reserved: &reserved,
                ..anchoring([500.0, 500.0])
            },
            None,
        );
        assert_eq!(placed.side, Side::Above);
        // The nearest clear position: just right of the first rectangle.
        assert_eq!(placed.rect, RectF::new(440.0, 390.0, 200.0, 100.0));
        assert!(placed.fits);
        assert_eq!(placed.tail, [500.0, 490.0]);
    }

    #[test]
    fn it_does_not_fit_when_it_is_too_big_or_cannot_get_clear() {
        let huge = place(
            &Anchoring {
                size: [2_000.0, 100.0],
                ..anchoring([500.0, 500.0])
            },
            None,
        );
        assert!(!huge.fits);
        assert!(huge.rect.w == 2_000.0);

        let everywhere = [SCREEN];
        let blocked = place(
            &Anchoring {
                reserved: &everywhere,
                ..anchoring([500.0, 500.0])
            },
            None,
        );
        assert!(!blocked.fits);
        assert_eq!(blocked.side, Side::Above, "all equal: the first choice");
    }

    #[test]
    fn the_tail_lies_on_the_edge_facing_the_point() {
        for (order, point) in [
            (
                [Side::Left, Side::Right, Side::Above, Side::Below],
                [500.0, 500.0],
            ),
            (
                [Side::Right, Side::Left, Side::Above, Side::Below],
                [500.0, 500.0],
            ),
            (
                [Side::Below, Side::Above, Side::Left, Side::Right],
                [500.0, 500.0],
            ),
            (
                [Side::Left, Side::Right, Side::Above, Side::Below],
                [500.0, 998.0],
            ),
        ] {
            let placed = place(
                &Anchoring {
                    order,
                    ..anchoring(point)
                },
                None,
            );
            let RectF { x, y, w, h } = placed.rect;
            let [tx, ty] = placed.tail;
            match placed.side {
                Side::Left => assert_eq!(tx, x + w),
                Side::Right => assert_eq!(tx, x),
                Side::Above => assert_eq!(ty, y + h),
                Side::Below => assert_eq!(ty, y),
            }
            if placed.side.is_vertical() {
                assert!(tx >= x + TAIL_INSET && tx <= x + w - TAIL_INSET);
            } else {
                assert!(ty >= y + TAIL_INSET && ty <= y + h - TAIL_INSET);
            }
        }
        // Near the bottom the popover on the left slides up, and the tail
        // stays inset from its bottom corner.
        let low = place(
            &Anchoring {
                order: [Side::Left, Side::Right, Side::Above, Side::Below],
                ..anchoring([500.0, 998.0])
            },
            None,
        );
        assert_eq!(low.rect, RectF::new(290.0, 900.0, 200.0, 100.0));
        assert_eq!(low.tail, [490.0, 994.0]);
    }

    #[test]
    fn nonsense_does_not_panic() {
        let placed = place(
            &Anchoring {
                point: [f32::NAN, 5.0],
                size: [-5.0, f32::NAN],
                ..anchoring([0.0, 0.0])
            },
            Some(Side::Left),
        );
        // Every side scores as unusable, so the previous one is kept, and
        // the nonsensical size is taken as nothing.
        assert_eq!(placed.side, Side::Left);
        assert_eq!((placed.rect.w, placed.rect.h), (0.0, 0.0));
    }

    #[test]
    fn deadband_keeps_a_jittering_origin_still() {
        let previous = RectF::new(100.0, 100.0, 50.0, 20.0);
        let jitter = RectF::new(100.8, 99.5, 52.0, 20.0);
        assert_eq!(
            deadband(Some(previous), jitter, 2.0),
            RectF::new(100.0, 100.0, 52.0, 20.0)
        );
        let moved = RectF::new(103.0, 100.0, 50.0, 20.0);
        assert_eq!(deadband(Some(previous), moved, 2.0), moved);
        assert_eq!(deadband(None, jitter, 2.0), jitter);
        assert_eq!(deadband(Some(previous), moved, 0.0), moved);
    }
}
