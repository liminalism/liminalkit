//! Per-phase frame timing, printed as percentiles.
//!
//! Enabled by `LIMINALKIT_FRAME_LOG=1`. The shell records tick, paint and
//! present durations for every frame and prints p50/p95/max every couple of
//! seconds — the number that decides whether a software renderer is enough is
//! not the mean but the tail.

use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct FrameTimer {
    enabled: bool,
    tick: Vec<Duration>,
    paint: Vec<Duration>,
    present: Vec<Duration>,
    /// Input event → present, for frames that had an input timestamp.
    input_to_present: Vec<Duration>,
    window_start: Instant,
    /// Print interval.
    every: Duration,
    /// Optional budget; p95 above it is flagged.
    budget: Option<Duration>,
}

impl FrameTimer {
    /// Reads `LIMINALKIT_FRAME_LOG` (any value enables) and
    /// `LIMINALKIT_FRAME_BUDGET_MS` (a number of milliseconds).
    pub fn from_env() -> FrameTimer {
        let enabled = std::env::var_os("LIMINALKIT_FRAME_LOG").is_some();
        let budget = std::env::var("LIMINALKIT_FRAME_BUDGET_MS")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .map(Duration::from_secs_f64)
            .map(|d| d / 1000);
        FrameTimer::new(enabled, budget)
    }

    pub fn new(enabled: bool, budget: Option<Duration>) -> FrameTimer {
        FrameTimer {
            enabled,
            tick: Vec::new(),
            paint: Vec::new(),
            present: Vec::new(),
            input_to_present: Vec::new(),
            window_start: Instant::now(),
            every: Duration::from_secs(2),
            budget,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Record one frame. Samples are kept even when printing is off, so an
    /// application can gate on [`Self::snapshot`] without `LIMINALKIT_FRAME_LOG`.
    pub fn record(&mut self, tick: Duration, paint: Duration, present: Duration) {
        self.record_full(tick, paint, present, None);
    }

    /// [`Self::record`] plus the time from the input event that caused the
    /// frame to the present, when that input was timestamped.
    pub fn record_full(
        &mut self,
        tick: Duration,
        paint: Duration,
        present: Duration,
        input_to_present: Option<Duration>,
    ) {
        push_cap(&mut self.tick, tick);
        push_cap(&mut self.paint, paint);
        push_cap(&mut self.present, present);
        if let Some(latency) = input_to_present {
            push_cap(&mut self.input_to_present, latency);
        }
        if self.enabled && self.window_start.elapsed() >= self.every {
            self.flush();
        }
    }

    /// Percentiles for the samples currently held. Does not reset them.
    pub fn snapshot(&self) -> FrameSnapshot {
        FrameSnapshot {
            tick: summarize(&self.tick),
            paint: summarize(&self.paint),
            present: summarize(&self.present),
            input_to_present: summarize(&self.input_to_present),
        }
    }

    /// Print and reset. Also useful at exit.
    pub fn flush(&mut self) {
        if !self.enabled || self.paint.is_empty() {
            return;
        }
        let frames = self.paint.len();
        let secs = self.window_start.elapsed().as_secs_f64().max(1e-9);
        let line = |name: &str, samples: &mut Vec<Duration>| {
            samples.sort();
            let pick = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
            format!(
                "{name} p50 {:>6.2}ms p95 {:>6.2}ms max {:>6.2}ms",
                pick(0.5).as_secs_f64() * 1e3,
                pick(0.95).as_secs_f64() * 1e3,
                samples[samples.len() - 1].as_secs_f64() * 1e3
            )
        };
        let mut total: Vec<Duration> = self
            .tick
            .iter()
            .zip(&self.paint)
            .zip(&self.present)
            .map(|((t, p), q)| *t + *p + *q)
            .collect();
        let mut input = self.input_to_present.clone();
        let over = self.budget.map(|b| {
            let n = total.iter().filter(|d| **d > b).count();
            format!(" over-budget {n}/{frames}")
        });
        let input_line = if input.is_empty() {
            "input→present (none)".to_string()
        } else {
            line("input→present", &mut input)
        };
        eprintln!(
            "[frames] {frames} in {secs:.1}s ({:.1}/s) | {} | {} | {} | {} | {}{}",
            frames as f64 / secs,
            line("tick", &mut self.tick),
            line("paint", &mut self.paint),
            line("present", &mut self.present),
            line("total", &mut total),
            input_line,
            over.unwrap_or_default()
        );
        self.tick.clear();
        self.paint.clear();
        self.present.clear();
        self.input_to_present.clear();
        self.window_start = Instant::now();
    }
}

const SAMPLE_CAP: usize = 4096;

fn push_cap(samples: &mut Vec<Duration>, sample: Duration) {
    if samples.len() >= SAMPLE_CAP {
        samples.drain(..SAMPLE_CAP / 2);
    }
    samples.push(sample);
}

/// p50 / p95 / max for one phase. `None` when no samples have been recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhaseStats {
    pub count: usize,
    pub p50: Duration,
    pub p95: Duration,
    pub max: Duration,
}

/// The four phases a frame can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSnapshot {
    pub tick: Option<PhaseStats>,
    pub paint: Option<PhaseStats>,
    pub present: Option<PhaseStats>,
    pub input_to_present: Option<PhaseStats>,
}

fn summarize(samples: &[Duration]) -> Option<PhaseStats> {
    if samples.is_empty() {
        return None;
    }
    let mut ordered = samples.to_vec();
    ordered.sort();
    let pick = |q: f64| ordered[((ordered.len() - 1) as f64 * q).round() as usize];
    Some(PhaseStats {
        count: ordered.len(),
        p50: pick(0.5),
        p95: pick(0.95),
        max: ordered[ordered.len() - 1],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_to_present_is_queryable_without_the_log_env() {
        let mut timer = FrameTimer::new(false, None);
        for (tick_ms, input_ms) in [(1u64, 4u64), (2, 6), (3, 8)] {
            timer.record_full(
                Duration::from_millis(tick_ms),
                Duration::from_millis(2),
                Duration::from_millis(3),
                Some(Duration::from_millis(input_ms)),
            );
        }
        let shot = timer.snapshot();
        let tick = shot.tick.expect("tick");
        assert_eq!(tick.count, 3);
        assert_eq!(tick.max, Duration::from_millis(3));
        assert_eq!(tick.p50, Duration::from_millis(2));
        assert_ne!(tick.p50, Duration::ZERO, "tick is recorded, not dropped");
        let input = shot.input_to_present.expect("input");
        assert_eq!(input.max, Duration::from_millis(8));
        assert_eq!(input.p50, Duration::from_millis(6));
        assert_eq!(input.p95, Duration::from_millis(8));
    }
}
