//! Stateless layout: stacks, grids, insets and alignment, as plain functions
//! from a rectangle to rectangles.
//!
//! Nothing here is retained between frames and nothing is owned by the
//! toolkit. A screen computes the rectangles it needs, in whatever order it
//! likes, and hands them to controls. Manual layout is meant to be easy, not
//! eliminated: [`vstack`]/[`hstack`] replace a column of hand-written
//! `Rect::new(panel.x + 12, panel.y + 70, ...)` arithmetic with a list of
//! [`Length`]s, [`Flow`] replaces the running `y += ...` cursor, and
//! [`Insets`] and [`place`] handle margins and alignment. A screen that is
//! happier with explicit arithmetic keeps using it; the two mix freely
//! because they produce the same thing.
//!
//! All sizes here are in whatever unit the rectangle is in. Controls take
//! device pixels, so convert logical sizes once, with [`Scale::px`] or
//! [`Ui::px`], before building a stack, or build the stack over a logical
//! rectangle and convert the results with [`Scale::rect`].
//!
//! [`Scale::px`]: liminalkit_shell::Scale::px
//! [`Scale::rect`]: liminalkit_shell::Scale::rect
//! [`Ui::px`]: crate::Ui::px
//!
//! # Grids
//!
//! Equally sized items placed left-to-right, wrapping to a new row when the
//! next one would not fit.
//!
//! `column_widths` (in `widget.rs`) answers "how wide is each of these few,
//! differently sized columns" for a table header. This answers a different
//! question — "where does item *N* of many identically sized ones go" — for
//! the Home Surface's file grid and the dock's icon row. Pure arithmetic over
//! rects, the same way `column_widths` is: no constraint solver, no owned
//! state, nothing to construct before asking it a question.
//!
//! [`grid_item`] computes one item's rect in O(1) rather than the whole
//! layout at once, so a file grid with thousands of entries can lay out only
//! the handful actually on screen — the same reason [`crate::Ui::table`]
//! builds only the rows its viewport can show rather than all of them.
//! [`grid`] is the convenience form for a grid short enough (a dock) that
//! building all of it costs nothing.
//!
//! A grid's *height* never bounds the layout: items keep flowing downward
//! for as many rows as `count` needs, exactly as `Ui::table`'s own content
//! height is `row_count * row_height` regardless of the viewport it is
//! later scrolled and clipped against. `grid_visible_range` is that same
//! scrolling arithmetic, generalised from one column of rows to a grid of
//! `columns` per row.

use liminalkit_raster::Rect;

/// How many `item_w`-wide items, `gap` apart, fit across `width` before
/// wrapping — always at least one, even when a single item does not itself
/// fit, so a narrow container crowds its items rather than laying out zero
/// columns and losing every item to the same overlapping cell. Zero for a
/// non-positive item width, which has no sensible layout at all.
pub fn columns_per_row(width: i32, item_w: i32, gap: i32) -> usize {
    if item_w <= 0 {
        return 0;
    }
    let gap = gap.max(0);
    let step = item_w + gap;
    (((width + gap) / step).max(1)) as usize
}

/// The rect of item `index` in a left-to-right, top-to-bottom flow of
/// `item`-sized cells, `gap` apart, wrapping at `columns` per row and
/// anchored at `origin`.
pub fn grid_item(
    origin: (i32, i32),
    item: (i32, i32),
    gap: (i32, i32),
    columns: usize,
    index: usize,
) -> Rect {
    let columns = columns.max(1);
    let gap = (gap.0.max(0), gap.1.max(0));
    let row = (index / columns) as i32;
    let col = (index % columns) as i32;
    let x = origin.0 + col * (item.0 + gap.0);
    let y = origin.1 + row * (item.1 + gap.1);
    Rect::new(x, y, item.0, item.1)
}

