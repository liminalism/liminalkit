//! A font face: everything needed to turn characters into positioned glyphs.
//!
//! Faces are parsed from bytes the application embeds ([`Face::from_bytes`])
//! and registered in a [`FontSet`], which hands out [`FaceId`]s and resolves a
//! per-face fallback chain. A [`TextStyle`] names a face, a pixel size and a
//! letter-spacing; that is what the shaper and the cache key on.

use crate::layout::{self, Gdef, LayoutTable, ShapedGlyph};
use crate::sfnt::{CharacterMap, FontMetrics, GlyphSource, HorizontalMetrics, Outline, Sfnt};

/// Substitutions applied to every run.
///
/// `ccmp` is the one that matters for Thai: it decomposes SARA AM and selects
/// the lowered tone-mark forms used over tall consonants. `liga` is applied
/// too because the face defines Thai ligatures there. `aalt` is deliberately
/// absent — it offers stylistic alternates for a user to choose between, and
/// there is no user here to choose.
const SUBSTITUTION_FEATURES: [&[u8; 4]; 2] = [b"ccmp", b"liga"];

/// Positioning applied to every run: kerning, mark-to-base and mark-to-mark.
const POSITIONING_FEATURES: [&[u8; 4]; 3] = [b"kern", b"mark", b"mkmk"];

/// One parsed face.
pub struct Face {
    metrics: FontMetrics,
    characters: CharacterMap,
    advances: HorizontalMetrics,
    glyphs: GlyphSource<'static>,
    gsub: Option<LayoutTable<'static>>,
    gpos: Option<LayoutTable<'static>>,
    gdef: Gdef<'static>,
    substitution_lookups: Vec<u16>,
    positioning_lookups: Vec<u16>,
}

impl std::fmt::Debug for Face {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Face")
            .field("units_per_em", &self.metrics.units_per_em)
            .field("glyph_count", &self.metrics.glyph_count)
            .field("substitution_lookups", &self.substitution_lookups.len())
            .field("positioning_lookups", &self.positioning_lookups.len())
            .finish()
    }
}

impl Face {
    /// Parse a TrueType (`glyf`) face from bytes the caller keeps alive for
    /// the program's lifetime — normally an `include_bytes!`. Returns `None`
    /// for anything this parser cannot read: CFF/OTF outlines, variable-only
    /// fonts, collections.
    pub fn from_bytes(data: &'static [u8]) -> Option<Face> {
        let sfnt = Sfnt::parse(data)?;
        let metrics = FontMetrics::parse(&sfnt)?;
        let characters = CharacterMap::parse(&sfnt)?;
        let advances = HorizontalMetrics::parse(&sfnt, &metrics)?;
        let glyphs = GlyphSource::parse(&sfnt, &metrics)?;

        let gsub = sfnt.table(b"GSUB").and_then(LayoutTable::parse);
        let gpos = sfnt.table(b"GPOS").and_then(LayoutTable::parse);
        let gdef = sfnt
            .table(b"GDEF")
            .and_then(Gdef::parse)
            .unwrap_or_else(Gdef::empty);

        let substitution_lookups = gsub
            .as_ref()
            .map(|table| table.lookups_for(&SUBSTITUTION_FEATURES))
            .unwrap_or_default();
        let positioning_lookups = gpos
            .as_ref()
            .map(|table| table.lookups_for(&POSITIONING_FEATURES))
            .unwrap_or_default();

        Some(Face {
            metrics,
            characters,
            advances,
            glyphs,
            gsub,
            gpos,
            gdef,
            substitution_lookups,
            positioning_lookups,
        })
    }

    pub fn units_per_em(&self) -> f32 {
        f32::from(self.metrics.units_per_em)
    }

    pub fn ascender(&self) -> f32 {
        f32::from(self.metrics.ascender)
    }

    pub fn descender(&self) -> f32 {
        f32::from(self.metrics.descender)
    }

    pub fn line_gap(&self) -> f32 {
        f32::from(self.metrics.line_gap)
    }

    pub fn covers(&self, character: char) -> bool {
        self.characters.covers(character)
    }

    pub fn glyph_for(&self, character: char) -> Option<u16> {
        self.characters.glyph(character)
    }

    pub fn outline(&self, glyph: u16) -> Option<Outline> {
        self.glyphs.outline(glyph)
    }

    pub fn advance(&self, glyph: u16) -> f32 {
        f32::from(self.advances.advance(glyph))
    }

    pub fn is_mark(&self, glyph: u16) -> bool {
        self.gdef.is_mark(glyph)
    }

    /// Turn text into positioned glyphs, in design units, left to right.
    ///
    /// `cluster_base` is added to every glyph's cluster, so a caller shaping
    /// several runs of one string gets clusters that index the whole string.
    pub fn shape(&self, text: &str, cluster_base: usize) -> Vec<ShapedGlyph> {
        self.shape_run(text, cluster_base, false)
    }

