//! Easing curves: how a value moves between two ends over a fixed time.
//!
//! EXPERIMENTAL: the set of curves is new and may grow.

use std::f64::consts::{PI, TAU};

/// The shape of a timed move: maps progress through the time, 0 to 1, to
/// progress through the distance.
///
/// Every curve starts exactly at 0 and ends exactly at 1, whatever floating
/// point would make of its formula, so an animation driven by one lands on
/// its target and not a rounding error away. In between, [`BackOut`] and
/// [`ElasticOut`] go past 1 (and a [`Bezier`] may, if its control points
/// do); the rest stay within 0 to 1 and never go backwards.
///
/// [`BackOut`]: Ease::BackOut
/// [`ElasticOut`]: Ease::ElasticOut
/// [`Bezier`]: Ease::Bezier
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ease {
    /// Constant speed.
    Linear,
    /// Starts slow: `t²`.
    QuadIn,
    /// Ends slow: `1 - (1 - t)²`.
    QuadOut,
    /// Slow at both ends, quadratic.
    QuadInOut,
    /// Starts slow: `t³`.
    CubicIn,
    /// Ends slow: `1 - (1 - t)³`. The same curve the scroll motion
    /// controller uses for a programmed move, so the two feel alike.
    CubicOut,
    /// Slow at both ends, cubic.
    CubicInOut,
    /// Ends slower still: `1 - (1 - t)⁴`.
    QuartOut,
    /// Exponential ease out: almost all of the distance early, then a long
    /// approach. Scaled so that it reaches 1 at the end instead of stopping
    /// a thousandth short.
    ExpoOut,
    /// Slow at both ends, along a half cosine.
    SineInOut,
    /// Overshoots and comes back. `overshoot` sets how far: 1.70158 (see
    /// [`Ease::BACK`]) is the usual amount, about a tenth past the target;
    /// zero gives a plain cubic ease out.
    BackOut {
        /// How far past the end it swings, as a tension; not a distance.
        overshoot: f32,
    },
    /// Rings around the end like a plucked spring, decaying quickly.
    /// `period` is the length of one oscillation as a share of the whole
    /// move; 0.3 (see [`Ease::ELASTIC`]) is the usual. A period that is not
    /// positive and finite is taken as 0.3.
    ElasticOut {
        /// One oscillation, as a share of the move's duration.
        period: f32,
    },
    /// Jumps in this many equal steps, holding each (CSS `steps(n,
    /// jump-end)`): it reaches 1 only at the very end. Zero steps is taken
    /// as one.
    Steps(u16),
    /// A CSS `cubic-bezier(x1, y1, x2, y2)`. The `x` control values are
    /// clamped into 0 to 1, as CSS requires, so that time never runs
    /// backwards; the `y` values may go outside it for an overshoot. A
    /// control value that is not finite makes the curve linear.
    Bezier([f32; 4]),
}

impl Ease {
    /// [`BackOut`](Ease::BackOut) with the conventional overshoot.
    pub const BACK: Ease = Ease::BackOut { overshoot: 1.70158 };
    /// [`ElasticOut`](Ease::ElasticOut) with the conventional period.
    pub const ELASTIC: Ease = Ease::ElasticOut { period: 0.3 };
    /// CSS `ease`: `cubic-bezier(0.25, 0.1, 0.25, 1)`.
    pub const CSS_EASE: Ease = Ease::Bezier([0.25, 0.1, 0.25, 1.0]);

    /// Progress through the distance at progress `t` through the time.
    /// `t` is clamped into 0 to 1 (NaN counts as 0); the result is exactly 0
    /// at 0 and exactly 1 at 1.
    pub fn apply(self, t: f32) -> f32 {
        if t.is_nan() || t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        // Worked in f64 so the curves with powers and transcendentals agree
        // closely across platforms; the result is only ever shown as f32.
        let t = f64::from(t);
        let eased = match self {
            Ease::Linear => t,
            Ease::QuadIn => t * t,
            Ease::QuadOut => 1.0 - (1.0 - t) * (1.0 - t),
            Ease::QuadInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (2.0 - 2.0 * t).powi(2) / 2.0
                }
            }
            Ease::CubicIn => t * t * t,
            Ease::CubicOut => 1.0 - (1.0 - t).powi(3),
            Ease::CubicInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (2.0 - 2.0 * t).powi(3) / 2.0
                }
            }
            Ease::QuartOut => 1.0 - (1.0 - t).powi(4),
            Ease::ExpoOut => (1.0 - 2f64.powf(-10.0 * t)) / (1.0 - 2f64.powi(-10)),
            Ease::SineInOut => (1.0 - (PI * t).cos()) / 2.0,
            Ease::BackOut { overshoot } => {
                let c1 = f64::from(overshoot);
                let c3 = c1 + 1.0;
                let u = t - 1.0;
                1.0 + c3 * u * u * u + c1 * u * u
            }
            Ease::ElasticOut { period } => {
                let p = f64::from(period);
                let p = if p.is_finite() && p > 0.0 { p } else { 0.3 };
                2f64.powf(-10.0 * t) * ((t - p / 4.0) * TAU / p).sin() + 1.0
            }
            Ease::Steps(steps) => {
                let n = f64::from(steps.max(1));
                (t * n).floor() / n
            }
            Ease::Bezier([x1, y1, x2, y2]) => bezier(x1, y1, x2, y2, t),
        };
        eased as f32
    }
}

