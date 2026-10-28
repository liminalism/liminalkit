//! Analytic scanline coverage — exact-area anti-aliasing.
//!
//! Ported from the lege-pdf renderer's `pdf-render-cpu::raster`, trimmed to
//! what glyph-sized fills need: the AVX2 conversion, the cancellation probe,
//! the active-edge-table fallback and the sparse-window heuristic all exist
//! there for whole-page vector art and are dead weight for a 20-pixel Thai
//! consonant.
//!
//! The algorithm is the AGG/font-rs family. Device-space edges deposit *signed
//! area* into a bbox-local accumulation buffer — each edge walked once,
//! touching only the cells it actually crosses — and a per-row prefix sum turns
//! that into per-pixel coverage. It is exact in both axes, with no
//! supersampling: a 45° stem edge produces the true fractional area, not a
//! quantised approximation of it. That matters more here than it does for a
//! page of body text, because a Thai tone mark at receipt sizes is a handful of
//! pixels and any coverage error is a visible change of shape.
//!
//! [`RasterKernel::fill_reference`] is the straightforward per-scanline
//! implementation, kept as a cross-check: the two must agree byte for byte, and
//! a test asserts that they do.

/// How to turn a winding number into coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillRule {
    /// Glyph outlines. Outer and inner contours wind oppositely, so a counter
    /// (the hole in ด) cancels to zero.
    NonZero,
    EvenOdd,
}

/// An edge made monotonic in y, in device space.
#[derive(Debug, Clone, Copy)]
struct Edge {
    y_top: f32,
    y_bot: f32,
    x_at_top: f32,
    dxdy: f32,
    /// +1 downward, −1 upward. This is what makes the winding number work.
    dir: f32,
}

/// Reusable scratch for rasterizing. One per thread; the buffers grow to the
/// largest fill they have seen and are then reused, so a page of text allocates
/// once rather than per glyph.
#[derive(Debug, Default)]
pub struct RasterKernel {
    edges: Vec<Edge>,
    /// Bbox-local signed-area accumulation, row-major with per-fill `stride`.
    /// Only touched cells are reset between fills.
    acc: Vec<f32>,
    /// Per-row touched column range, in local column indices. `min > max`
    /// marks a row nothing reached.
    row_min: Vec<u32>,
    row_max: Vec<u32>,
    /// Per-scanline coverage, indexed by absolute device column.
    cov: Vec<u8>,
    /// Scratch for the reference implementation.
    acc_row: Vec<f32>,
}

impl RasterKernel {
    pub fn new() -> RasterKernel {
        RasterKernel::default()
    }

