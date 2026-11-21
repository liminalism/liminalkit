//! Shapes in time that are worked out from elapsed time alone: intensity
//! envelopes, keyframe tracks, staggered starts, and a trigger to measure
//! them from.
//!
//! None of these hold a value that moves. Each answers "what is it at this
//! elapsed time?", so the same question asked at the same time always gets
//! the same answer, however many frames came between. The only state is a
//! [`Trigger`]: when the thing was fired.
//!
//! EXPERIMENTAL: the API is new.

use std::time::Duration;

use super::{Ease, Lerp};

/// How many times an [`Envelope`] plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Repeat {
    /// Once.
    Once,
    /// This many times in all, back to back. `Times(0)` is taken as once.
    Times(u32),
    /// Over and over, resting at 0 for `gap` between plays.
    Forever {
        /// The pause after each play, before the next begins.
        gap: Duration,
    },
}

/// An attack / hold / release intensity curve, 0..1.
///
/// It rises from 0 to 1 over `attack`, stays at 1 for `hold`, and falls back
/// to 0 over `release`: a flash on a hit, a glow on a pickup, a pulse on a
/// waypoint. It is a function of the time since it began (see [`Trigger`]),
/// and is 0 before it begins and once it has finished.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Envelope {
    /// The rise from 0 to 1.
    pub attack: Duration,
    /// Time spent at 1.
    pub hold: Duration,
    /// The fall from 1 to 0.
    pub release: Duration,
    /// The shape of the rise.
    pub attack_ease: Ease,
    /// The shape of the fall, as progress from 1 down to 0: the intensity
    /// during the release is `1 - release_ease.apply(progress)`. An ease out
    /// here falls fast and lingers.
    pub release_ease: Ease,
    /// How many times it plays.
    pub repeat: Repeat,
}

impl Envelope {
    /// A single flash: up over `attack`, held, and back down over `release`.
    ///
    /// Both ends ease out. The rise is quick and slows into the peak; the
    /// fall drops fast and lingers in a tail, which is what a flash of light
    /// does and why a hit reads as a hit.
    pub const fn flash(attack: Duration, hold: Duration, release: Duration) -> Envelope {
        Envelope {
            attack,
            hold,
            release,
            attack_ease: Ease::CubicOut,
            release_ease: Ease::CubicOut,
            repeat: Repeat::Once,
        }
    }

    /// The same envelope, played as `repeat` says.
    pub const fn repeat(mut self, repeat: Repeat) -> Envelope {
        self.repeat = repeat;
        self
    }

    /// One play: attack, hold and release.
    pub fn period(&self) -> Duration {
        self.attack
            .saturating_add(self.hold)
            .saturating_add(self.release)
    }

    /// The intensity, 0 to 1 for the eases that do not overshoot, at
    /// `elapsed` since it began. 0 after it has finished.
    pub fn at(&self, elapsed: Duration) -> f32 {
        let period = self.period();
        if period.is_zero() {
            return 0.0;
        }
        let local = match self.repeat {
            Repeat::Once | Repeat::Times(_) => {
                if self.finished(elapsed) {
                    return 0.0;
                }
                remainder(elapsed, period)
            }
            Repeat::Forever { gap } => {
                let local = remainder(elapsed, period.saturating_add(gap));
                if local >= period {
                    return 0.0;
                }
                local
            }
        };
        self.shape(local)
    }

    /// The intensity within one play, `local` from its start.
    fn shape(&self, local: Duration) -> f32 {
        if local < self.attack {
            return self.attack_ease.apply(fraction(local, self.attack));
        }
        let held = self.attack.saturating_add(self.hold);
        if local < held {
            return 1.0;
        }
        if self.release.is_zero() {
            return 0.0;
        }
        1.0 - self
            .release_ease
            .apply(fraction(local.saturating_sub(held), self.release))
    }

    /// How long it lasts from the start of the first play to the end of the
    /// last, or `None` if it repeats forever.
    pub fn length(&self) -> Option<Duration> {
        let plays = match self.repeat {
            Repeat::Once => 1,
            Repeat::Times(n) => n.max(1),
            Repeat::Forever { .. } => return None,
        };
        Some(self.period().checked_mul(plays).unwrap_or(Duration::MAX))
    }

    /// Whether it is over at `elapsed`: never, for one that repeats forever.
    pub fn finished(&self, elapsed: Duration) -> bool {
        self.length().is_some_and(|length| elapsed >= length)
    }
}

/// `a` modulo `b`, for a non-zero `b`.
fn remainder(a: Duration, b: Duration) -> Duration {
    let nanos = a.as_nanos() % b.as_nanos();
    // The remainder is below `b`, which came from a Duration, so it fits.
    Duration::new(
        u64::try_from(nanos / 1_000_000_000).unwrap_or(u64::MAX),
        (nanos % 1_000_000_000) as u32,
    )
}

