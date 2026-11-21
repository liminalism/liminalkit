//! Values that move over time: tweens, springs, and the curves and
//! envelopes they follow.
//!
//! Everything here is caller-owned state driven by the host's clock. Nothing
//! reads the system clock: time arrives as `Option<Duration>`, a monotonic
//! time relative to whatever the host likes, and `None` means the host has
//! no clock at all (a test, a headless render, a host that only draws when
//! asked). With no clock there is no later frame to animate in, so every
//! animation settles at its target at once. Time that runs backwards is
//! ignored: each animation keeps the latest time it has seen.
//!
//! A [`Tween`] moves between two values along an [`Ease`] in a fixed time.
//! A [`Spring`] chases a target with physics, so it can be retargeted or
//! pushed at any moment and always moves smoothly; it is integrated in
//! fixed steps on an absolute grid, so where it is at a given time does not
//! depend on how often it was drawn. [`Envelope`] and [`keyframes`] are pure
//! functions of elapsed time, measured from a [`Trigger`].
//!
//! EXPERIMENTAL: the API is new, and the spring presets have not been tuned
//! on a screen.

use std::time::Duration;

use crate::geom::{Argb, RectF};

mod ease;
mod envelope;

pub use ease::Ease;
pub use envelope::{keyframes, stagger, Envelope, Key, Repeat, Trigger};

/// Straight-line interpolation between two values.
///
/// `t` is usually 0 to 1 but may go outside it, for an ease that overshoots.
/// Implementations return `a` exactly at 0 and `b` exactly at 1, so a tween
/// that finishes is at its target and not a rounding error away.
pub trait Lerp: Copy {
    /// The value `t` of the way from `a` to `b`.
    fn lerp(a: Self, b: Self, t: f32) -> Self;
}

/// A value with the arithmetic a [`Spring`] needs: a vector space with a
/// length.
pub trait Linear: Lerp {
    /// The zero vector.
    const ZERO: Self;
    /// `self + o`.
    fn add(self, o: Self) -> Self;
    /// `self - o`.
    fn sub(self, o: Self) -> Self;
    /// `self * k`.
    fn scale(self, k: f32) -> Self;
    /// The squared length.
    fn norm_sq(self) -> f32;
}

impl Lerp for f32 {
    fn lerp(a: f32, b: f32, t: f32) -> f32 {
        // This form, rather than `a + (b - a) * t`, is exact at both ends.
        a * (1.0 - t) + b * t
    }
}

impl Lerp for f64 {
    fn lerp(a: f64, b: f64, t: f32) -> f64 {
        let t = f64::from(t);
        a * (1.0 - t) + b * t
    }
}

impl<const N: usize> Lerp for [f32; N] {
    fn lerp(a: [f32; N], b: [f32; N], t: f32) -> [f32; N] {
        let mut out = a;
        for (out, b) in out.iter_mut().zip(b) {
            *out = f32::lerp(*out, b, t);
        }
        out
    }
}

impl Lerp for RectF {
    fn lerp(a: RectF, b: RectF, t: f32) -> RectF {
        RectF::new(
            f32::lerp(a.x, b.x, t),
            f32::lerp(a.y, b.y, t),
            f32::lerp(a.w, b.w, t),
            f32::lerp(a.h, b.h, t),
        )
    }
}

impl Lerp for Argb {
    /// Per channel, alpha included, on the encoded (sRGB) bytes, rounded to
    /// the nearest and clamped. Straight colour is interpolated, so a fade
    /// between a colour and transparent drifts towards the transparent
    /// colour's channels; give the transparent end the same colour (with
    /// [`Argb::with_alpha`]) to fade only the alpha.
    fn lerp(a: Argb, b: Argb, t: f32) -> Argb {
        let (a, b) = (a.channels(), b.channels());
        let mut out = [0_u8; 4];
        for ((out, a), b) in out.iter_mut().zip(a).zip(b) {
            let mixed = f32::lerp(f32::from(a), f32::from(b), t);
            *out = mixed.round().clamp(0.0, 255.0) as u8;
        }
        Argb::from_channels(out)
    }
}

