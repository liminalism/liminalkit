//! A surface drawn after everything else that owns input while it is open:
//! a modal dialog centred in the window, or a popover hung from a control.
//!
//! The same shape as [`Dropdown`](crate::Dropdown). Immediate mode has no
//! z-order, so the overlay is drawn last; and because the background draws
//! first, the overlay has to claim input *before* the background does.
//! [`Overlay::handle_input`] runs at the start of the frame, before the
//! `Ui` is built (`Screen::before_frame` in the `liminalkit` crate, or the
//! top of `PixelApp::render`): it takes every key, swallows clicks that land
//! outside the surface, and decides whether Escape or an outside click
//! dismisses it. [`Overlay::draw`] runs last: it paints the scrim and the
//! surface, hands the keys back for the overlay's own controls, and calls
//! the content closure with the inner rectangle and the overlay's own
//! [`Focus`], which is what traps Tab inside it.
//!
//! What persists between frames is one value the screen owns: whether the
//! overlay is open, how it is placed, the keys taken this frame, and its
//! focus. The content is whatever the closure draws; the overlay stores
//! nothing about it.

use crate::focus::Focus;
use crate::widget::Ui;
use liminalkit_raster::Rect;
use liminalkit_shell::{Input, KeyInput, Scale};

/// Where the surface goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Centred in the bounds. A dialog.
    Centred,
    /// Below the anchor, left-aligned with it; above it when there is no
    /// room below; shifted left to stay inside the bounds. A popover.
    Anchored(Rect),
}

impl Placement {
    fn rect(self, bounds: Rect, size: (i32, i32)) -> Rect {
        let (w, h) = (
            size.0.max(1).min(bounds.w.max(1)),
            size.1.max(1).min(bounds.h.max(1)),
        );
        match self {
            Placement::Centred => crate::layout::centre(bounds, (w, h)),
            Placement::Anchored(anchor) => {
                let x = anchor.x.min(bounds.right() - w).max(bounds.x);
                let below = anchor.bottom();
                let y = if below + h <= bounds.bottom() || anchor.y - h < bounds.y {
                    below.min(bounds.bottom() - h).max(bounds.y)
                } else {
                    anchor.y - h
                };
                Rect::new(x, y, w, h)
            }
        }
    }
}

/// An overlay's open/closed state, owned by the screen.
#[derive(Debug, Default)]
pub struct Overlay {
    open: Option<Open>,
}

#[derive(Debug)]
struct Open {
    placement: Placement,
    /// Logical size; scaled when the frame's scale is known.
    size: (i32, i32),
    modal: bool,
    /// Keys taken by `handle_input`, handed back in `draw`.
    keys: Vec<KeyInput>,
    pending: Option<Action>,
    focus: Focus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Continue,
    Dismiss,
}

/// What the overlay's frame asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayOutcome {
    /// Escape, or for a popover a click outside, closed it this frame. The
    /// content closure still ran, so a form can read its own state.
    pub dismissed: bool,
}

impl Overlay {
    pub fn new() -> Overlay {
        Overlay::default()
    }

    /// Open a modal dialog of a logical `size`, centred. Clicks outside are
    /// swallowed; Escape dismisses.
    pub fn open_dialog(&mut self, size: (i32, i32)) {
        self.open_with(Placement::Centred, size, true);
    }

    /// Open a popover of a logical `size` hung from `anchor` (a device-pixel
    /// rectangle, usually the control that opened it). A click outside
    /// dismisses it and is *not* delivered to what was clicked, so a popover
    /// cannot be closed and a background button pressed by one click.
    pub fn open_popover(&mut self, anchor: Rect, size: (i32, i32)) {
        self.open_with(Placement::Anchored(anchor), size, false);
    }

