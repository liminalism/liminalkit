//! Record what a wheel or touchpad actually sends.
//!
//! ```sh
//! cargo run -p liminalkit --example scroll_capture -- "notched wheel"
//! cargo run -p liminalkit --example scroll_capture -- "free-spin wheel" free-spin.trace
//! ```
//!
//! The first argument names the device, and goes into the recording; the
//! second is where to write it (default `scroll-capture.trace` in the current
//! directory). Scroll over the window, then press `S` to save, `C` to start
//! again, `Q` to save and quit.
//!
//! Nothing here interprets the input. Each sample is written exactly as the
//! host delivered it — its unit, its value, its phase, the modifiers held and
//! when it arrived — because the point of a recording is to find out what a
//! device does before deciding what to do about it. Things worth capturing
//! from each device, one recording apiece or with pauses between:
//!
//! - single slow notches or short strokes, a second apart;
//! - a steady roll, then the fastest roll the device allows;
//! - a flick followed by lifting the fingers or letting the wheel spin down,
//!   then leaving it alone for two seconds (does anything keep arriving?);
//! - a reversal mid-roll;
//! - a diagonal stroke on a touchpad, and a sideways one;
//! - the same with Shift, then Ctrl, held.
//!
//! EXPERIMENTAL: the recordings this makes are the evidence the scroll
//! module's defaults are still waiting for.

use std::path::PathBuf;
use std::time::Duration;

use liminalkit::raster::{Painter, RasterKernel, Rect, WindowBuffer};
use liminalkit::shell::scroll::{GesturePhase, WheelSample, WheelTrace, WheelUnit};
use liminalkit::shell::{Input, KeyInput, Scale};
use liminalkit::text::{Align, TextCache, TextStyle};
use liminalkit::ui::{Theme, Ui};
use liminalkit::windowing::{run_app, PixelApp, WindowConfig};
use liminalkit_test_fonts::{set, LATIN};

struct Capture {
    text: TextCache,
    kernel: RasterKernel,
    input: Input,
    device: String,
    out: PathBuf,
    trace: WheelTrace,
    scale: f32,
    status: String,
    quit: bool,
}

impl Capture {
    fn reset(&mut self) {
        self.trace = WheelTrace::new();
        self.trace.note("device", &self.device);
        self.trace.note("platform", std::env::consts::OS);
        self.trace.note("scale", &self.scale.to_string());
        self.trace.note("stamp", "host delivery time");
    }

    fn save(&mut self) {
        if self.trace.samples.is_empty() {
            self.status = "nothing recorded yet".to_owned();
            return;
        }
        self.status = match std::fs::write(&self.out, self.trace.to_text()) {
            Ok(()) => format!(
                "saved {} samples to {}",
                self.trace.samples.len(),
                self.out.display()
            ),
            Err(error) => format!("could not write {}: {error}", self.out.display()),
        };
    }
}

fn describe(sample: &WheelSample, gap: Option<Duration>) -> String {
    let unit = match sample.unit {
        WheelUnit::Windows120 => "/120",
        WheelUnit::Lines => "lines",
        WheelUnit::Pixels => "px",
    };
    let phase = match sample.phase {
        GesturePhase::Started => "started",
        GesturePhase::Moved => "moved",
        GesturePhase::Ended => "ended",
        GesturePhase::Cancelled => "cancelled",
    };
    let m = sample.modifiers;
    let mods = [
        (m.shift, "shift"),
        (m.control, "ctrl"),
        (m.alt, "alt"),
        (m.logo, "logo"),
    ]
    .iter()
    .filter(|(held, _)| *held)
    .map(|(_, name)| *name)
    .collect::<Vec<_>>()
    .join("+");
    let gap = gap.map_or("     —".to_owned(), |gap| {
        format!("{:>6.1}", gap.as_secs_f64() * 1_000.0)
    });
    let source = match sample.source {
        liminalkit::shell::scroll::SourceHint::Touchpad => "  [touchpad]",
        liminalkit::shell::scroll::SourceHint::Wheel => "  [wheel]",
        liminalkit::shell::scroll::SourceHint::Unknown => "",
    };
    format!(
        "{:>9.3}s  +{gap} ms   x {:>8}  y {:>8}  {unit:<5}  {phase:<9} {mods}{source}",
        sample.at.as_secs_f64(),
        sample.x,
        sample.y
    )
}

