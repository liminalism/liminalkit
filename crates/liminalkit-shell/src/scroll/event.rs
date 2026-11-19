//! One wheel movement, as the platform reported it.

use crate::input::Modifiers;
use std::time::{Duration, Instant};

/// How long after the process first asked `now` is.
///
/// Wheel samples and frame times are both measured from the same start, so
/// that a sample can be compared with the frame it is drawn in and both can
/// be written down. The start is whenever this is first called.
pub fn host_time(now: Instant) -> Duration {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    now.saturating_duration_since(*EPOCH.get_or_init(|| now))
}

/// Touch or gesture phase. The platform's phase is forwarded as it came;
/// nothing is invented here. A plain mouse wheel only ever reports
/// [`GesturePhase::Moved`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GesturePhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// The unit a wheel delta is in, kept with the delta rather than converted
/// away on arrival.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelUnit {
    /// 120ths of a wheel notch: the raw Windows `WHEEL_DELTA` scale. A
    /// notched wheel sends multiples of 120; a finer wheel or a touchpad on
    /// the legacy path sends smaller values. Only the Windows wheel path
    /// produces this, because only there is the original integer recoverable.
    Windows120,
    /// Lines, as some platforms report a wheel (X11, a macOS mouse). May be
    /// fractional, and on macOS has already been accelerated by the system.
    Lines,
    /// Physical pixels (a macOS or Wayland touchpad).
    Pixels,
}

/// What the host knows about the device behind a sample. A hint, never a
/// promise: most platforms cannot say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceHint {
    #[default]
    Unknown,
    Wheel,
    Touchpad,
}

/// Where a sample's timestamp came from.
///
/// The two are not interchangeable. A delivery time says when the application
/// got round to the event, which is later than the user moved the wheel by
/// however long the event sat in the queue. Anything that reads cadence from
/// the spacing of samples has to know which it is holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stamp {
    /// Taken when the host received the event.
    #[default]
    Delivered,
    /// Reported by the platform for the hardware event itself.
    Hardware,
}

/// One wheel or touchpad movement.
///
/// The sign is the platform's and the toolkit's long-standing one: positive
/// `y` is the wheel turning away from the user, which moves content down and
/// the view towards the top. A scroll offset therefore changes by the
/// negative of what a sample converts to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelSample {
    /// Horizontal delta, in `unit`.
    pub x: f64,
    /// Vertical delta, in `unit`.
    pub y: f64,
    pub unit: WheelUnit,
    pub phase: GesturePhase,
    /// Whether this movement is the system's own momentum after the fingers
    /// lifted. `None` when the platform does not say, which is most of them.
    /// Kept apart from [`SourceHint`]: what the device is and whether the
    /// system is coasting on its behalf are separate facts.
    pub momentum: Option<bool>,
    pub source: SourceHint,
    pub modifiers: Modifiers,
    /// When, measured from an epoch the producer chose and keeps to. Relative
    /// so that a sample can be written down and read back.
    pub at: Duration,
    pub stamp: Stamp,
    /// The host was busy just before this arrived, so it may have waited in
    /// the queue and its distance in time from its neighbours means little.
    pub after_busy: bool,
}

impl WheelSample {
    /// A sample from a platform line delta.
    ///
    /// `windows_wheel` says the delta came from the Windows wheel message,
    /// where the platform layer divided an integer by 120 and the integer is
    /// worth having back. Anywhere else a line is a line and stays one.
    pub fn from_lines(
        x: f32,
        y: f32,
        windows_wheel: bool,
        phase: GesturePhase,
        at: Duration,
    ) -> WheelSample {
        let (x, y, unit) = if windows_wheel {
            (
                (f64::from(x) * 120.0).round(),
                (f64::from(y) * 120.0).round(),
                WheelUnit::Windows120,
            )
        } else {
            (f64::from(x), f64::from(y), WheelUnit::Lines)
        };
        WheelSample {
            x,
            y,
            unit,
            phase,
            momentum: None,
            source: SourceHint::Unknown,
            modifiers: Modifiers::default(),
            at,
            stamp: Stamp::Delivered,
            after_busy: false,
        }
    }

    /// A sample from a platform pixel delta, in physical pixels.
    pub fn from_pixels(x: f64, y: f64, phase: GesturePhase, at: Duration) -> WheelSample {
        WheelSample {
            x,
            y,
            unit: WheelUnit::Pixels,
            phase,
            momentum: None,
            source: SourceHint::Unknown,
            modifiers: Modifiers::default(),
            at,
            stamp: Stamp::Delivered,
            after_busy: false,
        }
    }

    pub fn with_modifiers(mut self, modifiers: Modifiers) -> WheelSample {
        self.modifiers = modifiers;
        self
    }

    /// The delta as wheel notches, for the units that have notches. A pixel
    /// delta has none.
    pub fn notches(&self) -> Option<(f64, f64)> {
        match self.unit {
            WheelUnit::Windows120 => Some((self.x / 120.0, self.y / 120.0)),
            WheelUnit::Lines => Some((self.x, self.y)),
            WheelUnit::Pixels => None,
        }
    }

