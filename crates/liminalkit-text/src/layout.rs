//! OpenType layout: the GSUB, GPOS and GDEF tables.
//!
//! This is what makes Thai render correctly rather than approximately. The
//! naive approach — draw each character at the pen and nudge combining marks
//! by hand-tuned offsets — falls apart on exactly the cases Thai is full of:
//!
//! * **ป้า** — a tone mark over ป, whose ascender occupies the space the mark
//!   would normally take. The font has a lowered alternate for the mark, and a
//!   contextual substitution that selects it.
//! * **ปุ๋ย** — a below-vowel *and* a tone mark on a consonant with a
//!   descender, needing both a lowered mark and a shortened base.
//! * **น้ำ** — SARA AM, which decomposes into two glyphs, one of which must
//!   sort before the tone mark that was typed after it.
//!
//! The font already knows all of this. It is in the `ccmp` substitutions and
//! the `mark`/`mkmk` anchors. Reading them is less code than the fudge factors
//! would be, and it stays correct when the font is replaced.
//!
//! Scope is exactly what the embedded face uses, verified by
//! [`tests::the_face_uses_only_lookups_this_module_implements`]:
//!
//! | Table | Lookup types |
//! |---|---|
//! | GSUB | 1 (single, both formats), 2 (multiple), 3 (alternate), 4 (ligature), 6 format 3 (chained context) |
//! | GPOS | 2 (pair, both formats), 4 (mark-to-base), 6 (mark-to-mark) |
//!
//! Anything else is skipped rather than mis-applied.

use crate::sfnt::Reader;

/// A glyph in the buffer being shaped. Positions are in font design units;
/// scaling to pixels happens once, at the end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    pub glyph: u16,
    /// Which character of the source string this came from. Two glyphs from
    /// one character (SARA AM) share a cluster, which is what keeps line
    /// breaking from splitting them.
    pub cluster: usize,
    pub x_offset: f32,
    pub y_offset: f32,
    pub x_advance: f32,
}

impl ShapedGlyph {
    pub fn new(glyph: u16, cluster: usize, x_advance: f32) -> ShapedGlyph {
        ShapedGlyph {
            glyph,
            cluster,
            x_offset: 0.0,
            y_offset: 0.0,
            x_advance,
        }
    }
}

// --- GDEF ------------------------------------------------------------------

/// Glyph classes from GDEF. The values are the specification's.
pub const CLASS_BASE: u16 = 1;
pub const CLASS_LIGATURE: u16 = 2;
pub const CLASS_MARK: u16 = 3;
pub const CLASS_COMPONENT: u16 = 4;

/// Lookup flag bits that matter here.
const IGNORE_BASE_GLYPHS: u16 = 0x0002;
const IGNORE_LIGATURES: u16 = 0x0004;
const IGNORE_MARKS: u16 = 0x0008;
const USE_MARK_FILTERING_SET: u16 = 0x0010;

/// Which glyphs are bases, which are marks, and which mark sets exist.
///
/// Without this a kerning pair would be broken by an intervening tone mark,
/// and a mark-to-mark attachment would have no way to find the mark below it.
#[derive(Debug, Clone)]
pub struct Gdef<'a> {
    data: &'a [u8],
    glyph_class_def: Option<usize>,
    mark_glyph_sets: Option<usize>,
}

impl<'a> Gdef<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Gdef<'a>> {
        let mut reader = Reader::new(data);
        let version = reader.u32()?;
        let glyph_class_def = reader.u16()? as usize;
        let _attach_list = reader.u16()?;
        let _lig_caret_list = reader.u16()?;
        let _mark_attach_class_def = reader.u16()?;
        // Mark glyph sets arrived in GDEF 1.2; earlier tables simply end here.
        let mark_glyph_sets = if version >= 0x0001_0002 {
            match reader.u16()? {
                0 => None,
                offset => Some(usize::from(offset)),
            }
        } else {
            None
        };
        Some(Gdef {
            data,
            glyph_class_def: (glyph_class_def != 0).then_some(glyph_class_def),
            mark_glyph_sets,
        })
    }

    /// An empty GDEF, for a face that has none. Everything is then a base,
    /// which is the right default: without mark information, treating a glyph
    /// as a mark would move it.
    pub fn empty() -> Gdef<'static> {
        Gdef {
            data: &[],
            glyph_class_def: None,
            mark_glyph_sets: None,
        }
    }

    pub fn class_of(&self, glyph: u16) -> u16 {
        match self.glyph_class_def {
            Some(offset) => class_of(self.data, offset, glyph),
            None => 0,
        }
    }

    pub fn is_mark(&self, glyph: u16) -> bool {
        self.class_of(glyph) == CLASS_MARK
    }

    /// Whether a glyph belongs to a numbered mark filtering set. Used by
    /// mark-to-mark lookups to attach only to the right kind of mark below —
    /// a tone mark stacks on an above-vowel, not on a below-vowel.
    pub fn in_mark_set(&self, set: u16, glyph: u16) -> bool {
        let Some(sets) = self.mark_glyph_sets else {
            return true;
        };
        let reader = Reader::new(self.data);
        let Some(count) = reader.u16_at(sets + 2) else {
            return true;
        };
        if set >= count {
            return true;
        }
        let slot = sets + 4 + usize::from(set) * 4;
        let Some(bytes) = self.data.get(slot..slot + 4) else {
            return true;
        };
        let offset = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        coverage_index(self.data, sets + offset, glyph).is_some()
    }
}

// --- Common structures -----------------------------------------------------

