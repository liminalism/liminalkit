//! Turning a string into pixels.
//!
//! One path serves every consumer: shape a string against a [`FontSet`] and a
//! [`TextStyle`], rasterize it into a coverage bitmap with the exact-area
//! kernel, and let the caller blend that into a pixel buffer (or threshold it
//! for a 1-bit printer — the receipt path this was written for).

use crate::font::{self, Face, FaceId, FontSet, TextStyle};
use crate::layout::ShapedGlyph;
use liminalkit_raster::kernel::{CoverageBitmap, FillRule, RasterKernel};
use std::collections::BTreeMap;

use crate::sfnt::Outline;

/// A stretch of text drawn by one face.
#[derive(Debug)]
pub struct TextRun {
    pub face: &'static Face,
    pub glyphs: Vec<ShapedGlyph>,
}

impl TextRun {
    /// Advance in design units.
    fn advance_units(&self) -> f32 {
        self.glyphs.iter().map(|glyph| glyph.x_advance).sum()
    }

    /// Design units to pixels for this run's face.
    fn scale(&self, size_px: f32) -> f32 {
        size_px / self.face.units_per_em()
    }
}

/// A shaped string, ready to measure or draw.
#[derive(Debug, Default)]
pub struct ShapedText {
    pub runs: Vec<TextRun>,
    /// Letter-spacing in ems, added after every cluster (including the last).
    pub tracking: f32,
    /// Glyphs that carry their own advance (not marks): tracking is added
    /// after each of them, and after nothing else.
    spacing_slots: usize,
}

/// Shape a string with `style`'s face (and its fallbacks), splitting it into
/// runs by face and recording the style's tracking.
pub fn shape(set: &FontSet, style: TextStyle, text: &str) -> ShapedText {
    let mut shaped = shape_face(set, style.face, text);
    shaped.tracking = style.tracking;
    shaped
}