/// A CSS cubic Bézier from (0, 0) to (1, 1) with control points
/// (x1, y1) and (x2, y2), evaluated at time `t`.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f64) -> f64 {
    if ![x1, y1, x2, y2].iter().all(|v| v.is_finite()) {
        return t;
    }
    let x1 = f64::from(x1.clamp(0.0, 1.0));
    let x2 = f64::from(x2.clamp(0.0, 1.0));
    let (y1, y2) = (f64::from(y1), f64::from(y2));

    // One coordinate of the curve as a polynomial in its parameter s:
    // B(s) = ((a s + b) s + c) s.
    let coefficients = |p1: f64, p2: f64| {
        let c = 3.0 * p1;
        let b = 3.0 * (p2 - p1) - c;
        let a = 1.0 - c - b;
        (a, b, c)
    };
    let (ax, bx, cx) = coefficients(x1, x2);
    let (ay, by, cy) = coefficients(y1, y2);
    let x_at = |s: f64| ((ax * s + bx) * s + cx) * s;
    let slope_at = |s: f64| (3.0 * ax * s + 2.0 * bx) * s + cx;
    let y_at = |s: f64| ((ay * s + by) * s + cy) * s;

    const CLOSE: f64 = 1e-9;
    // Newton's method is fast where the curve has slope; bisection is the
    // fallback where it is flat (an x control at 0 or 1) and always ends,
    // because x rises monotonically with s once its controls are in 0..1.
    let mut s = t;
    for _ in 0..8 {
        let error = x_at(s) - t;
        if error.abs() < CLOSE {
            return y_at(s);
        }
        let slope = slope_at(s);
        if slope.abs() < 1e-6 {
            break;
        }
        s -= error / slope;
        if !(0.0..=1.0).contains(&s) {
            break;
        }
    }
    let (mut low, mut high) = (0.0, 1.0);
    s = t;
    for _ in 0..60 {
        let x = x_at(s);
        if (x - t).abs() < CLOSE {
            break;
        }
        if x < t {
            low = s;
        } else {
            high = s;
        }
        s = (low + high) / 2.0;
    }
    y_at(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Ease; 18] = [
        Ease::Linear,
        Ease::QuadIn,
        Ease::QuadOut,
        Ease::QuadInOut,
        Ease::CubicIn,
        Ease::CubicOut,
        Ease::CubicInOut,
        Ease::QuartOut,
        Ease::ExpoOut,
        Ease::SineInOut,
        Ease::BACK,
        Ease::BackOut { overshoot: 0.0 },
        Ease::ELASTIC,
        Ease::ElasticOut { period: -1.0 },
        Ease::Steps(4),
        Ease::Steps(0),
        Ease::CSS_EASE,
        Ease::Bezier([0.68, -0.6, 0.32, 1.6]),
    ];

    const STEADY: [Ease; 13] = [
        Ease::Linear,
        Ease::QuadIn,
        Ease::QuadOut,
        Ease::QuadInOut,
        Ease::CubicIn,
        Ease::CubicOut,
        Ease::CubicInOut,
        Ease::QuartOut,
        Ease::ExpoOut,
        Ease::SineInOut,
        Ease::Steps(5),
        Ease::CSS_EASE,
        Ease::Bezier([0.42, 0.0, 0.58, 1.0]),
    ];

    #[test]
    fn every_curve_starts_and_ends_exactly() {
        for ease in ALL {
            assert_eq!(ease.apply(0.0), 0.0, "{ease:?}");
            assert_eq!(ease.apply(1.0), 1.0, "{ease:?}");
            assert_eq!(ease.apply(-3.0), 0.0, "{ease:?}");
            assert_eq!(ease.apply(7.0), 1.0, "{ease:?}");
            assert_eq!(ease.apply(f32::NAN), 0.0, "{ease:?}");
        }
    }

    #[test]
    fn curves_without_overshoot_never_go_backwards_or_leave_the_range() {
        for ease in STEADY {
            let mut last = 0.0;
            for step in 0..=1000 {
                let value = ease.apply(step as f32 / 1000.0);
                assert!(value >= last, "{ease:?} at step {step}");
                assert!((0.0..=1.0).contains(&value), "{ease:?} at step {step}");
                last = value;
            }
        }
    }

    #[test]
    fn curves_are_close_to_continuous_at_their_ends() {
        // Nothing but steps jumps at the start or the finish.
        for ease in STEADY.into_iter().filter(|e| !matches!(e, Ease::Steps(_))) {
            assert!(ease.apply(1e-4) < 0.01, "{ease:?}");
            assert!(ease.apply(1.0 - 1e-4) > 0.99, "{ease:?}");
        }
        assert!(Ease::BACK.apply(1.0 - 1e-4) > 0.99);
        assert!((Ease::ELASTIC.apply(1.0 - 1e-4) - 1.0).abs() < 0.01);
    }

    #[test]
    fn cubic_out_is_one_minus_the_cube_of_what_is_left() {
        for t in [0.1_f32, 0.25, 0.5, 0.75, 0.9] {
            let expected = 1.0 - (1.0 - t).powi(3);
            assert!((Ease::CubicOut.apply(t) - expected).abs() < 1e-6);
        }
        // Half way through: seven eighths of the distance, as in the scroll
        // motion controller.
        assert_eq!(Ease::CubicOut.apply(0.5), 0.875);
    }

    #[test]
    fn the_polynomial_curves_follow_their_formulas() {
        assert_eq!(Ease::QuadIn.apply(0.5), 0.25);
        assert_eq!(Ease::QuadOut.apply(0.5), 0.75);
        assert_eq!(Ease::QuadInOut.apply(0.25), 0.125);
        assert_eq!(Ease::QuadInOut.apply(0.5), 0.5);
        assert_eq!(Ease::CubicIn.apply(0.5), 0.125);
        assert_eq!(Ease::CubicInOut.apply(0.5), 0.5);
        assert_eq!(Ease::QuartOut.apply(0.5), 0.9375);
        assert!((Ease::SineInOut.apply(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn back_and_elastic_overshoot_the_end() {
        let peak = (1..1000)
            .map(|step| Ease::BACK.apply(step as f32 / 1000.0))
            .fold(0.0_f32, f32::max);
        assert!(peak > 1.09 && peak < 1.11, "about a tenth past: {peak}");
        assert!((1..1000).any(|step| Ease::ELASTIC.apply(step as f32 / 1000.0) > 1.0));
        // With no tension, back out is a cubic ease out.
        let plain = Ease::BackOut { overshoot: 0.0 };
        assert!((plain.apply(0.3) - Ease::CubicOut.apply(0.3)).abs() < 1e-6);
    }

    #[test]
    fn steps_hold_each_level_and_reach_one_only_at_the_end() {
        let steps = Ease::Steps(4);
        assert_eq!(steps.apply(0.1), 0.0);
        assert_eq!(steps.apply(0.25), 0.25);
        assert_eq!(steps.apply(0.49), 0.25);
        assert_eq!(steps.apply(0.5), 0.5);
        assert_eq!(steps.apply(0.99), 0.75);
        assert_eq!(steps.apply(1.0), 1.0);
        assert_eq!(Ease::Steps(0).apply(0.99), 0.0, "zero steps is one");
    }

    #[test]
    fn the_css_ease_bezier_matches_the_browser_curve() {
        // Values a browser gives for cubic-bezier(0.25, 0.1, 0.25, 1).
        let ease = Ease::CSS_EASE;
        assert!((ease.apply(0.5) - 0.802_403_4).abs() < 1e-4);
        assert!((ease.apply(0.25) - 0.409_365_4).abs() < 1e-3);
        assert!(ease.apply(0.1) > 0.09 && ease.apply(0.1) < 0.11);
    }

    #[test]
    fn a_straight_bezier_is_the_identity() {
        for ease in [
            Ease::Bezier([0.0, 0.0, 1.0, 1.0]),
            Ease::Bezier([1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0]),
            Ease::Bezier([0.3, 0.3, 0.7, 0.7]),
        ] {
            for step in 0..=100 {
                let t = step as f32 / 100.0;
                assert!((ease.apply(t) - t).abs() < 1e-5, "{ease:?} at {t}");
            }
        }
        let broken = Ease::Bezier([f32::NAN, 0.0, 1.0, 1.0]);
        assert_eq!(broken.apply(0.3), 0.3, "an unusable curve is linear");
    }

    #[test]
    fn a_bezier_with_x_outside_the_unit_range_is_clamped_and_still_solves() {
        let ease = Ease::Bezier([-2.0, 0.0, 3.0, 1.0]);
        let mut last = 0.0;
        for step in 0..=100 {
            let value = ease.apply(step as f32 / 100.0);
            assert!(value.is_finite() && value >= last - 1e-6);
            last = value;
        }
    }
}
