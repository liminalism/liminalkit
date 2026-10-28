//! EXPERIMENTAL / UNTESTED: original gesture policy, not hardware/feel validated.
//! No external gesture implementation is used. Coordinates and thresholds must
//! share units (prefer logical pixels; convert native events before delivery).
//! One recognizer belongs to one captured surface. Hit-test on Started, retain
//! that owner through release, and cancel before changing surface identity.
use crate::{TouchDelta, TouchEvent, TouchId, TouchPhase, TouchState};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragAxis {
    Free,
    Horizontal,
    Vertical,
}

/// Experimental defaults are tuning starting points, not calibrated values.
#[derive(Debug, Clone, Copy)]
pub struct GestureConfig {
    pub slop: f64,
    pub tap_timeout: Duration,
    pub velocity_window: Duration,
    pub pause_timeout: Duration,
    pub max_velocity: f64,
    pub axis: DragAxis,
}
impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            slop: 8.0,
            tap_timeout: Duration::from_millis(450),
            velocity_window: Duration::from_millis(100),
            pause_timeout: Duration::from_millis(60),
            max_velocity: 6000.0,
            axis: DragAxis::Free,
        }
    }
}
impl GestureConfig {
    pub fn validate(self) -> Result<Self, &'static str> {
        if !self.slop.is_finite()
            || self.slop < 0.0
            || !self.max_velocity.is_finite()
            || self.max_velocity <= 0.0
            || self.velocity_window.is_zero()
            || self.pause_timeout.is_zero()
            || self.tap_timeout.is_zero()
        {
            return Err("gesture thresholds must be finite and durations/velocity positive");
        }
        Ok(self)
    }
}

/// Output is ordered. Pressed is visual feedback only; only Tap activates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    Pressed {
        position: (f64, f64),
    },
    Tap {
        position: (f64, f64),
    },
    DragStarted,
    Drag {
        delta: (f64, f64),
    },
    DragEnded {
        velocity: (f64, f64),
    },
    Transform(TouchDelta),
    /// Retire pressed feedback and tap candidates; never fling on cancellation.
    Cancelled,
    /// All fingers lifted, including after a transform or rejected tap.
    Released,
}

/// Bounded recent-position least-squares estimate. Host receipt timestamps can
/// contain OS batching jitter; this is not a sensor-rate measurement.
#[derive(Debug, Clone, Default)]
pub struct VelocityTracker {
    samples: VecDeque<(Instant, (f64, f64))>,
}
impl VelocityTracker {
    pub fn clear(&mut self) {
        self.samples.clear();
    }
    pub fn add(&mut self, time: Instant, point: (f64, f64), window: Duration) {
        if !point.0.is_finite() || !point.1.is_finite() {
            return;
        }
        if let Some((last, _)) = self.samples.back() {
            if time < *last {
                return;
            }
            if time == *last {
                self.samples.pop_back();
            }
        }
        self.samples.push_back((time, point));
        while self.samples.len() > 32
            || self
                .samples
                .front()
                .is_some_and(|s| time.duration_since(s.0) > window)
        {
            self.samples.pop_front();
        }
    }
    pub fn estimate(&self, now: Instant, config: GestureConfig) -> (f64, f64) {
        let Some((last, _)) = self.samples.back() else {
            return (0.0, 0.0);
        };
        if now < *last || now.duration_since(*last) >= config.pause_timeout {
            return (0.0, 0.0);
        }
        let samples: Vec<_> = self
            .samples
            .iter()
            .filter(|s| now.duration_since(s.0) <= config.velocity_window)
            .collect();
        if samples.len() < 2 {
            return (0.0, 0.0);
        }
        let first = samples[0].0;
        let n = samples.len() as f64;
        let mean = samples.iter().fold((0.0, 0.0, 0.0), |s, p| {
            (
                s.0 + p.0.duration_since(first).as_secs_f64() / n,
                s.1 + p.1 .0 / n,
                s.2 + p.1 .1 / n,
            )
        });
        let sums = samples.iter().fold((0.0, 0.0, 0.0), |s, p| {
            let t = p.0.duration_since(first).as_secs_f64() - mean.0;
            (
                s.0 + t * t,
                s.1 + t * (p.1 .0 - mean.1),
                s.2 + t * (p.1 .1 - mean.2),
            )
        });
        if sums.0 < 1e-12 {
            return (0.0, 0.0);
        }
        let v = (sums.1 / sums.0, sums.2 / sums.0);
        let factor = (config.max_velocity / v.0.hypot(v.1)).min(1.0);
        (v.0 * factor, v.1 * factor)
    }
}

