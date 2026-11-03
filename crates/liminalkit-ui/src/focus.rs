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

/// Something a screen can name a focusable control by.
///
/// The default key is the draw-order index, which is what
/// [`Focus::register`] hands out. A screen whose controls come and go, or
/// move, names them with its own enum instead and registers through
/// [`Focus::register_id`]; focus then follows the name, not the slot.
pub trait FocusKey: Copy + Eq {}

impl<K: Copy + Eq> FocusKey for K {}

/// Which registered widget currently holds focus.
///
/// `K` is how a screen names its controls. With the default `usize` the
/// name is the draw-order index and [`register`](Focus::register) assigns
/// it. With a caller-supplied type, an application enum such as
/// `enum Field { Name, Email, Save }`, the screen passes the name to
/// [`register_id`](Focus::register_id), and focus stays with the named
/// control when a conditional control appears before it or the form is
/// reordered.
///
/// Both forms keep the same shape: one small value a screen owns, rebuilt
/// from what actually gets drawn, with no per-widget storage anywhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Focus<K: FocusKey = usize> {
    current: Option<K>,
    /// The previous frame's registrations in draw order, what movement is
    /// resolved against, since this frame's order is not known until it is
    /// over.
    order: Vec<K>,
    /// This frame's registrations so far.
    registering: Vec<K>,
    /// Where `current` sat in `order` the last time it was seen there, so a
    /// control that vanishes hands focus to whatever now sits in its slot.
    last_position: Option<usize>,
}

impl<K: FocusKey> Default for Focus<K> {
    fn default() -> Focus<K> {
        Focus {
            current: None,
            order: Vec::new(),
            registering: Vec::new(),
            last_position: None,
        }
    }
}

impl<K: FocusKey> Focus<K> {
    pub fn new() -> Focus<K> {
        Focus::default()
    }

    /// The key that currently has focus, if any.
    pub fn current(&self) -> Option<K> {
        self.current
    }

    /// Whether `key` is the focused control.
    pub fn is_focused(&self, key: K) -> bool {
        self.current == Some(key)
    }

    /// How many controls registered last frame.
    pub fn count(&self) -> usize {
        self.resolved().len()
    }

    /// The order movement resolves against: this frame's registrations once
    /// they have caught up with last frame's count, otherwise last frame's.
    /// Mid-frame, last frame's is the best information there is; after the
    /// last registration, this frame's is exact.
    fn resolved(&self) -> &[K] {
        if self.registering.len() >= self.order.len() {
            &self.registering
        } else {
            &self.order
        }
    }

    fn position_of(&self, key: K) -> Option<usize> {
        self.resolved().iter().position(|k| *k == key)
    }

    /// Move focus by `delta` registrations from last frame, wrapping at
    /// both ends. `advance(1)` is what Tab does; a screen (or a
    /// self-contained group of controls) that wants arrow keys to move
    /// between items calls this directly, or through
    /// [`handle_arrows`](Self::handle_arrows).
    pub fn advance(&mut self, delta: i32) {
        let order = self.resolved();
        if order.is_empty() {
            self.current = None;
            self.last_position = None;
            return;
        }
        let count = order.len() as i32;
        let position = self
            .current
            .and_then(|key| order.iter().position(|k| *k == key));
        let next = match position {
            Some(index) => (index as i32 + delta).rem_euclid(count),
            // Matches `TableState::move_by`: arrowing into nothing focused
            // lands at whichever end the direction implies.
            None if delta >= 0 => 0,
            None => count - 1,
        } as usize;
        self.current = Some(order[next]);
        self.last_position = Some(next);
    }

    /// Apply a frame's Tab / Shift-Tab presses, and start counting this
    /// frame's registrations. Call once, before the first
    /// [`register`](Self::register) or [`register_id`](Self::register_id)
    /// of the frame.
    pub fn handle_tab(&mut self, keys: &[KeyInput], modifiers: Modifiers) {
        // Close out the previous frame: what it drew is what Tab moves over.
        if !self.registering.is_empty() || self.order.is_empty() {
            self.order = std::mem::take(&mut self.registering);
        } else {
            self.registering.clear();
        }
        // A key that was focused but not drawn last frame hands focus to
        // whatever now occupies its slot, clamped into the new order, so a
        // removed field does not leave focus pointing at nothing for good.
        // An explicit `set` before the first registration must survive this.
        if let Some(key) = self.current {
            match self.position_of(key) {
                Some(index) => self.last_position = Some(index),
                None => {
                    if let (Some(position), false) = (self.last_position, self.order.is_empty()) {
                        let index = position.min(self.order.len() - 1);
                        self.current = Some(self.order[index]);
                        self.last_position = Some(index);
                    }
                }
            }
        }
        for key in keys {
            if matches!(key, KeyInput::Tab) {
                self.advance(if modifiers.shift { -1 } else { 1 });
            }
        }
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

    /// Register the control named `key` for this frame, in draw order, and
    /// report whether it currently has focus. The screen passes the result
    /// straight to whatever `focused: bool` a widget function takes.
    ///
    /// Registering the same key twice in a frame is a screen bug, and the
    /// second registration simply shares the first's focus.
    pub fn register_id(&mut self, key: K) -> bool {
        self.registering.push(key);
        self.current == Some(key)
    }

    /// [`register_id`](Self::register_id), acquiring focus on a pointer
    /// press inside `area` or an accessibility focus request, before the
    /// widget consumes its click.
    pub fn register_id_at(
        &mut self,
        key: K,
        input: &liminalkit_shell::Input,
        area: liminalkit_raster::Rect,
    ) -> bool {
        if input
            .press_position()
            .is_some_and(|(x, y)| area.contains(x, y))
            || input.focus_requested(area)
        {
            self.current = Some(key);
            self.last_position = Some(self.registering.len());
        }
        self.register_id(key)
    }

    /// Give a specific control focus outright — a click on a field should
    /// focus it whether or not Tab was ever pressed.
    pub fn set(&mut self, key: K) {
        self.current = Some(key);
        self.last_position = self.position_of(key).or(self.last_position);
    }

    /// Drop focus entirely — Escape leaving a field, or a dialog closing.
    pub fn clear(&mut self) {
        self.current = None;
        self.last_position = None;
    }

    /// Forget last frame's order as well as the current focus, for a page
    /// change where none of the old controls will be drawn again.
    pub fn reset(&mut self) {
        *self = Focus::default();
    }
}

impl Focus<usize> {
    /// Register one focusable widget for this frame, in draw order, and
    /// report whether it currently has focus. The draw-order position is
    /// the widget's name: the first control registered is `0`, the next
    /// `1`, and so on. A screen that wants names that survive reordering
    /// uses [`register_id`](Self::register_id) with its own key type.
    pub fn register(&mut self) -> bool {
        let index = self.registering.len();
        self.register_id(index)
    }

