//! What a wheel movement is worth.
//!
//! A [`WheelInterpreter`] takes [`WheelSample`]s one at a time and says how
//! far each should move whatever is being scrolled. It holds no scroll
//! position and knows nothing of bounds, pages or animation: it answers one
//! question, *how far*, and leaves *where* and *how it looks getting there*
//! to whoever asked. An application with its own scroll model uses this and
//! nothing else from the module.
//!
//! Three things happen to a sample, in order.
//!
//! **It is classed.** Recordings of real devices on Windows (kept under
//! `tests/scroll_traces`) show what there is to go on. Two mouse wheels, one
//! with coarse detents and one with fine ones, both send whole notches and
//! nothing else; they differ only in how fast the notches come, 70 ms apart
//! or more against 18 ms. A precision touchpad sends small fractions of a
//! notch every 18 to 21 ms and never a whole one. So the class is read from
//! the value: a whole notch is a wheel ([`InputClass::Notched`]), a fraction
//! is something continuous ([`InputClass::Fine`]), and a pixel delta or a
//! phased gesture is a touchpad that says so ([`InputClass::Precise`]). The
//! reading is deliberately slow to change its mind; see [`Classifier`].
//!
//! **It is converted** to the caller's unit, by the caller's own idea of a
//! line and a page ([`Units`]).
//!
//! **It is scaled.** Each class has a sensitivity, and whole notches can be
//! accelerated by *cadence*: how quickly they follow one another, never how
//! large one of them is. A wheel turned slowly moves a little for each
//! notch, and the same wheel spun moves much further per notch. Continuous
//! input is never accelerated: a touchpad's own driver has already decided
//! how far a stroke goes, and the same recordings show its stream carrying
//! on after the fingers lift, which is the system coasting on the user's
//! behalf.
//!
//! EXPERIMENTAL. The numbers in the presets are starting points chosen from
//! three recordings on one machine. Horizontal movement, the Shift and Ctrl
//! behaviour and every platform but Windows are untested on hardware.

use super::event::{GesturePhase, ScrollSettings, SourceHint, WheelLines, WheelSample, WheelUnit};
use crate::input::Modifiers;
use std::time::Duration;

/// What kind of input a sample appears to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputClass {
    /// Whole wheel notches. Accelerated by cadence, and worth smoothing,
    /// because each one is a jump.
    Notched,
    /// Fractions of a notch with no gesture phases: a touchpad on a platform
    /// that reports it as a wheel, or a wheel with no detents. Already
    /// continuous; never accelerated.
    Fine,
    /// Pixel deltas or a phased gesture: a touchpad that reports itself as
    /// one. Applied as it comes.
    Precise,
}

/// An override for when the automatic reading is wrong for a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeviceMode {
    /// Read the class from the input.
    #[default]
    Auto,
    /// Everything is a notched wheel.
    Wheel,
    /// Everything is continuous wheel input.
    SmoothWheel,
    /// Everything is a touchpad.
    Touchpad,
}

/// A modifier key, for the profile's choices of which one does what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierKey {
    Shift,
    Control,
    Alt,
    Logo,
}

impl ModifierKey {
    pub fn held(self, modifiers: Modifiers) -> bool {
        match self {
            ModifierKey::Shift => modifiers.shift,
            ModifierKey::Control => modifiers.control,
            ModifierKey::Alt => modifiers.alt,
            ModifierKey::Logo => modifiers.logo,
        }
    }
}

/// How notches are accelerated by the rate they arrive at.
///
/// The gain is 1 while notches come `slow` apart or more, rises as the gap
/// shortens, and reaches `max_gain` when they come `fast` apart or less.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cadence {
    /// The gap between notches at and above which there is no acceleration.
    pub slow: Duration,
    /// The gap at and below which acceleration is at its most.
    pub fast: Duration,
    /// How many times further a notch goes at full acceleration.
    pub max_gain: f64,
    /// The shape of the rise between `slow` and `fast`: 1 is a straight
    /// line, 2 holds back until the wheel is turning quickly.
    pub curve: f64,
    /// Notches in a row, the same way, before any acceleration at all. The
    /// first notch of a roll is never accelerated, so one deliberate click
    /// is always one step.
    pub min_streak: u32,
    /// A pause this long starts the count again.
    pub reset_gap: Duration,
    /// How much of each new gap goes into the running estimate, from just
    /// above 0 (slow to respond) to 1 (the latest gap alone).
    pub smoothing: f64,
}

/// How the interpreter turns samples into distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelProfile {
    /// Multiplier for whole notches.
    pub notch_sensitivity: f64,
    /// Multiplier for fractional wheel input.
    pub fine_sensitivity: f64,
    /// Multiplier for pixel and gesture input.
    pub precise_sensitivity: f64,
    /// Acceleration of notches by their rate. `None` for none.
    pub cadence: Option<Cadence>,
    /// A key that, held, multiplies wheel movement by `fast_multiplier`.
    pub fast_modifier: Option<ModifierKey>,
    pub fast_multiplier: f64,
    /// Whether Shift turns a vertical wheel into a horizontal one. Ignored
    /// on macOS, where the system has already done it.
    pub shift_swaps_axes: bool,
    /// The key that turns the wheel into zoom, reported as
    /// [`Interpreted::Zoom`] and not scrolled. `None` to scroll regardless.
    pub zoom_modifier: Option<ModifierKey>,
    pub device_mode: DeviceMode,
    /// A pause this long forgets what the input was classed as, so that
    /// putting down one device and picking up another starts afresh.
    pub class_reset: Duration,
    /// A pause this long ends a gesture.
    pub gesture_gap: Duration,
}

