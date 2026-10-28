//! EXPERIMENTAL / UNTESTED: original touch-feel playground. Compile checking is
//! not behavioral validation. No real touchscreen or interactive run validated.
//! R records a fresh list interaction; R stops; P replays with current tuning.
//! Mouse drag simulates one contact. Native touch and simulated mouse are kept
//! separate: after native touch is seen, mouse simulation is disabled.
use liminalkit_raster::{Painter, RasterKernel, Rect, WindowBuffer};
use liminalkit_shell::gesture::{DragAxis, Gesture, GestureConfig, GestureRecognizer};
use liminalkit_shell::scroll_physics::{Boundary, ScrollAxis, ScrollConfig};
use liminalkit_shell::touch_replay::{ReplayCursor, TouchRecording};
use liminalkit_shell::{Input, KeyInput, MouseButton, Scale, TouchEvent, TouchId, TouchPhase};
use liminalkit_text::{font::test_fonts::set, Align, TextCache};
use liminalkit_ui::{Theme, Ui};
use liminalkit_windowing::{run_app, PixelApp, WindowConfig};
use std::time::{Duration, Instant};

const TOP: f64 = 100.0;
const HEIGHT: f64 = 430.0;
const ROWS: usize = 40;
const LIST: Rect = Rect {
    x: 24,
    y: 100,
    w: 420,
    h: 430,
};
const BUTTONS: [&str; 14] = [
    "Record / stop (R)",
    "Replay (P)",
    "Friction - (1)",
    "Friction + (2)",
    "Damping - (3)",
    "Damping + (4)",
    "Drag slop - (5)",
    "Drag slop + (6)",
    "Elastic / clamp (B)",
    "Snap off / row (S)",
    "Spring - (7)",
    "Spring + (8)",
    "Max velocity - (9)",
    "Max velocity + (0)",
];