impl Linear for f32 {
    const ZERO: f32 = 0.0;
    fn add(self, o: f32) -> f32 {
        self + o
    }
    fn sub(self, o: f32) -> f32 {
        self - o
    }
    fn scale(self, k: f32) -> f32 {
        self * k
    }
    fn norm_sq(self) -> f32 {
        self * self
    }
}

macro_rules! linear_array {
    ($n:literal) => {
        impl Linear for [f32; $n] {
            const ZERO: [f32; $n] = [0.0; $n];
            fn add(self, o: [f32; $n]) -> [f32; $n] {
                let mut out = self;
                for (out, o) in out.iter_mut().zip(o) {
                    *out += o;
                }
                out
            }
            fn sub(self, o: [f32; $n]) -> [f32; $n] {
                let mut out = self;
                for (out, o) in out.iter_mut().zip(o) {
                    *out -= o;
                }
                out
            }
            fn scale(self, k: f32) -> [f32; $n] {
                self.map(|v| v * k)
            }
            fn norm_sq(self) -> f32 {
                self.iter().map(|v| v * v).sum()
            }
        }
    };
}

linear_array!(2);
linear_array!(3);
linear_array!(4);

/// What an animation showed after an update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Animated<T> {
    /// The value to draw.
    pub value: T,
    /// Whether it is still moving, so another frame is worth drawing.
    pub animating: bool,
    /// Whether the value differs from what the previous update returned (or,
    /// for the first update, from the value it was created with). A host
    /// that draws only on change can skip the frame when this is false.
    pub changed: bool,
}

/// A value moved from one place to another along an [`Ease`] in a fixed
/// time.
///
/// Where it is depends only on when it started and the latest time it has
/// been given, never on how many updates came between, so a tween drawn at
/// 60 Hz and one drawn at 144 Hz show the same value at the same instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween<T> {
    from: T,
    to: T,
    value: T,
    /// What the last update returned, for [`Animated::changed`].
    shown: T,
    /// The host time the move began (before any delay).
    start: Duration,
    delay: Duration,
    duration: Duration,
    ease: Ease,
    /// The latest host time seen.
    clock: Option<Duration>,
    running: bool,
}

impl<T: Lerp + PartialEq> Tween<T> {
    /// At rest at `value`.
    pub fn new(value: T) -> Self {
        Tween {
            from: value,
            to: value,
            value,
            shown: value,
            start: Duration::ZERO,
            delay: Duration::ZERO,
            duration: Duration::ZERO,
            ease: Ease::Linear,
            clock: None,
            running: false,
        }
    }

    /// Move to `to` over `duration`, starting at `now` from wherever it is
    /// at `now`. Retargeting a tween in flight is therefore continuous: it
    /// sets off from the value it had reached, not from where the old move
    /// began. (The speed can still change abruptly; a [`Spring`] keeps that
    /// continuous too.)
    pub fn start(&mut self, now: Option<Duration>, to: T, duration: Duration, ease: Ease) {
        self.start_after(now, Duration::ZERO, to, duration, ease);
    }

    /// As [`start`](Self::start), but holding still for `delay` first.
    pub fn start_after(
        &mut self,
        now: Option<Duration>,
        delay: Duration,
        to: T,
        duration: Duration,
        ease: Ease,
    ) {
        self.advance(now);
        self.begin(now, self.value, to, delay, duration, ease);
    }

    /// Move from `from` to `to` over `duration`, starting at `now`. The value
    /// jumps to `from` at once.
    pub fn start_from(
        &mut self,
        now: Option<Duration>,
        from: T,
        to: T,
        duration: Duration,
        ease: Ease,
    ) {
        self.advance(now);
        self.begin(now, from, to, Duration::ZERO, duration, ease);
    }

    fn begin(
        &mut self,
        now: Option<Duration>,
        from: T,
        to: T,
        delay: Duration,
        duration: Duration,
        ease: Ease,
    ) {
        self.from = from;
        self.to = to;
        self.value = from;
        self.delay = delay;
        self.duration = duration;
        self.ease = ease;
        let instant = from == to || (duration.is_zero() && delay.is_zero());
        match self.clock {
            Some(clock) if now.is_some() && !instant => {
                self.start = clock;
                self.running = true;
            }
            // No clock, nothing to cross or no time to cross it in: there.
            _ => self.land(),
        }
    }

