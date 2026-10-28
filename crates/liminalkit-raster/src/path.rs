//! A small vector path: lines and quadratic curves, flattened to polylines
//! the coverage kernel can fill.
//!
//! Everything the terminal draws that is not a rectangle or a glyph — an
//! execution pulse, a ring marker, an episode bracket, a rounded pill — goes
//! through here and comes out anti-aliased by the same exact-area kernel that
//! renders text, so shapes and type have the same edge quality.

use crate::kernel::{FillRule, RasterKernel};

/// Device-space path in pixels, y down. Coordinates are `f32` so half-pixel
/// placement (a 1px hairline centred on `x + 0.5`) is expressible.
#[derive(Debug, Clone, Default)]
pub struct Path {
    points: Vec<[f32; 2]>,
    subpaths: Vec<(usize, usize)>,
    open: Option<usize>,
}

/// Flattening tolerance in pixels. A tenth of a pixel keeps curves at UI sizes
/// visually smooth without a wasteful segment count.
const TOLERANCE: f32 = 0.1;

impl Path {
    pub fn new() -> Path {
        Path::default()
    }

    pub fn clear(&mut self) {
        self.points.clear();
        self.subpaths.clear();
        self.open = None;
    }

    pub fn is_empty(&self) -> bool {
        self.subpaths.is_empty() && self.open.is_none()
    }

    /// Start a new subpath. An unfinished one is closed first — every subpath
    /// is implicitly closed for filling anyway.
    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Path {
        self.close();
        self.open = Some(self.points.len());
        self.points.push([x, y]);
        self
    }

    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Path {
        if self.open.is_none() {
            return self.move_to(x, y);
        }
        self.points.push([x, y]);
        self
    }

