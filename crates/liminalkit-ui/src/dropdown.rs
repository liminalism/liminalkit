//! A dropdown/select: a closed field that, clicked, opens a list of options
//! drawn after everything else in the frame.
//!
//! The same problem [`Tooltip`](crate::Tooltip) solves, solved the same way.
//! Immediate mode has no z-order, so a widget cannot draw its own popup
//! inline — whatever the screen draws next would cover it. [`Ui::dropdown`]
//! draws the closed field and records what to show; [`Dropdown::draw`] draws
//! the open list, once, after everything else, and reports which option (if
//! any) was picked so the screen can apply it to its own selection state.

use crate::widget::Ui;
use liminalkit_raster::Rect;
use liminalkit_shell::KeyInput;
use liminalkit_text::Align;

/// A dropdown's open/closed state, owned by the screen alongside whatever
/// `usize` holds the selected option.
#[derive(Debug, Default)]
pub struct Dropdown {
    open: Option<Open>,
}

#[derive(Debug)]
struct Open {
    /// Physical rect of the closed field, so the popup knows where to hang.
    field: Rect,
    options: Vec<String>,
    selected: usize,
    /// A queued opening key may already have applied the popup's remaining keys.
    /// Picking still reports through `draw`, including for `Ui::dropdown` callers.
    pending_keys: Option<(Option<usize>, bool)>,
}

impl Open {
    /// Apply popup keys in order, stopping when it picks or dismisses.
    fn handle_keys(&mut self, keys: &[KeyInput]) -> (Option<usize>, bool) {
        for key in keys {
            match key {
                KeyInput::Up if !self.options.is_empty() => {
                    self.selected = (self.selected + self.options.len() - 1) % self.options.len();
                }
                KeyInput::Down if !self.options.is_empty() => {
                    self.selected = (self.selected + 1) % self.options.len();
                }
                KeyInput::Enter if !self.options.is_empty() => {
                    return (Some(self.selected), true);
                }
                KeyInput::Escape | KeyInput::Tab => return (None, true),
                _ => {}
            }
        }
        (None, false)
    }
}