    /// Rasterize closed subpaths and hand each covered row to `row`.
    ///
    /// `points` holds device-space vertices; `subpaths` gives `[start, end)`
    /// index ranges into it, each implicitly closed. `row(y, x0, x1, cov)` is
    /// called once per covered row with `cov[x0..=x1]` holding coverage bytes
    /// (255 = full). The slice outside that span is zero, and the span is
    /// re-zeroed after the callback returns, so the buffer is clean for the
    /// next row.
    pub fn fill(
        &mut self,
        points: &[[f32; 2]],
        subpaths: &[(usize, usize)],
        width: usize,
        height: usize,
        rule: FillRule,
        row: impl FnMut(usize, usize, usize, &mut [u8]),
    ) {
        if width == 0 || height == 0 {
            return;
        }
        if self.cov.len() < width {
            self.cov.resize(width, 0);
        }

        let Some(bounds) = self.build_edges(points, subpaths) else {
            return;
        };
        let (y_min, y_max, x_min, x_max) = bounds;

        let row_start = y_min.floor().max(0.0) as usize;
        let row_end = y_max.ceil().min(height as f32).max(0.0) as usize;
        if row_end <= row_start {
            return;
        }

        let fwidth = width as f32;
        let bx = x_min.floor().clamp(0.0, (width - 1) as f32) as usize;
        let right = x_max.ceil().clamp(0.0, fwidth) as usize;
        let bw = right.saturating_sub(bx).max(1);
        let bh = row_end - row_start;
        // One carry cell for the area that spills into the next column, plus a
        // slack cell so the final carry has somewhere to land.
        let stride = bw + 2;
        let cells = stride * bh;

        if self.acc.len() < cells {
            self.acc.resize(cells, 0.0);
        }
        if self.row_min.len() < bh {
            self.row_min.resize(bh, 0);
            self.row_max.resize(bh, 0);
        }
        for local_row in 0..bh {
            self.row_min[local_row] = u32::MAX;
            self.row_max[local_row] = 0;
        }

        // --- Pass 1: deposit each edge once, into only the rows it crosses ---
        for index in 0..self.edges.len() {
            let edge = self.edges[index];
            let start = (edge.y_top.floor() as isize).max(row_start as isize) as usize;
            let end = (edge.y_bot.ceil() as isize).min(row_end as isize).max(0) as usize;

            if edge.dxdy == 0.0 {
                // Vertical edges are the common case in a glyph stem, and they
                // need no x-walk: the column and its fractional split are fixed.
                let x = edge.x_at_top.clamp(0.0, fwidth);
                let column = x.floor().clamp(0.0, fwidth - 1.0) as usize;
                let local = column.saturating_sub(bx).min(stride - 2);
                let fraction = (column as f32 + 1.0) - x;
                for y in start..end {
                    let top = edge.y_top.max(y as f32);
                    let bottom = edge.y_bot.min(y as f32 + 1.0);
                    if bottom <= top {
                        continue;
                    }
                    let cover = (bottom - top) * edge.dir;
                    let local_row = y - row_start;
                    let base = local_row * stride;
                    let here = cover * fraction;
                    self.acc[base + local] += here;
                    self.acc[base + local + 1] += cover - here;
                    note_column(
                        local as u32,
                        &mut self.row_min[local_row],
                        &mut self.row_max[local_row],
                    );
                }
            } else {
                for y in start..end {
                    let top = edge.y_top.max(y as f32);
                    let bottom = edge.y_bot.min(y as f32 + 1.0);
                    if bottom <= top {
                        continue;
                    }
                    let xa = edge.x_at_top + (top - edge.y_top) * edge.dxdy;
                    let xb = edge.x_at_top + (bottom - edge.y_top) * edge.dxdy;
                    let local_row = y - row_start;
                    deposit_edge_row(
                        &mut self.acc,
                        local_row * stride,
                        xa,
                        top,
                        xb,
                        bottom,
                        edge.dir,
                        fwidth,
                        bx,
                        stride,
                        &mut self.row_min[local_row],
                        &mut self.row_max[local_row],
                    );
                }
            }
        }

        // --- Pass 2: prefix-sum each row into coverage, emit, reset ---
        let mut row = row;
        for local_row in 0..bh {
            let (min, max) = (self.row_min[local_row], self.row_max[local_row]);
            if min > max {
                continue;
            }
            let (min, max) = (min as usize, max as usize);
            let base = local_row * stride;
            let accumulator = &mut self.acc[base..base + stride];
            let mut first = width;
            let mut last = 0usize;
            let mut running = 0.0f32;
            for (offset, cell) in accumulator[min..=max].iter_mut().enumerate() {
                running += *cell;
                *cell = 0.0;
                let byte = coverage_byte(running, rule);
                let x = bx + min + offset;
                if x >= width {
                    break;
                }
                self.cov[x] = byte;
                if byte != 0 {
                    first = first.min(x);
                    last = x;
                }
            }
            accumulator[max + 1] = 0.0;

            if first <= last {
                row(row_start + local_row, first, last, &mut self.cov);
                for byte in &mut self.cov[first..=last] {
                    *byte = 0;
                }
            }
        }
    }

