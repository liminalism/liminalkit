//! When the KMS loop draws, apart from the I/O, so the policy is a test.
//!
//! Three reasons to draw: something happened (input, a wake), the
//! application's own deadline came due, or the application is animating —
//! and then the completed flip is the signal, one frame per vertical blank.
//! Never more than one frame in flight.

use std::time::{Duration, Instant};

pub struct Pacer {
    frame: Duration,
    dirty: bool,
    flip_pending: bool,
    /// The application's next deadline, for intervals longer than a frame.
    next_tick: Option<Instant>,
    /// The application asked for frames at (about) the refresh rate.
    continuous: bool,
}

impl Pacer {
    pub fn new(refresh_hz: u32) -> Pacer {
        Pacer {
            frame: Duration::from_secs(1) / refresh_hz.clamp(1, 1000),
            // The first frame replaces the black the display started with.
            dirty: true,
            flip_pending: false,
            next_tick: None,
            continuous: false,
        }
    }

    /// Something changed: draw when the display can take a frame.
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Take the application's `animation_interval` after its tick.
    pub fn schedule(&mut self, now: Instant, interval: Option<Duration>) {
        match interval {
            None => {
                self.continuous = false;
                self.next_tick = None;
            }
            // Anything up to a frame and a half is "every frame": the flip
            // paces it, not a timer that would beat against the refresh.
            Some(interval) if interval <= self.frame * 3 / 2 => {
                self.next_tick = None;
                if !self.continuous {
                    self.continuous = true;
                    self.dirty = true;
                }
            }
            Some(interval) => {
                self.continuous = false;
                match self.next_tick {
                    Some(due) if now < due => {}
                    Some(_) => {
                        self.dirty = true;
                        self.next_tick = Some(now + interval);
                    }
                    None => self.next_tick = Some(now + interval),
                }
            }
        }
    }

    pub fn should_render(&self) -> bool {
        self.dirty && !self.flip_pending
    }

    pub fn rendered(&mut self) {
        self.dirty = false;
        self.flip_pending = true;
    }

    /// The frame reached the glass.
    pub fn flipped(&mut self) {
        self.flip_pending = false;
        if self.continuous {
            self.dirty = true;
        }
    }

    /// How long `poll` may sleep: zero when a frame is owed and can be
    /// drawn, until the deadline when one is set, otherwise forever (`None`).
    /// While a flip is pending the display's own event ends the wait.
    pub fn timeout(&self, now: Instant) -> Option<Duration> {
        if self.should_render() {
            return Some(Duration::ZERO);
        }
        self.next_tick.map(|due| due.saturating_duration_since(now))
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
        p.rendered();
        assert_eq!(p.timeout(now), None, "waiting on the flip only");
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
        p.rendered();
        p.invalidate();
        p.invalidate();
        assert!(!p.should_render(), "one frame in flight");
        p.flipped();
        assert!(p.should_render(), "drawn from the newest state");
        p.rendered();
        p.flipped();
        assert!(!p.should_render(), "two inputs, one owed frame");
    }

    #[test]
    fn animation_is_paced_by_flips_not_a_timer() {
        let now = Instant::now();
        let mut p = Pacer::new(60);
        p.schedule(now, None);
        p.rendered();
        p.flipped();
        p.schedule(now, Some(Duration::from_millis(16)));
        assert!(p.should_render());
        for _ in 0..3 {
            p.rendered();
            assert!(!p.should_render());
            p.flipped();
            p.schedule(now, Some(Duration::from_millis(16)));
            assert!(p.should_render(), "every flip owes the next frame");
        }
        p.schedule(now, None);
        p.rendered();
        p.flipped();
        assert!(!p.should_render(), "stops when the animation does");
    }

    #[test]
    fn long_intervals_are_deadlines() {
        let now = Instant::now();
        let mut p = Pacer::new(60);
        p.schedule(now, None);
        p.rendered();
        p.flipped();
        p.schedule(now, Some(Duration::from_secs(25)));
        assert_eq!(p.timeout(now), Some(Duration::from_secs(25)));
        assert!(!p.should_render());
        let later = now + Duration::from_secs(25);
        p.schedule(later, Some(Duration::from_secs(25)));
        assert!(p.should_render(), "due");
        assert_eq!(p.timeout(later), Some(Duration::ZERO));
    }
}
