//! EXPERIMENTAL: game UI over a planet, drawn entirely on the CPU.
//!
//! ```sh
//! cargo run --release -p liminalkit --example planet --features game
//! ```
//!
//! A turning planet (ray-cast here in software, standing in for a game's own
//! renderer) with `liminalkit::game` layers over it:
//!
//! * two windows **lying on the planet**. Drag one by its title bar: it
//!   slides over the surface. Drag it past the horizon and it peels off into
//!   a flat window; drop a flat window on the planet and it lays back down
//!   with a ripple. Their buttons work the whole time.
//! * a flat status panel with a **shimmer**, and buttons that post
//!   **notifications**; an alert **flashes** and pops.
//! * a popover **anchored** to a beacon on the surface, which follows it
//!   round and steps aside from the panel.
//!
//! | Key | |
//! |---|---|
//! | `1` `2` `3` | post a low, normal, critical notification |
//! | `Space` | stop or start the planet |
//! | `Esc` | quit |
//!
//! `--frames N` renders N frames without a window and prints timings;
//! `--png PATH` also writes the last one.

use std::time::{Duration, Instant};

use liminalkit::game::anchor::{self, Anchoring, Side};
use liminalkit::game::anim::Envelope;
use liminalkit::game::composite::{composite, CompositeScratch, CompositeStats, Target};
use liminalkit::game::effect::{Flash, Glow, Pop, Scanlines, Shimmer};
use liminalkit::game::frame::GameFrame;
use liminalkit::game::geom::{Argb, RectF};
use liminalkit::game::input::{PointerEvent, Router};
use liminalkit::game::layer::{Band, Layer, LayerKey};
use liminalkit::game::math::{M3, V3};
use liminalkit::game::notify::toast::{apply_outcome, paint_toast, toast_height};
use liminalkit::game::notify::{Notice, NoticeId, NoticeQueue, Priority, StackStyle};
use liminalkit::game::sheet::{Extent, Phase, Scene, Sheet};
use liminalkit::game::surface::{PaintContext, Surface};
use liminalkit::game::view::{Pinhole, View};
use liminalkit::game::world::{Pose, Sphere};
use liminalkit::raster::{Painter, RasterKernel, Rect, WindowBuffer};
use liminalkit::shell::scroll::event::host_time;
use liminalkit::shell::{KeyInput, MouseButton, Scale, WheelSample};
use liminalkit::text::{Align, TextCache};
use liminalkit::ui::toast::ToastCard;
use liminalkit::ui::{Theme, Ui};
use liminalkit::windowing::{run_app, CursorShape, FrameInfo, PixelApp, WindowConfig};

const RADIUS: f64 = 6_371_000.0;
const ALTITUDE: f64 = 800_000.0;
const SURVEY: LayerKey = LayerKey(1);
const COMMS: LayerKey = LayerKey(2);
const STATUS: LayerKey = LayerKey(3);
const BEACON: LayerKey = LayerKey(4);
const FIRST_TOAST: u64 = 100;
const GLASS: u32 = 0x0010_1820;
const EDGE: u32 = 0x0058_7890;
const INK: u32 = 0x00e8_f0f8;
const DIM: u32 = 0x0090_a4b8;
const TITLE_BAR: i32 = 28;

struct Toast {
    summary: String,
    body: String,
    priority: Priority,
}

struct Demo {
    text: TextCache,
    kernel: RasterKernel,
    router: Router,
    scratch: CompositeScratch,
    /// Survey, comms, status, beacon — in that order.
    layers: Vec<Layer>,
    toasts: NoticeQueue<Toast>,
    /// A layer per notice on screen, made when its card first appears.
    toast_layers: Vec<(NoticeId, Layer)>,
    beacon_side: Option<Side>,
    beacon_at: Option<[f32; 2]>,
    /// The planet at half resolution, kept for its allocation.
    planet: WindowBuffer,
    viewport: [u32; 2],
    scale: Scale,
    now: Option<Duration>,
    last: Option<Duration>,
    spin: f64,
    spinning: bool,
    pings: u32,
    quit: bool,
    placed: bool,
    interval: Duration,
    stats: CompositeStats,
    /// Time spent on the stand-in planet, and on everything else, in total.
    planet_time: Duration,
    interface_time: Duration,
}

