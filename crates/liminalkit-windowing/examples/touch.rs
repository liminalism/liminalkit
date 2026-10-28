//! Touch diagnostics and a minimal touch-only UI. No hardware-specific driver.
//! Run with LIMINALKIT_FRAME_LOG=1 to measure host receipt → present return.
use liminalkit_raster::{Painter, RasterKernel, Rect, WindowBuffer};
use liminalkit_shell::{Input, Scale, TouchEvent, TouchPhase, TouchPointer, TouchState};
use liminalkit_text::{font::test_fonts::set, Align, TextCache};
use liminalkit_ui::{Theme, Ui};
use liminalkit_windowing::{run_app, PixelApp, WindowConfig};

struct Demo {
    touches: TouchState,
    pointer: TouchPointer,
    input: Input,
    text: TextCache,
    kernel: RasterKernel,
    presses: usize,
    zoom: f64,
    pan: (f64, f64),
    intervals: Vec<f64>,
}

impl PixelApp for Demo {
    fn on_touch(&mut self, event: &TouchEvent) {
        // Intervals are per contact, not between different fingers' reports.
        if event.phase == TouchPhase::Moved {
            if let Some(previous) = self.touches.contacts().iter().find(|c| c.id == event.id) {
                if let Some(dt) = event
                    .received_at
                    .checked_duration_since(previous.received_at)
                {
                    if self.intervals.len() == 512 {
                        self.intervals.remove(0);
                    }
                    self.intervals.push(dt.as_secs_f64() * 1000.0);
                }
            }
        }
        if let Some(delta) = self.touches.update(*event) {
            self.pan.0 += delta.translation.0;
            self.pan.1 += delta.translation.1;
            self.zoom = (self.zoom * delta.scale).clamp(0.25, 4.0);
        }
        self.pointer.update(&mut self.input, *event);
    }

    fn render(&mut self, buffer: &mut WindowBuffer, scale: Scale) {
        let width = scale.logical(buffer.width as i32) as i32;
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
            Rect::new(24, 20, width - 48, 32),
            "Touch input diagnostics",
            0x182027,
            Align::Left,
        );
        ui.label(
            Rect::new(24, 60, width - 48, 32),
            "Move fingers to pan; use two fingers to pinch.",
            0x182027,
            Align::Left,
        );
        if ui.button(
            Rect::new(24, 108, 240, 56),
            &format!("Press count: {}", self.presses),
        ) {
            self.presses += 1;
        }
        let mut intervals = self.intervals.clone();
        intervals.sort_by(f64::total_cmp);
        let interval = if intervals.is_empty() {
            "no motion reports".to_owned()
        } else {
            format!(
                "arrival interval p50 {:.1} ms / p95 {:.1} ms",
                intervals[intervals.len() / 2],
                intervals[(intervals.len() - 1) * 95 / 100]
            )
        };
        ui.label(
            Rect::new(24, 180, width - 48, 32),
            &format!("{} contacts | {}", self.touches.contacts().len(), interval),
            0x182027,
            Align::Left,
        );
        ui.label(
            Rect::new(24, 220, width - 48, 32),
            "Arrival intervals are not sensor or finger-to-photon latency.",
            0x68727b,
            Align::Left,
        );
        ui.label(
            Rect::new(24, 260, width - 48, 32),
            &format!(
                "Pan {:.0}, {:.0} physical px | zoom {:.2}x",
                self.pan.0, self.pan.1, self.zoom
            ),
            0x182027,
            Align::Left,
        );
        for contact in self.touches.contacts() {
            let size = scale.px(24);
            ui.painter.fill_rect(
                Rect::new(
                    contact.position.0.round() as i32 - size / 2,
                    contact.position.1.round() as i32 - size / 2,
                    size,
                    size,
                ),
                0x168bba,
            );
        }
        ui.input.end_frame();
    }
}

fn main() {
    run_app(
        Demo {
            touches: TouchState::default(),
            pointer: TouchPointer::default(),
            input: Input::new(),
            text: TextCache::new(set()),
            kernel: RasterKernel::new(),
            presses: 0,
            zoom: 1.0,
            pan: (0.0, 0.0),
            intervals: Vec::new(),
        },
        WindowConfig::new("liminalkit touchscreen", 900.0, 600.0),
    )
    .expect("run touch example");
}