    /// The straightforward version: for each row, clip every edge to
    /// `[y, y+1)` and accumulate into an O(width) buffer.
    ///
    /// O(edges × rows) rather than O(edges + touched cells), so it is not what
    /// runs in production — but it is obviously correct, and the fast path is
    /// tested against it.
    pub fn fill_reference(
        &mut self,
        points: &[[f32; 2]],
        subpaths: &[(usize, usize)],
        width: usize,
        height: usize,
        rule: FillRule,
        row: impl FnMut(usize, usize, usize, &mut [u8]),
    ) {
        if width == 0 || height == 0 {
            return;
        }
        if self.cov.len() < width {
            self.cov.resize(width, 0);
        }
        if self.acc_row.len() < width + 2 {
            self.acc_row.resize(width + 2, 0.0);
        }

        let Some((y_min, y_max, _, _)) = self.build_edges(points, subpaths) else {
            return;
        };
        let row_start = y_min.floor().max(0.0) as usize;
        let row_end = y_max.ceil().min(height as f32).max(0.0) as usize;
        let fwidth = width as f32;

        let mut row = row;
        for y in row_start..row_end {
            let mut min = width;
            let mut max = 0usize;
            for edge in &self.edges {
                let top = edge.y_top.max(y as f32);
                let bottom = edge.y_bot.min(y as f32 + 1.0);
                if bottom <= top {
                    continue;
                }
                let xa = edge.x_at_top + (top - edge.y_top) * edge.dxdy;
                let xb = edge.x_at_top + (bottom - edge.y_top) * edge.dxdy;
                deposit_edge_row(
                    &mut self.acc_row,
                    0,
                    xa,
                    top,
                    xb,
                    bottom,
                    edge.dir,
                    fwidth,
                    0,
                    width + 2,
                    &mut (min as u32),
                    &mut (max as u32),
                );
                // The bbox-local helper tracks columns for the fast path; here
                // the whole row is swept, so the range is recomputed simply.
                let (lo, hi) = (xa.min(xb), xa.max(xb));
                min = min.min(lo.floor().clamp(0.0, fwidth - 1.0) as usize);
                max = max.max(hi.ceil().clamp(0.0, fwidth - 1.0) as usize);
            }
            if min > max {
                continue;
            }

            let mut running = 0.0f32;
            let mut first = width;
            let mut last = 0usize;
            for column in min..=max {
                running += self.acc_row[column];
                self.acc_row[column] = 0.0;
                let byte = coverage_byte(running, rule);
                self.cov[column] = byte;
                if byte != 0 {
                    first = first.min(column);
                    last = column;
                }
            }
            self.acc_row[max + 1] = 0.0;

            if first <= last {
                row(y, first, last, &mut self.cov);
                for byte in &mut self.cov[first..=last] {
                    *byte = 0;
                }
            }
        }
    }

    /// Build the monotonic edge list and return `(y_min, y_max, x_min, x_max)`.
    fn build_edges(
        &mut self,
        points: &[[f32; 2]],
        subpaths: &[(usize, usize)],
    ) -> Option<(f32, f32, f32, f32)> {
        self.edges.clear();
        let mut y_min = f32::INFINITY;
        let mut y_max = f32::NEG_INFINITY;
        let mut x_min = f32::INFINITY;
        let mut x_max = f32::NEG_INFINITY;

        for &(start, end) in subpaths {
            let count = end.saturating_sub(start);
            if count < 2 {
                continue;
            }
            for i in 0..count {
                let a = points[start + i];
                // Implicitly closed: the last vertex joins the first.
                let b = points[start + (i + 1) % count];
                // A horizontal edge crosses no scanline and contributes no
                // winding; including it would divide by zero.
                if a[1] == b[1] {
                    continue;
                }
                let (y_top, y_bot, x_at_top, dir) = if a[1] < b[1] {
                    (a[1], b[1], a[0], 1.0)
                } else {
                    (b[1], a[1], b[0], -1.0)
                };
                let dxdy = (b[0] - a[0]) / (b[1] - a[1]);
                self.edges.push(Edge {
                    y_top,
                    y_bot,
                    x_at_top,
                    dxdy,
                    dir,
                });
                y_min = y_min.min(y_top);
                y_max = y_max.max(y_bot);
                let x_at_bot = x_at_top + dxdy * (y_bot - y_top);
                x_min = x_min.min(x_at_top.min(x_at_bot));
                x_max = x_max.max(x_at_top.max(x_at_bot));
            }
        }

        (!self.edges.is_empty()).then_some((y_min, y_max, x_min, x_max))
    }
}

#[inline]
fn note_column(column: u32, min: &mut u32, max: &mut u32) {
    if column < *min {
        *min = column;
    }
    if column > *max {
        *max = column;
    }
}