/// The camera: 800 km up, pitched so the horizon crosses the upper part of
/// the frame.
fn camera(viewport: [u32; 2]) -> Pinhole {
    let pitch = M3::rodrigues(V3::X, 0.8);
    Pinhole {
        eye: V3::new(0.0, 0.0, RADIUS + ALTITUDE),
        right: pitch.mul_vec(V3::X),
        up: pitch.mul_vec(V3::Y),
        back: pitch.mul_vec(V3::Z),
        tan_half_fov_y: (27.5_f64).to_radians().tan(),
        viewport,
    }
}

fn globe(spin: f64) -> Sphere {
    Sphere {
        radius_m: RADIUS,
        pose: Pose {
            origin: V3::ZERO,
            rotation: M3::rodrigues(V3::Y, spin),
        },
    }
}

fn sun() -> V3 {
    V3::new(-0.45, 0.35, 0.8).try_normalize().unwrap_or(V3::Z)
}

impl Demo {
    fn new() -> Demo {
        Demo {
            text: TextCache::new(liminalkit_test_fonts::set()),
            kernel: RasterKernel::new(),
            router: Router::new(),
            scratch: CompositeScratch::default(),
            layers: Vec::new(),
            toasts: NoticeQueue::new(StackStyle::default()),
            toast_layers: Vec::new(),
            beacon_side: None,
            beacon_at: None,
            planet: WindowBuffer::new(0, 0),
            viewport: [0, 0],
            scale: Scale(1.0),
            now: None,
            last: None,
            spin: 0.0,
            spinning: true,
            pings: 0,
            quit: false,
            placed: false,
            interval: Duration::from_micros(16_667),
            stats: CompositeStats::default(),
            planet_time: Duration::ZERO,
            interface_time: Duration::ZERO,
        }
    }

    /// Build the layers for this scale, and lay the two windows on the planet.
    fn place(&mut self) {
        let s = self.scale;
        let view = camera(self.viewport);
        let planet = globe(self.spin);
        let scene = Scene {
            surface: &planet,
            view: &view,
            light: Some(sun()),
        };
        let (w, h) = (self.viewport[0] as f64, self.viewport[1] as f64);
        let window = |key: LayerKey, size: [i32; 2], at: [f64; 2], margin: u32| {
            let size = [
                s.px(size[0]) as u32 + 2 * margin,
                s.px(size[1]) as u32 + 2 * margin,
            ];
            let mut surface = Surface::new(size[0], size[1]);
            surface.set_margin(margin);
            surface.set_mip_levels(2);
            // Start flat, then lay it down where it should be: with no clock
            // that is immediate.
            let mut sheet = Sheet::flat(size, [0.0, 0.0]);
            sheet.extent = Extent::Screen { scale: 1.0 };
            sheet.lay_at(&scene, at, None);
            sheet.advance(None);
            let mut layer = Layer::new(key, Band::WorldUnderlay, surface).with_sheet(sheet);
            layer.corner = s.px(8) as f32;
            layer.interaction.drag_handle = Some(Rect::new(
                margin as i32,
                margin as i32,
                size[0] as i32 - 2 * margin as i32,
                s.px(TITLE_BAR),
            ));
            layer
        };
        let glow_margin = s.px(14) as u32;
        let survey = window(SURVEY, [300, 180], [w * 0.36, h * 0.62], 0);
        let mut comms = window(COMMS, [260, 150], [w * 0.68, h * 0.52], glow_margin);
        comms.effects.scanlines = Some(Scanlines {
            period: s.px(3) as f32,
            depth: 0.22,
            speed: s.px(18) as f32,
            flicker: 0.06,
            started: Duration::ZERO,
        });
        comms.effects.glow = Some(Glow {
            color: Argb::rgb(0x40e0ff),
            intensity: 0.55,
            radius: glow_margin as f32 - 2.0,
            outline: s.px(2) as f32,
            pulse: Some((Duration::from_millis(2_400), 0.5)),
        });

        let mut status = Layer::new(
            STATUS,
            Band::Chrome,
            Surface::new(s.px(250) as u32, s.px(150) as u32),
        );
        status
            .sheet
            .set_flat_origin([s.px(16) as f32, s.px(16) as f32]);
        status.corner = s.px(8) as f32;
        status.effects.shimmer = Some(Shimmer {
            width: s.px(70) as f32,
            ..Shimmer::sheen(Duration::from_millis(600))
        });

        let mut beacon = Layer::new(
            BEACON,
            Band::WorldAnnotation,
            Surface::new(s.px(190) as u32, s.px(58) as u32),
        );
        beacon.corner = s.px(6) as f32;
        beacon.interaction.hit = liminalkit::game::surface::HitTest::Alpha(255);

        self.layers = vec![survey, comms, status, beacon];
        self.placed = true;
    }

