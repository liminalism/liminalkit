//! A frame's worth of input.
//!
//! The shell delivers events one at a time as they arrive; a screen wants to
//! know, while it is drawing, what happened since it last drew. This collects
//! the one into the other.
//!
//! The distinction that matters is between **state** and **edges**. Whether a
//! button is held down is state: it stays true across frames and is true for
//! whoever asks. That the button went down *this frame* is an edge, and it must
//! be consumed by exactly one widget or a single click activates every button
//! it happens to be over. So [`Input::take_click`] takes the click rather than
//! reading it, and [`Input::end_frame`] clears whatever nothing took.

/// A key the host recognised, as the application sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyInput {
    /// A printable character, already normalised — numpad and top-row digits
    /// arrive identically, because a cashier should not have to care.
    Character(char),
    Enter,
    Escape,
    Backspace,
    /// A function key, by number.
    Function(u8),
    Left,
    Right,
    Up,
    Down,
    Tab,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
}

/// Which modifiers were held when an event arrived.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    /// Cmd on macOS, the Windows/Super key elsewhere. Named `logo` after
    /// winit's own `super_key()`, since "super" reads as a privilege level
    /// to most of this codebase's readers.
    pub logo: bool,
}

impl Modifiers {
    /// Nothing held. The common case, and worth naming so a caller reads as
    /// "a plain click" rather than "no modifiers".
    pub fn none(self) -> bool {
        !self.shift && !self.control && !self.alt && !self.logo
    }

    /// The platform's accelerator modifier for a keyboard shortcut: Cmd on
    /// macOS, Ctrl everywhere else. A screen wiring up ⌘N/Ctrl+N checks this
    /// instead of `control` so the shortcut is right on both platforms.
    pub fn accel(self) -> bool {
        if cfg!(target_os = "macos") {
            self.logo
        } else {
            self.control
        }
    }
}

/// A mouse button the host reports. Extra buttons on a gaming mouse are
/// ignored rather than guessed at, for the same reason an unknown key is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// A validated native accessibility action for one ordinary text control.
/// Password controls never accept this plain-text route.
#[derive(Clone)]
pub enum AccessibleTextEdit {
    SetValue(String),
    SetSelection { anchor: usize, focus: usize },
}

impl std::fmt::Debug for AccessibleTextEdit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SetValue(_) => formatter.write_str("SetValue { contents redacted }"),
            Self::SetSelection { anchor, focus } => formatter
                .debug_struct("SetSelection")
                .field("anchor", anchor)
                .field("focus", focus)
                .finish(),
        }
    }
}

impl Drop for AccessibleTextEdit {
    fn drop(&mut self) {
        if let Self::SetValue(value) = self {
            zeroize::Zeroize::zeroize(value);
        }
    }
}

/// What happened since the last frame.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// Where the cursor is, in buffer pixels. `None` if it has left the window.
    cursor: Option<(i32, i32)>,
    /// Whether the left button is down right now.
    held: bool,
    /// A press that no widget has claimed yet.
    pending_click: Option<(i32, i32)>,
    /// A right-click that no widget has claimed yet.
    pending_context: Option<(i32, i32)>,
    /// Wheel movement, in pixels, not yet consumed.
    scroll: (f32, f32),
    /// Keys pressed this frame, in order.
    keys: Vec<KeyInput>,
    modifiers: Modifiers,
    clipboard: Option<crate::clipboard::ClipboardWorker>,
    focus_point: Option<(i32, i32)>,
    composition: String,
    interaction_epoch: u64,
    accessible_text_edits: Vec<(liminalkit_raster::Rect, Option<AccessibleTextEdit>)>,
}

impl Input {
    pub fn new() -> Input {
        Input::default()
    }