/// Distribute one edge's crossing of one scanline across the columns it spans.
#[allow(clippy::too_many_arguments)]
fn deposit_edge_row(
    acc: &mut [f32],
    base: usize,
    xa: f32,
    ya: f32,
    xb: f32,
    yb: f32,
    dir: f32,
    width: f32,
    bx: usize,
    stride: usize,
    col_min: &mut u32,
    col_max: &mut u32,
) {
    let xa = xa.clamp(0.0, width);
    let xb = xb.clamp(0.0, width);
    let cover_total = (yb - ya) * dir;
    let (x0, x1) = if xa <= xb { (xa, xb) } else { (xb, xa) };
    let span = x1 - x0;

    // A near-vertical crossing lands in one column; splitting it would divide
    // by a span of zero.
    if span < 1e-9 {
        deposit_column(
            acc,
            base,
            x0.floor(),
            cover_total,
            x0,
            width,
            bx,
            stride,
            col_min,
            col_max,
        );
        return;
    }

    let cover_per_x = cover_total / span;
    let mut column = x0.floor();
    while column < x1 - 1e-9 {
        let lo = column.max(x0);
        let hi = (column + 1.0).min(x1);
        if hi > lo {
            deposit_column(
                acc,
                base,
                column,
                cover_per_x * (hi - lo),
                0.5 * (lo + hi),
                width,
                bx,
                stride,
                col_min,
                col_max,
            );
        }
        column += 1.0;
    }
}

/// Deposit into one column, splitting the area between it and its neighbour by
/// where the crossing's midpoint fell.
#[allow(clippy::too_many_arguments)]
#[inline]
fn deposit_column(
    acc: &mut [f32],
    base: usize,
    column: f32,
    cover: f32,
    x_mid: f32,
    width: f32,
    bx: usize,
    stride: usize,
    col_min: &mut u32,
    col_max: &mut u32,
) {
    let column = column.clamp(0.0, width - 1.0) as usize;
    let here = cover * ((column as f32 + 1.0) - x_mid);
    let local = column.saturating_sub(bx).min(stride - 2);
    acc[base + local] += here;
    acc[base + local + 1] += cover - here;
    note_column(local as u32, col_min, col_max);
}

#[inline]
fn coverage_byte(running: f32, rule: FillRule) -> u8 {
    let coverage = match rule {
        FillRule::NonZero => running.abs().min(1.0),
        FillRule::EvenOdd => {
            let m = running - 2.0 * (running * 0.5).floor();
            if m > 1.0 {
                2.0 - m
            } else {
                m
            }
        }
    };
    (coverage * 255.0 + 0.5) as u8
}