impl PixelApp for Capture {
    fn render(&mut self, buffer: &mut WindowBuffer, s: Scale) {
        if self.scale != s.0 {
            self.scale = s.0;
            if self.trace.samples.is_empty() {
                self.reset();
            }
        }
        let mut ui = Ui::new(
            Painter::new(buffer),
            &mut self.text,
            &mut self.input,
            Theme::default(),
            s,
            &mut self.kernel,
        );
        ui.painter.clear(0xf4f3ef);
        let ink = 0x182027;
        let muted = 0x68727b;
        let heading = TextStyle::new(LATIN, s.font(15.0));
        let body = TextStyle::new(LATIN, s.font(12.0));
        let w = ui.painter.width();
        let h = ui.painter.height();
        let margin = s.px(16);
        let row = s.px(18);
        let mut y = margin;
        let line = |ui: &mut Ui<'_>, y: &mut i32, text: &str, style: TextStyle, colour: u32| {
            ui.label_styled(
                Rect::new(margin, *y, w - margin * 2, row),
                text,
                style,
                colour,
                Align::Left,
            );
            *y += row;
        };
        line(
            &mut ui,
            &mut y,
            &format!(
                "Recording: {}   ({} samples)",
                self.device,
                self.trace.samples.len()
            ),
            heading,
            ink,
        );
        line(
            &mut ui,
            &mut y,
            "Scroll over this window.   S save   C clear   Q save and quit",
            body,
            muted,
        );
        line(&mut ui, &mut y, &self.status, body, muted);
        y += row / 2;

        // The most recent samples, newest last, as many as fit.
        let fit = ((h - y - margin) / row).max(1) as usize;
        let samples = &self.trace.samples;
        let from = samples.len().saturating_sub(fit);
        for index in from..samples.len() {
            let gap = index
                .checked_sub(1)
                .map(|before| samples[index].at.saturating_sub(samples[before].at));
            line(&mut ui, &mut y, &describe(&samples[index], gap), body, ink);
        }
        ui.input.end_frame();
    }

    fn on_wheel(&mut self, sample: &WheelSample) {
        self.trace.record(*sample);
    }

    fn on_scroll_settings(&mut self, settings: &liminalkit::shell::ScrollSettings) {
        // Only the native host reads these; note what it found.
        self.trace
            .note("wheel_lines", &format!("{:?}", settings.wheel_lines));
        self.trace
            .note("wheel_chars", &settings.wheel_chars.to_string());
        self.status = format!(
            "system wheel: {:?}, {} characters sideways",
            settings.wheel_lines, settings.wheel_chars
        );
    }

    fn on_key(&mut self, key: &KeyInput) {
        match key {
            KeyInput::Character('s' | 'S') => self.save(),
            KeyInput::Character('c' | 'C') => {
                self.reset();
                self.status = "cleared".to_owned();
            }
            KeyInput::Character('q' | 'Q') => {
                self.save();
                self.quit = true;
            }
            _ => {}
        }
    }

    fn should_exit(&self) -> bool {
        self.quit
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let device = args.next().unwrap_or_else(|| "unnamed device".to_owned());
    let out = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "scroll-capture.trace".to_owned()),
    );
    let mut capture = Capture {
        text: TextCache::new(set()),
        kernel: RasterKernel::new(),
        input: Input::new(),
        device,
        out,
        trace: WheelTrace::new(),
        scale: 1.0,
        status: "nothing recorded yet".to_owned(),
        quit: false,
    };
    capture.reset();
    let config =
        WindowConfig::new("liminalkit scroll capture", 900.0, 600.0).min_size(640.0, 320.0);
    run_app(capture, config).expect("run");
}
