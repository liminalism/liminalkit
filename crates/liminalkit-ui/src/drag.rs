//! Drag and drop inside the application: a payload picked up from one
//! rectangle and dropped on another, with a ghost drawn after everything
//! else.
//!
//! One [`Drag`] per kind of thing that can be dragged, owned by the screen.
//! Every frame, in order: [`Drag::update`] first, before anything draws, so
//! a drag in progress is known before the controls under it decide what a
//! press means; [`Drag::source`] on each rectangle something can be picked
//! up from; [`Drag::target`] on each rectangle something can be dropped on;
//! and [`Drag::ghost`] last, so whatever is being carried sits above the
//! rest. A press that never travels past the threshold is left for the
//! control underneath, so a draggable row is still a clickable row.
//!
//! Drops from the operating system (files from a file manager) do not come
//! through here; they arrive on `PixelApp::on_file_drop`.

use liminalkit_raster::Rect;
use liminalkit_shell::Input;

/// Logical pixels a press must travel before it becomes a drag.
pub const DRAG_THRESHOLD: i32 = 6;

/// An in-app drag, carrying a `T`.
#[derive(Debug)]
pub struct Drag<T> {
    state: State<T>,
}

#[derive(Debug)]
enum State<T> {
    Idle,
    /// Pressed on a source; not yet moved far enough to count.
    Armed {
        payload: T,
        origin: (i32, i32),
    },
    Dragging {
        payload: T,
        origin: (i32, i32),
        position: (i32, i32),
    },
    /// Released this frame; whoever asks first takes it.
    Dropped {
        payload: T,
        position: (i32, i32),
    },
}

impl<T> Default for Drag<T> {
    fn default() -> Drag<T> {
        Drag { state: State::Idle }
    }
}

/// What a drop target saw this frame.
#[derive(Debug, PartialEq, Eq)]
pub struct DropOutcome<T> {
    /// A drag is in progress and the pointer is over the target: highlight it.
    pub hovering: bool,
    /// The payload was released over the target this frame.
    pub dropped: Option<T>,
}

impl<T> Drag<T> {
    pub fn new() -> Drag<T> {
        Drag::default()
    }

