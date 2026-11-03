//! Headless frame-time measurement for an operational screen at several
//! resolutions: how the cost of software rendering grows with pixels.
//!
//! No window. Each resolution renders the same screen (a panel, a 40-row
//! table, a scrolling list, labels at six sizes, and anti-aliased shapes) a
//! number of times into a `WindowBuffer`, and reports the p50, p95 and
//! maximum time of one full redraw, plus the process's resident memory.
//! The text cache is warm after the first frame, which is what a running
//! application sees; the first frame is reported separately as the cold
//! cost.
//!
//! ```sh
//! cargo run --release -p liminalkit --example frame_bench
//! cargo run --release -p liminalkit --example frame_bench -- 120
//! ```
//!
//! This measures liminalkit alone. A comparison against other toolkits on
//! the same screen is separate work and needs their windows, not a buffer.

use std::time::Instant;

use liminalkit::prelude::*;
use liminalkit::ui::{Cell, Column, Row};
use liminalkit_test_fonts::{set, LATIN};

struct Screen {
    text: TextCache,
    kernel: RasterKernel,
    input: Input,
    table: TableState,
    scroll: ScrollState,
    keys: Vec<String>,
}

impl Screen {
    fn draw(&mut self, buffer: &mut WindowBuffer, s: Scale) {
        let mut ui = Ui::new(
            Painter::new(buffer),
            &mut self.text,
            &mut self.input,
            Theme::default(),
            s,
            &mut self.kernel,
        );
        let theme = ui.theme;
        ui.painter.clear(theme.background);
        let bounds = ui.painter.bounds();
        let outer = Insets::uniform(16).scaled(s).apply(bounds);
        let panel = ui.panel(outer);
        let rows = vstack(
            panel,
            ui.px(12),
            &[Length::Px(ui.px(40)), Length::FILL, Length::Px(ui.px(140))],
        );
        ui.heading(rows[0], "ORDER FLOW — synthetic feed, 40 instruments");

        let columns = hstack(rows[1], ui.px(12), &[Length::FILL, Length::Px(ui.px(320))]);
        let cols = [
            Column::fixed("code", 90, Align::Left),
            Column::flex("name", 3, Align::Left),
            Column::flex("bid", 1, Align::Right),
            Column::flex("ask", 1, Align::Right),
            Column::flex("Δ", 1, Align::Right),
        ];
        let keys = &self.keys;
        ui.table(&mut self.table, columns[0], &cols, keys, |i| Row {
            cells: vec![
                Cell::new(keys[i].as_str()),
                Cell::new(format!("Instrument {i:02} — continuous")),
                Cell::new(format!("{:.2}", 6042.25 + i as f64 * 0.75)),
                Cell::new(format!("{:.2}", 6042.50 + i as f64 * 0.75)),
                Cell::coloured(
                    format!("{:+.2}", (i as f64 - 20.0) * 0.13),
                    if i % 2 == 0 { theme.good } else { theme.alarm },
                ),
            ],
        });

        let heights = vec![ui.px(22); 60];
        ui.scroll_list(&mut self.scroll, columns[1], &heights, |ui, i, rect| {
            if i % 2 == 1 {
                ui.painter.fill_rect(rect, ui.theme.panel_alt);
            }
            ui.label(
                rect.inset(ui.px(3)),
                &format!("event {i:03}  ADD  bid 6042.25 × {}", 10 + i * 7),
                ui.theme.text,
                Align::Left,
            );
        });

        let footer = hstack(rows[2], ui.px(12), &[Length::FILL, Length::FILL]);
        let mut flow = Flow::down(footer[0], ui.px(2));
        for size in [7.0, 8.0, 9.0, 10.0, 12.0, 14.0] {
            let style = ui.text_style(TextStyle::tracked(LATIN, size, 0.08));
            ui.label_styled(
                flow.take(ui.px(20)),
                &format!("{size}px  ORDERFLOWER 6,042.25 ×1 ½× ¼× Δ σ — bid/ask 0.62"),
                style,
                theme.text_dim,
                Align::Left,
            );
        }
        let shapes = footer[1];
        for i in 0..12 {
            let cx = shapes.x as f32 + s.f(20.0 + 26.0 * i as f32);
            let cy = shapes.y as f32 + s.f(24.0);
            ui.painter
                .fill_circle_aa(ui.kernel, cx, cy, s.f(9.0), theme.accent, 255);
            ui.painter.stroke_circle_aa(
                ui.kernel,
                cx,
                cy + s.f(30.0),
                s.f(9.0),
                s.f(2.0),
                theme.good,
                255,
            );
        }
        ui.painter.stroke_polyline_aa(
            ui.kernel,
            &(0..40)
                .map(|i| {
                    [
                        shapes.x as f32 + s.f(8.0 * i as f32),
                        shapes.bottom() as f32 - s.f(10.0 + 30.0 * ((i as f32) * 0.7).sin().abs()),
                    ]
                })
                .collect::<Vec<_>>(),
            s.f(1.5),
            theme.alarm,
            255,
        );
        ui.input.end_frame();
    }
}

fn resident_kib() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4)
}

fn main() {
    let frames: usize = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(60);
    let mut screen = Screen {
        text: TextCache::new(set()),
        kernel: RasterKernel::new(),
        input: Input::new(),
        table: TableState::new(),
        scroll: ScrollState::new(),
        keys: (0..40).map(|i| format!("INS{i:02}")).collect(),
    };
    let header = [
        "size", "scale", "cold ms", "p50 ms", "p95 ms", "max ms", "resident",
    ];
    println!(
        "{:>10} {:>6} {:>9} {:>9} {:>9} {:>9}  {}",
        header[0], header[1], header[2], header[3], header[4], header[5], header[6]
    );
    for (w, h, scale) in [
        (1280u32, 720u32, 1.0f32),
        (1920, 1080, 1.0),
        (1920, 1080, 1.5),
        (2560, 1440, 1.5),
        (3840, 2160, 2.0),
    ] {
        let mut buffer = WindowBuffer::new(w, h);
        let s = Scale(scale);
        let cold = Instant::now();
        screen.draw(&mut buffer, s);
        let cold = cold.elapsed().as_secs_f64() * 1000.0;
        let mut times: Vec<f64> = (0..frames)
            .map(|_| {
                let start = Instant::now();
                screen.draw(&mut buffer, s);
                start.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        times.sort_by(f64::total_cmp);
        let p = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
        let rss = resident_kib()
            .map(|kib| format!("{:.1} MiB", kib as f64 / 1024.0))
            .unwrap_or_else(|| "n/a".into());
        println!(
            "{:>10} {:>6} {:>9.2} {:>9.2} {:>9.2} {:>9.2}  {}",
            format!("{w}x{h}"),
            format!("{scale}x"),
            cold,
            p(0.5),
            p(0.95),
            times[times.len() - 1],
            rss
        );
    }
}
