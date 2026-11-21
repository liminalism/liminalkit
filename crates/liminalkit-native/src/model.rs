//! The platform-neutral half of native controls: what was declared, what the
//! application wants this frame, what the platform was last told, and the
//! events waiting to be read.
//!
//! Nothing here touches a window system, so every rule a backend relies on —
//! "placed this frame means visible", "a programmatic change is not an
//! event", "only differences are sent" — is tested on every platform.

use std::collections::VecDeque;

use liminalkit_raster::Rect;

/// A declared native control. Cheap to copy; valid for the run it was
/// declared in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlId(pub(crate) u32);

/// What kind of platform control a [`ControlId`] stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    /// A push button: reports [`NativeEvent::Clicked`].
    Button,
    /// A button that stays pressed while on — bold, italic, a panel switch.
    /// Reports [`NativeEvent::Toggled`].
    Toggle,
    /// A check box with a caption. Reports [`NativeEvent::Toggled`].
    Checkbox,
    /// A drop-down list of fixed choices. Reports [`NativeEvent::Selected`].
    Combo,
    /// A single-line text field. Reports [`NativeEvent::TextChanged`],
    /// [`NativeEvent::Submitted`] and [`NativeEvent::Cancelled`].
    Edit,
    /// Static text.
    Label,
}

/// Something the user did to a native control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeEvent {
    Clicked(ControlId),
    /// A toggle or check box changed; the new state.
    Toggled(ControlId, bool),
    /// A combo's selection changed; the new index.
    Selected(ControlId, Option<usize>),
    /// The user edited a text field; the whole new text.
    TextChanged(ControlId, String),
    /// Enter was pressed in a text field. Focus has returned to the canvas.
    Submitted(ControlId),
    /// Escape was pressed while a native control had focus. Focus has
    /// returned to the canvas.
    Cancelled(ControlId),
}

/// One instruction for a backend, produced by [`Model::end_frame`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Command {
    /// Move to `Rect` (physical pixels in the canvas) and make visible.
    Place(ControlId, Rect),
    Hide(ControlId),
    Enable(ControlId, bool),
    SetText(ControlId, String),
    SetChecked(ControlId, bool),
    Select(ControlId, Option<usize>),
    /// Give a control keyboard focus.
    Focus(ControlId),
    /// Return keyboard focus to the liminalkit canvas.
    FocusCanvas,
}

#[derive(Debug, Clone, PartialEq)]
struct Shown {
    rect: Option<Rect>,
    enabled: bool,
    text: String,
    checked: bool,
    selected: Option<usize>,
}

#[derive(Debug)]
struct Control {
    kind: ControlKind,
    items: Vec<String>,
    /// What the application wants. `rect` is `Some` once placed this frame.
    want: Shown,
    /// What the platform control was last told, or reported itself.
    have: Shown,
}

/// Everything a backend needs to create a control late and have it match.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) struct Snapshot {
    pub(crate) kind: ControlKind,
    pub(crate) items: Vec<String>,
    pub(crate) text: String,
    pub(crate) checked: bool,
    pub(crate) selected: Option<usize>,
    pub(crate) enabled: bool,
    pub(crate) rect: Option<Rect>,
}

/// Events older than this are dropped, oldest first, if nobody reads them.
const EVENT_LIMIT: usize = 1024;

#[derive(Debug, Default)]
pub(crate) struct Model {
    controls: Vec<Control>,
    events: VecDeque<NativeEvent>,
    focus: Option<Command>,
    /// Whether the host realises the controls on screen.
    pub(crate) native: bool,
    /// An event was read since the last check: the application may have
    /// changed state it has already drawn, so one more frame is owed.
    events_read: bool,
}

impl Model {
    pub(crate) fn declare(
        &mut self,
        kind: ControlKind,
        text: &str,
        items: &[&str],
        checked: bool,
        selected: Option<usize>,
    ) -> ControlId {
        let state = Shown {
            rect: None,
            enabled: true,
            text: text.to_owned(),
            checked,
            selected: selected.filter(|index| *index < items.len()),
        };
        self.controls.push(Control {
            kind,
            items: items.iter().map(|item| (*item).to_owned()).collect(),
            want: state.clone(),
            have: state,
        });
        ControlId(self.controls.len() as u32 - 1)
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn count(&self) -> usize {
        self.controls.len()
    }

    /// What the platform should show for a control it has not created yet:
    /// the state as of the last finished frame.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn snapshot(&self, id: ControlId) -> Option<Snapshot> {
        self.get(id).map(|control| Snapshot {
            kind: control.kind,
            items: control.items.clone(),
            text: control.have.text.clone(),
            checked: control.have.checked,
            selected: control.have.selected,
            enabled: control.have.enabled,
            rect: control.have.rect,
        })
    }