    fn post(&mut self, priority: Priority, summary: &str, body: &str) {
        let notice = Notice::new(Toast {
            summary: summary.to_string(),
            body: body.to_string(),
            priority,
        })
        .priority(priority)
        .lifetime(Duration::from_secs(if priority == Priority::Critical {
            9
        } else {
            5
        }));
        self.toasts.push(self.now, notice);
    }

    fn all_layers(&mut self) -> Vec<&mut Layer> {
        self.layers
            .iter_mut()
            .chain(self.toast_layers.iter_mut().map(|(_, layer)| layer))
            .collect()
    }

    fn pointer(&mut self, event: PointerEvent) {
        if !self.placed {
            return;
        }
        let view = camera(self.viewport);
        let planet = globe(self.spin);
        let scene = Scene {
            surface: &planet,
            view: &view,
            light: Some(sun()),
        };
        let now = self.now;
        let mut layers: Vec<&mut Layer> = self
            .layers
            .iter_mut()
            .chain(self.toast_layers.iter_mut().map(|(_, layer)| layer))
            .collect();
        self.router.pointer(event, &mut layers, Some(&scene), now);
    }

    /// Paint whatever needs it. Each layer's content is ordinary `Ui` code;
    /// what it returns is what its buttons asked for.
    fn paint(&mut self) {
        let (theme, scale, now) = (Theme::default(), self.scale, None::<Instant>);
        let mut posts: Vec<(Priority, &str, &str)> = Vec::new();
        let (mut flash_survey, mut toggle_spin) = (false, false);
        let spinning = self.spinning;
        let pings = self.pings;
        for layer in &mut self.layers {
            if !layer.surface.needs_paint() {
                continue;
            }
            let content = layer.surface.content();
            let flatness = layer.sheet.flatness();
            let cx = PaintContext {
                text: &mut self.text,
                kernel: &mut self.kernel,
                theme,
                scale,
                now,
                smooth_edges: true,
            };
            match layer.key {
                SURVEY => {
                    let pressed = layer.surface.paint(cx, |ui, _| {
                        let body = window_chrome(ui, content, "SURVEY 7", flatness);
                        let (line, rest) = body.split_top(ui.px(24));
                        ui.label(line, "Night side, sector 14", INK, Align::Left);
                        let (line, rest) = rest.split_top(ui.px(22));
                        let pings = format!("Pings sent: {pings}");
                        ui.label(line, &pings, DIM, Align::Left);
                        let (_, row) = rest.split_bottom(ui.px(32));
                        ui.button(row.split_left(ui.px(110)).0, "Ping")
                    });
                    if pressed {
                        flash_survey = true;
                        posts.push((
                            Priority::Normal,
                            "Ping sent",
                            "Survey 7 answered from the surface.",
                        ));
                    }
                }
                COMMS => {
                    layer.surface.paint(cx, |ui, _| {
                        let body = window_chrome(ui, content, "COMMS", flatness);
                        let (line, rest) = body.split_top(ui.px(24));
                        ui.label(line, "Carrier locked", 0x0070_f0ff, Align::Left);
                        let (line, _) = rest.split_top(ui.px(22));
                        ui.label(line, "Drag me off the planet", DIM, Align::Left);
                    });
                }
                STATUS => {
                    let asked = layer.surface.paint(cx, |ui, _| {
                        let all = ui.painter.bounds();
                        glass(ui, all, 0.0);
                        let inner = all.inset(ui.px(12));
                        let (title, rest) = inner.split_top(ui.px(26));
                        ui.heading(title, "Orbit 412");
                        let (line, rest) = rest.split_top(ui.px(22));
                        ui.label(line, "Windows lie on the planet.", DIM, Align::Left);
                        let (_, row) = rest.split_bottom(ui.px(32));
                        let (left, right) = row.split_left(row.w / 2);
                        let alert = ui.button(left.inset(ui.px(2)), "Alert");
                        let label = if spinning { "Stop" } else { "Spin" };
                        let spin = ui.button(right.inset(ui.px(2)), label);
                        (alert, spin)
                    });
                    if asked.0 {
                        posts.push((
                            Priority::Critical,
                            "Proximity alert",
                            "Debris crossing the orbit ahead.",
                        ));
                    }
                    toggle_spin = asked.1;
                }
                _ => {
                    layer.surface.paint(cx, |ui, _| {
                        let all = ui.painter.bounds();
                        glass(ui, all, 1.0);
                        let inner = all.inset(ui.px(8));
                        let (title, rest) = inner.split_top(ui.px(22));
                        ui.label(title, "Beacon K-2", INK, Align::Left);
                        ui.label(rest, "Anchored to the surface", DIM, Align::Left);
                    });
                }
            }
        }
        if flash_survey {
            self.pings += 1;
            let fired = self.now.unwrap_or_default();
            let survey = &mut self.layers[0];
            survey.effects.flash = Some(Flash {
                color: Argb::rgb(0x80d0ff),
                peak: 0.5,
                envelope: Envelope::flash(
                    Duration::from_millis(40),
                    Duration::from_millis(60),
                    Duration::from_millis(380),
                ),
                fired,
            });
            survey.surface.invalidate();
        }
        if toggle_spin {
            self.spinning = !self.spinning;
            self.layers[2].surface.invalidate();
        }
        for (priority, summary, body) in posts {
            self.post(priority, summary, body);
        }
    }