/// Position of a glyph within a coverage table, or `None` if absent.
fn coverage_index(data: &[u8], offset: usize, glyph: u16) -> Option<u16> {
    let reader = Reader::new(data);
    match reader.u16_at(offset)? {
        1 => {
            let count = reader.u16_at(offset + 2)?;
            // Sorted, so this could be a binary search; coverage tables in a
            // 140-glyph face are short enough that it would not pay.
            (0..count)
                .find(|&index| reader.u16_at(offset + 4 + usize::from(index) * 2) == Some(glyph))
        }
        2 => {
            let count = reader.u16_at(offset + 2)?;
            (0..count).find_map(|index| {
                let record = offset + 4 + usize::from(index) * 6;
                let start = reader.u16_at(record)?;
                let end = reader.u16_at(record + 2)?;
                let first = reader.u16_at(record + 4)?;
                (start..=end)
                    .contains(&glyph)
                    .then(|| first + (glyph - start))
            })
        }
        _ => None,
    }
}

/// Class of a glyph in a class-definition table. Unlisted glyphs are class 0.
fn class_of(data: &[u8], offset: usize, glyph: u16) -> u16 {
    let reader = Reader::new(data);
    match reader.u16_at(offset) {
        Some(1) => {
            let Some(start) = reader.u16_at(offset + 2) else {
                return 0;
            };
            let Some(count) = reader.u16_at(offset + 4) else {
                return 0;
            };
            if glyph < start || glyph >= start + count {
                return 0;
            }
            reader
                .u16_at(offset + 6 + usize::from(glyph - start) * 2)
                .unwrap_or(0)
        }
        Some(2) => {
            let Some(count) = reader.u16_at(offset + 2) else {
                return 0;
            };
            for index in 0..count {
                let record = offset + 4 + usize::from(index) * 6;
                let (Some(start), Some(end), Some(class)) = (
                    reader.u16_at(record),
                    reader.u16_at(record + 2),
                    reader.u16_at(record + 4),
                ) else {
                    return 0;
                };
                if (start..=end).contains(&glyph) {
                    return class;
                }
            }
            0
        }
        _ => 0,
    }
}

/// An anchor point, in design units. Formats 1 and 2 differ only in a hinting
/// hint we do not use; format 3 adds device tables, which only matter for
/// hinted bitmap sizes.
fn anchor_at(data: &[u8], offset: usize) -> Option<(f32, f32)> {
    let reader = Reader::new(data);
    let _format = reader.u16_at(offset)?;
    let x = reader.u16_at(offset + 2)? as i16;
    let y = reader.u16_at(offset + 4)? as i16;
    Some((f32::from(x), f32::from(y)))
}

/// How many bytes a value record of this format occupies.
fn value_record_size(format: u16) -> usize {
    usize::from(format.count_ones() as u16) * 2
}

/// The x-placement and x-advance out of a value record, ignoring the vertical
/// and device fields that horizontal Thai text does not use.
fn value_record(data: &[u8], offset: usize, format: u16) -> (f32, f32) {
    let reader = Reader::new(data);
    let mut cursor = offset;
    let mut x_placement = 0.0;
    let mut x_advance = 0.0;
    for bit in 0..8u16 {
        if format & (1 << bit) == 0 {
            continue;
        }
        let value = reader.u16_at(cursor).unwrap_or(0) as i16;
        match bit {
            0 => x_placement = f32::from(value),
            2 => x_advance = f32::from(value),
            _ => {}
        }
        cursor += 2;
    }
    (x_placement, x_advance)
}

// --- Lookup lists ----------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Lookup {
    pub kind: u16,
    pub flag: u16,
    /// Present when the flag sets `USE_MARK_FILTERING_SET`.
    pub mark_filtering_set: Option<u16>,
    /// Absolute offsets into the table's bytes.
    pub subtables: Vec<usize>,
}

impl Lookup {
    /// Whether a glyph is invisible to this lookup.
    fn skips(&self, glyph: u16, gdef: &Gdef<'_>) -> bool {
        let class = gdef.class_of(glyph);
        if self.flag & IGNORE_MARKS != 0 && class == CLASS_MARK {
            return true;
        }
        if self.flag & IGNORE_BASE_GLYPHS != 0 && class == CLASS_BASE {
            return true;
        }
        if self.flag & IGNORE_LIGATURES != 0 && class == CLASS_LIGATURE {
            return true;
        }
        if let Some(set) = self.mark_filtering_set {
            if class == CLASS_MARK && !gdef.in_mark_set(set, glyph) {
                return true;
            }
        }
        false
    }
}

/// A parsed GSUB or GPOS table: its lookups, and which features use them.
#[derive(Debug, Clone)]
pub struct LayoutTable<'a> {
    data: &'a [u8],
    lookups: Vec<Lookup>,
    features: Vec<([u8; 4], Vec<u16>)>,
}