impl Dropdown {
    pub fn new() -> Dropdown {
        Dropdown::default()
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    /// Draw the open popup, if there is one, inside `bounds`, and report
    /// which option was clicked this frame. Call once, last, after
    /// everything else the screen draws. Closes itself on a pick, on
    /// Escape, or on a click that lands inside `bounds` but on none of the
    /// options — the same "click outside dismisses it" a real menu has.
    pub fn draw(&mut self, ui: &mut Ui<'_>, bounds: Rect) -> Option<usize> {
        let Some(open) = &mut self.open else {
            return None;
        };
        #[cfg(feature = "accessibility")]
        ui.modal_semantics();
        let (mut picked, dismissed_by_key) = match open.pending_keys.take() {
            Some(result) => result,
            None => ui.input.consume_keys(|keys| open.handle_keys(keys)),
        };
        let theme = ui.theme;
        let row_h = ui.px(theme.row_height);
        let total_h = row_h * open.options.len() as i32;
        let below = Rect::new(open.field.x, open.field.bottom(), open.field.w, total_h);
        let fits_above = open.field.y - total_h >= bounds.y;
        let rect = if below.bottom() > bounds.bottom() && fits_above {
            Rect::new(open.field.x, open.field.y - total_h, open.field.w, total_h)
        } else {
            below
        };

        ui.painter.push_clip(bounds);
        ui.painter
            .shadow_rect(rect, ui.px(2), ui.px(3), 0x0010_1010, 40);
        ui.painter.rounded_rect(
            rect,
            theme.corner_radius,
            theme.border_width,
            theme.panel,
            theme.panel_edge,
        );

        for (index, label) in open.options.iter().enumerate() {
            let row = Rect::new(rect.x, rect.y + row_h * index as i32, rect.w, row_h);
            if ui.input.focus_requested(row) {
                open.selected = index;
            }
            #[cfg(feature = "accessibility")]
            ui.semantic(
                crate::semantics::Role::MenuListOption,
                label,
                row,
                index == open.selected,
                Some(index == open.selected),
                None,
            );
            if ui.input.hovering(row) || index == open.selected {
                ui.painter.fill_rect(row, theme.hover);
            }
            ui.label(row.inset(theme.padding / 2), label, theme.text, Align::Left);
            if ui.input.take_click(row) {
                picked = Some(index);
            }
        }
        ui.painter.pop_clip();

        // Whatever click did not land on a row — inside `bounds` but
        // elsewhere — dismisses the popup rather than being left pending
        // for something behind it to react to next frame.
        let dismissed_by_click = ui.input.take_click(bounds);
        if picked.is_some() || dismissed_by_click || dismissed_by_key {
            self.open = None;
            #[cfg(feature = "accessibility")]
            ui.modal_semantics();
        }
        picked
    }
}

impl Ui<'_> {
    /// A closed field showing `options[selected]`. Returns whether it was
    /// clicked (opening or closing the popup) — the popup's contents are
    /// drawn later, by [`Dropdown::draw`].
    pub fn dropdown(
        &mut self,
        dropdown: &mut Dropdown,
        area: Rect,
        options: &[&str],
        selected: usize,
    ) -> bool {
        self.dropdown_with_focus(dropdown, area, options, selected, false, false)
            .0
    }
    fn dropdown_with_focus(
        &mut self,
        dropdown: &mut Dropdown,
        area: Rect,
        options: &[&str],
        selected: usize,
        focused: bool,
        cycle_closed: bool,
    ) -> (bool, Option<usize>) {
        let focused = self.control_focus(area, focused);
        let mut selected = selected.min(options.len().saturating_sub(1));
        let mut picked = None;
        let mut clicked = self.input.take_click(area);
        if clicked {
            if dropdown.is_open() {
                dropdown.close();
            } else {
                dropdown.open = Some(Open {
                    field: area,
                    options: options.iter().map(|s| s.to_string()).collect(),
                    selected,
                    pending_keys: None,
                });
            }
        } else if focused && !dropdown.is_open() {
            self.input.consume_keys(|keys| {
                for (position, key) in keys.iter().enumerate() {
                    let delta = match key {
                        KeyInput::Up | KeyInput::Left if cycle_closed => -1,
                        KeyInput::Down | KeyInput::Right if cycle_closed => 1,
                        _ => 0,
                    };
                    if delta != 0 && !options.is_empty() {
                        selected =
                            (selected as i32 + delta).rem_euclid(options.len() as i32) as usize;
                        picked = Some(selected);
                    }
                    if matches!(key, KeyInput::Enter | KeyInput::Character(' ')) {
                        clicked = true;
                        let mut open = Open {
                            field: area,
                            options: options.iter().map(|s| s.to_string()).collect(),
                            selected,
                            pending_keys: None,
                        };
                        open.pending_keys = Some(open.handle_keys(&keys[position + 1..]));
                        dropdown.open = Some(open);
                        break;
                    }
                }
            });
        }
        let theme = self.theme;
        #[cfg(feature = "accessibility")]
        self.semantic(
            crate::semantics::Role::ComboBox,
            options.get(selected).copied().unwrap_or(""),
            area,
            focused,
            None,
            None,
        );
        let open = dropdown.is_open();
        let hovered = self.input.hovering(area);
        let fill = if open {
            theme.selection
        } else if hovered {
            theme.hover
        } else {
            theme.panel
        };
        self.painter.rounded_rect(
            area,
            theme.corner_radius,
            if focused {
                self.px(theme.focus_width)
            } else {
                theme.border_width
            },
            fill,
            if focused {
                theme.accent
            } else {
                theme.panel_edge
            },
        );

        let chevron_w = self.px(20);
        let (chevron_area, label_area) = area.inset(theme.padding / 2).split_right(chevron_w);
        let label = options.get(selected).copied().unwrap_or("");
        self.label(label_area, label, theme.text, Align::Left);
        self.draw_chevron(chevron_area, open, theme.text_dim);

        (clicked, picked)
    }

