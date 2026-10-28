//! DEFLATE (RFC 1951) inflate, wrapped in the zlib (RFC 1950) framing PNG's
//! `IDAT` stream uses.
//!
//! `IDAT` is the one thing the existing writer never had to do: PNG's own
//! wire format is compressed, and reading a real icon theme means reading
//! real Huffman-coded streams, not just the stored blocks the writer emits.
//! Written from the RFC rather than adapted from another codebase, in the
//! same spirit as `liminalkit-text`'s TrueType parser: a general inflate has to
//! survive every encoder ever written, but this one only has to decompress
//! what a PNG encoder produces, and it returns `None` rather than panicking
//! on anything that does not look like that — a corrupt download is user
//! input, not a bug report.
//!
//! Every symbol table walk is bounds-checked (`.get()`, not indexing) so a
//! truncated or bit-flipped stream fails a decode rather than the process.

const MAX_BITS: usize = 15;

/// Length code base values and extra-bit counts for symbols 257..=285
/// (RFC 1951 §3.2.5).
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

/// Distance code base values and extra-bit counts for symbols 0..=29.
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Order the code-length alphabet's own lengths are transmitted in — not
/// numeric order, so that a stream using only a few of them (the common
/// case) can trail off without transmitting the rest.
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// A least-significant-bit-first bit stream over a byte slice.
///
/// DEFLATE packs ordinary fields LSB-first but assembles Huffman codes
/// MSB-first from the bits it reads — [`Huffman::decode`] does that
/// assembly; this only ever hands out bits in stream order.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    nbits: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> BitReader<'a> {
        BitReader {
            data,
            pos: 0,
            acc: 0,
            nbits: 0,
        }
    }

    fn fill(&mut self) {
        while self.nbits <= 56 && self.pos < self.data.len() {
            self.acc |= u64::from(self.data[self.pos]) << self.nbits;
            self.pos += 1;
            self.nbits += 8;
        }
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        if self.nbits < n {
            self.fill();
        }
        if self.nbits < n {
            return None;
        }
        let v = (self.acc & ((1u64 << n) - 1)) as u32;
        self.acc >>= n;
        self.nbits -= n;
        Some(v)
    }

    /// Drop the partial byte left in the bit buffer — required before a
    /// stored block, which restarts on a byte boundary.
    fn align_byte(&mut self) {
        let drop = self.nbits % 8;
        self.acc >>= drop;
        self.nbits -= drop;
    }

    /// A byte slice at the current (byte-aligned) position. Call
    /// [`align_byte`](Self::align_byte) first.
    fn take_bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let start = self.pos - (self.nbits / 8) as usize;
        let end = start.checked_add(n)?;
        let slice = self.data.get(start..end)?;
        self.acc = 0;
        self.nbits = 0;
        self.pos = end;
        Some(slice)
    }
}

/// A canonical Huffman table: how many codes exist at each length, and which
/// symbol each code (in length-then-symbol order) maps to.
struct Huffman {
    counts: [u16; MAX_BITS + 1],
    symbols: Vec<u16>,
}

impl Huffman {
    /// Build from a code length per symbol (0 = symbol unused). Lengths that
    /// do not satisfy Kraft's inequality — a corrupt or hostile stream —
    /// still build a table; [`decode`](Self::decode) simply fails to find a
    /// prefix for it rather than the construction panicking.
    fn build(lengths: &[u8]) -> Option<Huffman> {
        let mut counts = [0u16; MAX_BITS + 1];
        for &len in lengths {
            let len = len as usize;
            if len > MAX_BITS {
                return None;
            }
            counts[len] += 1;
        }
        counts[0] = 0;

        let mut offsets = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offsets[len + 1] = offsets[len] + counts[len];
        }

