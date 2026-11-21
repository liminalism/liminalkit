//! liminalkit-native: operating-system controls inside a liminalkit window.
//!
//! A liminalkit GUI draws every pixel itself. This crate lets the same GUI
//! hand chosen controls to the platform instead: on Windows a declared
//! button is a real Win32 `BUTTON` child window, created through
//! [winsafe](https://docs.rs/winsafe), sitting on top of the software-drawn
//! frame at the rectangle the liminalkit layout gave it. Everywhere else —
//! and on Windows when asked — nothing native exists and the application
//! draws the control itself, from the same state.
//!
//! The window is still the ordinary liminalkit one: winit owns it and its
//! event loop, softbuffer presents the frame, and keyboard, IME, pointer
//! shapes, touch, file drop and multiple windows all behave as they do
//! without this crate.
//!
//! # Shape
//!
//! Controls are immediate-mode in the way that matters and retained in the
//! way the platform insists on:
//!
//! - **Declare** a control once and keep its [`ControlId`].
//! - **Place** it every frame it should be visible, with a rectangle in the
//!   same device pixels the painter uses. A control that is not placed during
//!   a frame is hidden — so a prompt that replaces a toolbar hides the
//!   toolbar's buttons by not placing them.
//! - **Read** what the user did: [`NativeControls::clicked`] for a button, or
//!   [`NativeControls::poll_event`] for everything.
//! - **Set** state (`set_checked`, `set_selected`, `set_text`) whenever the
//!   application's own state changes, every frame if that is easiest.
//!   Setting is not an event: syncing a toggle to the caret cannot loop.
//!
//! ```no_run
//! use liminalkit_native::NativeControls;
//! use liminalkit_raster::{Rect, WindowBuffer};
//! use liminalkit_shell::Scale;
//! use liminalkit_windowing::{run_app, PixelApp, WindowConfig};
//!
//! struct Demo {
//!     native: NativeControls,
//!     save: liminalkit_native::ControlId,
//!     saves: u32,
//! }
//!
//! impl PixelApp for Demo {
//!     fn render(&mut self, buffer: &mut WindowBuffer, scale: Scale) {
//!         buffer.pixels.fill(0x00f4_f4f4);
//!         if self.native.is_native() {
//!             self.native.place(self.save, scale.rect(Rect::new(12, 12, 88, 26)));
//!         } // else: draw a liminalkit button here
//!         if self.native.clicked(self.save) {
//!             self.saves += 1;
//!         }
//!     }
//! }
//!
//! let native = NativeControls::new();
//! let save = native.button("Save");
//! let app = Demo { native: native.clone(), save, saves: 0 };
//! run_app(native.host(app), WindowConfig::new("demo", 320.0, 200.0)).expect("run");
//! ```
//!
//! [`NativeControls::host`] is the one required step: it wraps the
//! application so the controls learn about the window and are reconciled
//! after every frame.
//!
//! # What native controls cost
//!
//! A native control is a child window, and child windows are always above
//! the drawn frame. A liminalkit dropdown, tooltip or overlay cannot paint
//! over one; do not place a native control on a frame where something drawn
//! must cover it. They also take keyboard focus when used. Buttons, toggles
//! and combos give it back to the canvas as soon as the interaction ends; a
//! text field keeps it until Enter, Escape, or a click on the canvas.
//!
//! # Platforms
//!
//! | Platform | Native controls |
//! |---|---|
//! | Windows | Yes, through winsafe. |
//! | macOS | Not yet: [`NativeControls::is_native`] is `false`. |
//! | Linux, others | No: `is_native` is `false`. |
//!
//! # Unsafe
//!
//! Embedding child windows in a window another library created means
//! subclassing its window procedure and creating raw controls, which Win32
//! exposes only unsafely. That code is confined to one Windows-only module;
//! the rest of the crate, and the rest of the workspace, denies it.

mod host;
mod model;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod win;

use std::cell::RefCell;
use std::rc::Rc;

use liminalkit_raster::Rect;
use model::Model;

pub use host::NativeHost;
pub use model::{ControlId, ControlKind, NativeEvent};

/// The winsafe this crate was built against, for applications that want to
/// reach the Win32 handles themselves.
#[cfg(target_os = "windows")]
pub use winsafe;