struct Surface {
    gestures: GestureRecognizer,
    scroll: ScrollAxis,
    zoom: f64,
    tapped: Option<usize>,
    pressed: Option<usize>,
    time: Instant,
}
impl Surface {
    fn new(config: ScrollConfig, gesture: GestureConfig, time: Instant) -> Self {
        let mut scroll = ScrollAxis::new(0.0, ROWS as f64 * 64.0 - HEIGHT, config).unwrap();
        scroll.jump_to(400.0);
        Self {
            gestures: GestureRecognizer::new(gesture).unwrap(),
            scroll,
            zoom: 1.0,
            tapped: None,
            pressed: None,
            time,
        }
    }
    fn advance(&mut self, time: Instant) {
        if let Some(dt) = time.checked_duration_since(self.time) {
            self.scroll.advance(dt);
            self.time = time;
        }
    }
    fn row_at(&self, position: (f64, f64)) -> Option<usize> {
        if !LIST.contains(position.0.round() as i32, position.1.round() as i32) {
            return None;
        }
        let row = ((position.1 - TOP + self.scroll.position()) / (64.0 * self.zoom)).floor();
        (row >= 0.0 && row < ROWS as f64).then_some(row as usize)
    }
    fn event(&mut self, event: TouchEvent) {
        self.advance(event.received_at);
        for gesture in self.gestures.update(event) {
            match gesture {
                Gesture::Pressed { position } => {
                    let catching = self.scroll.catch();
                    self.pressed = if catching {
                        None
                    } else {
                        self.row_at(position)
                    };
                    if catching {
                        self.gestures.suppress_tap();
                    }
                }
                Gesture::Tap { position } => {
                    if self.pressed.is_some() && self.row_at(position) == self.pressed {
                        self.tapped = self.pressed;
                    }
                }
                Gesture::DragStarted => {
                    self.pressed = None;
                    self.scroll.catch();
                }
                Gesture::Drag { delta } => {
                    self.scroll.drag_by(-delta.1);
                }
                Gesture::DragEnded { velocity } => self.scroll.release(-velocity.1),
                Gesture::Transform(delta) => {
                    self.pressed = None;
                    let old = self.zoom;
                    self.zoom = (old * delta.scale).clamp(0.5, 2.0);
                    // Preserve content under the moving centroid while scaling.
                    let anchor = delta.center.1 - TOP;
                    let offset = (self.scroll.position() + anchor - delta.translation.1)
                        * (self.zoom / old)
                        - anchor;
                    let mut config = self.scroll.config();
                    if config.snap_interval.is_some() {
                        config.snap_interval = Some(64.0 * self.zoom);
                    }
                    self.scroll.configure(config).unwrap();
                    self.scroll
                        .set_bounds(0.0, ROWS as f64 * 64.0 * self.zoom - HEIGHT)
                        .unwrap();
                    self.scroll.jump_to(offset);
                    self.scroll.catch();
                }
                Gesture::Cancelled => {
                    self.pressed = None;
                    if self.gestures.active() {
                        self.scroll.catch();
                    } else {
                        self.scroll.cancel();
                    }
                }
                Gesture::Released => {
                    self.pressed = None;
                    self.scroll.release(0.0);
                }
            }
        }
    }
    fn cancel(&mut self) {
        self.gestures.cancel();
        self.scroll.cancel();
        self.pressed = None;
    }
}
#[derive(Clone, Copy)]
enum Target {
    List,
    Button(usize),
}
struct Demo {
    surface: Surface,
    controls: GestureRecognizer,
    target: Option<Target>,
    config: ScrollConfig,
    gesture_config: GestureConfig,
    text: TextCache,
    kernel: RasterKernel,
    input: Input,
    scale: Scale,
    recording: TouchRecording,
    recording_on: bool,
    replay: Option<(Instant, ReplayCursor)>,
    status: String,
    graph: Vec<(f64, f64)>,
    mouse: (f64, f64),
    mouse_down: bool,
    native_seen: bool,
}
impl Demo {
    fn reset(&mut self, time: Instant) {
        self.surface = Surface::new(self.config, self.gesture_config, time);
        self.controls.cancel();
        self.target = None;
        self.graph.clear();
    }
    fn action(&mut self, index: usize) {
        if index == 0 {
            if self.recording_on {
                self.recording_on = false;
                self.surface.cancel();
                self.status = "Recording stopped; P replays from the fixed starting state".into();
            } else {
                self.replay = None;
                self.recording.clear();
                self.reset(Instant::now());
                self.recording_on = true;
                self.status = "RECORDING list input (max 4096 reports); R stops".into();
            }
            return;
        }
        if index == 1 {
            if self.recording.events().is_empty() || self.recording.truncated() {
                self.status = "Replay refused: record a nonempty, non-truncated trace".into();
                return;
            }
            self.recording_on = false;
            let now = Instant::now();
            self.reset(now);
            self.replay = Some((now, ReplayCursor::default()));
            self.status = "REPLAY with current tuning; live list input interrupts".into();
            return;
        }
        match index {
            2 => self.config.friction = (self.config.friction - 1.0).max(1.0),
            3 => self.config.friction = (self.config.friction + 1.0).min(20.0),
            4 => self.config.damping_ratio = (self.config.damping_ratio - 0.1).max(0.2),
            5 => self.config.damping_ratio = (self.config.damping_ratio + 0.1).min(2.0),
            6 => self.gesture_config.slop = (self.gesture_config.slop - 1.0).max(0.0),
            7 => self.gesture_config.slop = (self.gesture_config.slop + 1.0).min(30.0),
            8 => {
                self.config.boundary = if self.config.boundary == Boundary::Elastic {
                    Boundary::Clamp
                } else {
                    Boundary::Elastic
                }
            }
            9 => {
                self.config.snap_interval = if self.config.snap_interval.is_some() {
                    None
                } else {
                    Some(64.0)
                }
            }
            10 => self.config.spring_frequency = (self.config.spring_frequency - 2.0).max(2.0),
            11 => self.config.spring_frequency = (self.config.spring_frequency + 2.0).min(40.0),
            12 => {
                self.config.max_fling_velocity = (self.config.max_fling_velocity - 500.0).max(500.0)
            }
            13 => {
                self.config.max_fling_velocity =
                    (self.config.max_fling_velocity + 500.0).min(10000.0)
            }
            _ => return,
        }
        self.gesture_config.max_velocity = self.config.max_fling_velocity;
        self.replay = None;
        self.recording_on = false;
        self.reset(Instant::now());
        self.status = "Tuning changed; P compares the same recorded input".into();
    }
    fn route(&mut self, event: TouchEvent) {
        if self.target.is_none() && event.phase == TouchPhase::Started {
            let (x, y) = event.position;
            self.target = if LIST.contains(x.round() as i32, y.round() as i32) {
                Some(Target::List)
            } else {
                (0..BUTTONS.len())
                    .find(|i| button_rect(*i).contains(x.round() as i32, y.round() as i32))
                    .map(Target::Button)
            };
        }
        match self.target {
            Some(Target::List) => {
                if self.replay.take().is_some() {
                    self.surface.cancel();
                    self.status = "Replay interrupted".into();
                }
                if self.recording_on && !self.recording.record(event) {
                    self.recording_on = false;
                    self.status =
                        "Trace full or invalid; stopped. Truncated traces cannot replay".into();
                }
                self.surface.event(event);
                if !self.surface.gestures.active() {
                    self.target = None;
                }
            }
            Some(Target::Button(index)) => {
                for gesture in self.controls.update(event) {
                    if let Gesture::Tap { position } = gesture {
                        if button_rect(index)
                            .contains(position.0.round() as i32, position.1.round() as i32)
                        {
                            self.action(index);
                        }
                    }
                }
                if !self.controls.active() {
                    self.target = None;
                }
            }
            None => {}
        }
    }
    fn pump(&mut self, now: Instant) {
        if let Some((epoch, cursor)) = &mut self.replay {
            let elapsed = now.saturating_duration_since(*epoch);
            for recorded in cursor.due(&self.recording, elapsed) {
                if let Some(event) = recorded.event(*epoch) {
                    self.surface.event(event);
                }
            }
            // A recording stopped with contacts down must not leave them stuck.
            if cursor.finished(&self.recording) && self.surface.gestures.active() {
                self.surface.cancel();
            }
            if elapsed > self.recording.duration() + Duration::from_secs(4) {
                self.replay = None;
                self.status = "Replay finished; P repeats with current tuning".into();
            }
        }
        self.surface.advance(now);
    }
}
fn button_rect(index: usize) -> Rect {
    Rect::new(
        480 + (index % 2) as i32 * 240,
        100 + (index / 2) as i32 * 64,
        220,
        56,
    )
}
impl PixelApp for Demo {
    fn on_touch(&mut self, event: &TouchEvent) {
        if !self.native_seen {
            self.native_seen = true;
            self.mouse_down = false;
            self.target = None;
            self.surface.cancel();
            self.controls.cancel();
        }
        let mut event = *event;
        event.position.0 /= self.scale.0 as f64;
        event.position.1 /= self.scale.0 as f64;
        self.route(event);
    }
    fn on_cursor(&mut self, x: f32, y: f32) {
        self.mouse = (
            x as f64 / self.scale.0 as f64,
            y as f64 / self.scale.0 as f64,
        );
        if self.mouse_down && !self.native_seen {
            self.route(mouse_event(self.mouse, TouchPhase::Moved));
        }
    }
    fn on_mouse(&mut self, button: MouseButton, pressed: bool) {
        if button != MouseButton::Left || self.native_seen {
            return;
        }
        self.mouse_down = pressed;
        self.route(mouse_event(
            self.mouse,
            if pressed {
                TouchPhase::Started
            } else {
                TouchPhase::Ended
            },
        ));
    }
    fn on_cursor_left(&mut self) {
        if self.mouse_down {
            self.route(mouse_event(self.mouse, TouchPhase::Cancelled));
            self.mouse_down = false;
        }
    }
    fn on_key(&mut self, key: &KeyInput) {
        if let KeyInput::Character(c) = key {
            if let Some(index) = match c.to_ascii_lowercase() {
                'r' => Some(0),
                'p' => Some(1),
                '1'..='6' => Some(*c as usize - '1' as usize + 2),
                '7' => Some(10),
                '8' => Some(11),
                '9' => Some(12),
                '0' => Some(13),
                'b' => Some(8),
                's' => Some(9),
                _ => None,
            } {
                self.action(index);
            }
        }
    }
    fn on_focus_changed(&mut self, focused: bool) {
        if !focused {
            self.on_layout_changed();
        }
    }
    fn on_layout_changed(&mut self) {
        self.surface.cancel();
        self.controls.cancel();
        self.target = None;
        self.replay = None;
        self.recording_on = false;
        self.mouse_down = false;
        self.status = "Surface changed: gestures cancelled; recording stopped".into();
    }
    fn on_scale(&mut self, scale: Scale) {
        self.scale = scale;
    }
    fn animation_interval(&self) -> Option<Duration> {
        (self.surface.scroll.animating() || self.surface.gestures.active() || self.replay.is_some())
            .then_some(Duration::from_millis(8))
    }
    fn tick(&mut self) {
        self.pump(Instant::now());
    }
    fn render(&mut self, buffer: &mut WindowBuffer, scale: Scale) {
        self.scale = scale;
        self.pump(Instant::now());
        if self.graph.len() == 300 {
            self.graph.remove(0);
        }
        self.graph.push((
            self.surface.scroll.position(),
            self.surface.scroll.velocity(),
        ));
        let mut ui = Ui::new(
            Painter::new(buffer),
            &mut self.text,
            &mut self.input,
            Theme::default(),
            scale,
            &mut self.kernel,
        );
        ui.painter.clear(0xf4f3ef);
        ui.label(
            Rect::new(24, 12, 930, 32),
            "EXPERIMENTAL / UNTESTED - hardware, behavior and touch feel",
            0xa52a20,
            Align::Left,
        );
        ui.label(
            Rect::new(24, 48, 930, 32),
            "Drag / flick / catch / pinch. Tap rows on release. Mouse simulates one finger.",
            0x182027,
            Align::Left,
        );
        for i in 0..ROWS {
            let y = TOP + i as f64 * 64.0 * self.surface.zoom - self.surface.scroll.position();
            let h = 64.0 * self.surface.zoom;
            let top = y.max(TOP);
            let bottom = (y + h).min(TOP + HEIGHT);
            if bottom <= top {
                continue;
            }
            let colour = if self.surface.pressed == Some(i) {
                0xbedfec
            } else if self.surface.tapped == Some(i) {
                0xb8e1c0
            } else if i % 2 == 0 {
                0xe1e5e9
            } else {
                0xebeef0
            };
            ui.painter.fill_rect(
                Rect::new(
                    scale.px(24),
                    scale.pxf(top as f32),
                    scale.px(420),
                    scale.pxf((bottom - top) as f32),
                ),
                colour,
            );
            if y >= TOP && y + 30.0 <= TOP + HEIGHT {
                ui.label(
                    Rect::new(40, y.round() as i32, 380, 30),
                    &format!("Item {}  - release to select", i + 1),
                    0x182027,
                    Align::Left,
                );
            }
        }
        for (i, label) in BUTTONS.iter().enumerate() {
            let r = button_rect(i);
            ui.painter.fill_rect(
                Rect::new(scale.px(r.x), scale.px(r.y), scale.px(r.w), scale.px(r.h)),
                0xd8e2e9,
            );
            ui.label(r, label, 0x182027, Align::Centre);
        }
        ui.label(
            Rect::new(24, 540, 930, 32),
            &format!(
                "offset {:.1}  velocity {:.1}  zoom {:.2} | spring {:.0} rad/s | max velocity {:.0}",
                self.surface.scroll.position(),
                self.surface.scroll.velocity(),
                self.surface.zoom, self.config.spring_frequency, self.config.max_fling_velocity
            ),
            0x182027,
            Align::Left,
        );
        ui.label(
            Rect::new(24, 580, 920, 32),
            &format!(
                "friction {:.1}/s | damping {:.1} | slop {:.0} | {:?} | snap {:?}",
                self.config.friction,
                self.config.damping_ratio,
                self.gesture_config.slop,
                self.config.boundary,
                self.config.snap_interval
            ),
            0x182027,
            Align::Left,
        );
        ui.label(
            Rect::new(24, 616, 920, 32),
            &format!(
                "{} ({} reports)",
                self.status,
                self.recording.events().len()
            ),
            0x182027,
            Align::Left,
        );
        ui.label(
            Rect::new(24, 650, 920, 28),
            "Position (blue) and velocity (orange), last 300 rendered frames; independently scaled",
            0x68727b,
            Align::Left,
        );
        for series in 0..2 {
            let max = self
                .graph
                .iter()
                .map(|p| if series == 0 { p.0.abs() } else { p.1.abs() })
                .fold(1.0, f64::max);
            for (i, p) in self.graph.iter().enumerate() {
                let value = if series == 0 { p.0 } else { p.1 };
                let x = 24 + i as i32 * 3;
                let y = 730 - (value / max * 42.0) as i32;
                ui.painter.fill_rect(
                    Rect::new(scale.px(x), scale.px(y), scale.px(3), scale.px(3)),
                    if series == 0 { 0x168bba } else { 0xd96e20 },
                );
            }
        }
    }
}
fn mouse_event(position: (f64, f64), phase: TouchPhase) -> TouchEvent {
    TouchEvent {
        id: TouchId {
            device: u64::MAX,
            contact: 0,
        },
        phase,
        position,
        pressure: None,
        received_at: Instant::now(),
    }
}
fn main() {
    eprintln!(
        "EXPERIMENTAL / UNTESTED: no behavioral, hardware or touch-feel validation performed."
    );
    let config = ScrollConfig::default();
    let gesture_config = GestureConfig {
        axis: DragAxis::Vertical,
        ..GestureConfig::default()
    };
    run_app(
        Demo {
            surface: Surface::new(config, gesture_config, Instant::now()),
            controls: GestureRecognizer::new(GestureConfig::default()).unwrap(),
            target: None,
            config,
            gesture_config,
            text: TextCache::new(set()),
            kernel: RasterKernel::new(),
            input: Input::new(),
            scale: Scale::ONE,
            recording: TouchRecording::new(4096),
            recording_on: false,
            replay: None,
            status: "R starts a fresh recording; P replays".into(),
            graph: Vec::new(),
            mouse: (0.0, 0.0),
            mouse_down: false,
            native_seen: false,
        },
        WindowConfig::new("UNTESTED - liminalkit touch tuning", 980.0, 800.0).min_size(980.0, 800.0),
    )
    .expect("run experimental tuning example");
}