    /// Advance the drag from this frame's input. Call once, before drawing.
    /// `threshold` is in device pixels (scale [`DRAG_THRESHOLD`]).
    pub fn update(&mut self, input: &Input, threshold: i32) {
        let cursor = input.cursor();
        self.state = match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle | State::Dropped { .. } => State::Idle,
            State::Armed { payload, origin } => {
                if !input.is_held() {
                    State::Idle
                } else if let Some(position) = cursor.filter(|(x, y)| {
                    (x - origin.0).abs() >= threshold || (y - origin.1).abs() >= threshold
                }) {
                    State::Dragging {
                        payload,
                        origin,
                        position,
                    }
                } else {
                    State::Armed { payload, origin }
                }
            }
            State::Dragging {
                payload,
                origin,
                position,
            } => {
                let position = cursor.unwrap_or(position);
                if input.is_held() {
                    State::Dragging {
                        payload,
                        origin,
                        position,
                    }
                } else {
                    State::Dropped { payload, position }
                }
            }
        };
    }

    /// Arm a drag if the frame's press landed in `area` and nothing else is
    /// being dragged. The press is not taken: if it never becomes a drag,
    /// the control drawn here still gets its click. `payload` is built only
    /// when the press lands, so a list of a thousand rows builds nothing.
    pub fn source(&mut self, input: &Input, area: Rect, payload: impl FnOnce() -> T) {
        if !matches!(self.state, State::Idle) {
            return;
        }
        if let Some(origin) = input
            .press_position()
            .filter(|(x, y)| area.contains(*x, *y))
        {
            self.state = State::Armed {
                payload: payload(),
                origin,
            };
        }
    }

    /// Whether `area` is under a drag in progress, and the payload if it
    /// was dropped there this frame. The first target that contains the
    /// drop position takes it.
    pub fn target(&mut self, area: Rect) -> DropOutcome<T> {
        match &self.state {
            State::Dragging { position, .. } if area.contains(position.0, position.1) => {
                DropOutcome {
                    hovering: true,
                    dropped: None,
                }
            }
            State::Dropped { position, .. } if area.contains(position.0, position.1) => {
                let State::Dropped { payload, .. } =
                    std::mem::replace(&mut self.state, State::Idle)
                else {
                    unreachable!("matched above");
                };
                DropOutcome {
                    hovering: false,
                    dropped: Some(payload),
                }
            }
            _ => DropOutcome {
                hovering: false,
                dropped: None,
            },
        }
    }

    /// The payload being carried and the pointer's position, for drawing
    /// the ghost last. `None` unless a drag has passed the threshold.
    pub fn ghost(&self) -> Option<(&T, (i32, i32))> {
        match &self.state {
            State::Dragging {
                payload, position, ..
            } => Some((payload, *position)),
            _ => None,
        }
    }

    /// Where the drag started, while one is in progress.
    pub fn origin(&self) -> Option<(i32, i32)> {
        match &self.state {
            State::Dragging { origin, .. } => Some(*origin),
            _ => None,
        }
    }

    pub fn is_dragging(&self) -> bool {
        matches!(self.state, State::Dragging { .. })
    }

    /// Abandon whatever is in flight: the window lost focus, or the
    /// screen changed.
    pub fn cancel(&mut self) {
        self.state = State::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liminalkit_shell::MouseButton;

    fn press(input: &mut Input, x: f32, y: f32) {
        input.cursor_moved(x, y);
        input.mouse(MouseButton::Left, true);
    }

    fn frame(input: &mut Input) {
        input.end_frame();
    }

    const SOURCE: Rect = Rect::new(0, 0, 50, 20);
    const TARGET: Rect = Rect::new(100, 0, 50, 20);

    #[test]
    fn a_press_that_does_not_travel_is_left_for_the_control() {
        let mut input = Input::new();
        let mut drag = Drag::<u32>::new();
        press(&mut input, 10.0, 10.0);
        drag.update(&input, 6);
        drag.source(&input, SOURCE, || 7);
        assert!(input.take_click(SOURCE), "the press is still there to take");
        frame(&mut input);
        input.mouse(MouseButton::Left, false);
        drag.update(&input, 6);
        assert!(!drag.is_dragging());
        assert_eq!(drag.ghost(), None);
    }

    #[test]
    fn moving_past_the_threshold_starts_the_drag_and_release_over_a_target_drops_it() {
        let mut input = Input::new();
        let mut drag = Drag::<u32>::new();
        press(&mut input, 10.0, 10.0);
        drag.update(&input, 6);
        drag.source(&input, SOURCE, || 7);
        frame(&mut input);

        input.cursor_moved(12.0, 10.0);
        drag.update(&input, 6);
        assert!(!drag.is_dragging(), "two pixels is a wobble");
        frame(&mut input);

        input.cursor_moved(40.0, 10.0);
        drag.update(&input, 6);
        assert!(drag.is_dragging());
        assert_eq!(drag.ghost(), Some((&7, (40, 10))));
        assert_eq!(drag.origin(), Some((10, 10)));
        assert!(!drag.target(TARGET).hovering);
        frame(&mut input);

        input.cursor_moved(120.0, 10.0);
        drag.update(&input, 6);
        let outcome = drag.target(TARGET);
        assert!(outcome.hovering);
        assert_eq!(outcome.dropped, None);
        frame(&mut input);

        input.mouse(MouseButton::Left, false);
        drag.update(&input, 6);
        assert_eq!(drag.ghost(), None, "nothing is carried after release");
        let outcome = drag.target(TARGET);
        assert_eq!(outcome.dropped, Some(7));
        assert_eq!(drag.target(TARGET).dropped, None, "taken once");
        assert!(!drag.is_dragging());
    }

    #[test]
    fn a_release_away_from_any_target_drops_nothing_and_resets() {
        let mut input = Input::new();
        let mut drag = Drag::<u32>::new();
        press(&mut input, 10.0, 10.0);
        drag.update(&input, 6);
        drag.source(&input, SOURCE, || 7);
        frame(&mut input);
        input.cursor_moved(70.0, 70.0);
        drag.update(&input, 6);
        frame(&mut input);
        input.mouse(MouseButton::Left, false);
        drag.update(&input, 6);
        assert_eq!(drag.target(TARGET).dropped, None);
        drag.update(&input, 6);
        assert!(!drag.is_dragging());
        // A fresh press can start a new drag.
        press(&mut input, 10.0, 10.0);
        drag.update(&input, 6);
        drag.source(&input, SOURCE, || 8);
        assert!(matches!(drag.state, State::Armed { payload: 8, .. }));
    }

    #[test]
    fn a_second_source_cannot_steal_an_armed_drag() {
        let mut input = Input::new();
        let mut drag = Drag::<u32>::new();
        press(&mut input, 10.0, 10.0);
        drag.update(&input, 6);
        drag.source(&input, SOURCE, || 1);
        drag.source(&input, Rect::new(0, 0, 500, 500), || 2);
        assert!(matches!(drag.state, State::Armed { payload: 1, .. }));
    }
}
