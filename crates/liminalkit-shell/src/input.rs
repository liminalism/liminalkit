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

use crate::scroll::{GesturePhase, ScrollSettings, WheelSample};
use std::time::{Duration, Instant};

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

/// Which ways a scroll region can still move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CanScroll {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
}

impl CanScroll {
    /// Whether any of the ways `wanted` asks for is open.
    fn any_of(self, wanted: CanScroll) -> bool {
        (self.up && wanted.up)
            || (self.down && wanted.down)
            || (self.left && wanted.left)
            || (self.right && wanted.right)
    }
}

/// What a scroll region does with the wheel when it can go no further and
/// sits inside another that can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollBoundary {
    /// A roll that starts with the inner region at its end goes to the outer
    /// one; a roll that reaches the end part-way through stops there, so the
    /// page does not lurch when a list runs out under the wheel.
    #[default]
    Auto,
    /// The inner region keeps the wheel even at its end.
    Contain,
    /// Whatever the inner region cannot use goes to the outer one, at any
    /// point in a roll.
    Chain,
}

/// One scroll region as it declared itself while drawing.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ScrollRegion {
    area: liminalkit_raster::Rect,
    can: CanScroll,
    boundary: ScrollBoundary,
}

/// How long a pause in the wheel can be and still be the same roll, for
/// deciding who it belongs to.
const ROLL_GAP: Duration = Duration::from_millis(400);