    /// Advance the notification stack and place a layer for each card.
    fn notifications(&mut self) -> bool {
        let (theme, scale) = (Theme::default(), self.scale);
        let width = scale.px(320);
        let mut style = *self.toasts.style();
        style.width = width as f32;
        style.gap = scale.px(8) as f32;
        self.toasts.set_style(style);
        let hovered = self
            .router
            .hovered()
            .and_then(|key| self.toast_layers.iter().find(|(_, l)| l.key == key))
            .map(|(id, _)| *id);
        let text = &mut self.text;
        let view = self.toasts.update(self.now, hovered, |toast| {
            toast_height(text, &theme, scale, width, &card(toast)) as f32
        });
        while self.toasts.take_retired().is_some() {}

        let corner = [
            self.viewport[0] as f32 - scale.px(16) as f32,
            scale.px(16) as f32,
        ];
        let cards: Vec<_> = self
            .toasts
            .cards()
            .map(|c| (c.id, c.rect, c.opacity, c.scale, c.fresh, c.priority))
            .collect();
        self.toast_layers
            .retain(|(id, _)| cards.iter().any(|card| card.0 == *id));
        let now = self.now.unwrap_or_default();
        for (id, rect, opacity, _, fresh, priority) in cards {
            let size = [rect.w.round() as u32, rect.h.round() as u32];
            let index = match self.toast_layers.iter().position(|(have, _)| *have == id) {
                Some(index) => index,
                None => {
                    let key = LayerKey(FIRST_TOAST + id.get());
                    let layer = Layer::new(key, Band::Overlay, Surface::new(size[0], size[1]));
                    self.toast_layers.push((id, layer));
                    self.toast_layers.len() - 1
                }
            };
            let layer = &mut self.toast_layers[index].1;
            layer.surface.resize(size[0], size[1]);
            layer
                .sheet
                .set_flat_origin([corner[0] + rect.x, corner[1] + rect.y]);
            layer.opacity = opacity;
            if fresh && priority == Priority::Critical {
                layer.effects.flash = Some(Flash {
                    color: Argb::rgb(0xff5040),
                    peak: 0.7,
                    envelope: Envelope::flash(
                        Duration::from_millis(50),
                        Duration::from_millis(90),
                        Duration::from_millis(420),
                    )
                    .repeat(liminalkit::game::anim::Repeat::Times(3)),
                    fired: now,
                });
                layer.effects.pop = Some(Pop {
                    fired: now,
                    duration: Duration::from_millis(260),
                    from: 0.86,
                    ease: liminalkit::game::anim::Ease::BackOut { overshoot: 1.7 },
                });
            }
            if layer.surface.needs_paint() {
                if let Some(toast) = self.toasts.payload_mut(id) {
                    let outcome = paint_toast(
                        &mut layer.surface,
                        PaintContext {
                            text: &mut self.text,
                            kernel: &mut self.kernel,
                            theme,
                            scale,
                            now: None,
                            smooth_edges: true,
                        },
                        &card(toast),
                    );
                    apply_outcome(&mut self.toasts, self.now, id, &outcome);
                }
            }
        }
        view.animating || view.next_deadline.is_some()
    }

