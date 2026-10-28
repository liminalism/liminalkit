//! A TrueType (SFNT) parser, covering exactly what the two embedded faces use.
//!
//! Writing this rather than taking a font crate is a deliberate narrowing.
//! A general parser must survive every font in the world; this one has to read
//! two known files, both `glyf`-outlined, both shipped inside the binary. That
//! removes whole categories of work — CFF, variable fonts, colour tables,
//! collections — and leaves a few hundred lines that can be read in one sitting
//! and that no dependency update can change under a compliance document.
//!
//! What it is **not** is trusting. Every read is bounds-checked and every
//! malformed structure returns `None` rather than panicking, because "we
//! control the font" is a fact about today's binary, not a property the code
//! can rely on: fonts get replaced, and a panic in the receipt path at the
//! counter is worse than a missing glyph.

use std::collections::HashMap;

/// A big-endian cursor that never reads out of bounds.
#[derive(Debug, Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Reader<'a> {
        Reader { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn skip(&mut self, bytes: usize) {
        self.pos = self.pos.saturating_add(bytes);
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn u8(&mut self) -> Option<u8> {
        let byte = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(byte)
    }

    pub fn u16(&mut self) -> Option<u16> {
        let bytes = self.data.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    pub fn i16(&mut self) -> Option<i16> {
        self.u16().map(|value| value as i16)
    }

    pub fn u32(&mut self) -> Option<u32> {
        let bytes = self.data.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// An F2Dot14 fixed-point value, as component transforms use.
    pub fn f2dot14(&mut self) -> Option<f32> {
        self.i16().map(|raw| f32::from(raw) / 16384.0)
    }

    pub fn u16_at(&self, offset: usize) -> Option<u16> {
        let bytes = self.data.get(offset..offset + 2)?;
        Some(u16::from_be_bytes([bytes[0], bytes[1]]))
    }
}

/// A parsed font file: the table directory plus the bytes it points into.
#[derive(Debug, Clone)]
pub struct Sfnt<'a> {
    data: &'a [u8],
    tables: HashMap<[u8; 4], (usize, usize)>,
}

impl<'a> Sfnt<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Sfnt<'a>> {
        let mut reader = Reader::new(data);
        let version = reader.u32()?;
        // 0x00010000 is TrueType outlines; 'true' is the old Apple tag. `OTTO`
        // means CFF outlines, which this parser deliberately does not handle.
        if version != 0x0001_0000 && version != 0x7472_7565 {
            return None;
        }
        let table_count = reader.u16()?;
        reader.skip(6); // searchRange, entrySelector, rangeShift

        let mut tables = HashMap::with_capacity(usize::from(table_count));
        for _ in 0..table_count {
            let tag = [reader.u8()?, reader.u8()?, reader.u8()?, reader.u8()?];
            let _checksum = reader.u32()?;
            let offset = reader.u32()? as usize;
            let length = reader.u32()? as usize;
            if offset <= data.len() {
                // A length running past the file is clamped rather than
                // rejected: some shippers pad or truncate the last table, and
                // the readers below are bounds-checked anyway.
                tables.insert(tag, (offset, length.min(data.len() - offset)));
            }
        }
        Some(Sfnt { data, tables })
    }

    pub fn table(&self, tag: &[u8; 4]) -> Option<&'a [u8]> {
        let &(offset, length) = self.tables.get(tag)?;
        self.data.get(offset..offset + length)
    }

    pub fn has_table(&self, tag: &[u8; 4]) -> bool {
        self.tables.contains_key(tag)
    }
}

/// The handful of scalars from `head`, `hhea` and `maxp` that everything else
/// needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontMetrics {
    /// Design units per em. Every coordinate in the font is in these; dividing
    /// by it and multiplying by the pixel size is the whole of scaling.
    pub units_per_em: u16,
    pub glyph_count: u16,
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    /// 0 = `loca` holds u16 halves of the offset, 1 = u32 offsets.
    pub long_loca: bool,
    pub h_metric_count: u16,
}

