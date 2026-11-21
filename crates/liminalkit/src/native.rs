//! Native controls, with a drawn stand-in.
//!
//! Everything in [`liminalkit_native`] is re-exported here. The functions
//! below are the short road for a [`Screen`](crate::Screen): each takes the
//! rectangle a drawn control would take, and either places the platform's
//! control there or draws liminalkit's own, returning the same answer either
//! way. A screen written with them is a complete liminalkit GUI on every
//! platform and gets real Win32 controls on Windows.
//!
//! ```no_run
//! use liminalkit::native::{self, ControlId, NativeControls};
//! use liminalkit::prelude::*;
//!
//! struct Editor {
//!     controls: NativeControls,
//!     save: ControlId,
//!     bold: ControlId,
//! }
//!
//! impl Screen for Editor {
//!     fn draw(&mut self, ui: &mut Ui<'_>, _ctx: &mut Context) {
//!         ui.painter.clear(ui.theme.background);
//!         let bar = hstack(ui.rect(8, 8, 200, 26), 6, &[Length::Px(88), Length::Px(32)]);
//!         if native::button(ui, &self.controls, self.save, bar[0]) {
//!             // save
//!         }
//!         if native::toggle(ui, &self.controls, self.bold, bar[1]) {
//!             let _bold = self.controls.checked(self.bold);
//!         }
//!     }
//! }
//!
//! let controls = NativeControls::new();
//! let screen = Editor {
//!     save: controls.button("Save"),
//!     bold: controls.toggle("B", false),
//!     controls: controls.clone(),
//! };
//! let app = App::new(screen, liminalkit_test_fonts::set());
//! run_app(controls.host(app), WindowConfig::new("editor", 480.0, 320.0)).expect("run");
//! ```
//!
//! Combos and text fields have no one-line drawn equivalent — liminalkit's
//! [`Dropdown`](crate::ui::Dropdown) and
//! [`TextFieldState`](crate::ui::TextFieldState) keep state the caller owns —
//! so a screen branches on [`NativeControls::is_native`] for those, reading
//! and writing the same `selected` and `text` in both branches.

pub use liminalkit_native::*;

use crate::raster::Rect;
use crate::text::Align;
use crate::ui::Ui;

/// A push button at `area`. Returns whether it was clicked.
pub fn button(ui: &mut Ui<'_>, controls: &NativeControls, id: ControlId, area: Rect) -> bool {
    if controls.is_native() {
        controls.place(id, area);
        controls.clicked(id)
    } else {
        ui.button(area, &controls.text(id))
    }
}

/// A button that stays pressed while on. Returns whether the user flipped
/// it; the state is [`NativeControls::checked`], which the drawn stand-in
/// keeps up to date too.
pub fn toggle(ui: &mut Ui<'_>, controls: &NativeControls, id: ControlId, area: Rect) -> bool {
    if controls.is_native() {
        controls.place(id, area);
        controls.changed(id)
    } else {
        let on = controls.checked(id);
        let pressed = ui.button_focused(area, &controls.text(id), on);
        if pressed {
            controls.set_checked(id, !on);
        }
        pressed
    }
}

/// One line of static text at `area`, in `colour` when drawn.
pub fn label(ui: &mut Ui<'_>, controls: &NativeControls, id: ControlId, area: Rect, colour: u32) {
    if controls.is_native() {
        controls.place(id, area);
    } else {
        ui.label(area, &controls.text(id), colour, Align::Left);
    }
}