    /// Put the beacon's popover beside its point on the planet.
    fn anchor_beacon(&mut self, scene: &Scene<'_>) {
        let pose = scene.surface.pose();
        // A fixed place on the body: it turns with the planet.
        let body = V3::new(0.12, 0.24, 0.96).try_normalize().unwrap_or(V3::Z) * RADIUS;
        let world = pose.to_world(body);
        let facing = pose
            .dir_to_world(body * (1.0 / RADIUS))
            .dot(scene.view.toward_viewer(world));
        let point = scene
            .view
            .project(world)
            .filter(|_| facing > 0.1)
            .map(|p| [p.px[0] as f32, p.px[1] as f32]);
        let status = self.layers[2].sheet.flat_rect();
        let beacon = &mut self.layers[3];
        beacon.hidden = point.is_none();
        self.beacon_at = point;
        let Some(point) = point else {
            return;
        };
        let size = beacon.surface.size();
        let bounds = RectF::new(0.0, 0.0, self.viewport[0] as f32, self.viewport[1] as f32);
        let reserved = [status];
        let placed = anchor::place(
            &Anchoring {
                gap: self.scale.px(14) as f32,
                reserved: &reserved,
                ..Anchoring::new(point, [size[0] as f32, size[1] as f32], bounds)
            },
            self.beacon_side,
        );
        self.beacon_side = Some(placed.side);
        beacon
            .sheet
            .set_flat_origin([placed.rect.x.round(), placed.rect.y.round()]);
    }

    fn frame(&mut self, buffer: &mut WindowBuffer) {
        let viewport = [buffer.width, buffer.height];
        let began = Instant::now();
        if viewport != self.viewport || !self.placed {
            self.viewport = viewport;
            self.place();
        }
        if let (Some(now), Some(last), true) = (self.now, self.last, self.spinning) {
            self.spin += now.saturating_sub(last).as_secs_f64() * 0.004;
        }
        self.last = self.now;

        self.paint();
        self.notifications();

        let view = camera(viewport);
        let planet = globe(self.spin);
        let scene = Scene {
            surface: &planet,
            view: &view,
            light: Some(sun()),
        };
        self.anchor_beacon(&scene);
        let now = self.now;
        for layer in self.all_layers() {
            // A window that changed how flat it is repaints its chrome.
            let before = layer.sheet.flatness();
            layer.prepare(now, Some(&scene));
            if layer.sheet.flatness() != before {
                layer.surface.invalidate();
            }
            // Lying windows are part of the world; flat ones join the chrome.
            if matches!(layer.key, SURVEY | COMMS) {
                layer.band = if layer.sheet.phase() == Phase::Surface {
                    Band::WorldUnderlay
                } else {
                    Band::Chrome
                };
            }
        }

        let before_planet = Instant::now();
        draw_planet(buffer, &mut self.planet, &planet, &view);
        let after_planet = Instant::now();
        if let Some(at) = self.beacon_at {
            let mut painter = Painter::new(buffer);
            painter.stroke_circle_aa(&mut self.kernel, at[0], at[1], 6.0, 2.0, 0x00ff_d060, 255);
            painter.fill_circle_aa(&mut self.kernel, at[0], at[1], 2.5, 0x00ff_d060, 255);
        }
        let mut frame = GameFrame::with_capacity(viewport, 16);
        for layer in &self.layers {
            frame.push(layer);
        }
        for (_, layer) in &self.toast_layers {
            frame.push(layer);
        }
        frame.finish();
        let clip = Rect::new(0, 0, viewport[0] as i32, viewport[1] as i32);
        self.stats = composite(&frame, buffer, Target::Xrgb, clip, &mut self.scratch);
        self.planet_time += after_planet - before_planet;
        self.interface_time += (before_planet - began) + after_planet.elapsed();
    }
}

