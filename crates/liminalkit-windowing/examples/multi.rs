//! Two OS windows sharing one event loop.
//!
//! Click a window to bump its counter. `N` opens another window at runtime,
//! `X` closes just that window, `Q` quits everything.
//!
//! ```sh
//! cargo run -p liminalkit-windowing --example multi
//! ```

use std::collections::VecDeque;

use liminalkit_raster::{Painter, RasterKernel, Rect, WindowBuffer};
use liminalkit_shell::{Input, KeyInput, MouseButton, Scale};
use liminalkit_text::font::test_fonts::{set, LATIN};
use liminalkit_text::{Align, TextCache, TextStyle};
use liminalkit_ui::{Theme, Ui};
use liminalkit_windowing::{run_windows, PixelApp, WindowConfig, WindowRequest};

struct Counter {
    label: String,
    count: u32,
    text: TextCache,
    kernel: RasterKernel,
    input: Input,
    pending: VecDeque<WindowRequest>,
    quit: bool,
}

impl Counter {
    fn new(label: &str) -> Counter {
        Counter {
            label: label.to_owned(),
            count: 0,
            text: TextCache::new(set()),
            kernel: RasterKernel::new(),
            input: Input::new(),
            pending: VecDeque::new(),
            quit: false,
        }
    }
}

impl PixelApp for Counter {
    fn render(&mut self, buffer: &mut WindowBuffer, s: Scale) {
        let area = Rect::new(0, 0, buffer.width as i32, buffer.height as i32);
        let mut ui = Ui::new(
            Painter::new(buffer),
            &mut self.text,
            &mut self.input,
            Theme::default(),
            s,
            &mut self.kernel,
        );
        ui.painter.clear(0xf4f3ef);
        let body = TextStyle::new(LATIN, s.font(16.0));
        let mono = TextStyle::new(LATIN, s.font(12.0));
        if ui.input.take_click(area) {
            self.count += 1;
        }
        ui.label_styled(
            Rect::new(area.x, area.y + s.px(60), area.w, s.px(30)),
            &format!("{} — {}", self.label, self.count),
            body,
            0x182027,
            Align::Centre,
        );
        ui.label_styled(
            Rect::new(area.x, area.y + s.px(100), area.w, s.px(20)),
            "click: +1   n: new window   x: close this   q: quit all",
            mono,
            0x68727b,
            Align::Centre,
        );
        ui.input.end_frame();
    }

    fn poll_window(&mut self) -> Option<WindowRequest> {
        self.pending.pop_front()
    }

    fn on_key(&mut self, key: &KeyInput) {
        match key {
            KeyInput::Character('q') => self.quit = true,
            KeyInput::Character('n') => self.pending.push_back(WindowRequest::Open {
                config: WindowConfig::new("liminalkit counter — extra", 360.0, 240.0),
                app: Box::new(Counter::new("extra")),
            }),
            KeyInput::Character('x') => self.pending.push_back(WindowRequest::Close),
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
    run_windows(vec![
        (
            WindowConfig::new("liminalkit counter — left", 360.0, 240.0),
            Box::new(Counter::new("left")) as Box<dyn PixelApp>,
        ),
        (
            WindowConfig::new("liminalkit counter — right", 360.0, 240.0),
            Box::new(Counter::new("right")) as Box<dyn PixelApp>,
        ),
    ])
    .expect("run");
}
