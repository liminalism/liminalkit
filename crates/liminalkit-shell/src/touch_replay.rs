//! EXPERIMENTAL / UNTESTED bounded touch recording. Replay uses relative times,
//! preserving IDs/phases and inter-event timing, without a wall-clock dependency.
//! Recreate gesture/physics state with the same configuration before each run.
use crate::{TouchEvent, TouchId, TouchPhase};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecordedTouch {
    pub at: Duration,
    pub id: TouchId,
    pub phase: TouchPhase,
    pub position: (f64, f64),
    pub pressure: Option<f64>,
}
impl RecordedTouch {
    pub fn event(self, epoch: Instant) -> Option<TouchEvent> {
        Some(TouchEvent {
            received_at: epoch.checked_add(self.at)?,
            id: self.id,
            phase: self.phase,
            position: self.position,
            pressure: self.pressure,
        })
    }
}
#[derive(Debug, Clone)]
pub struct TouchRecording {
    capacity: usize,
    epoch: Option<Instant>,
    events: Vec<RecordedTouch>,
    truncated: bool,
}
impl TouchRecording {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            epoch: None,
            events: Vec::new(),
            truncated: false,
        }
    }
    pub fn clear(&mut self) {
        self.epoch = None;
        self.events.clear();
        self.truncated = false;
    }
    /// False means the event was rejected. A full trace is marked truncated;
    /// never replay its possibly missing releases without final cancellation.
    pub fn record(&mut self, event: TouchEvent) -> bool {
        if self.events.len() >= self.capacity {
            self.truncated = true;
            return false;
        }
        if !event.position.0.is_finite() || !event.position.1.is_finite() {
            return false;
        }
        let epoch = *self.epoch.get_or_insert(event.received_at);
        let Some(at) = event.received_at.checked_duration_since(epoch) else {
            return false;
        };
        if self.events.last().is_some_and(|last| at < last.at) {
            return false;
        }
        self.events.push(RecordedTouch {
            at,
            id: event.id,
            phase: event.phase,
            position: event.position,
            pressure: event.pressure,
        });
        true
    }
    pub fn events(&self) -> &[RecordedTouch] {
        &self.events
    }
    pub fn truncated(&self) -> bool {
        self.truncated
    }
    pub fn duration(&self) -> Duration {
        self.events.last().map_or(Duration::ZERO, |e| e.at)
    }
}
/// Caller advances physics to each returned event time, delivers the event, then
/// advances to its display time. This avoids quantizing replay input to frames.
#[derive(Debug, Clone, Default)]
pub struct ReplayCursor {
    next: usize,
    elapsed: Duration,
}
impl ReplayCursor {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn due<'a>(
        &mut self,
        recording: &'a TouchRecording,
        elapsed: Duration,
    ) -> &'a [RecordedTouch] {
        if elapsed < self.elapsed {
            return &[];
        }
        self.elapsed = elapsed;
        let first = self.next.min(recording.events.len());
        self.next = first;
        while self.next < recording.events.len() && recording.events[self.next].at <= elapsed {
            self.next += 1;
        }
        &recording.events[first..self.next]
    }
    pub fn finished(&self, recording: &TouchRecording) -> bool {
        self.next >= recording.events.len()
    }
}