    fn get(&self, id: ControlId) -> Option<&Control> {
        self.controls.get(id.0 as usize)
    }

    fn get_mut(&mut self, id: ControlId) -> Option<&mut Control> {
        self.controls.get_mut(id.0 as usize)
    }

    pub(crate) fn kind(&self, id: ControlId) -> Option<ControlKind> {
        self.get(id).map(|control| control.kind)
    }

    pub(crate) fn items(&self, id: ControlId) -> Vec<String> {
        self.get(id)
            .map(|control| control.items.clone())
            .unwrap_or_default()
    }

    pub(crate) fn text(&self, id: ControlId) -> String {
        self.get(id)
            .map(|control| control.want.text.clone())
            .unwrap_or_default()
    }

    pub(crate) fn checked(&self, id: ControlId) -> bool {
        self.get(id).is_some_and(|control| control.want.checked)
    }

    pub(crate) fn selected(&self, id: ControlId) -> Option<usize> {
        self.get(id).and_then(|control| control.want.selected)
    }

    pub(crate) fn enabled(&self, id: ControlId) -> bool {
        self.get(id).is_some_and(|control| control.want.enabled)
    }

    pub(crate) fn place(&mut self, id: ControlId, rect: Rect) {
        if let Some(control) = self.get_mut(id) {
            control.want.rect = Some(rect);
        }
    }

    pub(crate) fn set_enabled(&mut self, id: ControlId, enabled: bool) {
        if let Some(control) = self.get_mut(id) {
            control.want.enabled = enabled;
        }
    }

    pub(crate) fn set_text(&mut self, id: ControlId, text: &str) {
        if let Some(control) = self.get_mut(id) {
            if control.want.text != text {
                control.want.text = text.to_owned();
            }
        }
    }

    pub(crate) fn set_checked(&mut self, id: ControlId, checked: bool) {
        if let Some(control) = self.get_mut(id) {
            control.want.checked = checked;
        }
    }

    pub(crate) fn set_selected(&mut self, id: ControlId, selected: Option<usize>) {
        if let Some(control) = self.get_mut(id) {
            control.want.selected = selected.filter(|index| *index < control.items.len());
        }
    }

    pub(crate) fn request_focus(&mut self, id: Option<ControlId>) {
        self.focus = Some(match id {
            Some(id) => Command::Focus(id),
            None => Command::FocusCanvas,
        });
    }

