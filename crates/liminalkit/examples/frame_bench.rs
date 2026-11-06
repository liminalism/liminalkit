//! Headless frame-time measurement for an operational screen at several
//! resolutions: how the cost of software rendering grows with pixels, and
//! how much of it a frame that repaints only what changed avoids.
//!
//! No window. Each resolution renders the same screen (a panel, a 40-row
//! table, a scrolling list, labels at six sizes, and anti-aliased shapes) a
//! number of times into a `WindowBuffer`, and reports:
//!
//! - `cold`: the first frame, with nothing in the text cache;
//! - `p50`/`p95`/`max`: a full redraw with the cache warm, which is what a
//!   running application sees;
//! - `copy`: one whole-buffer copy at that size — what the desktop host
//!   paid on every present before the rasterizer drew straight into the
//!   presenter's buffer, and what the KMS host still pays for a frame that
//!   changed everything;
//! - `partial`: the same screen with the painter clipped to one changed
//!   table row, which is what a frame that knows what changed costs, and
//!   the share of the buffer it repainted;
//! - `resident`: the process's resident memory after that resolution.
//!
//! Then the text cache: what the screen left in it, and what happens when
//! a feed draws more new strings than the cache can hold, with the default
//! limits and with the limits a small kiosk might choose.
//!
//! ```sh
//! cargo run --release -p liminalkit --example frame_bench
//! cargo run --release -p liminalkit --example frame_bench -- 120
//! ```
//!
//! This measures liminalkit alone, and only what a buffer can measure.
//! Allocation counts are not here: a counting allocator is `unsafe impl
//! GlobalAlloc`, which this workspace forbids. Paint and present time and
//! the repainted share on a real window come from the host itself, with
//! `LIMINALKIT_FRAME_LOG=1` set on any application. A comparison against
//! other toolkits on the same screen is separate work and needs their
//! windows, not a buffer.

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
    /// Strings the feed has shown so far; each frame adds fresh ones.
    feed: u64,
}

impl Screen {
    /// Draw the screen. With `changed`, only that rectangle (plus whatever
    /// the buffer says is stale) is repainted: the painter is clipped and
    /// everything else runs as usual, which is the whole of the contract.
    /// Returns the rectangle of the table's first row, so a caller can
    /// pretend that is what changed.
    fn draw(&mut self, buffer: &mut WindowBuffer, s: Scale, changed: Option<Rect>) -> Rect {
        let stale = buffer.stale;
        let mut ui = Ui::new(
            Painter::new(buffer),
            &mut self.text,
            &mut self.input,
            Theme::default(),
            s,
            &mut self.kernel,
        );
        if let Some(changed) = changed {
            ui.painter.push_clip(stale.union(changed));
        }
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
        // The first body row: the header is one row_height, the body starts
        // under it. Close enough for "one row changed".
        let row_height = ui.px(ui.theme.row_height);
        let first_row = Rect::new(
            columns[0].x,
            columns[0].y + row_height,
            columns[0].w,
            row_height,
        );

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
        first_row
    }

    /// A feed: `fresh` strings nobody has drawn before, every frame. The
    /// kind of screen that cycles through more text than any cache holds.
    fn draw_feed(&mut self, buffer: &mut WindowBuffer, s: Scale, fresh: u64) {
        let mut ui = Ui::new(
            Painter::new(buffer),
            &mut self.text,
            &mut self.input,
            Theme::default(),
            s,
            &mut self.kernel,
        );
        ui.painter.clear(ui.theme.background);
        let bounds = ui.painter.bounds();
        let columns = hstack(bounds.inset(ui.px(16)), ui.px(12), &[Length::FILL; 4]);
        let per_column = fresh / 4;
        for (c, column) in columns.iter().enumerate() {
            let mut flow = Flow::down(*column, 0);
            for i in 0..per_column {
                let n = self.feed + c as u64 * per_column + i;
                ui.label(
                    flow.take(ui.px(18)),
                    &format!(
                        "{n:08}  fill  {:.2} × {}",
                        6000.0 + (n % 997) as f64 * 0.25,
                        n % 41
                    ),
                    ui.theme.text,
                    Align::Left,
                );
            }
        }
        self.feed += fresh;
        ui.input.end_frame();
    }
}

fn resident_kib() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4)
}