impl WheelProfile {
    /// No acceleration: every notch is worth the same. For text being
    /// written, where the view should go exactly as far as it was told.
    pub fn precision() -> WheelProfile {
        WheelProfile {
            notch_sensitivity: 1.0,
            fine_sensitivity: 1.0,
            precise_sensitivity: 1.0,
            cadence: None,
            fast_modifier: None,
            fast_multiplier: 1.0,
            shift_swaps_axes: true,
            zoom_modifier: Some(ModifierKey::Control),
            device_mode: DeviceMode::Auto,
            class_reset: Duration::from_secs(1),
            gesture_gap: Duration::from_millis(150),
        }
    }

    /// A little acceleration for a wheel that is being spun; none for one
    /// being clicked. The default.
    pub fn natural() -> WheelProfile {
        WheelProfile {
            cadence: Some(Cadence {
                slow: Duration::from_millis(110),
                fast: Duration::from_millis(30),
                max_gain: 2.0,
                curve: 2.0,
                min_streak: 3,
                reset_gap: Duration::from_millis(320),
                smoothing: 0.5,
            }),
            fast_modifier: Some(ModifierKey::Alt),
            fast_multiplier: 4.0,
            ..WheelProfile::precision()
        }
    }

    /// Enough acceleration to cross a long document with a few turns.
    pub fn accelerated() -> WheelProfile {
        WheelProfile {
            cadence: Some(Cadence {
                slow: Duration::from_millis(140),
                fast: Duration::from_millis(30),
                max_gain: 3.5,
                curve: 2.0,
                min_streak: 2,
                reset_gap: Duration::from_millis(320),
                smoothing: 0.5,
            }),
            fast_modifier: Some(ModifierKey::Alt),
            fast_multiplier: 5.0,
            ..WheelProfile::precision()
        }
    }

    /// For getting somewhere in hundreds of pages: a spun wheel covers
    /// several times the distance per notch.
    pub fn rapid_navigation() -> WheelProfile {
        WheelProfile {
            cadence: Some(Cadence {
                slow: Duration::from_millis(160),
                fast: Duration::from_millis(25),
                max_gain: 8.0,
                curve: 1.5,
                min_streak: 2,
                reset_gap: Duration::from_millis(350),
                smoothing: 0.5,
            }),
            fast_modifier: Some(ModifierKey::Alt),
            fast_multiplier: 5.0,
            ..WheelProfile::precision()
        }
    }

    /// The profile, or what is wrong with it.
    pub fn validate(self) -> Result<WheelProfile, &'static str> {
        let positive = |value: f64| value.is_finite() && value > 0.0;
        if !positive(self.notch_sensitivity)
            || !positive(self.fine_sensitivity)
            || !positive(self.precise_sensitivity)
        {
            return Err("sensitivities must be finite and above zero");
        }
        if !positive(self.fast_multiplier) {
            return Err("the fast multiplier must be finite and above zero");
        }
        if let Some(cadence) = self.cadence {
            if cadence.fast.is_zero() || cadence.fast >= cadence.slow {
                return Err("the fast gap must be above zero and shorter than the slow gap");
            }
            if cadence.reset_gap < cadence.slow {
                return Err("the reset gap must be at least the slow gap");
            }
            if !cadence.max_gain.is_finite() || !(1.0..=50.0).contains(&cadence.max_gain) {
                return Err("the largest gain must be between 1 and 50");
            }
            if !cadence.curve.is_finite() || !(0.25..=8.0).contains(&cadence.curve) {
                return Err("the curve must be between 0.25 and 8");
            }
            if cadence.min_streak == 0 {
                return Err("the streak must be at least one notch");
            }
            if !cadence.smoothing.is_finite() || cadence.smoothing <= 0.0 || cadence.smoothing > 1.0
            {
                return Err("the smoothing must be above 0 and at most 1");
            }
        }
        Ok(self)
    }
}

impl Default for WheelProfile {
    fn default() -> WheelProfile {
        WheelProfile::natural()
    }
}

/// What a line, a page and a pixel are worth, in whatever unit the caller
/// scrolls in.
///
/// A table scrolling in pixels passes its row height as the line and its
/// viewport as the page, with a pixel worth 1. An editor that keeps its
/// position in pages passes the fraction of a page a line is, 1 for the
/// page, and the fraction of a page a pixel is. The interpreter never needs
/// to know which.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Units {
    /// One wheel line, per axis `(x, y)`.
    pub line: (f64, f64),
    /// One page, per axis.
    pub page: (f64, f64),
    /// One physical pixel.
    pub pixel: f64,
    pub settings: ScrollSettings,
}

