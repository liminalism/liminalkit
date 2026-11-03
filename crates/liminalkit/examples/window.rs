//! A live window: the gallery's controls, interactive, through the `App`
//! adapter and the stack layout, with the frame timer.
//!
//! The same screen written directly against `PixelApp`, with every resource
//! owned by hand, is `window_lowlevel.rs`.
//!
//! ```sh
//! LIMINALKIT_FRAME_LOG=1 cargo run -p liminalkit --features test-fonts --example window
//! ```

use liminalkit::prelude::*;
use liminalkit::text::font::test_fonts::{set, LATIN};
use liminalkit::ui::{ButtonStyle, Dropdown};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Email,
    Notes,
}

struct Demo {
    tabs: usize,
    speed: usize,
    quantity: i32,
    scroll: ScrollState,
    name: TextFieldState,
    email: TextFieldState,
    notes: TextFieldState,
    /// Keyboard focus by name: the Email field can be hidden on the REPLAY
    /// tab and reappear without focus shifting to a neighbour.
    focus: Focus<Field>,
    dropdown: Dropdown,
    unit: usize,
    frames: u64,
}

impl Screen for Demo {
    fn draw(&mut self, ui: &mut Ui<'_>, ctx: &mut Context) {
        self.frames += 1;
        if ctx.key_pressed(KeyInput::Character('q')) {
            ctx.exit = true;
        }
        // An open popup owns input before anything underneath it draws.
        let (_, row_h) = ui.metrics();
        self.dropdown
            .handle_input(ui.input, ui.painter.bounds(), row_h);
        ui.painter.clear(0xf4f3ef);
        let ink = 0x182027;
        let muted = 0x68727b;
        let line = 0xd7d8d4;
        let mono = ui.text_style(TextStyle::tracked(LATIN, 10.0, 0.08));
        let body = ui.text_style(TextStyle::new(LATIN, 13.0));
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

        // The window, minus a margin, is the panel. Everything below is a
        // stack inside it: a header row, the content, a status line.
        let outer = Insets::uniform(16)
            .scaled(ui.scale)
            .apply(ui.painter.bounds());
        let panel = ui.flat_panel(outer, 0xfaf9f5, line, true);
        let gap = ui.px(12);
        let rows = vstack(
            panel,
            gap,
            &[Length::Px(ui.px(48)), Length::FILL, Length::Px(ui.px(20))],
        );
        let (header, content, status) = (rows[0], rows[1], rows[2]);

        // Header: tabs on the left, the speed control on the right.
        let header = hstack(
            header,
            gap,
            &[Length::Px(ui.px(360)), Length::FILL, Length::Px(ui.px(220))],
        );
        ui.underline_tabs(
            &mut self.tabs,
            header[0],
            &["CAUSAL FIELD", "REPLAY", "JOURNAL"],
            mono,
            ink,
            muted,
            ink,
            3,
        );
        let speed = centre(header[2], (header[2].w, ui.px(26)));
        if let Some(i) = ui.segmented(
            speed,
            &["¼×", "½×", "1×", "2×", "4×"],
            self.speed,
            mono,
            buttons,
            4,
        ) {
            self.speed = i;
        }

        // Content: a form column and the list beside it.
        let columns = hstack(content, gap, &[Length::Px(ui.px(220)), Length::FILL]);
        let mut form = Flow::down(columns[0], ui.px(10));
        let d = ui.stepper(
            form.take(ui.px(40)),
            &self.quantity.to_string(),
            "CONTRACTS",
            body,
            mono,
            buttons,
            ink,
            muted,
        );
        self.quantity = (self.quantity + d).clamp(1, 5);

        // Typed focus: the screen registers each field by name, in draw
        // order, and passes the answer to the control.
        self.focus.handle_tab(ui.input.keys(), ui.input.modifiers());
        let field_h = ui.px(ui.theme.row_height);
        let name = form.take(field_h);
        let focused = self.focus.register_id_at(Field::Name, ui.input, name);
        ui.text_field(&mut self.name, name, "name", focused, false);
        if self.tabs != 1 {
            let email = form.take(field_h);
            let focused = self.focus.register_id_at(Field::Email, ui.input, email);
            ui.text_field(&mut self.email, email, "email", focused, false);
        }
        let notes = form.take(field_h);
        let focused = self.focus.register_id_at(Field::Notes, ui.input, notes);
        ui.text_field(&mut self.notes, notes, "notes", focused, false);
        ctx.ime_caret = [&self.name, &self.email, &self.notes]
            .iter()
            .find_map(|field| field.caret_area());

        let units = ["metres", "feet", "cubits"];
        ui.dropdown(&mut self.dropdown, form.take(field_h), &units, self.unit);

        let list = columns[1];
        ui.flat_panel(list, 0xffffff, line, false);
        let heights = vec![ui.px(22); 200];
        let tabs = self.tabs;
        let speed = self.speed;
        ui.scroll_list(
            &mut self.scroll,
            list.inset(ui.px(4)),
            &heights,
            |ui, i, rect| {
                if ui.input.hovering(rect) {
                    ui.painter.fill_rect(rect, 0xedf0ef);
                }
                ui.label_styled(
                    rect.inset(ui.px(3)),
                    &format!("row {i:03}  tab {tabs} speed {speed} — hover me"),
                    body,
                    ink,
                    Align::Left,
                );
            },
        );

        ui.label_styled(
            status,
            &format!(
                "frame {}   focus {:?}   q quits",
                self.frames,
                self.focus.current()
            ),
            mono,
            muted,
            Align::Left,
        );

        // Popups draw last so they sit above everything.
        if let Some(i) = self.dropdown.draw(ui, ui.painter.bounds()) {
            self.unit = i;
        }
    }

    fn on_text_commit(&mut self, text: &str) {
        let field = match self.focus.current() {
            Some(Field::Name) => &mut self.name,
            Some(Field::Email) => &mut self.email,
            Some(Field::Notes) => &mut self.notes,
            None => return,
        };
        field.insert_str(text);
    }
}

fn main() {
    let demo = Demo {
        tabs: 0,
        speed: 2,
        quantity: 1,
        scroll: ScrollState::new(),
        name: TextFieldState::new(),
        email: TextFieldState::new(),
        notes: TextFieldState::new(),
        focus: Focus::new(),
        dropdown: Dropdown::default(),
        unit: 0,
        frames: 0,
    };
    App::new(demo, set())
        .theme(Theme {
            background: 0xf4f3ef,
            panel: 0xfaf9f5,
            panel_edge: 0xd7d8d4,
            text: 0x182027,
            text_dim: 0x68727b,
            hover: 0xecebe6,
            selection: 0xe3e6e2,
            accent: 0x182027,
            body: TextStyle::new(LATIN, 13.0),
            heading: TextStyle::new(LATIN, 18.0),
            ..Theme::default()
        })
        .run(WindowConfig::new("liminalkit window", 900.0, 600.0).min_size(480.0, 320.0))
        .expect("run");
}
