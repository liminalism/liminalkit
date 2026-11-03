//! The smallest liminalkit application: a heading, a button, Escape to quit.
//!
//! ```sh
//! cargo run -p liminalkit --example hello
//! ```

use liminalkit::prelude::*;

struct Hello {
    clicks: u32,
}

impl Screen for Hello {
    fn draw(&mut self, ui: &mut Ui<'_>, ctx: &mut Context) {
        ui.painter.clear(ui.theme.background);
        // Layout in logical pixels, converted once; the controls take device pixels.
        let rows = vstack(
            ui.rect(16, 16, 240, 80),
            8,
            &[Length::Px(28), Length::Px(36)],
        );
        ui.heading(rows[0], &format!("{} clicks", self.clicks));
        if ui.button(rows[1], "Click me") {
            self.clicks += 1;
        }
        if ctx.key_pressed(KeyInput::Escape) {
            ctx.exit = true;
        }
    }
}

fn main() {
    liminalkit::run(
        Hello { clicks: 0 },
        liminalkit_test_fonts::set(),
        WindowConfig::new("liminalkit hello", 320.0, 200.0),
    )
    .expect("run");
}
