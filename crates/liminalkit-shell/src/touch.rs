//! Host-independent touchscreen input. Positions are physical buffer pixels;
//! timestamps measure host receipt, never sensor or finger-to-photon latency.
use std::time::Instant;

use crate::{Input, MouseButton};

/// Contact IDs are unique within a device, and may be reused after release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TouchId {
    pub device: u64,
    pub contact: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchEvent {
    pub id: TouchId,
    pub phase: TouchPhase,
    pub position: (f64, f64),
    /// Normalized pressure, when supplied by the backend; not contact area.
    pub pressure: Option<f64>,
    pub received_at: Instant,
}

/// Affine calibration for custom backends: `(a*x + b*y + c, d*x + e*y + f)`.
/// Native winit coordinates are already mapped by the OS; use IDENTITY there.
/// For raw normalized input on a clockwise-rotated W×H buffer, for example,
/// use `[0, -(W-1), W-1, H-1, 0, 0]`. Configure calibration before contact tracking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchTransform(pub [f64; 6]);

impl TouchTransform {
    pub const IDENTITY: Self = Self([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

    /// Map position only, preserving identity, pressure, phase and timestamp.
    /// None indicates invalid coordinates/calibration. Always forward terminal
    /// events to your contact tracker, even when mapping fails.
    pub fn map(self, mut event: TouchEvent) -> Option<TouchEvent> {
        let [a, b, c, d, e, f] = self.0;
        let (x, y) = event.position;
        event.position = (a * x + b * y + c, d * x + e * y + f);
        (event.position.0.is_finite() && event.position.1.is_finite()).then_some(event)
    }
}

/// Incremental direct-manipulation change for contacts from one device.
/// Apply translation in physical pixels and multiply zoom by `scale`.
/// Contact-count changes rebase the gesture and produce no delta.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchDelta {
    pub translation: (f64, f64),
    pub scale: f64,
    pub center: (f64, f64),
}

/// Latest contact state, independent of frame rate. Feed every event, including
/// terminal events. No unbounded per-frame event queue or motion smoothing.
#[derive(Debug, Clone, Default)]
pub struct TouchState {
    contacts: Vec<TouchEvent>,
}

impl TouchState {
    pub fn contacts(&self) -> &[TouchEvent] {
        &self.contacts
    }

    /// Ignores orphan moves/releases, duplicate starts, nonfinite positions,
    /// and backwards timestamps. Terminal events always retire known contacts.
    pub fn update(&mut self, event: TouchEvent) -> Option<TouchDelta> {
        let index = self.contacts.iter().position(|c| c.id == event.id);
        if matches!(event.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            if let Some(index) = index {
                self.contacts.remove(index);
            }
            return None;
        }
        if !event.position.0.is_finite() || !event.position.1.is_finite() {
            return None;
        }
        match (event.phase, index) {
            (TouchPhase::Started, None) => {
                self.contacts.push(event);
                None
            }
            (TouchPhase::Moved, Some(index)) => {
                if event.received_at < self.contacts[index].received_at {
                    return None;
                }
                let before = self.geometry(event.id.device);
                self.contacts[index] = event;
                let after = self.geometry(event.id.device);
                Some(TouchDelta {
                    translation: (after.0 .0 - before.0 .0, after.0 .1 - before.0 .1),
                    scale: if before.1 > f64::EPSILON {
                        after.1 / before.1
                    } else {
                        1.0
                    },
                    center: after.0,
                })
            }
            _ => None,
        }
    }

    // RMS radius generalizes pinch to any number of contacts on this device.
    fn geometry(&self, device: u64) -> ((f64, f64), f64) {
        let contacts = || self.contacts.iter().filter(|c| c.id.device == device);
        let n = contacts().count() as f64;
        let center = contacts().fold((0.0, 0.0), |s, c| {
            (s.0 + c.position.0 / n, s.1 + c.position.1 / n)
        });
        let radius = contacts()
            .map(|c| {
                (c.position.0 - center.0)
                    .hypot(c.position.1 - center.1)
                    .powi(2)
                    / n
            })
            .sum::<f64>()
            .sqrt();
        (center, radius)
    }

    /// Drain as cancellations for focus loss, suspension or surface replacement.
    pub fn cancel(&mut self, now: Instant) -> impl Iterator<Item = TouchEvent> + '_ {
        self.contacts.drain(..).map(move |mut event| {
            event.phase = TouchPhase::Cancelled;
            event.received_at = now;
            event
        })
    }
}

/// Explicit compatibility bridge for press-activated widgets and drag handles.
/// The first finger owns the pointer until release; other fingers are never
/// promoted mid-gesture. Use a separate Input for touch if mouse input is also
/// routed by the platform (some systems synthesize mouse events from touch).
/// This adapter intentionally does not arbitrate scrolling versus button taps.
#[derive(Debug, Clone, Default)]
pub struct TouchPointer {
    state: TouchState,
    primary: Option<TouchId>,
}

impl TouchPointer {
    pub fn update(&mut self, input: &mut Input, event: TouchEvent) {
        let empty = self.state.contacts.is_empty();
        let known = self.state.contacts.iter().any(|c| c.id == event.id);
        let delta = self.state.update(event);
        if event.phase == TouchPhase::Started
            && !known
            && empty
            && self.state.contacts.iter().any(|c| c.id == event.id)
        {
            self.primary = Some(event.id);
            input.cursor_moved(event.position.0 as f32, event.position.1 as f32);
            input.mouse(MouseButton::Left, true);
        } else if self.primary == Some(event.id) {
            match event.phase {
                TouchPhase::Moved if delta.is_some() => {
                    input.cursor_moved(event.position.0 as f32, event.position.1 as f32)
                }
                TouchPhase::Ended => {
                    input.mouse(MouseButton::Left, false);
                    input.cursor_left();
                    self.primary = None;
                }
                TouchPhase::Cancelled => {
                    input.cancel_pointer();
                    self.primary = None;
                }
                _ => {}
            }
        }
    }