impl FontMetrics {
    pub fn parse(sfnt: &Sfnt<'_>) -> Option<FontMetrics> {
        let head = sfnt.table(b"head")?;
        let head_reader = Reader::new(head);
        let units_per_em = head_reader.u16_at(18)?;
        let long_loca = head_reader.u16_at(50)? == 1;

        let maxp = sfnt.table(b"maxp")?;
        let glyph_count = Reader::new(maxp).u16_at(4)?;

        let hhea = sfnt.table(b"hhea")?;
        let hhea_reader = Reader::new(hhea);
        let ascender = hhea_reader.u16_at(4)? as i16;
        let descender = hhea_reader.u16_at(6)? as i16;
        let line_gap = hhea_reader.u16_at(8)? as i16;
        let h_metric_count = hhea_reader.u16_at(34)?;

        (units_per_em > 0).then_some(FontMetrics {
            units_per_em,
            glyph_count,
            ascender,
            descender,
            line_gap,
            long_loca,
            h_metric_count,
        })
    }
}

/// Codepoint → glyph index.
///
/// Both formats seen in practice for these faces: format 4 for the Basic
/// Multilingual Plane, format 12 for anything beyond it.
#[derive(Debug, Clone, Default)]
pub struct CharacterMap {
    map: HashMap<u32, u16>,
}

impl CharacterMap {
    pub fn parse(sfnt: &Sfnt<'_>) -> Option<CharacterMap> {
        let cmap = sfnt.table(b"cmap")?;
        let mut reader = Reader::new(cmap);
        reader.skip(2); // version
        let table_count = reader.u16()?;

        // Prefer a full-Unicode subtable, then a BMP one. Anything
        // platform-specific (Macintosh Roman, symbol) is ignored: these fonts
        // are read by codepoint, not by legacy byte value.
        let mut best: Option<(u8, usize)> = None;
        for _ in 0..table_count {
            let platform = reader.u16()?;
            let encoding = reader.u16()?;
            let offset = reader.u32()? as usize;
            let rank = match (platform, encoding) {
                (3, 10) | (0, 4) | (0, 6) => 2, // full Unicode
                (3, 1) | (0, 3) => 1,           // BMP
                _ => continue,
            };
            if best.is_none_or(|(best_rank, _)| rank > best_rank) {
                best = Some((rank, offset));
            }
        }

        let (_, offset) = best?;
        let mut sub = Reader::at(cmap, offset);
        let map = match sub.u16()? {
            4 => parse_cmap_format4(cmap, offset)?,
            12 => parse_cmap_format12(&mut sub)?,
            _ => return None,
        };
        Some(CharacterMap { map })
    }

    pub fn glyph(&self, codepoint: char) -> Option<u16> {
        self.map.get(&(codepoint as u32)).copied()
    }