    /// Turn one directional run into positioned glyphs, in design units.
    ///
    /// A right-to-left run comes out in visual order: substitution happens in
    /// logical order, then the buffer is reversed one base-plus-marks group at
    /// a time (a mark stays after the base it attaches to), and positioning
    /// runs over the visual sequence. That is the order OpenType pair and
    /// mark lookups in a right-to-left font expect, and it is what a full
    /// shaper does.
    ///
    /// Which runs are right-to-left is the caller's business; with the
    /// `bidi` feature [`crate::text::shape`] works it out per line.
    pub fn shape_run(&self, text: &str, cluster_base: usize, rtl: bool) -> Vec<ShapedGlyph> {
        let mut glyphs = self.map_characters(text, cluster_base);

        if let Some(gsub) = &self.gsub {
            layout::apply_substitutions(gsub, &self.gdef, &self.substitution_lookups, &mut glyphs);
        }
        // Advances are re-read after substitution: a ligature or an alternate
        // has its own width, and the pre-substitution one would be wrong.
        for glyph in &mut glyphs {
            glyph.x_advance = self.advance(glyph.glyph);
        }
        if rtl {
            self.reverse_groups(&mut glyphs);
        }
        if let Some(gpos) = &self.gpos {
            layout::apply_positioning(gpos, &self.gdef, &self.positioning_lookups, &mut glyphs);
        }
        glyphs
    }

    /// Reverse a glyph buffer keeping each base with the marks that follow it.
    fn reverse_groups(&self, glyphs: &mut [ShapedGlyph]) {
        let mut groups: Vec<(usize, usize)> = Vec::new();
        for (index, glyph) in glyphs.iter().enumerate() {
            match groups.last_mut() {
                Some((_, end)) if self.is_mark(glyph.glyph) => *end = index + 1,
                _ => groups.push((index, index + 1)),
            }
        }
        let reordered: Vec<ShapedGlyph> = groups
            .iter()
            .rev()
            .flat_map(|&(start, end)| glyphs[start..end].iter().copied())
            .collect();
        glyphs.copy_from_slice(&reordered);
    }

    /// Map characters to glyphs, applying the one reordering the font's own
    /// tables cannot express.
    fn map_characters(&self, text: &str, cluster_base: usize) -> Vec<ShapedGlyph> {
        let characters: Vec<(usize, char)> = text.char_indices().collect();
        let mut glyphs = Vec::with_capacity(characters.len() + 2);

        let mut index = 0;
        while index < characters.len() {
            let (offset, character) = characters[index];

            // Direction marks and embeddings say how a line is ordered and
            // have no glyph of their own; a `.notdef` box for one would be
            // wrong in every font.
            if is_bidi_control(character) {
                index += 1;
                continue;
            }

            // SARA AM (ำ) is a single character that renders as two marks: a
            // NIKHAHIT above and a SARA AA beside. When a tone mark was typed
            // between the base and the SARA AM — น + ้ + ำ, which is how น้ำ is
            // spelled — the NIKHAHIT belongs *below* that tone mark, not above
            // it. No GSUB rule can express that, because it moves a glyph
            // across one that was typed earlier, so it is done here.
            if character == SARA_AM {
                let previous_is_tone = glyphs
                    .last()
                    .and_then(|glyph: &ShapedGlyph| self.glyph_character(glyph.glyph))
                    .is_some_and(is_tone_mark);

                if previous_is_tone {
                    let tone = glyphs.pop().expect("checked just above");
                    self.push_glyph(&mut glyphs, NIKHAHIT, cluster_base + offset);
                    glyphs.push(tone);
                    self.push_glyph(&mut glyphs, SARA_AA, cluster_base + offset);
                    index += 1;
                    continue;
                }
                // With no intervening tone mark, the font's own `ccmp` handles
                // the decomposition, so the character is passed through.
            }

            self.push_glyph(&mut glyphs, character, cluster_base + offset);
            index += 1;
        }
        glyphs
    }

    fn push_glyph(&self, glyphs: &mut Vec<ShapedGlyph>, character: char, cluster: usize) {
        // Glyph 0 is `.notdef`, which is the correct thing to draw for a
        // character the face does not have: a visible box beats a silent gap.
        let glyph = self.glyph_for(character).unwrap_or(0);
        glyphs.push(ShapedGlyph::new(glyph, cluster, self.advance(glyph)));
    }

    /// Reverse lookup, used only by the SARA AM reordering to recognise a tone
    /// mark that has already been mapped. Linear over the Thai block, which is
    /// 87 characters.
    fn glyph_character(&self, glyph: u16) -> Option<char> {
        ('\u{0E00}'..='\u{0E7F}').find(|&candidate| self.glyph_for(candidate) == Some(glyph))
    }
}