#[derive(Debug, Clone)]
pub struct GestureRecognizer {
    config: GestureConfig,
    contacts: TouchState,
    primary: Option<TouchId>,
    origin: (f64, f64),
    last: (f64, f64),
    started: Option<Instant>,
    last_time: Option<Instant>,
    dragging: bool,
    tap: bool,
    velocity: VelocityTracker,
}
impl GestureRecognizer {
    pub fn new(config: GestureConfig) -> Result<Self, &'static str> {
        Ok(Self {
            config: config.validate()?,
            contacts: TouchState::default(),
            primary: None,
            origin: (0.0, 0.0),
            last: (0.0, 0.0),
            started: None,
            last_time: None,
            dragging: false,
            tap: false,
            velocity: VelocityTracker::default(),
        })
    }
    pub fn config(&self) -> GestureConfig {
        self.config
    }
    pub fn active(&self) -> bool {
        !self.contacts.contacts().is_empty()
    }
    /// Call when a press catches a moving scroll: it must not also activate.
    pub fn suppress_tap(&mut self) {
        self.tap = false;
    }
    /// Cancel before replacing this recognizer or changing its coordinate scale.
    pub fn cancel(&mut self) -> Vec<Gesture> {
        let active = self.active();
        self.contacts = TouchState::default();
        self.primary = None;
        self.dragging = false;
        self.tap = false;
        self.velocity.clear();
        self.last_time = None;
        if active {
            vec![Gesture::Cancelled, Gesture::Released]
        } else {
            Vec::new()
        }
    }
    fn constrained(&self, p: (f64, f64)) -> (f64, f64) {
        match self.config.axis {
            DragAxis::Free => p,
            DragAxis::Horizontal => (p.0, 0.0),
            DragAxis::Vertical => (0.0, p.1),
        }
    }
    fn movement(&mut self, event: TouchEvent, out: &mut Vec<Gesture>) {
        let total = (
            event.position.0 - self.origin.0,
            event.position.1 - self.origin.1,
        );
        if total.0.hypot(total.1) > self.config.slop {
            self.tap = false;
        }
        let allowed = self.constrained(total);
        if !self.dragging && allowed.0.hypot(allowed.1) > self.config.slop {
            self.dragging = true;
            self.tap = false;
            out.push(Gesture::DragStarted);
            // Consume the threshold without a jump at recognition.
            self.last = event.position;
        }
        if self.dragging {
            let delta = self.constrained((
                event.position.0 - self.last.0,
                event.position.1 - self.last.1,
            ));
            if delta != (0.0, 0.0) {
                out.push(Gesture::Drag { delta });
            }
        }
        self.last = event.position;
        self.velocity.add(
            event.received_at,
            event.position,
            self.config.velocity_window,
        );
    }
    pub fn update(&mut self, event: TouchEvent) -> Vec<Gesture> {
        let mut out = Vec::new();
        let known = self.contacts.contacts().iter().any(|c| c.id == event.id);
        if self.primary.is_some_and(|id| id.device != event.id.device) {
            return out;
        }
        if event.phase == TouchPhase::Cancelled && known {
            return self.cancel();
        }
        if !event.position.0.is_finite() || !event.position.1.is_finite() {
            return if known && event.phase == TouchPhase::Ended {
                self.cancel()
            } else {
                out
            };
        }
        if self.last_time.is_some_and(|t| event.received_at < t) {
            return if known && event.phase == TouchPhase::Ended {
                self.cancel()
            } else {
                out
            };
        }
        if (event.phase == TouchPhase::Started && known)
            || (event.phase != TouchPhase::Started && !known)
        {
            return out;
        }
        self.last_time = Some(event.received_at);
        let before = self.contacts.contacts().len();
        match event.phase {
            TouchPhase::Started => {
                self.contacts.update(event);
                if before == 0 {
                    self.primary = Some(event.id);
                    self.origin = event.position;
                    self.last = event.position;
                    self.started = Some(event.received_at);
                    self.dragging = false;
                    self.tap = true;
                    self.velocity.clear();
                    self.velocity.add(
                        event.received_at,
                        event.position,
                        self.config.velocity_window,
                    );
                    out.push(Gesture::Pressed {
                        position: event.position,
                    });
                } else {
                    self.tap = false;
                    self.dragging = false;
                    self.velocity.clear();
                    out.push(Gesture::Cancelled);
                }
            }
            TouchPhase::Moved if before > 1 => {
                if let Some(delta) = self.contacts.update(event) {
                    out.push(Gesture::Transform(delta));
                }
            }
            TouchPhase::Moved => {
                self.contacts.update(event);
                self.movement(event, &mut out);
            }
            TouchPhase::Ended => {
                if before == 1 {
                    // Include a final changed coordinate, but do not manufacture a
                    // fresh velocity sample for a stationary release after a pause.
                    if event.position != self.last {
                        self.movement(event, &mut out);
                    }
                    if self.dragging {
                        out.push(Gesture::DragEnded {
                            velocity: self.constrained(
                                self.velocity.estimate(event.received_at, self.config),
                            ),
                        });
                    } else if self.tap
                        && self.started.is_some_and(|t| {
                            event.received_at.duration_since(t) <= self.config.tap_timeout
                        })
                    {
                        out.push(Gesture::Tap {
                            position: event.position,
                        });
                    }
                }
                self.contacts.update(event);
                self.tap = false;
                self.velocity.clear();
                self.dragging = false;
                if let Some(remaining) = self.contacts.contacts().first() {
                    self.primary = Some(remaining.id);
                    self.origin = remaining.position;
                    self.last = remaining.position;
                    self.velocity.add(
                        event.received_at,
                        remaining.position,
                        self.config.velocity_window,
                    );
                } else {
                    self.primary = None;
                    self.last_time = None;
                    out.push(Gesture::Released);
                }
            }
            TouchPhase::Cancelled => {}
        }
        out
    }
}
