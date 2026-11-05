//! How glyph outlines become pixels: the tunable part of text rendering.
//!
//! Three standard techniques, each a field of [`TextRendering`]:
//!
//! - **Coverage correction** (`gamma`, `contrast`). Anti-aliased coverage is
//!   blended in sRGB, which is not linear in light: dark text on a light
//!   panel comes out heavy and grey-fringed, light text on a dark panel thin.
//!   A 256-entry table in front of the blend corrects it, the way Skia and
//!   WebRender do, chosen by the lightness of the text colour and of the
//!   background the string is drawn on (read from the buffer at the corners
//!   of the string's box); see [`liminalkit_raster::gamma`]. Applied when a
//!   [`TextCache`](crate::TextCache) draws, so it costs a table lookup per
//!   partly covered pixel and never re-rasterizes a cached string.
//! - **Vertical grid fitting** (`hinting`). The rasterizer is unhinted, so
//!   the baseline is on a pixel row but the x-height, cap height and
//!   ascender land wherever the size puts them, and their edges smear over
//!   two rows. [`Hinting::Vertical`] stretches each glyph vertically, piece by
//!   piece, so those heights fall on whole pixels and the overshoot of round
//!   letters is folded onto them at small sizes — FreeType's "light"
//!   autohinting idea, done from measured heights instead of a stem analysis.
//!   Horizontal positions are untouched, so advances, kerning and layout are
//!   exactly what they are without it.
//! - **Stem darkening** (`stem_darkening`). Thin stems at small sizes lose
//!   weight under gamma-correct blending; widening every stem horizontally by
//!   a fraction of a pixel (the amount macOS uses, scaled) restores it.
//!   Horizontal only, so a grid-fitted top edge stays on its row.
//!
//! The first is drawing-time state; the other two change the rasterized
//! bitmap.

use crate::font::Face;

/// Vertical grid fitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Hinting {
    /// Outlines are scaled uniformly; only the baseline is on a pixel row.
    #[default]
    None,
    /// The baseline, x-height, cap height, ascender and descender of faces
    /// that have Latin letters are fitted to whole pixels, and small
    /// overshoots are folded onto them. A face without `x` and `H` (a Thai
    /// or symbol face, say) is drawn unhinted.
    Vertical,
}

/// How text is rasterized and blended. See the [module docs](self).
///
/// `TextRendering::default()` is what a [`TextCache`](crate::TextCache)
/// starts with; [`TextRendering::UNCORRECTED`] is the toolkit's rendering
/// before these options existed, byte for byte.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextRendering {
    /// Exponent of the blend the coverage correction emulates: 1.0 is the
    /// plain sRGB blend, 2.2 a blend in linear light. Default 1.8.
    pub gamma: f32,
    /// Extra steepening of coverage on a light background, 0..=1, tapering
    /// to nothing on black (Skia's `SK_GAMMA_CONTRAST`). It gives dark text
    /// back the weight gamma-correct blending takes from it. Default 0.5.
    pub contrast: f32,
    /// Vertical grid fitting. Default [`Hinting::None`].
    pub hinting: Hinting,
    /// Horizontal stem widening, as a multiple of the macOS amount (each
    /// side of a stem moves out by `0.0121 × size` pixels, at most 0.3,
    /// and nothing above 72 px). 0 is off. Default 0.
    pub stem_darkening: f32,
}

impl Default for TextRendering {
    fn default() -> TextRendering {
        TextRendering::DEFAULT
    }
}

impl TextRendering {
    /// The defaults: coverage correction on, no hinting, no darkening.
    ///
    /// Gamma 1.8 with contrast 0.5 keeps dark text on a light panel at
    /// about the weight it had uncorrected (4% less ink, measured in light,
    /// on Inter and Noto Sans at 11 to 17 px) with lighter, tighter fringes,
    /// and gives light text on a dark panel the quarter more ink it was
    /// missing. Gamma 2.2 without contrast is the physically linear blend;
    /// it balances the two exactly and reads as washed out for dark text.
    pub const DEFAULT: TextRendering = TextRendering {
        gamma: 1.8,
        contrast: 0.5,
        hinting: Hinting::None,
        stem_darkening: 0.0,
    };

    /// Coverage blended as rasterized, unhinted and undarkened: exactly
    /// what the toolkit drew before these options existed.
    pub const UNCORRECTED: TextRendering = TextRendering {
        gamma: 1.0,
        contrast: 0.0,
        hinting: Hinting::None,
        stem_darkening: 0.0,
    };

    pub const fn with_gamma(self, gamma: f32, contrast: f32) -> TextRendering {
        TextRendering {
            gamma,
            contrast,
            ..self
        }
    }

