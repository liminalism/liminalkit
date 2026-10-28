//! What the raster kernels a touchscreen frame leans on cost, one at a time.
//!
//! ```sh
//! cargo run --release -p liminalkit-raster --example kernel_profile
//! LIMINALKIT_SIMD=scalar cargo run --release -p liminalkit-raster --example kernel_profile
//! ```
//!
//! Medians of many repetitions, in microseconds, for the operations a frame
//! of a list-and-cards kiosk is made of. Compare runs on the same machine;
//! emulation (qemu) proves correctness, never speed.

use std::hint::black_box;
use std::time::{Duration, Instant};

use liminalkit_raster::{Fit, Icon, Painter, PreparedImage, RasterKernel, Rect, WindowBuffer};

/// The lowest of five medians: a laptop's clock scaling and other load only
/// ever add time, so the minimum is the steadiest estimate of the work.
fn median(mut f: impl FnMut(), runs: usize) -> Duration {
    (0..5)
        .map(|_| {
            let mut samples: Vec<Duration> = (0..runs)
                .map(|_| {
                    let started = Instant::now();
                    f();
                    started.elapsed()
                })
                .collect();
            samples.sort();
            samples[runs / 2]
        })
        .min()
        .expect("five rounds")
}

fn report(name: &str, d: Duration) {
    println!("{name:<44} {:>9.1} µs", d.as_secs_f64() * 1e6);
}

/// Coverage shaped like a line of anti-aliased text, synthetic: the rows
/// above the letters and below the baseline are empty, the rows between have
/// gaps between letters, solid stems, and partial edges.
fn glyph_rows(width: usize, rows: usize) -> Vec<Vec<u8>> {
    (0..rows)
        .map(|y| {
            let inked = (rows / 5..rows * 4 / 5).contains(&y);
            (0..width)
                .map(|x| {
                    if !inked {
                        return 0;
                    }
                    match (x + y * 3) % 40 {
                        0..=19 => 0,
                        20..=27 => 255,
                        n => ((n * 37 + y * 11) % 254 + 1) as u8,
                    }
                })
                .collect()
        })
        .collect()
}

fn main() {
    println!(
        "fast paths: {}",
        liminalkit_raster::blend::fast_paths_enabled()
    );
    let (w, h) = (1280u32, 800u32);
    let mut buffer = WindowBuffer::new(w, h);

    let rows = glyph_rows(600, 40);
    report(
        "text: 40 glyph rows x 600 px blended",
        median(
            || {
                let mut p = Painter::new(&mut buffer);
                for (y, row) in rows.iter().enumerate() {
                    p.blend_coverage_row(100, 100 + y as i32, 0x231c20, row);
                }
            },
            400,
        ),
    );

    report(
        "clear 1280x800",
        median(|| Painter::new(&mut buffer).clear(0xfaf6f4), 200),
    );

    report(
        "gradient 1280x128, horizontal",
        median(
            || {
                Painter::new(&mut buffer).fill_gradient(
                    Rect::new(0, 0, 1280, 128),
                    0xb0174f,
                    0x5e2a7e,
                    false,
                )
            },
            400,
        ),
    );

    // A card with a soft shadow: transparent margin, translucent shadow,
    // anti-aliased rounded edge, opaque interior.
    let card = PreparedImage::from_fn(836, 596, 28.0, |x, y| {
        let edge = x < 28 || y < 28 || x >= 808 || y >= 568;
        if edge {
            (((x + y) % 80) << 24) | 0x101010
        } else {
            0xffff_ffff
        }
    });
    report(
        "card image 836x596 (shadowed edges)",
        median(
            || Painter::new(&mut buffer).draw_image(&card, 200, 100),
            300,
        ),
    );

    let photo = {
        let mut b = liminalkit_raster::Bitmap::new(600, 450);
        for (i, p) in b.pixels.iter_mut().enumerate() {
            *p = 0xff00_0000 | (i as u32 * 2654435761) >> 8;
        }
        b
    };
    let thumb = PreparedImage::from_bitmap(&photo, 84, 84, Fit::Cover, 14.0);
    report(
        "6 thumbnails 84x84 (rounded)",
        median(
            || {
                let mut p = Painter::new(&mut buffer);
                for i in 0..6 {
                    p.draw_image(&thumb, 60, 140 + i * 112);
                }
            },
            400,
        ),
    );

    let mut kernel = RasterKernel::new();
    report(
        "AA pill 210x68 rasterized",
        median(
            || {
                Painter::new(&mut buffer).fill_rounded_rect_aa(
                    &mut kernel,
                    Rect::new(700, 30, 210, 68),
                    34.0,
                    0xffffff,
                    46,
                )
            },
            400,
        ),
    );

    let icon = Icon::new(24.0, &["M12 2a10 10 0 1 1 0 20a10 10 0 1 1 0-20z"]);
    let mask = icon.rasterize(&mut kernel, 30).unwrap();
    report(
        "12 icon masks 30x30 drawn",
        median(
            || {
                let mut p = Painter::new(&mut buffer);
                for i in 0..12 {
                    p.draw_mask(&mask, 1180, 140 + i * 50, 0x75696f);
                }
            },
            400,
        ),
    );
    black_box(&buffer);
}