impl Units {
    /// Units for something that scrolls in physical pixels.
    pub fn pixels(line: (f64, f64), page: (f64, f64)) -> Units {
        Units {
            line,
            page,
            pixel: 1.0,
            settings: ScrollSettings::default(),
        }
    }
}

/// How far one sample moves things.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scroll {
    /// The distance, `(x, y)`, in the caller's unit and in the direction a
    /// scroll offset moves: positive is further right and further down the
    /// content. This is the opposite sign to the sample, whose positive `y`
    /// is the wheel turning away from the user.
    pub displacement: (f64, f64),
    pub class: InputClass,
    /// Whether the class has been seen often enough to be relied on. A lone
    /// first notch is classed as a notch but is not yet settled.
    pub settled: bool,
    /// Passed through from the sample: the system coasting after the
    /// fingers lifted. Whoever applies this must not add a fling of its own
    /// to a stream that says `Some(true)`.
    pub momentum: Option<bool>,
    /// The first sample after a pause, or one whose phase says a gesture
    /// started.
    pub gesture_began: bool,
    /// The acceleration applied, 1 for none. For a readout while tuning.
    pub gain: f64,
}

/// A wheel movement that is zoom, not scroll, handed back unconverted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    /// Wheel notches, possibly fractional, when the input has notches.
    /// Positive is the wheel turning away from the user: zoom in.
    pub notches: Option<f64>,
    /// Physical pixels, when the input is in pixels.
    pub pixels: Option<f64>,
}

/// What a sample turned out to be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Interpreted {
    Scroll(Scroll),
    Zoom(Zoom),
}

/// Reads the class of the input from the samples themselves.
///
/// It is conservative in one direction. A fraction of a notch is conclusive:
/// a wheel with detents cannot send one, so a single fraction makes the
/// input [`InputClass::Fine`] at once. A whole notch proves much less,
/// because continuous input lands on a whole number now and then, so it
/// takes three in a row to turn `Fine` back into `Notched`. From nothing, a
/// whole notch is taken for a notch straight away (treating it as one does
/// no harm, and a wheel's first click should behave like its second) but is
/// reported as unsettled until a second arrives.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Classifier {
    class: Option<InputClass>,
    /// Whole-notch samples in a row.
    whole_run: u32,
    last: Option<Duration>,
}

impl Classifier {
    /// The class of `sample`, and whether it is settled.
    pub fn classify(
        &mut self,
        sample: &WheelSample,
        mode: DeviceMode,
        reset: Duration,
    ) -> (InputClass, bool) {
        if self
            .last
            .is_some_and(|last| sample.at.saturating_sub(last) > reset)
        {
            self.class = None;
            self.whole_run = 0;
        }
        self.last = Some(sample.at);
        match mode {
            DeviceMode::Wheel => return (InputClass::Notched, true),
            DeviceMode::SmoothWheel => return (InputClass::Fine, true),
            DeviceMode::Touchpad => return (InputClass::Precise, true),
            DeviceMode::Auto => {}
        }
        if sample.unit == WheelUnit::Pixels
            || sample.phase != GesturePhase::Moved
            || sample.momentum.is_some()
        {
            self.class = Some(InputClass::Precise);
            self.whole_run = 0;
            return (InputClass::Precise, true);
        }
        let quantum = match sample.unit {
            WheelUnit::Windows120 => 120.0,
            _ => 1.0,
        };
        let whole = |value: f64| (value / quantum).fract() == 0.0;
        let is_notch = whole(sample.x)
            && whole(sample.y)
            && (sample.x != 0.0 || sample.y != 0.0)
            && sample.source != SourceHint::Touchpad;
        if !is_notch {
            self.class = Some(InputClass::Fine);
            self.whole_run = 0;
            return (InputClass::Fine, true);
        }
        self.whole_run = self.whole_run.saturating_add(1);
        match self.class {
            Some(InputClass::Fine) if self.whole_run < 3 => (InputClass::Fine, true),
            _ => {
                self.class = Some(InputClass::Notched);
                (InputClass::Notched, self.whole_run >= 2)
            }
        }
    }
}

/// The rate notches are arriving at on one axis, and the gain it earns.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CadenceTracker {
    last: Option<Duration>,
    direction: i8,
    /// Running estimate of the gap between notches, in seconds.
    gap: Option<f64>,
    streak: u32,
}

impl CadenceTracker {
    pub fn reset(&mut self) {
        *self = CadenceTracker::default();
    }