    /// Be at `value` now, at rest.
    pub fn jump(&mut self, value: T) {
        self.from = value;
        self.to = value;
        self.value = value;
        self.running = false;
    }

    fn land(&mut self) {
        self.value = self.to;
        self.running = false;
    }

    /// Note the time and move the value to where it is at that time.
    fn advance(&mut self, now: Option<Duration>) {
        let Some(now) = now else {
            // No clock, so no later frame to arrive in.
            self.land();
            return;
        };
        let clock = self.clock.map_or(now, |clock| clock.max(now));
        self.clock = Some(clock);
        if !self.running {
            return;
        }
        let since = clock.saturating_sub(self.start);
        if since < self.delay {
            self.value = self.from;
            return;
        }
        let moving = since - self.delay;
        if moving >= self.duration {
            self.land();
            return;
        }
        let progress = (moving.as_secs_f64() / self.duration.as_secs_f64()) as f32;
        self.value = T::lerp(self.from, self.to, self.ease.apply(progress));
    }

    /// Move to where it is at `now`, and report it. With no clock it settles
    /// at its target.
    pub fn update(&mut self, now: Option<Duration>) -> Animated<T> {
        self.advance(now);
        let changed = self.value != self.shown;
        self.shown = self.value;
        Animated {
            value: self.value,
            animating: self.running,
            changed,
        }
    }

    /// Where it is, as of the latest time it was given.
    pub fn value(&self) -> T {
        self.value
    }

    /// Where it is going; equal to the value at rest.
    pub fn target(&self) -> T {
        self.to
    }

    /// Whether it is still on its way (a delay counts).
    pub fn is_animating(&self) -> bool {
        self.running
    }
}

/// How a [`Spring`] behaves.
///
/// The usual physical spring: a pull of `stiffness` per unit of distance
/// from the target, a drag of `damping` per unit of speed, acting on
/// `mass`. With `damping` at `2 * sqrt(stiffness * mass)` it is critically
/// damped: the fastest approach that does not overshoot. Below that it
/// bounces.
///
/// The spring is integrated in steps of 1/240 s, which stays stable while
/// `stiffness / mass` is below about 20 000 and `damping / mass` below about
/// 400; the presets and [`critical`](Self::critical) keep well inside that.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringParams {
    /// The pull towards the target, per unit of distance.
    pub stiffness: f32,
    /// The drag against movement, per unit of speed.
    pub damping: f32,
    /// What is being moved. Larger is more sluggish.
    pub mass: f32,
    /// Within this of the target, and slower than this (per second), the
    /// spring is at rest and snaps exactly to the target. In the value's own
    /// unit: a thousandth suits pixels and opacity alike.
    pub rest: f32,
}

impl SpringParams {
    /// Quick and just short of critically damped: a hint of settle, no
    /// visible bounce. About 0.2 s to arrive.
    pub const SNAPPY: SpringParams = SpringParams {
        stiffness: 500.0,
        damping: 40.0,
        mass: 1.0,
        rest: 0.001,
    };
    /// Unhurried and critically damped. About 0.6 s to arrive.
    pub const GENTLE: SpringParams = SpringParams {
        stiffness: 120.0,
        damping: 22.0,
        mass: 1.0,
        rest: 0.001,
    };
    /// Lightly damped: overshoots and rings a few times.
    pub const BOUNCY: SpringParams = SpringParams {
        stiffness: 300.0,
        damping: 12.0,
        mass: 1.0,
        rest: 0.001,
    };

    /// Critically damped, arriving within about 1% of the distance in
    /// `response` (and at rest a little after). Responses shorter than
    /// 50 ms are taken as 50 ms, which keeps the fixed-step integration
    /// stable.
    pub fn critical(response: Duration) -> SpringParams {
        // (1 + w t) e^(-w t) falls to 1% at w t of about 6.64.
        let response = response.as_secs_f32().max(0.05);
        let omega = 6.64 / response;
        SpringParams {
            stiffness: omega * omega,
            damping: 2.0 * omega,
            mass: 1.0,
            rest: 0.001,
        }
    }
}

/// Steps per second of spring integration.
const SPRING_RATE: u128 = 240;