fn card(toast: &Toast) -> ToastCard<'_> {
    ToastCard {
        app: match toast.priority {
            Priority::Critical => "ALERT",
            Priority::Low => "LOG",
            _ => "SHIP",
        },
        summary: &toast.summary,
        body: &toast.body,
        icon: None,
        actions: &[],
        dismiss_label: "Dismiss",
    }
}

/// Translucent panel glass. `emphasis` (0 lying … 1 flat) strengthens it:
/// a window that has come off the planet is made easier to read.
fn glass(ui: &mut Ui<'_>, rect: Rect, emphasis: f32) {
    let radius = ui.px(8) as f32;
    let alpha = (176.0 + 70.0 * emphasis) as u8;
    ui.painter
        .fill_rounded_rect_aa(ui.kernel, rect, radius, GLASS, alpha);
    let edge = (120.0 + 135.0 * emphasis) as u8;
    for (inset, alpha) in [(0, edge), (1, edge / 3)] {
        let r = rect.inset(inset);
        ui.painter.blend_rect(
            Rect::new(r.x + ui.px(8), r.y, r.w - ui.px(16), 1),
            EDGE,
            alpha,
        );
        ui.painter.blend_rect(
            Rect::new(r.x + ui.px(8), r.bottom() - 1, r.w - ui.px(16), 1),
            EDGE,
            alpha / 2,
        );
    }
}

/// A window's glass and title bar; returns the area left for its content.
fn window_chrome(ui: &mut Ui<'_>, content: Rect, title: &str, flatness: f32) -> Rect {
    glass(ui, content, flatness);
    let (bar, body) = content.split_top(ui.px(TITLE_BAR));
    ui.painter.blend_rect(
        Rect::new(bar.x + ui.px(8), bar.bottom() - 1, bar.w - ui.px(16), 1),
        EDGE,
        90,
    );
    ui.label(bar.inset(ui.px(8)), title, INK, Align::Left);
    let hint = if flatness > 0.5 { "flat" } else { "on surface" };
    ui.label(bar.inset(ui.px(8)), hint, DIM, Align::Right);
    body.inset(ui.px(12))
}