pub(crate) struct Shared {
    pub(crate) model: RefCell<Model>,
    #[cfg(target_os = "windows")]
    pub(crate) win: win::State,
}

/// The native controls of one window: declaration, per-frame placement,
/// state, and events.
///
/// A cheap handle (`Clone` shares the same controls) for the UI thread. Keep
/// one in the application and give the application to [`NativeControls::host`].
#[derive(Clone)]
pub struct NativeControls {
    shared: Rc<Shared>,
}

impl Default for NativeControls {
    fn default() -> NativeControls {
        NativeControls::new()
    }
}

impl std::fmt::Debug for NativeControls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeControls")
            .field("native", &self.is_native())
            .finish()
    }
}

impl NativeControls {
    /// Controls that are native where the platform supports it.
    pub fn new() -> NativeControls {
        NativeControls::with_native(cfg!(target_os = "windows"))
    }

    /// Controls that are never native, on any platform: the application
    /// draws them. For exercising the drawn fallback on Windows, and for a
    /// user setting that turns native controls off.
    pub fn drawn() -> NativeControls {
        NativeControls::with_native(false)
    }

    fn with_native(native: bool) -> NativeControls {
        let mut model = Model::default();
        model.native = native;
        NativeControls {
            shared: Rc::new(Shared {
                model: RefCell::new(model),
                #[cfg(target_os = "windows")]
                win: win::State::default(),
            }),
        }
    }

    /// Whether declared controls appear as platform controls. When `false`
    /// the application draws them itself; placing is then a no-op and no
    /// events arrive, but declared state (`text`, `checked`, `selected`)
    /// still reads and writes, so it can back the drawn version too.
    pub fn is_native(&self) -> bool {
        self.shared.model.borrow().native
    }

    /// Wrap the application so the controls follow its window and frames.
    /// Run the result wherever the application itself would have run.
    pub fn host<A: liminalkit_windowing::PixelApp>(&self, app: A) -> NativeHost<A> {
        NativeHost::new(self.clone(), app)
    }

    fn declare(
        &self,
        kind: ControlKind,
        text: &str,
        items: &[&str],
        checked: bool,
        selected: Option<usize>,
    ) -> ControlId {
        self.shared
            .model
            .borrow_mut()
            .declare(kind, text, items, checked, selected)
    }

    /// Declare a push button.
    pub fn button(&self, label: &str) -> ControlId {
        self.declare(ControlKind::Button, label, &[], false, None)
    }

    /// Declare a button that stays pressed while `on`.
    pub fn toggle(&self, label: &str, on: bool) -> ControlId {
        self.declare(ControlKind::Toggle, label, &[], on, None)
    }

    /// Declare a check box.
    pub fn checkbox(&self, label: &str, checked: bool) -> ControlId {
        self.declare(ControlKind::Checkbox, label, &[], checked, None)
    }

    /// Declare a drop-down list of fixed `items`.
    pub fn combo(&self, items: &[&str], selected: Option<usize>) -> ControlId {
        self.declare(ControlKind::Combo, "", items, false, selected)
    }

    /// Declare a single-line text field.
    pub fn edit(&self, text: &str) -> ControlId {
        self.declare(ControlKind::Edit, text, &[], false, None)
    }

    /// Declare static text.
    pub fn label(&self, text: &str) -> ControlId {
        self.declare(ControlKind::Label, text, &[], false, None)
    }

    /// Show the control at `rect` (device pixels, the painter's coordinates)
    /// for this frame. Call every frame the control should be visible.
    pub fn place(&self, id: ControlId, rect: Rect) {
        self.shared.model.borrow_mut().place(id, rect);
    }

    pub fn kind(&self, id: ControlId) -> Option<ControlKind> {
        self.shared.model.borrow().kind(id)
    }

    /// The control's caption or contents.
    pub fn text(&self, id: ControlId) -> String {
        self.shared.model.borrow().text(id)
    }

    /// Change the caption or contents. Not reported back as an event.
    pub fn set_text(&self, id: ControlId, text: &str) {
        self.shared.model.borrow_mut().set_text(id, text);
    }

    /// A combo's choices, as declared.
    pub fn items(&self, id: ControlId) -> Vec<String> {
        self.shared.model.borrow().items(id)
    }

    pub fn checked(&self, id: ControlId) -> bool {
        self.shared.model.borrow().checked(id)
    }