    pub fn covers(&self, codepoint: char) -> bool {
        self.map.contains_key(&(codepoint as u32))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

fn parse_cmap_format4(table: &[u8], offset: usize) -> Option<HashMap<u32, u16>> {
    let mut reader = Reader::at(table, offset);
    let _format = reader.u16()?;
    let _length = reader.u16()?;
    let _language = reader.u16()?;
    let segment_count = usize::from(reader.u16()?) / 2;
    reader.skip(6); // searchRange, entrySelector, rangeShift

    let end_codes_at = reader.pos();
    let start_codes_at = end_codes_at + segment_count * 2 + 2; // + reservedPad
    let deltas_at = start_codes_at + segment_count * 2;
    let range_offsets_at = deltas_at + segment_count * 2;

    let mut map = HashMap::new();
    for segment in 0..segment_count {
        let end = Reader::new(table).u16_at(end_codes_at + segment * 2)?;
        let start = Reader::new(table).u16_at(start_codes_at + segment * 2)?;
        let delta = Reader::new(table).u16_at(deltas_at + segment * 2)?;
        let range_offset = Reader::new(table).u16_at(range_offsets_at + segment * 2)?;
        if start > end {
            continue;
        }
        for code in start..=end {
            // 0xFFFF is the required terminating segment, not a character.
            if code == 0xFFFF {
                continue;
            }
            let glyph = if range_offset == 0 {
                code.wrapping_add(delta)
            } else {
                // The offset is measured from the range-offset slot itself,
                // which is the awkward part of this format.
                let slot = range_offsets_at
                    + segment * 2
                    + usize::from(range_offset)
                    + usize::from(code - start) * 2;
                match Reader::new(table).u16_at(slot) {
                    Some(0) | None => continue,
                    Some(glyph) => glyph.wrapping_add(delta),
                }
            };
            if glyph != 0 {
                map.insert(u32::from(code), glyph);
            }
        }
    }
    Some(map)
}

fn parse_cmap_format12(reader: &mut Reader<'_>) -> Option<HashMap<u32, u16>> {
    reader.skip(2); // reserved
    let _length = reader.u32()?;
    let _language = reader.u32()?;
    let group_count = reader.u32()?;

    let mut map = HashMap::new();
    for _ in 0..group_count {
        let start = reader.u32()?;
        let end = reader.u32()?;
        let start_glyph = reader.u32()?;
        if start > end || end.saturating_sub(start) > 0x10_FFFF {
            continue;
        }
        for code in start..=end {
            let glyph = start_glyph + (code - start);
            if let Ok(glyph) = u16::try_from(glyph) {
                if glyph != 0 {
                    map.insert(code, glyph);
                }
            }
        }
    }
    Some(map)
}

/// Horizontal advances, in design units.
#[derive(Debug, Clone)]
pub struct HorizontalMetrics {
    advances: Vec<u16>,
    left_side_bearings: Vec<i16>,
}

impl HorizontalMetrics {
    pub fn parse(sfnt: &Sfnt<'_>, metrics: &FontMetrics) -> Option<HorizontalMetrics> {
        let hmtx = sfnt.table(b"hmtx")?;
        let mut reader = Reader::new(hmtx);
        let count = usize::from(metrics.h_metric_count).min(usize::from(metrics.glyph_count));
        let mut advances = Vec::with_capacity(count);
        let mut left_side_bearings = Vec::with_capacity(usize::from(metrics.glyph_count));
        for _ in 0..count {
            advances.push(reader.u16()?);
            left_side_bearings.push(reader.i16()?);
        }
        // Monospaced tails: glyphs past `numberOfHMetrics` share the last
        // advance and carry only their own bearing.
        for _ in count..usize::from(metrics.glyph_count) {
            left_side_bearings.push(reader.i16().unwrap_or(0));
        }
        Some(HorizontalMetrics {
            advances,
            left_side_bearings,
        })
    }

    /// Advance for a glyph, in design units.
    pub fn advance(&self, glyph: u16) -> u16 {
        let index = usize::from(glyph);
        if index < self.advances.len() {
            self.advances[index]
        } else {
            self.advances.last().copied().unwrap_or(0)
        }
    }

    pub fn left_side_bearing(&self, glyph: u16) -> i16 {
        self.left_side_bearings
            .get(usize::from(glyph))
            .copied()
            .unwrap_or(0)
    }
}

/// One point of a quadratic contour, in design units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurvePoint {
    pub x: f32,
    pub y: f32,
    /// Off-curve points are quadratic control points.
    pub on_curve: bool,
}

/// A glyph outline: closed contours of quadratic curves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outline {
    pub contours: Vec<Vec<CurvePoint>>,
}

impl Outline {
    pub fn is_empty(&self) -> bool {
        self.contours.is_empty()
    }