    /// A notch event arrived at `at`, going `direction`; the gain for it.
    ///
    /// One event is one observation of the rate, however many notches it
    /// carries. A 240 arriving alone says the wheel moved two notches; it
    /// does not say when the first of them happened, and inventing a time
    /// for it would be inventing a rate. `waited` marks an event that sat in
    /// the queue, whose arrival time is the queue's doing and is left out of
    /// the estimate for the same reason.
    pub fn notch(&mut self, at: Duration, direction: i8, waited: bool, cadence: &Cadence) -> f64 {
        let gap = self.last.map(|last| at.saturating_sub(last));
        let continues =
            self.direction == direction && gap.is_some_and(|gap| gap <= cadence.reset_gap);
        self.last = Some(at);
        self.direction = direction;
        if !continues {
            self.gap = None;
            self.streak = 1;
            return 1.0;
        }
        self.streak = self.streak.saturating_add(1);
        let seconds = gap.unwrap_or_default().as_secs_f64();
        // Two events in the same millisecond were queued together.
        if !waited && seconds >= 0.001 {
            self.gap = Some(match self.gap {
                Some(estimate) => estimate + cadence.smoothing * (seconds - estimate),
                None => seconds,
            });
        }
        let Some(estimate) = self.gap else {
            return 1.0;
        };
        if self.streak < cadence.min_streak {
            return 1.0;
        }
        let slow = cadence.slow.as_secs_f64();
        let fast = cadence.fast.as_secs_f64();
        let boost = ((slow - estimate) / (slow - fast)).clamp(0.0, 1.0);
        1.0 + (cadence.max_gain - 1.0) * boost.powf(cadence.curve)
    }
}

/// Turns wheel samples into distance. See the module documentation.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WheelInterpreter {
    profile: WheelProfile,
    classifier: Classifier,
    cadence: [CadenceTracker; 2],
    last: Option<Duration>,
}

impl WheelInterpreter {
    pub fn new(profile: WheelProfile) -> WheelInterpreter {
        WheelInterpreter {
            profile,
            ..WheelInterpreter::default()
        }
    }

    pub fn profile(&self) -> &WheelProfile {
        &self.profile
    }

    /// Change the profile. What has been learnt about the input is kept; the
    /// notch rate is forgotten, since the new profile may measure it
    /// differently.
    pub fn set_profile(&mut self, profile: WheelProfile) {
        self.profile = profile;
        self.cadence = Default::default();
    }

    /// Forget the notch rate, because the view was moved by something other
    /// than the wheel: a jump to a search result, a drag of the scrollbar.
    /// The next notch then starts a fresh roll and is not accelerated by a
    /// roll that ended somewhere else.
    pub fn interrupt(&mut self) {
        self.cadence = Default::default();
    }