    /// Set a toggle or check box. Not reported back as an event.
    pub fn set_checked(&self, id: ControlId, checked: bool) {
        self.shared.model.borrow_mut().set_checked(id, checked);
    }

    pub fn selected(&self, id: ControlId) -> Option<usize> {
        self.shared.model.borrow().selected(id)
    }

    /// Set a combo's selection. Not reported back as an event.
    pub fn set_selected(&self, id: ControlId, selected: Option<usize>) {
        self.shared.model.borrow_mut().set_selected(id, selected);
    }

    pub fn enabled(&self, id: ControlId) -> bool {
        self.shared.model.borrow().enabled(id)
    }

    pub fn set_enabled(&self, id: ControlId, enabled: bool) {
        self.shared.model.borrow_mut().set_enabled(id, enabled);
    }

    /// Give a control keyboard focus after this frame — a find field on
    /// Ctrl+F.
    pub fn focus(&self, id: ControlId) {
        self.shared.model.borrow_mut().request_focus(Some(id));
    }

    /// Return keyboard focus to the liminalkit canvas after this frame.
    pub fn focus_canvas(&self) {
        self.shared.model.borrow_mut().request_focus(None);
    }

    /// The next thing the user did to a native control, oldest first.
    pub fn poll_event(&self) -> Option<NativeEvent> {
        self.shared.model.borrow_mut().poll_event()
    }

    /// Whether the button was clicked since this was last asked. Consumes
    /// one click and leaves other events queued.
    pub fn clicked(&self, id: ControlId) -> bool {
        self.shared
            .model
            .borrow_mut()
            .take_event(|event| *event == NativeEvent::Clicked(id))
    }

    /// Whether the toggle, check box, combo or text field was changed by the
    /// user since this was last asked. Read the new value with `checked`,
    /// `selected` or `text`.
    pub fn changed(&self, id: ControlId) -> bool {
        self.shared.model.borrow_mut().take_event(|event| {
            matches!(event,
                NativeEvent::Toggled(e, _)
                | NativeEvent::Selected(e, _)
                | NativeEvent::TextChanged(e, _) if *e == id)
        })
    }

    /// Whether Enter was pressed in the text field since this was last asked.
    pub fn submitted(&self, id: ControlId) -> bool {
        self.shared
            .model
            .borrow_mut()
            .take_event(|event| *event == NativeEvent::Submitted(id))
    }

    /// Colours for the text and background of labels and check boxes, as
    /// `0x00RRGGBB`, so they sit on the drawn frame instead of on a grey
    /// system-coloured box. `None` restores the system colours. Buttons,
    /// combos and text fields always use the system theme.
    pub fn set_backdrop(&self, colours: Option<(u32, u32)>) {
        #[cfg(target_os = "windows")]
        win::set_backdrop(&self.shared, colours);
        #[cfg(not(target_os = "windows"))]
        let _ = colours;
    }

    /// The Win32 window of a control, once it exists (after the first frame
    /// that follows window creation). For styling or messages this crate
    /// does not wrap; the handle belongs to this crate and dies with the
    /// window.
    #[cfg(target_os = "windows")]
    pub fn hwnd(&self, id: ControlId) -> Option<winsafe::HWND> {
        win::hwnd(&self.shared, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawn_controls_keep_state_but_are_never_native() {
        let native = NativeControls::drawn();
        assert!(!native.is_native());
        let bold = native.toggle("B", false);
        let style = native.combo(&["Body", "Subhead"], Some(0));
        native.set_checked(bold, true);
        native.set_selected(style, Some(1));
        assert!(native.checked(bold));
        assert_eq!(native.selected(style), Some(1));
        assert_eq!(native.items(style), vec!["Body", "Subhead"]);
        assert_eq!(native.text(bold), "B");
        assert_eq!(native.kind(style), Some(ControlKind::Combo));
        assert_eq!(native.poll_event(), None);
    }

    #[test]
    fn native_follows_the_platform() {
        assert_eq!(
            NativeControls::new().is_native(),
            cfg!(target_os = "windows")
        );
    }

    #[test]
    fn clones_share_one_set_of_controls() {
        let native = NativeControls::drawn();
        let other = native.clone();
        let field = native.edit("a");
        other.set_text(field, "b");
        assert_eq!(native.text(field), "b");
    }
}