    fn push(&mut self, event: NativeEvent) {
        if self.events.len() >= EVENT_LIMIT {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    /// The user clicked, submitted or cancelled: nothing to reconcile.
    pub(crate) fn user_event(&mut self, event: NativeEvent) {
        self.push(event);
    }

    /// The platform control reports a new checked state.
    pub(crate) fn user_toggled(&mut self, id: ControlId, checked: bool) {
        let Some(control) = self.get_mut(id) else {
            return;
        };
        control.have.checked = checked;
        if control.want.checked != checked {
            control.want.checked = checked;
            self.push(NativeEvent::Toggled(id, checked));
        }
    }

    /// The platform control reports a new selection.
    pub(crate) fn user_selected(&mut self, id: ControlId, selected: Option<usize>) {
        let Some(control) = self.get_mut(id) else {
            return;
        };
        control.have.selected = selected;
        if control.want.selected != selected {
            control.want.selected = selected;
            self.push(NativeEvent::Selected(id, selected));
        }
    }

    /// The platform control reports its text. A report that only echoes what
    /// the application set is not an event, so setting a field's text while
    /// handling its change cannot loop.
    pub(crate) fn user_text(&mut self, id: ControlId, text: String) {
        let Some(control) = self.get_mut(id) else {
            return;
        };
        control.have.text.clone_from(&text);
        if control.want.text != text {
            control.want.text.clone_from(&text);
            self.push(NativeEvent::TextChanged(id, text));
        }
    }

    pub(crate) fn poll_event(&mut self) -> Option<NativeEvent> {
        let event = self.events.pop_front();
        self.events_read |= event.is_some();
        event
    }

    /// Remove and report the first event matching `wanted`.
    pub(crate) fn take_event(&mut self, wanted: impl Fn(&NativeEvent) -> bool) -> bool {
        let Some(index) = self.events.iter().position(wanted) else {
            return false;
        };
        self.events.remove(index);
        self.events_read = true;
        true
    }

    /// Whether an event was read since this was last asked.
    pub(crate) fn take_events_read(&mut self) -> bool {
        std::mem::take(&mut self.events_read)
    }

    /// A frame is about to be drawn: nothing is placed yet.
    pub(crate) fn begin_frame(&mut self) {
        for control in &mut self.controls {
            control.want.rect = None;
        }
    }

    /// The frame was drawn: what must change on the platform to match it.
    /// A control that was not placed during the frame is hidden.
    pub(crate) fn end_frame(&mut self) -> Vec<Command> {
        let mut commands = Vec::new();
        for (index, control) in self.controls.iter_mut().enumerate() {
            let id = ControlId(index as u32);
            let (want, have) = (&control.want, &mut control.have);
            // State first, so a control that appears this frame appears
            // already carrying its new text and selection.
            if want.text != have.text {
                commands.push(Command::SetText(id, want.text.clone()));
            }
            if want.checked != have.checked {
                commands.push(Command::SetChecked(id, want.checked));
            }
            if want.selected != have.selected {
                commands.push(Command::Select(id, want.selected));
            }
            if want.enabled != have.enabled {
                commands.push(Command::Enable(id, want.enabled));
            }
            if want.rect != have.rect {
                commands.push(match want.rect {
                    Some(rect) => Command::Place(id, rect),
                    None => Command::Hide(id),
                });
            }
            *have = want.clone();
        }
        commands.extend(self.focus.take());
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> (Model, ControlId, ControlId, ControlId, ControlId) {
        let mut model = Model::default();
        let button = model.declare(ControlKind::Button, "Save", &[], false, None);
        let toggle = model.declare(ControlKind::Toggle, "B", &[], false, None);
        let combo = model.declare(ControlKind::Combo, "", &["Body", "Subhead"], false, Some(0));
        let edit = model.declare(ControlKind::Edit, "", &[], false, None);
        (model, button, toggle, combo, edit)
    }

    #[test]
    fn a_control_is_hidden_until_placed_and_hidden_again_when_not() {
        let (mut model, button, ..) = model();
        model.begin_frame();
        assert!(model.end_frame().is_empty(), "nothing placed, nothing sent");

        let rect = Rect::new(4, 4, 80, 24);
        model.begin_frame();
        model.place(button, rect);
        assert_eq!(model.end_frame(), vec![Command::Place(button, rect)]);

        // The frame a prompt replaces the toolbar: the button is not placed.
        model.begin_frame();
        assert_eq!(model.end_frame(), vec![Command::Hide(button)]);
    }

    #[test]
    fn an_unchanged_frame_sends_nothing() {
        let (mut model, button, ..) = model();
        let rect = Rect::new(0, 0, 10, 10);
        for frame in 0..3 {
            model.begin_frame();
            model.place(button, rect);
            let commands = model.end_frame();
            assert_eq!(commands.is_empty(), frame > 0);
        }
    }

    #[test]
    fn a_resize_moves_a_control_once() {
        let (mut model, button, ..) = model();
        model.begin_frame();
        model.place(button, Rect::new(0, 0, 10, 10));
        model.end_frame();
        let moved = Rect::new(20, 0, 10, 10);
        model.begin_frame();
        model.place(button, moved);
        assert_eq!(model.end_frame(), vec![Command::Place(button, moved)]);
    }

    #[test]
    fn caret_driven_sync_reaches_the_platform_without_an_event() {
        let (mut model, _, toggle, combo, _) = model();
        // The caret moved into bold text under a Subhead paragraph.
        model.set_checked(toggle, true);
        model.set_selected(combo, Some(1));
        model.begin_frame();
        assert_eq!(
            model.end_frame(),
            vec![
                Command::SetChecked(toggle, true),
                Command::Select(combo, Some(1))
            ]
        );
        assert_eq!(
            model.poll_event(),
            None,
            "the application did it, not the user"
        );
    }

    #[test]
    fn a_user_change_is_an_event_and_is_not_sent_back() {
        let (mut model, _, toggle, combo, _) = model();
        model.user_toggled(toggle, true);
        model.user_selected(combo, Some(1));
        assert!(model.checked(toggle));
        assert_eq!(model.selected(combo), Some(1));
        assert_eq!(model.poll_event(), Some(NativeEvent::Toggled(toggle, true)));
        assert_eq!(
            model.poll_event(),
            Some(NativeEvent::Selected(combo, Some(1)))
        );
        model.begin_frame();
        assert!(
            model.end_frame().is_empty(),
            "the platform already shows it"
        );
    }

    #[test]
    fn setting_text_programmatically_does_not_echo_as_a_change() {
        let (mut model, .., edit) = model();
        model.set_text(edit, "chapter 2");
        model.begin_frame();
        assert_eq!(
            model.end_frame(),
            vec![Command::SetText(edit, "chapter 2".to_owned())]
        );
        // The platform notifies about the text it was just given.
        model.user_text(edit, "chapter 2".to_owned());
        assert_eq!(model.poll_event(), None);
        // Then the user types.
        model.user_text(edit, "chapter 23".to_owned());
        assert_eq!(
            model.poll_event(),
            Some(NativeEvent::TextChanged(edit, "chapter 23".to_owned()))
        );
        assert_eq!(model.text(edit), "chapter 23");
    }

    #[test]
    fn a_click_is_taken_once_and_leaves_other_events_alone() {
        let (mut model, button, toggle, ..) = model();
        model.user_toggled(toggle, true);
        model.user_event(NativeEvent::Clicked(button));
        let clicked = |event: &NativeEvent| *event == NativeEvent::Clicked(button);
        assert!(model.take_event(clicked));
        assert!(!model.take_event(clicked));
        assert_eq!(model.poll_event(), Some(NativeEvent::Toggled(toggle, true)));
    }

    #[test]
    fn reading_an_event_owes_one_more_frame() {
        let (mut model, button, ..) = model();
        assert!(!model.take_events_read());
        assert_eq!(model.poll_event(), None);
        assert!(!model.take_events_read(), "an empty poll read nothing");
        model.user_event(NativeEvent::Clicked(button));
        model.poll_event();
        assert!(model.take_events_read());
        assert!(!model.take_events_read(), "owed once");
    }

    #[test]
    fn unread_events_are_bounded() {
        let (mut model, button, ..) = model();
        for _ in 0..EVENT_LIMIT + 10 {
            model.user_event(NativeEvent::Clicked(button));
        }
        let mut count = 0;
        while model.poll_event().is_some() {
            count += 1;
        }
        assert_eq!(count, EVENT_LIMIT);
    }

    #[test]
    fn a_selection_outside_the_items_is_no_selection() {
        let (mut model, _, _, combo, _) = model();
        model.set_selected(combo, Some(7));
        assert_eq!(model.selected(combo), None);
    }

    #[test]
    fn focus_requests_are_sent_once_after_the_state() {
        let (mut model, .., edit) = model();
        model.request_focus(Some(edit));
        model.begin_frame();
        model.place(edit, Rect::new(0, 0, 100, 20));
        let commands = model.end_frame();
        assert_eq!(commands.last(), Some(&Command::Focus(edit)));
        model.begin_frame();
        model.place(edit, Rect::new(0, 0, 100, 20));
        assert!(model.end_frame().is_empty());
        model.request_focus(None);
        model.begin_frame();
        model.place(edit, Rect::new(0, 0, 100, 20));
        assert_eq!(model.end_frame(), vec![Command::FocusCanvas]);
    }

    #[test]
    fn an_unknown_id_is_ignored_everywhere() {
        let mut model = Model::default();
        let ghost = ControlId(9);
        model.place(ghost, Rect::new(0, 0, 1, 1));
        model.set_text(ghost, "x");
        model.user_toggled(ghost, true);
        assert_eq!(model.text(ghost), "");
        assert!(!model.checked(ghost));
        assert_eq!(model.kind(ghost), None);
        model.begin_frame();
        assert!(model.end_frame().is_empty());
    }
}
