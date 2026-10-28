//! A tooltip requested during the frame and drawn after everything else.
//!
//! Immediate mode has no z-order, so the widget that wants a tooltip cannot
//! draw it — whatever is painted next would cover it. It records a request
//! instead; the app calls [`Tooltip::draw`] last.

use crate::widget::Ui;
use liminalkit_raster::Rect;
use liminalkit_text::{Align, TextStyle};

#[derive(Debug, Default)]
pub struct Tooltip {
    request: Option<Request>,
}

#[derive(Debug)]
struct Request {
    /// Physical anchor: the pointer, usually.
    x: i32,
    y: i32,
    lines: Vec<(String, TextStyle, u32)>,
}

/// Look of the tooltip surface.
#[derive(Debug, Clone, Copy)]
pub struct TooltipStyle {
    pub fill: u32,
    pub edge: u32,
    /// Logical padding.
    pub padding: i32,
    /// Logical max width; lines are truncated with an ellipsis beyond it.
    pub max_width: i32,
    /// Logical offset from the anchor.
    pub offset: (i32, i32),
}

impl Tooltip {
    pub fn new() -> Tooltip {
        Tooltip::default()
    }

    /// Ask for a tooltip at a physical anchor. Later requests in the same
    /// frame replace earlier ones.
    pub fn request(&mut self, x: i32, y: i32, lines: Vec<(String, TextStyle, u32)>) {
        self.request = Some(Request { x, y, lines });
    }

    pub fn is_pending(&self) -> bool {
        self.request.is_some()
    }

    /// Draw the pending tooltip (if any) inside `bounds`, then clear it.
    pub fn draw(&mut self, ui: &mut Ui<'_>, bounds: Rect, style: TooltipStyle) {
        let Some(request) = self.request.take() else {
            return;
        };
        if request.lines.is_empty() {
            return;
        }
        let padding = ui.px(style.padding);
        let max_width = ui.px(style.max_width);
        let mut width = 0;
        let mut height = 0;
        let mut rows = Vec::with_capacity(request.lines.len());
        for (text, text_style, colour) in &request.lines {
            let line_h = ui.text.line_height(*text_style);
            let w = ui.text.measure(text, *text_style).min(max_width);
            width = width.max(w);
            rows.push((line_h, *text_style, *colour, text.clone()));
            height += line_h;
        }
        let w = width + padding * 2;
        let h = height + padding * 2;
        let mut x = request.x + ui.px(style.offset.0);
        let mut y = request.y + ui.px(style.offset.1);
        // Keep it inside the bounds: flip left/up when it would overflow.
        if x + w > bounds.right() {
            x = (request.x - ui.px(style.offset.0) - w).max(bounds.x);
        }
        if y + h > bounds.bottom() {
            y = (request.y - ui.px(style.offset.1) - h).max(bounds.y);
        }
        let rect = Rect::new(x, y, w, h);
        ui.painter.push_clip(bounds);
        ui.painter
            .shadow_rect(rect, ui.px(2), ui.px(3), 0x101010, 40);
        ui.painter.fill_rect(rect, style.fill);
        ui.painter
            .stroke_rect(rect, ui.scale.hairline(), style.edge);
        let inner = rect.inset(padding);
        let mut cy = inner.y;
        for (line_h, text_style, colour, text) in rows {
            ui.text.draw_fitted(
                &mut ui.painter,
                &text,
                inner,
                cy,
                text_style,
                colour,
                Align::Left,
            );
            cy += line_h;
        }
        ui.painter.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::Theme;
    use liminalkit_raster::{Painter, RasterKernel, WindowBuffer};
    use liminalkit_shell::{Input, Scale};
    use liminalkit_text::font::test_fonts::{set, style as test_style};
    use liminalkit_text::TextCache;

    #[test]
    fn a_tooltip_near_the_edge_flips_inside_the_bounds() {
        let mut buffer = WindowBuffer::new(200, 100);
        let mut text = TextCache::new(set());
        let mut kernel = RasterKernel::new();
        let mut input = Input::new();
        let mut tip = Tooltip::new();
        tip.request(
            195,
            95,
            vec![("hello world".into(), test_style(16.0), 0xffffff)],
        );
        {
            let mut ui = Ui::new(
                Painter::new(&mut buffer),
                &mut text,
                &mut input,
                Theme::default(),
                Scale::ONE,
                &mut kernel,
            );
            ui.painter.clear(0);
            let style = TooltipStyle {
                fill: 0x202020,
                edge: 0x404040,
                padding: 4,
                max_width: 150,
                offset: (10, 10),
            };
            tip.draw(&mut ui, Rect::new(0, 0, 200, 100), style);
        }
        assert!(!tip.is_pending());
        // Ink landed left of and above the anchor, not off-screen.
        let lit = buffer.pixels.iter().filter(|&&p| p != 0).count();
        assert!(lit > 0);
        assert_eq!(
            buffer.pixels[99 * 200 + 199],
            0,
            "nothing at the far corner"
        );
    }
}