    /// Attach an asynchronous system clipboard owner whose completion callback
    /// wakes this application's existing event loop.
    pub fn attach_clipboard(&mut self, clipboard: crate::clipboard::ClipboardWorker) {
        self.clipboard = Some(clipboard);
    }
    pub fn clipboard(&self) -> Option<&crate::clipboard::ClipboardWorker> {
        self.clipboard.as_ref()
    }
    /// Pending asynchronous edits belong to one uninterrupted focus interaction.
    pub fn interaction_epoch(&self) -> u64 {
        self.interaction_epoch
    }
    fn advance_interaction(&mut self) {
        self.interaction_epoch = self
            .interaction_epoch
            .checked_add(1)
            .expect("input epoch exhausted");
        zeroize::Zeroize::zeroize(&mut self.composition);
    }
    fn invalidate_interaction(&mut self) {
        self.advance_interaction();
        self.clear_accessible_text_edit();
    }
    /// Window focus was lost. Do not clear a field's text or its logical focus.
    pub fn blur(&mut self) {
        self.invalidate_interaction();
        self.end_frame();
        self.held = false;
        self.modifiers = Modifiers::default();
    }
    pub fn focus_at(&mut self, x: i32, y: i32) {
        self.invalidate_interaction();
        self.focus_point = Some((x, y));
    }
    pub fn focus_requested(&self, area: liminalkit_raster::Rect) -> bool {
        self.focus_point.is_some_and(|(x, y)| area.contains(x, y))
    }
    pub fn set_composition(&mut self, text: &str) {
        if self.composition != text {
            self.invalidate_interaction();
            self.composition.push_str(text);
        }
    }
    pub fn composition(&self) -> &str {
        &self.composition
    }

    /// Queue a live ordinary control's edit before drawing. Actions retain their
    /// order within a frame; a later selection must not replace an earlier value.
    /// Identity/layout changes must call `clear_accessible_text_edit`.
    pub fn set_accessible_text_edit(
        &mut self,
        area: liminalkit_raster::Rect,
        edit: AccessibleTextEdit,
    ) {
        self.advance_interaction();
        self.focus_point = Some((area.x + area.w / 2, area.y + area.h / 2));
        self.accessible_text_edits.push((area, Some(edit)));
    }

    /// Consume one matching edit, without moving other controls' queued payloads.
    pub fn take_accessible_text_edit(
        &mut self,
        area: liminalkit_raster::Rect,
    ) -> Option<AccessibleTextEdit> {
        self.accessible_text_edits
            .iter_mut()
            .find(|(target, edit)| *target == area && edit.is_some())
            .and_then(|(_, edit)| edit.take())
    }

    /// Retire actions before replacing a surface/control identity or its layout.
    pub fn clear_accessible_text_edit(&mut self) {
        self.accessible_text_edits.clear();
    }

    // --- what the shell calls -------------------------------------------

    pub fn cursor_moved(&mut self, x: f32, y: f32) {
        self.cursor = Some((x.round() as i32, y.round() as i32));
    }

    /// Abort a pointer interaction without activating a pending press.
    pub fn cancel_pointer(&mut self) {
        self.cursor = None;
        self.held = false;
        self.pending_click = None;
        self.pending_context = None;
    }

    pub fn cursor_left(&mut self) {
        self.cursor = None;
        // A drag that ends outside the window never sends a release, so
        // holding this true would leave every widget thinking the button is
        // still down.
        self.held = false;
    }

    pub fn mouse(&mut self, button: MouseButton, pressed: bool) {
        match (button, pressed) {
            (MouseButton::Left, true) => {
                self.invalidate_interaction();
                self.held = true;
                self.pending_click = self.cursor;
            }
            (MouseButton::Left, false) => self.held = false,
            (MouseButton::Right, true) => self.pending_context = self.cursor,
            _ => {}
        }
    }

    pub fn scrolled(&mut self, dx: f32, dy: f32) {
        self.scroll.0 += dx;
        self.scroll.1 += dy;
    }

    pub fn key(&mut self, key: KeyInput) {
        self.invalidate_interaction();
        self.keys.push(key);
    }

    pub fn modifiers_changed(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
    }

    /// Drop anything no widget claimed. Called after each frame is drawn.
    ///
    /// Unclaimed input is discarded rather than carried forward: a click on
    /// empty space is a click on empty space, and holding it until something
    /// happens to be drawn there next frame would make the interface act on
    /// intentions nobody had.
    pub fn end_frame(&mut self) {
        self.pending_click = None;
        self.pending_context = None;
        self.focus_point = None;
        self.scroll = (0.0, 0.0);
        self.keys.clear();
        self.clear_accessible_text_edit();
    }

    // --- what widgets ask -----------------------------------------------