/// Shape a string with a face and its fallbacks, no tracking.
///
/// The split is per face, not per script, and runs are shaped independently.
/// That is deliberate: a Thai cluster's marks must never be separated from
/// their base by a run boundary, and since marks are always Thai and always
/// adjacent to their Thai base, a face split cannot fall inside a cluster.
pub fn shape_face(set: &FontSet, face_id: FaceId, text: &str) -> ShapedText {
    let mut runs: Vec<TextRun> = Vec::new();
    let mut current: Option<(&'static Face, usize, usize)> = None;

    for (offset, character) in text.char_indices() {
        let (face, _) = set.face_for(face_id, character);
        let end = offset + character.len_utf8();
        match &mut current {
            Some((run_face, _, run_end)) if std::ptr::eq(*run_face, face) => {
                *run_end = end;
            }
            Some((run_face, start, run_end)) => {
                runs.push(TextRun {
                    face: run_face,
                    glyphs: run_face.shape(&text[*start..*run_end], *start),
                });
                current = Some((face, offset, end));
            }
            None => current = Some((face, offset, end)),
        }
    }
    if let Some((face, start, end)) = current {
        runs.push(TextRun {
            face,
            glyphs: face.shape(&text[start..end], start),
        });
    }

    let spacing_slots = runs
        .iter()
        .map(|run| {
            run.glyphs
                .iter()
                .filter(|glyph| !run.face.is_mark(glyph.glyph))
                .count()
        })
        .sum();
    ShapedText {
        runs,
        tracking: 0.0,
        spacing_slots,
    }
}

impl ShapedText {
    pub fn with_tracking(mut self, tracking_em: f32) -> ShapedText {
        self.tracking = tracking_em;
        self
    }

    /// Width in pixels at a given size, tracking included.
    pub fn width(&self, size_px: f32) -> f32 {
        let glyphs: f32 = self
            .runs
            .iter()
            .map(|run| run.advance_units() * run.scale(size_px))
            .sum();
        glyphs + self.tracking * size_px * self.spacing_slots as f32
    }

    pub fn is_empty(&self) -> bool {
        self.runs.iter().all(|run| run.glyphs.is_empty())
    }

    /// Distance from the baseline to the top of the line box, in pixels.
    pub fn ascent(&self, size_px: f32) -> f32 {
        self.runs
            .iter()
            .map(|run| run.face.ascender() * run.scale(size_px))
            .fold(0.0, f32::max)
    }

    /// Distance from the baseline down to the bottom of the line box.
    pub fn descent(&self, size_px: f32) -> f32 {
        self.runs
            .iter()
            .map(|run| -run.face.descender() * run.scale(size_px))
            .fold(0.0, f32::max)
    }

    /// Where to cut so that what remains fits `max_width_px`.
    ///
    /// Returns a byte offset into the string that was shaped, or `None` if all
    /// of it fits. The cut is always on a cluster boundary, so a tone mark is
    /// never separated from the consonant it belongs to — the same rule
    /// [`wrap`] follows, enforced here by construction rather than by walking
    /// backwards afterwards, because a cluster is exactly what a `ShapedGlyph`
    /// already records.
    ///
    /// One shaped result answers this for any width. Measuring successive
    /// prefixes instead means reshaping the string once per character, and
    /// assumes shaping a prefix gives the same advances as shaping the whole —
    /// which contextual substitution does not owe anyone.
    pub fn fit(&self, max_width_px: f32, size_px: f32) -> Option<usize> {
        // Summed per cluster: two glyphs from one character (SARA AM) must be
        // kept or dropped together, and a mark has no advance of its own to
        // account for separately.
        let tracking = self.tracking * size_px;
        let mut clusters: BTreeMap<usize, f32> = BTreeMap::new();
        for run in &self.runs {
            let scale = run.scale(size_px);
            for glyph in &run.glyphs {
                let spacing = if run.face.is_mark(glyph.glyph) {
                    0.0
                } else {
                    tracking
                };
                *clusters.entry(glyph.cluster).or_insert(0.0) += glyph.x_advance * scale + spacing;
            }
        }

        let mut used = 0.0f32;
        for (offset, advance) in clusters {
            if used + advance > max_width_px {
                return Some(offset);
            }
            used += advance;
        }
        None
    }
}

/// The height of a rendered line at a pixel size, from the fonts' metrics.
///
/// The maximum over the face and its fallbacks, so a row is the same height
/// whichever of them a string happens to use — a column that shifted by a
/// pixel depending on its content would be worse than one that is uniformly
/// a little generous.
///
/// This is a bound on what [`render`] produces, not typographic leading, so it
/// includes the rasterizer's [`PADDING`] on both sides. `render_shaped` starts
/// its ink box at the face's nominal ascent and descent and only grows it, so
/// the nominal height plus the padding covers every string that does not
/// overshoot its own metrics.
pub fn line_height(set: &FontSet, face: FaceId, size_px: f32) -> f32 {
    set.family(face)
        .map(|face| {
            let scale = size_px / face.units_per_em();
            // The same two roundings `render_shaped` performs, in the same
            // order. Adding the padding and rounding once gives a number up to
            // a pixel short of what the rasterizer actually produces, and a
            // row a pixel short of its glyphs is exactly the bug this is meant
            // to rule out.
            let baseline = (PADDING + face.ascender() * scale).ceil();
            (baseline - face.descender() * scale + PADDING).ceil()
        })
        .fold(0.0, f32::max)
}

/// A rasterized line of text.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedText {
    pub bitmap: CoverageBitmap,
    /// Where the pen started, in bitmap coordinates. Usually 0, but negative
    /// left side bearings — which Thai marks have in abundance — can push ink
    /// left of the origin, and the bitmap grows to hold it.
    pub origin_x: i32,
    /// The baseline's row in the bitmap.
    pub baseline_y: i32,
    /// How far the pen moved: what to add before drawing the next thing.
    pub advance: f32,
}

/// One pixel of margin so anti-aliased edges are never clipped by the bitmap's
/// own boundary.
const PADDING: f32 = 1.0;

/// Render a string with a style.
pub fn render(set: &FontSet, style: TextStyle, text: &str) -> RenderedText {
    render_shaped(&shape(set, style, text), style.size)
}