    /// Bounding box in design units, or `None` for an empty outline.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let mut bounds: Option<[f32; 4]> = None;
        for contour in &self.contours {
            for point in contour {
                bounds = Some(match bounds {
                    None => [point.x, point.y, point.x, point.y],
                    Some([x0, y0, x1, y1]) => [
                        x0.min(point.x),
                        y0.min(point.y),
                        x1.max(point.x),
                        y1.max(point.y),
                    ],
                });
            }
        }
        bounds
    }

    /// Shift every point, for mark placement.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        for contour in &mut self.contours {
            for point in contour {
                point.x += dx;
                point.y += dy;
            }
        }
    }
}

/// The `glyf`/`loca` pair: glyph outlines by index.
#[derive(Debug, Clone)]
pub struct GlyphSource<'a> {
    glyf: &'a [u8],
    offsets: Vec<u32>,
}

/// How deep a composite glyph may nest before it is treated as malformed.
/// Real composites are one or two levels; a cycle would otherwise recurse
/// until the stack ran out.
const MAX_COMPOSITE_DEPTH: usize = 8;

impl<'a> GlyphSource<'a> {
    pub fn parse(sfnt: &Sfnt<'a>, metrics: &FontMetrics) -> Option<GlyphSource<'a>> {
        let glyf = sfnt.table(b"glyf")?;
        let loca = sfnt.table(b"loca")?;
        let count = usize::from(metrics.glyph_count) + 1;
        let mut reader = Reader::new(loca);
        let mut offsets = Vec::with_capacity(count);
        for _ in 0..count {
            let offset = if metrics.long_loca {
                reader.u32()?
            } else {
                // Short format stores halves, so a 16-bit value reaches 128 KB.
                u32::from(reader.u16()?) * 2
            };
            offsets.push(offset);
        }
        Some(GlyphSource { glyf, offsets })
    }

    /// The outline of one glyph, in design units. An empty outline (a space)
    /// is `Some` with no contours, not `None`; `None` means the glyph index is
    /// out of range or the data is malformed.
    pub fn outline(&self, glyph: u16) -> Option<Outline> {
        self.outline_at_depth(glyph, 0)
    }

    fn outline_at_depth(&self, glyph: u16, depth: usize) -> Option<Outline> {
        if depth > MAX_COMPOSITE_DEPTH {
            return None;
        }
        let index = usize::from(glyph);
        let start = *self.offsets.get(index)? as usize;
        let end = *self.offsets.get(index + 1)? as usize;
        if end <= start {
            // Zero length is a legitimately empty glyph, such as a space.
            return Some(Outline::default());
        }
        let data = self.glyf.get(start..end)?;
        let mut reader = Reader::new(data);
        let contour_count = reader.i16()?;
        reader.skip(8); // xMin, yMin, xMax, yMax

        if contour_count >= 0 {
            parse_simple_glyph(&mut reader, contour_count as usize)
        } else {
            self.parse_composite_glyph(&mut reader, depth)
        }
    }

    fn parse_composite_glyph(&self, reader: &mut Reader<'_>, depth: usize) -> Option<Outline> {
        const ARGS_ARE_WORDS: u16 = 0x0001;
        const ARGS_ARE_XY: u16 = 0x0002;
        const HAVE_SCALE: u16 = 0x0008;
        const MORE_COMPONENTS: u16 = 0x0020;
        const HAVE_XY_SCALE: u16 = 0x0040;
        const HAVE_TWO_BY_TWO: u16 = 0x0080;

        let mut combined = Outline::default();
        loop {
            let flags = reader.u16()?;
            let component = reader.u16()?;

            let (dx, dy) = if flags & ARGS_ARE_WORDS != 0 {
                (f32::from(reader.i16()?), f32::from(reader.i16()?))
            } else {
                (f32::from(reader.u8()? as i8), f32::from(reader.u8()? as i8))
            };
            // Point-matched components (args as point indices rather than
            // offsets) are vanishingly rare and not used by these faces; the
            // offset is taken as zero rather than guessed at.
            let (dx, dy) = if flags & ARGS_ARE_XY != 0 {
                (dx, dy)
            } else {
                (0.0, 0.0)
            };

            let [a, b, c, d] = if flags & HAVE_TWO_BY_TWO != 0 {
                [
                    reader.f2dot14()?,
                    reader.f2dot14()?,
                    reader.f2dot14()?,
                    reader.f2dot14()?,
                ]
            } else if flags & HAVE_XY_SCALE != 0 {
                let (x, y) = (reader.f2dot14()?, reader.f2dot14()?);
                [x, 0.0, 0.0, y]
            } else if flags & HAVE_SCALE != 0 {
                let scale = reader.f2dot14()?;
                [scale, 0.0, 0.0, scale]
            } else {
                [1.0, 0.0, 0.0, 1.0]
            };

            if let Some(part) = self.outline_at_depth(component, depth + 1) {
                for contour in part.contours {
                    combined.contours.push(
                        contour
                            .into_iter()
                            .map(|point| CurvePoint {
                                x: a * point.x + c * point.y + dx,
                                y: b * point.x + d * point.y + dy,
                                on_curve: point.on_curve,
                            })
                            .collect(),
                    );
                }
            }

            if flags & MORE_COMPONENTS == 0 {
                break;
            }
        }
        Some(combined)
    }
}

fn parse_simple_glyph(reader: &mut Reader<'_>, contour_count: usize) -> Option<Outline> {
    const ON_CURVE: u8 = 0x01;
    const X_SHORT: u8 = 0x02;
    const Y_SHORT: u8 = 0x04;
    const REPEAT: u8 = 0x08;
    const X_SAME_OR_POSITIVE: u8 = 0x10;
    const Y_SAME_OR_POSITIVE: u8 = 0x20;

    let mut end_points = Vec::with_capacity(contour_count);
    for _ in 0..contour_count {
        end_points.push(reader.u16()?);
    }
    let point_count = match end_points.last() {
        Some(&last) => usize::from(last) + 1,
        None => return Some(Outline::default()),
    };

    let instruction_length = usize::from(reader.u16()?);
    reader.skip(instruction_length);

    // Flags, run-length encoded.
    let mut flags = Vec::with_capacity(point_count);
    while flags.len() < point_count {
        let flag = reader.u8()?;
        flags.push(flag);
        if flag & REPEAT != 0 {
            let repeats = reader.u8()?;
            for _ in 0..repeats {
                if flags.len() >= point_count {
                    break;
                }
                flags.push(flag);
            }
        }
    }

    // Coordinates, stored as deltas in two separate runs.
    let mut xs = Vec::with_capacity(point_count);
    let mut x = 0i32;
    for &flag in &flags {
        if flag & X_SHORT != 0 {
            let delta = i32::from(reader.u8()?);
            x += if flag & X_SAME_OR_POSITIVE != 0 {
                delta
            } else {
                -delta
            };
        } else if flag & X_SAME_OR_POSITIVE == 0 {
            x += i32::from(reader.i16()?);
        }
        xs.push(x);
    }

    let mut ys = Vec::with_capacity(point_count);
    let mut y = 0i32;
    for &flag in &flags {
        if flag & Y_SHORT != 0 {
            let delta = i32::from(reader.u8()?);
            y += if flag & Y_SAME_OR_POSITIVE != 0 {
                delta
            } else {
                -delta
            };
        } else if flag & Y_SAME_OR_POSITIVE == 0 {
            y += i32::from(reader.i16()?);
        }
        ys.push(y);
    }

    let mut contours = Vec::with_capacity(contour_count);
    let mut start = 0usize;
    for &end in &end_points {
        let end = usize::from(end);
        if end < start || end >= point_count {
            break;
        }
        let contour: Vec<CurvePoint> = (start..=end)
            .map(|index| CurvePoint {
                x: xs[index] as f32,
                y: ys[index] as f32,
                on_curve: flags[index] & ON_CURVE != 0,
            })
            .collect();
        if contour.len() >= 2 {
            contours.push(contour);
        }
        start = end + 1;
    }

    Some(Outline { contours })
}

#[cfg(test)]
mod tests {
    use super::*;

