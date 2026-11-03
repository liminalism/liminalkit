# Tutorial: from hello to an inventory screen

This grows `crates/liminalkit/examples/hello.rs` into `crates/liminalkit/examples/tutorial.rs`: a filterable table with a dropdown, a checkbox, and a confirmation dialog. Each step is a few lines. Run the finished screen with:

```sh
cargo run -p liminalkit --example tutorial
```

## Step 0: hello

```rust
use liminalkit::prelude::*;

struct Hello { clicks: u32 }

impl Screen for Hello {
    fn draw(&mut self, ui: &mut Ui<'_>, ctx: &mut Context) {
        ui.painter.clear(ui.theme.background);
        let rows = vstack(ui.rect(16, 16, 240, 80), 8, &[Length::Px(28), Length::Px(36)]);
        ui.heading(rows[0], &format!("{} clicks", self.clicks));
        if ui.button(rows[1], "Click me") { self.clicks += 1; }
        if ctx.key_pressed(KeyInput::Escape) { ctx.exit = true; }
    }
}

fn main() {
    // The examples draw with the workspace's Noto fixtures; an application
    // builds a `FontSet` from faces it embeds itself (see the README).
    liminalkit::run(Hello { clicks: 0 }, liminalkit_test_fonts::set(),
        WindowConfig::new("hello", 320.0, 200.0)).expect("run");
}
```

Three things to notice. The screen is a struct and its state is a field. A control takes a rectangle and returns what happened. Rectangles are device pixels, so layout is written in logical pixels and converted once with `ui.rect`, `ui.px`, or `Insets::scaled`.

## Step 1: the screen is a struct

Replace `Hello` with the state an inventory screen needs. Every control's state is a field the screen owns; the toolkit stores nothing.

```rust
struct Inventory {
    items: Vec<Item>,
    table: TableState,        // scroll offset and the selected row's key
    filter: TextFieldState,   // caret, selection, text
    show_empty: bool,
    unit: usize,              // which dropdown option is selected
    units: Dropdown,          // whether the popup is open
    confirm: Overlay,         // whether the dialog is open
    status: String,
}
```

## Step 2: layout is a stack inside a panel

`Ui::panel` draws a surface and returns the rectangle inside its padding. `vstack` divides it by a list of lengths; `ui.metrics()` gives the theme's padding and row height in device pixels.

```rust
let bounds = ui.painter.bounds();
let panel = ui.panel(Insets::uniform(16).scaled(ui.scale).apply(bounds));
let (_, row_h) = ui.metrics();
let rows = vstack(panel, ui.px(10),
    &[Length::Px(row_h), Length::Px(row_h), Length::FILL, Length::Px(row_h)]);
```

`rows[0]` is the toolbar, `rows[1]` a count line, `rows[2]` takes whatever is left for the table, `rows[3]` is the status line.

## Step 3: a toolbar of controls

`hstack` splits the toolbar row. Each control takes `&mut` to its state and answers a question.

```rust
let toolbar = hstack(rows[0], ui.px(10),
    &[Length::FILL, Length::Px(ui.px(150)), Length::Px(ui.px(120)), Length::Px(ui.px(110))]);
ui.text_field(&mut self.filter, toolbar[0], "filter by code or name", false, false);
ui.checkbox(toolbar[1], &mut self.show_empty, "show empty");
ui.dropdown(&mut self.units, toolbar[2], &UNITS, self.unit);
if ui.button(toolbar[3], "Remove…") && self.table.selected().is_some() {
    self.confirm.open_dialog((300, 120));
}
ctx.ime_caret = self.filter.caret_area();
```

The text field's `focused` argument is `false` here because `App` attached Tab traversal to the `Ui`: the standard controls register themselves and the field is focused by a click or by Tab. A screen that wants to name its fields uses `Focus<Field>` and the `_focused` control variants instead. The last line tells the host where to put an input method's candidate window.

## Step 4: a table keyed by the data

The table's selection is a key, not an index, so it follows the row through filtering and sorting. The row closure is called only for the rows on screen.

```rust
let columns = [
    Column::fixed("code", 90, Align::Left),
    Column::flex("name", 3, Align::Left),
    Column::flex("quantity", 1, Align::Right),
];
let keys: Vec<String> = shown.iter().map(|&i| self.items[i].code.clone()).collect();
let outcome = ui.table(&mut self.table, rows[2], &columns, &keys, |row| {
    let item = &items[shown[row]];
    Row { cells: vec![
        Cell::new(item.code.as_str()),
        Cell::new(item.name.as_str()),
        Cell::new(format!("{} {unit}", item.quantity)),
    ] }
});
if let Some(code) = outcome.activated { self.status = format!("selected {code}"); }
```

`shown` is the filtered list of item indices, computed from `self.filter.text()` and `self.show_empty` each frame. Nothing is cached, and nothing needs invalidating.

## Step 5: popups claim input first and draw last

Immediate mode has no z-order. A dropdown's list or a dialog is drawn after everything else, and because the background draws first, the popup claims its clicks and keys before the background sees them. Under `App` that is `before_frame`:

```rust
fn before_frame(&mut self, input: &mut Input, bounds: Rect, ctx: &Context) {
    let row_h = ctx.scale.px(Theme::default().row_height);
    self.units.handle_input(input, bounds, row_h);
    self.confirm.handle_input(input, bounds, ctx.scale);
}
```

and at the end of `draw`:

```rust
if let Some(i) = self.units.draw(ui, bounds) { self.unit = i; }
let mut decision = None;
self.confirm.draw(ui, bounds, "Remove the selected item?", |ui, inner, focus| {
    let rows = vstack(inner, ui.px(8), &[Length::FILL, Length::Px(ui.px(30))]);
    ui.label(rows[0], selected.as_deref().unwrap_or(""), ui.theme.text, Align::Left);
    let buttons = hstack(rows[1], ui.px(8),
        &[Length::FILL, Length::Px(ui.px(90)), Length::Px(ui.px(90))]);
    let (cancel, remove) = (buttons[1], buttons[2]);
    if ui.button_focused(cancel, "Cancel", focus.register_at(ui.input, cancel)) { decision = Some(false); }
    if ui.button_focused(remove, "Remove", focus.register_at(ui.input, remove)) { decision = Some(true); }
});
```

The dialog hands its content closure its own `Focus`, so Tab moves between Cancel and Remove and nowhere else. Escape dismisses it; a click outside is swallowed. The screen decides what the answer means: here it removes the item from `self.items` and closes the dialog.

## Where to go next

- `crates/liminalkit/examples/window.rs` adds typed focus, a multi-line field, a context menu and the frame timer.
- `crates/liminalkit/examples/window_lowlevel.rs` is the same kind of screen over `PixelApp`, owning every resource by hand.
- `docs/ARCHITECTURE.md` is the map of crates, the frame and where state lives.