/// Every item's rect for a grid of `count` items sized `item`, `gap` apart,
/// wrapped to fit `area`'s width and starting at its top-left corner.
/// `area.h` does not limit how many rows are produced — see the module docs.
///
/// For a grid large enough that building every rect every frame matters,
/// call [`columns_per_row`] once and [`grid_item`] per visible index
/// instead, the way [`crate::Ui::table`] builds only the rows it draws.
pub fn grid(area: Rect, item: (i32, i32), gap: (i32, i32), count: usize) -> Vec<Rect> {
    let columns = columns_per_row(area.w, item.0, gap.0);
    if columns == 0 {
        return Vec::new();
    }
    (0..count)
        .map(|index| grid_item((area.x, area.y), item, gap, columns, index))
        .collect()
}

/// The total height a grid of `count` items needs — exactly enough rows,
/// with no trailing gap after the last one, which is what
/// [`ScrollState::max_offset`](crate::ScrollState::max_offset) should be
/// given as `content`.
pub fn grid_content_height(item_h: i32, gap_y: i32, columns: usize, count: usize) -> i32 {
    if columns == 0 || count == 0 {
        return 0;
    }
    let gap_y = gap_y.max(0);
    let rows = count.div_ceil(columns) as i32;
    rows * item_h + (rows - 1).max(0) * gap_y
}

/// Which item indices (`first..last`, half-open, `last` exclusive and
/// possibly past `count`) could be visible in a `viewport_h`-tall window
/// scrolled `offset` pixels into a grid of `count` items — the windowing
/// [`Ui::table`](crate::Ui::table) does for its rows, generalised to a
/// grid's rows of `columns` items each.
pub fn grid_visible_range(
    viewport_h: i32,
    offset: i32,
    item_h: i32,
    gap_y: i32,
    columns: usize,
    count: usize,
) -> (usize, usize) {
    if columns == 0 || count == 0 {
        return (0, 0);
    }
    let row_h = (item_h + gap_y.max(0)).max(1);
    let first_row = (offset / row_h).max(0) as usize;
    let last_row = ((offset + viewport_h) / row_h) as usize + 1;
    let first = (first_row * columns).min(count);
    let last = (last_row * columns).min(count);
    (first, last)
}

// --- stacks -----------------------------------------------------------------

/// How much of a stack's main axis one item takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    /// Exactly this many pixels.
    Px(i32),
    /// A weighted share of whatever is left once every `Px` item and every
    /// gap has been placed. Two `Flex(1.0)` items split the remainder
    /// evenly; `Flex(2.0)` beside `Flex(1.0)` takes two thirds. A negative
    /// or zero weight takes nothing.
    Flex(f32),
    /// A weighted share of the remainder, but never less than `min`
    /// pixels: a flexible column that must stay readable.
    FlexMin(f32, i32),
}

impl Length {
    /// `Flex(1.0)`: an even share of the remainder.
    pub const FILL: Length = Length::Flex(1.0);
}

/// Resolve `lengths` along a `total`-long axis with `gap` between
/// consecutive items. The result has one entry per input, in order, and
/// the entries plus the gaps sum to exactly `total` whenever any item is
/// flexible and the fixed items fit. When they do not fit, the fixed items
/// keep their sizes, flexible ones get their minimums, and the stack
/// overflows the axis rather than squeezing a fixed item.
///
/// Rounding is cumulative, so a remainder of 100 split three ways is
/// 33, 34, 33 and nothing is lost.
pub fn stack_lengths(total: i32, gap: i32, lengths: &[Length]) -> Vec<i32> {
    if lengths.is_empty() {
        return Vec::new();
    }
    let gap = gap.max(0);
    let gaps = gap * (lengths.len() as i32 - 1);
    let fixed: i32 = lengths
        .iter()
        .map(|length| match length {
            Length::Px(px) => (*px).max(0),
            Length::FlexMin(_, min) => (*min).max(0),
            Length::Flex(_) => 0,
        })
        .sum();
    let spare = (total - gaps - fixed).max(0);
    let weight_of = |length: &Length| match length {
        Length::Flex(w) | Length::FlexMin(w, _) => w.max(0.0),
        Length::Px(_) => 0.0,
    };
    let weights: f32 = lengths.iter().map(weight_of).sum();

    let mut out = Vec::with_capacity(lengths.len());
    let mut cumulative = 0.0f32;
    let mut given = 0;
    for length in lengths {
        let share = if weights > 0.0 {
            cumulative += weight_of(length);
            let target = (spare as f32 * cumulative / weights).round() as i32;
            let share = (target - given).max(0);
            given += share;
            share
        } else {
            0
        };
        out.push(match length {
            Length::Px(px) => (*px).max(0),
            Length::Flex(_) => share,
            Length::FlexMin(_, min) => (*min).max(0) + share,
        });
    }
    out
}