/// The Unicode bidirectional format characters: marks, embeddings, overrides
/// and isolates (U+061C, U+200E, U+200F, U+202A..=U+202E, U+2066..=U+2069).
/// They are default-ignorable and are never drawn.
pub fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// U+0E33 THAI CHARACTER SARA AM.
pub const SARA_AM: char = '\u{0E33}';
/// U+0E4D THAI CHARACTER NIKHAHIT — the ring SARA AM decomposes into.
pub const NIKHAHIT: char = '\u{0E4D}';
/// U+0E32 THAI CHARACTER SARA AA — the tail SARA AM decomposes into.
pub const SARA_AA: char = '\u{0E32}';

/// The four Thai tone marks, plus THANTHAKHAT which behaves like one for
/// ordering purposes.
pub fn is_tone_mark(character: char) -> bool {
    matches!(character, '\u{0E48}'..='\u{0E4B}' | '\u{0E4C}')
}

/// Marks that attach to a base rather than occupying their own width.
///
/// Used for line breaking: a break must never fall between a base and the
/// marks that belong to it, or the mark lands on whatever begins the next line.
pub fn is_thai_combining(character: char) -> bool {
    matches!(
        character,
        '\u{0E31}'                  // MAI HAN AKAT
        | '\u{0E34}'..='\u{0E3A}'   // above and below vowels, PHINTHU
        | '\u{0E47}'..='\u{0E4E}'   // tone marks, THANTHAKHAT, NIKHAHIT, YAMAKKAN
    )
}

/// Whether a character belongs to the Thai block.
pub fn is_thai(character: char) -> bool {
    ('\u{0E00}'..='\u{0E7F}').contains(&character)
}

/// Index of a face inside a [`FontSet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceId(pub u8);

/// How a string should be set: which face, how large, how tracked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub face: FaceId,
    /// Pixel size (em height in device pixels).
    pub size: f32,
    /// Letter-spacing in ems, CSS semantics: added after every cluster,
    /// including the last.
    pub tracking: f32,
}

impl TextStyle {
    pub const fn new(face: FaceId, size: f32) -> TextStyle {
        TextStyle {
            face,
            size,
            tracking: 0.0,
        }
    }

    pub const fn tracked(face: FaceId, size: f32, tracking: f32) -> TextStyle {
        TextStyle {
            face,
            size,
            tracking,
        }
    }

    pub fn with_size(self, size: f32) -> TextStyle {
        TextStyle { size, ..self }
    }

    /// Letter-spacing in pixels at this style's size.
    pub fn tracking_px(&self) -> f32 {
        self.tracking * self.size
    }
}

/// The faces an application draws with, plus each face's fallback chain.
///
/// Faces are `&'static` so a shaped run can hold a plain reference; embed
/// them in a `OnceLock` (or leak a `Box`) and register the references here.
#[derive(Debug, Default)]
pub struct FontSet {
    faces: Vec<&'static Face>,
    fallbacks: Vec<Vec<FaceId>>,
}

impl FontSet {
    pub fn new() -> FontSet {
        FontSet::default()
    }

    /// Register a face and get its id. Fallbacks start empty.
    pub fn add(&mut self, face: &'static Face) -> FaceId {
        assert!(self.faces.len() < 255, "a FontSet holds at most 255 faces");
        self.faces.push(face);
        self.fallbacks.push(Vec::new());
        FaceId((self.faces.len() - 1) as u8)
    }

    /// Faces tried, in order, when `face` lacks a character.
    pub fn set_fallbacks(&mut self, face: FaceId, chain: &[FaceId]) {
        self.fallbacks[usize::from(face.0)] = chain.to_vec();
    }

    pub fn face(&self, id: FaceId) -> &'static Face {
        self.faces[usize::from(id.0)]
    }

    pub fn len(&self) -> usize {
        self.faces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// Which face draws `character` under `style`: the style's face if it
    /// covers it, else the first fallback that does, else the style's face
    /// (whose `.notdef` box is the right thing to show).
    pub fn face_for(&self, style: FaceId, character: char) -> (&'static Face, FaceId) {
        let primary = self.face(style);
        if primary.covers(character) {
            return (primary, style);
        }
        for &fallback in &self.fallbacks[usize::from(style.0)] {
            let face = self.face(fallback);
            if face.covers(character) {
                return (face, fallback);
            }
        }
        (primary, style)
    }

    /// The face and every fallback it can reach, for metrics that must be
    /// stable across whichever face a string happens to use.
    pub fn family(&self, style: FaceId) -> impl Iterator<Item = &'static Face> + '_ {
        std::iter::once(style)
            .chain(self.fallbacks[usize::from(style.0)].iter().copied())
            .map(move |id| self.face(id))
    }
}

