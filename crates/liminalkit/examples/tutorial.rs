//! The tutorial's finished screen: `docs/TUTORIAL.md` grows `hello.rs`
//! into this, one step at a time.
//!
//! ```sh
//! cargo run -p liminalkit --features test-fonts --example tutorial
//! ```

use liminalkit::prelude::*;
use liminalkit::ui::{Cell, Column, Row};

/// Step 1: the screen is a struct. Every piece of state is a field.
struct Inventory {
    items: Vec<Item>,
    table: TableState,
    filter: TextFieldState,
    show_empty: bool,
    unit: usize,
    units: Dropdown,
    confirm: Overlay,
    status: String,
}

struct Item {
    code: String,
    name: String,
    quantity: u32,
}

const UNITS: [&str; 3] = ["each", "kg", "litres"];

impl Screen for Inventory {
    // Step 5: an open popup claims its input before anything draws.
    fn before_frame(&mut self, input: &mut Input, bounds: Rect, ctx: &Context) {
        let row_h = ctx.scale.px(Theme::default().row_height);
        self.units.handle_input(input, bounds, row_h);
        self.confirm.handle_input(input, bounds, ctx.scale);
    }

    fn draw(&mut self, ui: &mut Ui<'_>, ctx: &mut Context) {
        ui.painter.clear(ui.theme.background);
        if ctx.key_pressed(KeyInput::Escape) && !self.confirm.is_open() {
            ctx.exit = true;
        }

        // Step 2: layout is a stack inside a panel, in logical pixels,
        // converted once.
        let bounds = ui.painter.bounds();
        let panel = ui.panel(Insets::uniform(16).scaled(ui.scale).apply(bounds));
        let (_, row_h) = ui.metrics();
        let rows = vstack(
            panel,
            ui.px(10),
            &[
                Length::Px(row_h),
                Length::Px(row_h),
                Length::FILL,
                Length::Px(row_h),
            ],
        );

        // Step 3: a toolbar row of controls, each taking `&mut` to its state.
        let toolbar = hstack(
            rows[0],
            ui.px(10),
            &[
                Length::FILL,
                Length::Px(ui.px(150)),
                Length::Px(ui.px(120)),
                Length::Px(ui.px(110)),
            ],
        );
        ui.text_field(
            &mut self.filter,
            toolbar[0],
            "filter by code or name",
            false,
            false,
        );
        ui.checkbox(toolbar[1], &mut self.show_empty, "show empty");
        ui.dropdown(&mut self.units, toolbar[2], &UNITS, self.unit);
        if ui.button(toolbar[3], "Remove…") && self.table.selected().is_some() {
            self.confirm.open_dialog((300, 120));
        }
        ctx.ime_caret = self.filter.caret_area();

        let filter = self.filter.text().to_lowercase();
        let shown: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| self.show_empty || item.quantity > 0)
            .filter(|(_, item)| {
                filter.is_empty()
                    || item.code.to_lowercase().contains(&filter)
                    || item.name.to_lowercase().contains(&filter)
            })
            .map(|(i, _)| i)
            .collect();
        ui.label(
            rows[1],
            &format!("{} of {} items", shown.len(), self.items.len()),
            ui.theme.text_dim,
            Align::Left,
        );

        // Step 4: a table keyed by the item's code, so the selection follows
        // the row through filtering.
        let columns = [
            Column::fixed("code", 90, Align::Left),
            Column::flex("name", 3, Align::Left),
            Column::flex("quantity", 1, Align::Right),
        ];
        let keys: Vec<String> = shown.iter().map(|&i| self.items[i].code.clone()).collect();
        let items = &self.items;
        let unit = UNITS[self.unit];
        let outcome = ui.table(&mut self.table, rows[2], &columns, &keys, |row| {
            let item = &items[shown[row]];
            Row {
                cells: vec![
                    Cell::new(item.code.as_str()),
                    Cell::new(item.name.as_str()),
                    Cell::new(format!("{} {unit}", item.quantity)),
                ],
            }
        });
        if let Some(code) = outcome.activated {
            self.status = format!("selected {code}");
        }
        ui.label(rows[3], &self.status, ui.theme.text_dim, Align::Left);

        // Step 5, continued: popups draw last.
        if let Some(i) = self.units.draw(ui, bounds) {
            self.unit = i;
        }
        let selected = self.table.selected().map(str::to_owned);
        let mut decision = None;
        self.confirm.draw(
            ui,
            bounds,
            "Remove the selected item?",
            |ui, inner, focus| {
                let rows = vstack(inner, ui.px(8), &[Length::FILL, Length::Px(ui.px(30))]);
                ui.label(
                    rows[0],
                    selected.as_deref().unwrap_or(""),
                    ui.theme.text,
                    Align::Left,
                );
                let buttons = hstack(
                    rows[1],
                    ui.px(8),
                    &[Length::FILL, Length::Px(ui.px(90)), Length::Px(ui.px(90))],
                );
                let (cancel, remove) = (buttons[1], buttons[2]);
                if ui.button_focused(cancel, "Cancel", focus.register_at(ui.input, cancel)) {
                    decision = Some(false);
                }
                if ui.button_focused(remove, "Remove", focus.register_at(ui.input, remove)) {
                    decision = Some(true);
                }
            },
        );
        if let Some(remove) = decision {
            if remove {
                if let Some(code) = selected {
                    self.items.retain(|item| item.code != code);
                    self.status = format!("removed {code}");
                }
            }
            self.confirm.close();
        }
    }

    fn on_text_commit(&mut self, text: &str) {
        self.filter.insert_str(text);
    }
}

fn main() {
    let items = [
        ("FLR-01", "Flour, strong white", 12),
        ("FLR-02", "Flour, wholemeal", 0),
        ("SUG-01", "Sugar, caster", 7),
        ("BUT-01", "Butter, unsalted", 3),
        ("EGG-01", "Eggs, free range", 0),
        ("MLK-01", "Milk, whole", 9),
    ]
    .into_iter()
    .map(|(code, name, quantity)| Item {
        code: code.to_owned(),
        name: name.to_owned(),
        quantity,
    })
    .collect();
    liminalkit::run(
        Inventory {
            items,
            table: TableState::new(),
            filter: TextFieldState::new(),
            show_empty: false,
            unit: 0,
            units: Dropdown::new(),
            confirm: Overlay::new(),
            status: String::new(),
        },
        liminalkit::text::font::test_fonts::set(),
        WindowConfig::new("liminalkit tutorial", 720.0, 480.0),
    )
    .expect("run");
}