    /// Quadratic Bézier from the current point through `(cx, cy)` to `(x, y)`.
    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) -> &mut Path {
        let Some(&from) = self.points.last().filter(|_| self.open.is_some()) else {
            return self.move_to(x, y);
        };
        push_quadratic(&mut self.points, from, [cx, cy], [x, y]);
        self
    }

    /// Cubic Bézier, flattened directly.
    pub fn cubic_to(
        &mut self,
        c1x: f32,
        c1y: f32,
        c2x: f32,
        c2y: f32,
        x: f32,
        y: f32,
    ) -> &mut Path {
        let Some(&from) = self.points.last().filter(|_| self.open.is_some()) else {
            return self.move_to(x, y);
        };
        push_cubic(&mut self.points, from, [c1x, c1y], [c2x, c2y], [x, y]);
        self
    }

    pub fn close(&mut self) -> &mut Path {
        if let Some(start) = self.open.take() {
            let end = self.points.len();
            if end - start >= 3 {
                self.subpaths.push((start, end));
            } else {
                self.points.truncate(start);
            }
        }
        self
    }

    /// Append an axis-aligned rectangle (clockwise in device space).
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) -> &mut Path {
        self.move_to(x, y)
            .line_to(x + w, y)
            .line_to(x + w, y + h)
            .line_to(x, y + h)
            .close()
    }

    /// Append a full circle as four cubic arcs.
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32) -> &mut Path {
        self.ellipse(cx, cy, r, r)
    }

    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32) -> &mut Path {
        // Kappa for a quarter circle from four cubics.
        const K: f32 = 0.552_284_8;
        let (kx, ky) = (rx * K, ry * K);
        self.move_to(cx + rx, cy)
            .cubic_to(cx + rx, cy + ky, cx + kx, cy + ry, cx, cy + ry)
            .cubic_to(cx - kx, cy + ry, cx - rx, cy + ky, cx - rx, cy)
            .cubic_to(cx - rx, cy - ky, cx - kx, cy - ry, cx, cy - ry)
            .cubic_to(cx + kx, cy - ry, cx + rx, cy - ky, cx + rx, cy)
            .close()
    }

    /// Append the same circle wound the other way — combined with `circle`
    /// under the non-zero rule this cuts a hole, which is how a ring is drawn.
    pub fn circle_reversed(&mut self, cx: f32, cy: f32, r: f32) -> &mut Path {
        const K: f32 = 0.552_284_8;
        let k = r * K;
        self.move_to(cx + r, cy)
            .cubic_to(cx + r, cy - k, cx + k, cy - r, cx, cy - r)
            .cubic_to(cx - k, cy - r, cx - r, cy - k, cx - r, cy)
            .cubic_to(cx - r, cy + k, cx - k, cy + r, cx, cy + r)
            .cubic_to(cx + k, cy + r, cx + r, cy + k, cx + r, cy)
            .close()
    }

    /// Append a ring: outer radius `r`, stroke width `w` inward.
    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, w: f32) -> &mut Path {
        self.circle(cx, cy, r);
        let inner = (r - w).max(0.0);
        if inner > 0.0 {
            self.circle_reversed(cx, cy, inner);
        }
        self
    }

    /// Append a rounded rectangle.
    pub fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32) -> &mut Path {
        let r = radius.max(0.0).min(w / 2.0).min(h / 2.0);
        if r <= 0.0 {
            return self.rect(x, y, w, h);
        }
        const K: f32 = 0.552_284_8;
        let k = r * K;
        let (x1, y1) = (x + w, y + h);
        self.move_to(x + r, y)
            .line_to(x1 - r, y)
            .cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r)
            .line_to(x1, y1 - r)
            .cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1)
            .line_to(x + r, y1)
            .cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r)
            .line_to(x, y + r)
            .cubic_to(x, y + r - k, x + r - k, y, x + r, y)
            .close()
    }

    /// Append a stroked polyline as one quad per segment (butt caps, no
    /// joins). Fine for hairlines and brackets; a proper joined stroker is
    /// not needed by any current consumer.
    pub fn stroke_polyline(&mut self, points: &[[f32; 2]], width: f32) -> &mut Path {
        let half = width / 2.0;
        for pair in points.windows(2) {
            let ([x0, y0], [x1, y1]) = (pair[0], pair[1]);
            let (dx, dy) = (x1 - x0, y1 - y0);
            let len = (dx * dx + dy * dy).sqrt();
            if len <= f32::EPSILON {
                continue;
            }
            let (nx, ny) = (-dy / len * half, dx / len * half);
            self.move_to(x0 + nx, y0 + ny)
                .line_to(x1 + nx, y1 + ny)
                .line_to(x1 - nx, y1 - ny)
                .line_to(x0 - nx, y0 - ny)
                .close();
        }
        self
    }

    /// Append a straight stroked segment.
    pub fn stroke_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32) -> &mut Path {
        self.stroke_polyline(&[[x0, y0], [x1, y1]], width)
    }

    /// The flattened geometry, closing any open subpath.
    pub fn geometry(&mut self) -> (&[[f32; 2]], &[(usize, usize)]) {
        self.close();
        (&self.points, &self.subpaths)
    }

    /// Bounding box `[x0, y0, x1, y1]` of the flattened points.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let mut it = self.points.iter();
        let first = it.next()?;
        let mut b = [first[0], first[1], first[0], first[1]];
        for p in it {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
        }
        Some(b)
    }

    /// Rasterize into `row(y, x0, x1, coverage)` callbacks over a `w × h`
    /// device area — the same contract as [`RasterKernel::fill`].
    pub fn fill_with(
        &mut self,
        kernel: &mut RasterKernel,
        width: usize,
        height: usize,
        rule: FillRule,
        row: impl FnMut(usize, usize, usize, &mut [u8]),
    ) {
        self.close();
        if self.subpaths.is_empty() {
            return;
        }
        kernel.fill(&self.points, &self.subpaths, width, height, rule, row);
    }
}

fn push_quadratic(points: &mut Vec<[f32; 2]>, from: [f32; 2], control: [f32; 2], to: [f32; 2]) {
    // Segment count from the control polygon's deviation, like text.rs.
    let dx = from[0] - 2.0 * control[0] + to[0];
    let dy = from[1] - 2.0 * control[1] + to[1];
    let dev = (dx * dx + dy * dy).sqrt();
    let segments = ((dev / (4.0 * TOLERANCE)).sqrt().ceil() as usize).clamp(1, 64);
    for i in 1..=segments {
        let t = i as f32 / segments as f32;
        let mt = 1.0 - t;
        let x = mt * mt * from[0] + 2.0 * mt * t * control[0] + t * t * to[0];
        let y = mt * mt * from[1] + 2.0 * mt * t * control[1] + t * t * to[1];
        points.push([x, y]);
    }
}

