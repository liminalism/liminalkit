//! A long document to scroll, with the scroll module's settings on the keys.
//!
//! ```sh
//! cargo run -p liminalkit --example scroll_tuning
//! LIMINALKIT_FRAME_LOG=1 cargo run -p liminalkit --example scroll_tuning
//! ```
//!
//! This is where the module's numbers are meant to be argued with. It wires a
//! [`WheelInterpreter`] to a [`MotionController`] the way an application
//! would, draws ten thousand rows through them, and shows what each wheel
//! movement was taken for: its class, the acceleration it earned, how far it
//! went, and how fast the view is moving.
//!
//! | Key | Does |
//! |---|---|
//! | `1` `2` `3` `4` | profile: precision, natural, accelerated, rapid navigation |
//! | `S` | notch smoothing time: off, 20, 40, 60, 90 ms |
//! | `F` | smoothing for continuous input: off, 8, 12, 20 ms |
//! | `L` | lead, the share of a notch shown at once: 0, 20%, 40% |
//! | `D` | device mode: auto, wheel, smooth wheel, touchpad |
//! | `M` | speed limit: none, 8 000, 20 000 px/s |
//! | Page Up / Down, Home, End | programmed moves |
//! | `R` | start or stop recording wheel input |
//! | `P` | replay the recording through the current settings |
//! | `W` | write the recording to `scroll-tuning.trace` |
//! | `Q` | quit |
//!
//! Record a stroke once, then press `P` after each change of setting: the
//! same input, replayed with its original timing, is the fair way to compare
//! two settings.
//!
//! EXPERIMENTAL: nothing here has been calibrated; that is what it is for.

use std::time::Duration;

use liminalkit::raster::{Painter, RasterKernel, Rect, WindowBuffer};
use liminalkit::shell::scroll::{
    DeviceMode, InputClass, Interpreted, MotionController, Smoothing, Units, WheelInterpreter,
    WheelProfile, WheelSample, WheelTrace,
};
use liminalkit::shell::{Input, KeyInput, Scale};
use liminalkit::text::{Align, TextCache, TextStyle};
use liminalkit::ui::{scrollbar_thumb, Theme, Ui};
use liminalkit::windowing::host::host_time;
use liminalkit::windowing::{run_app, FrameInfo, PixelApp, WindowConfig};
use liminalkit_test_fonts::{set, LATIN};

const ROWS: i64 = 10_000;
const ROW: i32 = 22;
const NOTCH_TIMES: [u64; 5] = [0, 20, 40, 60, 90];
const FINE_TIMES: [u64; 4] = [0, 8, 12, 20];
const LEADS: [f64; 3] = [0.0, 0.2, 0.4];
const LIMITS: [Option<f64>; 3] = [None, Some(8_000.0), Some(20_000.0)];
const PROFILES: [&str; 4] = ["precision", "natural", "accelerated", "rapid navigation"];
const MODES: [(DeviceMode, &str); 4] = [
    (DeviceMode::Auto, "auto"),
    (DeviceMode::Wheel, "wheel"),
    (DeviceMode::SmoothWheel, "smooth wheel"),
    (DeviceMode::Touchpad, "touchpad"),
];

fn smoothing(ms: u64) -> Smoothing {
    if ms == 0 {
        Smoothing::None
    } else {
        Smoothing::Exponential {
            time: Duration::from_millis(ms),
        }
    }
}

struct Tuning {
    text: TextCache,
    kernel: RasterKernel,
    input: Input,
    interpreter: WheelInterpreter,
    motion: MotionController,
    profile: usize,
    notch_time: usize,
    fine_time: usize,
    lead: usize,
    mode: usize,
    limit: usize,
    /// Host time of the frame being drawn, and one display frame.
    now: Duration,
    interval: Duration,
    /// What the list can show, from the last frame: a row, and the viewport.
    row_px: f64,
    view: (f64, f64),
    last: Option<String>,
    recording: bool,
    trace: WheelTrace,
    /// A replay in progress: the samples left, and the host time the
    /// recording's first sample was restarted at.
    replay: Option<(Vec<WheelSample>, Duration)>,
    status: String,
    quit: bool,
}