    /// The plain conversion to physical pixels: no sensitivity, no
    /// acceleration, nothing rounded.
    ///
    /// `line_px` is what one line is worth on each axis for the thing being
    /// scrolled (a table passes its row height); `page_px` is what a page is
    /// worth, used when the system is set to scroll a page per notch. The
    /// sign is the sample's own.
    pub fn pixels(
        &self,
        settings: &ScrollSettings,
        line_px: (f64, f64),
        page_px: (f64, f64),
    ) -> (f64, f64) {
        let Some((nx, ny)) = self.notches() else {
            return (self.x, self.y);
        };
        let y = match settings.wheel_lines {
            WheelLines::Lines(lines) => ny * lines * line_px.1,
            WheelLines::Page => ny * page_px.1,
            WheelLines::Off => 0.0,
        };
        (nx * settings.wheel_chars * line_px.0, y)
    }
}

/// How far the system says one wheel notch goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelLines {
    /// This many lines per notch.
    Lines(f64),
    /// One page per notch.
    Page,
    /// The wheel does not scroll.
    Off,
}

/// The system's wheel settings, with defaults for when nobody has read them.
///
/// The defaults are Windows' own: three lines per notch, three characters per
/// horizontal notch. A host that can read the real values replaces them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollSettings {
    pub wheel_lines: WheelLines,
    /// Characters per horizontal notch.
    pub wheel_chars: f64,
    /// What one line is worth, in logical pixels, for content that has no
    /// lines of its own. A third of a hundred, so that three lines make the
    /// hundred pixels a notch moves in most desktop software.
    pub line_logical_px: f64,
}

impl Default for ScrollSettings {
    fn default() -> ScrollSettings {
        ScrollSettings {
            wheel_lines: WheelLines::Lines(3.0),
            wheel_chars: 3.0,
            line_logical_px: 100.0 / 3.0,
        }
    }
}

impl ScrollSettings {
    /// One line in physical pixels at a display scale, for content with no
    /// lines of its own.
    pub fn default_line_px(&self, scale: f64) -> f64 {
        self.line_logical_px * scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: Duration = Duration::from_millis(5);

    #[test]
    fn a_windows_line_delta_gives_back_the_integer_it_was_made_from() {
        // The platform layer divides the raw value by 120 in f32. A quarter
        // notch from a fine wheel must come back as exactly 30.
        for raw in [-360_i32, -120, -30, -1, 1, 7, 30, 120, 240] {
            let lines = raw as f32 / 120.0;
            let sample = WheelSample::from_lines(0.0, lines, true, GesturePhase::Moved, AT);
            assert_eq!(sample.unit, WheelUnit::Windows120);
            assert_eq!(sample.y, f64::from(raw), "raw {raw}");
        }
    }

    #[test]
    fn a_line_delta_from_elsewhere_stays_a_line() {
        // Nothing says an X11 or macOS line is a number of 120ths, so nothing
        // pretends it is.
        let sample = WheelSample::from_lines(0.5, -1.25, false, GesturePhase::Moved, AT);
        assert_eq!(sample.unit, WheelUnit::Lines);
        assert_eq!((sample.x, sample.y), (0.5, -1.25));
        assert_eq!(sample.notches(), Some((0.5, -1.25)));
    }

    #[test]
    fn a_notch_is_three_lines_of_whatever_is_being_scrolled() {
        let settings = ScrollSettings::default();
        let notch = WheelSample::from_lines(0.0, -1.0, true, GesturePhase::Moved, AT);
        // A table with 20 px rows moves three rows.
        assert_eq!(
            notch.pixels(&settings, (8.0, 20.0), (600.0, 400.0)),
            (0.0, -60.0)
        );
        // A quarter notch moves a quarter of that, not nothing.
        let fine = WheelSample::from_lines(0.0, -0.25, true, GesturePhase::Moved, AT);
        assert_eq!(
            fine.pixels(&settings, (8.0, 20.0), (600.0, 400.0)),
            (0.0, -15.0)
        );
    }

    #[test]
    fn the_system_setting_decides_how_far_a_notch_goes() {
        let notch = WheelSample::from_lines(1.0, -1.0, true, GesturePhase::Moved, AT);
        let page = ScrollSettings {
            wheel_lines: WheelLines::Page,
            ..ScrollSettings::default()
        };
        assert_eq!(
            notch.pixels(&page, (8.0, 20.0), (600.0, 400.0)),
            (24.0, -400.0)
        );
        let off = ScrollSettings {
            wheel_lines: WheelLines::Off,
            ..ScrollSettings::default()
        };
        assert_eq!(notch.pixels(&off, (8.0, 20.0), (600.0, 400.0)).1, 0.0);
    }

    #[test]
    fn pixels_pass_through_untouched() {
        let sample = WheelSample::from_pixels(0.25, -0.3, GesturePhase::Moved, AT);
        assert_eq!(sample.notches(), None);
        assert_eq!(
            sample.pixels(&ScrollSettings::default(), (8.0, 20.0), (600.0, 400.0)),
            (0.25, -0.3)
        );
    }

    #[test]
    fn the_default_line_follows_the_display_scale() {
        // The old constant was 24 physical pixels at any scale, so a notch
        // went half as far, in the user's terms, on a 200% display.
        let settings = ScrollSettings::default();
        assert!((settings.default_line_px(1.0) * 3.0 - 100.0).abs() < 1e-9);
        assert!((settings.default_line_px(2.0) * 3.0 - 200.0).abs() < 1e-9);
    }
}