impl<'a> LayoutTable<'a> {
    pub fn parse(data: &'a [u8]) -> Option<LayoutTable<'a>> {
        let reader = Reader::new(data);
        let _version = reader.u16_at(0)?;
        let feature_list = usize::from(reader.u16_at(6)?);
        let lookup_list = usize::from(reader.u16_at(8)?);

        // Features.
        let feature_count = reader.u16_at(feature_list)?;
        let mut features = Vec::with_capacity(usize::from(feature_count));
        for index in 0..feature_count {
            let record = feature_list + 2 + usize::from(index) * 6;
            let tag = data.get(record..record + 4)?;
            let tag = [tag[0], tag[1], tag[2], tag[3]];
            let feature = feature_list + usize::from(reader.u16_at(record + 4)?);
            let lookup_count = reader.u16_at(feature + 2)?;
            let mut indices = Vec::with_capacity(usize::from(lookup_count));
            for lookup in 0..lookup_count {
                indices.push(reader.u16_at(feature + 4 + usize::from(lookup) * 2)?);
            }
            features.push((tag, indices));
        }

        // Lookups.
        let lookup_count = reader.u16_at(lookup_list)?;
        let mut lookups = Vec::with_capacity(usize::from(lookup_count));
        for index in 0..lookup_count {
            let lookup =
                lookup_list + usize::from(reader.u16_at(lookup_list + 2 + usize::from(index) * 2)?);
            let kind = reader.u16_at(lookup)?;
            let flag = reader.u16_at(lookup + 2)?;
            let subtable_count = reader.u16_at(lookup + 4)?;
            let mut subtables = Vec::with_capacity(usize::from(subtable_count));
            for sub in 0..subtable_count {
                subtables
                    .push(lookup + usize::from(reader.u16_at(lookup + 6 + usize::from(sub) * 2)?));
            }
            let mark_filtering_set = if flag & USE_MARK_FILTERING_SET != 0 {
                reader.u16_at(lookup + 6 + usize::from(subtable_count) * 2)
            } else {
                None
            };
            lookups.push(Lookup {
                kind,
                flag,
                mark_filtering_set,
                subtables,
            });
        }

        Some(LayoutTable {
            data,
            lookups,
            features,
        })
    }

    pub fn lookups(&self) -> &[Lookup] {
        &self.lookups
    }

    /// Lookup indices used by the named features, in lookup-list order.
    ///
    /// The specification requires lookups to run in list order, not feature
    /// order: a font author who wanted a different sequence would have
    /// numbered the lookups differently.
    pub fn lookups_for(&self, tags: &[&[u8; 4]]) -> Vec<u16> {
        let mut indices: Vec<u16> = self
            .features
            .iter()
            .filter(|(tag, _)| tags.contains(&tag))
            .flat_map(|(_, lookups)| lookups.iter().copied())
            .collect();
        indices.sort_unstable();
        indices.dedup();
        indices
    }

    pub fn feature_tags(&self) -> Vec<String> {
        self.features
            .iter()
            .map(|(tag, _)| String::from_utf8_lossy(tag).into_owned())
            .collect()
    }

    /// Next glyph position this lookup can see, at or after `from`.
    fn next_visible(
        &self,
        glyphs: &[ShapedGlyph],
        lookup: &Lookup,
        gdef: &Gdef<'_>,
        from: usize,
    ) -> Option<usize> {
        (from..glyphs.len()).find(|&index| !lookup.skips(glyphs[index].glyph, gdef))
    }

    /// Previous glyph position this lookup can see, strictly before `before`.
    fn previous_visible(
        &self,
        glyphs: &[ShapedGlyph],
        lookup: &Lookup,
        gdef: &Gdef<'_>,
        before: usize,
    ) -> Option<usize> {
        (0..before)
            .rev()
            .find(|&index| !lookup.skips(glyphs[index].glyph, gdef))
    }
}

// --- GSUB ------------------------------------------------------------------

/// Apply a set of substitution lookups to the buffer.
pub fn apply_substitutions(
    table: &LayoutTable<'_>,
    gdef: &Gdef<'_>,
    lookup_indices: &[u16],
    glyphs: &mut Vec<ShapedGlyph>,
) {
    for &index in lookup_indices {
        apply_substitution_lookup(table, gdef, usize::from(index), glyphs);
    }
}

fn apply_substitution_lookup(
    table: &LayoutTable<'_>,
    gdef: &Gdef<'_>,
    index: usize,
    glyphs: &mut Vec<ShapedGlyph>,
) {
    let Some(lookup) = table.lookups.get(index).cloned() else {
        return;
    };
    let mut position = 0;
    while position < glyphs.len() {
        let consumed = apply_substitution_at(table, gdef, &lookup, index, glyphs, position);
        // Always advance: a lookup that matched nothing still moves on, and a
        // lookup that matched must not be able to re-match its own output.
        position += consumed.max(1);
    }
}

/// Try every subtable of one lookup at one position. Returns how many glyph
/// positions were consumed, or 0 if nothing matched.
fn apply_substitution_at(
    table: &LayoutTable<'_>,
    gdef: &Gdef<'_>,
    lookup: &Lookup,
    lookup_index: usize,
    glyphs: &mut Vec<ShapedGlyph>,
    position: usize,
) -> usize {
    if lookup.skips(glyphs[position].glyph, gdef) {
        return 0;
    }
    for &subtable in &lookup.subtables {
        let consumed = match lookup.kind {
            1 => apply_single(table.data, subtable, glyphs, position),
            2 => apply_multiple(table.data, subtable, glyphs, position),
            // Alternate substitution picks a numbered variant. With no user
            // choice to express, the first alternate is the right one — that
            // is what `aalt` offers as its default.
            3 => apply_alternate(table.data, subtable, glyphs, position),
            4 => apply_ligature(table.data, subtable, lookup, gdef, glyphs, position),
            6 => apply_chain_context(table, gdef, subtable, lookup_index, glyphs, position),
            _ => 0,
        };
        if consumed > 0 {
            return consumed;
        }
    }
    0
}

fn apply_single(
    data: &[u8],
    subtable: usize,
    glyphs: &mut [ShapedGlyph],
    position: usize,
) -> usize {
    let reader = Reader::new(data);
    let glyph = glyphs[position].glyph;
    let Some(coverage) = reader.u16_at(subtable + 2) else {
        return 0;
    };
    let Some(index) = coverage_index(data, subtable + usize::from(coverage), glyph) else {
        return 0;
    };
    let substitute = match reader.u16_at(subtable) {
        Some(1) => {
            let delta = reader.u16_at(subtable + 4).unwrap_or(0) as i16;
            glyph.wrapping_add(delta as u16)
        }
        Some(2) => {
            let count = reader.u16_at(subtable + 4).unwrap_or(0);
            if index >= count {
                return 0;
            }
            match reader.u16_at(subtable + 6 + usize::from(index) * 2) {
                Some(glyph) => glyph,
                None => return 0,
            }
        }
        _ => return 0,
    };
    glyphs[position].glyph = substitute;
    1
}

/// One glyph becomes several. This is how SARA AM decomposes.
fn apply_multiple(
    data: &[u8],
    subtable: usize,
    glyphs: &mut Vec<ShapedGlyph>,
    position: usize,
) -> usize {
    let reader = Reader::new(data);
    if reader.u16_at(subtable) != Some(1) {
        return 0;
    }
    let Some(coverage) = reader.u16_at(subtable + 2) else {
        return 0;
    };
    let Some(index) = coverage_index(
        data,
        subtable + usize::from(coverage),
        glyphs[position].glyph,
    ) else {
        return 0;
    };
    let Some(count) = reader.u16_at(subtable + 4) else {
        return 0;
    };
    if index >= count {
        return 0;
    }
    let Some(sequence) = reader.u16_at(subtable + 6 + usize::from(index) * 2) else {
        return 0;
    };
    let sequence = subtable + usize::from(sequence);
    let Some(glyph_count) = reader.u16_at(sequence) else {
        return 0;
    };
    if glyph_count == 0 {
        // A zero-length sequence deletes the glyph. Legal, and used by some
        // fonts to drop a mark; handled rather than left as a surprise.
        glyphs.remove(position);
        return 1;
    }

    let template = glyphs[position];
    let mut replacements = Vec::with_capacity(usize::from(glyph_count));
    for slot in 0..glyph_count {
        let Some(glyph) = reader.u16_at(sequence + 2 + usize::from(slot) * 2) else {
            return 0;
        };
        replacements.push(ShapedGlyph {
            glyph,
            // Every product keeps the source cluster, so the pieces of a
            // decomposed character stay one unit for line breaking.
            cluster: template.cluster,
            x_offset: 0.0,
            y_offset: 0.0,
            // The advance belongs to the first product; the rest are marks.
            x_advance: 0.0,
        });
    }
    glyphs.splice(position..position + 1, replacements);
    usize::from(glyph_count)
}

fn apply_alternate(
    data: &[u8],
    subtable: usize,
    glyphs: &mut [ShapedGlyph],
    position: usize,
) -> usize {
    let reader = Reader::new(data);
    if reader.u16_at(subtable) != Some(1) {
        return 0;
    }
    let Some(coverage) = reader.u16_at(subtable + 2) else {
        return 0;
    };
    let Some(index) = coverage_index(
        data,
        subtable + usize::from(coverage),
        glyphs[position].glyph,
    ) else {
        return 0;
    };
    let Some(set_offset) = reader.u16_at(subtable + 6 + usize::from(index) * 2) else {
        return 0;
    };
    let set = subtable + usize::from(set_offset);
    if reader.u16_at(set).unwrap_or(0) == 0 {
        return 0;
    }
    let Some(alternate) = reader.u16_at(set + 2) else {
        return 0;
    };
    glyphs[position].glyph = alternate;
    1
}

/// Several glyphs become one.
fn apply_ligature(
    data: &[u8],
    subtable: usize,
    lookup: &Lookup,
    gdef: &Gdef<'_>,
    glyphs: &mut Vec<ShapedGlyph>,
    position: usize,
) -> usize {
    let reader = Reader::new(data);
    if reader.u16_at(subtable) != Some(1) {
        return 0;
    }
    let Some(coverage) = reader.u16_at(subtable + 2) else {
        return 0;
    };
    let Some(index) = coverage_index(
        data,
        subtable + usize::from(coverage),
        glyphs[position].glyph,
    ) else {
        return 0;
    };
    let Some(set_count) = reader.u16_at(subtable + 4) else {
        return 0;
    };
    if index >= set_count {
        return 0;
    }
    let Some(set_offset) = reader.u16_at(subtable + 6 + usize::from(index) * 2) else {
        return 0;
    };
    let set = subtable + usize::from(set_offset);
    let Some(ligature_count) = reader.u16_at(set) else {
        return 0;
    };

    for ligature_index in 0..ligature_count {
        let Some(offset) = reader.u16_at(set + 2 + usize::from(ligature_index) * 2) else {
            continue;
        };
        let ligature = set + usize::from(offset);
        let (Some(glyph), Some(components)) =
            (reader.u16_at(ligature), reader.u16_at(ligature + 2))
        else {
            continue;
        };
        if components == 0 {
            continue;
        }

        // Match the remaining components, skipping what the lookup ignores.
        let mut matched = vec![position];
        let mut cursor = position + 1;
        let mut ok = true;
        for component in 1..components {
            let mut found = None;
            while cursor < glyphs.len() {
                if lookup.skips(glyphs[cursor].glyph, gdef) {
                    cursor += 1;
                    continue;
                }
                found = Some(cursor);
                break;
            }
            let Some(at) = found else {
                ok = false;
                break;
            };
            let Some(expected) = reader.u16_at(ligature + 2 + usize::from(component) * 2) else {
                ok = false;
                break;
            };
            if glyphs[at].glyph != expected {
                ok = false;
                break;
            }
            matched.push(at);
            cursor = at + 1;
        }
        if !ok {
            continue;
        }

        let cluster = glyphs[position].cluster;
        let advance = glyphs[position].x_advance;
        // Remove from the back so the earlier indices stay valid.
        for &at in matched.iter().skip(1).rev() {
            glyphs.remove(at);
        }
        glyphs[position] = ShapedGlyph {
            glyph,
            cluster,
            x_offset: 0.0,
            y_offset: 0.0,
            x_advance: advance,
        };
        return 1;
    }
    0
}

/// Chained context, format 3: match backtrack, input and lookahead coverage
/// sequences, then run other lookups at named positions inside the match.
///
/// This is the mechanism behind Thai's contextual mark forms: "a tone mark,
/// preceded by a tall consonant, becomes the lowered variant".
fn apply_chain_context(
    table: &LayoutTable<'_>,
    gdef: &Gdef<'_>,
    subtable: usize,
    lookup_index: usize,
    glyphs: &mut Vec<ShapedGlyph>,
    position: usize,
) -> usize {
    let data = table.data;
    let reader = Reader::new(data);
    if reader.u16_at(subtable) != Some(3) {
        return 0;
    }

    let mut cursor = subtable + 2;
    let Some(backtrack_count) = reader.u16_at(cursor) else {
        return 0;
    };
    cursor += 2;
    let backtrack_at = cursor;
    cursor += usize::from(backtrack_count) * 2;

    let Some(input_count) = reader.u16_at(cursor) else {
        return 0;
    };
    cursor += 2;
    let input_at = cursor;
    cursor += usize::from(input_count) * 2;

    let Some(lookahead_count) = reader.u16_at(cursor) else {
        return 0;
    };
    cursor += 2;
    let lookahead_at = cursor;
    cursor += usize::from(lookahead_count) * 2;

    let Some(record_count) = reader.u16_at(cursor) else {
        return 0;
    };
    let records_at = cursor + 2;

    if input_count == 0 || position + usize::from(input_count) > glyphs.len() {
        return 0;
    }

    // Backtrack is listed nearest-first, walking backwards from the match.
    for step in 0..usize::from(backtrack_count) {
        let Some(index) = position.checked_sub(step + 1) else {
            return 0;
        };
        let Some(coverage) = reader.u16_at(backtrack_at + step * 2) else {
            return 0;
        };
        if coverage_index(data, subtable + usize::from(coverage), glyphs[index].glyph).is_none() {
            return 0;
        }
    }

    for step in 0..usize::from(input_count) {
        let Some(coverage) = reader.u16_at(input_at + step * 2) else {
            return 0;
        };
        if coverage_index(
            data,
            subtable + usize::from(coverage),
            glyphs[position + step].glyph,
        )
        .is_none()
        {
            return 0;
        }
    }

    for step in 0..usize::from(lookahead_count) {
        let index = position + usize::from(input_count) + step;
        let Some(glyph) = glyphs.get(index) else {
            return 0;
        };
        let Some(coverage) = reader.u16_at(lookahead_at + step * 2) else {
            return 0;
        };
        if coverage_index(data, subtable + usize::from(coverage), glyph.glyph).is_none() {
            return 0;
        }
    }

    // Matched. Run the nested lookups.
    for record in 0..record_count {
        let at = records_at + usize::from(record) * 4;
        let (Some(sequence_index), Some(nested)) = (reader.u16_at(at), reader.u16_at(at + 2))
        else {
            continue;
        };
        // A lookup that invoked itself would not terminate.
        if usize::from(nested) == lookup_index {
            continue;
        }
        let target = position + usize::from(sequence_index);
        if target >= glyphs.len() {
            continue;
        }
        let Some(nested_lookup) = table.lookups.get(usize::from(nested)).cloned() else {
            continue;
        };
        apply_substitution_at(
            table,
            gdef,
            &nested_lookup,
            usize::from(nested),
            glyphs,
            target,
        );
    }
    usize::from(input_count)
}

// --- GPOS ------------------------------------------------------------------

/// Apply a set of positioning lookups to the buffer.
pub fn apply_positioning(
    table: &LayoutTable<'_>,
    gdef: &Gdef<'_>,
    lookup_indices: &[u16],
    glyphs: &mut [ShapedGlyph],
) {
    for &index in lookup_indices {
        let Some(lookup) = table.lookups.get(usize::from(index)).cloned() else {
            continue;
        };
        for position in 0..glyphs.len() {
            if lookup.skips(glyphs[position].glyph, gdef) {
                continue;
            }
            for &subtable in &lookup.subtables {
                let applied = match lookup.kind {
                    2 => apply_pair(table, gdef, &lookup, subtable, glyphs, position),
                    4 => apply_mark_to_base(table, gdef, &lookup, subtable, glyphs, position),
                    6 => apply_mark_to_mark(table, gdef, &lookup, subtable, glyphs, position),
                    _ => false,
                };
                if applied {
                    break;
                }
            }
        }
    }
}

/// Kerning between two glyphs, by pair or by class.
fn apply_pair(
    table: &LayoutTable<'_>,
    gdef: &Gdef<'_>,
    lookup: &Lookup,
    subtable: usize,
    glyphs: &mut [ShapedGlyph],
    position: usize,
) -> bool {
    let data = table.data;
    let reader = Reader::new(data);
    let Some(second) = table.next_visible(glyphs, lookup, gdef, position + 1) else {
        return false;
    };
    let (first_glyph, second_glyph) = (glyphs[position].glyph, glyphs[second].glyph);

    let Some(coverage) = reader.u16_at(subtable + 2) else {
        return false;
    };
    let Some(index) = coverage_index(data, subtable + usize::from(coverage), first_glyph) else {
        return false;
    };
    let (Some(format1), Some(format2)) = (reader.u16_at(subtable + 4), reader.u16_at(subtable + 6))
    else {
        return false;
    };
    let size1 = value_record_size(format1);
    let size2 = value_record_size(format2);

    match reader.u16_at(subtable) {
        Some(1) => {
            let Some(set_count) = reader.u16_at(subtable + 8) else {
                return false;
            };
            if index >= set_count {
                return false;
            }
            let Some(set_offset) = reader.u16_at(subtable + 10 + usize::from(index) * 2) else {
                return false;
            };
            let set = subtable + usize::from(set_offset);
            let Some(pair_count) = reader.u16_at(set) else {
                return false;
            };
            let record_size = 2 + size1 + size2;
            for pair in 0..pair_count {
                let record = set + 2 + usize::from(pair) * record_size;
                if reader.u16_at(record) != Some(second_glyph) {
                    continue;
                }
                let (placement1, advance1) = value_record(data, record + 2, format1);
                let (placement2, advance2) = value_record(data, record + 2 + size1, format2);
                glyphs[position].x_offset += placement1;
                glyphs[position].x_advance += advance1;
                glyphs[second].x_offset += placement2;
                glyphs[second].x_advance += advance2;
                return true;
            }
            false
        }
        Some(2) => {
            let (Some(class_def1), Some(class_def2)) =
                (reader.u16_at(subtable + 8), reader.u16_at(subtable + 10))
            else {
                return false;
            };
            let (Some(class1_count), Some(class2_count)) =
                (reader.u16_at(subtable + 12), reader.u16_at(subtable + 14))
            else {
                return false;
            };
            let class1 = class_of(data, subtable + usize::from(class_def1), first_glyph);
            let class2 = class_of(data, subtable + usize::from(class_def2), second_glyph);
            if class1 >= class1_count || class2 >= class2_count {
                return false;
            }
            let record_size = size1 + size2;
            let record = subtable
                + 16
                + (usize::from(class1) * usize::from(class2_count) + usize::from(class2))
                    * record_size;
            let (placement1, advance1) = value_record(data, record, format1);
            let (placement2, advance2) = value_record(data, record + size1, format2);
            if placement1 == 0.0 && advance1 == 0.0 && placement2 == 0.0 && advance2 == 0.0 {
                // Class pairs are a dense matrix and most cells are zero;
                // reporting "applied" for a zero cell would stop later
                // subtables from getting a chance.
                return false;
            }
            glyphs[position].x_offset += placement1;
            glyphs[position].x_advance += advance1;
            glyphs[second].x_offset += placement2;
            glyphs[second].x_advance += advance2;
            true
        }
        _ => false,
    }
}

/// Attach a mark to the base glyph before it, by anchor.
fn apply_mark_to_base(
    table: &LayoutTable<'_>,
    gdef: &Gdef<'_>,
    lookup: &Lookup,
    subtable: usize,
    glyphs: &mut [ShapedGlyph],
    position: usize,
) -> bool {
    let data = table.data;
    let reader = Reader::new(data);
    if reader.u16_at(subtable) != Some(1) {
        return false;
    }
    let (Some(mark_coverage), Some(base_coverage)) =
        (reader.u16_at(subtable + 2), reader.u16_at(subtable + 4))
    else {
        return false;
    };
    let Some(mark_index) = coverage_index(
        data,
        subtable + usize::from(mark_coverage),
        glyphs[position].glyph,
    ) else {
        return false;
    };

    // The base is the nearest preceding glyph that is not itself a mark.
    let mut base_position = None;
    for candidate in (0..position).rev() {
        if !gdef.is_mark(glyphs[candidate].glyph) {
            base_position = Some(candidate);
            break;
        }
    }
    let Some(base_position) = base_position else {
        return false;
    };
    let Some(base_index) = coverage_index(
        data,
        subtable + usize::from(base_coverage),
        glyphs[base_position].glyph,
    ) else {
        return false;
    };

    let (Some(class_count), Some(mark_array), Some(base_array)) = (
        reader.u16_at(subtable + 6),
        reader.u16_at(subtable + 8),
        reader.u16_at(subtable + 10),
    ) else {
        return false;
    };

    let Some((class, mark_anchor)) =
        mark_array_entry(data, subtable + usize::from(mark_array), mark_index)
    else {
        return false;
    };
    if class >= class_count {
        return false;
    }
    let base_array = subtable + usize::from(base_array);
    let Some(base_anchor) = anchor_array_entry(data, base_array, base_index, class, class_count)
    else {
        return false;
    };

    attach(glyphs, base_position, position, base_anchor, mark_anchor);
    let _ = lookup;
    true
}

/// Attach a mark to the mark below it — a tone mark stacking on a vowel.
fn apply_mark_to_mark(
    table: &LayoutTable<'_>,
    gdef: &Gdef<'_>,
    lookup: &Lookup,
    subtable: usize,
    glyphs: &mut [ShapedGlyph],
    position: usize,
) -> bool {
    let data = table.data;
    let reader = Reader::new(data);
    if reader.u16_at(subtable) != Some(1) {
        return false;
    }
    let (Some(mark1_coverage), Some(mark2_coverage)) =
        (reader.u16_at(subtable + 2), reader.u16_at(subtable + 4))
    else {
        return false;
    };
    let Some(mark1_index) = coverage_index(
        data,
        subtable + usize::from(mark1_coverage),
        glyphs[position].glyph,
    ) else {
        return false;
    };

    // The glyph below is the nearest preceding one this lookup can see —
    // which the mark filtering set has already narrowed to the right kind.
    let Some(below) = table.previous_visible(glyphs, lookup, gdef, position) else {
        return false;
    };
    let Some(mark2_index) = coverage_index(
        data,
        subtable + usize::from(mark2_coverage),
        glyphs[below].glyph,
    ) else {
        return false;
    };

    let (Some(class_count), Some(mark1_array), Some(mark2_array)) = (
        reader.u16_at(subtable + 6),
        reader.u16_at(subtable + 8),
        reader.u16_at(subtable + 10),
    ) else {
        return false;
    };

    let Some((class, mark_anchor)) =
        mark_array_entry(data, subtable + usize::from(mark1_array), mark1_index)
    else {
        return false;
    };
    if class >= class_count {
        return false;
    }
    let Some(below_anchor) = anchor_array_entry(
        data,
        subtable + usize::from(mark2_array),
        mark2_index,
        class,
        class_count,
    ) else {
        return false;
    };

    attach(glyphs, below, position, below_anchor, mark_anchor);
    true
}

/// `(mark class, anchor)` for one entry of a mark array.
fn mark_array_entry(data: &[u8], array: usize, index: u16) -> Option<(u16, (f32, f32))> {
    let reader = Reader::new(data);
    let count = reader.u16_at(array)?;
    if index >= count {
        return None;
    }
    let record = array + 2 + usize::from(index) * 4;
    let class = reader.u16_at(record)?;
    let anchor_offset = reader.u16_at(record + 2)?;
    if anchor_offset == 0 {
        return None;
    }
    let anchor = anchor_at(data, array + usize::from(anchor_offset))?;
    Some((class, anchor))
}

/// The anchor for one (glyph, mark class) pair of a base or mark2 array.
fn anchor_array_entry(
    data: &[u8],
    array: usize,
    index: u16,
    class: u16,
    class_count: u16,
) -> Option<(f32, f32)> {
    let reader = Reader::new(data);
    let count = reader.u16_at(array)?;
    if index >= count {
        return None;
    }
    let record = array + 2 + usize::from(index) * usize::from(class_count) * 2;
    let anchor_offset = reader.u16_at(record + usize::from(class) * 2)?;
    // A null offset means this class does not attach to this glyph.
    if anchor_offset == 0 {
        return None;
    }
    anchor_at(data, array + usize::from(anchor_offset))
}

/// Place `mark` so its anchor lands on `base`'s anchor.
///
/// The mark's pen position is the base's plus every advance in between, so
/// that distance has to come back out of the offset. Marks have zero advance,
/// which is what lets several stack on one base.
fn attach(
    glyphs: &mut [ShapedGlyph],
    base: usize,
    mark: usize,
    base_anchor: (f32, f32),
    mark_anchor: (f32, f32),
) {
    let advances: f32 = glyphs[base..mark].iter().map(|glyph| glyph.x_advance).sum();
    glyphs[mark].x_offset = glyphs[base].x_offset + base_anchor.0 - mark_anchor.0 - advances;
    glyphs[mark].y_offset = glyphs[base].y_offset + base_anchor.1 - mark_anchor.1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sfnt::Sfnt;

    const THAI: &[u8] = include_bytes!("../test-fonts/NotoSansThai-Regular.ttf");

    /// The font is embedded, so its tables borrow from `'static` bytes and
    /// outlive the `Sfnt` that located them.
    fn table(tag: &[u8; 4]) -> &'static [u8] {
        Sfnt::parse(THAI).unwrap().table(tag).unwrap()
    }

