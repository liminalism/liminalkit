//! A virtualised, scrollable list of variable-height rows.
//!
//! The caller supplies row heights up front (cheap for the row counts a
//! screen holds) and a closure that draws one row; only rows intersecting the
//! viewport are drawn. Wheel input over the area scrolls it; a thin scrollbar
//! shows where you are.

pub use crate::widget::ScrollState;
use crate::widget::Ui;
use liminalkit_raster::Rect;

/// Result of laying out a scroll list this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollList {
    /// The viewport rows are drawn into (excludes the scrollbar gutter).
    pub viewport: Rect,
    /// Total content height.
    pub content: i32,
    /// First and one-past-last row indices that were visible.
    pub visible: (usize, usize),
}

/// Scrollbar gutter width in logical pixels.
pub const GUTTER: i32 = 6;

impl Ui<'_> {
    /// Draw a scroll list. `heights[i]` is row `i`'s height in physical
    /// pixels; `draw(ui, index, rect)` paints one row. Returns what was
    /// visible so callers can hit-test against the same geometry.
    pub fn scroll_list(
        &mut self,
        state: &mut ScrollState,
        area: Rect,
        heights: &[i32],
        mut draw: impl FnMut(&mut Ui<'_>, usize, Rect),
    ) -> ScrollList {
        let content: i32 = heights.iter().sum();
        let gutter = self.px(GUTTER);
        let needs_bar = content > area.h;
        let viewport = if needs_bar {
            Rect::new(area.x, area.y, (area.w - gutter).max(0), area.h)
        } else {
            area
        };

        let (_, dy) = self.input.take_scroll(area);
        if dy != 0.0 {
            state.scroll_by(-dy.round() as i32, viewport.h, content);
        }
        state.clamp_to(viewport.h, content);
        let offset = state.offset();

        self.painter.push_clip(viewport);
        let mut y = viewport.y - offset;
        let mut first = None;
        let mut last = 0;
        for (index, &height) in heights.iter().enumerate() {
            let rect = Rect::new(viewport.x, y, viewport.w, height);
            if rect.bottom() > viewport.y && rect.y < viewport.bottom() {
                if first.is_none() {
                    first = Some(index);
                }
                last = index + 1;
                draw(self, index, rect);
            } else if rect.y >= viewport.bottom() {
                break;
            }
            y += height;
        }
        self.painter.pop_clip();

        if needs_bar {
            let track = Rect::new(viewport.right(), area.y, gutter, area.h);
            let thumb_h = ((area.h as i64 * area.h as i64) / content.max(1) as i64)
                .max(self.px(18) as i64) as i32;
            let max = ScrollState::max_offset(viewport.h, content);
            let travel = (area.h - thumb_h).max(0);
            let thumb_y = if max > 0 {
                area.y + (travel as i64 * offset as i64 / max as i64) as i32
            } else {
                area.y
            };
            let inset = self.px(2);
            let bar = Rect::new(
                track.x + inset,
                thumb_y,
                (gutter - inset * 2).max(1),
                thumb_h.min(area.h),
            );
            self.painter.fill_rect(bar, self.theme.panel_edge);
        }

        ScrollList {
            viewport,
            content,
            visible: (first.unwrap_or(0), last),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::Theme;
    use liminalkit_raster::{Painter, RasterKernel, WindowBuffer};
    use liminalkit_shell::{Input, MouseButton, Scale};
    use liminalkit_text::font::test_fonts::set;
    use liminalkit_text::TextCache;

    #[test]
    fn native_wheel_reveals_hidden_row_and_click_activates_that_row() {
        let mut buffer = WindowBuffer::new(200, 100);
        let mut text = TextCache::new(set());
        let mut kernel = RasterKernel::new();
        let mut input = Input::new();
        let mut state = ScrollState::new();
        let heights = [30; 20];
        let area = Rect::new(0, 0, 200, 100);
        input.cursor_moved(50.0, 50.0);
        // The native host reports wheel-down as negative, like scrollable().
        input.scrolled(0.0, -95.0);
        {
            let mut ui = Ui::new(
                Painter::new(&mut buffer),
                &mut text,
                &mut input,
                Theme::default(),
                Scale::ONE,
                &mut kernel,
            );
            let visible = ui.scroll_list(&mut state, area, &heights, |_, _, _| {});
            assert_eq!(visible.visible, (3, 7));
        }
        input.cursor_moved(50.0, 40.0);
        input.mouse(MouseButton::Left, true);
        let mut activated = None;
        {
            let mut ui = Ui::new(
                Painter::new(&mut buffer),
                &mut text,
                &mut input,
                Theme::default(),
                Scale::ONE,
                &mut kernel,
            );
            ui.scroll_list(&mut state, area, &heights, |ui, row, rect| {
                if ui.button(rect, "Choose") {
                    activated = Some(row);
                }
            });
        }
        assert_eq!(
            activated,
            Some(4),
            "the revealed row owns its painted hit target"
        );
        input.mouse(MouseButton::Left, false);
        input.scrolled(0.0, 95.0);
        let mut ui = Ui::new(
            Painter::new(&mut buffer),
            &mut text,
            &mut input,
            Theme::default(),
            Scale::ONE,
            &mut kernel,
        );
        let visible = ui.scroll_list(&mut state, area, &heights, |_, _, _| {});
        assert_eq!(
            visible.visible,
            (0, 4),
            "wheel-up returns to the first rows"
        );
    }

    #[test]
    fn a_short_list_neither_scrolls_nor_reserves_a_gutter() {
        let mut buffer = WindowBuffer::new(200, 100);
        let mut text = TextCache::new(set());
        let mut kernel = RasterKernel::new();
        let mut input = Input::new();
        input.cursor_moved(50.0, 50.0);
        input.scrolled(0.0, 40.0);
        let mut state = ScrollState::new();
        let mut ui = Ui::new(
            Painter::new(&mut buffer),
            &mut text,
            &mut input,
            Theme::default(),
            Scale::ONE,
            &mut kernel,
        );
        let out = ui.scroll_list(
            &mut state,
            Rect::new(0, 0, 200, 100),
            &[20, 20],
            |_, _, _| {},
        );
        assert_eq!(state.offset(), 0);
        assert_eq!(out.viewport.w, 200);
    }
}