    /// Keyboard focus cycles the same choices that the pointer popup offers.
    /// Enter/Space opens it; later keys in the same frame belong to the popup.
    /// Returns a newly selected index; popup clicks still come from `draw`.
    pub fn dropdown_focused(
        &mut self,
        dropdown: &mut Dropdown,
        area: Rect,
        options: &[&str],
        selected: usize,
        focused: bool,
    ) -> Option<usize> {
        self.dropdown_with_focus(dropdown, area, options, selected, focused, true)
            .1
    }

    /// A small up/down chevron, `open` picking which way it points.
    fn draw_chevron(&mut self, area: Rect, open: bool, colour: u32) {
        let cx = area.x as f32 + area.w as f32 / 2.0;
        let cy = area.y as f32 + area.h as f32 / 2.0;
        let s = self.scale.f(4.0);
        let points = if open {
            [
                [cx - s, cy + s * 0.4],
                [cx, cy - s * 0.5],
                [cx + s, cy + s * 0.4],
            ]
        } else {
            [
                [cx - s, cy - s * 0.4],
                [cx, cy + s * 0.5],
                [cx + s, cy - s * 0.4],
            ]
        };
        self.painter
            .stroke_polyline_aa(self.kernel, &points, self.scale.f(1.5), colour, 255);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::Theme;
    use liminalkit_raster::{Painter, RasterKernel, WindowBuffer};
    use liminalkit_shell::{Input, MouseButton, Scale};
    use liminalkit_text::font::test_fonts::set;
    use liminalkit_text::TextCache;

    struct Harness {
        buffer: WindowBuffer,
        text: TextCache,
        kernel: RasterKernel,
        input: Input,
    }

    impl Harness {
        fn new() -> Harness {
            Harness {
                buffer: WindowBuffer::new(200, 200),
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
            run(&mut ui)
        }

        fn click(&mut self, x: f32, y: f32) {
            self.input.cursor_moved(x, y);
            self.input.mouse(MouseButton::Left, true);
        }
    }

    fn options() -> Vec<&'static str> {
        vec!["Light", "Dark", "System"]
    }

    fn field() -> Rect {
        Rect::new(10, 10, 100, 24)
    }

    #[test]
    fn clicking_the_closed_field_opens_it() {
        let mut h = Harness::new();
        let items = options();
        h.click(50.0, 20.0);
        let clicked = h.frame(|ui| {
            let mut dropdown = Dropdown::new();
            let clicked = ui.dropdown(&mut dropdown, field(), &items, 0);
            assert!(dropdown.is_open());
            clicked
        });
        assert!(clicked);
    }

    #[test]
    fn drawing_a_closed_dropdown_reports_nothing_picked() {
        let mut h = Harness::new();
        let mut dropdown = Dropdown::new();
        let picked = h.frame(|ui| dropdown.draw(ui, Rect::new(0, 0, 200, 200)));
        assert_eq!(picked, None);
    }

    #[test]
    fn picking_an_option_reports_it_and_closes_the_popup() {
        let mut h = Harness::new();
        let items = options();
        let mut dropdown = Dropdown::new();

        h.click(50.0, 20.0);
        h.frame(|ui| ui.dropdown(&mut dropdown, field(), &items, 0));
        h.input.end_frame();

        // The popup hangs below the field; click its second row.
        let row_h = Theme::default().row_height;
        h.click(
            field().x as f32 + 5.0,
            (field().bottom() + row_h + 2) as f32,
        );
        let picked = h.frame(|ui| dropdown.draw(ui, Rect::new(0, 0, 200, 200)));

        assert_eq!(picked, Some(1));
        assert!(!dropdown.is_open());
    }

    #[test]
    fn clicking_inside_bounds_but_off_the_list_dismisses_it_without_a_pick() {
        let mut h = Harness::new();
        let items = options();
        let mut dropdown = Dropdown::new();

        h.click(50.0, 20.0);
        h.frame(|ui| ui.dropdown(&mut dropdown, field(), &items, 0));
        h.input.end_frame();

        h.click(190.0, 190.0); // far from the popup
        let picked = h.frame(|ui| dropdown.draw(ui, Rect::new(0, 0, 200, 200)));

        assert_eq!(picked, None);
        assert!(!dropdown.is_open());
    }

    #[test]
    fn escape_closes_the_popup() {
        let mut h = Harness::new();
        let items = options();
        let mut dropdown = Dropdown::new();

        h.click(50.0, 20.0);
        h.frame(|ui| ui.dropdown(&mut dropdown, field(), &items, 0));
        h.input.end_frame();

        h.input.key(KeyInput::Escape);
        let picked = h.frame(|ui| dropdown.draw(ui, Rect::new(0, 0, 200, 200)));
        assert_eq!(picked, None);
        assert!(!dropdown.is_open());
    }

    #[test]
    fn clicking_the_open_field_again_closes_it_without_reopening() {
        let mut h = Harness::new();
        let items = options();
        let mut dropdown = Dropdown::new();

        h.click(50.0, 20.0);
        h.frame(|ui| ui.dropdown(&mut dropdown, field(), &items, 0));
        assert!(dropdown.is_open());
        h.input.end_frame();

        h.click(50.0, 20.0); // the field again
        h.frame(|ui| ui.dropdown(&mut dropdown, field(), &items, 0));
        assert!(!dropdown.is_open());

        // With nothing open, draw is a no-op — the field's own click already
        // consumed the frame's pending click.
        let picked = h.frame(|ui| dropdown.draw(ui, Rect::new(0, 0, 200, 200)));
        assert_eq!(picked, None);
    }

    #[test]
    fn a_field_near_the_bottom_opens_its_popup_upward() {
        let mut h = Harness::new();
        let items = options();
        let mut dropdown = Dropdown::new();
        let near_bottom = Rect::new(10, 180, 100, 15); // little room below

        h.click(50.0, 185.0);
        h.frame(|ui| ui.dropdown(&mut dropdown, near_bottom, &items, 0));
        h.input.end_frame();

        h.frame(|ui| dropdown.draw(ui, Rect::new(0, 0, 200, 200)));
        // Something was painted above the field's top edge.
        let above = (0..near_bottom.y as usize)
            .any(|y| (0..200usize).any(|x| h.buffer.pixels[y * 200 + x] != 0));
        assert!(above, "the popup should have opened upward");
    }

    #[test]
    fn an_empty_options_list_does_not_panic() {
        let mut h = Harness::new();
        let mut dropdown = Dropdown::new();
        h.click(50.0, 20.0);
        h.frame(|ui| ui.dropdown(&mut dropdown, field(), &[], 0));
        h.input.end_frame();
        let picked = h.frame(|ui| dropdown.draw(ui, Rect::new(0, 0, 200, 200)));
        assert_eq!(picked, None);
    }

    #[test]
    fn queued_keyboard_pick_is_reported_by_the_plain_dropdown_popup() {
        let mut h = Harness::new();
        let mut dropdown = Dropdown::new();
        let mut focus = crate::Focus::new();
        focus.set(0);
        for key in [KeyInput::Enter, KeyInput::Down, KeyInput::Enter] {
            h.input.key(key);
        }
        let mut ui = Ui::new(
            Painter::new(&mut h.buffer),
            &mut h.text,
            &mut h.input,
            Theme::default(),
            Scale::ONE,
            &mut h.kernel,
        )
        .with_focus(&mut focus);
        assert!(ui.dropdown(&mut dropdown, field(), &options(), 0));
        assert_eq!(dropdown.draw(&mut ui, Rect::new(0, 0, 200, 200)), Some(1));
        assert!(!dropdown.is_open());
    }

    #[test]
    fn queued_closed_arrows_precede_popup_navigation_and_selection() {
        let mut h = Harness::new();
        let mut dropdown = Dropdown::new();
        for key in [
            KeyInput::Right,
            KeyInput::Enter,
            KeyInput::Down,
            KeyInput::Enter,
        ] {
            h.input.key(key);
        }
        h.frame(|ui| {
            assert_eq!(
                ui.dropdown_focused(&mut dropdown, field(), &options(), 0, true),
                Some(1)
            );
            assert_eq!(dropdown.draw(ui, Rect::new(0, 0, 200, 200)), Some(2));
        });
        assert!(!dropdown.is_open());
    }
}