/// Render an already-shaped string, so a caller that measured first does not
/// pay to shape twice.
pub fn render_shaped(shaped: &ShapedText, size_px: f32) -> RenderedText {
    let advance = shaped.width(size_px);
    let tracking = shaped.tracking * size_px;
    let ascent = shaped.ascent(size_px);
    let descent = shaped.descent(size_px);

    // Ink bounds in device space, relative to a pen at x = 0 and the baseline
    // at y = 0, with y increasing downward.
    let mut left = 0.0f32;
    let mut right = advance;
    let mut top = -ascent;
    let mut bottom = descent;

    let mut pen = 0.0f32;
    for run in &shaped.runs {
        let scale = run.scale(size_px);
        for glyph in &run.glyphs {
            if let Some(bounds) = run
                .face
                .outline(glyph.glyph)
                .and_then(|outline| outline.bounds())
            {
                let x = pen + glyph.x_offset * scale;
                let y = -glyph.y_offset * scale;
                left = left.min(x + bounds[0] * scale);
                right = right.max(x + bounds[2] * scale);
                // Design-space y is upward; device y is downward.
                top = top.min(y - bounds[3] * scale);
                bottom = bottom.max(y - bounds[1] * scale);
            }
            pen += glyph.x_advance * scale;
            if !run.face.is_mark(glyph.glyph) {
                pen += tracking;
            }
        }
    }

    let origin_x = (PADDING - left).ceil();
    let baseline_y = (PADDING - top).ceil();
    let width = (origin_x + right + PADDING).ceil().max(1.0) as usize;
    let height = (baseline_y + bottom + PADDING).ceil().max(1.0) as usize;

    let mut bitmap = CoverageBitmap::new(width, height);
    let mut kernel = RasterKernel::new();
    let mut points: Vec<[f32; 2]> = Vec::new();
    let mut subpaths: Vec<(usize, usize)> = Vec::new();

    let mut pen = origin_x;
    for run in &shaped.runs {
        let scale = run.scale(size_px);
        for glyph in &run.glyphs {
            let x = pen + glyph.x_offset * scale;
            let y = baseline_y - glyph.y_offset * scale;
            if let Some(outline) = run.face.outline(glyph.glyph) {
                points.clear();
                subpaths.clear();
                flatten(&outline, scale, x, y, &mut points, &mut subpaths);
                if !subpaths.is_empty() {
                    kernel.fill(
                        &points,
                        &subpaths,
                        width,
                        height,
                        FillRule::NonZero,
                        |row, x0, x1, coverage| {
                            bitmap.blend_max_row(x0, row, &coverage[x0..=x1]);
                        },
                    );
                }
            }
            pen += glyph.x_advance * scale;
            if !run.face.is_mark(glyph.glyph) {
                pen += tracking;
            }
        }
    }

    RenderedText {
        bitmap,
        origin_x: origin_x as i32,
        baseline_y: baseline_y as i32,
        advance,
    }
}

/// Flatten a glyph outline into device-space polylines.
///
/// TrueType contours alternate on- and off-curve points, and two consecutive
/// off-curve points imply an on-curve point halfway between them — a
/// compression trick that saves a point per curve and costs this
/// reconstruction.
fn flatten(
    outline: &Outline,
    scale: f32,
    origin_x: f32,
    origin_y: f32,
    points: &mut Vec<[f32; 2]>,
    subpaths: &mut Vec<(usize, usize)>,
) {
    // Design space is y-up, device space is y-down.
    let map = |x: f32, y: f32| [origin_x + x * scale, origin_y - y * scale];

    for contour in &outline.contours {
        if contour.len() < 2 {
            continue;
        }
        let start = points.len();

        // Find a point to start from. If every point is off-curve — a circle
        // drawn entirely in curves, which Thai's rings and loops really are —
        // the start is the midpoint of the last and first.
        let first_on = contour.iter().position(|point| point.on_curve);
        let (start_point, begin) = match first_on {
            Some(index) => ((contour[index].x, contour[index].y), index),
            None => {
                let last = contour[contour.len() - 1];
                let first = contour[0];
                (((last.x + first.x) / 2.0, (last.y + first.y) / 2.0), 0)
            }
        };

        points.push(map(start_point.0, start_point.1));
        let mut current = start_point;

        let count = contour.len();
        let mut index = 0;
        while index < count {
            let point = contour[(begin + index + 1) % count];
            if point.on_curve {
                points.push(map(point.x, point.y));
                current = (point.x, point.y);
                index += 1;
                continue;
            }

            // An off-curve point is a quadratic control. Its end is the next
            // on-curve point, or the implied midpoint before another control.
            let next = contour[(begin + index + 2) % count];
            let end = if next.on_curve {
                index += 2;
                (next.x, next.y)
            } else {
                index += 1;
                ((point.x + next.x) / 2.0, (point.y + next.y) / 2.0)
            };
            push_quadratic(
                points,
                map(current.0, current.1),
                map(point.x, point.y),
                map(end.0, end.1),
            );
            current = end;
        }

        if points.len() - start >= 3 {
            subpaths.push((start, points.len()));
        } else {
            points.truncate(start);
        }
    }
}