impl Tuning {
    fn apply_profile(&mut self) {
        let mut profile = match self.profile {
            0 => WheelProfile::precision(),
            1 => WheelProfile::natural(),
            2 => WheelProfile::accelerated(),
            _ => WheelProfile::rapid_navigation(),
        };
        profile.device_mode = MODES[self.mode].0;
        self.interpreter.set_profile(profile);
        self.motion.set_max_speed(LIMITS[self.limit]);
    }

    /// One wheel movement, through the interpreter and into the motion.
    fn feed(&mut self, sample: &WheelSample, at: Duration) {
        let units = Units::pixels((self.row_px, self.row_px), self.view);
        match self.interpreter.interpret(sample, &units) {
            Interpreted::Scroll(scroll) => {
                let (_, dy) = scroll.displacement;
                let accepted = match scroll.class {
                    InputClass::Notched => self.motion.wheel(
                        at,
                        dy,
                        smoothing(NOTCH_TIMES[self.notch_time]),
                        LEADS[self.lead],
                    ),
                    InputClass::Fine => {
                        self.motion
                            .wheel(at, dy, smoothing(FINE_TIMES[self.fine_time]), 0.0)
                    }
                    InputClass::Precise => self.motion.nudge(at, dy),
                };
                self.last = Some(format!(
                    "{:?}{}  gain {:.2}  asked {:+.1} px  took {:+.1} px{}",
                    scroll.class,
                    if scroll.settled { "" } else { " (unsettled)" },
                    scroll.gain,
                    dy,
                    accepted,
                    if sample.after_busy { "  queued" } else { "" },
                ));
            }
            Interpreted::Zoom(zoom) => {
                self.last = Some(format!(
                    "zoom  notches {:?}  pixels {:?}",
                    zoom.notches, zoom.pixels
                ));
            }
        }
    }

    fn page(&mut self, pages: f64) {
        self.interpreter.interrupt();
        let to = self.motion.target() + pages * (self.view.1 - self.row_px);
        self.motion
            .animate_to(self.now, to, Duration::from_millis(160));
    }
}

impl PixelApp for Tuning {
    fn on_frame(&mut self, frame: &FrameInfo) {
        self.now = host_time(frame.now);
        self.interval = frame.interval;
    }

    fn animation_interval(&self) -> Option<Duration> {
        (self.motion.animating() || self.replay.is_some()).then_some(self.interval)
    }

    fn on_wheel(&mut self, sample: &WheelSample) {
        if self.recording {
            self.trace.record(*sample);
        }
        if self.replay.is_none() {
            self.feed(sample, sample.at);
        }
    }