/// The same two Noto faces `liminalkit-test-fonts` provides, for this crate's
/// own unit tests, which need them as *this* crate's `Face` rather than a
/// dependency's. Only under `cfg(test)`: the files live in the unpublished
/// fixtures crate (`crates/liminalkit-test-fonts/fonts/README.md` says what
/// they are), so a published copy of this crate builds without them and its
/// unit tests are run from the repository.
#[cfg(test)]
pub mod test_fonts {
    use super::*;
    use std::sync::OnceLock;

    pub fn thai() -> &'static Face {
        static FACE: OnceLock<Face> = OnceLock::new();
        FACE.get_or_init(|| {
            Face::from_bytes(include_bytes!(
                "../../liminalkit-test-fonts/fonts/NotoSansThai-Regular.ttf"
            ))
            .unwrap()
        })
    }

    pub fn latin() -> &'static Face {
        static FACE: OnceLock<Face> = OnceLock::new();
        FACE.get_or_init(|| {
            Face::from_bytes(include_bytes!(
                "../../liminalkit-test-fonts/fonts/NotoSans-Regular.ttf"
            ))
            .unwrap()
        })
    }

    pub const LATIN: FaceId = FaceId(0);
    pub const THAI: FaceId = FaceId(1);

    /// Latin first with Thai as fallback, and Thai with Latin as fallback.
    pub fn set() -> &'static FontSet {
        static SET: OnceLock<FontSet> = OnceLock::new();
        SET.get_or_init(|| {
            let mut set = FontSet::new();
            let latin = set.add(latin());
            let thai = set.add(thai());
            set.set_fallbacks(latin, &[thai]);
            set.set_fallbacks(thai, &[latin]);
            set
        })
    }

    /// Latin-styled text: what a receipt line looks like.
    pub const fn style(size: f32) -> TextStyle {
        TextStyle::new(LATIN, size)
    }
}

#[cfg(test)]
mod tests {
    use super::test_fonts::*;
    use super::*;

    #[test]
    fn a_direction_mark_has_no_glyph() {
        let plain = latin().shape("ab", 0);
        let marked = latin().shape("a\u{200F}b", 0);
        assert_eq!(marked.len(), plain.len());
        assert_eq!(
            marked.iter().map(|g| g.glyph).collect::<Vec<_>>(),
            plain.iter().map(|g| g.glyph).collect::<Vec<_>>()
        );
        // Clusters still index the original string.
        assert_eq!(marked[1].cluster, 4);
    }

    #[test]
    fn a_right_to_left_run_is_reversed_by_base_and_marks() {
        // ก + tone mark + า: the mark must stay with ก after the reversal.
        let glyphs = thai().shape_run("ก\u{0E49}า", 0, true);
        let clusters: Vec<usize> = glyphs.iter().map(|g| g.cluster).collect();
        assert_eq!(clusters, vec![6, 0, 3]);
        assert!(thai().is_mark(glyphs[2].glyph));
    }

    #[test]
    fn a_left_to_right_run_keeps_logical_order() {
        let glyphs = latin().shape_run("abc", 10, false);
        let clusters: Vec<usize> = glyphs.iter().map(|g| g.cluster).collect();
        assert_eq!(clusters, vec![10, 11, 12]);
    }

    #[test]
    fn both_fixture_faces_parse() {
        assert_eq!(thai().units_per_em(), 1000.0);
        assert!(latin().units_per_em() > 0.0);
        assert!(thai().ascender() > 0.0);
        assert!(thai().descender() < 0.0);
    }

    #[test]
    fn the_thai_face_has_the_lookups_the_shaper_will_use() {
        let face = thai();
        assert!(!face.substitution_lookups.is_empty());
        assert!(!face.positioning_lookups.is_empty());
    }

    #[test]
    fn characters_route_through_the_fallback_chain() {
        let set = set();
        assert!(std::ptr::eq(set.face_for(LATIN, 'ก').0, thai()));
        assert!(std::ptr::eq(set.face_for(LATIN, '฿').0, thai()));
        assert!(std::ptr::eq(set.face_for(LATIN, '1').0, latin()));
        assert!(std::ptr::eq(set.face_for(THAI, '1').0, latin()));
        assert!(std::ptr::eq(set.face_for(THAI, 'ก').0, thai()));
        // Nobody covers it: the style face draws its notdef box.
        assert!(std::ptr::eq(set.face_for(THAI, '\u{1F600}').0, thai()));
    }

    #[test]
    fn garbage_does_not_parse() {
        assert!(Face::from_bytes(b"not a font at all").is_none());
    }

    #[test]
    fn tracking_scales_with_size() {
        let style = TextStyle::tracked(LATIN, 10.0, 0.1);
        assert!((style.tracking_px() - 1.0).abs() < 1e-6);
    }
}
