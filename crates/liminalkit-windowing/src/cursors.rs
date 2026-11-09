//! Pointer images the platform does not ship.
//!
//! Windows has no open-hand or closed-hand cursor: winit maps
//! `CursorIcon::Grab` and `Grabbing` to `IDC_SIZEALL`, the four-way move
//! arrows, which says "move this window piece", not "pick this up". The hands
//! are therefore drawn here, with the toolkit's own coverage kernel, as a white
//! hand with a dark outline (the look of the system arrow), and handed to the
//! platform as a custom cursor. Platforms with native hands (macOS, X11 and
//! Wayland cursor themes) keep their own.
//!
//! Pure: no window, no event loop, so the images are testable directly.

use liminalkit_raster::{FillRule, Path, RasterKernel};

/// A cursor image: straight-alpha RGBA rows, its size and its hotspot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImage {
    pub rgba: Vec<u8>,
    pub width: u16,
    pub height: u16,
    pub hotspot_x: u16,
    pub hotspot_y: u16,
}

/// The cursor's edge length in physical pixels for a display scale: 32 at
/// 100%, grown with the scale like the system cursors, never below 16.
pub fn cursor_size(scale: f32) -> u16 {
    (32.0 * scale).round().clamp(16.0, 256.0) as u16
}

/// A capsule from `(x0, y0)` to `(x1, y1)` with radius `r`, as one closed
/// subpath: half a circle around each end joined by straight sides.
fn capsule(path: &mut Path, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) {
    let angle = (y1 - y0).atan2(x1 - x0);
    let steps = 10;
    let mut first = true;
    let mut point = |path: &mut Path, x: f32, y: f32| {
        if first {
            path.move_to(x, y);
            first = false;
        } else {
            path.line_to(x, y);
        }
    };
    // Around the far end, then back around the near end.
    for i in 0..=steps {
        let a =
            angle - std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / steps as f32;
        point(path, x1 + r * a.cos(), y1 + r * a.sin());
    }
    for i in 0..=steps {
        let a =
            angle + std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / steps as f32;
        point(path, x0 + r * a.cos(), y0 + r * a.sin());
    }
    path.close();
}

/// The hand, in a 32-unit design square: an open hand (fingers spread a
/// little, thumb out) or, `closed`, the same hand with fingers curled. One
/// path per part: parts overlap, and filled separately and combined by
/// maximum, their winding directions cannot cancel.
fn hand_parts(closed: bool, unit: f32) -> Vec<Path> {
    let mut parts = Vec::new();
    let s = |v: f32| v * unit;
    // Fingers: centres 3.9 units apart, 3 units wide, so a 0.9-unit gap shows
    // as a dark line between them once the outline is drawn.
    let fingers: [(f32, f32, f32); 4] = if closed {
        [
            (10.6, 12.5, 1.5),
            (14.5, 11.5, 1.5),
            (18.4, 12.0, 1.5),
            (22.1, 13.5, 1.4),
        ]
    } else {
        [
            (10.6, 7.0, 1.5),
            (14.5, 4.5, 1.5),
            (18.4, 5.5, 1.5),
            (22.1, 8.5, 1.4),
        ]
    };
    for (cx, top, r) in fingers {
        let mut p = Path::new();
        capsule(&mut p, s(cx), s(top + r), s(cx), s(17.0), s(r));
        parts.push(p);
    }
    let mut palm = Path::new();
    palm.rounded_rect(s(8.8), s(14.5), s(15.0), s(13.0), s(4.5));
    parts.push(palm);
    let mut thumb = Path::new();
    if closed {
        capsule(&mut thumb, s(9.6), s(21.0), s(7.4), s(17.6), s(1.6));
    } else {
        capsule(&mut thumb, s(9.8), s(21.5), s(5.2), s(15.6), s(1.6));
    }
    parts.push(thumb);
    parts
}

