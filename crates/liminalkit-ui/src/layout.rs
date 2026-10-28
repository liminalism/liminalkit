//! Grid/flow layout: equally sized items placed left-to-right, wrapping to a
//! new row when the next one would not fit.
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
}