/// Longer than this between updates and a spring gives up on simulating the
/// gap and is simply at its target: a stalled frame or a suspended window
/// should not cost a burst of catch-up work, and nobody saw the motion.
const SPRING_CATCH_UP: Duration = Duration::from_millis(250);

/// A value pulled towards a target by a damped spring.
///
/// Unlike a [`Tween`], it carries its speed: retargeting it mid-flight, or
/// knocking it with an [`impulse`](Self::impulse), bends its path without a
/// corner. It is integrated with semi-implicit Euler in fixed 1/240 s steps
/// counted from host time zero (step `n` ends at `n / 240` s), so the state
/// it reaches at a given time is bit-for-bit the same however the frames in
/// between fell, as long as no gap exceeded 250 ms. A longer gap, or no
/// clock, puts it at its target at rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring<T: Linear> {
    value: T,
    velocity: T,
    target: T,
    params: SpringParams,
    /// What the last update returned, for [`Animated::changed`].
    shown: T,
    /// The latest host time seen.
    clock: Option<Duration>,
}

impl<T: Linear> Spring<T> {
    /// At rest at `value`.
    pub fn new(value: T, params: SpringParams) -> Self {
        Spring {
            value,
            velocity: T::ZERO,
            target: value,
            params,
            shown: value,
            clock: None,
        }
    }

    /// The spring's behaviour.
    pub fn params(&self) -> SpringParams {
        self.params
    }

    /// Change the spring's behaviour; the motion carries on from where it is.
    pub fn set_params(&mut self, params: SpringParams) {
        self.params = params;
    }

    /// Pull towards `target` from `now`. The spring is first brought up to
    /// `now`, so the change takes effect at that moment whenever the next
    /// update comes. With no clock it is at `target` at once.
    pub fn set_target(&mut self, now: Option<Duration>, target: T) {
        self.advance(now);
        self.target = target;
        if now.is_none() {
            self.snap();
        }
    }

    /// Add `velocity` (units a second) to its speed, as a knock would.
    pub fn impulse(&mut self, velocity: T) {
        self.velocity = self.velocity.add(velocity);
    }

    /// Be at `value` now, at rest, with that as the target.
    pub fn jump(&mut self, value: T) {
        self.value = value;
        self.target = value;
        self.velocity = T::ZERO;
    }

    fn snap(&mut self) {
        self.value = self.target;
        self.velocity = T::ZERO;
    }

    fn at_rest(&self) -> bool {
        self.value.sub(self.target).norm_sq() == 0.0 && self.velocity.norm_sq() == 0.0
    }

    /// Integrate up to `now`.
    fn advance(&mut self, now: Option<Duration>) {
        let Some(now) = now else {
            self.snap();
            return;
        };
        let Some(before) = self.clock else {
            // The first time seen: the spring starts from here.
            self.clock = Some(now);
            return;
        };
        let now = before.max(now);
        self.clock = Some(now);
        if now - before > SPRING_CATCH_UP {
            self.snap();
            return;
        }
        let steps = step_index(now) - step_index(before);
        for _ in 0..steps {
            if self.at_rest() {
                break;
            }
            self.step();
        }
    }

    /// One fixed step of semi-implicit Euler: speed first, then position
    /// with the new speed, which keeps an undamped spring's energy bounded.
    fn step(&mut self) {
        let h = 1.0 / SPRING_RATE as f32;
        let SpringParams {
            stiffness,
            damping,
            mass,
            rest,
        } = self.params;
        let mass = if mass > 0.0 { mass } else { 1.0 };
        let before = self.value;
        let offset = self.value.sub(self.target);
        let force = offset.scale(-stiffness).add(self.velocity.scale(-damping));
        self.velocity = self.velocity.add(force.scale(h / mass));
        self.value = self.value.add(self.velocity.scale(h));

        let offset = self.value.sub(self.target).norm_sq();
        let speed = self.velocity.norm_sq();
        let rest = rest * rest;
        // Far from zero, f32 cannot represent a step smaller than its
        // spacing there, and a slow spring can freeze a few units of that
        // spacing short of the target with a small speed it never loses.
        // A step that moved nothing, that close, is as near as it can get.
        let stalled = self.value.sub(before).norm_sq() == 0.0
            && offset <= rest.max(self.target.norm_sq() * 1e-8);
        if !(offset.is_finite() && speed.is_finite())
            || (offset <= rest && speed <= rest)
            || stalled
        {
            self.snap();
        }
    }

