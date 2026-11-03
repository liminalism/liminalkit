//! A context menu: a list of commands opened at a point, usually a
//! right-click, drawn after everything else and closed by a pick, Escape,
//! or a click elsewhere.
//!
//! It is a [`Dropdown`] popup without a field: the same input prepass, the
//! same keyboard navigation, the same draw-last contract. The screen owns
//! one `Menu` per kind of menu it shows and decides what the picked index
//! means.

use crate::dropdown::Dropdown;
use crate::widget::Ui;
use liminalkit_raster::Rect;
use liminalkit_shell::Input;

/// A context menu's open/closed state, owned by the screen.
#[derive(Debug, Default)]
pub struct Menu {
    popup: Dropdown,
}

impl Menu {
    pub fn new() -> Menu {
        Menu::default()
    }

    /// Open at a device-pixel point, `width` device pixels wide, with these
    /// items. The list hangs below the point and flips above it when there
    /// is no room, exactly as a dropdown's popup does.
    pub fn open_at(&mut self, x: i32, y: i32, width: i32, items: &[&str]) {
        self.popup
            .open_at(Rect::new(x, y, width.max(1), 0), items, 0);
    }

    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    pub fn close(&mut self) {
        self.popup.close();
    }

    /// Claim the open menu's input before the background draws. Call at the
    /// start of the frame, before the `Ui` is built. `row_height` is device
    /// pixels: scale the theme token.
    pub fn handle_input(&mut self, input: &mut Input, bounds: Rect, row_height: i32) {
        self.popup.handle_input(input, bounds, row_height);
    }

    /// Draw the open menu last and report the picked item, if any. Closes
    /// on a pick, on Escape, and on a click elsewhere.
    pub fn draw(&mut self, ui: &mut Ui<'_>, bounds: Rect) -> Option<usize> {
        self.popup.draw(ui, bounds)
    }
}

impl Ui<'_> {
    /// The width a menu needs for `items` in the body style, plus padding.
    pub fn menu_width(&mut self, items: &[&str]) -> i32 {
        let style = self.theme.body;
        let widest = items
            .iter()
            .map(|item| self.text.measure(item, style))
            .max()
            .unwrap_or(0);
        widest + self.px(self.theme.padding) * 2
    }

    /// Open `menu` at the pointer if `area` was right-clicked this frame.
    /// Returns whether it opened.
    pub fn context_menu(&mut self, menu: &mut Menu, area: Rect, items: &[&str]) -> bool {
        let Some((x, y)) = self.input.cursor() else {
            return false;
        };
        if !self.input.take_context_click(area) {
            return false;
        }
        let width = self.menu_width(items);
        menu.open_at(x, y, width, items);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::Theme;
    use liminalkit_raster::{Painter, RasterKernel, WindowBuffer};
    use liminalkit_shell::{KeyInput, MouseButton, Scale};
    use liminalkit_text::{font::test_fonts::set, TextCache};

    struct Harness {
        buffer: WindowBuffer,
        text: TextCache,
        kernel: RasterKernel,
        input: Input,
    }

    impl Harness {
        fn new() -> Harness {
            Harness {
                buffer: WindowBuffer::new(300, 200),
                text: TextCache::new(set()),
                kernel: RasterKernel::new(),
                input: Input::new(),
            }
        }

        fn frame<R>(&mut self, run: impl FnOnce(&mut Ui<'_>) -> R) -> R {
            self.buffer.pixels.fill(0);
            let mut ui = Ui::new(
                Painter::new(&mut self.buffer),
                &mut self.text,
                &mut self.input,
                Theme::default(),
                Scale::ONE,
                &mut self.kernel,
            );
            let result = run(&mut ui);
            ui.input.end_frame();
            result
        }
    }

    const BOUNDS: Rect = Rect::new(0, 0, 300, 200);
    const ITEMS: [&str; 3] = ["Cut", "Copy", "Paste"];

    #[test]
    fn a_right_click_on_the_area_opens_the_menu_at_the_pointer() {
        let mut h = Harness::new();
        let mut menu = Menu::new();
        h.input.cursor_moved(40.0, 30.0);
        h.input.mouse(MouseButton::Right, true);
        h.input.mouse(MouseButton::Right, false);
        let opened = h.frame(|ui| ui.context_menu(&mut menu, Rect::new(0, 0, 100, 100), &ITEMS));
        assert!(opened);
        assert!(menu.is_open());
    }

    #[test]
    fn a_right_click_elsewhere_does_not_open_it() {
        let mut h = Harness::new();
        let mut menu = Menu::new();
        h.input.cursor_moved(150.0, 150.0);
        h.input.mouse(MouseButton::Right, true);
        h.input.mouse(MouseButton::Right, false);
        let opened = h.frame(|ui| ui.context_menu(&mut menu, Rect::new(0, 0, 100, 100), &ITEMS));
        assert!(!opened);
    }

    #[test]
    fn arrow_keys_and_enter_pick_an_item_and_close_the_menu() {
        let mut h = Harness::new();
        let mut menu = Menu::new();
        menu.open_at(40, 30, 100, &ITEMS);
        for key in [KeyInput::Down, KeyInput::Down, KeyInput::Enter] {
            h.input.key(key);
        }
        menu.handle_input(&mut h.input, BOUNDS, 30);
        let picked = h.frame(|ui| menu.draw(ui, BOUNDS));
        assert_eq!(picked, Some(2));
        assert!(!menu.is_open());
    }

    #[test]
    fn a_click_on_a_row_picks_it_and_a_click_elsewhere_dismisses() {
        let mut h = Harness::new();
        let mut menu = Menu::new();
        menu.open_at(40, 30, 100, &ITEMS);
        // Rows hang below (40, 30), 30 tall each: the second is y 60..90.
        h.input.cursor_moved(60.0, 75.0);
        h.input.mouse(MouseButton::Left, true);
        h.input.mouse(MouseButton::Left, false);
        menu.handle_input(&mut h.input, BOUNDS, 30);
        let picked = h.frame(|ui| menu.draw(ui, BOUNDS));
        assert_eq!(picked, Some(1));

        menu.open_at(40, 30, 100, &ITEMS);
        h.input.cursor_moved(250.0, 180.0);
        h.input.mouse(MouseButton::Left, true);
        h.input.mouse(MouseButton::Left, false);
        menu.handle_input(&mut h.input, BOUNDS, 30);
        let picked = h.frame(|ui| menu.draw(ui, BOUNDS));
        assert_eq!(picked, None);
        assert!(!menu.is_open());
    }
}
