//! A live window: the gallery's controls, interactive, with the frame timer.
//!
//! ```sh
//! LIMINALKIT_FRAME_LOG=1 cargo run -p liminalkit-ui --example window --features test-fonts
//! ```

use liminalkit_raster::{Painter, RasterKernel, Rect, WindowBuffer};
use liminalkit_shell::{Input, KeyInput, MouseButton, Scale};
use liminalkit_text::font::test_fonts::{set, LATIN};
use liminalkit_text::{Align, TextCache, TextStyle};
use liminalkit_ui::{ButtonStyle, ScrollState, Theme, Ui};
use liminalkit_windowing::{run_app, PixelApp, WindowConfig};

struct Demo {
    text: TextCache,
    kernel: RasterKernel,
    input: Input,
    tabs: usize,
    speed: usize,
    quantity: i32,
    scroll: ScrollState,
    quit: bool,
    frames: u64,
}

impl PixelApp for Demo {
    fn render(&mut self, buffer: &mut WindowBuffer, s: Scale) {
        self.frames += 1;
        let mut ui = Ui::new(
            Painter::new(buffer),
            &mut self.text,
            &mut self.input,
            Theme::default(),
            s,
            &mut self.kernel,
        );
        ui.painter.clear(0xf4f3ef);
        let ink = 0x182027;
        let muted = 0x68727b;
        let line = 0xd7d8d4;
        let mono = TextStyle::tracked(LATIN, s.font(10.0), 0.08);
        let body = TextStyle::new(LATIN, s.font(13.0));
        let buttons = ButtonStyle {
            text: muted,
            text_active: 0xffffff,
            fill: 0xfaf9f5,
            fill_hover: 0xecebe6,
            fill_active: ink,
            edge: line,
            edge_active: ink,
            radius: 0,
        };
        let w = ui.painter.width();
        let panel = ui.flat_panel(
            Rect::new(
                s.px(16),
                s.px(16),
                w - s.px(32),
                ui.painter.height() - s.px(32),
            ),
            0xfaf9f5,
            line,
            true,
        );
        ui.underline_tabs(
            &mut self.tabs,
            Rect::new(panel.x, panel.y, s.px(360), s.px(48)),
            &["CAUSAL FIELD", "REPLAY", "JOURNAL"],
            mono,
            ink,
            muted,
            ink,
            3,
        );
        if let Some(i) = ui.segmented(
            Rect::new(panel.x + s.px(380), panel.y + s.px(11), s.px(220), s.px(26)),
            &["¼×", "½×", "1×", "2×", "4×"],
            self.speed,
            mono,
            buttons,
            4,
        ) {
            self.speed = i;
        }
        let d = ui.stepper(
            Rect::new(panel.x + s.px(12), panel.y + s.px(70), s.px(160), s.px(40)),
            &self.quantity.to_string(),
            "CONTRACTS",
            body,
            mono,
            buttons,
            ink,
            muted,
        );
        self.quantity = (self.quantity + d).clamp(1, 5);
        let list = Rect::new(
            panel.x + s.px(200),
            panel.y + s.px(70),
            (panel.w - s.px(212)).max(50),
            (panel.h - s.px(82)).max(50),
        );
        ui.flat_panel(list, 0xffffff, line, false);
        let heights = vec![s.px(22); 200];
        ui.scroll_list(
            &mut self.scroll,
            list.inset(s.px(4)),
            &heights,
            |ui, i, rect| {
                if ui.input.hovering(rect) {
                    ui.painter.fill_rect(rect, 0xedf0ef);
                }
                ui.label_styled(
                    rect.inset(s.px(3)),
                    &format!(
                        "row {i:03}  tab {} speed {} — hover me",
                        self.tabs, self.speed
                    ),
                    body,
                    ink,
                    Align::Left,
                );
            },
        );
        ui.label_styled(
            Rect::new(
                panel.x + s.px(12),
                panel.bottom() - s.px(30),
                s.px(180),
                s.px(20),
            ),
            &format!("frame {}", self.frames),
            mono,
            muted,
            Align::Left,
        );
        ui.input.end_frame();
    }
    fn on_key(&mut self, key: &KeyInput) {
        if let KeyInput::Character('q') = key {
            self.quit = true;
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
    fn on_scroll(&mut self, dx: f32, dy: f32) {
        self.input.scrolled(dx, dy);
    }
    fn should_exit(&self) -> bool {
        self.quit
    }
}

fn main() {
    let demo = Demo {
        text: TextCache::new(set()),
        kernel: RasterKernel::new(),
        input: Input::new(),
        tabs: 0,
        speed: 2,
        quantity: 1,
        scroll: ScrollState::new(),
        quit: false,
        frames: 0,
    };
    run_app(
        demo,
        WindowConfig::new("liminalkit window", 900.0, 600.0).min_size(480.0, 320.0),
    )
    .expect("run");
}