/// Rasterize the open (`closed == false`) or closed hand at `size × size`.
pub fn hand_cursor(closed: bool, size: u16) -> CursorImage {
    let n = size.max(8) as usize;
    let unit = n as f32 / 32.0;
    // Fill coverage of the hand.
    let mut fill = vec![0u8; n * n];
    let mut kernel = RasterKernel::new();
    for mut part in hand_parts(closed, unit) {
        part.fill_with(&mut kernel, n, n, FillRule::NonZero, |y, x0, x1, cov| {
            let row = &mut fill[y * n..y * n + n];
            for x in x0..=x1.min(n - 1) {
                row[x] = row[x].max(cov[x]);
            }
        });
    }
    // Outline: the fill dilated by about a pixel per 32 px of size.
    let radius = (unit.round() as isize).max(1);
    let mut edge = vec![0u8; n * n];
    for y in 0..n as isize {
        for x in 0..n as isize {
            let mut m = 0u8;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx * dx + dy * dy > radius * radius + radius {
                        continue;
                    }
                    let (sx, sy) = (x + dx, y + dy);
                    if sx >= 0 && sy >= 0 && sx < n as isize && sy < n as isize {
                        m = m.max(fill[sy as usize * n + sx as usize]);
                    }
                }
            }
            edge[y as usize * n + x as usize] = m;
        }
    }
    // White fill over a near-black outline, composited straight-alpha.
    let mut rgba = vec![0u8; n * n * 4];
    for i in 0..n * n {
        let w = fill[i] as f32 / 255.0;
        let b = edge[i] as f32 / 255.0;
        let a = w + b * (1.0 - w);
        if a <= 0.0 {
            continue;
        }
        // Premultiplied: white contributes w, the outline colour (12) b*(1-w).
        let lum = (255.0 * w + 12.0 * b * (1.0 - w)) / a;
        let l = lum.round().clamp(0.0, 255.0) as u8;
        rgba[i * 4..i * 4 + 4].copy_from_slice(&[l, l, l, (a * 255.0).round() as u8]);
    }
    let hot = |v: f32| (v * unit).round().clamp(0.0, (n - 1) as f32) as u16;
    CursorImage {
        rgba,
        width: n as u16,
        height: n as u16,
        hotspot_x: hot(16.0),
        hotspot_y: hot(if closed { 18.0 } else { 16.0 }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(img: &CursorImage, x: usize, y: usize) -> u8 {
        img.rgba[(y * img.width as usize + x) * 4 + 3]
    }
    fn lum(img: &CursorImage, x: usize, y: usize) -> u8 {
        img.rgba[(y * img.width as usize + x) * 4]
    }

    #[test]
    fn the_hand_is_white_inside_a_dark_outline_on_a_clear_ground() {
        for closed in [false, true] {
            let img = hand_cursor(closed, 32);
            assert_eq!((img.width, img.height), (32, 32));
            assert_eq!(img.rgba.len(), 32 * 32 * 4);
            // The palm centre is opaque white; the corners are clear.
            assert_eq!(alpha(&img, 16, 22), 255);
            assert!(lum(&img, 16, 22) > 240, "palm {}", lum(&img, 16, 22));
            assert_eq!(alpha(&img, 0, 0), 0);
            assert_eq!(alpha(&img, 31, 31), 0);
            // A dark outline surrounds the white: no white pixel touches clear ground.
            let dark = (0..32 * 32)
                .filter(|&i| img.rgba[i * 4 + 3] > 128 && img.rgba[i * 4] < 100)
                .count();
            assert!(dark > 50, "outline pixels: {dark}");
            for y in 1..31 {
                for x in 1..31 {
                    if alpha(&img, x, y) == 255 && lum(&img, x, y) > 200 {
                        for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                            assert!(
                                alpha(&img, nx, ny) > 0,
                                "white at ({x},{y}) touches clear ground"
                            );
                        }
                    }
                }
            }
            // The hotspot sits on the hand.
            assert!(alpha(&img, img.hotspot_x as usize, img.hotspot_y as usize) > 200);
        }
    }

    #[test]
    fn the_open_hand_reaches_higher_than_the_closed_one() {
        let top = |img: &CursorImage| {
            (0..img.height as usize)
                .find(|&y| (0..img.width as usize).any(|x| alpha(img, x, y) > 0))
                .unwrap()
        };
        let open = hand_cursor(false, 32);
        let closed = hand_cursor(true, 32);
        assert!(
            top(&open) + 4 <= top(&closed),
            "open {} closed {}",
            top(&open),
            top(&closed)
        );
        assert_ne!(open, closed);
    }

    #[test]
    fn the_cursor_scales_with_the_display() {
        assert_eq!(cursor_size(1.0), 32);
        assert_eq!(cursor_size(1.5), 48);
        assert_eq!(cursor_size(2.0), 64);
        assert_eq!(cursor_size(0.25), 16);
        let big = hand_cursor(false, 64);
        assert_eq!(big.rgba.len(), 64 * 64 * 4);
        assert_eq!((big.hotspot_x, big.hotspot_y), (32, 32));
    }

    /// Review render: both hands at 32 and 48 px, 8x enlarged, over light and
    /// dark grounds. `LIMINALKIT_CURSOR_SNAPSHOT=dir cargo test -p
    /// liminalkit-windowing hand_cursor_review -- --ignored`.
    #[test]
    #[ignore = "writes a review PNG"]
    fn hand_cursor_review_snapshot() {
        let Some(dir) = std::env::var_os("LIMINALKIT_CURSOR_SNAPSHOT") else {
            return;
        };
        let zoom = 8usize;
        let imgs = [
            hand_cursor(false, 32),
            hand_cursor(true, 32),
            hand_cursor(false, 48),
            hand_cursor(true, 48),
        ];
        let cell = 48 * zoom + 16;
        let mut buf =
            liminalkit_raster::WindowBuffer::new((cell * imgs.len()) as u32, (cell * 2) as u32);
        for (row, ground) in [0x00f0_f0f0u32, 0x0030_3030].into_iter().enumerate() {
            for (col, img) in imgs.iter().enumerate() {
                for y in 0..cell {
                    for x in 0..cell {
                        let (ix, iy) = (x / zoom, y / zoom);
                        let mut c = ground;
                        if ix < img.width as usize && iy < img.height as usize {
                            let i = (iy * img.width as usize + ix) * 4;
                            let a = img.rgba[i + 3] as u32;
                            let l = img.rgba[i] as u32;
                            let g = ground & 0xff;
                            let v = (l * a + g * (255 - a)) / 255;
                            c = v << 16 | v << 8 | v;
                        }
                        buf.pixels[(row * cell + y) * buf.width as usize + col * cell + x] = c;
                    }
                }
            }
        }
        let path = std::path::Path::new(&dir).join("hand-cursors.png");
        liminalkit_raster::png::write(&path, &buf).unwrap();
    }
}
