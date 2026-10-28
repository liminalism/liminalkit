//! Tiled panes and floating windows in one OS window.
//!
//! Drag any pane edge to resize it — both panes sharing the edge move. Drag a
//! floating window by its title bar, resize it by any edge or corner, close it
//! with its `x` button. Keys: `1`/`2` split the first pane right/bottom,
//! Backspace removes the last split pane, `N` opens a floating window,
//! `Q` quits.
//!
//! ```sh
//! cargo run -p liminalkit-windowing --example panes
//! ```

use liminalkit_raster::{Painter, RasterKernel, Rect, WindowBuffer};
use liminalkit_shell::{Input, KeyInput, MouseButton, Scale};
use liminalkit_text::font::test_fonts::{set, LATIN};
use liminalkit_text::{Align, TextCache, TextStyle};
use liminalkit_ui::{Theme, Ui};
use liminalkit_windowing::{
    run_app, CursorShape, PaneId, PaneMetrics, Panes, PixelApp, Side, WindowConfig, WindowMetrics,
    Windows,
};

struct Demo {
    text: TextCache,
    kernel: RasterKernel,
    input: Input,
    panes: Panes<String>,
    /// Panes created by splitting, newest last, so Backspace removes them in
    /// reverse.
    created: Vec<PaneId>,
    floats: Windows<String>,
    float_count: u32,
    area: Rect,
    cursor: CursorShape,
    quit: bool,
}

impl Demo {
    fn split_first(&mut self, side: Side) {
        let Some((first, _)) = self.panes.panes(self.area).first().copied() else {
            return;
        };
        let label = format!("pane {}", self.created.len() + 2);
        if let Some(id) = self.panes.split(first, side, label, 0.5) {
            self.created.push(id);
        }
    }

    fn open_float(&mut self) {
        self.float_count += 1;
        let step = (self.float_count as i32 * 28) % 200;
        self.floats.open(
            format!("float {}", self.float_count),
            "drag me by the title bar\nresize me by any edge".to_owned(),
            Rect::new(140 + step, 90 + step, 280, 170),
        );
    }
}

impl PixelApp for Demo {
    fn render(&mut self, buffer: &mut WindowBuffer, s: Scale) {
        let area = Rect::new(0, 0, buffer.width as i32, buffer.height as i32);
        self.area = area;
        let pane_metrics = PaneMetrics::scaled(s);
        let win_metrics = WindowMetrics::scaled(s);
        // State first, before anything draws: a grabbed divider or title bar
        // claims its press ahead of the content underneath.
        self.panes.update(&mut self.input, area, pane_metrics);
        self.floats.update(&mut self.input, area, win_metrics);
        while let Some(id) = self.floats.take_closed() {
            self.floats.close(id);
        }
        let hover = self.floats.hover_cursor(&self.input, win_metrics);
        self.cursor = if hover != CursorShape::Default {
            hover
        } else {
            self.panes.hover_cursor(&self.input, area, pane_metrics)
        };

        let mut ui = Ui::new(
            Painter::new(buffer),
            &mut self.text,
            &mut self.input,
            Theme::default(),
            s,
            &mut self.kernel,
        );
        ui.painter.clear(0x2b3238);
        let ink = 0x182027;
        let muted = 0x68727b;
        let line = 0xd7d8d4;
        let body = TextStyle::new(LATIN, s.font(13.0));

        for (id, rect) in self.panes.panes(area) {
            let label = self
                .panes
                .get(id)
                .cloned()
                .unwrap_or_else(|| "?".to_owned());
            let inner = ui.flat_panel(rect.inset(2), 0xfaf9f5, line, false);
            ui.label_styled(
                Rect::new(inner.x + s.px(10), inner.y + s.px(8), inner.w, s.px(20)),
                &label,
                body,
                ink,
                Align::Left,
            );
            ui.label_styled(
                Rect::new(inner.x + s.px(10), inner.y + s.px(30), inner.w, s.px(20)),
                "drag any edge — the neighbour follows",
                body,
                muted,
                Align::Left,
            );
        }

        let order = self.floats.order();
        let focused = self.floats.focused();
        for (id, rect) in order {
            let title = self.floats.title(id).unwrap_or_default().to_owned();
            let content = self.floats.content(id, win_metrics).unwrap_or(rect);
            let title_bar = self.floats.title_bar(id, win_metrics).unwrap_or(rect);
            let close = self.floats.close_button(id, win_metrics).unwrap_or(rect);
            let active = Some(id) == focused;
            ui.painter
                .shadow_rect(rect, ui.px(2), ui.px(3), 0x101010, 60);
            ui.painter.fill_rect(rect, 0xfaf9f5);
            ui.painter
                .fill_rect(title_bar, if active { 0x182027 } else { 0x68727b });
            ui.label_styled(
                Rect::new(
                    title_bar.x + s.px(8),
                    title_bar.y,
                    title_bar.w - title_bar.h - s.px(12),
                    title_bar.h,
                ),
                &title,
                body,
                0xffffff,
                Align::Left,
            );
            ui.painter.fill_rect(
                close,
                if ui.input.hovering(close) {
                    0xb3362b
                } else if active {
                    0x3a444c
                } else {
                    0x8a949c
                },
            );
            ui.label_styled(close, "x", body, 0xffffff, Align::Centre);
            if let Some(payload) = self.floats.get(id) {
                ui.label_styled(
                    Rect::new(
                        content.x + s.px(10),
                        content.y + s.px(8),
                        (content.w - s.px(20)).max(0),
                        s.px(40),
                    ),
                    payload,
                    body,
                    ink,
                    Align::Left,
                );
            }
        }
        ui.input.end_frame();
    }

    fn cursor_shape(&self) -> CursorShape {
        self.cursor
    }

    fn on_key(&mut self, key: &KeyInput) {
        match key {
            KeyInput::Character('q') => self.quit = true,
            KeyInput::Character('1') => self.split_first(Side::Right),
            KeyInput::Character('2') => self.split_first(Side::Bottom),
            KeyInput::Character('n') => self.open_float(),
            KeyInput::Backspace => {
                while let Some(id) = self.created.pop() {
                    if self.panes.remove(id).is_some() {
                        break;
                    }
                }
            }
            _ => {}
        }
        self.input.key(key.clone());
    }

    fn on_cursor(&mut self, x: f32, y: f32) {
        self.input.cursor_moved(x, y);
    }

    fn on_cursor_left(&mut self) {
        self.input.cursor_left();
    }

    fn on_mouse(&mut self, button: MouseButton, pressed: bool) {
        self.input.mouse(button, pressed);
    }

    fn should_exit(&self) -> bool {
        self.quit
    }
}

fn main() {
    let mut floats = Windows::new();
    floats.open(
        "inspector",
        "drag me by the title bar\nresize me by any edge".to_owned(),
        Rect::new(420, 120, 280, 170),
    );
    let demo = Demo {
        text: TextCache::new(set()),
        kernel: RasterKernel::new(),
        input: Input::new(),
        panes: Panes::new("pane 1".to_owned()),
        created: Vec::new(),
        floats,
        float_count: 1,
        area: Rect::new(0, 0, 1, 1),
        cursor: CursorShape::Default,
        quit: false,
    };
    run_app(
        demo,
        WindowConfig::new("liminalkit panes", 900.0, 600.0).min_size(480.0, 320.0),
    )
    .expect("run");
}