/// A rasterized shape as a dense coverage bitmap.
///
/// One byte per pixel, 0 = uncovered, 255 = fully covered. Two consumers use
/// it unchanged: the GUI blends it into a 32-bit pixel buffer, and the thermal
/// receipt path thresholds it to one bit. That is the whole reason it is a
/// coverage map rather than a colour: colour is the caller's business.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageBitmap {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl CoverageBitmap {
    pub fn new(width: usize, height: usize) -> CoverageBitmap {
        CoverageBitmap {
            width,
            height,
            pixels: vec![0; width * height],
        }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.pixels[y * self.width + x]
    }

    /// Merge coverage in, keeping the greater value.
    ///
    /// `max` rather than adding: two overlapping contours of the same glyph —
    /// a tone mark whose bounding box overlaps its base — must not produce a
    /// doubly-dark seam where they meet.
    #[inline]
    pub fn blend_max(&mut self, x: usize, y: usize, coverage: u8) {
        if x >= self.width || y >= self.height {
            return;
        }
        let slot = &mut self.pixels[y * self.width + x];
        *slot = (*slot).max(coverage);
    }

    /// Merge a horizontal run of coverage in, keeping the greater value.
    ///
    /// The span form of [`CoverageBitmap::blend_max`], and what the fill
    /// callback actually wants: a glyph arrives as rows, not as pixels, and
    /// merging a row at a time replaces two bounds checks per pixel with two
    /// per row. Anything reaching past the right edge or below the last row is
    /// cut rather than refused — an outline that overshoots its bounding box
    /// by a fraction of a pixel is a rounding artifact, not a reason to panic
    /// halfway through a receipt.
    #[inline]
    pub fn blend_max_row(&mut self, x: usize, y: usize, coverage: &[u8]) {
        if y >= self.height || x >= self.width {
            return;
        }
        let take = (self.width - x).min(coverage.len());
        let start = y * self.width + x;
        crate::blend::max_span(&mut self.pixels[start..start + take], &coverage[..take]);
    }

    pub fn is_blank(&self) -> bool {
        crate::blend::all_zero(&self.pixels)
    }

    /// Threshold to one bit per pixel, MSB first, rows padded to a byte
    /// boundary — the layout an ESC/POS raster command expects.
    ///
    /// A receipt is the longest single-purpose buffer this system builds — a
    /// 576-dot page a thousand rows tall is half a million coverage bytes,
    /// converted every time something prints — so this is done a row at a time
    /// through the vector packer rather than a pixel at a time through `get`.
    pub fn to_1bit_rows(&self, threshold: u8) -> Vec<u8> {
        let stride = self.width.div_ceil(8);
        let mut out = vec![0u8; stride * self.height];
        if stride == 0 {
            return out;
        }
        for (y, packed) in out.chunks_exact_mut(stride).enumerate() {
            let row = &self.pixels[y * self.width..(y + 1) * self.width];
            crate::blend::pack_row_1bit(row, threshold, packed);
        }
        out
    }

    /// Binary PGM, for golden-image tests and for eyeballing a rendering.
    pub fn to_pgm(&self) -> Vec<u8> {
        let mut out = format!("P5\n{} {}\n255\n", self.width, self.height).into_bytes();
        out.extend_from_slice(&self.pixels);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rasterize into a dense bitmap using the fast path.
    fn rasterize(
        points: &[[f32; 2]],
        subpaths: &[(usize, usize)],
        width: usize,
        height: usize,
    ) -> CoverageBitmap {
        let mut kernel = RasterKernel::new();
        let mut bitmap = CoverageBitmap::new(width, height);
        kernel.fill(
            points,
            subpaths,
            width,
            height,
            FillRule::NonZero,
            |y, x0, x1, cov| {
                for (x, &coverage) in cov[x0..=x1].iter().enumerate() {
                    bitmap.blend_max(x0 + x, y, coverage);
                }
            },
        );
        bitmap
    }

    fn rasterize_reference(
        points: &[[f32; 2]],
        subpaths: &[(usize, usize)],
        width: usize,
        height: usize,
    ) -> CoverageBitmap {
        let mut kernel = RasterKernel::new();
        let mut bitmap = CoverageBitmap::new(width, height);
        kernel.fill_reference(
            points,
            subpaths,
            width,
            height,
            FillRule::NonZero,
            |y, x0, x1, cov| {
                for (x, &coverage) in cov[x0..=x1].iter().enumerate() {
                    bitmap.blend_max(x0 + x, y, coverage);
                }
            },
        );
        bitmap
    }

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    #[test]
    fn a_pixel_aligned_rectangle_is_fully_covered_and_nothing_else_is() {
        let points = rect(2.0, 2.0, 6.0, 5.0);
        let bitmap = rasterize(&points, &[(0, 4)], 10, 8);
        for y in 0..8 {
            for x in 0..10 {
                let inside = (2..6).contains(&x) && (2..5).contains(&y);
                assert_eq!(
                    bitmap.get(x, y),
                    if inside { 255 } else { 0 },
                    "pixel ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn a_half_covered_pixel_reports_half_coverage() {
        // A rectangle covering exactly the left half of column 0.
        let points = rect(0.0, 0.0, 0.5, 1.0);
        let bitmap = rasterize(&points, &[(0, 4)], 4, 2);
        assert_eq!(bitmap.get(0, 0), 128, "0.5 coverage rounds to 128");
        assert_eq!(bitmap.get(1, 0), 0);
    }

    #[test]
    fn coverage_is_exact_in_both_axes_not_supersampled() {
        // A quarter-covered pixel: half in x by half in y. Supersampling at
        // any fixed rate would quantise this; exact area does not.
        let points = rect(0.0, 0.0, 0.5, 0.5);
        let bitmap = rasterize(&points, &[(0, 4)], 4, 4);
        assert_eq!(bitmap.get(0, 0), 64, "0.25 coverage rounds to 64");
    }

    #[test]
    fn a_counter_is_cut_out_by_the_nonzero_rule() {
        // An outer square wound one way and an inner square the other: the
        // hole in ด, in miniature.
        let mut points = rect(0.0, 0.0, 8.0, 8.0);
        points.extend_from_slice(&[[2.0, 2.0], [2.0, 6.0], [6.0, 6.0], [6.0, 2.0]]);
        let bitmap = rasterize(&points, &[(0, 4), (4, 8)], 8, 8);
        assert_eq!(bitmap.get(1, 1), 255, "the ring is filled");
        assert_eq!(bitmap.get(4, 4), 0, "the counter is empty");
    }

    #[test]
    fn the_fast_path_and_the_reference_agree_byte_for_byte() {
        // A shape with diagonals, verticals and horizontals, deliberately off
        // the pixel grid so every fractional case is exercised.
        let points = vec![[1.3, 0.7], [7.9, 2.1], [6.4, 9.2], [3.0, 9.2], [0.5, 4.8]];
        let fast = rasterize(&points, &[(0, 5)], 12, 12);
        let reference = rasterize_reference(&points, &[(0, 5)], 12, 12);
        assert_eq!(fast.pixels, reference.pixels);
    }

    #[test]
    fn a_shape_outside_the_surface_draws_nothing_and_does_not_panic() {
        let points = rect(-20.0, -20.0, -10.0, -10.0);
        assert!(rasterize(&points, &[(0, 4)], 8, 8).is_blank());
        let points = rect(50.0, 50.0, 60.0, 60.0);
        assert!(rasterize(&points, &[(0, 4)], 8, 8).is_blank());
    }

    #[test]
    fn a_shape_straddling_the_edge_is_clipped_rather_than_wrapped() {
        let points = rect(-2.0, -2.0, 3.0, 3.0);
        let bitmap = rasterize(&points, &[(0, 4)], 8, 8);
        assert_eq!(bitmap.get(0, 0), 255);
        assert_eq!(bitmap.get(2, 2), 255);
        assert_eq!(bitmap.get(3, 3), 0);
        // Nothing wrapped around to the far side.
        assert_eq!(bitmap.get(7, 7), 0);
    }

    #[test]
    fn degenerate_input_is_ignored_rather_than_drawn() {
        // Fewer than two points, and a purely horizontal "shape" with no area.
        assert!(rasterize(&[[1.0, 1.0]], &[(0, 1)], 8, 8).is_blank());
        let flat = vec![[1.0, 1.0], [5.0, 1.0], [3.0, 1.0]];
        assert!(rasterize(&flat, &[(0, 3)], 8, 8).is_blank());
    }

    #[test]
    fn the_kernel_is_reusable_without_leaking_coverage_between_fills() {
        let mut kernel = RasterKernel::new();
        let mut first = CoverageBitmap::new(8, 8);
        let points = rect(0.0, 0.0, 4.0, 4.0);
        kernel.fill(
            &points,
            &[(0, 4)],
            8,
            8,
            FillRule::NonZero,
            |y, x0, x1, cov| {
                for (x, &c) in cov[x0..=x1].iter().enumerate() {
                    first.blend_max(x0 + x, y, c);
                }
            },
        );

        let mut second = CoverageBitmap::new(8, 8);
        let elsewhere = rect(5.0, 5.0, 7.0, 7.0);
        kernel.fill(
            &elsewhere,
            &[(0, 4)],
            8,
            8,
            FillRule::NonZero,
            |y, x0, x1, cov| {
                for (x, &c) in cov[x0..=x1].iter().enumerate() {
                    second.blend_max(x0 + x, y, c);
                }
            },
        );
        assert_eq!(second.get(0, 0), 0, "the first fill leaked into the second");
        assert_eq!(second.get(5, 5), 255);
    }

    #[test]
    fn even_odd_differs_from_nonzero_on_same_wound_nested_contours() {
        // Both contours wound the same way: nonzero fills the middle, even-odd
        // punches it out.
        let mut points = rect(0.0, 0.0, 8.0, 8.0);
        points.extend_from_slice(&rect(2.0, 2.0, 6.0, 6.0));
        let subpaths = [(0, 4), (4, 8)];

        let mut kernel = RasterKernel::new();
        let mut nonzero = CoverageBitmap::new(8, 8);
        kernel.fill(
            &points,
            &subpaths,
            8,
            8,
            FillRule::NonZero,
            |y, x0, x1, cov| {
                for (x, &c) in cov[x0..=x1].iter().enumerate() {
                    nonzero.blend_max(x0 + x, y, c);
                }
            },
        );
        let mut even_odd = CoverageBitmap::new(8, 8);
        kernel.fill(
            &points,
            &subpaths,
            8,
            8,
            FillRule::EvenOdd,
            |y, x0, x1, cov| {
                for (x, &c) in cov[x0..=x1].iter().enumerate() {
                    even_odd.blend_max(x0 + x, y, c);
                }
            },
        );

        assert_eq!(nonzero.get(4, 4), 255);
        assert_eq!(even_odd.get(4, 4), 0);
    }

    #[test]
    fn merging_a_row_is_the_same_as_merging_its_pixels() {
        let coverage: Vec<u8> = (0..12u16).map(|column| (column * 21 % 256) as u8).collect();
        let mut by_row = CoverageBitmap::new(16, 3);
        let mut by_pixel = CoverageBitmap::new(16, 3);

        for (x, y) in [(0usize, 0usize), (4, 1), (9, 1), (15, 2)] {
            by_row.blend_max_row(x, y, &coverage);
            for (offset, &cover) in coverage.iter().enumerate() {
                by_pixel.blend_max(x + offset, y, cover);
            }
        }
        assert_eq!(by_row.pixels, by_pixel.pixels);
    }

    #[test]
    fn a_row_reaching_past_the_edge_is_cut_rather_than_wrapped_or_refused() {
        // An outline that overshoots its bounding box by a rounding step is a
        // rounding step, not a reason to panic in the middle of a receipt.
        let mut bitmap = CoverageBitmap::new(6, 2);
        bitmap.blend_max_row(4, 0, &[255; 10]);
        bitmap.blend_max_row(6, 0, &[255; 4]);
        bitmap.blend_max_row(0, 2, &[255; 4]);
        assert_eq!(bitmap.get(4, 0), 255);
        assert_eq!(bitmap.get(5, 0), 255);
        assert_eq!(bitmap.get(0, 1), 0, "coverage wrapped onto the next row");
        assert!(bitmap.pixels[6..].iter().all(|&pixel| pixel == 0));
    }

    #[test]
    fn one_bit_conversion_packs_rows_the_way_a_thermal_printer_reads_them() {
        let mut bitmap = CoverageBitmap::new(9, 2);
        bitmap.blend_max(0, 0, 255);
        bitmap.blend_max(7, 0, 255);
        bitmap.blend_max(8, 1, 200);
        let rows = bitmap.to_1bit_rows(128);
        // 9 pixels wide is two bytes per row.
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], 0b1000_0001);
        assert_eq!(rows[1], 0);
        assert_eq!(rows[2], 0);
        assert_eq!(rows[3], 0b1000_0000);
    }

    #[test]
    fn thresholding_drops_faint_antialiasing_a_printer_cannot_render() {
        let mut bitmap = CoverageBitmap::new(8, 1);
        bitmap.blend_max(0, 0, 127);
        bitmap.blend_max(1, 0, 128);
        let rows = bitmap.to_1bit_rows(128);
        assert_eq!(rows[0], 0b0100_0000);
    }

    #[test]
    fn a_pgm_header_describes_the_bitmap_that_follows() {
        let bitmap = CoverageBitmap::new(3, 2);
        let pgm = bitmap.to_pgm();
        assert!(pgm.starts_with(b"P5\n3 2\n255\n"));
        assert_eq!(pgm.len(), 11 + 6);
    }
}
