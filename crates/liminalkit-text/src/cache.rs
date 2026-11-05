//! Drawing text onto the pixel buffer, with a string-level cache.
//!
//! A screen redraws the same strings — labels, headings, the price that has
//! not changed — many times a second, and reshaping and rasterizing them on
//! every frame would be pure waste. Entries are keyed by [`TextStyle`] (face,
//! size, tracking) and string.
//!
//! The cache also holds the [`TextRendering`] its strings are rasterized and
//! drawn with. Changing it with [`TextCache::set_rendering`] drops the
//! bitmaps only if the rasterization changed; the coverage curves it draws
//! with are built per text and background lightness and kept.

use std::collections::HashMap;
use std::rc::Rc;

use liminalkit_raster::gamma::{self, CoverageCurve};
use liminalkit_raster::{Painter, Rect};

use crate::font::{FontSet, TextStyle};
use crate::rendering::TextRendering;
use crate::text::{self, RenderedText};

/// How many rendered strings to keep. A counter screen holds a few dozen
/// distinct strings; this is generous enough never to thrash and small enough
/// never to matter.
const CACHE_LIMIT: usize = 512;
/// A desktop terminal draws several hundred distinct strings a frame; the
/// default limits are for that. Constructors take explicit ones.
pub const DEFAULT_ENTRY_LIMIT: usize = 4096;
pub const DEFAULT_BITMAP_BUDGET: usize = 48 * 1024 * 1024;
/// Raster data, not entry count, is what dominates the cache on a small
/// machine. Four MiB is enough for the stable text on every current screen
/// without allowing a report full of long labels to grow indefinitely.
#[cfg(test)]
const BITMAP_BUDGET: usize = 4 * 1024 * 1024;
/// Truncation strings are tiny beside bitmaps, but user/data-derived labels
/// still need a byte bound rather than only an entry bound.
const FITTED_TEXT_BUDGET: usize = 256 * 1024;

/// Where a string sits relative to the position given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Centre,
    Right,
}

/// Rendered text, kept between frames.
pub struct TextCache {
    fonts: &'static FontSet,
    entry_limit: usize,
    bitmap_budget: usize,
    /// Wholesale clears since creation — a frame that clears every frame is
    /// a cache too small for its screen.
    evictions: u64,
    /// Nested by style so the inner map can look up an `Rc<str>` with `&str`.
    /// A tuple key would construct a fresh owned `String` on every cache
    /// hit — an allocation for every label on every frame.
    entries: HashMap<u64, HashMap<Rc<str>, RenderedText>>,
    entry_count: usize,
    bitmap_bytes: usize,
    /// Truncation results, keyed by string, column width and size.
    ///
    /// Separate from `entries` on purpose. Truncation used to measure every
    /// prefix of every string, and each of those measurements landed in the
    /// shared cache — so a table of fifty long Thai names filled a 512-entry
    /// map with prefixes, and eviction (a wholesale clear) then threw away the
    /// strings actually on screen. The result of a truncation is one entry and
    /// is what gets asked for again next frame.
    fitted: HashMap<(i32, u64), HashMap<Rc<str>, Rc<str>>>,
    fitted_count: usize,
    fitted_bytes: usize,
    /// Wrap results, keyed by width and style; a paragraph redrawn every
    /// frame is wrapped once.
    wrapped: HashMap<(i32, u64), HashMap<Rc<str>, Lines>>,
    wrapped_count: usize,
    rendering: TextRendering,
    /// Coverage curves by text and background lightness ([`gamma::luma`]),
    /// built on first use. A screen uses a handful of pairs; the map is
    /// cleared if it ever holds [`CURVE_LIMIT`]. Empty when `rendering` does
    /// not correct coverage.
    curves: HashMap<(u8, u8), Box<CoverageCurve>>,
}

/// More text-and-background pairs than any screen draws: 256 KiB of tables.
const CURVE_LIMIT: usize = 1024;