/// How far `part` is through `whole`, 0 to 1.
fn fraction(part: Duration, whole: Duration) -> f32 {
    if whole.is_zero() {
        return 1.0;
    }
    (part.as_secs_f64() / whole.as_secs_f64()).min(1.0) as f32
}

/// When something was fired, in host time.
///
/// The one piece of state behind an effect driven by an [`Envelope`] or a
/// keyframe track: fire it when the event happens, and ask it each frame
/// how long ago that was.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Trigger {
    fired: Option<Duration>,
}

impl Trigger {
    /// Mark it fired at `now`. Firing again restarts it.
    ///
    /// With no clock (`now` is `None`) it is left unfired: there is no later
    /// frame to play an effect in, so the effect is over at once.
    pub fn fire(&mut self, now: Option<Duration>) {
        self.fired = now;
    }

    /// How long ago it was fired, or `None` if it has not been or there is
    /// no clock. A `now` before the firing counts as the moment of firing.
    pub fn elapsed(&self, now: Option<Duration>) -> Option<Duration> {
        Some(now?.saturating_sub(self.fired?))
    }

    /// Forget the firing.
    pub fn clear(&mut self) {
        self.fired = None;
    }

    /// Whether it has been fired (and not cleared).
    pub fn is_fired(&self) -> bool {
        self.fired.is_some()
    }
}

/// One point of a keyframe track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key<T> {
    /// When, from the start of the track.
    pub at: Duration,
    /// The value there.
    pub value: T,
    /// The ease used arriving *at* this key from the one before. The first
    /// key's ease is not used.
    pub ease: Ease,
}

impl<T> Key<T> {
    /// A key at `at` with `value`, arrived at along `ease`.
    pub const fn new(at: Duration, value: T, ease: Ease) -> Key<T> {
        Key { at, value, ease }
    }
}

/// The value of a keyframe track at `elapsed`, or `None` for a track with no
/// keys.
///
/// Keys are expected in order of time. Before the first key the value is
/// the first key's; after the last, the last's. Two keys at the same time
/// make a jump.
pub fn keyframes<T: Lerp>(keys: &[Key<T>], elapsed: Duration) -> Option<T> {
    let first = keys.first()?;
    if elapsed <= first.at {
        return Some(first.value);
    }
    for pair in keys.windows(2) {
        let (from, to) = (&pair[0], &pair[1]);
        if elapsed < to.at {
            let span = to.at.saturating_sub(from.at);
            let progress = fraction(elapsed.saturating_sub(from.at), span);
            return Some(T::lerp(from.value, to.value, to.ease.apply(progress)));
        }
    }
    keys.last().map(|key| key.value)
}

