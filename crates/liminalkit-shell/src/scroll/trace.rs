//! Wheel samples written down as text, and read back.
//!
//! What a device sends is the one thing about scrolling that cannot be
//! reasoned out: whether a free-spinning wheel reports whole notches or
//! fractions of one, how fast, and whether a touchpad's stream carries on
//! after the fingers lift. A trace records it once, on the real device, and
//! the recording then drives tests on a machine with no such device attached.
//!
//! The format is one sample per line and meant to be read by a person:
//!
//! ```text
//! liminalkit-wheel-trace 1
//! # anything after a hash is a comment
//! meta device=notched wheel
//! @16230 d 0 -120 phase=moved mom=? src=? mods=---- stamp=d busy=0
//! @24871 p -0.5 -12.25 phase=moved mom=0 src=touchpad mods=s--- stamp=h busy=0
//! ```
//!
//! `@` is microseconds from the start of the recording. The unit letter is
//! `d` for 120ths of a notch, `l` for lines and `p` for pixels. Numbers are
//! written in the shortest form that reads back to the same value, so a trace
//! round-trips exactly.

use super::event::{GesturePhase, SourceHint, Stamp, WheelSample, WheelUnit};
use crate::input::Modifiers;
use std::fmt::Write as _;
use std::time::Duration;

const HEADER: &str = "liminalkit-wheel-trace 1";

/// A line of a trace that could not be read, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceError {
    /// One-based line number.
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for TraceError {}

/// A recording of wheel samples, with free-form notes about where it came
/// from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WheelTrace {
    /// `key=value` notes: the device, the platform, the driver.
    pub meta: Vec<(String, String)>,
    pub samples: Vec<WheelSample>,
}

impl WheelTrace {
    pub fn new() -> WheelTrace {
        WheelTrace::default()
    }

    pub fn note(&mut self, key: &str, value: &str) {
        self.meta.push((key.to_owned(), value.to_owned()));
    }

    pub fn record(&mut self, sample: WheelSample) {
        self.samples.push(sample);
    }

    /// The recording cut into frames, as a window redrawing every `interval`
    /// would have seen it.
    ///
    /// Each entry is a frame's time and the samples that arrived since the
    /// frame before, which is how they reach anything that scrolls: in
    /// batches, a frame at a time. Frames in which nothing arrived are
    /// included, because a scroll that is still moving has to be drawn in
    /// them too, and `tail` adds frames after the last sample for it to come
    /// to rest in. Replaying one recording at several intervals is how a
    /// test shows that the result does not depend on the frame rate.
    pub fn frames(&self, interval: Duration, tail: Duration) -> Vec<(Duration, &[WheelSample])> {
        let (Some(first), Some(last)) = (self.samples.first(), self.samples.last()) else {
            return Vec::new();
        };
        let interval = interval.max(Duration::from_micros(1));
        let end = last.at + tail;
        let mut frames = Vec::new();
        let mut time = first.at;
        let mut from = 0;
        loop {
            let to = from
                + self.samples[from..]
                    .iter()
                    .take_while(|sample| sample.at <= time)
                    .count();
            frames.push((time, &self.samples[from..to]));
            from = to;
            if time >= end {
                return frames;
            }
            time += interval;
        }
    }

    pub fn to_text(&self) -> String {
        let mut text = String::from(HEADER);
        text.push('\n');
        for (key, value) in &self.meta {
            let _ = writeln!(text, "meta {key}={value}");
        }
        for sample in &self.samples {
            let unit = match sample.unit {
                WheelUnit::Windows120 => 'd',
                WheelUnit::Lines => 'l',
                WheelUnit::Pixels => 'p',
            };
            let phase = match sample.phase {
                GesturePhase::Started => "started",
                GesturePhase::Moved => "moved",
                GesturePhase::Ended => "ended",
                GesturePhase::Cancelled => "cancelled",
            };
            let momentum = match sample.momentum {
                None => "?",
                Some(false) => "0",
                Some(true) => "1",
            };
            let source = match sample.source {
                SourceHint::Unknown => "?",
                SourceHint::Wheel => "wheel",
                SourceHint::Touchpad => "touchpad",
            };
            let m = sample.modifiers;
            let flag = |on: bool, letter: char| if on { letter } else { '-' };
            let _ = writeln!(
                text,
                "@{} {} {} {} phase={} mom={} src={} mods={}{}{}{} stamp={} busy={}",
                sample.at.as_micros(),
                unit,
                sample.x,
                sample.y,
                phase,
                momentum,
                source,
                flag(m.shift, 's'),
                flag(m.control, 'c'),
                flag(m.alt, 'a'),
                flag(m.logo, 'l'),
                match sample.stamp {
                    Stamp::Delivered => 'd',
                    Stamp::Hardware => 'h',
                },
                u8::from(sample.after_busy),
            );
        }
        text
    }