/// Byte ranges of the lines a string wraps into.
pub type Lines = Rc<[std::ops::Range<usize>]>;

/// The cache key for a style: face, size to a tenth of a pixel, tracking to
/// a thousandth of an em.
fn style_key(style: TextStyle) -> u64 {
    let size = (style.size * 10.0).round().max(0.0) as u64;
    let tracking = ((style.tracking * 1000.0).round() as i64 + (1 << 20)) as u64;
    (u64::from(style.face.0) << 56) | (tracking << 24) | (size & 0x00ff_ffff)
}

impl TextCache {
    pub fn new(fonts: &'static FontSet) -> TextCache {
        TextCache::with_limits(fonts, DEFAULT_ENTRY_LIMIT, DEFAULT_BITMAP_BUDGET)
    }

    /// A cache with explicit bounds (entries, rasterized bytes).
    pub fn with_limits(
        fonts: &'static FontSet,
        entry_limit: usize,
        bitmap_budget: usize,
    ) -> TextCache {
        Self {
            fonts,
            entry_limit: entry_limit.max(1),
            bitmap_budget: bitmap_budget.max(1),
            evictions: 0,
            entries: HashMap::new(),
            entry_count: 0,
            bitmap_bytes: 0,
            fitted: HashMap::new(),
            fitted_count: 0,
            fitted_bytes: 0,
            wrapped: HashMap::new(),
            wrapped_count: 0,
            rendering: TextRendering::default(),
            curves: HashMap::new(),
        }
    }

    /// The same cache, rendering with `rendering` instead of the default.
    pub fn with_rendering(mut self, rendering: TextRendering) -> TextCache {
        self.set_rendering(rendering);
        self
    }

    /// How strings are rasterized and blended from now on.
    ///
    /// Rendered bitmaps are dropped only if hinting or stem darkening
    /// changed; a new gamma or contrast takes effect on the next draw
    /// without re-rasterizing anything. Measurements never change: neither
    /// option moves an advance.
    pub fn set_rendering(&mut self, rendering: TextRendering) {
        if !rendering.rasterizes_like(&self.rendering) {
            self.entries.clear();
            self.entry_count = 0;
            self.bitmap_bytes = 0;
        }
        if rendering.gamma != self.rendering.gamma || rendering.contrast != self.rendering.contrast
        {
            self.curves.clear();
        }
        self.rendering = rendering;
    }

    pub fn rendering(&self) -> TextRendering {
        self.rendering
    }

    /// The coverage curve for text in `color` drawn over the painter's
    /// pixels at `area`, or `None` when coverage is drawn uncorrected.
    ///
    /// The background is read, not assumed: the opaque pixels at the four
    /// corners of the string's bitmap, which its padding keeps clear of its
    /// own ink, give their median lightness. Where none can be read (all
    /// clipped, or a transparent layer) the text colour's opposite stands in,
    /// as in Skia.
    fn curve(&mut self, painter: &Painter<'_>, color: u32, area: Rect) -> Option<CoverageCurve> {
        if !self.rendering.corrects_coverage() {
            return None;
        }
        let TextRendering {
            gamma, contrast, ..
        } = self.rendering;
        let text = gamma::luma(color, gamma);
        let mut corners = [0u8; 4];
        let mut count = 0;
        // Corners are nearly always one colour; convert each colour once.
        let mut last: Option<(u32, u8)> = None;
        for (x, y) in [
            (area.x, area.y),
            (area.right() - 1, area.y),
            (area.x, area.bottom() - 1),
            (area.right() - 1, area.bottom() - 1),
        ] {
            if let Some(pixel) = painter.backdrop(x, y) {
                let luma = match last {
                    Some((seen, luma)) if seen == pixel => luma,
                    _ => gamma::luma(pixel, gamma),
                };
                last = Some((pixel, luma));
                corners[count] = luma;
                count += 1;
            }
        }
        let corners = &mut corners[..count];
        corners.sort_unstable();
        let background = match count {
            0 => gamma::opposite(text),
            _ if count % 2 == 1 => corners[count / 2],
            _ => ((u16::from(corners[count / 2 - 1]) + u16::from(corners[count / 2])) / 2) as u8,
        };
        if self.curves.len() >= CURVE_LIMIT {
            self.curves.clear();
        }
        Some(
            **self
                .curves
                .entry((text, background))
                .or_insert_with(|| Box::new(gamma::curve(text, background, gamma, contrast))),
        )
    }

