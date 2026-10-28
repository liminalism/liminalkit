//! Checkbox, toggle switch and radio group: the controls a settings form
//! needs that a table, a tab strip and a button do not already cover.
//!
//! Each follows the same shape as everything else in `widget.rs`/`chrome.rs`:
//! the screen owns the value (`&mut bool`, `&mut usize`), the widget draws
//! itself and hit-tests through [`Input`](liminalkit_shell::Input), and
//! returns whether it changed this frame. No new state types, because none
//! of these need one — a checkbox's whole state *is* the `bool` a screen
//! already has a field for.

use crate::widget::Ui;
use liminalkit_raster::Rect;
use liminalkit_shell::KeyInput;
use liminalkit_text::Align;

/// Logical size of a checkbox's box.
pub const CHECKBOX_SIZE: i32 = 18;
/// Logical size of a radio button's dot.
pub const RADIO_SIZE: i32 = 16;
/// Logical `(width, height)` of a toggle switch's track.
pub const TOGGLE_SIZE: (i32, i32) = (36, 20);

impl Ui<'_> {
    /// A checkbox with a trailing label; clicking the box *or* the label
    /// toggles it, matching a native checkbox's hit area. Returns whether it
    /// was toggled this frame.
    pub fn checkbox(&mut self, area: Rect, checked: &mut bool, label: &str) -> bool {
        self.checkbox_focused(area, checked, label, false)
    }

    /// A checkbox with optional caller-managed focus; a bound Focus takes precedence.
    pub fn checkbox_focused(
        &mut self,
        area: Rect,
        checked: &mut bool,
        label: &str,
        focused: bool,
    ) -> bool {
        let focused = self.control_focus(area, focused);
        #[cfg(feature = "accessibility")]
        self.semantic(
            crate::semantics::Role::CheckBox,
            label,
            area,
            focused,
            Some(*checked),
            None,
        );
        let theme = self.theme;
        let hovered = self.input.hovering(area);
        let box_size = self.px(CHECKBOX_SIZE).min(area.h);
        let box_rect = Rect::new(area.x, area.y + (area.h - box_size) / 2, box_size, box_size);

        let fill = if *checked {
            theme.accent
        } else if hovered {
            theme.hover
        } else {
            theme.panel
        };
        let edge = if *checked || focused {
            theme.accent
        } else {
            theme.panel_edge
        };
        self.painter.rounded_rect(
            box_rect,
            self.px(4),
            if focused {
                self.px(theme.focus_width)
            } else {
                theme.border_width
            },
            fill,
            edge,
        );

        if *checked {
            let b = box_rect;
            let points = [
                [
                    b.x as f32 + b.w as f32 * 0.22,
                    b.y as f32 + b.h as f32 * 0.55,
                ],
                [
                    b.x as f32 + b.w as f32 * 0.42,
                    b.y as f32 + b.h as f32 * 0.75,
                ],
                [
                    b.x as f32 + b.w as f32 * 0.80,
                    b.y as f32 + b.h as f32 * 0.25,
                ],
            ];
            self.painter.stroke_polyline_aa(
                self.kernel,
                &points,
                self.scale.f(2.0),
                theme.text,
                255,
            );
        }

        if !label.is_empty() {
            let gap = theme.padding / 2;
            let label_area = Rect::new(
                box_rect.right() + gap,
                area.y,
                (area.w - box_size - gap).max(0),
                area.h,
            );
            self.label(label_area, label, theme.text, Align::Left);
        }

        let clicked = self.input.take_click(area)
            || (focused
                && self.input.consume_keys(|keys| {
                    keys.iter()
                        .any(|key| matches!(key, KeyInput::Enter | KeyInput::Character(' ')))
                }));
        if clicked {
            *checked = !*checked;
        }
        clicked
    }

    /// A pill-shaped on/off switch with a trailing label. Returns whether it
    /// was toggled this frame.
    pub fn toggle(&mut self, area: Rect, on: &mut bool, label: &str) -> bool {
        let focused = self.control_focus(area, false);
        #[cfg(feature = "accessibility")]
        self.semantic(
            crate::semantics::Role::Switch,
            label,
            area,
            focused,
            Some(*on),
            None,
        );
        let theme = self.theme;
        let hovered = self.input.hovering(area);
        let (logical_w, logical_h) = TOGGLE_SIZE;
        let track_w = self.px(logical_w);
        let track_h = self.px(logical_h).min(area.h);
        let track = Rect::new(area.x, area.y + (area.h - track_h) / 2, track_w, track_h);

        let fill = if *on {
            theme.accent
        } else if hovered {
            theme.hover
        } else {
            theme.panel_edge
        };
        self.painter.fill_rounded_rect(track, track_h / 2, fill);

        let margin = self.px(2);
        let knob_r = (track_h - margin * 2) as f32 / 2.0;
        let cx = if *on {
            track.right() as f32 - margin as f32 - knob_r
        } else {
            track.x as f32 + margin as f32 + knob_r
        };
        let cy = track.y as f32 + track.h as f32 / 2.0;
        self.painter
            .fill_circle_aa(self.kernel, cx, cy, knob_r, theme.text, 255);

        if !label.is_empty() {
            let gap = theme.padding / 2;
            let label_area = Rect::new(
                track.right() + gap,
                area.y,
                (area.w - track_w - gap).max(0),
                area.h,
            );
            self.label(label_area, label, theme.text, Align::Left);
        }
        if focused {
            self.painter
                .stroke_rect(area, self.px(theme.focus_width), theme.accent);
        }

        let clicked = self.input.take_click(area)
            || (focused
                && self.input.consume_keys(|keys| {
                    keys.iter()
                        .any(|key| matches!(key, KeyInput::Enter | KeyInput::Character(' ')))
                }));
        if clicked {
            *on = !*on;
        }
        clicked
    }

    /// A column of radio buttons, one row per label. Clicking a row selects
    /// it; when `focused` is true, Up/Down also move the selection, wrapping
    /// at the ends — a radio group's arrow keys move *among its own
    /// options*, which is a different thing from [`crate::Focus`] moving
    /// *between widgets*, so this owns that behaviour itself rather than
    /// going through `Focus` at all. Returns whether the selection changed.
    pub fn radio_group(
        &mut self,
        selected: &mut usize,
        area: Rect,
        labels: &[&str],
        focused: bool,
    ) -> bool {
        let focused = self.control_focus(area, focused);
        if labels.is_empty() {
            return false;
        }
        *selected = (*selected).min(labels.len() - 1);
        let theme = self.theme;
        let row_h = area.h / labels.len() as i32;
        let dot_d = self.px(RADIO_SIZE);
        let mut changed = false;

        for (index, label) in labels.iter().enumerate() {
            let row = Rect::new(area.x, area.y + row_h * index as i32, area.w, row_h);
            let active = index == *selected;
            #[cfg(feature = "accessibility")]
            self.semantic(
                crate::semantics::Role::RadioButton,
                label,
                row,
                focused && active,
                Some(active),
                None,
            );
            let hovered = self.input.hovering(row);

            let cx = row.x as f32 + dot_d as f32 / 2.0;
            let cy = row.y as f32 + row.h as f32 / 2.0;
            let outer_r = dot_d as f32 / 2.0 - 1.0;
            let ring = if active || hovered {
                theme.accent
            } else {
                theme.panel_edge
            };
            self.painter.stroke_circle_aa(
                self.kernel,
                cx,
                cy,
                outer_r,
                if focused && active {
                    self.px(theme.focus_width) as f32
                } else {
                    self.scale.hairline() as f32
                },
                ring,
                255,
            );
            if active {
                let inner_r = (outer_r - self.px(5) as f32).max(1.0);
                self.painter
                    .fill_circle_aa(self.kernel, cx, cy, inner_r, theme.accent, 255);
            }

            let gap = theme.padding / 2;
            let label_area = Rect::new(
                row.x + dot_d + gap,
                row.y,
                (row.w - dot_d - gap).max(0),
                row.h,
            );
            self.label(label_area, label, theme.text, Align::Left);

            if self.input.take_click(row) && !active {
                *selected = index;
                changed = true;
            }
        }

        if focused {
            let delta = self.input.consume_keys(|keys| {
                let mut delta = 0i32;
                for key in keys {
                    match key {
                        KeyInput::Down => delta += 1,
                        KeyInput::Up => delta -= 1,
                        _ => {}
                    }
                }
                delta
            });
            if delta != 0 {
                let count = labels.len() as i32;
                *selected = (*selected as i32 + delta).rem_euclid(count) as usize;
                changed = true;
            }
        }
        changed
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

    fn ui_harness<R>(input: &mut Input, run: impl FnOnce(&mut Ui<'_>) -> R) -> (WindowBuffer, R) {
        let mut buffer = WindowBuffer::new(200, 100);
        let mut text = TextCache::new(set());
        let mut kernel = RasterKernel::new();
        let result;
        {
            let mut ui = Ui::new(
                Painter::new(&mut buffer),
                &mut text,
                input,
                Theme::default(),
                Scale::ONE,
                &mut kernel,
            );
            ui.painter.clear(0);
            result = run(&mut ui);
        }
        (buffer, result)
    }

    fn click_at(x: f32, y: f32) -> Input {
        let mut input = Input::new();
        input.cursor_moved(x, y);
        input.mouse(MouseButton::Left, true);
        input
    }

    #[test]
    fn bound_focus_makes_standard_checkbox_keyboard_operable() {
        let mut input = Input::new();
        let mut focus = crate::Focus::new();
        focus.set(0);
        input.key(KeyInput::Character(' '));
        let mut checked = false;
        let mut buffer = WindowBuffer::new(200, 100);
        let mut text = TextCache::new(set());
        let mut kernel = RasterKernel::new();
        let mut ui = Ui::new(
            Painter::new(&mut buffer),
            &mut text,
            &mut input,
            Theme::default(),
            Scale::ONE,
            &mut kernel,
        )
        .with_focus(&mut focus);
        let changed = ui.checkbox(Rect::new(0, 0, 120, 24), &mut checked, "Notify me");
        assert!(changed);
        assert!(checked);
    }
    // --- checkbox ---------------------------------------------------------

    #[test]
    fn clicking_a_checkbox_flips_it() {
        let area = Rect::new(0, 0, 120, 24);
        let mut input = click_at(10.0, 12.0);
        let mut checked = false;
        let (_, toggled) = ui_harness(&mut input, |ui| {
            ui.checkbox(area, &mut checked, "Notify me")
        });
        assert!(toggled);
        assert!(checked);
    }

    #[test]
    fn clicking_a_checkboxs_label_also_toggles_it() {
        // A native checkbox's label is part of its hit target.
        let area = Rect::new(0, 0, 120, 24);
        let mut input = click_at(100.0, 12.0); // well past the box itself
        let mut checked = false;
        let (_, toggled) = ui_harness(&mut input, |ui| {
            ui.checkbox(area, &mut checked, "Notify me")
        });
        assert!(toggled);
    }

    #[test]
    fn clicking_outside_a_checkbox_does_nothing() {
        let area = Rect::new(0, 0, 120, 24);
        let mut input = click_at(150.0, 50.0);
        let mut checked = false;
        let (_, toggled) = ui_harness(&mut input, |ui| {
            ui.checkbox(area, &mut checked, "Notify me")
        });
        assert!(!toggled);
        assert!(!checked);
    }

    #[test]
    fn a_checkbox_with_no_label_still_draws_and_toggles() {
        let area = Rect::new(0, 0, 24, 24);
        let mut input = click_at(10.0, 10.0);
        let mut checked = false;
        let (_, toggled) = ui_harness(&mut input, |ui| ui.checkbox(area, &mut checked, ""));
        assert!(toggled);
    }

    // --- toggle -------------------------------------------------------------

    #[test]
    fn clicking_a_toggle_flips_it() {
        let area = Rect::new(0, 0, 120, 24);
        let mut input = click_at(10.0, 12.0);
        let mut on = false;
        let (_, toggled) = ui_harness(&mut input, |ui| ui.toggle(area, &mut on, "Wi-Fi"));
        assert!(toggled);
        assert!(on);
        // And back off again.
        let mut input = click_at(10.0, 12.0);
        let (_, toggled) = ui_harness(&mut input, |ui| ui.toggle(area, &mut on, "Wi-Fi"));
        assert!(toggled);
        assert!(!on);
    }

    #[test]
    fn a_toggles_knob_moves_from_one_side_to_the_other() {
        let area = Rect::new(0, 0, 120, 24);
        let accent = Theme::default().text; // the knob's colour
        let mut off_input = Input::new();
        let mut on = false;
        let (off_buffer, _) = ui_harness(&mut off_input, |ui| ui.toggle(area, &mut on, ""));
        let mut on_input = Input::new();
        on = true;
        let (on_buffer, _) = ui_harness(&mut on_input, |ui| ui.toggle(area, &mut on, ""));

        let rightmost = |buffer: &WindowBuffer, colour: u32| {
            (0..buffer.width as i32).rev().find(|&x| {
                (0..buffer.height as i32).any(|y| {
                    buffer.pixels[y as usize * buffer.width as usize + x as usize] == colour
                })
            })
        };
        let off_x = rightmost(&off_buffer, accent).unwrap();
        let on_x = rightmost(&on_buffer, accent).unwrap();
        assert!(
            on_x > off_x,
            "on ({on_x}) should sit further right than off ({off_x})"
        );
    }

    // --- radio group ---------------------------------------------------------

    fn labels() -> Vec<&'static str> {
        vec!["Small", "Medium", "Large"]
    }

    #[test]
    fn clicking_a_row_selects_it() {
        let area = Rect::new(0, 0, 150, 90); // 3 rows of 30
        let mut input = click_at(50.0, 45.0); // second row
        let mut selected = 0;
        let items = labels();
        let (_, changed) = ui_harness(&mut input, |ui| {
            ui.radio_group(&mut selected, area, &items, false)
        });
        assert!(changed);
        assert_eq!(selected, 1);
    }

    #[test]
    fn clicking_the_already_selected_row_reports_no_change() {
        let area = Rect::new(0, 0, 150, 90);
        let mut input = click_at(50.0, 15.0); // first row
        let mut selected = 0;
        let items = labels();
        let (_, changed) = ui_harness(&mut input, |ui| {
            ui.radio_group(&mut selected, area, &items, false)
        });
        assert!(!changed);
    }

    #[test]
    fn arrow_keys_move_the_selection_only_when_focused() {
        let area = Rect::new(0, 0, 150, 90);
        let mut input = Input::new();
        input.key(KeyInput::Down);
        let mut selected = 0;
        let items = labels();
        let (_, changed) = ui_harness(&mut input, |ui| {
            ui.radio_group(&mut selected, area, &items, false)
        });
        assert!(!changed, "not focused, so arrows do nothing");
        assert_eq!(selected, 0);

        let mut input = Input::new();
        input.key(KeyInput::Down);
        let (_, changed) = ui_harness(&mut input, |ui| {
            ui.radio_group(&mut selected, area, &items, true)
        });
        assert!(changed);
        assert_eq!(selected, 1);
    }

    #[test]
    fn arrow_keys_wrap_at_both_ends() {
        let area = Rect::new(0, 0, 150, 90);
        let items = labels();

        let mut selected = 2; // last
        let mut input = Input::new();
        input.key(KeyInput::Down);
        ui_harness(&mut input, |ui| {
            ui.radio_group(&mut selected, area, &items, true)
        });
        assert_eq!(selected, 0, "down from the last wraps to the first");

        let mut input = Input::new();
        input.key(KeyInput::Up);
        ui_harness(&mut input, |ui| {
            ui.radio_group(&mut selected, area, &items, true)
        });
        assert_eq!(selected, 2, "up from the first wraps to the last");
    }

    #[test]
    fn an_empty_radio_group_does_not_panic() {
        let area = Rect::new(0, 0, 150, 90);
        let mut input = Input::new();
        let mut selected = 0;
        let (_, changed) = ui_harness(&mut input, |ui| {
            ui.radio_group(&mut selected, area, &[], true)
        });
        assert!(!changed);
    }

    #[test]
    fn a_stale_selection_past_the_end_snaps_back_rather_than_panicking() {
        let area = Rect::new(0, 0, 150, 90);
        let mut input = Input::new();
        let mut selected = 99;
        let items = labels();
        ui_harness(&mut input, |ui| {
            ui.radio_group(&mut selected, area, &items, false)
        });
        assert_eq!(selected, 2);
    }
}