    pub const fn with_hinting(self, hinting: Hinting) -> TextRendering {
        TextRendering { hinting, ..self }
    }

    pub const fn with_stem_darkening(self, stem_darkening: f32) -> TextRendering {
        TextRendering {
            stem_darkening,
            ..self
        }
    }

    /// Whether drawing applies a coverage curve at all.
    pub fn corrects_coverage(&self) -> bool {
        !liminalkit_raster::gamma::is_identity(self.gamma, self.contrast)
    }

    /// Whether two settings produce the same bitmaps (they may still blend
    /// them differently).
    pub fn rasterizes_like(&self, other: &TextRendering) -> bool {
        self.hinting == other.hinting && self.stem_darkening == other.stem_darkening
    }

    /// How far each side of a stem moves out at `size_px`, in pixels.
    pub(crate) fn darkening_px(&self, size_px: f32) -> f32 {
        if self.stem_darkening <= 0.0 || size_px > MAX_DARKENING_SIZE {
            return 0.0;
        }
        self.stem_darkening * (size_px * DARKENING_PER_PIXEL).min(MAX_DARKENING)
    }
}

/// The macOS stem-darkening curve, as Pathfinder documents it.
const DARKENING_PER_PIXEL: f32 = 0.0121;
const MAX_DARKENING: f32 = 0.3;
const MAX_DARKENING_SIZE: f32 = 72.0;

/// The heights a face's Latin letters line up on, in design units, measured
/// from the glyphs themselves (OS/2 heights are often missing or rounded).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct VerticalZones {
    /// Top of `x`: the flat x-height.
    x_height: f32,
    /// Top of `H`.
    cap_height: f32,
    /// Top of `o` and bottom of `o`: the round overshoots.
    x_overshoot: Option<f32>,
    baseline_overshoot: Option<f32>,
    /// Top of `l`, bottom of `p`.
    ascender: Option<f32>,
    descender: Option<f32>,
}

impl VerticalZones {
    pub(crate) fn measure(face: &Face) -> Option<VerticalZones> {
        let bounds = |character: char| {
            face.glyph_for(character)
                .and_then(|glyph| face.outline(glyph))
                .and_then(|outline| outline.bounds())
        };
        let x_height = bounds('x')?[3];
        let cap_height = bounds('H')?[3];
        if !(x_height > 0.0 && cap_height > x_height) {
            return None;
        }
        let o = bounds('o');
        Some(VerticalZones {
            x_height,
            cap_height,
            x_overshoot: o.map(|b| b[3]).filter(|&top| top > x_height),
            baseline_overshoot: o.map(|b| b[1]).filter(|&bottom| bottom < 0.0),
            ascender: bounds('l').map(|b| b[3]).filter(|&top| top > x_height),
            descender: bounds('p').map(|b| b[1]).filter(|&bottom| bottom < 0.0),
        })
    }
}

/// A monotone piecewise-linear map from design-unit heights to pixel
/// heights (both upward from the baseline) for one face at one size.
///
/// Knots sit on the measured zones, rounded to whole pixels, between two
/// unrounded anchors at the face's ascender and descender. Heights beyond the
/// anchors scale uniformly, so nothing leaves the line box the face's metrics
/// promise and [`crate::line_height`] stays a bound.
#[derive(Debug, Clone)]
pub(crate) struct VerticalGrid {
    /// (design, pixels), strictly increasing in design and non-decreasing in
    /// pixels.
    knots: Vec<(f32, f32)>,
    scale: f32,
}

