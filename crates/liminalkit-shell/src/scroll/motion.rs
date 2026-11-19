//! Getting there: one axis of scroll position, moved over time.
//!
//! The interpreter says how far a wheel movement goes. A [`MotionController`]
//! holds a position and decides how it gets there: at once, or eased over a
//! few frames so that a notch reads as movement and not as a jump.
//!
//! The rule it keeps is that **distance is settled when the input arrives,
//! and time only decides when it is shown.** Each movement is added to a
//! *target* at the moment of its own timestamp; the position then closes on
//! the target by a formula in elapsed time, not by a step per frame. Two
//! things follow. Smoothing cannot change how far anything goes, only how it
//! looks on the way, so it can be tuned (or turned off) without touching the
//! acceleration. And the same input gives the same result whether frames
//! come at 60 a second, at 144, or irregularly, which is what lets a recorded
//! trace be replayed in a test.
//!
//! It is deliberately small. There is direct movement, a wheel target that is
//! chased, a timed animation to a chosen place, a fling that coasts to a
//! stop, and rules for what interrupts what. There is no overscroll: the
//! position stops at its ends. (`scroll_physics::ScrollAxis` has elastic
//! ends, for a touch surface that wants them.)
//!
//! The controller is one axis in any unit: pixels for a list, pages for a
//! paginated editor. It never rounds. Whoever draws rounds the position to
//! whatever it draws in.
//!
//! EXPERIMENTAL: the defaults have not been tuned on a screen.

use std::time::Duration;

/// How a wheel movement is shown arriving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Smoothing {
    /// At once.
    None,
    /// Closing on the target so that the remaining distance falls to about
    /// 37% in each `time`. Fastest at the start, so the first frame after a
    /// notch already shows movement.
    Exponential { time: Duration },
}

/// Where the distance went. Everything is in the controller's own unit and
/// counted as a magnitude.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Ledger {
    /// What wheel and direct movement asked for.
    pub requested: f64,
    /// What was added to the target.
    pub accepted: f64,
    /// What was refused because it would have passed an end. It is gone: it
    /// is not kept back to be shown later.
    pub clamped: f64,
    /// What was still owed when the direction reversed, and was dropped so
    /// that turning the wheel back stops the view where it is.
    pub reversed: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Idle,
    /// Closing on `target` after wheel input.
    Chase {
        time: f64,
    },
    /// A timed move from `from`, begun at `start`.
    Animate {
        from: f64,
        start: Duration,
        duration: f64,
    },
    /// Coasting from `from` since `start`, launched at `velocity` and slowed
    /// by `friction`.
    Fling {
        from: f64,
        start: Duration,
        velocity: f64,
        friction: f64,
    },
}

/// One axis of scroll position. See the module documentation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionController {
    position: f64,
    target: f64,
    min: f64,
    max: f64,
    mode: Mode,
    /// The time the position is correct for.
    clock: Duration,
    /// The fastest the position may move, in units a second.
    max_speed: Option<f64>,
    /// Within this of the target, the position is the target.
    rest: f64,
    ledger: Ledger,
}

impl Default for MotionController {
    fn default() -> MotionController {
        MotionController::new()
    }
}

impl MotionController {
    /// At zero, with no room to move until [`set_bounds`](Self::set_bounds)
    /// says there is.
    pub fn new() -> MotionController {
        MotionController {
            position: 0.0,
            target: 0.0,
            min: 0.0,
            max: 0.0,
            mode: Mode::Idle,
            clock: Duration::ZERO,
            max_speed: None,
            rest: 0.01,
            ledger: Ledger::default(),
        }
    }

    /// The fastest the position may be shown moving, in units a second, or
    /// `None` for no limit. A limit does not shorten the distance: what a
    /// burst of input asked for still arrives, a little later.
    pub fn set_max_speed(&mut self, speed: Option<f64>) {
        self.max_speed = speed.filter(|speed| speed.is_finite() && *speed > 0.0);
    }

    /// How close to the target counts as arrived. A hundredth of a unit by
    /// default, which suits pixels; something that scrolls in pages wants a
    /// much smaller number.
    pub fn set_rest(&mut self, rest: f64) {
        if rest.is_finite() && rest > 0.0 {
            self.rest = rest;
        }
    }