    pub fn cursor(&self) -> Option<(i32, i32)> {
        self.cursor
    }

    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Whether the cursor is inside a rectangle.
    pub fn hovering(&self, area: liminalkit_raster::Rect) -> bool {
        self.cursor.is_some_and(|(x, y)| area.contains(x, y))
    }

    /// Whether the button is held with the cursor inside a rectangle — for a
    /// widget that wants to look pressed.
    pub fn pressing(&self, area: liminalkit_raster::Rect) -> bool {
        self.held && self.hovering(area)
    }

    /// Whether the left button is held down, wherever the cursor is. A drag
    /// starts on its handle and then leaves it; [`Input::pressing`] goes
    /// false at the handle's edge, this stays true until the release.
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Where the unclaimed left press is, if any. Peeking does not consume
    /// it: a pane divider at a junction, or a title bar under another
    /// window's content, looks first and only the topmost claimant calls
    /// [`Input::take_click`].
    pub fn press_position(&self) -> Option<(i32, i32)> {
        self.pending_click
    }

    /// Claim a click inside `area`, if there is one to claim.
    ///
    /// Taking rather than reading is the whole design: a click belongs to one
    /// widget. Two overlapping widgets both reading it would both fire, and
    /// the second would usually be the one drawn underneath.
    pub fn take_click(&mut self, area: liminalkit_raster::Rect) -> bool {
        match self.pending_click {
            Some((x, y)) if area.contains(x, y) => {
                self.pending_click = None;
                true
            }
            _ => false,
        }
    }

    pub fn take_context_click(&mut self, area: liminalkit_raster::Rect) -> bool {
        match self.pending_context {
            Some((x, y)) if area.contains(x, y) => {
                self.pending_context = None;
                true
            }
            _ => false,
        }
    }

    /// Claim wheel movement over `area`.
    ///
    /// Returns `(dx, dy)` and clears them, so a scroll over nested regions
    /// moves the innermost one rather than both.
    pub fn take_scroll(&mut self, area: liminalkit_raster::Rect) -> (f32, f32) {
        if self.scroll == (0.0, 0.0) || !self.hovering(area) {
            return (0.0, 0.0);
        }
        std::mem::replace(&mut self.scroll, (0.0, 0.0))
    }

    /// Keys pressed this frame, for whatever has focus.
    pub fn keys(&self) -> &[KeyInput] {
        &self.keys
    }

    /// Claim the keys, so only the focused widget sees them.
    pub fn take_keys(&mut self) -> Vec<KeyInput> {
        std::mem::take(&mut self.keys)
    }

    /// Let one focused widget consume the key slice while retaining the
    /// collector's allocation for the next frame.
    ///
    /// `take_keys` is useful when ownership must leave the input object, but a
    /// text field only needs to inspect keys. Moving the `Vec` out there made
    /// every subsequent keystroke allocate a new buffer.
    pub fn consume_keys<R>(&mut self, consume: impl FnOnce(&[KeyInput]) -> R) -> R {
        let result = consume(&self.keys);
        self.keys.clear();
        result
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.composition);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liminalkit_raster::Rect;
    #[test]
    fn queued_typing_or_ime_edit_retires_paste_before_the_next_frame() {
        let mut input = Input::new();
        let submitted = input.interaction_epoch();
        input.key(KeyInput::Character('文'));
        assert_ne!(input.interaction_epoch(), submitted);
        let typed = input.interaction_epoch();
        input.set_composition("ไทย");
        assert_ne!(input.interaction_epoch(), typed);
        assert_eq!(input.composition(), "ไทย");
    }
    #[test]
    fn blur_retires_an_async_edit_even_if_focus_returns_before_a_frame() {
        let mut input = Input::new();
        input.set_composition("รหัส😀");
        input.modifiers_changed(Modifiers {
            control: true,
            ..Modifiers::default()
        });
        input.key(KeyInput::Character('v'));
        let submitted = input.interaction_epoch();
        input.blur();
        assert_ne!(input.interaction_epoch(), submitted);
        assert!(input.composition().is_empty());
        assert!(input.keys().is_empty());
        assert_eq!(input.modifiers(), Modifiers::default());
        let blurred = input.interaction_epoch();
        input.focus_at(10, 20);
        assert_ne!(input.interaction_epoch(), blurred);
        assert_ne!(input.interaction_epoch(), submitted);
    }