impl VerticalGrid {
    pub(crate) fn new(face: &Face, scale: f32) -> Option<VerticalGrid> {
        let zones = face.vertical_zones()?;
        let round = |design: f32| (design * scale).round();
        let x_height = round(zones.x_height);
        let baseline = 0.0f32;
        // An overshoot is a fraction of a pixel at UI sizes; folding it onto
        // the flat height it overshoots is what makes an `o` sit on the same
        // rows as an `x`. Once it rounds to a pixel it is kept as one.
        let fold = |overshoot: f32| (overshoot * scale).round();

        let top = face.ascender().max(zones.cap_height);
        let bottom = face.descender().min(0.0);
        let mut rounded = vec![
            (0.0, baseline),
            (zones.x_height, x_height),
            (zones.cap_height, round(zones.cap_height)),
        ];
        if let Some(over) = zones.x_overshoot {
            rounded.push((over, x_height + fold(over - zones.x_height)));
        }
        if let Some(under) = zones.baseline_overshoot {
            rounded.push((under, baseline - fold(-under)));
        }
        if let Some(ascender) = zones.ascender {
            rounded.push((ascender, round(ascender)));
        }
        if let Some(descender) = zones.descender {
            rounded.push((descender, round(descender)));
        }
        // Rounded knots must stay inside the unrounded anchors.
        let (low, high) = (bottom * scale, top * scale);
        let mut knots: Vec<(f32, f32)> = rounded
            .into_iter()
            .filter(|&(design, _)| design > bottom && design < top)
            .map(|(design, pixels)| (design, pixels.clamp(low, high)))
            .collect();
        knots.push((bottom, low));
        knots.push((top, high));
        knots.sort_by(|a, b| a.0.total_cmp(&b.0));

        // Keep the map monotone: a knot that would fold back below the one
        // before it (two zones within half a pixel, rounding apart) is
        // dropped, as is a repeated height.
        let mut monotone: Vec<(f32, f32)> = Vec::with_capacity(knots.len());
        for knot in knots {
            match monotone.last() {
                Some(&(design, pixels)) if knot.0 <= design || knot.1 < pixels => {}
                _ => monotone.push(knot),
            }
        }
        // The top anchor can only have been dropped by a rounded knot above
        // it in pixels, which the clamp rules out.
        Some(VerticalGrid {
            knots: monotone,
            scale,
        })
    }

    /// The fitted height in pixels of a design-unit height.
    pub(crate) fn map(&self, design: f32) -> f32 {
        let first = self.knots[0];
        let last = self.knots[self.knots.len() - 1];
        if design <= first.0 {
            return first.1 + (design - first.0) * self.scale;
        }
        if design >= last.0 {
            return last.1 + (design - last.0) * self.scale;
        }
        let index = self.knots.partition_point(|&(y, _)| y <= design);
        let (y0, p0) = self.knots[index - 1];
        let (y1, p1) = self.knots[index];
        p0 + (design - y0) / (y1 - y0) * (p1 - p0)
    }
}