    /// The ends of the range. The position and the target are brought inside
    /// it, so content that shrinks under a scroll in progress does not leave
    /// the view past its new end.
    pub fn set_bounds(&mut self, min: f64, max: f64) {
        if !min.is_finite() || !max.is_finite() {
            return;
        }
        self.min = min;
        self.max = max.max(min);
        self.position = self.position.clamp(self.min, self.max);
        let target = self.target.clamp(self.min, self.max);
        if target != self.target {
            self.target = target;
            if let Mode::Animate { .. } = self.mode {
                // The place it was going no longer exists; go to the nearest
                // that does, from here, in the time that was left.
                self.mode = Mode::Chase { time: 0.05 };
            }
        }
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    /// Where the position is heading. Equal to it at rest. Something that
    /// prepares content ahead of the view (a renderer, a prefetcher) wants
    /// this, not the position.
    pub fn target(&self) -> f64 {
        self.target
    }

    pub fn bounds(&self) -> (f64, f64) {
        (self.min, self.max)
    }

    /// Whether the position is still moving, so that another frame is worth
    /// drawing.
    pub fn animating(&self) -> bool {
        self.mode != Mode::Idle
    }

    /// Units a second, signed, at the time last advanced to.
    pub fn velocity(&self) -> f64 {
        match self.mode {
            Mode::Idle => 0.0,
            Mode::Chase { time } => {
                let speed = (self.target - self.position) / time;
                match self.max_speed {
                    Some(limit) => speed.clamp(-limit, limit),
                    None => speed,
                }
            }
            Mode::Animate {
                from,
                start,
                duration,
            } => {
                let s = (self.clock.saturating_sub(start).as_secs_f64() / duration).min(1.0);
                3.0 * (self.target - from) * (1.0 - s) * (1.0 - s) / duration
            }
            Mode::Fling {
                start,
                velocity,
                friction,
                ..
            } => velocity * (-friction * self.clock.saturating_sub(start).as_secs_f64()).exp(),
        }
    }

    pub fn ledger(&self) -> Ledger {
        self.ledger
    }

    /// Move the position to where it should be at `now`. Time that runs
    /// backwards is ignored; a long gap (a stalled frame, a suspended
    /// window) simply finishes whatever was in progress.
    pub fn advance(&mut self, now: Duration) {
        let elapsed = now.saturating_sub(self.clock).as_secs_f64();
        self.clock = self.clock.max(now);
        match self.mode {
            Mode::Idle => {}
            Mode::Chase { time } => {
                let owed = self.target - self.position;
                let direction = owed.signum();
                let distance = owed.abs();
                // Above this distance the exponential would exceed the speed
                // limit, so the position runs at the limit until it is
                // within it and eases in from there. The speed is continuous
                // where the two meet.
                let knee = self.max_speed.map_or(f64::INFINITY, |limit| limit * time);
                let left = if distance > knee {
                    let limit = knee / time;
                    let at_limit = (distance - knee) / limit;
                    if elapsed <= at_limit {
                        distance - limit * elapsed
                    } else {
                        knee * (-(elapsed - at_limit) / time).exp()
                    }
                } else {
                    distance * (-elapsed / time).exp()
                };
                if left <= self.rest {
                    self.arrive();
                } else {
                    self.position = self.target - direction * left;
                }
            }
            Mode::Animate {
                from,
                start,
                duration,
            } => {
                let s = self.clock.saturating_sub(start).as_secs_f64() / duration;
                if s >= 1.0 {
                    self.arrive();
                } else {
                    let ease = 1.0 - (1.0 - s).powi(3);
                    self.position = from + (self.target - from) * ease;
                }
            }
            Mode::Fling {
                from,
                start,
                velocity,
                friction,
            } => {
                // Speed falls off exponentially, so the distance covered is
                // a formula in the time since launch and the whole coast is
                // known the moment it starts.
                let since = self.clock.saturating_sub(start).as_secs_f64();
                let reach = velocity / friction;
                let left = reach * (-friction * since).exp();
                let free = from + reach - left;
                if left.abs() <= self.rest || free <= self.min || free >= self.max {
                    // Come to rest, or run into an end: either way it stops
                    // where it is, with nothing held back.
                    self.position = if left.abs() <= self.rest {
                        self.target
                    } else {
                        free.clamp(self.min, self.max)
                    };
                    self.target = self.position;
                    self.mode = Mode::Idle;
                } else {
                    self.position = free;
                }
            }
        }
    }

    fn arrive(&mut self) {
        self.position = self.target;
        self.mode = Mode::Idle;
    }

    /// Add `delta` within the bounds, measured from `base`; what was
    /// accepted. The rest is counted as clamped and discarded.
    fn accept(&mut self, base: f64, delta: f64) -> f64 {
        let accepted = (base + delta).clamp(self.min, self.max) - base;
        self.ledger.requested += delta.abs();
        self.ledger.accepted += accepted.abs();
        self.ledger.clamped += (delta - accepted).abs();
        accepted
    }

    /// Wheel movement of `delta` that arrived at `at`. Returns what was
    /// accepted; less than `delta` at an end of the range.
    ///
    /// `lead` is the share of the movement shown at once, from 0 to 1, with
    /// the rest eased in. A little lead makes a notch feel attached to the
    /// finger without turning it back into a jump.
    ///
    /// Turning the wheel the other way drops whatever was still owed in the
    /// old direction, so a reversal stops the view where it is and moves it
    /// back, instead of first finishing a movement the user has abandoned.
    pub fn wheel(&mut self, at: Duration, delta: f64, smoothing: Smoothing, lead: f64) -> f64 {
        if !delta.is_finite() || delta == 0.0 {
            return 0.0;
        }
        self.advance(at);
        if matches!(self.mode, Mode::Animate { .. } | Mode::Fling { .. }) {
            // The user took over from a programmed move, or caught a coast.
            self.target = self.position;
        }
        let owed = self.target - self.position;
        if owed != 0.0 && owed.signum() != delta.signum() {
            self.ledger.reversed += owed.abs();
            self.target = self.position;
        }
        let accepted = self.accept(self.target, delta);
        self.target += accepted;
        match smoothing {
            Smoothing::Exponential { time } if !time.is_zero() => {
                let shown = accepted * lead.clamp(0.0, 1.0);
                self.position += shown;
                self.mode = Mode::Chase {
                    time: time.as_secs_f64(),
                };
                if (self.target - self.position).abs() <= self.rest {
                    self.arrive();
                }
            }
            _ => self.arrive(),
        }
        accepted
    }

    /// Movement applied as it comes: a touchpad that is already continuous,
    /// a drag. The position moves by `delta` at once. Anything a wheel still
    /// owed moves along with it, so the two do not fight.
    pub fn nudge(&mut self, at: Duration, delta: f64) -> f64 {
        if !delta.is_finite() || delta == 0.0 {
            return 0.0;
        }
        self.advance(at);
        if matches!(self.mode, Mode::Animate { .. } | Mode::Fling { .. }) {
            self.target = self.position;
            self.mode = Mode::Idle;
        }
        let accepted = self.accept(self.position, delta);
        self.position += accepted;
        self.target = (self.target + accepted).clamp(self.min, self.max);
        if (self.target - self.position).abs() <= self.rest {
            self.arrive();
        }
        accepted
    }

    /// Go to `position` now: a scrollbar drag, a restored view. Ends any
    /// movement in progress.
    pub fn jump_to(&mut self, at: Duration, position: f64) {
        if !position.is_finite() {
            return;
        }
        self.advance(at);
        self.target = position.clamp(self.min, self.max);
        self.arrive();
    }

    /// Go to `position` over `duration`, fast at first and easing in. For a
    /// page key or a jump to a search result. Calling it again while it runs
    /// starts a new move from wherever the position has reached.
    pub fn animate_to(&mut self, at: Duration, position: f64, duration: Duration) {
        if !position.is_finite() {
            return;
        }
        self.advance(at);
        self.target = position.clamp(self.min, self.max);
        if duration.is_zero() || (self.target - self.position).abs() <= self.rest {
            self.arrive();
            return;
        }
        self.mode = Mode::Animate {
            from: self.position,
            start: self.clock,
            duration: duration.as_secs_f64(),
        };
    }

    /// Let go at `velocity`, in units a second, and coast to a stop.
    ///
    /// For the end of a drag: the content was moving with the pointer and
    /// carries on, slowing by `friction` (the share of its speed lost each
    /// second, as a rate: 2 is a long glide, 10 a short one). It covers
    /// `velocity / friction` in all unless an end of the range stops it
    /// first, and [`target`](Self::target) says where it will rest from the
    /// moment it is launched. Never call this for input the system is
    /// already coasting on its own account.
    pub fn fling(&mut self, at: Duration, velocity: f64, friction: f64) {
        if !velocity.is_finite() || !friction.is_finite() || friction <= 0.0 {
            return;
        }
        self.advance(at);
        let rest = self.position + velocity / friction;
        self.target = rest.clamp(self.min, self.max);
        if (velocity / friction).abs() <= self.rest || self.target == self.position {
            self.arrive();
            return;
        }
        self.mode = Mode::Fling {
            from: self.position,
            start: self.clock,
            velocity,
            friction,
        };
    }

    /// Stop where it is. A press on the content does this, so that what is
    /// under the pointer stays under it.
    pub fn halt(&mut self) {
        self.target = self.position;
        self.mode = Mode::Idle;
    }

    /// Finish at the target at once. For when there is no clock, and so no
    /// later frame to finish in.
    pub fn settle(&mut self) {
        self.arrive();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scroll::{Interpreted, Units, WheelInterpreter, WheelProfile, WheelTrace};

    const NOTCHED: &str = include_str!("../../tests/scroll_traces/notched-wheel.trace");
    const FINE_NOTCHED: &str = include_str!("../../tests/scroll_traces/fine-notched-wheel.trace");
    const TOUCHPAD: &str = include_str!("../../tests/scroll_traces/precision-touchpad.trace");

    const EASE: Smoothing = Smoothing::Exponential {
        time: Duration::from_millis(40),
    };

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn ranged(max: f64) -> MotionController {
        let mut motion = MotionController::new();
        motion.set_bounds(0.0, max);
        motion
    }

    #[test]
    fn a_notch_starts_moving_at_once_and_arrives_exactly() {
        let mut motion = ranged(10_000.0);
        assert_eq!(motion.wheel(ms(0), 100.0, EASE, 0.2), 100.0);
        assert_eq!(motion.position(), 20.0, "the lead is shown immediately");
        assert_eq!(motion.target(), 100.0);
        assert!(motion.animating());
        motion.advance(ms(40));
        // One time constant: about 37% of what was left remains.
        assert!((motion.position() - (100.0 - 80.0 * (-1.0_f64).exp())).abs() < 1e-9);
        motion.advance(ms(2_000));
        assert_eq!(motion.position(), 100.0);
        assert!(!motion.animating(), "and then it is at rest");
        assert_eq!(motion.velocity(), 0.0);
    }

    #[test]
    fn the_position_never_passes_the_target_or_goes_back() {
        let mut motion = ranged(10_000.0);
        let mut last = 0.0;
        for step in 0..400_u64 {
            if step % 25 == 0 && step < 200 {
                motion.wheel(ms(step * 4), 100.0, EASE, 0.2);
            }
            motion.advance(ms(step * 4));
            assert!(motion.position() >= last, "step {step}");
            assert!(motion.position() <= motion.target() + 1e-9, "step {step}");
            last = motion.position();
        }
        assert_eq!(motion.position(), 800.0);
    }

    #[test]
    fn without_smoothing_movement_is_immediate() {
        let mut motion = ranged(1_000.0);
        motion.wheel(ms(0), 60.0, Smoothing::None, 0.0);
        assert_eq!((motion.position(), motion.animating()), (60.0, false));
        motion.nudge(ms(5), 0.3);
        assert_eq!(motion.position(), 60.3);
    }

    #[test]
    fn reversing_stops_the_view_where_it_is() {
        let mut motion = ranged(10_000.0);
        motion.wheel(ms(0), 500.0, EASE, 0.0);
        motion.advance(ms(20));
        let reached = motion.position();
        assert!(reached > 0.0 && reached < 500.0);
        motion.wheel(ms(20), -60.0, EASE, 0.0);
        assert_eq!(motion.target(), reached - 60.0);
        motion.advance(ms(5_000));
        assert!((motion.position() - (reached - 60.0)).abs() < 1e-9);
        assert!((motion.ledger().reversed - (500.0 - reached)).abs() < 1e-9);
    }

    #[test]
    fn movement_refused_at_an_end_is_gone_not_waiting() {
        let mut motion = ranged(100.0);
        assert_eq!(motion.wheel(ms(0), 250.0, EASE, 0.0), 100.0);
        assert_eq!(motion.ledger().clamped, 150.0);
        motion.advance(ms(1_000));
        assert_eq!(motion.position(), 100.0);
        // One step back moves back one step. Nothing was saved up to be
        // worked off first, and nothing moves later on its own.
        motion.wheel(ms(1_000), -10.0, EASE, 0.0);
        motion.advance(ms(3_000));
        assert_eq!(motion.position(), 90.0);
        let rested = motion.position();
        motion.advance(ms(60_000));
        assert_eq!(motion.position(), rested);
    }

    #[test]
    fn a_speed_limit_delays_a_burst_without_shortening_it() {
        let mut motion = ranged(100_000.0);
        motion.set_max_speed(Some(1_000.0));
        motion.wheel(ms(0), 5_000.0, EASE, 0.0);
        motion.advance(ms(1_000));
        assert!((motion.position() - 1_000.0).abs() < 1e-6, "at the limit");
        assert!((motion.velocity() - 1_000.0).abs() < 1e-6);
        motion.advance(ms(20_000));
        assert_eq!(motion.position(), 5_000.0, "all of it arrives");
    }

    #[test]
    fn a_long_stall_just_finishes_and_time_does_not_run_backwards() {
        let mut motion = ranged(10_000.0);
        motion.wheel(ms(100), 300.0, EASE, 0.2);
        motion.advance(ms(50));
        assert_eq!(motion.position(), 60.0, "an earlier time changes nothing");
        motion.advance(Duration::from_secs(3_600));
        assert_eq!(motion.position(), 300.0);
    }

    #[test]
    fn a_programmed_move_arrives_on_time_and_the_wheel_takes_over_from_it() {
        let mut motion = ranged(10_000.0);
        motion.animate_to(ms(0), 1_000.0, ms(200));
        assert_eq!(motion.target(), 1_000.0);
        motion.advance(ms(100));
        // Half way through an ease-out cubic: seven eighths of the distance.
        assert!((motion.position() - 875.0).abs() < 1e-9);
        motion.advance(ms(200));
        assert_eq!((motion.position(), motion.animating()), (1_000.0, false));

        motion.animate_to(ms(300), 0.0, ms(200));
        motion.advance(ms(350));
        let reached = motion.position();
        motion.wheel(ms(350), 60.0, EASE, 0.0);
        assert_eq!(motion.target(), reached + 60.0, "from where it had got to");
    }

    #[test]
    fn a_fling_coasts_to_a_stop_it_knew_from_the_start() {
        let mut motion = ranged(100_000.0);
        motion.fling(ms(0), 2_000.0, 4.0);
        // 2000 px/s losing speed at a rate of 4 covers 500 px in all.
        assert_eq!(motion.target(), 500.0);
        assert!(motion.animating());
        let mut last = 0.0;
        for step in 1..=400_u64 {
            motion.advance(ms(step * 10));
            assert!(motion.position() >= last, "never backwards");
            last = motion.position();
        }
        assert_eq!(motion.position(), 500.0);
        assert!(!motion.animating());
        assert_eq!(motion.velocity(), 0.0);
    }

    #[test]
    fn a_fling_is_the_same_however_often_it_is_drawn() {
        let mut coarse = ranged(100_000.0);
        let mut fine = ranged(100_000.0);
        coarse.fling(ms(0), -3_000.0, 5.0);
        fine.fling(ms(0), -3_000.0, 5.0);
        coarse.jump_to(ms(0), 50_000.0);
        fine.jump_to(ms(0), 50_000.0);
        coarse.fling(ms(0), -3_000.0, 5.0);
        fine.fling(ms(0), -3_000.0, 5.0);
        for step in 1..=300_u64 {
            fine.advance(ms(step));
        }
        coarse.advance(ms(300));
        assert!((coarse.position() - fine.position()).abs() < 1e-9);
    }

    #[test]
    fn a_fling_stops_dead_at_an_end_and_a_touch_of_the_wheel_catches_it() {
        let mut motion = ranged(300.0);
        motion.fling(ms(0), 5_000.0, 2.0);
        assert_eq!(motion.target(), 300.0, "it knows it will not get further");
        motion.advance(ms(2_000));
        assert_eq!((motion.position(), motion.animating()), (300.0, false));

        let mut caught = ranged(100_000.0);
        caught.fling(ms(0), 4_000.0, 2.0);
        caught.advance(ms(100));
        let at = caught.position();
        caught.wheel(ms(100), -60.0, Smoothing::None, 0.0);
        assert_eq!(caught.position(), at - 60.0, "the coast is over");
        caught.advance(ms(5_000));
        assert_eq!(caught.position(), at - 60.0);
    }

    #[test]
    fn a_jump_ends_whatever_was_happening() {
        let mut motion = ranged(10_000.0);
        motion.wheel(ms(0), 900.0, EASE, 0.0);
        motion.jump_to(ms(10), 4_000.0);
        assert_eq!((motion.position(), motion.target()), (4_000.0, 4_000.0));
        assert!(!motion.animating());
        motion.jump_to(ms(20), 99_999.0);
        assert_eq!(motion.position(), 10_000.0, "inside the range");
    }

    #[test]
    fn halting_keeps_what_is_on_screen_on_screen() {
        let mut motion = ranged(10_000.0);
        motion.wheel(ms(0), 900.0, EASE, 0.0);
        motion.advance(ms(30));
        let shown = motion.position();
        motion.halt();
        motion.advance(ms(5_000));
        assert_eq!(motion.position(), shown);
    }

    #[test]
    fn content_that_shrinks_mid_scroll_takes_the_view_with_it() {
        let mut motion = ranged(10_000.0);
        motion.wheel(ms(0), 5_000.0, EASE, 0.0);
        motion.advance(ms(40));
        motion.set_bounds(0.0, 1_000.0);
        assert!(motion.position() <= 1_000.0 && motion.target() == 1_000.0);
        motion.advance(ms(5_000));
        assert_eq!(motion.position(), 1_000.0);
    }

    #[test]
    fn nonsense_is_ignored() {
        let mut motion = ranged(100.0);
        motion.wheel(ms(0), f64::NAN, EASE, 0.2);
        motion.nudge(ms(0), f64::INFINITY);
        motion.jump_to(ms(0), f64::NAN);
        motion.set_bounds(f64::NAN, 5.0);
        assert_eq!((motion.position(), motion.bounds()), (0.0, (0.0, 100.0)));
    }

    /// Replay a recording through the interpreter and a controller, drawing
    /// a frame at each of `frames`. Returns the position after each frame.
    fn replay(frames: &[(Duration, &[crate::scroll::WheelSample])]) -> Vec<f64> {
        let mut interpreter = WheelInterpreter::new(WheelProfile::accelerated());
        let mut motion = ranged(1.0e9);
        motion.jump_to(Duration::ZERO, 5.0e8);
        let units = Units::pixels((8.0, 20.0), (800.0, 600.0));
        let mut shown = Vec::new();
        for (time, samples) in frames {
            for sample in *samples {
                if let Interpreted::Scroll(scroll) = interpreter.interpret(sample, &units) {
                    match scroll.class {
                        crate::scroll::InputClass::Notched => {
                            motion.wheel(sample.at, scroll.displacement.1, EASE, 0.2);
                        }
                        _ => {
                            motion.nudge(sample.at, scroll.displacement.1);
                        }
                    }
                }
            }
            motion.advance(*time);
            shown.push(motion.position());
        }
        shown
    }

    #[test]
    fn a_recording_is_shown_the_same_at_any_frame_rate() {
        // The claim the module rests on, checked on what real devices sent:
        // where a scroll ends up, and where it is at any moment on the way,
        // does not depend on how often it happened to be drawn. Each
        // recording is replayed at four frame rates, and again at eight
        // times each rate; every frame of the slower replay must show what
        // the faster one showed at that same instant.
        let tail = Duration::from_secs(2);
        for text in [NOTCHED, FINE_NOTCHED, TOUCHPAD] {
            let trace = WheelTrace::from_text(text).expect("readable");
            let mut ends = Vec::new();
            for interval_us in [16_667_u64, 6_944, 33_333, 11_111] {
                let interval = Duration::from_micros(interval_us);
                let frames = trace.frames(interval, tail);
                let shown = replay(&frames);
                let finer = replay(&trace.frames(interval / 8, tail));
                for (index, position) in shown.iter().enumerate() {
                    let Some(same_instant) = finer.get(index * 8) else {
                        break;
                    };
                    assert!(
                        (position - same_instant).abs() < 1e-6,
                        "frame {index} at {interval_us} us: {position} against {same_instant}"
                    );
                }
                ends.push(shown[shown.len() - 1]);
            }
            assert!(
                (ends[0] - 5.0e8).abs() > 100.0,
                "the recording moved something"
            );
            for end in &ends {
                assert!((end - ends[0]).abs() < 1e-6, "ends: {ends:?}");
            }
        }
    }

    #[test]
    fn nothing_moves_once_a_recording_has_come_to_rest() {
        let trace = WheelTrace::from_text(NOTCHED).expect("readable");
        let frames = trace.frames(Duration::from_millis(16), Duration::from_secs(3));
        let shown = replay(&frames);
        let end = shown[shown.len() - 1];
        let settled = shown.len() - 60;
        assert!(shown[settled..].iter().all(|position| *position == end));
    }
}