/// The most a quadratic is ever split into. At receipt and screen sizes a
/// glyph curve spans a few pixels; beyond this the segments are shorter than
/// the anti-aliasing can express.
const MAX_CURVE_SEGMENTS: usize = 24;

fn push_quadratic(points: &mut Vec<[f32; 2]>, from: [f32; 2], control: [f32; 2], to: [f32; 2]) {
    // Segment count from the control polygon's length in pixels: a curve two
    // pixels long needs one segment, a large display glyph needs many.
    let length = distance(from, control) + distance(control, to);
    let segments = ((length / 3.0).ceil() as usize).clamp(1, MAX_CURVE_SEGMENTS);
    for step in 1..=segments {
        let t = step as f32 / segments as f32;
        let inverse = 1.0 - t;
        points.push([
            inverse * inverse * from[0] + 2.0 * inverse * t * control[0] + t * t * to[0],
            inverse * inverse * from[1] + 2.0 * inverse * t * control[1] + t * t * to[1],
        ]);
    }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Break text into lines that fit `max_width_px`.
///
/// Thai is written without spaces between words, so proper line breaking needs
/// a dictionary. This is the honest fallback until one exists: break anywhere,
/// with two rules that keep the result readable rather than wrong.
///
/// 1. **Never break before a combining mark.** A tone mark separated from its
///    consonant lands on whatever starts the next line, which is not a
///    typographic blemish — it changes the word.
/// 2. **Prefer a space if the line has one.** Thai text mixed with numbers,
///    English or punctuation usually does, and a space is always a legitimate
///    break.
///
/// Returns byte ranges into `text`.
pub fn wrap(
    set: &FontSet,
    style: TextStyle,
    text: &str,
    max_width_px: f32,
) -> Vec<std::ops::Range<usize>> {
    if text.is_empty() {
        // One empty line, not zero lines: an empty string still occupies a
        // row on a receipt.
        return vec![std::ops::Range { start: 0, end: 0 }];
    }
    // Shape once and take per-cluster advances from that, exactly as `fit`
    // does. Measuring every prefix by reshaping is quadratic — a paragraph of
    // forty words was costing a millisecond a frame per paragraph.
    let shaped = shape(set, style, text);
    let size_px = style.size;
    let tracking = shaped.tracking * size_px;
    let mut clusters: BTreeMap<usize, f32> = BTreeMap::new();
    for run in &shaped.runs {
        let scale = run.scale(size_px);
        for glyph in &run.glyphs {
            let spacing = if run.face.is_mark(glyph.glyph) {
                0.0
            } else {
                tracking
            };
            *clusters.entry(glyph.cluster).or_insert(0.0) += glyph.x_advance * scale + spacing;
        }
    }
    let clusters: Vec<(usize, f32)> = clusters.into_iter().collect();

    let mut lines = Vec::new();
    let mut line_start = 0usize;
    let mut used = 0.0f32;
    let mut last_space: Option<usize> = None;
    let mut index = 0usize;
    while index < clusters.len() {
        let (offset, advance) = clusters[index];
        let character = text[offset..].chars().next().unwrap_or(' ');
        if character == ' ' {
            last_space = Some(offset);
        }
        if used + advance > max_width_px && offset > line_start {
            let mut split = match last_space {
                Some(space) if space > line_start => space + 1,
                _ => offset,
            };
            // Rule 1: walk back off any combining marks.
            while split > line_start
                && text[split..]
                    .chars()
                    .next()
                    .is_some_and(font::is_thai_combining)
            {
                split -= text[..split]
                    .chars()
                    .next_back()
                    .map(char::len_utf8)
                    .unwrap_or(1);
            }
            if split <= line_start {
                split = offset;
            }
            lines.push(line_start..split);
            line_start = split;
            last_space = None;
            // Re-measure from the new start.
            index = clusters
                .iter()
                .position(|(o, _)| *o >= split)
                .unwrap_or(clusters.len());
            used = 0.0;
            continue;
        }
        used += advance;
        index += 1;
    }

    if line_start < text.len() || lines.is_empty() {
        lines.push(line_start..text.len());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: f32 = 24.0;
    #[allow(unused_imports)]
    use crate::font::test_fonts::{latin, set, style, thai};

    fn shape(text: &str) -> ShapedText {
        super::shape(set(), style(SIZE), text)
    }
    fn render(text: &str, size: f32) -> RenderedText {
        super::render(set(), style(size), text)
    }
    #[allow(dead_code)]
    fn wrap(text: &str, width: f32, size: f32) -> Vec<std::ops::Range<usize>> {
        super::wrap(set(), style(size), text, width)
    }
    #[allow(dead_code)]
    fn line_height(size: f32) -> f32 {
        super::line_height(set(), style(size).face, size)
    }

    fn ink_columns(rendered: &RenderedText) -> Vec<usize> {
        (0..rendered.bitmap.width)
            .filter(|&x| (0..rendered.bitmap.height).any(|y| rendered.bitmap.get(x, y) > 0))
            .collect()
    }

    #[test]
    fn a_thai_string_is_one_run() {
        let shaped = shape("ผัดกะเพรา");
        assert_eq!(shaped.runs.len(), 1);
        assert!(std::ptr::eq(shaped.runs[0].face, thai()));
    }

    #[test]
    fn a_price_splits_into_the_two_faces_it_needs() {
        // ฿ is Thai-block; the digits are not. This is the ordinary case.
        let shaped = shape("฿1,240.50");
        assert_eq!(shaped.runs.len(), 2);
        assert!(std::ptr::eq(shaped.runs[0].face, thai()));
        assert!(std::ptr::eq(shaped.runs[1].face, latin()));
        assert_eq!(shaped.runs[0].glyphs.len(), 1);
        assert_eq!(shaped.runs[1].glyphs.len(), 8);
    }

    #[test]
    fn a_mixed_line_alternates_runs_without_losing_characters() {
        let shaped = shape("ผัดกะเพรา ฿70");
        let total: usize = shaped.runs.iter().map(|run| run.glyphs.len()).sum();
        assert!(total >= 12);
        // Clusters index the original string, across run boundaries.
        let clusters: Vec<usize> = shaped
            .runs
            .iter()
            .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.cluster))
            .collect();
        assert!(clusters.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(clusters[0], 0);
    }

    #[test]
    fn width_grows_with_the_text_and_with_the_size() {
        let short = shape("ก").width(SIZE);
        let long = shape("กกกก").width(SIZE);
        assert!(long > short);
        assert!((long - short * 4.0).abs() < 0.01, "advances should add up");
        assert!(shape("ก").width(SIZE * 2.0) > short * 1.9);
    }

    #[test]
    fn combining_marks_add_no_width() {
        // The visible consequence of marks having zero advance.
        assert_eq!(shape("ก").width(SIZE), shape("ก่").width(SIZE));
        assert_eq!(shape("ก").width(SIZE), shape("กี่").width(SIZE));
    }

    #[test]
    fn rendering_produces_ink_where_the_text_is() {
        let rendered = render("ผัดกะเพรา", SIZE);
        assert!(!rendered.bitmap.is_blank());
        assert!(rendered.bitmap.width > 0 && rendered.bitmap.height > 0);
        assert!(rendered.advance > 0.0);
        // The bitmap is at least as wide as the advance it reports.
        assert!(rendered.bitmap.width as f32 >= rendered.advance);
    }

    #[test]
    fn the_baseline_sits_below_the_bulk_of_the_ink() {
        let rendered = render("ผัดกะเพรา", SIZE);
        let baseline = rendered.baseline_y as usize;
        let above: u32 = (0..baseline)
            .flat_map(|y| (0..rendered.bitmap.width).map(move |x| (x, y)))
            .map(|(x, y)| u32::from(rendered.bitmap.get(x, y)))
            .sum();
        let below: u32 = (baseline..rendered.bitmap.height)
            .flat_map(|y| (0..rendered.bitmap.width).map(move |x| (x, y)))
            .map(|(x, y)| u32::from(rendered.bitmap.get(x, y)))
            .sum();
        assert!(above > below, "most ink should be above the baseline");
    }

    #[test]
    fn a_tone_mark_adds_ink_above_without_widening_the_line() {
        let plain = render("ก", SIZE);
        let marked = render("ก่", SIZE);
        assert_eq!(plain.advance, marked.advance, "a mark adds no advance");

        let plain_ink: u32 = plain.bitmap.pixels.iter().map(|&p| u32::from(p)).sum();
        let marked_ink: u32 = marked.bitmap.pixels.iter().map(|&p| u32::from(p)).sum();
        assert!(marked_ink > plain_ink, "the mark should add ink");

        // And that ink is higher than the consonant's own top.
        let plain_top = (0..plain.bitmap.height)
            .find(|&y| (0..plain.bitmap.width).any(|x| plain.bitmap.get(x, y) > 0))
            .unwrap();
        let marked_top = (0..marked.bitmap.height)
            .find(|&y| (0..marked.bitmap.width).any(|x| marked.bitmap.get(x, y) > 0))
            .unwrap();
        // Both are measured from their own baselines, which may differ by a
        // pixel; compare relative to the baseline instead.
        let plain_height = plain.baseline_y as usize - plain_top;
        let marked_height = marked.baseline_y as usize - marked_top;
        assert!(
            marked_height > plain_height,
            "the marked form should be taller: {marked_height} vs {plain_height}"
        );
    }

    #[test]
    fn rendering_an_empty_string_is_a_blank_bitmap_not_a_panic() {
        let rendered = render("", SIZE);
        assert!(rendered.bitmap.is_blank());
        assert_eq!(rendered.advance, 0.0);
    }

    #[test]
    fn a_space_leaves_a_gap_between_ink() {
        let rendered = render("ก ก", SIZE);
        let columns = ink_columns(&rendered);
        // A gap exists somewhere in the middle.
        let gaps = columns
            .windows(2)
            .filter(|pair| pair[1] - pair[0] > 1)
            .count();
        assert!(gaps >= 1, "columns: {columns:?}");
    }

    #[test]
    fn the_same_text_renders_identically_every_time() {
        // Determinism matters more here than usual: a golden test and a
        // reprinted receipt both depend on it.
        assert_eq!(render("ผัดกะเพรา ฿70", SIZE), render("ผัดกะเพรา ฿70", SIZE));
    }

    #[test]
    fn a_receipt_line_thresholds_to_one_bit_without_losing_its_shape() {
        let rendered = render("ผัดกะเพรา", 24.0);
        let rows = rendered.bitmap.to_1bit_rows(128);
        assert!(
            rows.iter().any(|&byte| byte != 0),
            "the raster is not blank"
        );
        assert_eq!(
            rows.len(),
            rendered.bitmap.width.div_ceil(8) * rendered.bitmap.height
        );
    }

    #[test]
    fn tiny_and_large_sizes_both_produce_ink() {
        // 8 px is about the smallest a thermal printer resolves; 96 px is a
        // change-due display.
        for size in [8.0, 12.0, 24.0, 48.0, 96.0] {
            let rendered = render("ก฿9", size);
            assert!(!rendered.bitmap.is_blank(), "blank at {size} px");
        }
    }

    #[test]
    fn wrapping_returns_the_whole_string_when_it_fits() {
        let text = "ผัดกะเพรา";
        let width = shape(text).width(SIZE) + 10.0;
        assert_eq!(wrap(text, width, SIZE), vec![0..text.len()]);
    }

    #[test]
    fn wrapping_splits_a_long_line_and_loses_nothing() {
        let text = "ผัดกะเพราหมูสับไข่ดาวเผ็ดมากพิเศษ";
        let lines = wrap(text, 80.0, SIZE);
        assert!(lines.len() > 1);
        // Contiguous and complete: every byte lands on exactly one line.
        assert_eq!(lines[0].start, 0);
        assert_eq!(lines.last().unwrap().end, text.len());
        for pair in lines.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        for line in &lines {
            assert!(text.get(line.clone()).is_some(), "split mid-character");
        }
    }

    #[test]
    fn no_line_ever_begins_with_a_combining_mark() {
        // The rule that makes break-anywhere acceptable: a tone mark that
        // starts a line has landed on the wrong consonant, which changes the
        // word rather than merely looking untidy.
        let text = "ผัดกะเพราหมูสับไข่ดาวเผ็ดมากพิเศษน้ำปลาพริก";
        for width in [40.0, 60.0, 80.0, 120.0, 200.0] {
            for line in wrap(text, width, SIZE) {
                let Some(first) = text[line.clone()].chars().next() else {
                    continue;
                };
                assert!(
                    !font::is_thai_combining(first),
                    "line at width {width} starts with the combining mark {first}"
                );
            }
        }
    }

    #[test]
    fn wrapping_prefers_a_space_when_the_line_has_one() {
        let text = "ผัดกะเพรา ข้าวผัด";
        let lines = wrap(text, shape("ผัดกะเพรา ").width(SIZE) + 4.0, SIZE);
        assert!(lines.len() >= 2);
        // The first line ends at the space rather than mid-word.
        assert!(text[lines[0].clone()].ends_with(' ') || text[lines[0].clone()] == *"ผัดกะเพรา");
    }

    #[test]
    fn wrapping_an_empty_string_yields_one_empty_line() {
        assert_eq!(wrap("", 100.0, SIZE), vec![0..0]);
    }

    #[test]
    fn a_single_character_wider_than_the_line_still_gets_a_line() {
        // Degenerate but reachable: a narrow column on a receipt.
        let lines = wrap("ผัดกะเพรา", 1.0, SIZE);
        assert!(!lines.is_empty());
        assert_eq!(lines.last().unwrap().end, "ผัดกะเพรา".len());
    }
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    const SIZE: f32 = 24.0;
    #[allow(unused_imports)]
    use crate::font::test_fonts::{latin, set, style, thai};

    fn shape(text: &str) -> ShapedText {
        super::shape(set(), style(SIZE), text)
    }
    fn render(text: &str, size: f32) -> RenderedText {
        super::render(set(), style(size), text)
    }
    #[allow(dead_code)]
    fn wrap(text: &str, width: f32, size: f32) -> Vec<std::ops::Range<usize>> {
        super::wrap(set(), style(size), text, width)
    }
    #[allow(dead_code)]
    fn line_height(size: f32) -> f32 {
        super::line_height(set(), style(size).face, size)
    }

    #[test]
    fn everything_that_fits_reports_that_it_fits() {
        let text = "ข้าวสาร";
        let shaped = shape(text);
        let width = shaped.width(SIZE);
        assert_eq!(shaped.fit(width, SIZE), None);
        assert_eq!(shaped.fit(width + 100.0, SIZE), None);
    }

    #[test]
    fn a_cut_lands_on_a_cluster_boundary_at_every_width() {
        // The rule this exists to enforce, checked exhaustively rather than
        // at a few chosen widths.
        let text = "กิน้ำพริกเผาเผ็ดมากจริงๆนะ";
        let shaped = shape(text);
        let full = shaped.width(SIZE);

        let mut widths = 0;
        let mut width = 0.0;
        while width <= full {
            if let Some(cut) = shaped.fit(width, SIZE) {
                widths += 1;
                assert!(text.is_char_boundary(cut), "{cut} splits a character");
                assert!(
                    !text[cut..]
                        .chars()
                        .next()
                        .is_some_and(font::is_thai_combining),
                    "width {width} cut at {cut}, before a combining mark"
                );
                // And what is kept really does fit.
                assert!(
                    shape(&text[..cut]).width(SIZE) <= width + 0.01,
                    "width {width} kept {:?}, which is wider",
                    &text[..cut]
                );
            }
            width += 1.0;
        }
        assert!(widths > 20, "only {widths} widths actually truncated");
    }

    #[test]
    fn sara_am_is_kept_or_dropped_whole() {
        // U+0E33 shapes into two glyphs sharing one cluster. Cutting between
        // them would leave a nikhahit with no sara aa.
        let text = "น้ำ";
        let shaped = shape(text);
        let full = shaped.width(SIZE);
        let mut width = 0.0;
        while width <= full {
            if let Some(cut) = shaped.fit(width, SIZE) {
                assert!(
                    cut == 0 || cut == text.len() || text.is_char_boundary(cut),
                    "cut {cut} fell inside a cluster"
                );
                assert!(!text[..cut].ends_with('\u{0E33}') || text[..cut].chars().count() > 1);
            }
            width += 0.5;
        }
    }

    #[test]
    fn nothing_fits_in_no_width() {
        let shaped = shape("ข้าวสาร");
        assert_eq!(shaped.fit(0.0, SIZE), Some(0));
        assert_eq!(shaped.fit(-5.0, SIZE), Some(0));
    }

    #[test]
    fn an_empty_string_always_fits() {
        assert_eq!(shape("").fit(0.0, SIZE), None);
    }

    #[test]
    fn a_mixed_run_cuts_correctly_across_the_face_boundary() {
        // Thai and Latin are shaped as separate runs with their own scales.
        // Cluster offsets are absolute, so the cut must still be monotonic.
        let text = "ข้าวสาร 1,240.50 บาท";
        let shaped = shape(text);
        let full = shaped.width(SIZE);
        let mut previous = 0;
        let mut width = 0.0;
        while width <= full {
            let cut = shaped.fit(width, SIZE).unwrap_or(text.len());
            assert!(cut >= previous, "the cut moved backwards as width grew");
            assert!(text.is_char_boundary(cut));
            previous = cut;
            width += 2.0;
        }
    }

    #[test]
    fn a_line_is_tall_enough_for_everything_drawn_on_it() {
        // The property a table row actually needs: whatever goes in a cell,
        // its ink fits the row. Checked against the awkward cases — a tone
        // mark stacked on an upper vowel, a below-vowel with a tone above, a
        // descender consonant — rather than assumed from one sample.
        let height = line_height(SIZE);
        for text in [
            "ปฏิ",
            "น้ำ",
            "ที่",
            "ปุ๋ย",
            "ก็",
            "ฏ์",
            "Ay",
            "ญ",
            "ฐ",
            "ข้าวสาร",
            "เนื้อวัว",
            "1,240.50",
            "฿70.00",
            "ผัดกะเพรา",
            "ที่นั่งฟรี",
        ] {
            let rendered = render(text, SIZE);
            assert!(
                rendered.bitmap.height as f32 <= height,
                "{text:?} renders {}px against a {height:.2}px row",
                rendered.bitmap.height
            );
        }
    }

    #[test]
    fn a_row_is_not_wastefully_taller_than_its_tallest_glyph() {
        // The other half: a bound nobody reaches is a table with gaps in it.
        let height = line_height(SIZE);
        let tallest = ["ปฏิ", "น้ำ", "ปุ๋ย", "ฐ"]
            .into_iter()
            .map(|text| render(text, SIZE).bitmap.height)
            .max()
            .unwrap() as f32;
        assert!(
            height - tallest <= 2.0,
            "a {height:.2}px row for {tallest}px of ink"
        );
    }

    #[test]
    fn line_height_scales_and_is_stable() {
        assert!(line_height(48.0) > line_height(24.0));
        assert_eq!(line_height(24.0), line_height(24.0));
        assert!(line_height(24.0) > 0.0);
    }
}

#[cfg(test)]
mod tracking_tests {
    use super::*;
    use crate::font::test_fonts::{set, LATIN};

    #[test]
    fn tracking_widens_by_one_slot_per_spacing_glyph() {
        let plain = shape(set(), TextStyle::new(LATIN, 20.0), "ABCD");
        let tracked = shape(set(), TextStyle::tracked(LATIN, 20.0, 0.1), "ABCD");
        let expected = plain.width(20.0) + 4.0 * 2.0;
        assert!((tracked.width(20.0) - expected).abs() < 1e-3);
    }

    #[test]
    fn marks_do_not_receive_tracking() {
        // ก่ is one base plus one mark: one spacing slot, not two.
        let base = shape(set(), TextStyle::tracked(LATIN, 20.0, 0.5), "ก");
        let marked = shape(set(), TextStyle::tracked(LATIN, 20.0, 0.5), "ก่");
        assert!((base.width(20.0) - marked.width(20.0)).abs() < 1e-3);
    }

    #[test]
    fn rendered_advance_matches_measured_width_with_tracking() {
        let style = TextStyle::tracked(LATIN, 16.0, 0.16);
        let shaped = shape(set(), style, "ORDERFLOWER");
        let rendered = render_shaped(&shaped, 16.0);
        assert!((rendered.advance - shaped.width(16.0)).abs() < 1e-3);
        // Ink spreads further with tracking than without.
        let plain = render(set(), TextStyle::new(LATIN, 16.0), "ORDERFLOWER");
        assert!(rendered.bitmap.width > plain.bitmap.width + 10);
    }

    #[test]
    fn fit_accounts_for_tracking() {
        let style = TextStyle::tracked(LATIN, 20.0, 0.2);
        let shaped = shape(set(), style, "ABCDEFGH");
        let full = shaped.width(20.0);
        assert_eq!(shaped.fit(full, 20.0), None);
        let cut = shaped.fit(full - 1.0, 20.0).unwrap();
        assert!(cut < "ABCDEFGH".len());
        assert!(shape(set(), style, &"ABCDEFGH"[..cut]).width(20.0) <= full - 1.0);
    }
}