/// Stack `lengths` top to bottom inside `area`, `gap` apart, each item as
/// wide as `area`. See [`stack_lengths`] for how the remainder is shared.
pub fn vstack(area: Rect, gap: i32, lengths: &[Length]) -> Vec<Rect> {
    let gap = gap.max(0);
    let mut y = area.y;
    stack_lengths(area.h, gap, lengths)
        .into_iter()
        .map(|h| {
            let rect = Rect::new(area.x, y, area.w, h);
            y += h + gap;
            rect
        })
        .collect()
}

/// Stack `lengths` left to right inside `area`, `gap` apart, each item as
/// tall as `area`. See [`stack_lengths`] for how the remainder is shared.
pub fn hstack(area: Rect, gap: i32, lengths: &[Length]) -> Vec<Rect> {
    let gap = gap.max(0);
    let mut x = area.x;
    stack_lengths(area.w, gap, lengths)
        .into_iter()
        .map(|w| {
            let rect = Rect::new(x, area.y, w, area.h);
            x += w + gap;
            rect
        })
        .collect()
}

/// A running cursor over one axis of a rectangle, for the common screen that
/// places a few controls of known size and gives the rest to one of them.
///
/// ```
/// use liminalkit_raster::Rect;
/// use liminalkit_ui::layout::Flow;
///
/// let mut flow = Flow::down(Rect::new(0, 0, 300, 400), 8);
/// let title = flow.take(32);
/// let toolbar = flow.take(40);
/// let status = flow.take_end(24);
/// let body = flow.rest();
/// assert_eq!(title, Rect::new(0, 0, 300, 32));
/// assert_eq!(toolbar, Rect::new(0, 40, 300, 40));
/// assert_eq!(status, Rect::new(0, 376, 300, 24));
/// assert_eq!(body, Rect::new(0, 88, 300, 280));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flow {
    remaining: Rect,
    gap: i32,
    vertical: bool,
}

impl Flow {
    /// Place items from the top of `area` downwards.
    pub fn down(area: Rect, gap: i32) -> Flow {
        Flow {
            remaining: area,
            gap: gap.max(0),
            vertical: true,
        }
    }

    /// Place items from the left of `area` rightwards.
    pub fn right(area: Rect, gap: i32) -> Flow {
        Flow {
            remaining: area,
            gap: gap.max(0),
            vertical: false,
        }
    }

    /// The next `length` pixels from the start, then a gap.
    pub fn take(&mut self, length: i32) -> Rect {
        let length = length.max(0);
        let (item, rest) = if self.vertical {
            self.remaining.split_top(length)
        } else {
            self.remaining.split_left(length)
        };
        self.remaining = self.skip_in(rest);
        item
    }

    /// `length` pixels from the far end, then a gap, leaving the start
    /// alone: a status line at the bottom, a close button at the right.
    pub fn take_end(&mut self, length: i32) -> Rect {
        let length = length.max(0);
        let (item, rest) = if self.vertical {
            self.remaining.split_bottom(length)
        } else {
            self.remaining.split_right(length)
        };
        self.remaining = if self.vertical {
            Rect::new(rest.x, rest.y, rest.w, (rest.h - self.gap).max(0))
        } else {
            Rect::new(rest.x, rest.y, (rest.w - self.gap).max(0), rest.h)
        };
        item
    }