/// What happened since the last frame.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// Where the cursor is, in buffer pixels. `None` if it has left the window.
    cursor: Option<(i32, i32)>,
    /// Whether the left button is down right now.
    held: bool,
    /// Whether the middle button is down right now.
    middle_held: bool,
    /// The cursor has left the window with a button still down. The drag it
    /// is part of goes on; the cursor stops counting as being anywhere once
    /// the button comes up.
    outside: bool,
    /// A middle press that no widget has claimed yet.
    pending_middle: Option<(i32, i32)>,
    /// A button that came up this frame, and where.
    released: Option<(MouseButton, (i32, i32))>,
    /// The pointer shape a control asked for while drawing this frame.
    cursor_request: Option<crate::CursorShape>,
    /// A press that no widget has claimed yet.
    pending_click: Option<(i32, i32)>,
    /// A right-click that no widget has claimed yet.
    pending_context: Option<(i32, i32)>,
    /// Wheel movement not yet consumed: every sample of the frame, each as
    /// the host reported it. Not a sum. Adding them up here threw away their
    /// timing and their units before anything that scrolls had seen them.
    wheel: Vec<WheelSample>,
    scroll_settings: ScrollSettings,
    /// The display scale, for turning a wheel line into physical pixels.
    scale: crate::Scale,
    /// The scroll regions drawn last frame, in the order they were drawn, and
    /// those drawn so far in this one. An outer region is drawn, and asks for
    /// the wheel, before the regions inside it exist this frame; last
    /// frame's list is how it knows they are there.
    regions_before: Vec<ScrollRegion>,
    regions: Vec<ScrollRegion>,
    /// The region the current roll of the wheel went to, and when.
    latched: Option<(liminalkit_raster::Rect, Duration)>,
    /// Movement an inner region could not use, for the region around it.
    leftover: Option<(liminalkit_raster::Rect, (f64, f64))>,
    /// When this frame is being drawn, if the host said.
    now: Option<Instant>,
    /// A control asked, while drawing this frame, for another to follow.
    frame_wanted: bool,
    /// The same request once the frame is over, for the host to read.
    frame_requested: bool,
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
        self.middle_held = false;
        if std::mem::take(&mut self.outside) {
            self.cursor = None;
        }
        self.modifiers = Modifiers::default();
    }
    pub fn focus_at(&mut self, x: i32, y: i32) {
        self.invalidate_interaction();
        self.focus_point = Some((x, y));
    }
    /// A modal input owner consumed the current accessibility focus request.
    pub fn clear_focus_request(&mut self) {
        self.focus_point = None;
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
        if !self.held && !self.middle_held {
            // A move with no button down can only come from inside.
            self.outside = false;
        }
    }

    /// Abort a pointer interaction without activating a pending press.
    pub fn cancel_pointer(&mut self) {
        self.cursor = None;
        self.held = false;
        self.middle_held = false;
        self.outside = false;
        self.pending_click = None;
        self.pending_context = None;
        self.pending_middle = None;
        self.released = None;
    }

    /// The cursor left the window.
    ///
    /// With no button down that is the end of it: nothing is hovered. With a
    /// button down a drag is under way, and the platform goes on reporting
    /// the pointer and, in the end, the release, for as long as the button
    /// is held; the window has the pointer captured. So the drag carries on
    /// outside the window: a scrollbar thumb can be dragged past the edge of
    /// the window it belongs to, as every scrollbar can. A drag that loses
    /// the pointer some other way is ended by [`Input::blur`] or
    /// [`Input::cancel_pointer`].
    pub fn cursor_left(&mut self) {
        if self.held || self.middle_held {
            self.outside = true;
        } else {
            self.cursor = None;
        }
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
            (MouseButton::Middle, true) => {
                self.middle_held = true;
                self.pending_middle = self.cursor;
            }
            (MouseButton::Middle, false) => self.middle_held = false,
            (MouseButton::Right, false) => {}
        }
        if !pressed {
            if let Some(at) = self.cursor {
                self.released = Some((button, at));
            }
            if self.outside && !self.held && !self.middle_held {
                // The drag that took the cursor outside is over, and the
                // cursor is not over anything in this window.
                self.outside = false;
                self.cursor = None;
            }
        }
    }

    /// Wheel movement already in physical pixels. For a host or a test that
    /// has no richer event to give; a host that has one calls
    /// [`Input::wheel`].
    pub fn scrolled(&mut self, dx: f32, dy: f32) {
        let at = self.wheel.last().map_or(Duration::ZERO, |last| last.at);
        self.wheel.push(WheelSample::from_pixels(
            f64::from(dx),
            f64::from(dy),
            GesturePhase::Moved,
            at,
        ));
    }

    /// The frame about to be drawn, and when. Called by the host before each
    /// frame that has a clock to offer.
    ///
    /// Nothing in the toolkit reads the system clock while it draws: a
    /// control that moves over time is told the time, so the same inputs at
    /// the same times draw the same frames, in a test as on a screen.
    pub fn begin_frame(&mut self, now: Instant) {
        self.now = Some(now);
    }

    /// One wheel or touchpad movement, as the host reported it.
    pub fn wheel(&mut self, sample: WheelSample) {
        self.wheel.push(sample);
    }

    /// The system's wheel settings, when the host has read them.
    pub fn set_scroll_settings(&mut self, settings: ScrollSettings) {
        self.scroll_settings = settings;
    }

    /// The display scale. A wheel line is a logical distance, so what it is
    /// worth in physical pixels depends on this.
    pub fn set_scale(&mut self, scale: crate::Scale) {
        self.scale = scale;
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
        self.pending_middle = None;
        self.released = None;
        self.focus_point = None;
        self.wheel.clear();
        self.regions_before = std::mem::take(&mut self.regions);
        self.frame_requested = std::mem::take(&mut self.frame_wanted);
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

    /// Whether the cursor is inside a rectangle. Never while a drag has
    /// taken it out of the window: its position is still reported then, for
    /// the drag, but it is not over anything.
    pub fn hovering(&self, area: liminalkit_raster::Rect) -> bool {
        !self.outside && self.cursor.is_some_and(|(x, y)| area.contains(x, y))
    }

    /// Whether the middle button is held down, wherever the cursor is.
    pub fn is_middle_held(&self) -> bool {
        self.middle_held
    }

    /// Where the unclaimed middle press is, if any, without claiming it.
    pub fn middle_press_position(&self) -> Option<(i32, i32)> {
        self.pending_middle
    }

    /// Claim a middle press inside `area`, if there is one to claim.
    pub fn take_middle_click(&mut self, area: liminalkit_raster::Rect) -> bool {
        match self.pending_middle {
            Some((x, y)) if area.contains(x, y) => {
                self.pending_middle = None;
                true
            }
            _ => false,
        }
    }

    /// A button that came up this frame, and where the cursor was. A drag
    /// that needs to know where it was dropped reads this; it is not
    /// claimed, because a release ends whatever was being dragged and
    /// belongs to nothing else.
    pub fn released(&self) -> Option<(MouseButton, (i32, i32))> {
        self.released
    }

    /// Ask for a pointer shape. The last control to ask in a frame wins,
    /// which is the one drawn on top; nothing asking leaves the choice to
    /// the screen.
    pub fn request_cursor(&mut self, shape: crate::CursorShape) {
        self.cursor_request = Some(shape);
    }

    /// The shape a control asked for since this was last called. For the
    /// host adapter, after the frame is drawn.
    pub fn take_cursor_request(&mut self) -> Option<crate::CursorShape> {
        self.cursor_request.take()
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
    /// Returns `(dx, dy)` in physical pixels and clears them, so a scroll
    /// over nested regions moves the innermost one rather than both.
    ///
    /// This is the simple form: a wheel line is worth the system default,
    /// and the result is `f32`. Something that scrolls content with lines of
    /// its own, or that must not lose a fraction of a pixel, calls
    /// [`Input::take_scroll_pixels`] or [`Input::consume_wheel`].
    pub fn take_scroll(&mut self, area: liminalkit_raster::Rect) -> (f32, f32) {
        let line = self
            .scroll_settings
            .default_line_px(f64::from(self.scale.0));
        let (dx, dy) =
            self.take_scroll_pixels(area, (line, line), (f64::from(area.w), f64::from(area.h)));
        (dx as f32, dy as f32)
    }

    /// Claim wheel movement over `area`, converted to physical pixels with
    /// the caller's own idea of a line and a page.
    ///
    /// `line_px` is what one wheel line is worth on each axis (a table passes
    /// its row height, so a notch moves whole rows); `page_px` is used when
    /// the system is set to scroll a page per notch. Nothing is rounded: the
    /// fraction of a pixel a touchpad reports is returned as a fraction.
    pub fn take_scroll_pixels(
        &mut self,
        area: liminalkit_raster::Rect,
        line_px: (f64, f64),
        page_px: (f64, f64),
    ) -> (f64, f64) {
        let settings = self.scroll_settings;
        self.consume_wheel(area, |samples| {
            samples.iter().fold((0.0, 0.0), |(dx, dy), sample| {
                let (x, y) = sample.pixels(&settings, line_px, page_px);
                (dx + x, dy + y)
            })
        })
        .unwrap_or((0.0, 0.0))
    }

    /// Claim this frame's wheel samples over `area`, one by one and
    /// unconverted, for a scroller that reads their timing and units itself.
    /// `None` when there are none or the cursor is elsewhere.
    pub fn consume_wheel<R>(
        &mut self,
        area: liminalkit_raster::Rect,
        consume: impl FnOnce(&[WheelSample]) -> R,
    ) -> Option<R> {
        if self.wheel.is_empty() || !self.hovering(area) {
            return None;
        }
        let result = consume(&self.wheel);
        self.wheel.clear();
        Some(result)
    }

    /// When this frame is being drawn. `None` when the host has not said,
    /// which a control that animates must treat as "finish at once": with no
    /// clock there is no later frame to finish in.
    pub fn now(&self) -> Option<Instant> {
        self.now
    }

    /// Ask for another frame after this one, because something drawn in it
    /// is still moving. The request lasts one frame; whatever is still
    /// moving in the next asks again. Nothing asking means the window goes
    /// back to sleeping until there is input.
    pub fn request_frame(&mut self) {
        self.frame_wanted = true;
    }

    /// Whether the frame just ended asked for another. For the host.
    pub fn frame_requested(&self) -> bool {
        self.frame_requested
    }

    /// Declare a scroll region and claim the wheel for it if the wheel is
    /// its to have.
    ///
    /// This is [`Input::consume_wheel`] for regions that may sit inside one
    /// another. The difficulty is order: an outer region draws first, and
    /// would take the wheel before the list inside it had been drawn at all.
    /// So every region that calls this is remembered for one frame, and an
    /// outer region looks at last frame's list for a region inside it under
    /// the cursor. If there is one, and it can still move the way the wheel
    /// is turning, the outer region leaves the wheel for it. `boundary`
    /// says what happens when the inner one cannot; see [`ScrollBoundary`].
    ///
    /// A region nobody has seen before (the first frame it is drawn) is not
    /// in the list, and the first region under the cursor to ask gets the
    /// wheel, as with `consume_wheel`.
    pub fn route_wheel<R>(
        &mut self,
        area: liminalkit_raster::Rect,
        can: CanScroll,
        boundary: ScrollBoundary,
        consume: impl FnOnce(&[WheelSample]) -> R,
    ) -> Option<R> {
        self.regions.push(ScrollRegion {
            area,
            can,
            boundary,
        });
        if self.wheel.is_empty() || !self.hovering(area) {
            return None;
        }
        let first = self.wheel[0].at;
        let rolling_in = |region: liminalkit_raster::Rect| {
            self.latched
                .is_some_and(|(held, at)| held == region && first.saturating_sub(at) <= ROLL_GAP)
        };
        // Which way the wheel is turning this frame. Positive is the wheel
        // turning away from the user: the view moves up, or left.
        let (dx, dy) = self
            .wheel
            .iter()
            .fold((0.0, 0.0), |(x, y), sample| (x + sample.x, y + sample.y));
        let wanted = CanScroll {
            up: dy > 0.0,
            down: dy < 0.0,
            left: dx > 0.0,
            right: dx < 0.0,
        };
        let cursor = self.cursor;
        let inner = self.regions_before.iter().rev().find(|region| {
            region.area != area
                && region.area.intersect(area) == region.area
                && cursor.is_some_and(|(x, y)| region.area.contains(x, y))
        });
        if let Some(inner) = inner {
            let leave_it = !rolling_in(area)
                && (rolling_in(inner.area)
                    || inner.boundary == ScrollBoundary::Contain
                    || inner.can.any_of(wanted));
            if leave_it {
                return None;
            }
        }
        let last = self.wheel.last().map_or(first, |sample| sample.at);
        self.latched = Some((area, last));
        let result = consume(&self.wheel);
        self.wheel.clear();
        Some(result)
    }

    /// Hand on movement a region could not use, in physical pixels in the
    /// direction a scroll offset moves, for the region around it to take
    /// next frame. Only a region whose boundary is [`ScrollBoundary::Chain`]
    /// does this.
    pub fn return_scroll(&mut self, from: liminalkit_raster::Rect, unused: (f64, f64)) {
        if unused != (0.0, 0.0) {
            self.leftover = Some((from, unused));
        }
    }

    /// Take movement handed on by a region inside `area`.
    pub fn take_returned_scroll(&mut self, area: liminalkit_raster::Rect) -> Option<(f64, f64)> {
        match self.leftover {
            Some((from, unused)) if from != area && from.intersect(area) == from => {
                self.leftover = None;
                Some(unused)
            }
            _ => None,
        }
    }

    /// This frame's unclaimed wheel samples, without claiming them.
    pub fn wheel_samples(&self) -> &[WheelSample] {
        &self.wheel
    }

    pub fn scroll_settings(&self) -> &ScrollSettings {
        &self.scroll_settings
    }

    pub fn scale(&self) -> crate::Scale {
        self.scale
    }

    /// Keys pressed this frame, for whatever has focus.
    pub fn keys(&self) -> &[KeyInput] {
        &self.keys
    }

    /// Claim the keys, so only the focused widget sees them.
    pub fn take_keys(&mut self) -> Vec<KeyInput> {
        std::mem::take(&mut self.keys)
    }

    /// Hand keys back that an input owner took early and did not use, so
    /// what it draws later can still see them. A modal overlay takes every
    /// key before the background draws and restores them for its own
    /// controls. Unlike [`key`](Self::key), this does not start a new
    /// interaction epoch: the keys are the same frame's keys, not new ones.
    pub fn restore_keys(&mut self, keys: Vec<KeyInput>) {
        if self.keys.is_empty() {
            self.keys = keys;
        } else {
            let mut keys = keys;
            keys.append(&mut self.keys);
            self.keys = keys;
        }
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
    fn a_drag_goes_on_outside_the_window_and_ends_at_the_release() {
        // The platform keeps reporting the pointer while a button is down,
        // so a thumb dragged past the window's edge keeps following. This
        // used to end the drag at the edge.
        let mut input = at(10.0, 10.0);
        input.mouse(MouseButton::Left, true);
        input.end_frame();
        input.cursor_left();
        input.cursor_moved(-40.0, 300.0);

        assert!(input.is_held(), "still dragging");
        assert_eq!(input.cursor(), Some((-40, 300)), "and still followed");
        assert!(
            !input.hovering(Rect::new(-100, 0, 200, 400)),
            "but over nothing"
        );

        input.mouse(MouseButton::Left, false);
        assert!(!input.is_held());
        assert_eq!(input.released(), Some((MouseButton::Left, (-40, 300))));
        assert_eq!(input.cursor(), None, "the drag over, the cursor is gone");
        input.end_frame();
        assert_eq!(input.released(), None, "a release is one frame's news");
    }

    #[test]
    fn leaving_with_no_button_down_ends_hover() {
        let mut input = at(10.0, 10.0);
        input.cursor_left();
        assert!(!input.hovering(Rect::new(0, 0, 20, 20)));
        assert_eq!(input.cursor(), None);
    }

    #[test]
    fn losing_focus_ends_a_drag_that_was_outside() {
        // The one way a captured drag ends without a release.
        let mut input = at(10.0, 10.0);
        input.mouse(MouseButton::Left, true);
        input.cursor_left();
        input.cursor_moved(500.0, 500.0);
        input.blur();
        assert!(!input.is_held());
        assert_eq!(input.cursor(), None);
        // Coming back in works as it always did.
        input.cursor_moved(5.0, 5.0);
        assert!(input.hovering(Rect::new(0, 0, 20, 20)));
    }

    #[test]
    fn the_middle_button_is_a_press_of_its_own() {
        let mut input = at(10.0, 10.0);
        let area = Rect::new(0, 0, 20, 20);
        input.mouse(MouseButton::Middle, true);
        assert!(input.is_middle_held());
        assert!(!input.is_held(), "not the left button");
        assert!(!input.take_click(area));
        assert_eq!(input.middle_press_position(), Some((10, 10)));
        assert!(input.take_middle_click(area));
        assert!(!input.take_middle_click(area), "claimed once");
        input.end_frame();
        assert!(
            input.is_middle_held(),
            "held is state, the press was an edge"
        );
        input.mouse(MouseButton::Middle, false);
        assert!(!input.is_middle_held());
        assert_eq!(input.released(), Some((MouseButton::Middle, (10, 10))));
    }

    #[test]
    fn a_control_can_ask_for_a_pointer_shape_and_the_last_to_ask_wins() {
        let mut input = Input::new();
        assert_eq!(input.take_cursor_request(), None);
        input.request_cursor(crate::CursorShape::Grab);
        input.request_cursor(crate::CursorShape::Grabbing);
        assert_eq!(
            input.take_cursor_request(),
            Some(crate::CursorShape::Grabbing)
        );
        assert_eq!(input.take_cursor_request(), None);
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
    fn wheel_samples_reach_a_scroller_one_by_one_with_their_timing() {
        // Summed, a slow notch and a fast burst look the same. Whatever
        // reads cadence needs each sample and when it came.
        let mut input = at(10.0, 10.0);
        for (ms, raw) in [(0_u64, -1.0_f32), (9, -0.25), (17, -0.25)] {
            input.wheel(WheelSample::from_lines(
                0.0,
                raw,
                true,
                GesturePhase::Moved,
                Duration::from_millis(ms),
            ));
        }
        let area = Rect::new(0, 0, 20, 20);
        let seen = input
            .consume_wheel(area, |samples| {
                samples
                    .iter()
                    .map(|sample| (sample.at.as_millis(), sample.y))
                    .collect::<Vec<_>>()
            })
            .expect("the cursor is over the area");
        assert_eq!(seen, vec![(0, -120.0), (9, -30.0), (17, -30.0)]);
        assert!(input.consume_wheel(area, |_| ()).is_none(), "claimed once");
    }

    #[test]
    fn a_fraction_of_a_notch_or_a_pixel_is_not_rounded_away() {
        let mut input = at(10.0, 10.0);
        let area = Rect::new(0, 0, 20, 20);
        // A quarter notch over 20 px rows: fifteen pixels, not zero and not
        // a whole notch.
        input.wheel(WheelSample::from_lines(
            0.0,
            -0.25,
            true,
            GesturePhase::Moved,
            Duration::ZERO,
        ));
        assert_eq!(
            input.take_scroll_pixels(area, (8.0, 20.0), (20.0, 20.0)),
            (0.0, -15.0)
        );
        // A touchpad moving a third of a pixel.
        input.wheel(WheelSample::from_pixels(
            0.0,
            -1.0 / 3.0,
            GesturePhase::Moved,
            Duration::ZERO,
        ));
        let (_, dy) = input.take_scroll_pixels(area, (8.0, 20.0), (20.0, 20.0));
        assert_eq!(dy, -1.0 / 3.0);
    }

    #[test]
    fn a_wheel_line_follows_the_display_scale_and_the_system_setting() {
        let area = Rect::new(0, 0, 20, 20);
        let notch = WheelSample::from_lines(0.0, -1.0, true, GesturePhase::Moved, Duration::ZERO);

        let mut input = at(10.0, 10.0);
        input.wheel(notch);
        let (_, at_one) = input.take_scroll(area);
        assert!((at_one + 100.0).abs() < 1e-3, "a hundred logical pixels");

        input.set_scale(crate::Scale(2.0));
        input.wheel(notch);
        let (_, at_two) = input.take_scroll(area);
        assert!((at_two + 200.0).abs() < 1e-3, "the same distance on screen");

        input.set_scroll_settings(ScrollSettings {
            wheel_lines: crate::scroll::WheelLines::Lines(1.0),
            ..ScrollSettings::default()
        });
        input.wheel(notch);
        let (_, one_line) = input.take_scroll(area);
        assert!((one_line + 200.0 / 3.0).abs() < 1e-3);
    }

    #[test]
    fn a_request_for_another_frame_lasts_exactly_one_frame() {
        // Something still moving asks again each frame. If the request
        // outlived the frame it was made in, a window would never go back to
        // sleep once anything had animated.
        let mut input = Input::new();
        assert!(!input.frame_requested());
        input.request_frame();
        assert!(!input.frame_requested(), "not until the frame is over");
        input.end_frame();
        assert!(input.frame_requested());
        input.end_frame();
        assert!(!input.frame_requested(), "nobody asked during that one");
    }

    #[test]
    fn the_frame_time_is_whatever_the_host_said_and_nothing_until_it_does() {
        let mut input = Input::new();
        assert_eq!(input.now(), None);
        let now = Instant::now();
        input.begin_frame(now);
        assert_eq!(input.now(), Some(now));
        input.end_frame();
        assert_eq!(input.now(), Some(now), "still the last frame's time");
    }

    fn notch_at(ms: u64, raw: f32) -> WheelSample {
        WheelSample::from_lines(
            0.0,
            raw,
            true,
            GesturePhase::Moved,
            Duration::from_millis(ms),
        )
    }

    const OUTER: Rect = Rect::new(0, 0, 100, 100);
    const INNER: Rect = Rect::new(10, 10, 40, 40);
    const FREE: CanScroll = CanScroll {
        up: true,
        down: true,
        left: false,
        right: false,
    };
    const AT_BOTTOM: CanScroll = CanScroll {
        up: true,
        down: false,
        left: false,
        right: false,
    };

    /// One frame of an outer region with a list inside it. Returns who got
    /// the wheel: `(outer, inner)`.
    fn nested_frame(input: &mut Input, inner: CanScroll, boundary: ScrollBoundary) -> (bool, bool) {
        let outer_got = input
            .route_wheel(OUTER, FREE, ScrollBoundary::Auto, |_| ())
            .is_some();
        let inner_got = input.route_wheel(INNER, inner, boundary, |_| ()).is_some();
        input.end_frame();
        (outer_got, inner_got)
    }

    #[test]
    fn the_wheel_goes_to_the_region_inside_even_though_the_outer_one_asks_first() {
        let mut input = at(20.0, 20.0);
        // The first frame nobody knows the inner region exists.
        nested_frame(&mut input, FREE, ScrollBoundary::Auto);
        input.wheel(notch_at(0, -1.0));
        assert_eq!(
            nested_frame(&mut input, FREE, ScrollBoundary::Auto),
            (false, true)
        );
        // Outside the inner region the outer one scrolls.
        input.cursor_moved(80.0, 80.0);
        input.wheel(notch_at(1_000, -1.0));
        assert_eq!(
            nested_frame(&mut input, FREE, ScrollBoundary::Auto),
            (true, false)
        );
    }

    #[test]
    fn a_roll_that_starts_at_the_end_of_a_list_scrolls_the_page_around_it() {
        let mut input = at(20.0, 20.0);
        nested_frame(&mut input, AT_BOTTOM, ScrollBoundary::Auto);
        input.wheel(notch_at(0, -1.0));
        assert_eq!(
            nested_frame(&mut input, AT_BOTTOM, ScrollBoundary::Auto),
            (true, false)
        );
        // Turning back within the same roll stays with the page: who a roll
        // belongs to does not change in the middle of it.
        input.wheel(notch_at(100, 1.0));
        assert_eq!(
            nested_frame(&mut input, AT_BOTTOM, ScrollBoundary::Auto),
            (true, false)
        );
    }

    #[test]
    fn a_roll_that_runs_out_of_list_stops_there_and_the_next_one_moves_the_page() {
        let mut input = at(20.0, 20.0);
        nested_frame(&mut input, FREE, ScrollBoundary::Auto);
        input.wheel(notch_at(0, -1.0));
        assert_eq!(
            nested_frame(&mut input, FREE, ScrollBoundary::Auto),
            (false, true)
        );
        // The list has reached its end; the roll goes on. The page must not
        // lurch: the wheel still belongs to the list.
        input.wheel(notch_at(100, -1.0));
        assert_eq!(
            nested_frame(&mut input, AT_BOTTOM, ScrollBoundary::Auto),
            (false, true)
        );
        // After a pause it is a new roll, and that one moves the page.
        input.wheel(notch_at(1_000, -1.0));
        assert_eq!(
            nested_frame(&mut input, AT_BOTTOM, ScrollBoundary::Auto),
            (true, false)
        );
    }

    #[test]
    fn a_region_can_keep_the_wheel_at_its_end_or_hand_it_on_at_once() {
        let mut contained = at(20.0, 20.0);
        nested_frame(&mut contained, AT_BOTTOM, ScrollBoundary::Contain);
        contained.wheel(notch_at(0, -1.0));
        assert_eq!(
            nested_frame(&mut contained, AT_BOTTOM, ScrollBoundary::Contain),
            (false, true)
        );

        // Chain: the inner region hands on what it could not use, and the
        // outer one picks it up the next frame.
        let mut chained = at(20.0, 20.0);
        chained.return_scroll(INNER, (0.0, 37.5));
        assert_eq!(chained.take_returned_scroll(INNER), None, "not its own");
        assert_eq!(chained.take_returned_scroll(OUTER), Some((0.0, 37.5)));
        assert_eq!(chained.take_returned_scroll(OUTER), None, "taken once");
    }

    #[test]
    fn a_region_seen_for_the_first_time_gets_the_wheel_if_it_asks_first() {
        // No list of regions yet: the first claimant under the cursor wins,
        // as it always has.
        let mut input = at(20.0, 20.0);
        input.wheel(notch_at(0, -1.0));
        assert!(input
            .route_wheel(OUTER, FREE, ScrollBoundary::Auto, |_| ())
            .is_some());
        assert!(input
            .route_wheel(INNER, FREE, ScrollBoundary::Auto, |_| ())
            .is_none());
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