    const THAI: &[u8] = include_bytes!("../test-fonts/NotoSansThai-Regular.ttf");
    const LATIN: &[u8] = include_bytes!("../test-fonts/NotoSans-Regular.ttf");

    fn thai() -> Sfnt<'static> {
        Sfnt::parse(THAI).expect("the embedded Thai face must parse")
    }

    #[test]
    fn the_embedded_faces_parse_and_carry_the_tables_the_shaper_needs() {
        for data in [THAI, LATIN] {
            let sfnt = Sfnt::parse(data).expect("a Noto face");
            for tag in [
                b"head", b"maxp", b"hhea", b"hmtx", b"cmap", b"loca", b"glyf",
            ] {
                assert!(
                    sfnt.has_table(tag),
                    "missing {}",
                    String::from_utf8_lossy(tag)
                );
            }
        }
        // Thai specifically needs the layout tables.
        let sfnt = thai();
        assert!(sfnt.has_table(b"GSUB"));
        assert!(sfnt.has_table(b"GPOS"));
        assert!(sfnt.has_table(b"GDEF"));
    }

    #[test]
    fn a_non_sfnt_file_is_refused_rather_than_misread() {
        assert!(Sfnt::parse(b"not a font at all").is_none());
        assert!(Sfnt::parse(&[]).is_none());
        // `OTTO` is a real font, but with CFF outlines this parser cannot read.
        assert!(Sfnt::parse(b"OTTO\0\0\0\0\0\0\0\0").is_none());
    }