fn resident() -> String {
    resident_kib()
        .map(|kib| format!("{:.1} MiB", kib as f64 / 1024.0))
        .unwrap_or_else(|| "n/a".into())
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

/// p50, p95 and max of a sorted sample.
fn percentiles(times: &mut [f64]) -> (f64, f64, f64) {
    times.sort_by(f64::total_cmp);
    let p = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
    (p(0.5), p(0.95), times[times.len() - 1])
}

fn new_screen(text: TextCache) -> Screen {
    Screen {
        text,
        kernel: RasterKernel::new(),
        input: Input::new(),
        table: TableState::new(),
        scroll: ScrollState::new(),
        keys: (0..40).map(|i| format!("INS{i:02}")).collect(),
        feed: 0,
    }
}

fn cache_line(stats: CacheStats) -> String {
    format!(
        "{} entries ({} limit), {:.1} MiB of bitmaps ({} MiB budget), {} misses in {} lookups, {} evictions",
        stats.entries,
        stats.entry_limit,
        stats.bitmap_bytes as f64 / (1024.0 * 1024.0),
        stats.bitmap_budget / (1024 * 1024),
        stats.misses,
        stats.lookups,
        stats.evictions
    )
}

fn main() {
    let frames: usize = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(60);
    let mut screen = new_screen(TextCache::new(set()));
    println!(
        "{:>10} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>18}  resident",
        "size", "scale", "cold ms", "p50 ms", "p95 ms", "max ms", "copy ms", "partial ms (area)",
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
        let row = screen.draw(&mut buffer, s, None);
        let cold = ms(cold);
        let mut times: Vec<f64> = (0..frames)
            .map(|_| {
                let start = Instant::now();
                screen.draw(&mut buffer, s, None);
                ms(start)
            })
            .collect();
        let (p50, p95, max) = percentiles(&mut times);

        // The present copy: the whole buffer, once, into memory of its own.
        let mut shown = vec![0u32; buffer.pixels.len()];
        let mut copies: Vec<f64> = (0..frames.max(10))
            .map(|_| {
                let start = Instant::now();
                shown.copy_from_slice(&buffer.pixels);
                ms(start)
            })
            .collect();
        let (copy, _, _) = percentiles(&mut copies);
        std::hint::black_box(&shown);

        // One row changed, and the buffer still holds the last frame.
        buffer.stale = Rect::default();
        let pixels = buffer.pixels.len() as f64;
        let mut partials: Vec<f64> = (0..frames)
            .map(|_| {
                buffer.damage = Rect::default();
                let start = Instant::now();
                screen.draw(&mut buffer, s, Some(row));
                ms(start)
            })
            .collect();
        let damage = buffer.damage;
        let share = (damage.w.max(0) * damage.h.max(0)) as f64 / pixels * 100.0;
        let (partial, _, _) = percentiles(&mut partials);

        println!(
            "{:>10} {:>6} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>11.2} ({:>4.1}%)  {}",
            format!("{w}x{h}"),
            format!("{scale}x"),
            cold,
            p50,
            p95,
            max,
            copy,
            partial,
            share,
            resident()
        );
    }
    println!();
    println!(
        "text cache after the screen: {}",
        cache_line(screen.text.stats())
    );

    // A feed of fresh strings at 1080p: 200 new strings a frame, for a
    // cache that can hold them and one that cannot. What an eviction costs
    // is the frame that follows it.
    println!();
    println!("feed of 200 new strings a frame at 1920x1080, {frames} frames:");
    for (name, text) in [
        ("default limits", TextCache::new(set())),
        (
            "1024 entries, 16 MiB",
            TextCache::with_limits(set(), 1024, 16 << 20),
        ),
        (
            "256 entries, 4 MiB",
            TextCache::with_limits(set(), 256, 4 << 20),
        ),
    ] {
        let mut feed = new_screen(text);
        let mut buffer = WindowBuffer::new(1920, 1080);
        let mut times: Vec<f64> = (0..frames)
            .map(|_| {
                let start = Instant::now();
                feed.draw_feed(&mut buffer, Scale(1.0), 200);
                ms(start)
            })
            .collect();
        let (p50, p95, max) = percentiles(&mut times);
        println!(
            "  {name:<22} p50 {p50:>6.2} ms  p95 {p95:>6.2} ms  max {max:>6.2} ms  | {}",
            cache_line(feed.text.stats())
        );
    }
}