    /// Line ranges for `text` wrapped into `width` pixels, cached.
    pub fn wrap(
        &mut self,
        text: &str,
        width: i32,
        style: TextStyle,
    ) -> Rc<[std::ops::Range<usize>]> {
        let key = (width, style_key(style));
        if let Some(lines) = self.wrapped.get(&key).and_then(|bucket| bucket.get(text)) {
            return lines.clone();
        }
        let lines: Rc<[std::ops::Range<usize>]> =
            text::wrap(self.fonts, style, text, width as f32).into();
        if self.wrapped_count >= CACHE_LIMIT {
            self.wrapped.clear();
            self.wrapped_count = 0;
        }
        self.wrapped
            .entry(key)
            .or_default()
            .insert(Rc::from(text), lines.clone());
        self.wrapped_count += 1;
        lines
    }

    pub fn fonts(&self) -> &'static FontSet {
        self.fonts
    }

    pub fn len(&self) -> usize {
        self.entry_count
    }

    pub fn is_empty(&self) -> bool {
        self.entry_count == 0
    }

    /// Bytes held by rasterized glyph coverage, excluding small map metadata.
    pub fn bitmap_bytes(&self) -> usize {
        self.bitmap_bytes
    }

    /// Wholesale evictions so far. Rising every frame means the limits are
    /// too small for the screen.
    pub fn evictions(&self) -> u64 {
        self.evictions
    }

    /// Number of cached fitted strings.
    pub fn fitted_len(&self) -> usize {
        self.fitted_count
    }

    fn rendered(&mut self, text: &str, style: TextStyle) -> &RenderedText {
        let size_key = style_key(style);
        let missing = self
            .entries
            .get(&size_key)
            .and_then(|bucket| bucket.get(text))
            .is_none();
        if missing {
            let rendered = text::render_with(self.fonts, style, text, &self.rendering);
            let bytes = rendered.bitmap.pixels.capacity();
            if self.entry_count >= self.entry_limit
                || (self.entry_count > 0
                    && self.bitmap_bytes.saturating_add(bytes) > self.bitmap_budget)
            {
                self.evictions += 1;
                // Wholesale eviction is intentional: the working set is
                // screen-sized, and an LRU would add a timestamp and mutation
                // to every otherwise read-only cache hit.
                self.entries.clear();
                self.entry_count = 0;
                self.bitmap_bytes = 0;
            }
            self.entries
                .entry(size_key)
                .or_default()
                .insert(Rc::from(text), rendered);
            self.entry_count += 1;
            self.bitmap_bytes = self.bitmap_bytes.saturating_add(bytes);
        }
        self.entries
            .get(&size_key)
            .and_then(|bucket| bucket.get(text))
            .expect("rendered text was inserted")
    }

    /// Width in pixels, without drawing.
    pub fn measure(&mut self, text: &str, style: TextStyle) -> i32 {
        self.rendered(text, style).advance.ceil() as i32
    }

    /// Distance from the top of a line to its baseline.
    pub fn baseline(&mut self, text: &str, style: TextStyle) -> i32 {
        self.rendered(text, style).baseline_y
    }

    /// Draw, with `y` at the top of the line box. Returns the advance.
    pub fn draw(
        &mut self,
        painter: &mut Painter<'_>,
        text: &str,
        x: i32,
        y: i32,
        style: TextStyle,
        color: u32,
    ) -> i32 {
        if text.is_empty() {
            return 0;
        }
        let (origin_x, width, height) = {
            let rendered = self.rendered(text, style);
            (
                rendered.origin_x,
                rendered.bitmap.width,
                rendered.bitmap.height,
            )
        };
        let area = Rect::new(x - origin_x, y, width as i32, height as i32);
        let curve = self.curve(painter, color, area);
        let rendered = self.rendered(text, style);
        // The bitmap holds a pixel of padding and any ink left of the origin,
        // so the pen position is offset by the recorded origin.
        let left = x - rendered.origin_x;
        // A row at a time, not a pixel at a time. The clip test, the bounds
        // check and the zero-coverage branch used to run once per pixel of
        // every glyph's bounding box — most of which is empty — for every
        // label on every frame. Handing the painter a whole row lets it clip
        // once and blend the rest as a span.
        let bitmap = &rendered.bitmap;
        for row in 0..bitmap.height {
            let start = row * bitmap.width;
            let coverage = &bitmap.pixels[start..start + bitmap.width];
            match &curve {
                Some(curve) => {
                    painter.blend_coverage_row_mapped(left, y + row as i32, color, coverage, curve)
                }
                None => painter.blend_coverage_row(left, y + row as i32, color, coverage),
            }
        }
        rendered.advance.ceil() as i32
    }

    /// Draw a one-pixel stroke around `text`, and nothing in the middle.
    ///
    /// Meant to be drawn *under* the same string in its own colour, which
    /// leaves the character outlined rather than recoloured — the difference
    /// between "this one is different" and "this one changed", and only the
    /// first is true of a digit that has just been typed.
    ///
    /// Eight blits of one glyph. Deliberately a stroke that is simply on and
    /// then off rather than one that fades: a fade has to be redrawn on every
    /// frame it lasts, and on the machine this is for, not waking up is the
    /// entire objective.
    pub fn draw_stroke(
        &mut self,
        painter: &mut Painter<'_>,
        text: &str,
        x: i32,
        y: i32,
        style: TextStyle,
        color: u32,
    ) {
        if text.is_empty() {
            return;
        }
        const AROUND: [(i32, i32); 8] = [
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ];
        let (origin_x, width, height) = {
            let rendered = self.rendered(text, style);
            (
                rendered.origin_x,
                rendered.bitmap.width,
                rendered.bitmap.height,
            )
        };
        let area = Rect::new(x - origin_x - 1, y - 1, width as i32 + 2, height as i32 + 2);
        let curve = self.curve(painter, color, area);
        let rendered = self.rendered(text, style);
        let left = x - rendered.origin_x;
        let bitmap = &rendered.bitmap;
        for (dx, dy) in AROUND {
            for row in 0..bitmap.height {
                let start = row * bitmap.width;
                let coverage = &bitmap.pixels[start..start + bitmap.width];
                let (x, y) = (left + dx, y + row as i32 + dy);
                match &curve {
                    Some(curve) => painter.blend_coverage_row_mapped(x, y, color, coverage, curve),
                    None => painter.blend_coverage_row(x, y, color, coverage),
                }
            }
        }
    }

    /// Draw aligned within a rectangle's width.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_aligned(
        &mut self,
        painter: &mut Painter<'_>,
        text: &str,
        area: Rect,
        y: i32,
        style: TextStyle,
        color: u32,
        align: Align,
    ) -> i32 {
        let width = self.measure(text, style);
        let x = match align {
            Align::Left => area.x,
            Align::Centre => area.x + (area.w - width) / 2,
            Align::Right => area.right() - width,
        };
        self.draw(painter, text, x, y, style, color)
    }

    /// The height of one line at a size, for laying rows out.
    ///
    /// From the font's own ascent and descent, not from the ink of a sample
    /// string. Measuring a sample gives whatever that sample happens to reach:
    /// `ปฏิ` has an ascender and a descender but no tone mark stacked above an
    /// upper vowel, so น้ำ and ที่ are taller than it, and a table sized by the
    /// sample clips their tone marks or laps them onto the row above.
    pub fn line_height(&mut self, style: TextStyle) -> i32 {
        text::line_height(self.fonts, style.face, style.size).ceil() as i32
    }

    /// Shorten `text` to fit `width`, ending in an ellipsis if anything was
    /// dropped.
    ///
    /// A table column is a fixed width and an ingredient name is not, so
    /// something has to give. Clipping the glyphs mid-stroke would be cheaper
    /// and reads as a rendering fault; an ellipsis reads as "there is more".
    ///
    /// The cut never lands before a combining mark. A Thai tone mark separated
    /// from its consonant does not look untidy — it attaches to the ellipsis
    /// and changes the word.
    pub fn truncate(&mut self, text: &str, width: i32, style: TextStyle) -> String {
        self.fitted_text(text, width, style).to_string()
    }

    fn fitted_text(&mut self, text: &str, width: i32, style: TextStyle) -> Rc<str> {
        if width <= 0 {
            return Rc::from("");
        }
        let key = (width, style_key(style));
        if let Some(fitted) = self.fitted.get(&key).and_then(|bucket| bucket.get(text)) {
            return fitted.clone();
        }
        let fitted: Rc<str> = Rc::from(self.compute_truncation(text, width, style));
        let bytes = text.len().saturating_add(fitted.len());
        if self.fitted_count >= CACHE_LIMIT
            || (self.fitted_count > 0
                && self.fitted_bytes.saturating_add(bytes) > FITTED_TEXT_BUDGET)
        {
            self.fitted.clear();
            self.fitted_count = 0;
            self.fitted_bytes = 0;
        }
        self.fitted
            .entry(key)
            .or_default()
            .insert(Rc::from(text), fitted.clone());
        self.fitted_count += 1;
        self.fitted_bytes = self.fitted_bytes.saturating_add(bytes);
        fitted
    }

    fn compute_truncation(&mut self, text: &str, width: i32, style: TextStyle) -> String {
        if self.measure(text, style) <= width {
            return text.to_owned();
        }

        const ELLIPSIS: &str = "…";
        let ellipsis = self.measure(ELLIPSIS, style);
        // No room for even the ellipsis: better to show nothing than a lone
        // ellipsis in a column too narrow to have said anything.
        if ellipsis > width {
            return String::new();
        }

        // One shaping, and the cut comes back on a cluster boundary. The
        // obvious implementation — measure `text[..n]` for growing `n` — is
        // quadratic, fills the shared cache with prefixes nothing will ever
        // ask for again, and assumes shaping a prefix gives the same advances
        // as shaping the whole string.
        let budget = (width - ellipsis) as f32;
        let kept = text::shape(self.fonts, style, text)
            .fit(budget, style.size)
            .unwrap_or(text.len());

        if kept == 0 {
            return ELLIPSIS.to_owned();
        }
        format!("{}{ELLIPSIS}", &text[..kept])
    }

    /// Draw, shortened to fit `area`, aligned within it.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_fitted(
        &mut self,
        painter: &mut Painter<'_>,
        text: &str,
        area: Rect,
        y: i32,
        style: TextStyle,
        color: u32,
        align: Align,
    ) -> i32 {
        let fitted = self.fitted_text(text, area.w, style);
        self.draw_aligned(painter, &fitted, area, y, style, color, align)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::test_fonts::{set, style};
    use crate::TextRendering;
    use liminalkit_raster::WindowBuffer;

    fn cache() -> TextCache {
        TextCache::new(set())
    }

    fn ink(buffer: &WindowBuffer) -> usize {
        buffer.pixels.iter().filter(|&&pixel| pixel != 0).count()
    }

    #[test]
    fn drawing_thai_puts_ink_on_the_buffer() {
        let mut buffer = WindowBuffer::new(200, 60);
        let mut cache = cache();
        let mut painter = Painter::new(&mut buffer);
        let advance = cache.draw(&mut painter, "ผัดกะเพรา", 10, 10, style(24.0), 0x00ff_ffff);
        assert!(advance > 0);
        assert!(ink(&buffer) > 0);
    }

    #[test]
    fn the_cache_reuses_a_string_rather_than_reshaping_it() {
        let mut cache = cache();
        assert!(cache.is_empty());
        cache.measure("ผัดกะเพรา", style(24.0));
        assert_eq!(cache.len(), 1);
        cache.measure("ผัดกะเพรา", style(24.0));
        assert_eq!(
            cache.len(),
            1,
            "the same string at the same size is one entry"
        );
        cache.measure("ผัดกะเพรา", style(26.0));
        assert_eq!(cache.len(), 2, "a different size is a different rendering");
    }

    #[test]
    fn the_cache_is_bounded() {
        let mut cache = TextCache::with_limits(set(), CACHE_LIMIT, BITMAP_BUDGET);
        for index in 0..CACHE_LIMIT + 10 {
            cache.measure(&format!("{index}"), style(20.0));
        }
        assert!(cache.len() <= CACHE_LIMIT);
        assert!(cache.bitmap_bytes() <= BITMAP_BUDGET);
        assert!(cache.evictions() >= 1);
    }

    #[test]
    fn fitted_text_is_reused_without_growing_the_cache_each_frame() {
        let mut cache = cache();
        let text = "ผงชาไทยพิเศษสูตรโบราณของร้าน";
        let width = cache.measure(text, style(20.0)) / 2;
        assert!(cache.truncate(text, width, style(20.0)).ends_with('…'));
        assert_eq!(cache.fitted_len(), 1);
        for _ in 0..100 {
            cache.truncate(text, width, style(20.0));
        }
        assert_eq!(cache.fitted_len(), 1);
    }

    #[test]
    fn alignment_places_text_where_it_says() {
        let area = Rect::new(0, 0, 200, 40);
        let mut cache = cache();
        let text = "฿70.00";
        let width = cache.measure(text, style(20.0));

        for (align, expected_left) in [
            (Align::Left, 0),
            (Align::Centre, (200 - width) / 2),
            (Align::Right, 200 - width),
        ] {
            let mut buffer = WindowBuffer::new(200, 40);
            let mut painter = Painter::new(&mut buffer);
            cache.draw_aligned(&mut painter, text, area, 5, style(20.0), 0x00ff_ffff, align);

            let first_column = (0..200)
                .find(|&x| (0..40).any(|y| buffer.pixels[y * 200 + x] != 0))
                .expect("some ink");
            // Within a couple of pixels: the first inked column is inside the
            // glyph, not at its advance origin.
            assert!(
                (first_column as i32 - expected_left).abs() <= 4,
                "{align:?} put ink at {first_column}, expected near {expected_left}"
            );
        }
    }

    #[test]
    fn an_empty_string_draws_nothing_and_advances_nothing() {
        let mut buffer = WindowBuffer::new(50, 20);
        let mut cache = cache();
        let mut painter = Painter::new(&mut buffer);
        assert_eq!(
            cache.draw(&mut painter, "", 0, 0, style(20.0), 0x00ff_ffff),
            0
        );
        assert_eq!(ink(&buffer), 0);
    }

    #[test]
    fn line_height_does_not_depend_on_what_is_being_drawn() {
        let mut cache = cache();
        let height = cache.line_height(style(24.0));
        assert!(height > 0);
        assert_eq!(cache.line_height(style(24.0)), height);
        assert!(cache.line_height(style(48.0)) > height);
    }

    #[test]
    fn text_that_fits_is_returned_unchanged() {
        let mut cache = cache();
        let text = "ข้าวสาร";
        let width = cache.measure(text, style(20.0));
        assert_eq!(cache.truncate(text, width, style(20.0)), text);
        assert_eq!(cache.truncate(text, width + 50, style(20.0)), text);
    }

    #[test]
    fn text_that_does_not_fit_is_shortened_and_marked() {
        let mut cache = cache();
        let text = "ผงชาไทยพิเศษสูตรโบราณ";
        let full = cache.measure(text, style(20.0));
        let fitted = cache.truncate(text, full / 2, style(20.0));

        assert!(fitted.ends_with('…'), "{fitted:?}");
        assert!(fitted.chars().count() < text.chars().count());
        assert!(
            cache.measure(&fitted, style(20.0)) <= full / 2,
            "{fitted:?} still does not fit"
        );
    }

    #[test]
    fn a_cut_never_lands_before_a_combining_mark() {
        // The rule that makes this worth writing rather than using char
        // counts: ก้ truncated between ก and ้ leaves the tone mark on the
        // ellipsis, which is a different word rather than an untidy one.
        let mut cache = cache();
        let text = "กิน้ำพริกเผาเผ็ดมากจริงๆนะ";
        for width in 1..cache.measure(text, style(18.0)) {
            let fitted = cache.truncate(text, width, style(18.0));
            let body = fitted.trim_end_matches('…');
            assert!(
                !text[body.len()..]
                    .chars()
                    .next()
                    .is_some_and(crate::font::is_thai_combining),
                "width {width} cut before a mark: {fitted:?}"
            );
        }
    }

    #[test]
    fn a_column_too_narrow_to_say_anything_says_nothing() {
        let mut cache = cache();
        assert_eq!(cache.truncate("ข้าวสาร", 0, style(20.0)), "");
        assert_eq!(cache.truncate("ข้าวสาร", -5, style(20.0)), "");
        // Narrower than the ellipsis itself.
        assert_eq!(cache.truncate("ข้าวสาร", 1, style(20.0)), "");
    }

    #[test]
    fn fitted_drawing_stays_inside_its_column() {
        let mut buffer = WindowBuffer::new(120, 30);
        let mut cache = cache();
        let area = Rect::new(10, 0, 60, 30);
        {
            let mut painter = Painter::new(&mut buffer);
            cache.draw_fitted(
                &mut painter,
                "ผงชาไทยพิเศษสูตรโบราณของร้าน",
                area,
                5,
                style(18.0),
                0x00ff_ffff,
                Align::Left,
            );
        }
        let rightmost = (0..120)
            .rev()
            .find(|&x| (0..30).any(|y| buffer.pixels[y * 120 + x] != 0))
            .expect("some ink");
        assert!(
            (rightmost as i32) < area.right() + 2,
            "ink reached {rightmost}, past the column at {}",
            area.right()
        );
    }

    /// `text` drawn in `color` over a panel of `background`, with `rendering`.
    fn drawn(rendering: TextRendering, background: u32, color: u32) -> Vec<u32> {
        let mut buffer = WindowBuffer::new(220, 30);
        let mut cache = cache().with_rendering(rendering);
        let mut painter = Painter::new(&mut buffer);
        painter.clear(background);
        cache.draw(&mut painter, "Settings minimum", 4, 4, style(15.0), color);
        buffer.pixels
    }

    /// How far the pixels moved from the background toward the text colour,
    /// in sRGB green-channel steps, summed.
    fn ink_toward(pixels: &[u32], background: u32) -> i64 {
        let green = |pixel: u32| i64::from((pixel >> 8) & 0xff);
        pixels
            .iter()
            .map(|&pixel| (green(pixel) - green(background)).abs())
            .sum()
    }

    #[test]
    fn coverage_correction_lightens_dark_fringes_and_brightens_light_text() {
        let (paper, ink) = (0x00f6_f6f4, 0x001c_1c1e);
        let plain = drawn(TextRendering::UNCORRECTED, paper, ink);
        let corrected = drawn(TextRendering::DEFAULT, paper, ink);
        // Nothing the plain blend left alone is touched: the correction
        // reweights edges and never spreads them.
        for (&a, &b) in plain.iter().zip(&corrected) {
            assert!(a != paper || b == paper);
        }
        let linear = drawn(TextRendering::DEFAULT.with_gamma(2.2, 0.0), paper, ink);
        assert!(ink_toward(&linear, paper) < ink_toward(&plain, paper));
        // Contrast gives dark text back most of its weight.
        assert!(ink_toward(&corrected, paper) > ink_toward(&linear, paper));

        let (night, chalk) = (0x001c_1c1e, 0x00e8_e8e6);
        let plain = drawn(TextRendering::UNCORRECTED, night, chalk);
        let corrected = drawn(TextRendering::DEFAULT, night, chalk);
        assert!(
            ink_toward(&corrected, night) as f32 > ink_toward(&plain, night) as f32 * 1.1,
            "light text on a dark panel should gain weight"
        );
    }

    #[test]
    fn grey_text_is_corrected_against_the_panel_it_is_drawn_on() {
        // Grey on white. Guessing the background from the text colour alone
        // would assume mid-grey and correct almost nothing; read from the
        // buffer it is corrected like black on white.
        let (paper, grey) = (0x00ff_ffff, 0x006a_6a6a);
        let plain = ink_toward(&drawn(TextRendering::UNCORRECTED, paper, grey), paper);
        let linear = TextRendering::DEFAULT.with_gamma(2.2, 0.0);
        let read = ink_toward(&drawn(linear, paper, grey), paper);
        assert!((read as f32) < plain as f32 * 0.9, "{read} vs {plain}");
    }

    #[test]
    fn changing_only_the_blend_keeps_the_rendered_strings() {
        let mut cache = cache();
        cache.measure("Settings", style(15.0));
        cache.set_rendering(TextRendering::DEFAULT.with_gamma(2.2, 0.0));
        assert_eq!(cache.len(), 1, "a new gamma re-rasterizes nothing");
        cache.set_rendering(cache.rendering().with_hinting(crate::Hinting::Vertical));
        assert_eq!(cache.len(), 0, "hinting changes the bitmaps");
        let hinted = cache.measure("Settings", style(15.0));
        assert_eq!(
            hinted,
            TextCache::new(set()).measure("Settings", style(15.0))
        );
    }

    #[test]
    fn uncorrected_drawing_is_the_plain_blend() {
        // The old kernel, byte for byte: what an application that opts out
        // gets.
        let rendered = text::render(set(), style(15.0), "Settings minimum");
        let mut expected = WindowBuffer::new(220, 30);
        {
            let mut painter = Painter::new(&mut expected);
            painter.clear(0x00f6_f6f4);
            for row in 0..rendered.bitmap.height {
                let start = row * rendered.bitmap.width;
                painter.blend_coverage_row(
                    4 - rendered.origin_x,
                    4 + row as i32,
                    0x001c_1c1e,
                    &rendered.bitmap.pixels[start..start + rendered.bitmap.width],
                );
            }
        }
        assert_eq!(
            drawn(TextRendering::UNCORRECTED, 0x00f6_f6f4, 0x001c_1c1e),
            expected.pixels
        );
    }

    #[test]
    fn a_price_line_draws_both_faces() {
        // The mixed-face case, on screen: ฿ from Thai, digits from Latin.
        let mut buffer = WindowBuffer::new(300, 50);
        let mut cache = cache();
        let mut painter = Painter::new(&mut buffer);
        cache.draw(&mut painter, "฿1,240.50", 5, 5, style(26.0), 0x00ff_ffff);

        let columns: Vec<usize> = (0..300)
            .filter(|&x| (0..50).any(|y| buffer.pixels[y * 300 + x] != 0))
            .collect();
        let span = columns.last().unwrap() - columns.first().unwrap();
        assert!(
            span > 80,
            "the whole price should be drawn, span was {span}"
        );
    }
}