/// The delay for the `index`th of a row of things that start one after
/// another, `step` apart: `step * index`, saturating.
pub fn stagger(index: usize, step: Duration) -> Duration {
    let index = u32::try_from(index).unwrap_or(u32::MAX);
    step.checked_mul(index).unwrap_or(Duration::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn linear(attack: u64, hold: u64, release: u64, repeat: Repeat) -> Envelope {
        Envelope {
            attack: ms(attack),
            hold: ms(hold),
            release: ms(release),
            attack_ease: Ease::Linear,
            release_ease: Ease::Linear,
            repeat,
        }
    }

    #[test]
    fn an_envelope_rises_holds_and_falls() {
        let envelope = linear(100, 50, 200, Repeat::Once);
        assert_eq!(envelope.at(ms(0)), 0.0);
        assert!((envelope.at(ms(50)) - 0.5).abs() < 1e-6);
        assert_eq!(envelope.at(ms(100)), 1.0);
        assert_eq!(envelope.at(ms(149)), 1.0);
        assert_eq!(envelope.at(ms(150)), 1.0, "the release starts at 1");
        assert!((envelope.at(ms(250)) - 0.5).abs() < 1e-6);
        assert!(envelope.at(ms(349)) > 0.0);
        assert_eq!(envelope.at(ms(350)), 0.0);
        assert_eq!(envelope.at(ms(10_000)), 0.0, "zero after the end");
        assert_eq!(envelope.length(), Some(ms(350)));
        assert!(!envelope.finished(ms(349)));
        assert!(envelope.finished(ms(350)));
    }

    #[test]
    fn a_flash_eases_out_on_both_sides() {
        let flash = Envelope::flash(ms(100), ms(0), ms(100));
        assert_eq!(flash.at(ms(50)), Ease::CubicOut.apply(0.5));
        assert_eq!(flash.at(ms(150)), 1.0 - Ease::CubicOut.apply(0.5));
        assert_eq!(flash.at(ms(100)), 1.0);
        assert_eq!(flash.repeat, Repeat::Once);
    }

    #[test]
    fn an_envelope_with_no_time_in_it_is_always_zero() {
        let nothing = linear(0, 0, 0, Repeat::Forever { gap: ms(0) });
        assert_eq!(nothing.at(ms(0)), 0.0);
        assert_eq!(nothing.at(ms(5)), 0.0);
        let instant = linear(0, 100, 0, Repeat::Once);
        assert_eq!(instant.at(ms(0)), 1.0, "no attack starts at full");
        assert_eq!(instant.at(ms(100)), 0.0);
    }

    #[test]
    fn repeating_a_number_of_times_plays_back_to_back() {
        let envelope = linear(100, 0, 100, Repeat::Times(3));
        assert_eq!(envelope.length(), Some(ms(600)));
        assert_eq!(envelope.at(ms(100)), 1.0);
        assert_eq!(envelope.at(ms(300)), 1.0, "the second peak");
        assert_eq!(envelope.at(ms(500)), 1.0, "the third peak");
        assert!((envelope.at(ms(450)) - 0.5).abs() < 1e-6);
        assert_eq!(envelope.at(ms(600)), 0.0);
        assert!(envelope.finished(ms(600)));
        let zero = linear(100, 0, 100, Repeat::Times(0));
        assert_eq!(zero.length(), Some(ms(200)), "zero times is once");
    }

    #[test]
    fn repeating_forever_rests_for_the_gap_between_plays() {
        let envelope = linear(100, 0, 100, Repeat::Forever { gap: ms(50) });
        assert_eq!(envelope.length(), None);
        assert!(!envelope.finished(Duration::from_secs(3_600)));
        assert_eq!(envelope.at(ms(100)), 1.0);
        assert_eq!(envelope.at(ms(220)), 0.0, "in the gap");
        assert_eq!(envelope.at(ms(250)), 0.0, "the next play starts at 0");
        assert_eq!(envelope.at(ms(350)), 1.0, "and peaks a period later");
        assert_eq!(envelope.at(ms(250 * 40 + 100)), 1.0);
    }

    #[test]
    fn a_trigger_measures_time_since_it_was_fired() {
        let mut trigger = Trigger::default();
        assert!(!trigger.is_fired());
        assert_eq!(trigger.elapsed(Some(ms(100))), None);
        trigger.fire(Some(ms(100)));
        assert!(trigger.is_fired());
        assert_eq!(trigger.elapsed(Some(ms(250))), Some(ms(150)));
        assert_eq!(trigger.elapsed(Some(ms(50))), Some(ms(0)), "saturating");
        assert_eq!(trigger.elapsed(None), None, "no clock, no elapsed time");
        trigger.fire(Some(ms(300)));
        assert_eq!(trigger.elapsed(Some(ms(310))), Some(ms(10)), "refired");
        trigger.clear();
        assert!(!trigger.is_fired());
    }

    #[test]
    fn a_trigger_fired_with_no_clock_stays_unfired() {
        let mut trigger = Trigger::default();
        trigger.fire(Some(ms(5)));
        trigger.fire(None);
        assert!(!trigger.is_fired());
        assert_eq!(trigger.elapsed(Some(ms(10))), None);
    }

    #[test]
    fn keyframes_interpolate_between_keys_and_clamp_at_the_ends() {
        let keys = [
            Key::new(ms(100), 0.0_f32, Ease::Linear),
            Key::new(ms(200), 10.0, Ease::Linear),
            Key::new(ms(400), 20.0, Ease::QuadIn),
        ];
        assert_eq!(keyframes(&keys, ms(0)), Some(0.0));
        assert_eq!(keyframes(&keys, ms(100)), Some(0.0));
        assert_eq!(keyframes(&keys, ms(150)), Some(5.0));
        assert_eq!(keyframes(&keys, ms(200)), Some(10.0));
        assert_eq!(keyframes(&keys, ms(300)), Some(12.5), "the arrival ease");
        assert_eq!(keyframes(&keys, ms(400)), Some(20.0));
        assert_eq!(keyframes(&keys, ms(9_000)), Some(20.0));
        assert_eq!(keyframes::<f32>(&[], ms(10)), None);
        let single = [Key::new(ms(50), [1.0_f32, 2.0], Ease::Linear)];
        assert_eq!(keyframes(&single, ms(0)), Some([1.0, 2.0]));
        assert_eq!(keyframes(&single, ms(90)), Some([1.0, 2.0]));
    }

    #[test]
    fn two_keys_at_the_same_time_make_a_jump() {
        let keys = [
            Key::new(ms(0), 0.0_f32, Ease::Linear),
            Key::new(ms(100), 1.0, Ease::Linear),
            Key::new(ms(100), 5.0, Ease::Linear),
            Key::new(ms(200), 5.0, Ease::Linear),
        ];
        assert_eq!(keyframes(&keys, ms(99)), Some(0.99));
        assert_eq!(keyframes(&keys, ms(100)), Some(5.0));
    }

    #[test]
    fn stagger_spaces_starts_evenly_and_saturates() {
        assert_eq!(stagger(0, ms(40)), ms(0));
        assert_eq!(stagger(3, ms(40)), ms(120));
        assert_eq!(stagger(usize::MAX, Duration::MAX), Duration::MAX);
    }
}
