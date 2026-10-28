//! Render each face at a range of sizes and report characters it lacks.
//!
//! ```sh
//! cargo run -p liminalkit-text --example glyph_sheet --features test-fonts -- [font.ttf ...]
//! ```
//! With no arguments the two test faces are used. Writes `target/glyph_sheet.png`.

use liminalkit_raster::{png, Painter, WindowBuffer};
use liminalkit_text::{Face, FaceId, FontSet, TextCache, TextStyle};

const SAMPLE: &str =
    "ORDERFLOWER 6,042.25 ×1 ½× ¼× Δ σ — ← → · … ⌄ bid/ask 0.62 CAUSAL FIELD 18 LEVELS";
const CHECK: &[char] = &[
    'Δ', 'σ', '¼', '½', '·', '—', '×', '…', '←', '→', '⌄', '−', '↺', 'Ⅱ', '▷', '│', '•', '°', '±',
    '≥', '≤', '€', '£', '¥', 'µ',
];
const SIZES: &[f32] = &[7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 24.0];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut set = FontSet::new();
    let mut names = Vec::new();
    if args.is_empty() {
        #[cfg(feature = "test-fonts")]
        {
            use liminalkit_text::font::test_fonts;
            set.add(test_fonts::latin());
            set.add(test_fonts::thai());
            names.push("NotoSans-Regular (test)".to_string());
            names.push("NotoSansThai-Regular (test)".to_string());
        }
        #[cfg(not(feature = "test-fonts"))]
        {
            eprintln!("pass font paths, or build with --features test-fonts");
            std::process::exit(2);
        }
    } else {
        for path in &args {
            let bytes: &'static [u8] =
                Box::leak(std::fs::read(path).expect("read font").into_boxed_slice());
            let face: &'static Face = Box::leak(Box::new(
                Face::from_bytes(bytes).expect("parse font (TrueType glyf only)"),
            ));
            set.add(face);
            names.push(path.clone());
        }
    }
    let set: &'static FontSet = Box::leak(Box::new(set));
    let mut cache = TextCache::new(set);

    let row_h: i32 = SIZES.iter().map(|s| (*s as i32) + 10).sum::<i32>() + 24;
    let mut buffer = WindowBuffer::new(1100, (row_h * set.len() as i32 + 20) as u32);
    let mut painter = Painter::new(&mut buffer);
    painter.clear(0xffffff);
    let mut y = 10;
    for (index, name) in names.iter().enumerate() {
        let face = FaceId(index as u8);
        let missing: String = CHECK
            .iter()
            .filter(|c| !set.face(face).covers(**c))
            .collect();
        println!(
            "{name}: missing {}",
            if missing.is_empty() {
                "nothing from the check list".to_string()
            } else {
                missing
                    .chars()
                    .map(|c| format!("{c} (U+{:04X})", c as u32))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        );
        cache.draw(
            &mut painter,
            name,
            10,
            y,
            TextStyle::new(face, 12.0),
            0x9a4f3d,
        );
        y += 20;
        for size in SIZES {
            let style = TextStyle::tracked(face, *size, 0.06);
            cache.draw(
                &mut painter,
                &format!("{size:>4}px "),
                10,
                y,
                TextStyle::new(face, 9.0),
                0x68727b,
            );
            cache.draw(&mut painter, SAMPLE, 60, y, style, 0x182027);
            y += *size as i32 + 10;
        }
        y += 4;
    }
    drop(painter);
    std::fs::create_dir_all("target").ok();
    png::write("target/glyph_sheet.png", &buffer).expect("png");
    println!("wrote target/glyph_sheet.png");
}