/// Move every point of a glyph's flattened contours horizontally away from
/// the ink by up to `amount` pixels: FreeType's `FT_Outline_EmboldenXY` with
/// no vertical strength, on polylines.
///
/// Each vertex moves along the miter of its two edges' outward normals, and
/// only the horizontal part of that is kept. A point on a vertical edge moves
/// the full amount, one on a horizontal edge does not move, and a corner
/// moves with its vertical edge, so stems get wider and nothing gets taller.
/// "Outward" comes from the glyph's overall winding, which covers holes too:
/// a counter's contour runs the other way and its edges move into the
/// counter, which also adds ink.
pub(crate) fn embolden_horizontally(
    points: &mut [[f32; 2]],
    subpaths: &[(usize, usize)],
    amount: f32,
) {
    if amount <= 0.0 {
        return;
    }
    let mut area = 0.0f32;
    for &(start, end) in subpaths {
        let contour = &points[start..end];
        for (index, a) in contour.iter().enumerate() {
            let b = contour[(index + 1) % contour.len()];
            area += a[0] * b[1] - b[0] * a[1];
        }
    }
    if area == 0.0 {
        return;
    }
    // With y down, a contour of positive shoelace area has its ink on the
    // left of travel, so (dy, -dx) points away from it.
    let sign = area.signum();
    let normal = |from: [f32; 2], to: [f32; 2]| -> Option<[f32; 2]> {
        let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
        let length = (dx * dx + dy * dy).sqrt();
        (length > 1e-6).then(|| [sign * dy / length, -sign * dx / length])
    };

    let mut shifted: Vec<f32> = Vec::new();
    for &(start, end) in subpaths {
        let contour = &points[start..end];
        let count = contour.len();
        shifted.clear();
        for index in 0..count {
            let here = contour[index];
            // The nearest distinct neighbours: flattening can repeat a point.
            let previous = (1..count)
                .map(|step| contour[(index + count - step) % count])
                .find(|p| *p != here);
            let next = (1..count)
                .map(|step| contour[(index + step) % count])
                .find(|p| *p != here);
            let dx = match (
                previous.and_then(|p| normal(p, here)),
                next.and_then(|n| normal(here, n)),
            ) {
                (Some(a), Some(b)) => {
                    let cosine = a[0] * b[0] + a[1] * b[1];
                    // A hairpin's miter is unbounded; cap it at twice the
                    // amount, as FreeType does.
                    let denominator = (1.0 + cosine).max(0.5);
                    (a[0] + b[0]) / denominator * amount
                }
                _ => 0.0,
            };
            shifted.push(here[0] + dx);
        }
        for (point, x) in points[start..end].iter_mut().zip(&shifted) {
            point[0] = *x;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::test_fonts::{latin, thai};

    #[test]
    fn a_latin_face_has_zones_and_a_thai_face_does_not() {
        let zones = latin().vertical_zones().expect("Noto Sans has x and H");
        assert!(zones.x_height < zones.cap_height);
        assert!(zones.x_overshoot.unwrap() > zones.x_height);
        assert!(zones.baseline_overshoot.unwrap() < 0.0);
        // No `x` or `H`: Thai runs are drawn unhinted.
        assert!(thai().vertical_zones().is_none());
    }

    #[test]
    fn the_grid_puts_the_zones_on_whole_pixels_and_keeps_the_baseline() {
        let face = latin();
        let zones = face.vertical_zones().unwrap();
        for size in [9.0, 11.0, 12.5, 13.0, 14.0, 15.0, 16.0, 17.0, 21.25, 34.0] {
            let scale = size / face.units_per_em();
            let grid = VerticalGrid::new(face, scale).unwrap();
            assert_eq!(grid.map(0.0), 0.0);
            for height in [zones.x_height, zones.cap_height] {
                let pixels = grid.map(height);
                assert_eq!(pixels, pixels.round(), "{size}px: {height} -> {pixels}");
                assert!((pixels - height * scale).abs() <= 0.5 + 1e-4);
            }
            // The anchors are where uniform scaling puts them.
            assert!((grid.map(face.ascender()) - face.ascender() * scale).abs() < 1e-3);
            assert!((grid.map(face.descender()) - face.descender() * scale).abs() < 1e-3);
            // And the map never runs backwards.
            let mut previous = f32::MIN;
            let mut y = face.descender() - 200.0;
            while y < face.ascender() + 200.0 {
                let mapped = grid.map(y);
                assert!(mapped >= previous - 1e-4, "{size}px folds back at {y}");
                previous = mapped;
                y += 3.0;
            }
        }
    }

    #[test]
    fn a_small_overshoot_is_folded_onto_the_x_height() {
        let face = latin();
        let zones = face.vertical_zones().unwrap();
        let scale = 12.0 / face.units_per_em();
        let grid = VerticalGrid::new(face, scale).unwrap();
        assert_eq!(
            grid.map(zones.x_overshoot.unwrap()),
            grid.map(zones.x_height)
        );
        assert_eq!(grid.map(zones.baseline_overshoot.unwrap()), 0.0);
    }

    #[test]
    fn emboldening_widens_a_square_sideways_whichever_way_it_winds() {
        for clockwise in [false, true] {
            let mut points = vec![[2.0, 2.0], [6.0, 2.0], [6.0, 6.0], [2.0, 6.0]];
            if clockwise {
                points.reverse();
            }
            embolden_horizontally(&mut points, &[(0, 4)], 0.25);
            let xs: Vec<f32> = points.iter().map(|p| p[0]).collect();
            let ys: Vec<f32> = points.iter().map(|p| p[1]).collect();
            assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), 1.75);
            assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 6.25);
            assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), 2.0);
            assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 6.0);
        }
    }

    #[test]
    fn emboldening_shrinks_a_counter() {
        // An outer square and a hole wound the other way: the hole's sides
        // move inward, toward its middle.
        let mut points = vec![
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [3.0, 3.0],
            [3.0, 7.0],
            [7.0, 7.0],
            [7.0, 3.0],
        ];
        embolden_horizontally(&mut points, &[(0, 4), (4, 8)], 0.5);
        assert_eq!(points[0][0], -0.5);
        assert_eq!(points[1][0], 10.5);
        assert_eq!(points[4][0], 3.5);
        assert_eq!(points[6][0], 6.5);
    }

    #[test]
    fn the_defaults_and_the_old_rendering() {
        assert_eq!(TextRendering::default(), TextRendering::DEFAULT);
        assert!(TextRendering::DEFAULT.corrects_coverage());
        assert!(!TextRendering::UNCORRECTED.corrects_coverage());
        assert!(TextRendering::DEFAULT.rasterizes_like(&TextRendering::UNCORRECTED));
        assert!(!TextRendering::DEFAULT
            .with_hinting(Hinting::Vertical)
            .rasterizes_like(&TextRendering::DEFAULT));
        assert_eq!(TextRendering::DEFAULT.darkening_px(16.0), 0.0);
        let dark = TextRendering::DEFAULT.with_stem_darkening(1.0);
        assert!((dark.darkening_px(10.0) - 0.121).abs() < 1e-6);
        assert_eq!(dark.darkening_px(40.0), 0.3);
        assert_eq!(dark.darkening_px(100.0), 0.0);
    }
}