    fn at(x: f32, y: f32) -> Input {
        let mut input = Input::new();
        input.cursor_moved(x, y);
        input
    }

    #[test]
    fn a_click_belongs_to_exactly_one_widget() {
        // Two widgets overlapping the same point. Without claiming, both fire
        // and the one drawn underneath usually wins.
        let mut input = at(10.0, 10.0);
        input.mouse(MouseButton::Left, true);

        let over = Rect::new(0, 0, 20, 20);
        assert!(input.take_click(over), "the first claimant gets it");
        assert!(!input.take_click(over), "and there is nothing left to take");
    }

    #[test]
    fn a_click_outside_is_not_claimed_and_stays_available() {
        let mut input = at(50.0, 50.0);
        input.mouse(MouseButton::Left, true);

        assert!(!input.take_click(Rect::new(0, 0, 20, 20)));
        assert!(
            input.take_click(Rect::new(40, 40, 20, 20)),
            "a widget that does contain it can still claim it"
        );
    }

    #[test]
    fn unclaimed_input_does_not_survive_the_frame() {
        // A click on empty space is a click on empty space. Carrying it
        // forward would fire whatever happens to be drawn there next frame.
        let mut input = at(10.0, 10.0);
        input.mouse(MouseButton::Left, true);
        input.scrolled(0.0, -50.0);
        input.key(KeyInput::Enter);

        input.end_frame();

        assert!(!input.take_click(Rect::new(0, 0, 20, 20)));
        assert_eq!(input.take_scroll(Rect::new(0, 0, 20, 20)), (0.0, 0.0));
        assert!(input.keys().is_empty());
    }

    #[test]
    fn holding_the_button_persists_across_frames_but_the_click_does_not() {
        // Held is state; the press is an edge. A widget that wants to look
        // pressed asks the first, a widget that wants to act asks the second.
        let mut input = at(10.0, 10.0);
        input.mouse(MouseButton::Left, true);
        let area = Rect::new(0, 0, 20, 20);

        assert!(input.pressing(area));
        input.end_frame();
        assert!(input.pressing(area), "still held");
        assert!(!input.take_click(area), "but not clicked again");

        input.mouse(MouseButton::Left, false);
        assert!(!input.pressing(area));
    }

    #[test]
    fn a_drag_that_leaves_the_window_does_not_stay_held() {
        // No release arrives when the cursor leaves, so a widget would think
        // the button was down forever.
        let mut input = at(10.0, 10.0);
        input.mouse(MouseButton::Left, true);
        input.cursor_left();

        assert!(!input.pressing(Rect::new(0, 0, 20, 20)));
        assert!(!input.hovering(Rect::new(0, 0, 20, 20)));
        assert_eq!(input.cursor(), None);
    }

    #[test]
    fn scrolling_moves_the_region_under_the_cursor_and_only_that_one() {
        // Nested scrollable regions: the inner one claims the wheel, and the
        // outer one must not also move.
        let mut input = at(10.0, 10.0);
        input.scrolled(0.0, -48.0);

        let outer = Rect::new(0, 0, 100, 100);
        let inner = Rect::new(0, 0, 20, 20);

        assert_eq!(input.take_scroll(inner), (0.0, -48.0));
        assert_eq!(input.take_scroll(outer), (0.0, 0.0));
    }

    #[test]
    fn scrolling_somewhere_else_leaves_it_for_whoever_is_under_the_cursor() {
        let mut input = at(90.0, 90.0);
        input.scrolled(0.0, -48.0);
        assert_eq!(input.take_scroll(Rect::new(0, 0, 20, 20)), (0.0, 0.0));
        assert_eq!(input.take_scroll(Rect::new(80, 80, 40, 40)), (0.0, -48.0));
    }

    #[test]
    fn wheel_movement_within_a_frame_accumulates() {
        // A fast flick arrives as several events before anything redraws.
        let mut input = at(10.0, 10.0);
        input.scrolled(0.0, -10.0);
        input.scrolled(0.0, -15.0);
        assert_eq!(input.take_scroll(Rect::new(0, 0, 20, 20)), (0.0, -25.0));
    }