    pub fn from_text(text: &str) -> Result<WheelTrace, TraceError> {
        let mut lines = text.lines().enumerate().map(|(index, line)| {
            let line = line.split('#').next().unwrap_or("").trim();
            (index + 1, line)
        });
        let fail = |line: usize, message: &str| TraceError {
            line,
            message: message.to_owned(),
        };
        match lines.by_ref().find(|(_, line)| !line.is_empty()) {
            Some((_, line)) if line == HEADER => {}
            Some((number, _)) => return Err(fail(number, "not a version 1 wheel trace")),
            None => return Err(fail(1, "empty trace")),
        }
        let mut trace = WheelTrace::new();
        for (number, line) in lines {
            if line.is_empty() {
                continue;
            }
            if let Some(note) = line.strip_prefix("meta ") {
                let (key, value) = note
                    .split_once('=')
                    .ok_or_else(|| fail(number, "meta needs key=value"))?;
                trace.note(key.trim(), value.trim());
                continue;
            }
            trace
                .samples
                .push(parse_sample(line).map_err(|message| fail(number, &message))?);
        }
        Ok(trace)
    }
}

fn parse_sample(line: &str) -> Result<WheelSample, String> {
    let mut words = line.split_whitespace();
    let mut next = |what: &str| words.next().ok_or(format!("missing {what}"));
    let at = next("time")?
        .strip_prefix('@')
        .ok_or("a sample starts with @microseconds")?
        .parse::<u64>()
        .map_err(|_| "time is not a whole number of microseconds")?;
    let unit = match next("unit")? {
        "d" => WheelUnit::Windows120,
        "l" => WheelUnit::Lines,
        "p" => WheelUnit::Pixels,
        other => return Err(format!("unknown unit {other}")),
    };
    let number = |word: &str| {
        word.parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or(format!("{word} is not a finite number"))
    };
    let x = number(next("x")?)?;
    let y = number(next("y")?)?;
    let mut sample = WheelSample {
        x,
        y,
        unit,
        phase: GesturePhase::Moved,
        momentum: None,
        source: SourceHint::Unknown,
        modifiers: Modifiers::default(),
        at: Duration::from_micros(at),
        stamp: Stamp::Delivered,
        after_busy: false,
    };
    for word in words {
        let (key, value) = word
            .split_once('=')
            .ok_or(format!("{word} is not key=value"))?;
        match key {
            "phase" => {
                sample.phase = match value {
                    "started" => GesturePhase::Started,
                    "moved" => GesturePhase::Moved,
                    "ended" => GesturePhase::Ended,
                    "cancelled" => GesturePhase::Cancelled,
                    other => return Err(format!("unknown phase {other}")),
                }
            }
            "mom" => {
                sample.momentum = match value {
                    "?" => None,
                    "0" => Some(false),
                    "1" => Some(true),
                    other => return Err(format!("unknown momentum {other}")),
                }
            }
            "src" => {
                sample.source = match value {
                    "?" => SourceHint::Unknown,
                    "wheel" => SourceHint::Wheel,
                    "touchpad" => SourceHint::Touchpad,
                    other => return Err(format!("unknown source {other}")),
                }
            }
            "mods" => {
                sample.modifiers = Modifiers {
                    shift: value.contains('s'),
                    control: value.contains('c'),
                    alt: value.contains('a'),
                    logo: value.contains('l'),
                }
            }
            "stamp" => {
                sample.stamp = match value {
                    "d" => Stamp::Delivered,
                    "h" => Stamp::Hardware,
                    other => return Err(format!("unknown stamp {other}")),
                }
            }
            "busy" => sample.after_busy = value == "1",
            other => return Err(format!("unknown field {other}")),
        }
    }
    Ok(sample)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trace_reads_back_exactly_as_it_was_recorded() {
        let mut trace = WheelTrace::new();
        trace.note("device", "free-spin wheel");
        trace.record(WheelSample::from_lines(
            0.0,
            -0.25,
            true,
            GesturePhase::Moved,
            Duration::from_micros(16_230),
        ));
        let mut touch = WheelSample::from_pixels(
            -0.1,
            -12.333_333_333_333_334,
            GesturePhase::Ended,
            Duration::from_micros(24_871),
        )
        .with_modifiers(Modifiers {
            shift: true,
            logo: true,
            ..Modifiers::default()
        });
        touch.momentum = Some(true);
        touch.source = SourceHint::Touchpad;
        touch.stamp = Stamp::Hardware;
        touch.after_busy = true;
        trace.record(touch);
        trace.record(WheelSample::from_lines(
            1.5,
            0.0,
            false,
            GesturePhase::Started,
            Duration::from_micros(30_000),
        ));

        let text = trace.to_text();
        assert_eq!(WheelTrace::from_text(&text), Ok(trace));
    }

    #[test]
    fn cutting_a_recording_into_frames_loses_and_repeats_nothing() {
        let mut trace = WheelTrace::new();
        for ms in [0_u64, 3, 9, 16, 17, 40, 41, 95] {
            trace.record(WheelSample::from_lines(
                0.0,
                -1.0,
                true,
                GesturePhase::Moved,
                Duration::from_millis(ms),
            ));
        }
        for interval_us in [1_000_u64, 6_944, 16_667, 33_333, 250_000] {
            let frames = trace.frames(
                Duration::from_micros(interval_us),
                Duration::from_millis(100),
            );
            let seen: Vec<WheelSample> = frames
                .iter()
                .flat_map(|(_, samples)| samples.iter().copied())
                .collect();
            assert_eq!(seen, trace.samples, "at {interval_us} us a frame");
            for (time, samples) in &frames {
                assert!(samples.iter().all(|sample| sample.at <= *time));
            }
            let (last, _) = frames.last().expect("at least one frame");
            assert!(
                *last >= Duration::from_millis(195),
                "frames run on past the last sample, for motion to settle in"
            );
        }
        assert!(WheelTrace::new()
            .frames(Duration::from_millis(16), Duration::ZERO)
            .is_empty());
    }

    #[test]
    fn frames_with_nothing_in_them_are_still_frames() {
        let mut trace = WheelTrace::new();
        for ms in [0_u64, 100] {
            trace.record(WheelSample::from_pixels(
                0.0,
                -1.0,
                GesturePhase::Moved,
                Duration::from_millis(ms),
            ));
        }
        let frames = trace.frames(Duration::from_millis(10), Duration::ZERO);
        assert_eq!(frames.len(), 11);
        assert_eq!(frames.iter().filter(|(_, s)| s.is_empty()).count(), 9);
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let text = "\n# recorded by hand\nliminalkit-wheel-trace 1\n\n\
                    meta device = notched wheel # a comment\n\
                    @100 d 0 -120 phase=moved   # one notch down\n";
        let trace = WheelTrace::from_text(text).expect("readable");
        assert_eq!(
            trace.meta,
            vec![("device".to_owned(), "notched wheel".to_owned())]
        );
        assert_eq!(trace.samples.len(), 1);
        assert_eq!(trace.samples[0].y, -120.0);
        assert_eq!(trace.samples[0].at, Duration::from_micros(100));
    }

    #[test]
    fn a_bad_line_is_reported_by_number_rather_than_skipped() {
        // A trace that silently lost a sample would make a test pass on the
        // wrong input.
        let wrong_version = WheelTrace::from_text("liminalkit-wheel-trace 2\n");
        assert_eq!(wrong_version.unwrap_err().line, 1);

        let bad = "liminalkit-wheel-trace 1\n@100 d 0 -120\n@200 q 0 1\n";
        let error = WheelTrace::from_text(bad).unwrap_err();
        assert_eq!(error.line, 3);
        assert!(error.message.contains("unknown unit"));

        let not_finite = "liminalkit-wheel-trace 1\n@100 p 0 NaN\n";
        assert_eq!(WheelTrace::from_text(not_finite).unwrap_err().line, 2);
    }
}