    #[test]
    fn the_face_uses_only_lookups_this_module_implements() {
        // The scope claim in the module documentation, as an assertion. If a
        // replacement font needs something else, this fails rather than
        // silently rendering it wrong.
        let gsub = LayoutTable::parse(table(b"GSUB")).unwrap();
        for lookup in gsub.lookups() {
            assert!(
                matches!(lookup.kind, 1 | 2 | 3 | 4 | 6),
                "unimplemented GSUB lookup type {}",
                lookup.kind
            );
        }
        let gpos = LayoutTable::parse(table(b"GPOS")).unwrap();
        for lookup in gpos.lookups() {
            assert!(
                matches!(lookup.kind, 2 | 4 | 6),
                "unimplemented GPOS lookup type {}",
                lookup.kind
            );
        }
    }

    #[test]
    fn the_features_the_shaper_asks_for_are_present() {
        let gsub = LayoutTable::parse(table(b"GSUB")).unwrap();
        let tags = gsub.feature_tags();
        assert!(tags.contains(&"ccmp".to_owned()));
        assert!(tags.contains(&"liga".to_owned()));

        let gpos = LayoutTable::parse(table(b"GPOS")).unwrap();
        let tags = gpos.feature_tags();
        assert!(tags.contains(&"kern".to_owned()));
        assert!(tags.contains(&"mark".to_owned()));
        assert!(tags.contains(&"mkmk".to_owned()));
    }