    #[test]
    fn keys_arrive_in_the_order_they_were_typed() {
        // A cashier types faster than the screen repaints, and an order is
        // not an order if the digits arrive shuffled.
        let mut input = Input::new();
        for character in "341".chars() {
            input.key(KeyInput::Character(character));
        }
        input.key(KeyInput::Enter);

        assert_eq!(
            input.keys(),
            &[
                KeyInput::Character('3'),
                KeyInput::Character('4'),
                KeyInput::Character('1'),
                KeyInput::Enter,
            ]
        );
        assert_eq!(input.take_keys().len(), 4);
        assert!(input.keys().is_empty(), "taken means taken");
    }

    #[test]
    fn a_focused_widget_can_consume_keys_without_leaving_any_for_another() {
        let mut input = Input::new();
        input.key(KeyInput::Character('4'));
        input.key(KeyInput::Enter);
        let seen = input.consume_keys(|keys| keys.to_vec());
        assert_eq!(
            seen,
            vec![KeyInput::Character('4'), KeyInput::Enter],
            "the widget sees the original order"
        );
        assert!(input.keys().is_empty(), "the keys were claimed");
    }

    #[test]
    fn a_right_click_is_separate_from_a_left_one() {
        let mut input = at(10.0, 10.0);
        let area = Rect::new(0, 0, 20, 20);
        input.mouse(MouseButton::Right, true);

        assert!(!input.take_click(area), "not a left click");
        assert!(input.take_context_click(area));
        assert!(!input.take_context_click(area));
    }

    #[test]
    fn a_press_lands_where_the_cursor_was_not_where_it_ends_up() {
        // The button goes down, then the cursor moves before the frame is
        // drawn. The click belongs to what was under it at press time.
        let mut input = at(10.0, 10.0);
        input.mouse(MouseButton::Left, true);
        input.cursor_moved(90.0, 90.0);

        assert!(input.take_click(Rect::new(0, 0, 20, 20)));
    }

    #[test]
    fn a_press_with_the_cursor_outside_the_window_claims_nothing() {
        let mut input = Input::new();
        input.mouse(MouseButton::Left, true);
        assert!(!input.take_click(Rect::new(0, 0, 1_000, 1_000)));
    }

    #[test]
    fn held_outlives_the_handle_a_drag_started_on() {
        // A divider drag leaves its grab zone on the first pixel of
        // movement. `pressing` would end it there; `is_held` follows the
        // button, not the cursor.
        let mut input = at(10.0, 10.0);
        input.mouse(MouseButton::Left, true);
        input.end_frame();
        input.cursor_moved(400.0, 400.0);

        assert!(input.is_held());
        assert!(!input.pressing(Rect::new(0, 0, 20, 20)));

        input.mouse(MouseButton::Left, false);
        assert!(!input.is_held());
    }

    #[test]
    fn peeking_at_a_press_leaves_it_claimable() {
        // Occluded chrome peeks to find the topmost claimant; the claim
        // itself still goes through `take_click`, exactly once.
        let mut input = at(10.0, 10.0);
        input.mouse(MouseButton::Left, true);

        assert_eq!(input.press_position(), Some((10, 10)));
        assert_eq!(input.press_position(), Some((10, 10)), "peeking is free");
        assert!(input.take_click(Rect::new(0, 0, 20, 20)));
        assert_eq!(input.press_position(), None, "claimed means gone");
    }

    #[test]
    fn a_plain_click_is_distinguishable_from_a_modified_one() {
        assert!(Modifiers::default().none());
        assert!(!Modifiers {
            shift: true,
            ..Modifiers::default()
        }
        .none());
        assert!(!Modifiers {
            logo: true,
            ..Modifiers::default()
        }
        .none());
    }

    #[test]
    fn accel_reads_logo_on_macos_and_control_elsewhere() {
        let ctrl_only = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        let logo_only = Modifiers {
            logo: true,
            ..Modifiers::default()
        };
        if cfg!(target_os = "macos") {
            assert!(logo_only.accel());
            assert!(!ctrl_only.accel());
        } else {
            assert!(ctrl_only.accel());
            assert!(!logo_only.accel());
        }
    }
}
