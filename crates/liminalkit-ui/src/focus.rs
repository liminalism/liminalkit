//! Keyboard focus: which registered widget currently has it, and how Tab,
//! Shift-Tab and (optionally) arrow keys move it.
//!
//! [`Ui::text_field`](crate::widget::Ui::text_field)'s own doc comment used
//! to call `focused: bool` a screen's job "deliberately" — fine for one or
//! two fields, but a Settings page with a dozen of them turns into a dozen
//! bool fields and no way to move between them except a mouse click, which
//! fails a requirement that every control be keyboard-reachable.
//!
//! `Focus` fits the same shape as [`TableState`](crate::TableState): one
//! small value a screen owns, rebuilt from what actually gets drawn rather
//! than looked up by a name that could collide. There is still no widget
//! tree and no per-widget storage — a screen calls [`Focus::register`] once
//! per focusable thing, in the same order it draws them every frame, and
//! that draw-order position *is* the widget's identity for this purpose.
//! That is not a workaround standing in for a real identity system: tab
//! order is inherently positional, even in a native toolkit's own
//! accessibility tree ("the fourth field on the form"), so a call-order
//! index is the *correct* notion of identity here — unlike scroll offset or
//! table selection, which must survive a resort or a filter and so are keyed
//! by something that outlives its position.
//!
//! # The one wrinkle immediate mode adds
//!
//! How many focusable widgets exist this frame is not known until the last
//! one has registered — but a Tab press has to move focus before the first
//! one draws. [`Focus::handle_tab`] resolves movement against *last*
//! frame's count, which is the best information available, and
//! [`register`](Focus::register) reclamps against the real count as it
//! goes. In the one frame where the count actually shrinks under a stale
//! focus index, nothing matches it and focus is effectively nowhere for
//! that frame; the next call to `handle_tab` clamps the stale index into
//! the new, smaller count and focus recovers. A retained widget tree would
//! not have this wrinkle — it is the one-frame price of not having one, and
//! in practice imperceptible: field counts rarely change between frames,
//! and when they do it is because the screen just changed, which draws the
//! user's attention away from wherever focus was anyway.
//!
//! # Arrow keys are deliberately not handled by default
//!
//! Tab is unambiguous: nothing else in this toolkit gives it meaning.
//! Arrow keys are not — [`TextFieldState`](crate::TextFieldState)'s
//! Left/Right move the caret, [`TableState`](crate::TableState)'s Up/Down
//! move row selection — so `Focus` never consumes them on its own.
//! [`Focus::handle_arrows`] is here for a screen that *wants* arrows to move
//! between controls (a settings list with no text entry, say); a widget
//! that wants arrows to move among its own items only — a radio group's
//! options — is expected to do that itself, scoped to its own count, the
//! way [`Ui::tabs`](crate::widget::Ui::tabs) already owns its own selection
//! without going through this type at all.

use liminalkit_shell::{KeyInput, Modifiers};

/// Which registered widget (by draw order) currently holds focus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Focus {
    current: Option<usize>,
    /// The previous frame's total registrations — what movement is resolved
    /// against, since this frame's total is not known until it is over.
    count: usize,
    /// This frame's registrations so far.
    registered: usize,
}

impl Focus {
    pub fn new() -> Focus {
        Focus::default()
    }

    /// The index that currently has focus, if any.
    pub fn current(&self) -> Option<usize> {
        self.current
    }

    /// Move focus by `delta` registrations from last frame, wrapping at
    /// both ends. `advance(1)` is what Tab does; a screen (or a
    /// self-contained group of controls) that wants arrow keys to move
    /// between items calls this directly, or through
    /// [`handle_arrows`](Self::handle_arrows).
    pub fn advance(&mut self, delta: i32) {
        if self.count == 0 {
            self.current = None;
            return;
        }
        let count = self.count as i32;
        let next = match self.current {
            Some(index) => (index as i32 + delta).rem_euclid(count),
            // Matches `TableState::move_by`: arrowing into nothing focused
            // lands at whichever end the direction implies.
            None if delta >= 0 => 0,
            None => count - 1,
        };
        self.current = Some(next as usize);
    }

