//! When the KMS loop draws, apart from the I/O, so the policy is a test.
//!
//! Three reasons to draw: something happened (input, a wake), the
//! application's own deadline came due, or the application is animating —
//! and then the completed flip is the signal, one frame per vertical blank.
//! Never more than one frame in flight.

use std::time::{Duration, Instant};

/// A pending flip must complete or fail the host instead of sleeping forever.
const FLIP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy)]
enum FramePolicy {
    OnDemand,
    Continuous,
    Deadline { due: Instant, interval: Duration },
}
struct Submitted<T> {
    at: Instant,
    frame: T,
}

pub struct Pacer<T> {
    frame: Duration,
    dirty: bool,
    in_flight: Option<Submitted<T>>,
    policy: FramePolicy,
}

impl<T> Pacer<T> {
    pub fn new(refresh_hz: u32) -> Self {
        Self {
            frame: Duration::from_secs(1) / refresh_hz.clamp(1, 1000),
            dirty: true,
            in_flight: None,
            policy: FramePolicy::OnDemand,
        }
    }
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub fn schedule(&mut self, now: Instant, interval: Option<Duration>) {
        self.policy = match interval {
            None => FramePolicy::OnDemand,
            Some(interval) if interval <= self.frame * 3 / 2 => {
                if !matches!(self.policy, FramePolicy::Continuous) {
                    self.dirty = true;
                }
                FramePolicy::Continuous
            }
            Some(interval) => {
                let due = match self.policy {
                    FramePolicy::Deadline {
                        due,
                        interval: previous,
                    } if previous == interval => {
                        if now < due {
                            due
                        } else {
                            self.dirty = true;
                            now + interval
                        }
                    }
                    _ => now + interval,
                };
                FramePolicy::Deadline { due, interval }
            }
        };
    }
    pub fn should_render(&self) -> bool {
        self.dirty && self.in_flight.is_none()
    }

    /// Commit only after the display accepted submission. Timing ownership and
    /// the pending state move together, and duplicate submissions are rejected.
    pub fn rendered(&mut self, frame: T, now: Instant) {
        assert!(
            self.should_render(),
            "a frame can only be submitted when ready"
        );
        self.dirty = false;
        self.in_flight = Some(Submitted { at: now, frame });
    }
    pub fn flipped(&mut self) -> Option<T> {
        let submitted = self.in_flight.take()?;
        if matches!(self.policy, FramePolicy::Continuous) {
            self.dirty = true;
        }
        Some(submitted.frame)
    }
    pub fn stalled(&self, now: Instant) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|submitted| now.saturating_duration_since(submitted.at) >= FLIP_TIMEOUT)
    }
    pub fn timeout(&self, now: Instant) -> Option<Duration> {
        if self.should_render() {
            return Some(Duration::ZERO);
        }
        let deadline = match self.policy {
            FramePolicy::Deadline { due, .. } => Some(due.saturating_duration_since(now)),
            _ => None,
        };
        let flip = self.in_flight.as_ref().map(|submitted| {
            FLIP_TIMEOUT.saturating_sub(now.saturating_duration_since(submitted.at))
        });
        match (deadline, flip) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_sleeps_forever_and_input_draws_once() {
        let now = Instant::now();
        let mut p = Pacer::new(60);
        p.schedule(now, None);
        assert!(p.should_render(), "the first frame");
        p.rendered((), now);
        assert_eq!(
            p.timeout(now),
            Some(FLIP_TIMEOUT),
            "flip completion has a watchdog"
        );
        p.flipped();
        assert!(!p.should_render());
        assert_eq!(p.timeout(now), None, "idle: no wakeups at all");
        p.invalidate();
        assert_eq!(p.timeout(now), Some(Duration::ZERO));
    }

    #[test]
    fn input_during_a_flip_waits_for_it_and_never_stacks_frames() {
        let now = Instant::now();
        let mut p = Pacer::new(60);
        p.schedule(now, None);
        p.rendered((), now);
        p.invalidate();
        p.invalidate();
        assert!(!p.should_render(), "one frame in flight");
        p.flipped();
        assert!(p.should_render(), "drawn from the newest state");
        p.rendered((), now);
        p.flipped();
        assert!(!p.should_render(), "two inputs, one owed frame");
    }

    #[test]
    fn animation_is_paced_by_flips_not_a_timer() {
        let now = Instant::now();
        let mut p = Pacer::new(60);
        p.schedule(now, None);
        p.rendered((), now);
        p.flipped();
        p.schedule(now, Some(Duration::from_millis(16)));
        assert!(p.should_render());
        for _ in 0..3 {
            p.rendered((), now);
            assert!(!p.should_render());
            p.flipped();
            p.schedule(now, Some(Duration::from_millis(16)));
            assert!(p.should_render(), "every flip owes the next frame");
        }
        p.schedule(now, None);
        p.rendered((), now);
        p.flipped();
        assert!(!p.should_render(), "stops when the animation does");
    }

    #[test]
    fn long_intervals_are_deadlines() {
        let now = Instant::now();
        let mut p = Pacer::new(60);
        p.schedule(now, None);
        p.rendered((), now);
        p.flipped();
        p.schedule(now, Some(Duration::from_secs(25)));
        assert_eq!(p.timeout(now), Some(Duration::from_secs(25)));
        assert!(!p.should_render());
        let later = now + Duration::from_secs(25);
        p.schedule(later, Some(Duration::from_secs(25)));
        assert!(p.should_render(), "due");
        assert_eq!(p.timeout(later), Some(Duration::ZERO));
    }

    #[test]
    fn completion_owns_timing_once_and_missing_completion_has_a_deadline() {
        let now = Instant::now();
        let mut p = Pacer::new(60);
        p.rendered("frame timing", now);
        p.invalidate();
        assert!(!p.should_render());
        assert!(!p.stalled(now + FLIP_TIMEOUT - Duration::from_millis(1)));
        assert!(p.stalled(now + FLIP_TIMEOUT));
        assert_eq!(p.timeout(now + FLIP_TIMEOUT), Some(Duration::ZERO));
        assert_eq!(p.flipped(), Some("frame timing"));
        assert_eq!(p.flipped(), None);
        assert!(p.should_render());
        assert!(!p.stalled(now + FLIP_TIMEOUT));
    }

    #[test]
    fn changing_deadline_policy_replaces_the_previous_interval() {
        let now = Instant::now();
        let mut p = Pacer::new(60);
        p.rendered((), now);
        p.flipped();
        p.schedule(now, Some(Duration::from_secs(60)));
        p.schedule(now, Some(Duration::from_secs(1)));
        assert_eq!(p.timeout(now), Some(Duration::from_secs(1)));
    }
}