    /// Leave `length` pixels empty from the start.
    pub fn skip(&mut self, length: i32) {
        let length = length.max(0);
        self.remaining = if self.vertical {
            self.remaining.split_top(length).1
        } else {
            self.remaining.split_left(length).1
        };
    }

    /// Whatever has not been taken. Taking more afterwards is allowed: this
    /// does not consume anything.
    pub fn rest(&self) -> Rect {
        self.remaining
    }

    fn skip_in(&self, rest: Rect) -> Rect {
        if self.vertical {
            Rect::new(
                rest.x,
                rest.y + self.gap,
                rest.w,
                (rest.h - self.gap).max(0),
            )
        } else {
            Rect::new(
                rest.x + self.gap,
                rest.y,
                (rest.w - self.gap).max(0),
                rest.h,
            )
        }
    }
}

// --- insets and alignment ------------------------------------------------

/// Margins or padding on each side of a rectangle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Insets {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

impl Insets {
    /// The same amount on every side; [`Rect::inset`] with a name.
    pub const fn uniform(amount: i32) -> Insets {
        Insets {
            top: amount,
            right: amount,
            bottom: amount,
            left: amount,
        }
    }

    /// `vertical` on the top and bottom, `horizontal` on the left and right.
    pub const fn symmetric(vertical: i32, horizontal: i32) -> Insets {
        Insets {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    /// Every side through `scale`, for insets written in logical pixels.
    pub fn scaled(self, scale: liminalkit_shell::Scale) -> Insets {
        Insets {
            top: scale.px(self.top),
            right: scale.px(self.right),
            bottom: scale.px(self.bottom),
            left: scale.px(self.left),
        }
    }

    /// `area` shrunk by these insets. A rectangle too small for its insets
    /// collapses to zero size at its top-left plus the insets, rather than
    /// going negative.
    pub fn apply(self, area: Rect) -> Rect {
        Rect::new(
            area.x + self.left,
            area.y + self.top,
            (area.w - self.left - self.right).max(0),
            (area.h - self.top - self.bottom).max(0),
        )
    }

    /// `area` grown by these insets: the surface around some content.
    pub fn outset(self, area: Rect) -> Rect {
        Rect::new(
            area.x - self.left,
            area.y - self.top,
            area.w + self.left + self.right,
            area.h + self.top + self.bottom,
        )
    }
}

/// Where a box sits along one axis of the area it is placed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Centre,
    End,
}

/// A `size`d rectangle placed inside `area` at the given anchors: a button
/// centred in a footer, an icon flush right. A box larger than the area
/// is placed from the anchor side and overflows the other.
pub fn place(area: Rect, size: (i32, i32), horizontal: Anchor, vertical: Anchor) -> Rect {
    let (w, h) = (size.0.max(0), size.1.max(0));
    let along = |start: i32, extent: i32, length: i32, anchor: Anchor| match anchor {
        Anchor::Start => start,
        Anchor::Centre => start + (extent - length) / 2,
        Anchor::End => start + extent - length,
    };
    Rect::new(
        along(area.x, area.w, w, horizontal),
        along(area.y, area.h, h, vertical),
        w,
        h,
    )
}

/// [`place`] at the centre of both axes.
pub fn centre(area: Rect, size: (i32, i32)) -> Rect {
    place(area, size, Anchor::Centre, Anchor::Centre)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- columns_per_row --------------------------------------------------

    #[test]
    fn columns_fill_the_width_without_overflowing_it() {
        // 3 columns of 20 with a 10 gap between them is 80; a 4th would be
        // 110, past the 100 available.
        assert_eq!(columns_per_row(100, 20, 10), 3);
    }

    #[test]
    fn an_exact_fit_is_not_short_by_one() {
        // Exactly 3 columns of 20 with 10 gaps between: 20*3 + 10*2 = 80.
        assert_eq!(columns_per_row(80, 20, 10), 3);
    }

    #[test]
    fn a_container_narrower_than_one_item_still_gets_one_column() {
        // Crowds, the same way a fixed table column keeps its width in a
        // panel too narrow for it rather than being asked to vanish.
        assert_eq!(columns_per_row(10, 64, 8), 1);
    }

    #[test]
    fn zero_gap_packs_items_edge_to_edge() {
        assert_eq!(columns_per_row(100, 25, 0), 4);
    }

    #[test]
    fn a_non_positive_item_width_has_no_columns() {
        assert_eq!(columns_per_row(100, 0, 8), 0);
        assert_eq!(columns_per_row(100, -5, 8), 0);
    }

    #[test]
    fn a_negative_gap_is_treated_as_zero() {
        assert_eq!(columns_per_row(100, 25, -100), columns_per_row(100, 25, 0));
    }

    // --- grid_item ----------------------------------------------------------

    #[test]
    fn the_first_item_sits_at_the_origin() {
        let rect = grid_item((5, 5), (40, 30), (8, 8), 3, 0);
        assert_eq!(rect, Rect::new(5, 5, 40, 30));
    }

    #[test]
    fn items_advance_across_a_row_then_wrap_to_the_next() {
        let origin = (0, 0);
        let item = (40, 30);
        let gap = (8, 8);
        let columns = 3;
        assert_eq!(
            grid_item(origin, item, gap, columns, 1),
            Rect::new(48, 0, 40, 30)
        );
        assert_eq!(
            grid_item(origin, item, gap, columns, 2),
            Rect::new(96, 0, 40, 30)
        );
        // Index 3 wraps: back to column 0, one row down.
        assert_eq!(
            grid_item(origin, item, gap, columns, 3),
            Rect::new(0, 38, 40, 30)
        );
        assert_eq!(
            grid_item(origin, item, gap, columns, 4),
            Rect::new(48, 38, 40, 30)
        );
    }

    #[test]
    fn zero_columns_is_treated_as_one_rather_than_dividing_by_zero() {
        let rect = grid_item((0, 0), (10, 10), (0, 0), 0, 5);
        assert_eq!(rect, Rect::new(0, 50, 10, 10));
    }

    // --- grid -----------------------------------------------------------------

    #[test]
    fn a_grid_produces_exactly_count_rects_all_within_its_columns() {
        let area = Rect::new(0, 0, 130, 1_000); // 3 columns of 40 with 5 gaps
        let rects = grid(area, (40, 40), (5, 5), 10);
        assert_eq!(rects.len(), 10);
        for rect in &rects {
            assert!(
                rect.x >= area.x && rect.right() <= area.x + 130,
                "{rect:?} escaped the width"
            );
        }
        // Every rect is distinct — nothing landed on top of anything else.
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                assert_ne!(rects[i], rects[j], "{i} and {j} landed on the same cell");
            }
        }
    }

    #[test]
    fn a_grid_of_zero_items_is_empty_not_a_crash() {
        assert!(grid(Rect::new(0, 0, 200, 200), (40, 40), (8, 8), 0).is_empty());
    }

    #[test]
    fn a_grid_with_a_non_positive_item_size_is_empty_not_a_crash() {
        assert!(grid(Rect::new(0, 0, 200, 200), (0, 40), (8, 8), 5).is_empty());
    }

    #[test]
    fn a_grids_height_does_not_limit_how_many_rows_it_produces() {
        // A one-pixel-tall area still lays out every requested item — the
        // caller scrolls and clips, the layout does not truncate.
        let area = Rect::new(0, 0, 40, 1);
        let rects = grid(area, (40, 40), (0, 0), 20);
        assert_eq!(rects.len(), 20);
        assert_eq!(rects.last().unwrap().y, 19 * 40);
    }

    // --- grid_content_height ------------------------------------------------

    #[test]
    fn content_height_covers_exactly_the_rows_needed_with_no_trailing_gap() {
        // 7 items at 3 columns is 3 rows (3, 3, 1); 3 rows of 40 with 2 gaps
        // of 8 between them, and nothing after the last row.
        assert_eq!(grid_content_height(40, 8, 3, 7), 3 * 40 + 2 * 8);
    }

    #[test]
    fn a_full_last_row_still_has_no_trailing_gap() {
        assert_eq!(grid_content_height(40, 8, 3, 6), 2 * 40 + 8);
    }

    #[test]
    fn a_single_item_is_one_row_with_no_gap_at_all() {
        assert_eq!(grid_content_height(40, 8, 3, 1), 40);
    }

    #[test]
    fn no_items_is_zero_height() {
        assert_eq!(grid_content_height(40, 8, 3, 0), 0);
        assert_eq!(grid_content_height(40, 8, 0, 5), 0);
    }

    // --- grid_visible_range ---------------------------------------------------

    #[test]
    fn nothing_scrolled_shows_the_first_rows_that_fit() {
        // 100px viewport, 40px rows (30 item + 10 gap): rows 0, 1, and the
        // one straddling the bottom edge — three rows of 4 columns.
        let (first, last) = grid_visible_range(100, 0, 30, 10, 4, 100);
        assert_eq!((first, last), (0, 12));
    }

    #[test]
    fn scrolling_down_shifts_the_visible_window_by_whole_rows() {
        let (first, last) = grid_visible_range(100, 40, 30, 10, 4, 100);
        assert_eq!(first, 4, "the first row scrolled out");
        assert!(last > first);
    }

    #[test]
    fn the_visible_range_never_reaches_past_the_item_count() {
        let (first, last) = grid_visible_range(1_000, 0, 30, 10, 4, 10);
        assert_eq!(last, 10);
        assert!(first <= last);
    }

    #[test]
    fn an_empty_grid_has_nothing_visible() {
        assert_eq!(grid_visible_range(500, 0, 30, 10, 4, 0), (0, 0));
        assert_eq!(grid_visible_range(500, 0, 30, 10, 0, 20), (0, 0));
    }

    // --- stack_lengths -------------------------------------------------------

    #[test]
    fn fixed_items_keep_their_size_and_flex_takes_the_rest() {
        assert_eq!(
            stack_lengths(300, 10, &[Length::Px(40), Length::FILL, Length::Px(20)]),
            vec![40, 220, 20]
        );
    }

    #[test]
    fn flex_weights_share_the_remainder_in_proportion_and_sum_exactly() {
        // 100 left after no fixed items and two gaps of 0.
        let lengths = stack_lengths(100, 0, &[Length::FILL, Length::FILL, Length::FILL]);
        assert_eq!(
            lengths.iter().sum::<i32>(),
            100,
            "no pixel is lost to rounding"
        );
        assert_eq!(lengths, vec![33, 34, 33]);
        assert_eq!(
            stack_lengths(90, 0, &[Length::Flex(2.0), Length::Flex(1.0)]),
            vec![60, 30]
        );
    }

    #[test]
    fn a_flex_minimum_is_honoured_before_the_remainder_is_shared() {
        // 200 total, 20 fixed, 180 spare before minimums; FlexMin takes its
        // 100 first, leaving 80 to split by weight (1:1 → 40 each).
        assert_eq!(
            stack_lengths(
                200,
                0,
                &[Length::Px(20), Length::FlexMin(1.0, 100), Length::FILL]
            ),
            vec![20, 140, 40]
        );
    }

    #[test]
    fn when_fixed_items_do_not_fit_nothing_is_squeezed_and_flex_gets_zero() {
        assert_eq!(
            stack_lengths(50, 10, &[Length::Px(40), Length::FILL, Length::Px(40)]),
            vec![40, 0, 40]
        );
    }

    #[test]
    fn non_positive_weights_and_sizes_take_nothing() {
        assert_eq!(
            stack_lengths(
                100,
                0,
                &[Length::Flex(0.0), Length::Flex(-1.0), Length::FILL]
            ),
            vec![0, 0, 100]
        );
        assert_eq!(stack_lengths(100, 0, &[Length::Px(-5)]), vec![0]);
        assert!(stack_lengths(100, 0, &[]).is_empty());
    }

    #[test]
    fn only_fixed_items_leave_the_remainder_unused() {
        assert_eq!(
            stack_lengths(100, 5, &[Length::Px(10), Length::Px(10)]),
            vec![10, 10]
        );
    }

    // --- vstack / hstack -------------------------------------------------------

    #[test]
    fn a_vertical_stack_places_items_top_down_with_gaps_and_full_width() {
        let rects = vstack(
            Rect::new(10, 20, 200, 300),
            8,
            &[Length::Px(32), Length::FILL, Length::Px(24)],
        );
        assert_eq!(rects[0], Rect::new(10, 20, 200, 32));
        assert_eq!(rects[1], Rect::new(10, 60, 200, 228));
        assert_eq!(rects[2], Rect::new(10, 296, 200, 24));
        assert_eq!(
            rects[2].bottom(),
            320,
            "the last item ends at the area's bottom"
        );
    }

    #[test]
    fn a_horizontal_stack_places_items_left_to_right_with_full_height() {
        let rects = hstack(
            Rect::new(0, 0, 100, 40),
            10,
            &[Length::FILL, Length::Px(30), Length::FILL],
        );
        assert_eq!(rects[0], Rect::new(0, 0, 25, 40));
        assert_eq!(rects[1], Rect::new(35, 0, 30, 40));
        assert_eq!(rects[2], Rect::new(75, 0, 25, 40));
        assert_eq!(rects[2].right(), 100);
    }

    // --- Flow ------------------------------------------------------------------

    #[test]
    fn a_flow_hands_out_slices_in_order_and_the_rest_is_what_is_left() {
        let mut flow = Flow::right(Rect::new(0, 0, 100, 20), 4);
        assert_eq!(flow.take(30), Rect::new(0, 0, 30, 20));
        assert_eq!(flow.take_end(10), Rect::new(90, 0, 10, 20));
        flow.skip(6);
        assert_eq!(flow.rest(), Rect::new(40, 0, 46, 20));
    }

    #[test]
    fn an_exhausted_flow_hands_out_empty_rects_rather_than_negative_ones() {
        let mut flow = Flow::down(Rect::new(0, 0, 10, 10), 2);
        flow.take(10);
        let next = flow.take(5);
        assert_eq!(next.h, 0);
        assert!(flow.rest().h == 0 && flow.rest().w == 10);
    }

    // --- Insets / place ----------------------------------------------------------

    #[test]
    fn insets_shrink_and_outset_grows_back() {
        let insets = Insets::symmetric(2, 5);
        let inner = insets.apply(Rect::new(0, 0, 100, 50));
        assert_eq!(inner, Rect::new(5, 2, 90, 46));
        assert_eq!(insets.outset(inner), Rect::new(0, 0, 100, 50));
        assert_eq!(Insets::uniform(3).apply(Rect::new(0, 0, 4, 4)).w, 0);
    }

    #[test]
    fn insets_scale_each_side() {
        let scaled = Insets::symmetric(10, 4).scaled(liminalkit_shell::Scale(2.0));
        assert_eq!(scaled, Insets::symmetric(20, 8));
    }

    #[test]
    fn place_anchors_a_box_in_an_area() {
        let area = Rect::new(10, 10, 100, 60);
        assert_eq!(centre(area, (20, 10)), Rect::new(50, 35, 20, 10));
        assert_eq!(
            place(area, (20, 10), Anchor::End, Anchor::Start),
            Rect::new(90, 10, 20, 10)
        );
        assert_eq!(
            place(area, (200, 10), Anchor::Start, Anchor::End),
            Rect::new(10, 60, 200, 10),
            "an oversized box starts at its anchor and overflows"
        );
    }
}
