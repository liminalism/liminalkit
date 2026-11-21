//! A liminalkit screen whose toolbar is made of the platform's own controls.
//!
//! ```sh
//! cargo run -p liminalkit-native --example native_controls
//! cargo run -p liminalkit-native --example native_controls -- --drawn
//! ```
//!
//! On Windows the toolbar is real Win32 buttons, a combo box and a text
//! field; `--drawn` (and every other platform) draws the same toolbar with
//! liminalkit. The page below it is drawn by liminalkit either way. F2 swaps
//! the toolbar for a drawn prompt, which hides the native controls simply by
//! not placing them that frame. Escape quits.

use liminalkit::native::{self, ControlId, NativeControls, NativeEvent};
use liminalkit::prelude::*;

struct Demo {
    controls: NativeControls,
    save: ControlId,
    bold: ControlId,
    italic: ControlId,
    style: ControlId,
    find: ControlId,
    status: ControlId,
    saves: u32,
    prompt: bool,
    log: Vec<String>,
}

const STYLES: [&str; 3] = ["Body", "Subhead", "Block quote"];

impl Demo {
    fn note(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > 8 {
            self.log.remove(0);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui<'_>) {
        let bar = hstack(
            ui.rect(8, 8, 624, 26),
            6,
            &[
                Length::Px(ui.px(88)),
                Length::Px(ui.px(32)),
                Length::Px(ui.px(32)),
                Length::Px(ui.px(130)),
                Length::Px(ui.px(180)),
                Length::Flex(1.0),
            ],
        );
        if native::button(ui, &self.controls, self.save, bar[0]) {
            self.saves += 1;
            self.note(format!("Save clicked ({} so far)", self.saves));
        }
        for (id, rect, name) in [(self.bold, bar[1], "Bold"), (self.italic, bar[2], "Italic")] {
            if native::toggle(ui, &self.controls, id, rect) {
                let state = if self.controls.checked(id) {
                    "on"
                } else {
                    "off"
                };
                self.note(format!("{name} {state}"));
            }
        }
        if self.controls.is_native() {
            // A combo and a text field have caller-owned state when drawn,
            // so these two are placed natively and summarised when not.
            self.controls.place(self.style, bar[3]);
            self.controls.place(self.find, bar[4]);
        } else {
            let style = self.controls.selected(self.style).unwrap_or(0);
            let muted = ui.theme.text_dim;
            ui.label(bar[3], STYLES[style], muted, Align::Left);
            ui.label(bar[4], "(find: native only)", muted, Align::Left);
        }
        let text = ui.theme.text;
        native::label(ui, &self.controls, self.status, bar[5], text);
    }
}

impl Screen for Demo {
    fn draw(&mut self, ui: &mut Ui<'_>, ctx: &mut Context) {
        ui.painter.clear(ui.theme.background);
        if ctx.key_pressed(KeyInput::Escape) {
            ctx.exit = true;
        }
        if ctx.key_pressed(KeyInput::Function(2)) {
            self.prompt = !self.prompt;
        }

        if self.prompt {
            let text = ui.theme.text;
            ui.label(
                ui.rect(8, 8, 624, 26),
                "A drawn prompt owns the toolbar strip. F2 brings the controls back.",
                text,
                Align::Left,
            );
        } else {
            self.toolbar(ui);
        }

        // Whatever the toolbar did not already answer.
        while let Some(event) = self.controls.poll_event() {
            match event {
                NativeEvent::Selected(_, Some(index)) => {
                    self.note(format!("Style: {}", STYLES[index.min(STYLES.len() - 1)]));
                }
                NativeEvent::Submitted(id) if id == self.find => {
                    let query = self.controls.text(id);
                    self.note(format!("Find submitted: {query:?}"));
                }
                NativeEvent::Cancelled(_) => self.note("Find cancelled".to_owned()),
                _ => {}
            }
        }
        let kind = if self.controls.is_native() {
            "native"
        } else {
            "drawn"
        };
        self.controls.set_text(
            self.status,
            &format!("{kind} controls, {} saves", self.saves),
        );

        let page = ui.panel(ui.rect(8, 44, 624, 268));
        let rows = vstack(page, 2, &[Length::Px(ui.px(28)); 9]);
        ui.heading(rows[0], "The page is drawn by liminalkit");
        let text = ui.theme.text;
        for (row, line) in rows[1..].iter().zip(&self.log) {
            ui.label(*row, line, text, Align::Left);
        }
    }
}

fn main() {
    let drawn = std::env::args().any(|arg| arg == "--drawn");
    let controls = if drawn {
        NativeControls::drawn()
    } else {
        NativeControls::new()
    };
    let screen = Demo {
        save: controls.button("Save"),
        bold: controls.toggle("B", false),
        italic: controls.toggle("I", false),
        style: controls.combo(&STYLES, Some(0)),
        find: controls.edit(""),
        status: controls.label(""),
        controls: controls.clone(),
        saves: 0,
        prompt: false,
        log: vec!["Use the toolbar. F2 swaps it for a prompt; Escape quits.".to_owned()],
    };
    let theme = Theme::default();
    controls.set_backdrop(Some((theme.background, theme.text)));
    let app = App::new(screen, liminalkit_test_fonts::set()).theme(theme);
    run_app(
        controls.host(app),
        WindowConfig::new("liminalkit native controls", 640.0, 320.0),
    )
    .expect("run");
}
