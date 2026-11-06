//! A screen that repaints only what changed: a clock in one corner, a
//! counter in the other, and a button in between.
//!
//! ```sh
//! LIMINALKIT_FRAME_LOG=1 cargo run -p liminalkit --example clock
//! ```
//!
//! Every second the clock ticks and only its rectangle is painted; the
//! frame log shows the repainted share. Press the button and its
//! rectangle and the counter's are painted. Resize the window and
//! everything is, because `ctx.stale` says so. The screen does one thing
//! differently from `hello`: it clips the painter before drawing, to the
//! stale region plus the rectangles it knows changed. Everything else is the
//! same code a full repaint would run, and the host shows only what was
//! painted.

use std::time::{Duration, Instant};

use liminalkit::prelude::*;

struct Clock {
    started: Instant,
    shown_second: u64,
    clicks: u32,
    /// What changed since the last frame, in logical pixels. `None` means
    /// everything: the first frame, or a change this screen did not track.
    changed: Option<Rect>,
}

impl Clock {
    const CLOCK: Rect = Rect::new(16, 16, 200, 28);
    const BUTTON: Rect = Rect::new(16, 60, 120, 36);
    const COUNTER: Rect = Rect::new(16, 110, 288, 28);

    fn seconds(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

impl Screen for Clock {
    fn draw(&mut self, ui: &mut Ui<'_>, ctx: &mut Context) {
        // The contract, in one line: paint what is stale plus what changed.
        // A hover highlight moves with the pointer, so the button is always
        // counted as changed while the pointer is over it.
        let button = ui.rect(
            Self::BUTTON.x,
            Self::BUTTON.y,
            Self::BUTTON.w,
            Self::BUTTON.h,
        );
        let hovered = ui.input.hovering(button);
        if let Some(changed) = self.changed {
            let mut region = ctx
                .stale
                .union(ui.rect(changed.x, changed.y, changed.w, changed.h));
            if hovered {
                region = region.union(button);
            }
            ui.painter.push_clip(region);
        }
        self.changed = Some(Rect::default());

        ui.painter.clear(ui.theme.background);
        let second = self.seconds();
        let clock = ui.rect(Self::CLOCK.x, Self::CLOCK.y, Self::CLOCK.w, Self::CLOCK.h);
        ui.heading(
            clock,
            &format!(
                "{:02}:{:02}:{:02}",
                second / 3600,
                second / 60 % 60,
                second % 60
            ),
        );
        self.shown_second = second;
        if ui.button(button, "Count") {
            self.clicks += 1;
            self.changed = Some(Self::BUTTON.union(Self::COUNTER));
        }
        let counter = ui.rect(
            Self::COUNTER.x,
            Self::COUNTER.y,
            Self::COUNTER.w,
            Self::COUNTER.h,
        );
        ui.label(
            counter,
            &format!("{} clicks", self.clicks),
            ui.theme.text,
            Align::Left,
        );
        if ctx.key_pressed(KeyInput::Escape) {
            ctx.exit = true;
        }
    }

    fn animation_interval(&self) -> Option<Duration> {
        Some(Duration::from_millis(250))
    }

    fn tick(&mut self) {
        if self.seconds() != self.shown_second {
            self.changed = Some(self.changed.map_or(Self::CLOCK, |c| c.union(Self::CLOCK)));
        }
    }
}

fn main() {
    liminalkit::run(
        Clock {
            started: Instant::now(),
            shown_second: u64::MAX,
            clicks: 0,
            changed: None,
        },
        liminalkit_test_fonts::set(),
        WindowConfig::new("liminalkit clock", 320.0, 160.0),
    )
    .expect("run");
}