/// The stand-in for a game's renderer: a lit, turning sphere with a
/// graticule, at half resolution.
fn draw_planet(dest: &mut WindowBuffer, half: &mut WindowBuffer, planet: &Sphere, view: &Pinhole) {
    let (w, h) = (dest.width.div_ceil(2), dest.height.div_ceil(2));
    if (half.width, half.height) != (w, h) {
        half.resize(w, h);
    }
    let pose = planet.pose;
    let (eye, light) = (pose.to_body(view.eye), pose.dir_to_body(sun()));
    let grid = (15.0_f64).to_radians();
    let shade = |x: u32, y: u32| -> u32 {
        let px = [f64::from(x) * 2.0 + 1.0, f64::from(y) * 2.0 + 1.0];
        let Some(ray) = view.ray(px) else { return 0 };
        let dir = pose.dir_to_body(ray.dir);
        let b = eye.dot(dir);
        let disc = b * b - (eye.dot(eye) - RADIUS * RADIUS);
        let color = if disc >= 0.0 {
            let p = eye + dir * (-b - disc.sqrt());
            let n = p * (1.0 / RADIUS);
            let (lat, lon) = (n.y.asin(), n.x.atan2(n.z));
            let lit = (n.dot(light) * 0.85 + 0.15).clamp(0.04, 1.0);
            // Continents from a few products of the normal: no meaning, only
            // something to watch turn.
            let rough = (n.x * 9.0).sin() * (n.y * 7.0 + n.z * 4.0).cos();
            let land = n.x * n.y * 3.0 + n.z * n.x * n.x * 2.0 - n.y * n.z + rough * 0.5 > 0.05;
            let near = |angle: f64| {
                let off = (angle / grid).fract().abs();
                off.min(1.0 - off) * grid * RADIUS < 9_000.0
            };
            let mut rgb = if land {
                [70.0, 110.0, 70.0]
            } else {
                [22.0, 52.0, 110.0]
            };
            if near(lat) || near(lon) {
                rgb = [rgb[0] + 40.0, rgb[1] + 46.0, rgb[2] + 50.0];
            }
            // Thin air at the limb.
            let rim = (1.0 + n.dot(dir)).clamp(0.0, 1.0).powi(4);
            [
                rgb[0] * lit + 90.0 * rim,
                rgb[1] * lit + 140.0 * rim,
                rgb[2] * lit + 220.0 * rim,
            ]
        } else {
            // Space, with a glow hugging the planet and a few stars.
            let miss = ((-disc).sqrt() / RADIUS).min(1.0);
            let halo = (1.0 - miss * 9.0).max(0.0).powi(2);
            let hash = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) % 997;
            let star = if hash == 0 { 150.0 } else { 0.0 };
            [
                6.0 + 40.0 * halo + star,
                8.0 + 80.0 * halo + star,
                16.0 + 150.0 * halo + star,
            ]
        };
        let byte = |c: f64| c.clamp(0.0, 255.0) as u32;
        (byte(color[0]) << 16) | (byte(color[1]) << 8) | byte(color[2])
    };
    // A ray per pixel is the one thing here worth more than one core; a real
    // game does this on its GPU.
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(8);
    let rows = (h as usize).div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (band, pixels) in half.pixels.chunks_mut(rows * w as usize).enumerate() {
            let shade = &shade;
            scope.spawn(move || {
                for (index, pixel) in pixels.iter_mut().enumerate() {
                    let (x, y) = (index as u32 % w, (band * rows) as u32 + index as u32 / w);
                    *pixel = shade(x, y);
                }
            });
        }
    });
    for y in 0..dest.height {
        let from = &half.pixels[((y / 2) * w) as usize..][..w as usize];
        let to = &mut dest.pixels[(y * dest.width) as usize..][..dest.width as usize];
        for (x, pixel) in to.iter_mut().enumerate() {
            *pixel = from[x / 2];
        }
    }
}

impl PixelApp for Demo {
    fn render(&mut self, buffer: &mut WindowBuffer, scale: Scale) {
        self.scale = scale;
        self.frame(buffer);
    }

    fn on_frame(&mut self, frame: &FrameInfo) {
        self.now = Some(host_time(frame.now));
        self.interval = frame.interval;
    }

    fn on_scale(&mut self, scale: Scale) {
        self.scale = scale;
        self.placed = false;
    }

    fn on_key(&mut self, key: &KeyInput) {
        match key {
            KeyInput::Escape => self.quit = true,
            KeyInput::Character(' ') => {
                self.spinning = !self.spinning;
                if let Some(status) = self.layers.get_mut(2) {
                    status.surface.invalidate();
                }
            }
            KeyInput::Character('1') => self.post(
                Priority::Low,
                "Telemetry saved",
                "Orbit 412 written to the log.",
            ),
            KeyInput::Character('2') => self.post(
                Priority::Normal,
                "Signal acquired",
                "A repeating pattern on the night side.",
            ),
            KeyInput::Character('3') => self.post(
                Priority::Critical,
                "Proximity alert",
                "Debris crossing the orbit ahead.",
            ),
            _ => {}
        }
    }

    fn on_cursor(&mut self, x: f32, y: f32) {
        self.pointer(PointerEvent::Moved([x, y]));
    }

    fn on_cursor_left(&mut self) {
        self.pointer(PointerEvent::Left);
    }

    fn on_mouse(&mut self, button: MouseButton, pressed: bool) {
        self.pointer(PointerEvent::Button { button, pressed });
    }

    fn on_wheel(&mut self, sample: &WheelSample) {
        let _ = sample;
    }