    fn open_with(&mut self, placement: Placement, size: (i32, i32), modal: bool) {
        self.open = Some(Open {
            placement,
            size,
            modal,
            keys: Vec::new(),
            pending: None,
            focus: Focus::new(),
        });
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    /// The overlay's own keyboard focus, for a screen that wants to put it
    /// on a particular control when the overlay opens.
    pub fn focus(&mut self) -> Option<&mut Focus> {
        self.open.as_mut().map(|open| &mut open.focus)
    }

    /// The surface's device-pixel rectangle inside `bounds` at `scale`, or
    /// `None` when closed.
    pub fn rect(&self, bounds: Rect, scale: Scale) -> Option<Rect> {
        self.open.as_ref().map(|open| {
            open.placement
                .rect(bounds, (scale.px(open.size.0), scale.px(open.size.1)))
        })
    }

    /// Claim the frame's input before the background draws. Call once per
    /// frame, before the `Ui` is built; calling again before `draw` is
    /// harmless. Without this call the background still draws under a
    /// surface that merely covers it, and `draw` claims what is left.
    pub fn handle_input(&mut self, input: &mut Input, bounds: Rect, scale: Scale) {
        let Some(open) = &mut self.open else { return };
        if open.pending.is_some() {
            return;
        }
        let rect = open
            .placement
            .rect(bounds, (scale.px(open.size.0), scale.px(open.size.1)));
        let keys = input.take_keys();
        let mut action = if keys.contains(&KeyInput::Escape) {
            Action::Dismiss
        } else {
            Action::Continue
        };
        // Escape is the overlay's; everything else is for its controls.
        open.keys = keys
            .into_iter()
            .filter(|key| *key != KeyInput::Escape)
            .collect();

        let outside =
            |position: Option<(i32, i32)>| position.is_some_and(|(x, y)| !rect.contains(x, y));
        if outside(input.press_position()) {
            // Taken either way, so the control underneath never sees it.
            if input.take_click(bounds) && !open.modal {
                action = Action::Dismiss;
            }
            input.take_context_click(bounds);
        }
        if outside(input.cursor()) {
            input.take_scroll(bounds);
        }
        if !input.focus_requested(rect) {
            input.clear_focus_request();
        }
        open.pending = Some(action);
    }

    /// Paint the overlay last, run `content` inside it, and report what the
    /// frame asked for. `title` is drawn as a heading across the top of a
    /// dialog and omitted when empty; a popover has no title bar.
    ///
    /// `content(ui, inner, focus)` draws the overlay's controls into `inner`
    /// and registers the focusable ones with `focus`, the overlay's own, so
    /// Tab stays inside the surface:
    /// `ui.button_focused(r, "OK", focus.register_at(ui.input, r))`.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        bounds: Rect,
        title: &str,
        content: impl FnOnce(&mut Ui<'_>, Rect, &mut Focus),
    ) -> OverlayOutcome {
        if self
            .open
            .as_ref()
            .is_some_and(|open| open.pending.is_none())
        {
            self.handle_input(ui.input, bounds, ui.scale);
        }
        let Some(open) = &mut self.open else {
            return OverlayOutcome::default();
        };
        let action = open.pending.take().unwrap_or(Action::Continue);
        let rect = open
            .placement
            .rect(bounds, (ui.px(open.size.0), ui.px(open.size.1)));
        let theme = ui.theme;

        #[cfg(feature = "accessibility")]
        if open.modal {
            ui.modal_semantics();
        }
        ui.painter.push_clip(bounds);
        if open.modal {
            ui.painter.blend_rect(bounds, theme.background, 150);
        }
        ui.painter
            .shadow_rect(rect, ui.px(3), ui.px(6), 0x0010_1010, 50);
        ui.painter.rounded_rect(
            rect,
            ui.px(theme.corner_radius),
            ui.px(theme.border_width),
            theme.panel,
            theme.panel_edge,
        );
        let mut inner = rect.inset(ui.px(theme.padding));
        if open.modal && !title.is_empty() {
            let title_h = ui.text.line_height(theme.heading);
            let (head, rest) = inner.split_top(title_h);
            ui.heading(head, title);
            inner = Rect::new(
                rest.x,
                rest.y + ui.px(theme.padding) / 2,
                rest.w,
                (rest.h - ui.px(theme.padding) / 2).max(0),
            );
        }

        // The overlay's controls see the keys the background did not.
        ui.input.restore_keys(std::mem::take(&mut open.keys));
        open.focus.handle_tab(ui.input.keys(), ui.input.modifiers());
        content(ui, inner, &mut open.focus);
        ui.painter.pop_clip();

        let dismissed = action == Action::Dismiss;
        if dismissed {
            self.open = None;
            #[cfg(feature = "accessibility")]
            ui.modal_semantics();
        }
        OverlayOutcome { dismissed }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::Theme;
    use liminalkit_raster::{Painter, RasterKernel, WindowBuffer};
    use liminalkit_shell::MouseButton;
    use liminalkit_test_fonts::set;
    use liminalkit_text::TextCache;

    struct Harness {
        buffer: WindowBuffer<'static>,
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

        fn click(&mut self, x: f32, y: f32) {
            self.input.cursor_moved(x, y);
            self.input.mouse(MouseButton::Left, true);
            self.input.mouse(MouseButton::Left, false);
        }
    }

    const BOUNDS: Rect = Rect::new(0, 0, 300, 200);

    #[test]
    fn a_dialog_is_centred_and_a_popover_hangs_below_its_anchor_or_flips_above() {
        let mut overlay = Overlay::new();
        overlay.open_dialog((100, 50));
        assert_eq!(
            overlay.rect(BOUNDS, Scale::ONE),
            Some(Rect::new(100, 75, 100, 50))
        );
        overlay.open_popover(Rect::new(10, 10, 40, 20), (100, 50));
        assert_eq!(
            overlay.rect(BOUNDS, Scale::ONE),
            Some(Rect::new(10, 30, 100, 50))
        );
        overlay.open_popover(Rect::new(250, 170, 40, 20), (100, 50));
        assert_eq!(
            overlay.rect(BOUNDS, Scale::ONE),
            Some(Rect::new(200, 120, 100, 50)),
            "no room below or to the right: above the anchor, shifted left"
        );
        overlay.open_dialog((100, 50));
        assert_eq!(
            overlay.rect(BOUNDS, Scale(2.0)),
            Some(Rect::new(50, 50, 200, 100)),
            "the logical size follows the scale"
        );
    }

    #[test]
    fn a_click_outside_a_modal_dialog_reaches_nothing_and_does_not_close_it() {
        let mut h = Harness::new();
        let mut overlay = Overlay::new();
        overlay.open_dialog((100, 50));
        h.click(5.0, 5.0);
        overlay.handle_input(&mut h.input, BOUNDS, Scale::ONE);
        let background_clicked = h.frame(|ui| ui.button(Rect::new(0, 0, 20, 20), "under"));
        assert!(!background_clicked, "the dialog swallowed the click");
        assert!(overlay.is_open());
    }

    #[test]
    fn a_click_outside_a_popover_dismisses_it_without_reaching_what_was_clicked() {
        let mut h = Harness::new();
        let mut overlay = Overlay::new();
        overlay.open_popover(Rect::new(100, 10, 40, 20), (100, 50));
        h.click(5.0, 5.0);
        overlay.handle_input(&mut h.input, BOUNDS, Scale::ONE);
        let (background_clicked, outcome) = h.frame(|ui| {
            let clicked = ui.button(Rect::new(0, 0, 20, 20), "under");
            let outcome = overlay.draw(ui, BOUNDS, "", |_, _, _| {});
            (clicked, outcome)
        });
        assert!(!background_clicked);
        assert!(outcome.dismissed);
        assert!(!overlay.is_open());
    }

    #[test]
    fn keys_are_kept_from_the_background_and_given_to_the_overlay_content() {
        let mut h = Harness::new();
        let mut overlay = Overlay::new();
        overlay.open_dialog((200, 100));
        h.input.key(KeyInput::Character('x'));
        h.input.key(KeyInput::Tab);
        overlay.handle_input(&mut h.input, BOUNDS, Scale::ONE);
        assert!(h.input.keys().is_empty(), "nothing for the background");
        let seen = h.frame(|ui| {
            let mut seen = Vec::new();
            overlay.draw(ui, BOUNDS, "Title", |ui, inner, focus| {
                seen = ui.input.keys().to_vec();
                let (a, rest) = inner.split_top(20);
                let (b, _) = rest.split_top(20);
                let fa = focus.register_at(ui.input, a);
                let fb = focus.register_at(ui.input, b);
                assert!(!fa && !fb, "first frame establishes the order");
            });
            seen
        });
        assert_eq!(seen, vec![KeyInput::Character('x'), KeyInput::Tab]);

        // Tab now moves the overlay's own focus to its first control.
        h.input.key(KeyInput::Tab);
        overlay.handle_input(&mut h.input, BOUNDS, Scale::ONE);
        let focused = h.frame(|ui| {
            let mut focused = (false, false);
            overlay.draw(ui, BOUNDS, "Title", |ui, inner, focus| {
                let (a, rest) = inner.split_top(20);
                let (b, _) = rest.split_top(20);
                focused = (
                    focus.register_at(ui.input, a),
                    focus.register_at(ui.input, b),
                );
            });
            focused
        });
        assert_eq!(focused, (true, false));
    }

    #[test]
    fn escape_dismisses_and_is_not_passed_on_to_the_content() {
        let mut h = Harness::new();
        let mut overlay = Overlay::new();
        overlay.open_dialog((100, 50));
        h.input.key(KeyInput::Escape);
        overlay.handle_input(&mut h.input, BOUNDS, Scale::ONE);
        let (outcome, seen) = h.frame(|ui| {
            let mut seen = Vec::new();
            let outcome = overlay.draw(ui, BOUNDS, "", |ui, _, _| {
                seen = ui.input.keys().to_vec();
            });
            (outcome, seen)
        });
        assert!(outcome.dismissed);
        assert!(seen.is_empty());
        assert!(!overlay.is_open());
    }

    #[test]
    fn a_click_inside_the_surface_reaches_its_controls() {
        let mut h = Harness::new();
        let mut overlay = Overlay::new();
        overlay.open_dialog((200, 100));
        let rect = overlay.rect(BOUNDS, Scale::ONE).unwrap();
        let button = Rect::new(rect.x + 20, rect.y + 60, 60, 24);
        h.click(button.x as f32 + 5.0, button.y as f32 + 5.0);
        overlay.handle_input(&mut h.input, BOUNDS, Scale::ONE);
        let pressed = h.frame(|ui| {
            let mut pressed = false;
            overlay.draw(ui, BOUNDS, "Confirm", |ui, _, _| {
                pressed = ui.button(button, "OK");
            });
            pressed
        });
        assert!(pressed);
        assert!(overlay.is_open(), "a dialog closes when its screen says so");
    }

    #[test]
    fn drawing_a_closed_overlay_does_nothing() {
        let mut h = Harness::new();
        let mut overlay = Overlay::new();
        let outcome = h.frame(|ui| overlay.draw(ui, BOUNDS, "", |_, _, _| panic!("closed")));
        assert_eq!(outcome, OverlayOutcome::default());
    }
}
