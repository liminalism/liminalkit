//! Every widget once, rendered headlessly to `target/gallery.png`.
//!
//! ```sh
//! cargo run -p liminalkit-ui --example gallery --features test-fonts
//! ```

use liminalkit_raster::{png, Painter, RasterKernel, Rect, WindowBuffer};
use liminalkit_shell::{Input, Scale};
use liminalkit_text::font::test_fonts::{set, LATIN};
use liminalkit_text::{Align, TextCache, TextStyle};
use liminalkit_ui::{ButtonStyle, ScrollState, Theme, Tooltip, TooltipStyle, Ui};

fn main() {
    let scale = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(1.0);
    let s = Scale(scale);
    let mut buffer = WindowBuffer::new(s.px(720) as u32, s.px(520) as u32);
    let mut text = TextCache::new(set());
    let mut kernel = RasterKernel::new();
    let mut input = Input::new();
    input.cursor_moved(s.f(96.0), s.f(58.0)); // hover the second button
    let mut scroll = ScrollState::new();
    let mut tabs = 1usize;
    let mut tip = Tooltip::new();

    let mut ui = Ui::new(
        Painter::new(&mut buffer),
        &mut text,
        &mut input,
        Theme::default(),
        s,
        &mut kernel,
    );
    ui.painter.clear(0xf4f3ef);
    let ink = 0x182027;
    let muted = 0x68727b;
    let line = 0xd7d8d4;
    let mono = TextStyle::tracked(LATIN, s.font(10.0), 0.08);
    let body = TextStyle::new(LATIN, s.font(13.0));
    let big = TextStyle::new(LATIN, s.font(24.0));
    let buttons = ButtonStyle {
        text: muted,
        text_active: 0xffffff,
        fill: 0xfaf9f5,
        fill_hover: 0xecebe6,
        fill_active: ink,
        edge: line,
        edge_active: ink,
        radius: 0,
    };

    let panel = ui.flat_panel(
        Rect::new(s.px(16), s.px(16), s.px(688), s.px(488)),
        0xfaf9f5,
        line,
        true,
    );
    ui.label_styled(
        Rect::new(panel.x + s.px(12), panel.y + s.px(6), s.px(300), s.px(28)),
        "LIMINALKIT GALLERY",
        big,
        ink,
        Align::Left,
    );

    // Buttons
    let mut x = panel.x + s.px(12);
    let y = panel.y + s.px(44);
    for (i, label) in ["RESET", "PAUSE", "STEP"].iter().enumerate() {
        ui.text_button(
            Rect::new(x, y, s.px(72), s.px(26)),
            label,
            mono,
            buttons,
            i == 2,
            true,
        );
        x += s.px(80);
    }
    ui.segmented(
        Rect::new(x + s.px(12), y, s.px(200), s.px(26)),
        &["¼×", "½×", "1×", "2×", "4×"],
        2,
        mono,
        buttons,
        4,
    );
    ui.underline_tabs(
        &mut tabs,
        Rect::new(panel.x + s.px(12), y + s.px(40), s.px(348), s.px(40)),
        &["CAUSAL FIELD", "REPLAY", "JOURNAL"],
        mono,
        ink,
        muted,
        ink,
        3,
    );
    ui.stepper(
        Rect::new(panel.x + s.px(380), y + s.px(40), s.px(160), s.px(40)),
        "1",
        "CONTRACTS",
        body,
        mono,
        buttons,
        ink,
        muted,
    );

    // Tracks and dots
    let ty = y + s.px(100);
    for (i, (name, frac, colour)) in [
        ("Persistence", 0.72, 0x315e76u32),
        ("Approach hold", 0.4, 0x4e7f86),
        ("Replenishment", 0.9, 0x3e8b76),
        ("Size diversity", 0.15, 0x8d7953),
    ]
    .iter()
    .enumerate()
    {
        let row = ty + s.px(22) * i as i32;
        ui.label_styled(
            Rect::new(panel.x + s.px(12), row, s.px(110), s.px(18)),
            name,
            body,
            ink,
            Align::Left,
        );
        ui.track(
            Rect::new(panel.x + s.px(130), row + s.px(6), s.px(200), s.px(5)),
            *frac,
            0xecebe6,
            *colour,
        );
        ui.dot(
            s.f(350.0) + panel.x as f32,
            row as f32 + s.f(9.0),
            s.f(3.5),
            *colour,
            Some((s.f(3.0), 40)),
        );
    }
    ui.pill(
        Rect::new(panel.x + s.px(400), ty, s.px(150), s.px(24)),
        "SYNTHETIC MBO",
        mono,
        0x765c28,
        0xf4ecd9,
        0xc7b48f,
        true,
    );
    ui.stat(
        Rect::new(panel.x + s.px(400), ty + s.px(34), s.px(150), s.px(40)),
        "TOP IMBALANCE",
        "0.62",
        mono,
        big,
        muted,
        ink,
    );

    // AA shapes
    let sy = ty + s.px(110);
    ui.painter.stroke_circle_aa(
        ui.kernel,
        panel.x as f32 + s.f(40.0),
        sy as f32 + s.f(20.0),
        s.f(12.0),
        s.f(2.0),
        0x236b84,
        255,
    );
    ui.painter.fill_circle_aa(
        ui.kernel,
        panel.x as f32 + s.f(40.0),
        sy as f32 + s.f(20.0),
        s.f(3.0),
        0x236b84,
        255,
    );
    ui.painter.stroke_polyline_aa(
        ui.kernel,
        &[
            [panel.x as f32 + s.f(80.0), sy as f32 + s.f(4.0)],
            [panel.x as f32 + s.f(88.0), sy as f32 + s.f(4.0)],
            [panel.x as f32 + s.f(88.0), sy as f32 + s.f(36.0)],
            [panel.x as f32 + s.f(80.0), sy as f32 + s.f(36.0)],
        ],
        s.f(2.0),
        0x3e8b76,
        255,
    );
    ui.painter.fill_rounded_rect_aa(
        ui.kernel,
        Rect::new(panel.x + s.px(100), sy, s.px(60), s.px(40)),
        s.f(8.0),
        0x9a4f3d,
        255,
    );
    ui.painter.line_aa(
        ui.kernel,
        panel.x as f32 + s.f(180.0),
        sy as f32 + s.f(40.0),
        panel.x as f32 + s.f(260.0),
        sy as f32 + s.f(2.0),
        s.f(1.5),
        0x6d598d,
        255,
    );

    // Scroll list
    let list = Rect::new(panel.x + s.px(400), sy, s.px(270), s.px(150));
    ui.flat_panel(list, 0xffffff, line, false);
    let heights = vec![s.px(22); 30];
    let row_style = body;
    ui.scroll_list(&mut scroll, list.inset(s.px(4)), &heights, |ui, i, rect| {
        let fill = if i % 2 == 0 { 0xffffff } else { 0xf7f6f2 };
        ui.painter.fill_rect(rect, fill);
        ui.label_styled(
            rect.inset(s.px(3)),
            &format!("row {i:02}  ADD  bid 6042.25 × {}", 10 + i * 7),
            row_style,
            ink,
            Align::Left,
        );
    });

    // Text sizes
    let mut ty2 = sy + s.px(60);
    for size in [7.0, 8.0, 9.0, 10.0, 12.0, 14.0] {
        let st = TextStyle::tracked(LATIN, s.font(size), 0.08);
        ui.label_styled(
            Rect::new(panel.x + s.px(12), ty2, s.px(380), s.px(18)),
            &format!("{size}px  ORDERFLOWER 6,042.25 ×1 ½× ¼× Δ σ — bid/ask 0.62"),
            st,
            ink,
            Align::Left,
        );
        ty2 += s.px(18);
    }

    tip.request(
        s.px(300),
        s.px(300),
        vec![
            ("BID 6,042.25 — raw 312".into(), mono, ink),
            ("persist 0.72 · hold 0.40".into(), body, muted),
        ],
    );
    tip.draw(
        &mut ui,
        Rect::new(0, 0, s.px(720), s.px(520)),
        TooltipStyle {
            fill: 0xffffff,
            edge: line,
            padding: 6,
            max_width: 240,
            offset: (12, 12),
        },
    );
    ui.input.end_frame();
    drop(ui);

    std::fs::create_dir_all("target").ok();
    let path = format!("target/gallery@{scale}x.png");
    png::write(&path, &buffer).expect("write png");
    println!("wrote {path} ({}×{})", buffer.width, buffer.height);
}