    fn on_focus_changed(&mut self, focused: bool) {
        if !focused {
            self.pointer(PointerEvent::FocusLost);
        }
    }

    fn cursor_shape(&self) -> CursorShape {
        // The router's answer needs the layers mutably borrowed as a slice;
        // for the pointer shape the capture alone says enough.
        match self.router.captured() {
            Some(liminalkit::game::input::Capture::Move { .. }) => CursorShape::Grabbing,
            _ => CursorShape::Default,
        }
    }

    fn animation_interval(&self) -> Option<Duration> {
        // The planet turns, so every frame is different.
        Some(self.interval)
    }

    fn should_exit(&self) -> bool {
        self.quit
    }
}

/// Render frames with no window, driving a drag off the planet and back, and
/// report what it cost.
fn headless(frames: u32, png: Option<String>) {
    let mut demo = Demo::new();
    let mut buffer = WindowBuffer::new(1280, 720);
    let start = Instant::now();
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    let mut phases = Vec::new();
    for index in 0..frames {
        let at = start + Duration::from_micros(16_667 * u64::from(index));
        demo.on_frame(&FrameInfo {
            now: at,
            interval: Duration::from_micros(16_667),
        });
        // A scripted hand: take the comms window by its title bar, carry it
        // up past the horizon, across, and drop it back on the planet.
        let t = index as f32 / frames.max(1) as f32;
        if demo.placed {
            // The middle of its title bar, wherever that is drawn just now.
            let comms = &demo.layers[1];
            let bar = comms.interaction.drag_handle.unwrap_or_default();
            let grab = comms.sheet.screen_of([
                bar.x as f32 + bar.w as f32 / 2.0,
                bar.y as f32 + bar.h as f32 / 2.0,
            ]);
            match index {
                10 => {
                    if let Some(grab) = grab {
                        demo.on_cursor(grab[0], grab[1]);
                        demo.on_mouse(MouseButton::Left, true);
                    }
                }
                11.. if t < 0.45 => {
                    demo.on_cursor(870.0 - 300.0 * (t / 0.45), 370.0 - 360.0 * (t / 0.45))
                }
                _ if (0.45..0.8).contains(&t) => {
                    let k = (t - 0.45) / 0.35;
                    demo.on_cursor(570.0 - 150.0 * k, 10.0 + 440.0 * k);
                }
                _ if t >= 0.8 && demo.router.captured().is_some() => {
                    demo.on_mouse(MouseButton::Left, false)
                }
                _ => {}
            }
            if index == frames / 3 {
                demo.on_key(&KeyInput::Character('3'));
            }
        }
        let began = Instant::now();
        demo.render(&mut buffer, Scale(1.0));
        let took = began.elapsed();
        worst = worst.max(took);
        total += took;
        let phase = demo.layers[1].sheet.phase();
        if phases.last() != Some(&phase) {
            phases.push(phase);
        }
    }
    let per_frame = |d: Duration| d.as_secs_f64() * 1000.0 / f64::from(frames.max(1));
    println!(
        "{frames} frames at 1280x720: mean {:.2} ms, worst {:.2} ms",
        per_frame(total),
        worst.as_secs_f64() * 1000.0
    );
    println!(
        "  of which the stand-in planet {:.2} ms, the interface (paint, meshes, composite) {:.2} ms",
        per_frame(demo.planet_time),
        per_frame(demo.interface_time)
    );
    println!("the dragged window went through: {phases:?}");
    println!("last frame: {:?}", demo.stats);
    if let Some(path) = png {
        match std::fs::write(&path, liminalkit::raster::png::encode(&buffer)) {
            Ok(()) => println!("wrote {path}"),
            Err(error) => eprintln!("could not write {path}: {error}"),
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut frames, mut png) = (None, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => frames = args.next().and_then(|n| n.parse().ok()),
            "--png" => png = args.next(),
            other => eprintln!("unknown argument {other}"),
        }
    }
    if let Some(frames) = frames {
        headless(frames, png);
        return;
    }
    if let Err(error) = run_app(
        Demo::new(),
        WindowConfig::new("liminalkit game UI: planet", 1280.0, 720.0).min_size(800.0, 500.0),
    ) {
        eprintln!("{error}");
    }
}