fn push_cubic(points: &mut Vec<[f32; 2]>, p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2]) {
    let ddx = (p0[0] - 2.0 * p1[0] + p2[0])
        .abs()
        .max((p1[0] - 2.0 * p2[0] + p3[0]).abs());
    let ddy = (p0[1] - 2.0 * p1[1] + p2[1])
        .abs()
        .max((p1[1] - 2.0 * p2[1] + p3[1]).abs());
    let dev = (ddx * ddx + ddy * ddy).sqrt();
    let segments = ((dev * 0.75 / TOLERANCE).sqrt().ceil() as usize).clamp(1, 96);
    for i in 1..=segments {
        let t = i as f32 / segments as f32;
        let mt = 1.0 - t;
        let a = mt * mt * mt;
        let b = 3.0 * mt * mt * t;
        let c = 3.0 * mt * t * t;
        let d = t * t * t;
        points.push([
            a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
            a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total_coverage(path: &mut Path, w: usize, h: usize) -> f64 {
        let mut kernel = RasterKernel::new();
        let mut sum = 0.0;
        path.fill_with(&mut kernel, w, h, FillRule::NonZero, |_, x0, x1, cov| {
            for c in &cov[x0..=x1] {
                sum += f64::from(*c) / 255.0;
            }
        });
        sum
    }

    #[test]
    fn a_rect_covers_its_area_exactly() {
        let mut p = Path::new();
        p.rect(2.0, 3.0, 10.0, 5.0);
        let area = total_coverage(&mut p, 20, 20);
        assert!((area - 50.0).abs() < 0.01, "{area}");
    }

    #[test]
    fn a_half_pixel_rect_covers_half() {
        let mut p = Path::new();
        p.rect(2.5, 3.0, 10.0, 5.0);
        let area = total_coverage(&mut p, 20, 20);
        assert!((area - 50.0).abs() < 0.05, "{area}");
    }

    #[test]
    fn a_circle_area_is_close_to_pi_r_squared() {
        let mut p = Path::new();
        p.circle(20.0, 20.0, 10.0);
        let area = total_coverage(&mut p, 40, 40);
        let expected = std::f64::consts::PI * 100.0;
        assert!(
            (area - expected).abs() / expected < 0.01,
            "{area} vs {expected}"
        );
    }

    #[test]
    fn a_ring_is_the_difference_of_two_discs() {
        let mut p = Path::new();
        p.ring(20.0, 20.0, 10.0, 3.0);
        let area = total_coverage(&mut p, 40, 40);
        let expected = std::f64::consts::PI * (100.0 - 49.0);
        assert!(
            (area - expected).abs() / expected < 0.01,
            "{area} vs {expected}"
        );
    }

    #[test]
    fn a_stroke_covers_length_times_width() {
        let mut p = Path::new();
        p.stroke_line(2.0, 5.5, 22.0, 5.5, 1.0);
        let area = total_coverage(&mut p, 30, 30);
        assert!((area - 20.0).abs() < 0.05, "{area}");
    }

    #[test]
    fn a_circle_is_symmetric() {
        let mut p = Path::new();
        p.circle(10.0, 10.0, 6.0);
        let mut kernel = RasterKernel::new();
        let mut grid = vec![0u8; 20 * 20];
        p.fill_with(&mut kernel, 20, 20, FillRule::NonZero, |y, x0, x1, cov| {
            grid[y * 20 + x0..y * 20 + x1 + 1].copy_from_slice(&cov[x0..=x1]);
        });
        for y in 0..20 {
            for x in 0..20 {
                let mirror_x = grid[y * 20 + (19 - x)];
                let mirror_y = grid[(19 - y) * 20 + x];
                assert!(grid[y * 20 + x].abs_diff(mirror_x) <= 2);
                assert!(grid[y * 20 + x].abs_diff(mirror_y) <= 2);
            }
        }
    }

    #[test]
    fn degenerate_subpaths_are_dropped() {
        let mut p = Path::new();
        p.move_to(1.0, 1.0).line_to(2.0, 2.0).close();
        assert!(p.geometry().1.is_empty());
    }
}