    /// Move to where it is at `now`, and report it. With no clock, or after
    /// a gap of more than 250 ms, it is at its target.
    pub fn update(&mut self, now: Option<Duration>) -> Animated<T> {
        self.advance(now);
        let changed = self.value.sub(self.shown).norm_sq() != 0.0;
        self.shown = self.value;
        Animated {
            value: self.value,
            animating: !self.at_rest(),
            changed,
        }
    }

    /// Where it is, as of the latest time it was given.
    pub fn value(&self) -> T {
        self.value
    }

    /// Where it is being pulled.
    pub fn target(&self) -> T {
        self.target
    }

    /// Its speed, in units a second.
    pub fn velocity(&self) -> T {
        self.velocity
    }

    /// Whether it is still moving.
    pub fn is_animating(&self) -> bool {
        !self.at_rest()
    }
}

/// The index of the last integration step that has ended by `time`.
fn step_index(time: Duration) -> u128 {
    time.as_nanos() * SPRING_RATE / 1_000_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn at(value: u64) -> Option<Duration> {
        Some(ms(value))
    }

    #[test]
    fn lerp_is_exact_at_both_ends() {
        for (a, b) in [(0.1_f32, 0.3), (-7.5, 1e6), (3.0, 3.0)] {
            assert_eq!(f32::lerp(a, b, 0.0), a);
            assert_eq!(f32::lerp(a, b, 1.0), b);
        }
        assert_eq!(f64::lerp(0.1, 0.3, 1.0), 0.3);
        assert_eq!(f32::lerp(0.0, 10.0, 0.25), 2.5);
        assert_eq!(
            f32::lerp(0.0, 10.0, 1.5),
            15.0,
            "past the end for overshoot"
        );
    }

    #[test]
    fn arrays_and_rectangles_lerp_per_component() {
        assert_eq!(<[f32; 2]>::lerp([0.0, 10.0], [10.0, 0.0], 0.5), [5.0, 5.0]);
        assert_eq!(
            <[f32; 3]>::lerp([0.0; 3], [2.0, 4.0, 8.0], 0.5),
            [1.0, 2.0, 4.0]
        );
        assert_eq!(
            <[f32; 4]>::lerp([0.0; 4], [4.0; 4], 0.25),
            [1.0, 1.0, 1.0, 1.0]
        );
        let a = RectF::new(0.0, 0.0, 10.0, 10.0);
        let b = RectF::new(10.0, 20.0, 30.0, 0.0);
        assert_eq!(RectF::lerp(a, b, 0.5), RectF::new(5.0, 10.0, 20.0, 5.0));
        assert_eq!(RectF::lerp(a, b, 1.0), b);
    }

    #[test]
    fn colours_lerp_per_encoded_channel_with_rounding() {
        let a = Argb(0x0000_0000);
        let b = Argb(0xFFFF_8001);
        assert_eq!(Argb::lerp(a, b, 0.0), a);
        assert_eq!(Argb::lerp(a, b, 1.0), b);
        // 255 / 2 = 127.5 rounds up; 128 / 2 = 64; 1 / 2 = 0.5 rounds up.
        assert_eq!(Argb::lerp(a, b, 0.5), Argb(0x8080_4001));
        assert_eq!(Argb::lerp(a, b, 0.001), Argb(0x0000_0000));
        assert_eq!(Argb::lerp(a, b, 0.999), Argb(0xFFFF_8001));
        // An overshooting ease is clamped, not wrapped.
        assert_eq!(Argb::lerp(a, b, 1.5), Argb(0xFFFF_C002));
        assert_eq!(Argb::lerp(a, b, -1.0), Argb(0x0000_0000));
    }

    #[test]
    fn linear_arithmetic_works_per_component() {
        assert_eq!(<[f32; 2]>::ZERO, [0.0, 0.0]);
        assert_eq!([1.0_f32, 2.0].add([3.0, 4.0]), [4.0, 6.0]);
        assert_eq!([1.0_f32, 2.0, 3.0].sub([1.0, 1.0, 1.0]), [0.0, 1.0, 2.0]);
        assert_eq!([1.0_f32, 2.0, 3.0, 4.0].scale(2.0), [2.0, 4.0, 6.0, 8.0]);
        assert_eq!([3.0_f32, 4.0].norm_sq(), 25.0);
        assert_eq!(Linear::norm_sq(-3.0_f32), 9.0);
    }

    #[test]
    fn a_tween_with_no_clock_settles_at_its_target() {
        let mut tween = Tween::new(0.0_f32);
        tween.start(None, 10.0, ms(300), Ease::CubicOut);
        assert_eq!(tween.value(), 10.0);
        assert!(!tween.is_animating());
        let shown = tween.update(None);
        assert_eq!(
            shown,
            Animated {
                value: 10.0,
                animating: false,
                changed: true
            }
        );

        let mut running = Tween::new(0.0_f32);
        running.start(at(0), 10.0, ms(300), Ease::Linear);
        running.update(at(100));
        let shown = running.update(None);
        assert_eq!((shown.value, shown.animating), (10.0, false));
    }

    #[test]
    fn a_tween_reaches_its_target_exactly_on_time() {
        let mut tween = Tween::new(0.1_f32);
        tween.start(at(1_000), 0.3, ms(200), Ease::CubicOut);
        assert!(tween.is_animating());
        let half = tween.update(at(1_100));
        assert!((half.value - (0.1 + 0.2 * 0.875)).abs() < 1e-6);
        assert!(half.animating && half.changed);
        let almost = tween.update(at(1_199));
        assert!(almost.animating && almost.value < 0.3);
        let done = tween.update(at(1_200));
        assert_eq!(done.value, 0.3);
        assert!(!done.animating);
        let after = tween.update(at(5_000));
        assert_eq!(
            (after.value, after.animating, after.changed),
            (0.3, false, false)
        );
    }

    #[test]
    fn retargeting_a_tween_mid_flight_sets_off_from_where_it_was() {
        let mut tween = Tween::new(0.0_f32);
        tween.start(at(0), 100.0, ms(100), Ease::Linear);
        tween.update(at(40));
        // Retarget at 50 ms without an update at 50 first: it still sets
        // off from the value at 50 ms.
        tween.start(at(50), 0.0, ms(100), Ease::Linear);
        assert!((tween.value() - 50.0).abs() < 1e-4);
        let next = tween.update(at(60));
        assert!((next.value - 45.0).abs() < 1e-4);
        assert_eq!(tween.update(at(150)).value, 0.0);
    }

    #[test]
    fn an_earlier_time_is_ignored() {
        let mut tween = Tween::new(0.0_f32);
        tween.start(at(100), 10.0, ms(100), Ease::Linear);
        let shown = tween.update(at(150));
        let back = tween.update(at(120));
        assert_eq!(back.value, shown.value, "time does not run backwards");
        assert!(!back.changed);
        // A start stamped before the clock starts at the clock.
        tween.start(at(10), 20.0, ms(100), Ease::Linear);
        // From 5 at 150 ms, half way to 20 by 200 ms.
        let later = tween.update(at(200));
        assert!((later.value - 12.5).abs() < 1e-4, "{}", later.value);
    }

    #[test]
    fn a_delay_holds_the_start_value() {
        let mut tween = Tween::new(5.0_f32);
        tween.start_after(at(0), ms(100), 15.0, ms(100), Ease::Linear);
        assert!(tween.is_animating());
        let waiting = tween.update(at(99));
        assert_eq!(waiting.value, 5.0);
        assert!(waiting.animating && !waiting.changed);
        assert_eq!(tween.update(at(150)).value, 10.0);
        assert_eq!(tween.update(at(200)).value, 15.0);
        assert!(!tween.is_animating());

        let mut instant = Tween::new(0.0_f32);
        instant.start_after(at(0), ms(50), 1.0, Duration::ZERO, Ease::Linear);
        assert_eq!(instant.update(at(49)).value, 0.0);
        assert_eq!(instant.update(at(50)).value, 1.0);
    }

    #[test]
    fn a_zero_duration_or_a_move_to_where_it_is_lands_at_once() {
        let mut tween = Tween::new(0.0_f32);
        tween.start(at(0), 4.0, Duration::ZERO, Ease::Linear);
        assert_eq!((tween.value(), tween.is_animating()), (4.0, false));
        tween.start(at(10), 4.0, ms(100), Ease::Linear);
        assert!(!tween.is_animating(), "nothing to cross");
    }

    #[test]
    fn start_from_jumps_to_the_start_first() {
        let mut tween = Tween::new(0.0_f32);
        tween.start_from(at(0), 10.0, 20.0, ms(100), Ease::Linear);
        assert_eq!(tween.value(), 10.0);
        assert_eq!(tween.target(), 20.0);
        assert_eq!(tween.update(at(50)).value, 15.0);
        assert_eq!(tween.update(at(100)).value, 20.0);
    }

    #[test]
    fn jump_stops_the_tween_where_it_is_told() {
        let mut tween = Tween::new([0.0_f32, 0.0]);
        tween.start(at(0), [10.0, 10.0], ms(100), Ease::Linear);
        tween.update(at(30));
        tween.jump([-1.0, 2.0]);
        assert_eq!(tween.target(), [-1.0, 2.0]);
        let shown = tween.update(at(60));
        assert_eq!(
            shown,
            Animated {
                value: [-1.0, 2.0],
                animating: false,
                changed: true
            }
        );
        assert!(!tween.update(at(70)).changed);
    }

    #[test]
    fn a_tween_of_a_colour_rounds_and_lands_exactly() {
        let mut tween = Tween::new(Argb::BLACK);
        tween.start(at(0), Argb::WHITE, ms(100), Ease::Linear);
        assert_eq!(tween.update(at(50)).value, Argb(0xFF80_8080));
        assert_eq!(tween.update(at(100)).value, Argb::WHITE);
    }

    fn settle<T: Linear>(spring: &mut Spring<T>, from: u64, frame: u64) -> u64 {
        let mut now = from;
        while spring.update(at(now)).animating {
            now += frame;
            assert!(now < from + 10_000, "the spring came to rest");
        }
        now
    }

    #[test]
    fn a_spring_converges_and_comes_to_rest_exactly_at_its_target() {
        for params in [
            SpringParams::SNAPPY,
            SpringParams::GENTLE,
            SpringParams::BOUNCY,
            SpringParams::critical(ms(300)),
        ] {
            let mut spring = Spring::new(0.0_f32, params);
            spring.update(at(0));
            spring.set_target(at(0), 300.0);
            assert!(spring.is_animating());
            settle(&mut spring, 16, 16);
            assert_eq!(spring.value(), 300.0, "{params:?}");
            assert_eq!(spring.velocity(), 0.0);
            let still = spring.update(at(20_000));
            assert!(!still.animating && !still.changed);
        }
    }

    #[test]
    fn a_critically_damped_spring_arrives_in_about_its_response_and_never_overshoots() {
        let mut spring = Spring::new(0.0_f32, SpringParams::critical(ms(400)));
        spring.update(at(0));
        spring.set_target(at(0), 100.0);
        let mut peak = 0.0_f32;
        for frame in 1..=24_u64 {
            peak = peak.max(spring.update(at(frame * 1000 / 60)).value);
        }
        let at_response = spring.value();
        assert!(at_response > 98.0 && at_response < 99.5, "{at_response}");
        settle(&mut spring, 400, 16);
        assert!(peak <= 100.0);
    }

    #[test]
    fn a_bouncy_spring_overshoots() {
        let mut spring = Spring::new(0.0_f32, SpringParams::BOUNCY);
        spring.update(at(0));
        spring.set_target(at(0), 100.0);
        let peak = (1..60_u64)
            .map(|frame| spring.update(at(frame * 16)).value)
            .fold(0.0_f32, f32::max);
        assert!(peak > 110.0, "{peak}");
    }

    #[test]
    fn a_spring_is_identical_however_its_frames_fall() {
        // 60 Hz, 144 Hz, irregular, and one jump to the end: the same bits.
        let run = |times: &[Duration]| {
            let mut spring = Spring::new([0.0_f32, 50.0], SpringParams::BOUNCY);
            spring.update(Some(Duration::ZERO));
            spring.set_target(Some(Duration::ZERO), [200.0, -80.0]);
            spring.impulse([0.0, 400.0]);
            for time in times {
                spring.update(Some(*time));
            }
            (spring.value(), spring.velocity())
        };
        let end = ms(200);
        let sixty: Vec<_> = (1..=11)
            .map(|f| Duration::from_nanos(f * 16_666_666))
            .chain([end])
            .collect();
        let hundred_forty_four: Vec<_> = (1..=28)
            .map(|f| Duration::from_nanos(f * 6_944_444))
            .chain([end])
            .collect();
        let irregular = [ms(3), ms(4), ms(40), ms(41), ms(97), ms(150), end];
        let one = run(&[end]);
        assert!(
            one.1[0] != 0.0,
            "still moving at the end, or the test is empty"
        );
        assert_eq!(run(&sixty), one);
        assert_eq!(run(&hundred_forty_four), one);
        assert_eq!(run(&irregular), one);
    }

    #[test]
    fn a_retarget_takes_effect_at_its_own_time_not_the_next_frame() {
        let mut early = Spring::new(0.0_f32, SpringParams::SNAPPY);
        let mut late = early;
        early.update(at(0));
        late.update(at(0));
        early.update(at(50));
        early.set_target(at(50), 10.0);
        late.set_target(at(50), 10.0);
        assert_eq!(early.update(at(100)), late.update(at(100)));
    }

    #[test]
    fn a_gap_of_more_than_a_quarter_second_snaps_a_spring_to_its_target() {
        let mut spring = Spring::new(0.0_f32, SpringParams::GENTLE);
        spring.update(at(0));
        spring.set_target(at(0), 50.0);
        spring.update(at(16));
        let gap = spring.update(at(16 + 251));
        assert_eq!(gap.value, 50.0);
        assert!(!gap.animating);

        let mut short = Spring::new(0.0_f32, SpringParams::GENTLE);
        short.update(at(0));
        short.set_target(at(0), 50.0);
        let within = short.update(at(250));
        assert!(within.animating && within.value < 50.0);
    }

    #[test]
    fn an_impulse_sets_a_resting_spring_moving_and_it_comes_back() {
        let mut spring = Spring::new([0.0_f32, 0.0], SpringParams::SNAPPY);
        spring.update(at(0));
        spring.impulse([300.0, 0.0]);
        assert!(spring.is_animating());
        let moved = spring.update(at(50));
        assert!(moved.value[0] > 1.0 && moved.changed);
        assert_eq!(moved.value[1], 0.0);
        settle(&mut spring, 66, 16);
        assert_eq!(spring.value(), [0.0, 0.0]);
    }

    #[test]
    fn a_slow_spring_far_from_zero_still_comes_to_rest() {
        // Where f32 is coarse, the spring must not freeze just short.
        for target in [300.0_f32, 12_345.0, 1.0e6] {
            let mut spring = Spring::new(target - 500.0, SpringParams::GENTLE);
            spring.update(at(0));
            spring.set_target(at(0), target);
            settle(&mut spring, 16, 16);
            assert_eq!(spring.value(), target);
        }
    }

    #[test]
    fn a_spring_with_no_clock_snaps_to_its_target() {
        let mut spring = Spring::new(0.0_f32, SpringParams::SNAPPY);
        spring.set_target(None, 7.0);
        assert_eq!((spring.value(), spring.is_animating()), (7.0, false));

        let mut moving = Spring::new(0.0_f32, SpringParams::SNAPPY);
        moving.update(at(0));
        moving.set_target(at(0), 9.0);
        moving.update(at(30));
        let shown = moving.update(None);
        assert_eq!(
            (shown.value, shown.animating, shown.changed),
            (9.0, false, true)
        );
    }

    #[test]
    fn a_spring_jump_is_at_rest_where_it_landed() {
        let mut spring = Spring::new(0.0_f32, SpringParams::BOUNCY);
        spring.update(at(0));
        spring.set_target(at(0), 100.0);
        spring.update(at(30));
        spring.jump(-4.0);
        assert_eq!(
            (spring.value(), spring.target(), spring.velocity()),
            (-4.0, -4.0, 0.0)
        );
        assert!(!spring.update(at(60)).animating);
    }
}