    /// Apply a frame's Tab / Shift-Tab presses, and start counting this
    /// frame's registrations. Call once, before the first
    /// [`register`](Self::register) of the frame.
    pub fn handle_tab(&mut self, keys: &[KeyInput], modifiers: Modifiers) {
        // Pull a stale index back into last frame's own bounds first, so a
        // field removed since then does not leave focus pointing miles past
        // the end of a much shorter list — the same clamp `Ui::tabs` applies
        // to its own selection index.
        // An explicit set before the first registration must survive this frame.
        if self.count > 0 {
            self.current = self.current.map(|index| index.min(self.count - 1));
        }
        for key in keys {
            if matches!(key, KeyInput::Tab) {
                self.advance(if modifiers.shift { -1 } else { 1 });
            }
        }
        self.registered = 0;
    }

    /// Move focus in response to arrow keys, for a screen where the arrows
    /// mean "the next/previous control" rather than something local to
    /// whichever one is focused. `next`/`previous` are typically
    /// `KeyInput::Down`/`KeyInput::Up` or `KeyInput::Right`/`KeyInput::Left`
    /// — the caller picks, because a vertical list and a horizontal
    /// segmented row disagree about which pair means "forward". Call
    /// alongside, not instead of, [`handle_tab`](Self::handle_tab): Tab
    /// should still work regardless of what the arrows do.
    pub fn handle_arrows(&mut self, keys: &[KeyInput], next: KeyInput, previous: KeyInput) {
        for key in keys {
            if key == &next {
                self.advance(1);
            } else if key == &previous {
                self.advance(-1);
            }
        }
    }

    /// Register one focusable widget for this frame, in draw order, and
    /// report whether it currently has focus. The screen passes the result
    /// straight to whatever `focused: bool` a widget function takes.
    pub fn register(&mut self) -> bool {
        let index = self.registered;
        self.registered += 1;
        self.count = self.registered;
        self.current == Some(index)
    }

    /// Register a control and acquire focus on a pointer press or an
    /// accessibility focus request, before the widget consumes its click.
    pub fn register_at(
        &mut self,
        input: &liminalkit_shell::Input,
        area: liminalkit_raster::Rect,
    ) -> bool {
        if input
            .press_position()
            .is_some_and(|(x, y)| area.contains(x, y))
            || input.focus_requested(area)
        {
            self.current = Some(self.registered);
        }
        self.register()
    }

    /// Give a specific registration focus outright — a click on a field
    /// should focus it whether or not Tab was ever pressed. `index` is
    /// whatever [`register`](Self::register) returned `true` for, last time
    /// this widget drew, or simply a position the caller already knows.
    pub fn set(&mut self, index: usize) {
        self.current = Some(index);
    }