    pub fn cancel(&mut self, input: &mut Input) {
        self.state.contacts.clear();
        self.primary = None;
        input.cancel_pointer();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liminalkit_raster::Rect;
    use std::time::Duration;

    fn event(contact: u64, phase: TouchPhase, x: f64) -> TouchEvent {
        TouchEvent {
            id: TouchId { device: 0, contact },
            phase,
            position: (x, 10.0),
            pressure: None,
            received_at: Instant::now(),
        }
    }

    #[test]
    fn calibration_maps_rotation_and_preserves_metadata() {
        let mut raw = event(7, TouchPhase::Started, 0.25);
        raw.position.1 = 0.75;
        let mapped = TouchTransform([0.0, -799.0, 799.0, 1279.0, 0.0, 0.0])
            .map(raw)
            .unwrap();
        assert_eq!(mapped.position, (199.75, 319.75));
        assert_eq!(mapped.id, raw.id);
        assert_eq!(mapped.received_at, raw.received_at);
        assert_eq!(TouchTransform::IDENTITY.map(raw), Some(raw));
        assert!(TouchTransform([f64::NAN; 6]).map(raw).is_none());
    }

    #[test]
    fn identities_lifecycle_and_gesture_rebasing() {
        let mut state = TouchState::default();
        assert!(state.update(event(1, TouchPhase::Moved, 1.0)).is_none());
        state.update(event(1, TouchPhase::Started, 0.0));
        state.update(event(1, TouchPhase::Started, 90.0));
        assert_eq!(state.contacts()[0].position.0, 0.0);
        let delta = state.update(event(1, TouchPhase::Moved, 2.0)).unwrap();
        assert_eq!(delta.translation, (2.0, 0.0));
        assert_eq!(delta.scale, 1.0);
        assert!(state.update(event(2, TouchPhase::Started, 10.0)).is_none());
        let delta = state.update(event(2, TouchPhase::Moved, 18.0)).unwrap();
        assert_eq!(delta.translation, (4.0, 0.0));
        assert_eq!(delta.scale, 2.0);
        let mut other = event(1, TouchPhase::Started, 999.0);
        other.id.device = 1;
        state.update(other);
        assert_eq!(state.contacts().len(), 3);
        state.update(event(2, TouchPhase::Ended, 18.0));
        let delta = state.update(event(1, TouchPhase::Moved, 3.0)).unwrap();
        assert_eq!(delta.translation, (1.0, 0.0));
        assert_eq!(delta.scale, 1.0);
        assert_eq!(
            state
                .cancel(Instant::now())
                .filter(|e| e.phase == TouchPhase::Cancelled)
                .count(),
            2
        );
        assert!(state.contacts().is_empty());
    }

    #[test]
    fn invalid_and_stale_moves_do_not_corrupt_contacts() {
        let mut state = TouchState::default();
        let start = event(0, TouchPhase::Started, 2.0);
        state.update(start);
        let mut moved = start;
        moved.phase = TouchPhase::Moved;
        moved.received_at -= Duration::from_millis(1);
        assert!(state.update(moved).is_none());
        moved.received_at = start.received_at;
        moved.position.0 = f64::NAN;
        assert!(state.update(moved).is_none());
        assert_eq!(state.contacts()[0], start);
        moved.phase = TouchPhase::Cancelled;
        state.update(moved);
        assert!(state.contacts().is_empty());
    }

    #[test]
    fn quick_taps_survive_until_frame_but_cancellation_never_clicks() {
        let mut pointer = TouchPointer::default();
        let mut input = Input::new();
        let area = Rect::new(0, 0, 40, 40);
        pointer.update(&mut input, event(1, TouchPhase::Started, 10.0));
        pointer.update(&mut input, event(1, TouchPhase::Ended, 10.0));
        assert!(!input.is_held());
        assert_eq!(input.cursor(), None);
        assert!(input.take_click(area));
        pointer.update(&mut input, event(1, TouchPhase::Started, 10.0));
        pointer.update(&mut input, event(1, TouchPhase::Cancelled, 10.0));
        assert!(!input.take_click(area));
        assert!(!input.is_held());
    }

    #[test]
    fn secondary_fingers_never_steal_or_restart_pointer() {
        let mut pointer = TouchPointer::default();
        let mut input = Input::new();
        pointer.update(&mut input, event(1, TouchPhase::Started, 10.0));
        input.end_frame();
        pointer.update(&mut input, event(2, TouchPhase::Started, 20.0));
        pointer.update(&mut input, event(2, TouchPhase::Moved, 30.0));
        assert_eq!(input.cursor(), Some((10, 10)));
        pointer.update(&mut input, event(1, TouchPhase::Ended, 10.0));
        pointer.update(&mut input, event(3, TouchPhase::Started, 30.0));
        assert!(!input.is_held());
        assert_eq!(input.press_position(), None);
        pointer.cancel(&mut input);
        pointer.update(&mut input, event(3, TouchPhase::Started, 30.0));
        assert!(input.is_held());
    }
}
