//! liminalkit: an explicit-state, software-rendered GUI toolkit.
//!
//! The application owns its state, its layout and its pixels. The toolkit
//! supplies drawing and interaction primitives: there is no retained widget
//! tree, no widget identity map, and no mandatory GPU stack. The same
//! application runs in a desktop window or directly on a Linux DRM/KMS
//! display.
//!
//! This crate is the front door. It re-exports the six component crates
//! under short names and adds [`app`], the short road from a type that can
//! draw a screen to a running window:
//!
//! ```no_run
//! use liminalkit::prelude::*;
//!
//! struct Hello {
//!     clicks: u32,
//! }
//!
//! impl Screen for Hello {
//!     fn draw(&mut self, ui: &mut Ui<'_>, ctx: &mut Context) {
//!         ui.painter.clear(ui.theme.background);
//!         let [title, button] = *vstack(
//!             ui.rect(16, 16, 240, 80),
//!             8,
//!             &[Length::Px(28), Length::Px(36)],
//!         )
//!         .as_slice() else {
//!             return;
//!         };
//!         ui.heading(title, &format!("{} clicks", self.clicks));
//!         if ui.button(button, "Click me") {
//!             self.clicks += 1;
//!         }
//!         if ctx.key_pressed(KeyInput::Escape) {
//!             ctx.exit = true;
//!         }
//!     }
//! }
//!
//! fn main() {
//!     liminalkit::run(
//!         Hello { clicks: 0 },
//!         liminalkit_test_fonts::set(),
//!         WindowConfig::new("hello", 320.0, 200.0),
//!     )
//!     .expect("run");
//! }
//! ```
//!
//! Everything [`app::App`] does for a [`Screen`] is a few lines over the
//! [`windowing::PixelApp`] trait, which stays available for applications
//! that want to own their text cache, input and frame lifecycle directly.
//!
//! # Units
//!
//! Every rectangle a control takes is in device pixels, the pixels the
//! painter writes and the pointer reports. Layout is written in logical
//! pixels and converted once: [`ui::Ui::px`] for a length,
//! [`ui::Ui::rect`] for a rectangle, [`shell::Scale::rect`] for a whole
//! logical layout. [`ui::Theme`]'s sizes are logical; its font styles are
//! converted by [`ui::Ui::new`].

pub use liminalkit_raster as raster;
pub use liminalkit_shell as shell;
pub use liminalkit_text as text;
pub use liminalkit_ui as ui;
pub use liminalkit_windowing as windowing;

#[cfg(feature = "kms")]
pub use liminalkit_kms as kms;

pub mod app;

pub use app::{run, App, Context, Screen};

/// The names an ordinary screen uses, in one import.
pub mod prelude {
    pub use crate::app::{run, App, Context, Screen};
    pub use crate::raster::{Painter, RasterKernel, Rect, WindowBuffer};
    pub use crate::shell::{Input, KeyInput, Modifiers, MouseButton, Scale};
    pub use crate::text::{Align, Face, FaceId, FontSet, TextCache, TextStyle};
    pub use crate::ui::{
        centre, hstack, place, vstack, Anchor, ButtonStyle, Drag, Dropdown, Flow, Focus, Insets,
        Length, Menu, Overlay, ScrollState, TableState, TextAreaState, TextFieldState, Theme,
        Tooltip, Ui,
    };
    pub use crate::windowing::{
        run_app, run_windows, CursorShape, PixelApp, Waker, WindowConfig, WindowRequest,
    };
}