    #[test]
    fn metrics_come_out_of_the_header_tables() {
        let metrics = FontMetrics::parse(&thai()).unwrap();
        assert_eq!(metrics.units_per_em, 1000);
        assert_eq!(metrics.glyph_count, 140);
        assert!(metrics.ascender > 0);
        assert!(metrics.descender < 0);
    }

    #[test]
    fn the_character_map_covers_thai_but_not_latin() {
        let map = CharacterMap::parse(&thai()).unwrap();
        // The reason two faces are embedded, asserted rather than assumed.
        for thai_char in ['ผ', 'ด', 'ก', 'ะ', 'เ', 'พ', 'ร', 'า', '่', 'ำ'] {
            assert!(map.covers(thai_char), "Thai face should cover {thai_char}");
        }
        for latin in ['A', 'z', '0', '9'] {
            assert!(
                !map.covers(latin),
                "the Thai face unexpectedly covers {latin}"
            );
        }
        // It does carry Thai digits, which are a different set of codepoints.
        assert!(map.covers('๐'));
        assert!(map.covers('๙'));
    }

    #[test]
    fn the_latin_face_covers_the_digits_a_price_needs() {
        let sfnt = Sfnt::parse(LATIN).unwrap();
        let map = CharacterMap::parse(&sfnt).unwrap();
        for digit in "0123456789.,".chars() {
            assert!(map.covers(digit), "Latin face should cover {digit}");
        }
    }

    #[test]
    fn the_baht_sign_lives_in_the_thai_face_so_every_price_is_mixed() {
        // ฿ is U+0E3F — inside the Thai block, not with the currency symbols.
        // So `฿1,240.50` needs both faces on one line: the sign from Thai, the
        // digits from Latin. Every price the system prints is a mixed run, and
        // the font-selection rule has to handle that rather than treat it as an
        // edge case.
        let thai = CharacterMap::parse(&thai()).unwrap();
        let latin = CharacterMap::parse(&Sfnt::parse(LATIN).unwrap()).unwrap();
        assert!(thai.covers('฿'));
        assert!(!latin.covers('฿'));
        assert!(!thai.covers('1'));
        assert!(latin.covers('1'));
    }