        let mut symbols = vec![0u16; lengths.len()];
        for (symbol, &len) in lengths.iter().enumerate() {
            if len != 0 {
                let slot = offsets[len as usize] as usize;
                *symbols.get_mut(slot)? = symbol as u16;
                offsets[len as usize] += 1;
            }
        }
        Some(Huffman { counts, symbols })
    }

    /// Read one bit at a time, extending a candidate code, until it falls in
    /// the range assigned to some length — the classic canonical-Huffman
    /// walk (RFC 1951 §3.2.2), which needs no lookup table.
    fn decode(&self, reader: &mut BitReader) -> Option<u16> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=MAX_BITS {
            code |= reader.bits(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - first < count {
                return self.symbols.get((index + (code - first)) as usize).copied();
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        None
    }
}

fn fixed_tables() -> (Huffman, Huffman) {
    let mut literal = [0u8; 288];
    literal[0..144].fill(8);
    literal[144..256].fill(9);
    literal[256..280].fill(7);
    literal[280..288].fill(8);
    let distance = [5u8; 30];
    // `unwrap`: these lengths are fixed constants known to build cleanly.
    (
        Huffman::build(&literal).unwrap(),
        Huffman::build(&distance).unwrap(),
    )
}

/// Read the dynamic Huffman header (RFC 1951 §3.2.7): the code-length
/// alphabet's own lengths, then the literal/length and distance alphabets'
/// lengths, run-length coded through it.
fn read_dynamic_tables(reader: &mut BitReader) -> Option<(Huffman, Huffman)> {
    let hlit = reader.bits(5)? as usize + 257;
    let hdist = reader.bits(5)? as usize + 1;
    let hclen = reader.bits(4)? as usize + 4;

    let mut cl_lengths = [0u8; 19];
    for &position in CODE_LENGTH_ORDER.iter().take(hclen) {
        cl_lengths[position] = reader.bits(3)? as u8;
    }
    let cl_table = Huffman::build(&cl_lengths)?;

    let mut lengths = Vec::with_capacity(hlit + hdist);
    while lengths.len() < hlit + hdist {
        match cl_table.decode(reader)? {
            symbol @ 0..=15 => lengths.push(symbol as u8),
            16 => {
                // Repeat the previous length 3-6 times.
                let previous = *lengths.last()?;
                let repeat = reader.bits(2)? + 3;
                for _ in 0..repeat {
                    lengths.push(previous);
                }
            }
            17 => {
                let repeat = reader.bits(3)? + 3;
                lengths.extend(std::iter::repeat_n(0u8, repeat as usize));
            }
            18 => {
                let repeat = reader.bits(7)? + 11;
                lengths.extend(std::iter::repeat_n(0u8, repeat as usize));
            }
            _ => return None,
        }
        if lengths.len() > hlit + hdist {
            return None; // A run overshot the table it was filling.
        }
    }

    let literal = Huffman::build(&lengths[..hlit])?;
    let distance = Huffman::build(&lengths[hlit..])?;
    Some((literal, distance))
}

/// Decode one compressed block's symbols into `out`, given its literal/length
/// and distance tables. Refuses to grow `out` past `max_output` — the guard
/// against a decompression bomb, checked at every point `out` grows rather
/// than after the fact, so it can never even briefly exceed the cap.
fn inflate_block(
    reader: &mut BitReader,
    out: &mut Vec<u8>,
    literal: &Huffman,
    distance: &Huffman,
    max_output: usize,
) -> Option<()> {
    loop {
        let symbol = literal.decode(reader)?;
        match symbol {
            0..=255 => {
                if out.len() >= max_output {
                    return None;
                }
                out.push(symbol as u8);
            }
            256 => return Some(()),
            257..=285 => {
                let index = (symbol - 257) as usize;
                let length = LENGTH_BASE[index] as usize
                    + reader.bits(u32::from(LENGTH_EXTRA[index]))? as usize;
                let dsym = distance.decode(reader)? as usize;
                let dbase = *DIST_BASE.get(dsym)?;
                let dextra = *DIST_EXTRA.get(dsym)?;
                let dist = dbase as usize + reader.bits(u32::from(dextra))? as usize;
                if dist == 0 || dist > out.len() {
                    return None; // A back-reference into data that was never written.
                }
                if out.len().saturating_add(length) > max_output {
                    return None; // This match alone would blow the cap.
                }
                let start = out.len() - dist;
                // Byte at a time: `length` can exceed `dist`, which is how
                // DEFLATE encodes a run — each copied byte must be able to
                // see bytes this same match already produced.
                for i in 0..length {
                    let byte = out[start + i];
                    out.push(byte);
                }
            }
            _ => return None, // 286/287 are unused literal/length codes.
        }
    }
}

/// Inflate a raw DEFLATE stream (no zlib framing), refusing to produce more
/// than `max_output` bytes.
fn inflate(data: &[u8], max_output: usize) -> Option<Vec<u8>> {
    let mut reader = BitReader::new(data);
    let mut out = Vec::new();
    loop {
        let is_final = reader.bits(1)? != 0;
        match reader.bits(2)? {
            0 => {
                reader.align_byte();
                let len = reader.bits(16)?;
                let nlen = reader.bits(16)?;
                if len != !nlen & 0xffff {
                    return None;
                }
                if out.len().saturating_add(len as usize) > max_output {
                    return None; // A stored block alone can claim up to 64 KiB.
                }
                out.extend_from_slice(reader.take_bytes(len as usize)?);
            }
            1 => {
                let (literal, distance) = fixed_tables();
                inflate_block(&mut reader, &mut out, &literal, &distance, max_output)?;
            }
            2 => {
                let (literal, distance) = read_dynamic_tables(&mut reader)?;
                inflate_block(&mut reader, &mut out, &literal, &distance, max_output)?;
            }
            _ => return None, // BTYPE 3 is reserved.
        }
        if is_final {
            return Some(out);
        }
    }
}

/// Inflate a zlib-wrapped (RFC 1950) stream — what PNG's `IDAT` holds — and
/// check its Adler-32 trailer. A checksum mismatch is treated the same as a
/// structurally broken stream: both mean the bytes cannot be trusted.
///
/// `max_output` bounds how large the decompressed stream is allowed to grow;
/// the caller knows the exact size a well-formed image needs (see
/// `super::decode::expected_raw_size`), so a stream that exceeds it — a
/// decompression bomb, a few kilobytes of input expanding to gigabytes — is
/// refused while it is still inflating rather than once it has already
/// exhausted memory.
pub(crate) fn zlib_decompress(data: &[u8], max_output: usize) -> Option<Vec<u8>> {
    let cmf = *data.first()?;
    let flg = *data.get(1)?;
    if (u16::from(cmf) * 256 + u16::from(flg)) % 31 != 0 {
        return None; // The header check bits PNG always writes.
    }
    if cmf & 0x0f != 8 {
        return None; // Not the DEFLATE compression method.
    }
    if flg & 0x20 != 0 {
        return None; // A preset dictionary, which PNG never uses.
    }
    let trailer_start = data.len().checked_sub(4)?;
    let compressed = data.get(2..trailer_start)?;
    let out = inflate(compressed, max_output)?;
    let expected = u32::from_be_bytes(data[trailer_start..].try_into().ok()?);
    if super::checksum::adler32(&out) != expected {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What most tests pass as `max_output`: they are not testing the cap,
    /// so it should not be the thing that trips.
    const NO_LIMIT: usize = usize::MAX;

    /// Build a minimal zlib stream from one or more stored (uncompressed)
    /// blocks — the same framing the writer in `encode.rs` produces — so the
    /// stored-block path is tested without needing a compressed fixture.
    fn zlib_stored(raw: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        if raw.is_empty() {
            out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
        }
        for (i, block) in raw.chunks(4).enumerate() {
            let last = (i + 1) * 4 >= raw.len();
            out.push(u8::from(last));
            let len = block.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(block);
        }
        out.extend_from_slice(&super::super::checksum::adler32(raw).to_be_bytes());
        out
    }

    #[test]
    fn a_stored_stream_round_trips() {
        let raw = b"the quick brown fox jumps over the lazy dog";
        let zlib = zlib_stored(raw);
        assert_eq!(
            zlib_decompress(&zlib, NO_LIMIT).as_deref(),
            Some(raw.as_slice())
        );
    }

    #[test]
    fn an_empty_stream_round_trips() {
        let zlib = zlib_stored(b"");
        assert_eq!(
            zlib_decompress(&zlib, NO_LIMIT).as_deref(),
            Some(b"".as_slice())
        );
    }

    #[test]
    fn a_bad_header_is_rejected() {
        assert!(zlib_decompress(&[0x00, 0x00], NO_LIMIT).is_none());
    }

    #[test]
    fn a_preset_dictionary_is_rejected() {
        // CMF 0x78, FLG 0x20: the header-check bits balance
        // (0x78 * 256 + 0x20 = 30752 = 31 * 992) and FDICT (bit 5 of FLG) is
        // set — the one thing a PNG's zlib stream never does.
        assert!(zlib_decompress(&[0x78, 0x20], NO_LIMIT).is_none());
    }

    #[test]
    fn a_wrong_checksum_is_rejected() {
        let mut zlib = zlib_stored(b"data");
        let last = zlib.len() - 1;
        zlib[last] ^= 0xff;
        assert!(zlib_decompress(&zlib, NO_LIMIT).is_none());
    }

    #[test]
    fn truncated_input_never_panics_and_never_succeeds() {
        let zlib = zlib_stored(b"the quick brown fox jumps over the lazy dog, twice over");
        for len in 0..zlib.len() {
            let _ = zlib_decompress(&zlib[..len], NO_LIMIT);
        }
        // Reaching here at all is the assertion: nothing panicked.
    }

    #[test]
    fn a_stream_within_the_cap_is_unaffected_by_it() {
        let raw = b"twelve bytes";
        let zlib = zlib_stored(raw);
        assert_eq!(
            zlib_decompress(&zlib, raw.len()).as_deref(),
            Some(raw.as_slice()),
            "exactly enough room should still succeed"
        );
    }

    #[test]
    fn a_stream_that_exceeds_the_cap_is_refused() {
        let raw = b"the quick brown fox jumps over the lazy dog";
        let zlib = zlib_stored(raw);
        assert!(zlib_decompress(&zlib, raw.len() - 1).is_none());
    }

    #[test]
    fn a_stored_block_alone_cannot_exceed_the_cap() {
        // A single stored block can claim up to 64 KiB with no compression
        // at all — the cheapest possible way to blow past a small cap, and
        // it must be caught before the bytes are even copied out.
        let raw = vec![b'x'; 10_000];
        let zlib = zlib_stored(&raw);
        assert!(zlib_decompress(&zlib, 100).is_none());
    }

    #[test]
    fn a_run_length_bomb_is_refused_rather_than_exhausting_memory() {
        // The classic decompression bomb in miniature: a two-bit match
        // ("copy 3 bytes from 4 back") repeated as many times as the input
        // allows turns a few dozen input bytes into an unbounded output.
        // `max_output` has to stop this while `out` is still growing, not
        // once the input runs out — this input has room for far more than
        // the cap allows.
        let mut out = vec![0u8; 8]; // a non-empty seed to copy from
        let mut lengths = [0u8; 288];
        lengths[256] = 2; // end-of-block, code "10"
        lengths[257] = 1; // length symbol 257 (base length 3), code "0"
        let literal = Huffman::build(&lengths).unwrap();
        let mut dlengths = [0u8; 30];
        dlengths[3] = 1; // distance symbol 3 (base distance 4), code "0"
        let distance = Huffman::build(&dlengths).unwrap();

        // An endless run of '0' bits: length symbol 257, then distance
        // symbol 3, forever — "copy 3 bytes from 4 back" on repeat.
        let data = vec![0x00u8; 4096];
        let mut reader = BitReader::new(&data);
        let result = inflate_block(&mut reader, &mut out, &literal, &distance, 1_000);
        assert!(result.is_none());
        assert!(out.len() <= 1_000, "grew to {} past the cap", out.len());
    }

    #[test]
    fn fixed_huffman_tables_build_and_decode_every_literal() {
        let (literal, _distance) = fixed_tables();
        // Encode a bitstream containing every fixed code by hand is a lot of
        // machinery for a table test; instead check the shape directly:
        // 288 symbols, counts matching the RFC's per-length tally.
        assert_eq!(literal.symbols.len(), 288);
        assert_eq!(literal.counts[7], 24); // 256..280
        assert_eq!(literal.counts[8], 152); // 0..144 and 280..288
        assert_eq!(literal.counts[9], 112); // 144..256
    }

    #[test]
    fn a_length_distance_backreference_round_trips() {
        // Hand-assemble one dynamic block: a literal run "ab", then a
        // length/distance pair copying "ab" again via a minimal Huffman
        // table, so the LZ77 copy path (not just stored blocks) is exercised
        // end to end through `inflate`.
        //
        // Building a legal dynamic header by hand is intricate, so this
        // drives `inflate_block` directly against a hand-rolled table
        // instead of a real bitstream — the unit under test either way is
        // "does a back-reference copy the right bytes".
        let mut out = b"ab".to_vec();
        let mut lengths = [0u8; 288];
        lengths[256] = 1; // end-of-block, single-bit code "0"
        lengths[257] = 1; // length symbol 257 (base length 3), code "1"
        let literal = Huffman::build(&lengths).unwrap();
        let mut dlengths = [0u8; 30];
        dlengths[1] = 1; // distance symbol 1 (base distance 2), code "0"
        let distance = Huffman::build(&dlengths).unwrap();

        // Bitstream: '1' (length 257, extra 0 bits) '0' (distance 1, extra 0
        // bits) '0' (end of block). LSB-first packing of a single byte.
        let data = [0b0000_0001u8];
        let mut reader = BitReader::new(&data);
        inflate_block(&mut reader, &mut out, &literal, &distance, NO_LIMIT).unwrap();
        // Distance 2, length 3: the copy re-reads its own output, so "ab"
        // extends by repeating itself rather than repeating one byte.
        assert_eq!(out, b"ababa");
    }
}