    fn render(&mut self, buffer: &mut WindowBuffer, s: Scale) {
        // A replay hands the recording back with its own spacing, shifted to
        // start when `P` was pressed.
        if let Some((mut samples, started)) = self.replay.take() {
            let first = samples.first().map_or(Duration::ZERO, |sample| sample.at);
            let due = samples
                .iter()
                .take_while(|sample| started + (sample.at - first) <= self.now)
                .count();
            for mut sample in samples.drain(..due) {
                let at = started + (sample.at - first);
                sample.at = at;
                self.feed(&sample, at);
            }
            if !samples.is_empty() {
                self.replay = Some((samples, started));
            }
        }

        let w = buffer.width as i32;
        let h = buffer.height as i32;
        let panel_w = s.px(330).min(w / 2);
        let list = Rect::new(0, 0, w - panel_w, h);
        let row = s.px(ROW).max(1);
        self.row_px = f64::from(row);
        self.view = (f64::from(list.w), f64::from(list.h));
        let content = i64::from(row) * ROWS;
        self.motion
            .set_bounds(0.0, (content - i64::from(list.h)).max(0) as f64);
        self.motion.advance(self.now);
        // The one place a pixel is chosen: the position is rounded here, for
        // drawing, and nowhere else.
        let offset = (self.motion.position() + 0.5).floor() as i64;

        self.input.set_scale(s);
        let mut ui = Ui::new(
            Painter::new(buffer),
            &mut self.text,
            &mut self.input,
            Theme::default(),
            s,
            &mut self.kernel,
        );
        ui.painter.clear(0xfbfaf7);
        let ink = 0x182027;
        let muted = 0x68727b;
        let body = TextStyle::new(LATIN, s.font(12.0));
        let heading = TextStyle::new(LATIN, s.font(14.0));

        ui.painter.push_clip(list);
        let first = offset / i64::from(row);
        let last = ((offset + i64::from(list.h)) / i64::from(row) + 1).min(ROWS);
        for index in first..last {
            let top = (index * i64::from(row) - offset) as i32;
            let line = Rect::new(list.x, top, list.w, row);
            // A band every forty rows, so that speed is visible as banding
            // going past and a page is something the eye can count.
            let chapter = index % 40 == 0;
            if chapter {
                ui.painter.fill_rect(line, 0xdfe6ea);
            } else if index % 2 == 0 {
                ui.painter.fill_rect(line, 0xf3f2ee);
            }
            let label = if chapter {
                format!("— section {} —", index / 40 + 1)
            } else {
                format!("row {index}")
            };
            ui.label_styled(
                Rect::new(list.x + s.px(12), top, list.w - s.px(24), row),
                &label,
                body,
                if chapter { ink } else { muted },
                Align::Left,
            );
        }
        ui.painter.pop_clip();
        if let Some((top, length)) = scrollbar_thumb(
            list.h,
            list.h,
            content.min(i64::from(i32::MAX)) as i32,
            offset.min(i64::from(i32::MAX)) as i32,
            s.px(24),
        ) {
            ui.painter.fill_rect(
                Rect::new(list.right() - s.px(6), top, s.px(4), length),
                0x9aa4ab,
            );
        }

        let panel = Rect::new(list.right(), 0, panel_w, h);
        ui.painter.fill_rect(panel, 0xffffff);
        ui.painter
            .fill_rect(Rect::new(panel.x, 0, s.px(1).max(1), h), 0xd7d8d4);
        let step = s.px(19);
        let mut y = s.px(12);
        let mut line = |ui: &mut Ui<'_>, text: &str, style: TextStyle, colour: u32| {
            ui.label_styled(
                Rect::new(panel.x + s.px(14), y, panel.w - s.px(28), step),
                text,
                style,
                colour,
                Align::Left,
            );
            y += step;
        };
        let time = |ms: u64| {
            if ms == 0 {
                "off".to_owned()
            } else {
                format!("{ms} ms")
            }
        };
        let limit = LIMITS[self.limit].map_or("none".to_owned(), |v| format!("{v:.0} px/s"));
        line(&mut ui, "Scroll tuning", heading, ink);
        let settings = [
            format!("1-4  profile: {}", PROFILES[self.profile]),
            format!(
                "S    notch smoothing: {}",
                time(NOTCH_TIMES[self.notch_time])
            ),
            format!("F    fine smoothing: {}", time(FINE_TIMES[self.fine_time])),
            format!("L    lead: {:.0}%", LEADS[self.lead] * 100.0),
            format!("D    device mode: {}", MODES[self.mode].1),
            format!("M    speed limit: {limit}"),
            "PgUp PgDn Home End  programmed moves".to_owned(),
            format!(
                "R    record: {}   ({} samples)",
                if self.recording { "ON" } else { "off" },
                self.trace.samples.len()
            ),
            "P    replay    W  write    Q  quit".to_owned(),
        ];
        for text in &settings {
            line(&mut ui, text, body, muted);
        }
        line(&mut ui, "", body, muted);
        line(&mut ui, "Last wheel movement", heading, ink);
        line(
            &mut ui,
            self.last.as_deref().unwrap_or("none yet"),
            body,
            ink,
        );
        line(&mut ui, "", body, muted);
        line(&mut ui, "Motion", heading, ink);
        let ledger = self.motion.ledger();
        let motion = [
            format!("position  {:.2} px", self.motion.position()),
            format!("target    {:.2} px", self.motion.target()),
            format!(
                "owed      {:+.2} px",
                self.motion.target() - self.motion.position()
            ),
            format!("velocity  {:+.0} px/s", self.motion.velocity()),
            format!("row       {:.1}", self.motion.position() / self.row_px),
            format!(
                "frame     {:.2} ms{}",
                self.interval.as_secs_f64() * 1_000.0,
                if self.motion.animating() {
                    "  (animating)"
                } else {
                    ""
                }
            ),
            format!("asked     {:.0} px in all", ledger.requested),
            format!("refused   {:.0} px at the ends", ledger.clamped),
            format!("dropped   {:.0} px on reversing", ledger.reversed),
        ];
        for text in &motion {
            line(&mut ui, text, body, muted);
        }
        line(&mut ui, "", body, muted);
        line(&mut ui, &self.status, body, ink);
        ui.input.end_frame();
    }

    fn on_key(&mut self, key: &KeyInput) {
        let next = |index: &mut usize, count: usize| *index = (*index + 1) % count;
        match key {
            KeyInput::Character(digit @ '1'..='4') => {
                self.profile = *digit as usize - '1' as usize;
                self.apply_profile();
            }
            KeyInput::Character('s' | 'S') => next(&mut self.notch_time, NOTCH_TIMES.len()),
            KeyInput::Character('f' | 'F') => next(&mut self.fine_time, FINE_TIMES.len()),
            KeyInput::Character('l' | 'L') => next(&mut self.lead, LEADS.len()),
            KeyInput::Character('d' | 'D') => {
                next(&mut self.mode, MODES.len());
                self.apply_profile();
            }
            KeyInput::Character('m' | 'M') => {
                next(&mut self.limit, LIMITS.len());
                self.apply_profile();
            }
            KeyInput::PageDown => self.page(1.0),
            KeyInput::PageUp => self.page(-1.0),
            KeyInput::Home => {
                self.interpreter.interrupt();
                self.motion
                    .animate_to(self.now, 0.0, Duration::from_millis(220));
            }
            KeyInput::End => {
                self.interpreter.interrupt();
                let (_, end) = self.motion.bounds();
                self.motion
                    .animate_to(self.now, end, Duration::from_millis(220));
            }
            KeyInput::Character('r' | 'R') => {
                self.recording = !self.recording;
                if self.recording {
                    self.trace = WheelTrace::new();
                    self.trace.note("device", "scroll_tuning recording");
                    self.trace.note("platform", std::env::consts::OS);
                    self.status = "recording: scroll, then R to stop".to_owned();
                } else {
                    self.status = format!("recorded {} samples", self.trace.samples.len());
                }
            }
            KeyInput::Character('p' | 'P') => {
                if self.trace.samples.is_empty() {
                    self.status = "nothing recorded: press R first".to_owned();
                } else {
                    self.recording = false;
                    self.interpreter.interrupt();
                    self.replay = Some((self.trace.samples.clone(), self.now));
                    self.status = format!("replaying {} samples", self.trace.samples.len());
                }
            }
            KeyInput::Character('w' | 'W') => {
                self.status = match std::fs::write("scroll-tuning.trace", self.trace.to_text()) {
                    Ok(()) => "wrote scroll-tuning.trace".to_owned(),
                    Err(error) => format!("could not write: {error}"),
                };
            }
            KeyInput::Character('q' | 'Q') => self.quit = true,
            _ => {}
        }
    }

    fn should_exit(&self) -> bool {
        self.quit
    }
}

fn main() {
    let mut tuning = Tuning {
        text: TextCache::new(set()),
        kernel: RasterKernel::new(),
        input: Input::new(),
        interpreter: WheelInterpreter::default(),
        motion: MotionController::new(),
        profile: 1,
        notch_time: 2,
        fine_time: 2,
        lead: 1,
        mode: 0,
        limit: 0,
        now: Duration::ZERO,
        interval: Duration::from_nanos(16_666_667),
        row_px: f64::from(ROW),
        view: (800.0, 600.0),
        last: None,
        recording: false,
        trace: WheelTrace::new(),
        replay: None,
        status: "experimental: settings here are not calibrated".to_owned(),
        quit: false,
    };
    tuning.apply_profile();
    run_app(
        tuning,
        WindowConfig::new("liminalkit scroll tuning", 1100.0, 720.0).min_size(720.0, 420.0),
    )
    .expect("run");
}