    #[test]
    fn glyph_outlines_come_back_with_plausible_geometry() {
        let sfnt = thai();
        let metrics = FontMetrics::parse(&sfnt).unwrap();
        let map = CharacterMap::parse(&sfnt).unwrap();
        let glyphs = GlyphSource::parse(&sfnt, &metrics).unwrap();

        let glyph = map.glyph('ก').unwrap();
        let outline = glyphs.outline(glyph).unwrap();
        assert!(!outline.is_empty(), "ก should have contours");

        let [x0, y0, x1, y1] = outline.bounds().unwrap();
        assert!(x1 > x0 && y1 > y0);
        // A consonant sits on the baseline and rises towards the em height.
        assert!(
            (-50.0..=50.0).contains(&y0),
            "baseline-ish bottom, got {y0}"
        );
        assert!(y1 > 300.0, "should reach x-height, got {y1}");
        assert!(x1 <= f32::from(metrics.units_per_em));
    }

    #[test]
    fn an_above_vowel_sits_entirely_above_the_x_height() {
        let sfnt = thai();
        let metrics = FontMetrics::parse(&sfnt).unwrap();
        let map = CharacterMap::parse(&sfnt).unwrap();
        let glyphs = GlyphSource::parse(&sfnt, &metrics).unwrap();

        // SARA II (ี) is an above-vowel; MAI EK (่) is a tone mark.
        for mark in ['ี', '่'] {
            let outline = glyphs.outline(map.glyph(mark).unwrap()).unwrap();
            let [_, y0, _, _] = outline.bounds().unwrap();
            assert!(y0 > 300.0, "{mark} should sit high, bottom was {y0}");
        }
        // SARA U (ุ) is a below-vowel and hangs under the baseline.
        let outline = glyphs.outline(map.glyph('ุ').unwrap()).unwrap();
        let [_, _, _, y1] = outline.bounds().unwrap();
        assert!(y1 < 100.0, "ุ should hang low, top was {y1}");
    }

    #[test]
    fn advances_are_positive_for_consonants_and_zero_for_marks() {
        let sfnt = thai();
        let metrics = FontMetrics::parse(&sfnt).unwrap();
        let map = CharacterMap::parse(&sfnt).unwrap();
        let hmtx = HorizontalMetrics::parse(&sfnt, &metrics).unwrap();

        assert!(hmtx.advance(map.glyph('ก').unwrap()) > 0);
        // Combining marks take no width of their own; that is what makes them
        // stack rather than sit beside their base.
        assert_eq!(hmtx.advance(map.glyph('่').unwrap()), 0);
        assert_eq!(hmtx.advance(map.glyph('ุ').unwrap()), 0);
    }

    #[test]
    fn an_out_of_range_glyph_index_is_none_rather_than_a_panic() {
        let sfnt = thai();
        let metrics = FontMetrics::parse(&sfnt).unwrap();
        let glyphs = GlyphSource::parse(&sfnt, &metrics).unwrap();
        assert!(glyphs.outline(9_999).is_none());
    }

    #[test]
    fn a_truncated_font_fails_cleanly_at_every_length() {
        // Every prefix of a real font is malformed in a different way. None of
        // them may panic: a corrupt font must degrade to missing glyphs, not
        // take down the counter.
        for length in (0..THAI.len()).step_by(997) {
            let truncated = &THAI[..length];
            if let Some(sfnt) = Sfnt::parse(truncated) {
                if let Some(metrics) = FontMetrics::parse(&sfnt) {
                    let _ = CharacterMap::parse(&sfnt);
                    if let Some(glyphs) = GlyphSource::parse(&sfnt, &metrics) {
                        for glyph in 0..metrics.glyph_count.min(200) {
                            let _ = glyphs.outline(glyph);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_reader_never_reads_past_the_end() {
        let mut reader = Reader::new(&[0x12, 0x34]);
        assert_eq!(reader.u16(), Some(0x1234));
        assert_eq!(reader.u16(), None);
        assert_eq!(reader.u8(), None);
        // A skip past the end leaves it at the end rather than overflowing.
        reader.skip(usize::MAX);
        assert_eq!(reader.remaining(), 0);
    }
}