    /// Register a control and acquire focus on a pointer press or an
    /// accessibility focus request, before the widget consumes its click.
    pub fn register_at(
        &mut self,
        input: &liminalkit_shell::Input,
        area: liminalkit_raster::Rect,
    ) -> bool {
        let index = self.registering.len();
        self.register_id_at(index, input, area)
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
        let mut focus = Focus::<usize>::new();
        focus.handle_tab(&[KeyInput::Tab], Modifiers::default());
        focus.handle_arrows(&[KeyInput::Down], KeyInput::Down, KeyInput::Up);
        assert_eq!(focus.current(), None);
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Field {
        Name,
        Email,
        Extra,
        Save,
    }

    fn typed_frame(focus: &mut Focus<Field>, keys: &[KeyInput], fields: &[Field]) -> Vec<Field> {
        focus.handle_tab(keys, Modifiers::default());
        fields
            .iter()
            .copied()
            .filter(|field| focus.register_id(*field))
            .collect()
    }

    #[test]
    fn typed_focus_follows_the_name_when_a_control_appears_before_it() {
        let mut focus = Focus::<Field>::new();
        let form = [Field::Name, Field::Email, Field::Save];
        typed_frame(&mut focus, &[], &form);
        focus.set(Field::Email);
        assert_eq!(typed_frame(&mut focus, &[], &form), vec![Field::Email]);

        // A conditional control is inserted ahead of the focused one. A
        // positional index would now point at Extra; the name still points
        // at Email.
        let longer = [Field::Name, Field::Extra, Field::Email, Field::Save];
        assert_eq!(typed_frame(&mut focus, &[], &longer), vec![Field::Email]);
        // And Tab moves relative to the new order.
        assert_eq!(
            typed_frame(&mut focus, &[KeyInput::Tab], &longer),
            vec![Field::Save]
        );
    }

    #[test]
    fn typed_focus_survives_a_reorder() {
        let mut focus = Focus::<Field>::new();
        typed_frame(&mut focus, &[], &[Field::Name, Field::Email, Field::Save]);
        focus.set(Field::Save);
        assert_eq!(
            typed_frame(&mut focus, &[], &[Field::Save, Field::Name, Field::Email]),
            vec![Field::Save]
        );
        assert_eq!(
            typed_frame(
                &mut focus,
                &[KeyInput::Tab],
                &[Field::Save, Field::Name, Field::Email]
            ),
            vec![Field::Name],
            "Tab from Save goes to what follows it in the current order"
        );
    }

    #[test]
    fn a_removed_typed_control_hands_focus_to_whatever_took_its_slot() {
        let mut focus = Focus::<Field>::new();
        typed_frame(&mut focus, &[], &[Field::Name, Field::Extra, Field::Save]);
        focus.set(Field::Extra);
        typed_frame(&mut focus, &[], &[Field::Name, Field::Extra, Field::Save]);
        // Extra is gone this frame: nothing is focused for one frame...
        assert!(typed_frame(&mut focus, &[], &[Field::Name, Field::Save]).is_empty());
        // ...and the slot it occupied is where focus lands next.
        assert_eq!(
            typed_frame(&mut focus, &[], &[Field::Name, Field::Save]),
            vec![Field::Save]
        );
    }

    #[test]
    fn typed_tab_from_nothing_lands_on_the_first_and_shift_tab_on_the_last() {
        let mut focus = Focus::<Field>::new();
        let form = [Field::Name, Field::Email];
        typed_frame(&mut focus, &[], &form);
        assert_eq!(
            typed_frame(&mut focus, &[KeyInput::Tab], &form),
            vec![Field::Name]
        );
        focus.clear();
        focus.handle_tab(&[KeyInput::Tab], shift());
        assert_eq!(focus.current(), Some(Field::Email));
    }

    #[test]
    fn reset_forgets_the_old_order_as_well_as_the_focus() {
        let mut focus = Focus::<Field>::new();
        typed_frame(&mut focus, &[], &[Field::Name, Field::Email]);
        focus.set(Field::Email);
        focus.reset();
        assert_eq!(focus.current(), None);
        assert_eq!(focus.count(), 0);
        focus.handle_tab(&[KeyInput::Tab], Modifiers::default());
        assert_eq!(focus.current(), None, "nothing to move over yet");
    }
}