    #[test]
    fn lookups_come_back_in_list_order_without_duplicates() {
        let gsub = LayoutTable::parse(table(b"GSUB")).unwrap();
        let indices = gsub.lookups_for(&[b"ccmp", b"liga"]);
        assert!(!indices.is_empty());
        assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn an_unknown_feature_selects_nothing_rather_than_everything() {
        let gsub = LayoutTable::parse(table(b"GSUB")).unwrap();
        assert!(gsub.lookups_for(&[b"zzzz"]).is_empty());
    }

    #[test]
    fn gdef_distinguishes_thai_marks_from_consonants() {
        let sfnt = Sfnt::parse(THAI).unwrap();
        let metrics = crate::sfnt::FontMetrics::parse(&sfnt).unwrap();
        let map = crate::sfnt::CharacterMap::parse(&sfnt).unwrap();
        let _ = metrics;
        let gdef = Gdef::parse(table(b"GDEF")).unwrap();

        for consonant in ['ก', 'ผ', 'ด', 'ป'] {
            let glyph = map.glyph(consonant).unwrap();
            assert!(!gdef.is_mark(glyph), "{consonant} should be a base");
        }
        for mark in ['่', '้', 'ิ', 'ุ', '์'] {
            let glyph = map.glyph(mark).unwrap();
            assert!(gdef.is_mark(glyph), "{mark} should be a mark");
        }
    }

    #[test]
    fn a_missing_gdef_treats_everything_as_a_base() {
        // The safe default: a glyph wrongly called a mark would be moved on
        // top of its neighbour, which is worse than no attachment at all.
        let gdef = Gdef::empty();
        assert!(!gdef.is_mark(42));
        assert_eq!(gdef.class_of(42), 0);
    }

    #[test]
    fn lookup_flags_hide_the_glyphs_they_say_they_hide() {
        let gdef = Gdef::parse(table(b"GDEF")).unwrap();
        let sfnt = Sfnt::parse(THAI).unwrap();
        let map = crate::sfnt::CharacterMap::parse(&sfnt).unwrap();
        let consonant = map.glyph('ก').unwrap();
        let mark = map.glyph('่').unwrap();

        let ignoring_marks = Lookup {
            kind: 2,
            flag: IGNORE_MARKS,
            mark_filtering_set: None,
            subtables: Vec::new(),
        };
        assert!(ignoring_marks.skips(mark, &gdef));
        assert!(!ignoring_marks.skips(consonant, &gdef));

        let plain = Lookup {
            kind: 4,
            flag: 0,
            mark_filtering_set: None,
            subtables: Vec::new(),
        };
        assert!(!plain.skips(mark, &gdef));
    }

    #[test]
    fn a_value_record_is_sized_by_its_format_bits() {
        assert_eq!(value_record_size(0), 0);
        assert_eq!(value_record_size(0x0004), 2); // XAdvance only
        assert_eq!(value_record_size(0x0005), 4); // XPlacement + XAdvance
        assert_eq!(value_record_size(0x00FF), 16); // every field
    }

    #[test]
    fn attachment_cancels_the_advance_between_base_and_mark() {
        // A base of advance 600 with a mark after it: to put the mark's anchor
        // on the base's, the 600 the pen already moved has to come back out.
        let mut glyphs = vec![ShapedGlyph::new(1, 0, 600.0), ShapedGlyph::new(2, 1, 0.0)];
        attach(&mut glyphs, 0, 1, (300.0, 700.0), (100.0, 0.0));
        assert_eq!(glyphs[1].x_offset, 300.0 - 100.0 - 600.0);
        assert_eq!(glyphs[1].y_offset, 700.0);
    }

    #[test]
    fn stacking_a_second_mark_builds_on_the_first() {
        // mkmk attaches to a mark that has already been positioned, so the
        // base's own offset has to carry through.
        let mut glyphs = vec![
            ShapedGlyph::new(1, 0, 600.0),
            ShapedGlyph::new(2, 1, 0.0),
            ShapedGlyph::new(3, 2, 0.0),
        ];
        attach(&mut glyphs, 0, 1, (300.0, 700.0), (100.0, 0.0));
        let first = glyphs[1];
        // The second mark attaches to the first, whose anchors are in its own
        // em space — so the displacement the first already has carries through.
        // That accumulation is what stacks ่ above ี rather than on top of ก.
        attach(&mut glyphs, 1, 2, (150.0, 900.0), (150.0, 0.0));
        assert_eq!(
            glyphs[2].x_offset, first.x_offset,
            "stacked marks stay aligned"
        );
        assert_eq!(glyphs[2].y_offset, first.y_offset + 900.0);
        assert!(
            glyphs[2].y_offset > glyphs[1].y_offset,
            "the stack goes upward"
        );
    }

    #[test]
    fn coverage_lookup_handles_both_formats_and_misses() {
        // Format 1: a sorted glyph list. Built by hand so the parser is tested
        // rather than the font.
        let format1 = [0x00, 0x01, 0x00, 0x03, 0x00, 0x05, 0x00, 0x09, 0x00, 0x0A];
        assert_eq!(coverage_index(&format1, 0, 5), Some(0));
        assert_eq!(coverage_index(&format1, 0, 9), Some(1));
        assert_eq!(coverage_index(&format1, 0, 10), Some(2));
        assert_eq!(coverage_index(&format1, 0, 7), None);

        // Format 2: ranges with a starting coverage index.
        let format2 = [
            0x00, 0x02, 0x00, 0x01, // format, rangeCount
            0x00, 0x14, 0x00, 0x18, 0x00, 0x07, // 20..=24 starting at 7
        ];
        assert_eq!(coverage_index(&format2, 0, 20), Some(7));
        assert_eq!(coverage_index(&format2, 0, 24), Some(11));
        assert_eq!(coverage_index(&format2, 0, 25), None);
    }

    #[test]
    fn class_definitions_default_unlisted_glyphs_to_zero() {
        let format1 = [
            0x00, 0x01, 0x00, 0x0A, 0x00, 0x03, // format, startGlyph 10, count 3
            0x00, 0x02, 0x00, 0x05, 0x00, 0x01,
        ];
        assert_eq!(class_of(&format1, 0, 10), 2);
        assert_eq!(class_of(&format1, 0, 12), 1);
        assert_eq!(class_of(&format1, 0, 13), 0, "past the end is class 0");
        assert_eq!(class_of(&format1, 0, 9), 0, "before the start is class 0");
    }

    #[test]
    fn malformed_layout_bytes_are_refused_rather_than_indexed_into() {
        assert!(LayoutTable::parse(&[]).is_none());
        assert!(LayoutTable::parse(&[0, 1, 0, 0]).is_none());
        assert!(Gdef::parse(&[0, 1]).is_none());
        // Truncations of the real table must not panic.
        let gsub = table(b"GSUB");
        for length in (0..gsub.len()).step_by(101) {
            let _ = LayoutTable::parse(&gsub[..length]);
        }
    }
}
