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
    /// A second contact superseded single-contact recognition; capture continues.
    TransformStarted,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SingleMode {
    TapEligible,
    DragOnly,
    Dragging,
}

#[derive(Debug, Clone)]
struct SingleContact {
    id: TouchId,
    origin: (f64, f64),
    last: (f64, f64),
    started: Instant,
    last_time: Instant,
    mode: SingleMode,
    velocity: VelocityTracker,
}
impl SingleContact {
    fn new(event: TouchEvent, mode: SingleMode, config: GestureConfig) -> Self {
        let mut velocity = VelocityTracker::default();
        velocity.add(event.received_at, event.position, config.velocity_window);
        Self {
            id: event.id,
            origin: event.position,
            last: event.position,
            started: event.received_at,
            last_time: event.received_at,
            mode,
            velocity,
        }
    }
}

#[derive(Debug, Clone)]
enum Recognition {
    Idle,
    Single(SingleContact),
    Transform { device: u64, last_time: Instant },
}
impl Recognition {
    fn session(&self) -> Option<(u64, Instant)> {
        match self {
            Self::Idle => None,
            Self::Single(single) => Some((single.id.device, single.last_time)),
            Self::Transform { device, last_time } => Some((*device, *last_time)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GestureRecognizer {
    config: GestureConfig,
    contacts: TouchState,
    recognition: Recognition,
}
impl GestureRecognizer {
    pub fn new(config: GestureConfig) -> Result<Self, &'static str> {
        Ok(Self {
            config: config.validate()?,
            contacts: TouchState::default(),
            recognition: Recognition::Idle,
        })
    }
    pub fn config(&self) -> GestureConfig {
        self.config
    }
    pub fn active(&self) -> bool {
        !matches!(self.recognition, Recognition::Idle)
    }
    /// Call when a press catches a moving scroll: it must not also activate.
    pub fn suppress_tap(&mut self) {
        if let Recognition::Single(single) = &mut self.recognition {
            if single.mode == SingleMode::TapEligible {
                single.mode = SingleMode::DragOnly;
            }
        }
    }
    /// Cancel before replacing this recognizer or changing its coordinate scale.
    pub fn cancel(&mut self) -> Vec<Gesture> {
        let active = self.active();
        self.contacts = TouchState::default();
        self.recognition = Recognition::Idle;
        if active {
            vec![Gesture::Cancelled, Gesture::Released]
        } else {
            Vec::new()
        }
    }
    fn constrained(config: GestureConfig, p: (f64, f64)) -> (f64, f64) {
        match config.axis {
            DragAxis::Free => p,
            DragAxis::Horizontal => (p.0, 0.0),
            DragAxis::Vertical => (0.0, p.1),
        }
    }
    fn movement(
        config: GestureConfig,
        single: &mut SingleContact,
        event: TouchEvent,
        out: &mut Vec<Gesture>,
    ) {
        let total = (
            event.position.0 - single.origin.0,
            event.position.1 - single.origin.1,
        );
        if single.mode == SingleMode::TapEligible && total.0.hypot(total.1) > config.slop {
            single.mode = SingleMode::DragOnly;
        }
        let allowed = Self::constrained(config, total);
        if single.mode != SingleMode::Dragging && allowed.0.hypot(allowed.1) > config.slop {
            single.mode = SingleMode::Dragging;
            out.push(Gesture::DragStarted);
            // Consume the threshold without a jump at recognition.
            single.last = event.position;
        }
        if single.mode == SingleMode::Dragging {
            let delta = Self::constrained(
                config,
                (
                    event.position.0 - single.last.0,
                    event.position.1 - single.last.1,
                ),
            );
            if delta != (0.0, 0.0) {
                out.push(Gesture::Drag { delta });
            }
        }
        single.last = event.position;
        single.last_time = event.received_at;
        single
            .velocity
            .add(event.received_at, event.position, config.velocity_window);
    }
    pub fn update(&mut self, event: TouchEvent) -> Vec<Gesture> {
        let mut out = Vec::new();
        let known = self.contacts.contacts().iter().any(|c| c.id == event.id);
        let session = self.recognition.session();
        if session.is_some_and(|(device, _)| device != event.id.device) {
            return out;
        }
        if event.phase == TouchPhase::Cancelled && known {
            return self.cancel();
        }
        if !event.position.0.is_finite()
            || !event.position.1.is_finite()
            || session.is_some_and(|(_, time)| event.received_at < time)
        {
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
        let before = self.contacts.contacts().len();
        match event.phase {
            TouchPhase::Started => {
                self.contacts.update(event);
                if before == 0 {
                    self.recognition = Recognition::Single(SingleContact::new(
                        event,
                        SingleMode::TapEligible,
                        self.config,
                    ));
                    out.push(Gesture::Pressed {
                        position: event.position,
                    });
                } else {
                    if matches!(self.recognition, Recognition::Single(_)) {
                        out.push(Gesture::TransformStarted);
                    }
                    self.recognition = Recognition::Transform {
                        device: event.id.device,
                        last_time: event.received_at,
                    };
                }
            }
            TouchPhase::Moved => {
                let delta = self.contacts.update(event);
                match &mut self.recognition {
                    Recognition::Single(single) => {
                        Self::movement(self.config, single, event, &mut out)
                    }
                    Recognition::Transform { last_time, .. } => {
                        *last_time = event.received_at;
                        if let Some(delta) = delta {
                            out.push(Gesture::Transform(delta));
                        }
                    }
                    Recognition::Idle => unreachable!("known contact has a recognition session"),
                }
            }
            TouchPhase::Ended => {
                if let Recognition::Single(single) = &mut self.recognition {
                    // Stationary release must not manufacture a fresh velocity sample.
                    if event.position != single.last {
                        Self::movement(self.config, single, event, &mut out);
                    }
                    match single.mode {
                        SingleMode::Dragging => out.push(Gesture::DragEnded {
                            velocity: Self::constrained(
                                self.config,
                                single.velocity.estimate(event.received_at, self.config),
                            ),
                        }),
                        SingleMode::TapEligible
                            if event.received_at.duration_since(single.started)
                                <= self.config.tap_timeout =>
                        {
                            out.push(Gesture::Tap {
                                position: event.position,
                            });
                        }
                        _ => {}
                    }
                }
                self.contacts.update(event);
                self.recognition = match self.contacts.contacts() {
                    [] => {
                        out.push(Gesture::Released);
                        Recognition::Idle
                    }
                    [remaining] => {
                        let rebased = TouchEvent {
                            received_at: event.received_at,
                            ..*remaining
                        };
                        Recognition::Single(SingleContact::new(
                            rebased,
                            SingleMode::DragOnly,
                            self.config,
                        ))
                    }
                    _ => Recognition::Transform {
                        device: event.id.device,
                        last_time: event.received_at,
                    },
                };
            }
            TouchPhase::Cancelled => {}
        }
        out
    }
}