    /// Drop focus entirely — Escape leaving a field, or a dialog closing.
    pub fn clear(&mut self) {
        self.current = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::default()
        }
    }

    /// Run one simulated frame: apply Tab handling, then register `count`
    /// widgets, returning which indices reported themselves focused.
    fn frame(
        focus: &mut Focus,
        keys: &[KeyInput],
        modifiers: Modifiers,
        count: usize,
    ) -> Vec<usize> {
        focus.handle_tab(keys, modifiers);
        (0..count).filter(|_| focus.register()).collect()
    }

    #[test]
    fn nothing_is_focused_until_something_is_asked_for() {
        let mut focus = Focus::new();
        assert_eq!(focus.current(), None);
        assert!(!focus.register());
        assert!(!focus.register());
    }

    #[test]
    fn tab_focuses_the_first_widget_from_nothing_focused() {
        let mut focus = Focus::new();
        frame(&mut focus, &[], Modifiers::default(), 3); // establish the count
        assert_eq!(
            frame(&mut focus, &[KeyInput::Tab], Modifiers::default(), 3),
            vec![0]
        );
    }

    #[test]
    fn shift_tab_focuses_the_last_widget_from_nothing_focused() {
        let mut focus = Focus::new();
        frame(&mut focus, &[], Modifiers::default(), 3);
        assert_eq!(frame(&mut focus, &[KeyInput::Tab], shift(), 3), vec![2]);
    }

    #[test]
    fn repeated_tabs_step_through_every_widget_and_wrap() {
        let mut focus = Focus::new();
        frame(&mut focus, &[], Modifiers::default(), 3);
        assert_eq!(
            frame(&mut focus, &[KeyInput::Tab], Modifiers::default(), 3),
            vec![0]
        );
        assert_eq!(
            frame(&mut focus, &[KeyInput::Tab], Modifiers::default(), 3),
            vec![1]
        );
        assert_eq!(
            frame(&mut focus, &[KeyInput::Tab], Modifiers::default(), 3),
            vec![2]
        );
        assert_eq!(
            frame(&mut focus, &[KeyInput::Tab], Modifiers::default(), 3),
            vec![0],
            "wraps back to the first"
        );
    }

    #[test]
    fn shift_tab_steps_backwards_and_wraps() {
        let mut focus = Focus::new();
        frame(&mut focus, &[], Modifiers::default(), 3);
        frame(&mut focus, &[KeyInput::Tab], Modifiers::default(), 3); // focus 0
        assert_eq!(
            frame(&mut focus, &[KeyInput::Tab], shift(), 3),
            vec![2],
            "backwards from the first wraps to the last"
        );
    }

    #[test]
    fn several_tabs_in_one_frame_all_apply() {
        // A key repeated faster than the screen redraws, same as
        // `navigate`'s equivalent test for table selection.
        let mut focus = Focus::new();
        frame(&mut focus, &[], Modifiers::default(), 5);
        assert_eq!(
            frame(
                &mut focus,
                &[KeyInput::Tab, KeyInput::Tab, KeyInput::Tab],
                Modifiers::default(),
                5
            ),
            vec![2]
        );
    }

    #[test]
    fn a_field_removed_since_last_frame_leaves_focus_nowhere_for_one_frame_then_it_recovers() {
        let mut focus = Focus::new();
        frame(&mut focus, &[], Modifiers::default(), 5);
        focus.set(4); // the last of the five

        // The screen now only draws two widgets.
        let seen = frame(&mut focus, &[], Modifiers::default(), 2);
        assert!(seen.is_empty(), "index 4 does not exist among 2 widgets");

        // The next frame's Tab handling clamps the stale index into the
        // count that frame actually established, and focus is valid again.
        let seen = frame(&mut focus, &[], Modifiers::default(), 2);
        assert_eq!(seen, vec![1], "clamped to the new last index");
    }

    #[test]
    fn advance_wraps_for_deltas_larger_than_the_count() {
        let mut focus = Focus::new();
        frame(&mut focus, &[], Modifiers::default(), 3);
        focus.set(0);
        focus.advance(10); // 10 mod 3 == 1
        assert_eq!(focus.current(), Some(1));
        focus.advance(-10); // back down, wrapping
        assert_eq!(focus.current(), Some(0));
    }

    #[test]
    fn arrow_keys_are_ignored_unless_a_screen_opts_in() {
        let mut focus = Focus::new();
        frame(&mut focus, &[], Modifiers::default(), 3);
        focus.handle_tab(&[], Modifiers::default());
        focus.handle_arrows(
            &[KeyInput::Down, KeyInput::Character('a')],
            KeyInput::Right,
            KeyInput::Left,
        );
        assert_eq!(
            focus.current(),
            None,
            "Down was not the key this screen chose"
        );
    }

    #[test]
    fn arrow_keys_move_focus_once_a_screen_wires_them_up() {
        let mut focus = Focus::new();
        frame(&mut focus, &[], Modifiers::default(), 3);
        focus.handle_tab(&[], Modifiers::default());
        focus.handle_arrows(&[KeyInput::Right], KeyInput::Right, KeyInput::Left);
        assert_eq!(focus.current(), Some(0));
        focus.handle_tab(&[], Modifiers::default());
        focus.handle_arrows(&[KeyInput::Left], KeyInput::Right, KeyInput::Left);
        assert_eq!(
            focus.current(),
            Some(2),
            "left from the first wraps to the last"
        );
    }

    #[test]
    fn set_and_clear_override_whatever_tab_would_have_done() {
        let mut focus = Focus::new();
        frame(&mut focus, &[], Modifiers::default(), 3);
        focus.set(1);
        assert_eq!(focus.current(), Some(1));
        focus.clear();
        assert_eq!(focus.current(), None);
    }

    #[test]
    fn with_no_focusable_widgets_at_all_nothing_panics() {
        let mut focus = Focus::new();
        focus.handle_tab(&[KeyInput::Tab], Modifiers::default());
        focus.handle_arrows(&[KeyInput::Down], KeyInput::Down, KeyInput::Up);
        assert_eq!(focus.current(), None);
    }
}