    /// How far `sample` moves things, in `units`.
    pub fn interpret(&mut self, sample: &WheelSample, units: &Units) -> Interpreted {
        let profile = self.profile;
        let gesture_began = sample.phase == GesturePhase::Started
            || self
                .last
                .is_none_or(|last| sample.at.saturating_sub(last) > profile.gesture_gap);
        self.last = Some(sample.at);
        let (class, settled) =
            self.classifier
                .classify(sample, profile.device_mode, profile.class_reset);

        if profile
            .zoom_modifier
            .is_some_and(|key| key.held(sample.modifiers))
        {
            self.cadence = Default::default();
            let notches = sample.notches().map(|(_, y)| y);
            return Interpreted::Zoom(Zoom {
                notches,
                pixels: notches.is_none().then_some(sample.y),
            });
        }

        // The plain conversion, then into the offset's direction.
        let (mut x, mut y) = sample.pixels(&units.settings, units.line, units.page);
        if sample.unit == WheelUnit::Pixels {
            x *= units.pixel;
            y *= units.pixel;
        }
        let (mut x, mut y) = (-x, -y);

        let swap = profile.shift_swaps_axes
            && sample.modifiers.shift
            && class != InputClass::Precise
            && sample.x == 0.0
            && !cfg!(target_os = "macos");
        if swap {
            // A line of text is not a character wide: a sideways notch is
            // worth the horizontal line, not the vertical one.
            let notches = sample.notches().map_or(0.0, |(_, y)| y);
            x = -notches * units.settings.wheel_chars * units.line.0;
            y = 0.0;
        }

        let mut gain = 1.0;
        let sensitivity = match class {
            InputClass::Notched => {
                // A page a notch is a page a notch: accelerating it would
                // skip pages nobody saw.
                let paged = units.settings.wheel_lines == WheelLines::Page && !swap;
                if let (Some(cadence), false) = (profile.cadence, paged) {
                    for (axis, value) in [x, y].into_iter().enumerate() {
                        if value != 0.0 {
                            let direction = if value > 0.0 { 1 } else { -1 };
                            let earned = self.cadence[axis].notch(
                                sample.at,
                                direction,
                                sample.after_busy,
                                &cadence,
                            );
                            gain = f64::max(gain, earned);
                        }
                    }
                }
                profile.notch_sensitivity
            }
            InputClass::Fine => profile.fine_sensitivity,
            InputClass::Precise => profile.precise_sensitivity,
        };
        if class != InputClass::Notched {
            self.cadence = Default::default();
        }
        let mut factor = sensitivity * gain;
        if class != InputClass::Precise
            && profile
                .fast_modifier
                .is_some_and(|key| key.held(sample.modifiers))
        {
            factor *= profile.fast_multiplier;
        }
        Interpreted::Scroll(Scroll {
            displacement: (x * factor, y * factor),
            class,
            settled,
            momentum: sample.momentum,
            gesture_began,
            gain,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scroll::WheelTrace;

    const NOTCHED: &str = include_str!("../../tests/scroll_traces/notched-wheel.trace");
    const FINE_NOTCHED: &str = include_str!("../../tests/scroll_traces/fine-notched-wheel.trace");
    const TOUCHPAD: &str = include_str!("../../tests/scroll_traces/precision-touchpad.trace");
    const MIXED: &str = include_str!("../../tests/scroll_traces/touchpad-then-wheel-native.trace");

    fn trace(text: &str) -> WheelTrace {
        WheelTrace::from_text(text).expect("a readable recording")
    }

    /// A table of 20 px rows in a 600 px viewport.
    fn units() -> Units {
        Units::pixels((8.0, 20.0), (800.0, 600.0))
    }

    fn run(text: &str, profile: WheelProfile) -> Vec<Scroll> {
        let mut interpreter = WheelInterpreter::new(profile);
        trace(text)
            .samples
            .iter()
            .map(|sample| match interpreter.interpret(sample, &units()) {
                Interpreted::Scroll(scroll) => scroll,
                Interpreted::Zoom(_) => panic!("nothing in the recordings is a zoom"),
            })
            .collect()
    }

    fn notch(ms: u64, raw: f64) -> WheelSample {
        let mut sample = WheelSample::from_lines(
            0.0,
            0.0,
            true,
            GesturePhase::Moved,
            Duration::from_millis(ms),
        );
        sample.y = raw;
        sample
    }

    fn scroll(interpreter: &mut WheelInterpreter, sample: &WheelSample) -> Scroll {
        match interpreter.interpret(sample, &units()) {
            Interpreted::Scroll(scroll) => scroll,
            Interpreted::Zoom(_) => panic!("expected a scroll"),
        }
    }

    #[test]
    fn both_recorded_mouse_wheels_are_notched_throughout() {
        // The coarse wheel and the finely graded one send the same thing,
        // whole notches; nothing in the values tells them apart.
        for text in [NOTCHED, FINE_NOTCHED] {
            let samples = trace(text).samples;
            let scrolls = run(text, WheelProfile::natural());
            assert!(scrolls.len() > 40);
            assert!(scrolls.iter().all(|s| s.class == InputClass::Notched));
            // One notch is not yet a pattern: the first, and the first after
            // a pause long enough for the device to have been put down, are
            // classed but not settled. Every other one is.
            for (index, scroll) in scrolls.iter().enumerate() {
                let fresh = index == 0
                    || samples[index].at - samples[index - 1].at > Duration::from_secs(1);
                assert_eq!(scroll.settled, !fresh, "sample {index}");
            }
        }
    }

    #[test]
    fn the_recorded_touchpad_is_never_mistaken_for_a_wheel() {
        let scrolls = run(TOUCHPAD, WheelProfile::rapid_navigation());
        assert!(scrolls.len() > 100);
        assert!(scrolls.iter().all(|s| s.class == InputClass::Fine));
        assert!(
            scrolls.iter().all(|s| s.gain == 1.0),
            "continuous input is never accelerated, whatever the profile"
        );
    }

    #[test]
    fn a_touchpad_and_then_a_wheel_in_one_recording_are_each_read_for_what_they_are() {
        // One sitting: three touchpad strokes, a pause, then a wheel spun
        // both ways. The touchpad reached 145/120ths in one message, more
        // than a notch, and still never landed on a whole one.
        let samples = trace(MIXED).samples;
        let scrolls = run(MIXED, WheelProfile::accelerated());
        let (mut touchpad, mut wheel) = (0, 0);
        for (sample, scroll) in samples.iter().zip(&scrolls) {
            if sample.y % 120.0 == 0.0 {
                assert_eq!(scroll.class, InputClass::Notched);
                wheel += 1;
            } else {
                assert_eq!(scroll.class, InputClass::Fine);
                assert_eq!(scroll.gain, 1.0);
                touchpad += 1;
            }
        }
        assert_eq!((touchpad, wheel), (89, 61));
        // The system was asked, message by message, what sent each of
        // these, and said "touchpad" for none of them.
        assert!(samples
            .iter()
            .all(|sample| sample.source == crate::scroll::SourceHint::Unknown));
    }

    /// Not a test: prints, for each recording and each preset, how far the
    /// recording goes and how hard it is accelerated. Run it when changing a
    /// preset, to see the change in numbers before judging it by hand:
    ///
    /// ```sh
    /// cargo test -p liminalkit-shell --lib preset_report -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "a report for tuning, not a check"]
    fn preset_report() {
        let presets = [
            ("precision", WheelProfile::precision()),
            ("natural", WheelProfile::natural()),
            ("accelerated", WheelProfile::accelerated()),
            ("rapid", WheelProfile::rapid_navigation()),
        ];
        println!("distance is in notches' worth; a notch here is 60 px");
        for (name, text) in [
            ("coarse wheel", NOTCHED),
            ("fine wheel", FINE_NOTCHED),
            ("touchpad", TOUCHPAD),
            ("touchpad then wheel", MIXED),
        ] {
            println!("{name}");
            for (preset, profile) in presets {
                let scrolls = run(text, profile);
                let travelled: f64 = scrolls.iter().map(|s| s.displacement.1.abs()).sum();
                let peak = scrolls.iter().fold(1.0_f64, |m, s| m.max(s.gain));
                let boosted = scrolls.iter().filter(|s| s.gain > 1.0).count();
                let mean = scrolls.iter().map(|s| s.gain).sum::<f64>() / scrolls.len() as f64;
                let distance = travelled / 60.0;
                let count = scrolls.len();
                println!(
                    "  {preset:<12} travelled {distance:>7.1}  mean gain {mean:>5.2}  peak {peak:>5.2}  accelerated {boosted:>3} of {count}"
                );
            }
        }
    }

    /// Not a test: times the interpreter and the motion controller.
    ///
    /// ```sh
    /// cargo test --release -p liminalkit-shell --lib cost_report -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "a measurement, not a check"]
    fn cost_report() {
        use crate::scroll::{MotionController, Smoothing};
        let samples = trace(MIXED).samples;
        let rounds = 20_000;
        let mut interpreter = WheelInterpreter::new(WheelProfile::accelerated());
        let mut motion = MotionController::new();
        motion.set_bounds(0.0, 1.0e12);
        let ease = Smoothing::Exponential {
            time: Duration::from_millis(40),
        };
        let started = std::time::Instant::now();
        let mut checksum = 0.0;
        for round in 0..rounds {
            let base = Duration::from_secs(round * 20);
            for sample in &samples {
                let mut sample = *sample;
                sample.at += base;
                if let Interpreted::Scroll(scroll) = interpreter.interpret(&sample, &units()) {
                    motion.wheel(sample.at, scroll.displacement.1, ease, 0.2);
                    motion.advance(sample.at + Duration::from_millis(8));
                    checksum += motion.position();
                }
            }
        }
        let each = started.elapsed().as_nanos() as f64 / (rounds as f64 * samples.len() as f64);
        println!("interpret + wheel + advance: {each:.0} ns a sample (checksum {checksum:e})");
    }

    #[test]
    fn without_acceleration_every_recording_moves_exactly_what_it_said() {
        // No drift and nothing dropped: the total is the plain conversion of
        // every sample, to the last bit.
        for text in [NOTCHED, FINE_NOTCHED, TOUCHPAD] {
            let recording = trace(text);
            let expected = recording.samples.iter().fold(0.0, |sum, sample| {
                sum - sample
                    .pixels(&ScrollSettings::default(), (8.0, 20.0), (800.0, 600.0))
                    .1
            });
            let moved = run(text, WheelProfile::precision())
                .iter()
                .fold(0.0, |sum, scroll| sum + scroll.displacement.1);
            assert_eq!(moved, expected);
        }
    }

    #[test]
    fn a_wheel_clicked_slowly_is_not_accelerated_and_one_spun_is() {
        // The coarse wheel was rolled at its owner's pace: 70 ms between
        // notches at the quickest, mostly 100 to 500. The fine one was spun:
        // down to 18 ms. The same profile leaves the first nearly alone and
        // takes the second to its full gain.
        let profile = WheelProfile::natural();
        let top = profile.cadence.unwrap().max_gain;
        let coarse = run(NOTCHED, profile);
        let fine = run(FINE_NOTCHED, profile);
        let most = |scrolls: &[Scroll]| scrolls.iter().fold(1.0_f64, |m, s| m.max(s.gain));
        assert!(most(&coarse) < 1.2, "coarse wheel: {}", most(&coarse));
        assert!(most(&fine) > top - 0.05, "fine wheel: {}", most(&fine));
        let unaccelerated = coarse.iter().filter(|s| s.gain == 1.0).count();
        assert!(unaccelerated * 10 >= coarse.len() * 8);
    }

    #[test]
    fn the_first_notches_of_a_roll_and_any_reversal_go_exactly_one_step() {
        let recording = trace(FINE_NOTCHED);
        let scrolls = run(FINE_NOTCHED, WheelProfile::rapid_navigation());
        let step = 60.0; // three 20 px rows
        for (index, (sample, scroll)) in recording.samples.iter().zip(&scrolls).enumerate() {
            let turned = index > 0 && recording.samples[index - 1].y.signum() != sample.y.signum();
            let paused = index > 0
                && sample.at - recording.samples[index - 1].at > Duration::from_millis(350);
            if index == 0 || turned || paused {
                assert_eq!(scroll.gain, 1.0, "sample {index}");
                assert_eq!(scroll.displacement.1.abs(), step, "sample {index}");
            }
        }
    }

    #[test]
    fn gain_follows_how_fast_notches_come_not_how_big_one_is() {
        let profile = WheelProfile::accelerated();
        // One event carrying two notches, well apart from its neighbours:
        // twice the distance, no acceleration.
        let mut slow = WheelInterpreter::new(profile);
        for ms in [0, 300, 600] {
            scroll(&mut slow, &notch(ms, -120.0));
        }
        let double = scroll(&mut slow, &notch(900, -240.0));
        assert_eq!(double.gain, 1.0);
        assert_eq!(double.displacement.1, 120.0);

        // Single notches 20 ms apart: full acceleration.
        let mut quick = WheelInterpreter::new(profile);
        let mut last = 1.0;
        for step in 0..12 {
            last = scroll(&mut quick, &notch(step * 20, -120.0)).gain;
        }
        assert!((last - 3.5).abs() < 1e-9, "gain {last}");
    }

    #[test]
    fn a_double_notch_is_one_reading_of_the_rate_not_two() {
        // If a 240 counted as two notches half a gap apart, this roll at a
        // steady 100 ms would read as 50 ms and be accelerated harder than
        // the same roll in single notches.
        let profile = WheelProfile::accelerated();
        let mut singles = WheelInterpreter::new(profile);
        let mut doubles = WheelInterpreter::new(profile);
        let (mut single, mut double) = (1.0, 1.0);
        for step in 0..10 {
            single = scroll(&mut singles, &notch(step * 100, -120.0)).gain;
            double = scroll(&mut doubles, &notch(step * 100, -240.0)).gain;
        }
        assert_eq!(single, double);
    }

    #[test]
    fn samples_that_waited_in_the_queue_do_not_count_as_a_fast_roll() {
        let profile = WheelProfile::accelerated();
        let mut interpreter = WheelInterpreter::new(profile);
        for ms in [0, 200, 400, 600] {
            scroll(&mut interpreter, &notch(ms, -120.0));
        }
        // Three notches released together behind a slow frame.
        let mut gain = 1.0;
        for ms in [800, 800, 801] {
            let mut queued = notch(ms, -120.0);
            queued.after_busy = true;
            gain = scroll(&mut interpreter, &queued).gain;
        }
        assert_eq!(gain, 1.0, "their spacing was the queue's, not the wheel's");
    }

    #[test]
    fn one_fraction_makes_it_continuous_and_it_takes_three_notches_to_go_back() {
        let mut interpreter = WheelInterpreter::new(WheelProfile::natural());
        assert_eq!(
            scroll(&mut interpreter, &notch(0, -120.0)).class,
            InputClass::Notched
        );
        assert_eq!(
            scroll(&mut interpreter, &notch(20, -37.0)).class,
            InputClass::Fine
        );
        // A touchpad stroke that happens to land on 120 twice is still a
        // touchpad stroke.
        assert_eq!(
            scroll(&mut interpreter, &notch(40, -120.0)).class,
            InputClass::Fine
        );
        assert_eq!(
            scroll(&mut interpreter, &notch(60, -120.0)).class,
            InputClass::Fine
        );
        assert_eq!(
            scroll(&mut interpreter, &notch(80, -120.0)).class,
            InputClass::Notched
        );

        // After a second of nothing it is whatever arrives next.
        scroll(&mut interpreter, &notch(100, -11.0));
        let fresh = scroll(&mut interpreter, &notch(1_200, -120.0));
        assert_eq!((fresh.class, fresh.settled), (InputClass::Notched, false));
    }

    #[test]
    fn pixels_phases_and_momentum_are_a_touchpad_that_says_so() {
        let mut interpreter = WheelInterpreter::new(WheelProfile::rapid_navigation());
        let pixels =
            WheelSample::from_pixels(1.5, -4.25, GesturePhase::Moved, Duration::from_millis(5));
        let moved = scroll(&mut interpreter, &pixels);
        assert_eq!(moved.class, InputClass::Precise);
        assert_eq!(moved.displacement, (-1.5, 4.25), "as it came, sign turned");

        let mut coasting = pixels;
        coasting.momentum = Some(true);
        coasting.at = Duration::from_millis(20);
        assert_eq!(scroll(&mut interpreter, &coasting).momentum, Some(true));

        let started = WheelSample::from_lines(
            0.0,
            -1.0,
            false,
            GesturePhase::Started,
            Duration::from_secs(5),
        );
        let began = scroll(&mut interpreter, &started);
        assert_eq!(began.class, InputClass::Precise);
        assert!(began.gesture_began);
    }

    #[test]
    fn a_device_mode_overrides_the_reading() {
        let mut profile = WheelProfile::natural();
        profile.device_mode = DeviceMode::SmoothWheel;
        let mut interpreter = WheelInterpreter::new(profile);
        let forced = scroll(&mut interpreter, &notch(0, -120.0));
        assert_eq!((forced.class, forced.settled), (InputClass::Fine, true));
        profile.device_mode = DeviceMode::Wheel;
        interpreter.set_profile(profile);
        assert_eq!(
            scroll(&mut interpreter, &notch(20, -7.0)).class,
            InputClass::Notched
        );
    }

    #[test]
    fn the_caller_chooses_the_unit() {
        // An editor that scrolls in pages: a line is a fortieth of a page
        // and a physical pixel an eight-hundredth.
        let pages = Units {
            line: (0.0, 1.0 / 40.0),
            page: (1.0, 1.0),
            pixel: 1.0 / 800.0,
            settings: ScrollSettings::default(),
        };
        let mut interpreter = WheelInterpreter::new(WheelProfile::precision());
        let Interpreted::Scroll(by_notch) = interpreter.interpret(&notch(0, -120.0), &pages) else {
            panic!("a scroll");
        };
        assert!((by_notch.displacement.1 - 3.0 / 40.0).abs() < 1e-12);
        let touch =
            WheelSample::from_pixels(0.0, -80.0, GesturePhase::Moved, Duration::from_millis(9));
        let Interpreted::Scroll(by_pixel) = interpreter.interpret(&touch, &pages) else {
            panic!("a scroll");
        };
        assert!((by_pixel.displacement.1 - 0.1).abs() < 1e-12);
    }

    #[test]
    fn control_makes_it_zoom_and_hands_the_amount_back_whole() {
        let mut interpreter = WheelInterpreter::new(WheelProfile::natural());
        let control = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        let quarter = notch(0, 30.0).with_modifiers(control);
        assert_eq!(
            interpreter.interpret(&quarter, &units()),
            Interpreted::Zoom(Zoom {
                notches: Some(0.25),
                pixels: None
            })
        );
        let pinch = WheelSample::from_pixels(0.0, -3.5, GesturePhase::Moved, Duration::ZERO)
            .with_modifiers(control);
        assert_eq!(
            interpreter.interpret(&pinch, &units()),
            Interpreted::Zoom(Zoom {
                notches: None,
                pixels: Some(-3.5)
            })
        );
    }

    #[test]
    fn shift_turns_the_wheel_sideways_and_alt_makes_it_fast() {
        let mut interpreter = WheelInterpreter::new(WheelProfile::natural());
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let sideways = scroll(&mut interpreter, &notch(0, -120.0).with_modifiers(shift));
        if cfg!(target_os = "macos") {
            assert_eq!(sideways.displacement, (0.0, 60.0));
        } else {
            // Three characters of 8 px, to the right.
            assert_eq!(sideways.displacement, (24.0, 0.0));
        }
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        let fast = scroll(&mut interpreter, &notch(2_000, -120.0).with_modifiers(alt));
        assert_eq!(fast.displacement, (0.0, 240.0));
    }

    #[test]
    fn a_page_a_notch_is_never_accelerated_and_off_means_off() {
        let mut interpreter = WheelInterpreter::new(WheelProfile::rapid_navigation());
        let mut by_page = units();
        by_page.settings.wheel_lines = WheelLines::Page;
        for step in 0..10 {
            let Interpreted::Scroll(paged) =
                interpreter.interpret(&notch(step * 20, -120.0), &by_page)
            else {
                panic!("a scroll");
            };
            assert_eq!(paged.displacement.1, 600.0);
        }
        let mut off = units();
        off.settings.wheel_lines = WheelLines::Off;
        let Interpreted::Scroll(nothing) = interpreter.interpret(&notch(5_000, -120.0), &off)
        else {
            panic!("a scroll");
        };
        assert_eq!(nothing.displacement.1, 0.0);
    }

    #[test]
    fn moving_the_view_some_other_way_ends_the_roll() {
        let mut interpreter = WheelInterpreter::new(WheelProfile::accelerated());
        let mut gain = 1.0;
        for step in 0..8 {
            gain = scroll(&mut interpreter, &notch(step * 20, -120.0)).gain;
        }
        assert!(gain > 3.0);
        interpreter.interrupt();
        assert_eq!(scroll(&mut interpreter, &notch(180, -120.0)).gain, 1.0);
    }

    #[test]
    fn thousands_of_tiny_movements_add_up_without_drift() {
        let mut interpreter = WheelInterpreter::new(WheelProfile::natural());
        let mut total = 0.0;
        for step in 0..4_800_u64 {
            total += scroll(&mut interpreter, &notch(step * 8, -1.0))
                .displacement
                .1;
        }
        // 4800 hundred-and-twentieths are forty notches of 60 px.
        assert!((total - 2_400.0).abs() < 1e-6, "total {total}");
    }

    #[test]
    fn time_running_backwards_is_survived() {
        let mut interpreter = WheelInterpreter::new(WheelProfile::natural());
        scroll(&mut interpreter, &notch(500, -120.0));
        let earlier = scroll(&mut interpreter, &notch(100, -120.0));
        assert!(earlier.displacement.1.is_finite());
        assert_eq!(earlier.gain, 1.0);
    }

    #[test]
    fn the_presets_are_valid_and_nonsense_is_refused() {
        for profile in [
            WheelProfile::precision(),
            WheelProfile::natural(),
            WheelProfile::accelerated(),
            WheelProfile::rapid_navigation(),
        ] {
            assert!(profile.validate().is_ok());
        }
        let mut bad = WheelProfile::natural();
        bad.notch_sensitivity = 0.0;
        assert!(bad.validate().is_err());
        let mut inverted = WheelProfile::natural();
        if let Some(cadence) = inverted.cadence.as_mut() {
            cadence.fast = cadence.slow;
        }
        assert!(inverted.validate().is_err());
        let mut runaway = WheelProfile::natural();
        if let Some(cadence) = runaway.cadence.as_mut() {
            cadence.max_gain = f64::INFINITY;
        }
        assert!(runaway.validate().is_err());
    }
}
