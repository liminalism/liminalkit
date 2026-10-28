//! Reading a PNG: chunk framing, `IHDR`, `PLTE`/`tRNS`, and turning
//! reconstructed pixels into a [`Bitmap`].
//!
//! Scope is deliberate and matches what an icon theme actually ships: 8-bit
//! greyscale, RGB, palette and RGBA, interlaced or not, with `tRNS`
//! transparency. Not covered: bit depths other than 8 (real themes on this
//! machine used only 8 and 16; 16-bit icons exist but are rare enough, and
//! narrow enough a win, to leave for later rather than double the reconstruct
//! path's cases) and anything CMYK, ICC-tagged, or animated (`acTL`/`fdAT`).
//! **SVG is out of scope entirely** — this crate stays a raster path, and an
//! icon theme's SVG source is a separate, and much larger, problem: a path
//! grammar, gradients, a `<use>`/`<symbol>` reference model. A decision, not
//! an oversight.
//!
//! Every chunk's CRC is checked, every length is bounds-checked against what
//! is actually left in the buffer, and every lookup that depends on the
//! file's own claims (`PLTE` size, a palette index, a colour type) is
//! `.get()` rather than indexed — a truncated download or a corrupted file on
//! disk is ordinary input for a decoder that runs in a desktop shell, not a
//! bug report.

use super::checksum::crc32;
use super::filter::{self, ADAM7_PASSES};
use super::inflate::zlib_decompress;
use crate::bitmap::{argb, Bitmap};
use crate::color::from_rgb;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The largest width or height this decoder will act on, checked from
/// `IHDR` before anything is allocated.
///
/// Generous for what actually decodes through here — icon themes and
/// thumbnails, where a 16384×16384 image (a quarter of a billion pixels; no
/// real icon or thumbnail comes remotely close) is already an absurd
/// outlier — and small enough that a hostile `IHDR` cannot claim gigabytes
/// from a file that is otherwise a few dozen bytes. `LegeOS` decodes icons
/// and thumbnails from application-supplied and downloaded files, so this
/// bound has to hold *before* trusting anything the file says about its own
/// size, not just before trusting its pixel data.
const MAX_DIMENSION: u32 = 16_384;

/// Why [`decode`] could not read a PNG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Missing the eight-byte PNG signature.
    NotAPng,
    /// A chunk's declared length, or the file itself, ends before it should.
    Truncated,
    /// A chunk's CRC does not match its bytes.
    Corrupt,
    /// `IHDR` is missing, is not the file's first chunk, or is the wrong size.
    BadHeader,
    /// A bit depth or colour type this decoder does not read — see the
    /// module docs for what is covered.
    Unsupported { bit_depth: u8, color_type: u8 },
    /// `IHDR` declares a width or height past [`MAX_DIMENSION`] — refused
    /// before any pixel buffer is allocated, so a tiny file cannot claim an
    /// enormous one.
    ImageTooLarge { width: u32, height: u32 },
    /// `PLTE` or `tRNS` is the wrong size for what `IHDR` declared.
    BadChunk,
    /// A palette-colour pixel references an entry `PLTE` never defined.
    BadPaletteIndex,
    /// The `IDAT` stream's zlib/DEFLATE framing is malformed, is shorter
    /// than the pixel data it claims to hold, or — a decompression bomb —
    /// expands past what `IHDR` says the image needs.
    BadCompression,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::NotAPng => write!(f, "not a PNG file"),
            DecodeError::Truncated => write!(f, "truncated PNG"),
            DecodeError::Corrupt => write!(f, "chunk CRC mismatch"),
            DecodeError::BadHeader => write!(f, "missing or malformed IHDR"),
            DecodeError::Unsupported {
                bit_depth,
                color_type,
            } => {
                write!(
                    f,
                    "unsupported bit depth {bit_depth} / colour type {color_type}"
                )
            }
            DecodeError::ImageTooLarge { width, height } => {
                write!(
                    f,
                    "{width}x{height} exceeds the {MAX_DIMENSION}x{MAX_DIMENSION} limit"
                )
            }
            DecodeError::BadChunk => write!(f, "malformed PLTE or tRNS chunk"),
            DecodeError::BadPaletteIndex => write!(f, "palette index with no PLTE entry"),
            DecodeError::BadCompression => write!(f, "malformed IDAT stream"),
        }
    }
}

impl std::error::Error for DecodeError {}

struct Header {
    width: u32,
    height: u32,
    color_type: u8,
    interlace: u8,
}

/// Channels per pixel for a colour type this decoder reads. Bytes per pixel
/// too, since 8-bit depth is the only one supported.
fn channels(color_type: u8) -> Option<usize> {
    match color_type {
        0 => Some(1), // grey
        2 => Some(3), // RGB
        3 => Some(1), // palette index
        4 => Some(2), // grey + alpha
        6 => Some(4), // RGBA
        _ => None,
    }
}

fn parse_header(data: &[u8]) -> Result<Header, DecodeError> {
    let data: &[u8; 13] = data.try_into().map_err(|_| DecodeError::BadHeader)?;
    let width = u32::from_be_bytes(data[0..4].try_into().unwrap());
    let height = u32::from_be_bytes(data[4..8].try_into().unwrap());
    let bit_depth = data[8];
    let color_type = data[9];
    let compression = data[10];
    let filter_method = data[11];
    let interlace = data[12];
    if width == 0 || height == 0 || compression != 0 || filter_method != 0 || interlace > 1 {
        return Err(DecodeError::BadHeader);
    }
    // Before anything else is trusted about this file: no allocation sized
    // from `width`/`height` happens until this has passed.
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(DecodeError::ImageTooLarge { width, height });
    }
    if bit_depth != 8 || channels(color_type).is_none() {
        return Err(DecodeError::Unsupported {
            bit_depth,
            color_type,
        });
    }
    Ok(Header {
        width,
        height,
        color_type,
        interlace,
    })
}

fn parse_palette(data: &[u8]) -> Result<Vec<[u8; 3]>, DecodeError> {
    if data.is_empty() || data.len() % 3 != 0 {
        return Err(DecodeError::BadChunk);
    }
    Ok(data.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
}

/// The one sample value (or triple) `tRNS` marks fully transparent, for the
/// colour types that carry no alpha channel of their own — or, for a
/// palette, one alpha byte per palette entry.
enum Transparency {
    Grey(u8),
    Rgb(u8, u8, u8),
    Palette(Vec<u8>),
}

fn parse_trns(data: &[u8], color_type: u8) -> Result<Transparency, DecodeError> {
    match color_type {
        // A 2-byte sample per the spec even at 8-bit depth; only the low
        // byte is meaningful at this bit depth.
        0 if data.len() == 2 => Ok(Transparency::Grey(data[1])),
        2 if data.len() == 6 => Ok(Transparency::Rgb(data[1], data[3], data[5])),
        3 => Ok(Transparency::Palette(data.to_vec())),
        _ => Err(DecodeError::BadChunk),
    }
}

fn decode_pixel(
    sample: &[u8],
    color_type: u8,
    palette: Option<&[[u8; 3]]>,
    trns: Option<&Transparency>,
) -> Result<u32, DecodeError> {
    match color_type {
        0 => {
            let g = sample[0];
            let alpha = match trns {
                Some(Transparency::Grey(v)) if *v == g => 0,
                _ => 255,
            };
            Ok(argb(alpha, from_rgb(g, g, g)))
        }
        2 => {
            let (r, g, b) = (sample[0], sample[1], sample[2]);
            let alpha = match trns {
                Some(Transparency::Rgb(tr, tg, tb)) if (*tr, *tg, *tb) == (r, g, b) => 0,
                _ => 255,
            };
            Ok(argb(alpha, from_rgb(r, g, b)))
        }
        3 => {
            let index = sample[0] as usize;
            let palette = palette.ok_or(DecodeError::BadPaletteIndex)?;
            let &[r, g, b] = palette.get(index).ok_or(DecodeError::BadPaletteIndex)?;
            // Entries `tRNS` does not cover default to fully opaque — the
            // common case, since most palette images have no transparency at
            // all and the ones that do rarely need it on every entry.
            let alpha = match trns {
                Some(Transparency::Palette(alphas)) => alphas.get(index).copied().unwrap_or(255),
                _ => 255,
            };
            Ok(argb(alpha, from_rgb(r, g, b)))
        }
        4 => Ok(argb(sample[1], from_rgb(sample[0], sample[0], sample[0]))),
        6 => Ok(argb(sample[3], from_rgb(sample[0], sample[1], sample[2]))),
        _ => unreachable!("parse_header already rejected any other colour type"),
    }
}

#[allow(clippy::too_many_arguments)]
fn scatter_pass(
    bitmap: &mut Bitmap,
    x0: u32,
    y0: u32,
    dx: u32,
    dy: u32,
    pw: u32,
    ph: u32,
    pixels: &[u8],
    color_type: u8,
    bpp: usize,
    palette: Option<&[[u8; 3]]>,
    trns: Option<&Transparency>,
) -> Result<(), DecodeError> {
    for row in 0..ph {
        for col in 0..pw {
            let offset = (row as usize * pw as usize + col as usize) * bpp;
            let sample = pixels
                .get(offset..offset + bpp)
                .ok_or(DecodeError::Truncated)?;
            let pixel = decode_pixel(sample, color_type, palette, trns)?;
            let x = x0 + col * dx;
            let y = y0 + row * dy;
            bitmap.pixels[(y * bitmap.width + x) as usize] = pixel;
        }
    }
    Ok(())
}

fn reconstruct(
    header: &Header,
    raw: &[u8],
    palette: Option<&[[u8; 3]]>,
    trns: Option<&Transparency>,
) -> Result<Bitmap, DecodeError> {
    // `parse_header` already rejected any colour type this returns `None` for.
    let bpp = channels(header.color_type).unwrap();
    let mut bitmap = Bitmap::new(header.width, header.height);

    if header.interlace == 0 {
        let pixels = filter::unfilter(raw, header.width as usize, header.height as usize, bpp)
            .ok_or(DecodeError::Truncated)?;
        scatter_pass(
            &mut bitmap,
            0,
            0,
            1,
            1,
            header.width,
            header.height,
            &pixels,
            header.color_type,
            bpp,
            palette,
            trns,
        )?;
    } else {
        let mut cursor = 0usize;
        for pass in &ADAM7_PASSES {
            let (pw, ph) = filter::pass_dims(pass, header.width, header.height);
            if pw == 0 || ph == 0 {
                continue; // A pass an image this small has nothing in.
            }
            let pass_bytes = (pw as usize * bpp + 1)
                .checked_mul(ph as usize)
                .ok_or(DecodeError::Truncated)?;
            let end = cursor
                .checked_add(pass_bytes)
                .ok_or(DecodeError::Truncated)?;
            let pass_raw = raw.get(cursor..end).ok_or(DecodeError::Truncated)?;
            let pixels = filter::unfilter(pass_raw, pw as usize, ph as usize, bpp)
                .ok_or(DecodeError::Truncated)?;
            scatter_pass(
                &mut bitmap,
                pass.x0,
                pass.y0,
                pass.dx,
                pass.dy,
                pw,
                ph,
                &pixels,
                header.color_type,
                bpp,
                palette,
                trns,
            )?;
            cursor = end;
        }
    }
    Ok(bitmap)
}

/// The number of bytes `IDAT`, once decompressed, holds for a well-formed
/// image this size — one filter byte per scanline plus its pixels, summed
/// per Adam7 pass when interlaced — plus a small margin. This is what
/// [`reconstruct`] actually reads; passing it to [`zlib_decompress`] as an
/// output cap means a stream that expands past it is refused while it is
/// still growing rather than only once it has finished, which is what turns
/// a decompression bomb from "rejected" into "impossible to construct": no
/// amount of compressed input can make the decompressor allocate more than
/// this image could ever legitimately need.
///
/// `None` only if the arithmetic itself would overflow, which
/// [`MAX_DIMENSION`] already rules out in practice — kept `checked` anyway
/// so the bound on the bound cannot itself be the hole.
fn expected_raw_size(header: &Header, bpp: usize) -> Option<usize> {
    let total = if header.interlace == 0 {
        let stride = (header.width as usize).checked_mul(bpp)?;
        stride.checked_add(1)?.checked_mul(header.height as usize)?
    } else {
        let mut total = 0usize;
        for pass in &ADAM7_PASSES {
            let (pw, ph) = filter::pass_dims(pass, header.width, header.height);
            if pw == 0 || ph == 0 {
                continue;
            }
            let stride = (pw as usize).checked_mul(bpp)?;
            let pass_bytes = stride.checked_add(1)?.checked_mul(ph as usize)?;
            total = total.checked_add(pass_bytes)?;
        }
        total
    };
    // Slack for a real encoder's harmless padding, not for a bomb: 1 KiB is
    // nothing next to any image this bound would otherwise allow, and
    // everything next to what a hostile stream would need it for.
    total.checked_add(1024)
}

/// One `length, type, data, crc` chunk. `kind` and `data` are contiguous in
/// the source, so the CRC is checked over a slice rather than an assembled
/// copy.
struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
}

struct ChunkCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ChunkCursor<'a> {
    fn new(data: &'a [u8]) -> Result<ChunkCursor<'a>, DecodeError> {
        if data.get(..8) != Some(&SIGNATURE) {
            return Err(DecodeError::NotAPng);
        }
        Ok(ChunkCursor { data, pos: 8 })
    }

    /// The next chunk, or `Ok(None)` only at the very end of the buffer —
    /// which is itself an error one level up if `IEND` was never seen.
    fn next(&mut self) -> Result<Option<Chunk<'a>>, DecodeError> {
        if self.pos == self.data.len() {
            return Ok(None);
        }
        let header = self
            .data
            .get(self.pos..self.pos + 8)
            .ok_or(DecodeError::Truncated)?;
        let len = u32::from_be_bytes(header[0..4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = header[4..8].try_into().unwrap();
        let data_start = self.pos + 8;
        let data_end = data_start.checked_add(len).ok_or(DecodeError::Truncated)?;
        let crc_end = data_end.checked_add(4).ok_or(DecodeError::Truncated)?;
        let body = self
            .data
            .get(self.pos + 4..crc_end)
            .ok_or(DecodeError::Truncated)?;
        // `body` is `kind ++ data ++ crc`; the CRC covers everything before it.
        let (checked, crc_bytes) = body.split_at(body.len() - 4);
        let crc = u32::from_be_bytes(crc_bytes.try_into().unwrap());
        if crc32(checked) != crc {
            return Err(DecodeError::Corrupt);
        }
        let data = &self.data[data_start..data_end];
        self.pos = crc_end;
        Ok(Some(Chunk { kind, data }))
    }
}

/// Decode a PNG's bytes into a straight-alpha [`Bitmap`]. Never panics: any
/// input this cannot read comes back as an `Err`.
pub fn decode(data: &[u8]) -> Result<Bitmap, DecodeError> {
    let mut cursor = ChunkCursor::new(data)?;
    let first = cursor.next()?.ok_or(DecodeError::BadHeader)?;
    if &first.kind != b"IHDR" {
        return Err(DecodeError::BadHeader);
    }
    let header = parse_header(first.data)?;

    let mut palette: Option<Vec<[u8; 3]>> = None;
    let mut trns: Option<Transparency> = None;
    let mut idat = Vec::new();
    loop {
        let chunk = cursor.next()?.ok_or(DecodeError::Truncated)?; // Ran out of chunks without IEND.
        match &chunk.kind {
            b"IHDR" => return Err(DecodeError::BadHeader), // A second IHDR.
            b"PLTE" => palette = Some(parse_palette(chunk.data)?),
            b"tRNS" => trns = Some(parse_trns(chunk.data, header.color_type)?),
            b"IDAT" => idat.extend_from_slice(chunk.data),
            b"IEND" => break,
            _ => {} // Ancillary metadata this decoder has no use for.
        }
    }

    // `parse_header` already rejected any colour type this returns `None`
    // for, and any width/height past `MAX_DIMENSION` — so `expected_raw_size`
    // failing here would mean the arithmetic itself overflowed, not that the
    // image is merely large.
    let bpp = channels(header.color_type).expect("parse_header validated the colour type");
    let max_raw = expected_raw_size(&header, bpp).ok_or(DecodeError::ImageTooLarge {
        width: header.width,
        height: header.height,
    })?;
    let raw = zlib_decompress(&idat, max_raw).ok_or(DecodeError::BadCompression)?;
    reconstruct(&header, &raw, palette.as_deref(), trns.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitmap::{alpha_of, rgb_of};
    use crate::png::encode::chunk as write_chunk;

    fn stored_zlib(raw: &[u8]) -> Vec<u8> {
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

    /// Build a PNG by hand — the same "stored deflate block" trick the
    /// writer uses — so pixel-level correctness for chunk parsing,
    /// unfiltering, interlacing and colour handling can be checked without a
    /// second PNG library to generate fixtures.
    #[allow(clippy::too_many_arguments)]
    fn build_png(
        width: u32,
        height: u32,
        color_type: u8,
        interlace: u8,
        palette: Option<&[[u8; 3]]>,
        trns: Option<&[u8]>,
        pixel: impl Fn(u32, u32) -> Vec<u8>,
    ) -> Vec<u8> {
        let mut raw = Vec::new();
        if interlace == 0 {
            for y in 0..height {
                raw.push(0);
                for x in 0..width {
                    raw.extend_from_slice(&pixel(x, y));
                }
            }
        } else {
            for pass in &ADAM7_PASSES {
                let (pw, ph) = filter::pass_dims(pass, width, height);
                if pw == 0 || ph == 0 {
                    // Matches `reconstruct`: a pass with nothing in it writes
                    // no scanlines at all, not even a lone filter byte.
                    continue;
                }
                for row in 0..ph {
                    raw.push(0);
                    for col in 0..pw {
                        raw.extend_from_slice(&pixel(
                            pass.x0 + col * pass.dx,
                            pass.y0 + row * pass.dy,
                        ));
                    }
                }
            }
        }

        let mut out = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, color_type, 0, 0, interlace]);
        write_chunk(&mut out, b"IHDR", &ihdr);
        if let Some(pal) = palette {
            let mut data = Vec::new();
            for &[r, g, b] in pal {
                data.extend_from_slice(&[r, g, b]);
            }
            write_chunk(&mut out, b"PLTE", &data);
        }
        if let Some(t) = trns {
            write_chunk(&mut out, b"tRNS", t);
        }
        write_chunk(&mut out, b"IDAT", &stored_zlib(&raw));
        write_chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn a_non_interlaced_rgb_image_decodes_exactly() {
        let png = build_png(3, 2, 2, 0, None, None, |x, y| {
            vec![x as u8 * 10, y as u8 * 20, 5]
        });
        let bitmap = decode(&png).unwrap();
        assert_eq!((bitmap.width, bitmap.height), (3, 2));
        for y in 0..2u32 {
            for x in 0..3u32 {
                let pixel = bitmap.pixel(x, y);
                assert_eq!(alpha_of(pixel), 255);
                assert_eq!(rgb_of(pixel), from_rgb(x as u8 * 10, y as u8 * 20, 5));
            }
        }
    }

    #[test]
    fn greyscale_trns_makes_one_grey_level_transparent() {
        let png = build_png(2, 1, 0, 0, None, Some(&[0, 100]), |x, _| {
            vec![if x == 0 { 100 } else { 50 }]
        });
        let bitmap = decode(&png).unwrap();
        assert_eq!(
            alpha_of(bitmap.pixel(0, 0)),
            0,
            "matches the tRNS grey value"
        );
        assert_eq!(
            alpha_of(bitmap.pixel(1, 0)),
            255,
            "a different grey stays opaque"
        );
    }

    #[test]
    fn rgb_trns_makes_one_colour_transparent() {
        let key = [10u8, 20, 30];
        let trns = [0, key[0], 0, key[1], 0, key[2]];
        let png = build_png(2, 1, 2, 0, None, Some(&trns), |x, _| {
            if x == 0 {
                key.to_vec()
            } else {
                vec![1, 2, 3]
            }
        });
        let bitmap = decode(&png).unwrap();
        assert_eq!(alpha_of(bitmap.pixel(0, 0)), 0);
        assert_eq!(alpha_of(bitmap.pixel(1, 0)), 255);
    }

    #[test]
    fn palette_pixels_look_up_colour_and_per_index_alpha() {
        let palette = [[255, 0, 0], [0, 255, 0], [0, 0, 255]];
        let trns = [255u8, 128, 0]; // index 2 fully transparent, index 1 half
        let png = build_png(3, 1, 3, 0, Some(&palette), Some(&trns), |x, _| {
            vec![x as u8]
        });
        let bitmap = decode(&png).unwrap();
        assert_eq!(rgb_of(bitmap.pixel(0, 0)), from_rgb(255, 0, 0));
        assert_eq!(alpha_of(bitmap.pixel(0, 0)), 255);
        assert_eq!(alpha_of(bitmap.pixel(1, 0)), 128);
        assert_eq!(alpha_of(bitmap.pixel(2, 0)), 0);
    }

    #[test]
    fn palette_entries_trns_does_not_mention_default_to_opaque() {
        let palette = [[1, 2, 3], [4, 5, 6]];
        let trns = [0u8]; // only index 0 given; index 1 must default to 255
        let png = build_png(2, 1, 3, 0, Some(&palette), Some(&trns), |x, _| {
            vec![x as u8]
        });
        let bitmap = decode(&png).unwrap();
        assert_eq!(alpha_of(bitmap.pixel(1, 0)), 255);
    }

    #[test]
    fn a_palette_pixel_with_no_plte_chunk_is_an_error_not_a_panic() {
        let png = build_png(1, 1, 3, 0, None, None, |_, _| vec![0]);
        assert_eq!(decode(&png), Err(DecodeError::BadPaletteIndex));
    }

    #[test]
    fn a_palette_index_past_the_end_of_plte_is_an_error_not_a_panic() {
        let palette = [[1, 2, 3]];
        let png = build_png(1, 1, 3, 0, Some(&palette), None, |_, _| vec![5]);
        assert_eq!(decode(&png), Err(DecodeError::BadPaletteIndex));
    }

    #[test]
    fn grey_alpha_and_rgba_carry_their_own_alpha_channel() {
        let ga = build_png(1, 1, 4, 0, None, None, |_, _| vec![9, 200]);
        let bitmap = decode(&ga).unwrap();
        assert_eq!(rgb_of(bitmap.pixel(0, 0)), from_rgb(9, 9, 9));
        assert_eq!(alpha_of(bitmap.pixel(0, 0)), 200);

        let rgba = build_png(1, 1, 6, 0, None, None, |_, _| vec![1, 2, 3, 77]);
        let bitmap = decode(&rgba).unwrap();
        assert_eq!(rgb_of(bitmap.pixel(0, 0)), from_rgb(1, 2, 3));
        assert_eq!(alpha_of(bitmap.pixel(0, 0)), 77);
    }

    #[test]
    fn interlaced_and_non_interlaced_encodings_of_the_same_image_decode_identically() {
        // The real test of Adam7: not a single hand-picked case but the same
        // pixel source scattered through the seven passes and reassembled,
        // over sizes that land unevenly at every pass boundary.
        let pixel = |x: u32, y: u32| {
            vec![
                (x * 17 % 251) as u8,
                (y * 31 % 251) as u8,
                ((x + y) % 251) as u8,
                255,
            ]
        };
        for (width, height) in [(1, 1), (2, 2), (7, 1), (1, 7), (13, 9), (16, 16), (23, 5)] {
            let flat = build_png(width, height, 6, 0, None, None, pixel);
            let interlaced = build_png(width, height, 6, 1, None, None, pixel);
            let a = decode(&flat).unwrap();
            let b = decode(&interlaced).unwrap();
            assert_eq!(a.pixels, b.pixels, "at {width}x{height}");
        }
    }

    #[test]
    fn a_bad_signature_is_not_a_png() {
        assert_eq!(decode(b"not a png at all"), Err(DecodeError::NotAPng));
    }

    #[test]
    fn a_flipped_bit_in_a_chunk_fails_its_crc() {
        let mut png = build_png(2, 2, 2, 0, None, None, |_, _| vec![1, 2, 3]);
        let last = png.len() - 1; // inside the IEND chunk's CRC
        png[last] ^= 0xff;
        assert_eq!(decode(&png), Err(DecodeError::Corrupt));
    }

    #[test]
    fn an_unsupported_bit_depth_is_refused_cleanly() {
        let mut png = build_png(1, 1, 2, 0, None, None, |_, _| vec![0, 0, 0]);
        // Layout from byte 0: 8 signature, 4 length, 4 "IHDR", 13 IHDR data
        // (4 width, 4 height, then bit depth), 4 CRC.
        let type_start = 8 + 4;
        let data_start = type_start + 4;
        let data_end = data_start + 13;
        png[data_start + 8] = 16; // bit depth
                                  // The header CRC no longer matches; recompute it so this exercises
                                  // "bit depth 16 is unsupported" rather than "the file is corrupt".
        let crc = crc32(&png[type_start..data_end]);
        png[data_end..data_end + 4].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(
            decode(&png),
            Err(DecodeError::Unsupported {
                bit_depth: 16,
                color_type: 2
            })
        );
    }

    #[test]
    fn an_absurd_declared_size_is_refused_before_any_large_allocation() {
        // The exact shape of the exploit this guards against: a tiny file
        // with no real pixel data at all, declaring dimensions so large that
        // `Bitmap::new(width, height)` — called with no bound — would try to
        // allocate gigabytes and abort the process rather than return an
        // `Err`. If this test completes at all (instead of the whole test
        // binary aborting), no such allocation happened: `MAX_DIMENSION` has
        // to be checked from `IHDR` before that call, not after.
        let mut png = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&40_000u32.to_be_bytes());
        ihdr.extend_from_slice(&40_000u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB, non-interlaced
        write_chunk(&mut png, b"IHDR", &ihdr);
        write_chunk(&mut png, b"IDAT", &stored_zlib(&[])); // no real pixel data
        write_chunk(&mut png, b"IEND", &[]);

        assert!(
            png.len() < 100,
            "the file itself stayed tiny: {} bytes",
            png.len()
        );
        assert_eq!(
            decode(&png),
            Err(DecodeError::ImageTooLarge {
                width: 40_000,
                height: 40_000
            })
        );
    }

    #[test]
    fn a_declared_size_at_the_limit_is_still_accepted() {
        // The bound has to actually admit the sizes it claims to, not just
        // reject everything past some smaller, unstated number.
        let mut png = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&MAX_DIMENSION.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8-bit greyscale, 1 row
        write_chunk(&mut png, b"IHDR", &ihdr);
        let row = vec![0u8; MAX_DIMENSION as usize + 1]; // filter byte + one grey byte per pixel
        write_chunk(&mut png, b"IDAT", &stored_zlib(&row));
        write_chunk(&mut png, b"IEND", &[]);

        let bitmap = decode(&png).unwrap();
        assert_eq!(bitmap.width, MAX_DIMENSION);
    }

    #[test]
    fn a_decompressed_stream_far_larger_than_the_header_implies_is_refused() {
        // A declared 4x4 RGB image needs 52 bytes once decompressed (1
        // filter byte + 12 pixel bytes, per row, four rows). A stream that
        // inflates to far more than that is a decompression bomb — not
        // exponential in this construction, but the same threat `inflate`'s
        // own cap exists for — and must be refused rather than allocated.
        let huge_raw = vec![0u8; 5_000];
        let mut png = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&4u32.to_be_bytes());
        ihdr.extend_from_slice(&4u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        write_chunk(&mut png, b"IHDR", &ihdr);
        write_chunk(&mut png, b"IDAT", &stored_zlib(&huge_raw));
        write_chunk(&mut png, b"IEND", &[]);

        assert_eq!(decode(&png), Err(DecodeError::BadCompression));
    }

    #[test]
    fn truncating_a_valid_png_anywhere_never_panics_and_never_falsely_succeeds() {
        let png = build_png(4, 4, 6, 1, None, None, |x, y| {
            vec![x as u8, y as u8, 0, 255]
        });
        for len in 0..png.len() {
            let _ = decode(&png[..len]);
        }
        assert!(decode(&png).is_ok(), "the untruncated file is still valid");
    }

    #[test]
    fn a_png_with_no_iend_is_truncated_rather_than_silently_partial() {
        let mut png = build_png(1, 1, 2, 0, None, None, |_, _| vec![1, 2, 3]);
        let iend_len = 12; // length(4) + "IEND"(4) + crc(4), zero-length data
        png.truncate(png.len() - iend_len);
        assert_eq!(decode(&png), Err(DecodeError::Truncated));
    }

    /// Encoded by ImageMagick (`-interlace PNG -define png:color-type=6`),
    /// not by this crate — real dynamic-Huffman DEFLATE, real Adam7, and a
    /// handful of ancillary chunks (`cHRM`, `bKGD`, `tEXt`) this decoder must
    /// skip over. Pixel `(x, y)` is `(x*40, y*50, 100, 255 if (x+y) even
    /// else 128)`, confirmed against the source image before encoding.
    #[rustfmt::skip]
    const REAL_RGBA_INTERLACED: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
        0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x05, 0x08, 0x06, 0x00, 0x00, 0x01, 0x11, 0x5f, 0xad,
        0x70, 0x00, 0x00, 0x00, 0x20, 0x63, 0x48, 0x52, 0x4d, 0x00, 0x00, 0x7a, 0x26, 0x00, 0x00, 0x80,
        0x84, 0x00, 0x00, 0xfa, 0x00, 0x00, 0x00, 0x80, 0xe8, 0x00, 0x00, 0x75, 0x30, 0x00, 0x00, 0xea,
        0x60, 0x00, 0x00, 0x3a, 0x98, 0x00, 0x00, 0x17, 0x70, 0x9c, 0xba, 0x51, 0x3c, 0x00, 0x00, 0x00,
        0x06, 0x62, 0x4b, 0x47, 0x44, 0x00, 0xff, 0x00, 0xff, 0x00, 0xff, 0xa0, 0xbd, 0xa7, 0x93, 0x00,
        0x00, 0x00, 0x44, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x55, 0xca, 0x41, 0x0d, 0x80, 0x30, 0x18,
        0xc5, 0xe0, 0x6f, 0x04, 0x31, 0x3b, 0xa3, 0x64, 0x22, 0x9e, 0x0f, 0xc0, 0x09, 0x72, 0x70, 0xf5,
        0x73, 0x82, 0x8c, 0x5e, 0x9a, 0x26, 0x45, 0xca, 0x25, 0xd5, 0xdc, 0xa9, 0x0b, 0x86, 0xd4, 0xe2,
        0xa6, 0x49, 0x6a, 0x60, 0xa0, 0x75, 0x39, 0xde, 0x58, 0xc5, 0xc7, 0x2f, 0x9a, 0x2d, 0x47, 0x67,
        0xef, 0x9c, 0xb3, 0x7f, 0xd7, 0xcc, 0x03, 0x5a, 0xe7, 0x12, 0x5b, 0xa4, 0xd0, 0x06, 0x9b, 0x00,
        0x00, 0x00, 0x25, 0x74, 0x45, 0x58, 0x74, 0x64, 0x61, 0x74, 0x65, 0x3a, 0x63, 0x72, 0x65, 0x61,
        0x74, 0x65, 0x00, 0x32, 0x30, 0x32, 0x36, 0x2d, 0x30, 0x38, 0x2d, 0x32, 0x31, 0x54, 0x30, 0x39,
        0x3a, 0x35, 0x36, 0x3a, 0x30, 0x32, 0x2b, 0x30, 0x30, 0x3a, 0x30, 0x30, 0x61, 0xdb, 0xe7, 0x65,
        0x00, 0x00, 0x00, 0x25, 0x74, 0x45, 0x58, 0x74, 0x64, 0x61, 0x74, 0x65, 0x3a, 0x6d, 0x6f, 0x64,
        0x69, 0x66, 0x79, 0x00, 0x32, 0x30, 0x32, 0x36, 0x2d, 0x30, 0x38, 0x2d, 0x32, 0x31, 0x54, 0x30,
        0x39, 0x3a, 0x35, 0x36, 0x3a, 0x30, 0x32, 0x2b, 0x30, 0x30, 0x3a, 0x30, 0x30, 0x10, 0x86, 0x5f,
        0xd9, 0x00, 0x00, 0x00, 0x28, 0x74, 0x45, 0x58, 0x74, 0x64, 0x61, 0x74, 0x65, 0x3a, 0x74, 0x69,
        0x6d, 0x65, 0x73, 0x74, 0x61, 0x6d, 0x70, 0x00, 0x32, 0x30, 0x32, 0x36, 0x2d, 0x30, 0x38, 0x2d,
        0x32, 0x31, 0x54, 0x30, 0x39, 0x3a, 0x35, 0x36, 0x3a, 0x30, 0x39, 0x2b, 0x30, 0x30, 0x3a, 0x30,
        0x30, 0x45, 0x94, 0x2a, 0xfc, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60,
        0x82,
    ];

    #[test]
    fn a_real_encoder_interlaced_rgba_file_decodes_to_the_known_pixels() {
        let bitmap = decode(REAL_RGBA_INTERLACED).unwrap();
        assert_eq!((bitmap.width, bitmap.height), (6, 5));
        for y in 0..5u32 {
            for x in 0..6u32 {
                let pixel = bitmap.pixel(x, y);
                let expected_alpha = if (x + y) % 2 == 0 { 255 } else { 128 };
                assert_eq!(
                    rgb_of(pixel),
                    from_rgb((x * 40) as u8, (y * 50) as u8, 100),
                    "at ({x},{y})"
                );
                assert_eq!(alpha_of(pixel), expected_alpha, "at ({x},{y})");
            }
        }
    }

    /// A sample, not an exhaustive corpus: this machine's icon themes number
    /// in the thousands and running all of them on every `cargo test` would
    /// make the suite noticeably slower for a check that does not gain much
    /// after the first few hundred files. Skips cleanly where the directory
    /// does not exist, so this is not a source of CI flakiness elsewhere.
    #[test]
    fn real_icon_theme_files_decode_without_panicking() {
        let root = std::path::Path::new("/usr/share/icons");
        if !root.exists() {
            return;
        }
        let mut files = Vec::new();
        collect_pngs(root, &mut files, 500);
        assert!(
            files.len() > 50,
            "expected plenty of PNGs under {root:?}, found {}",
            files.len()
        );

        let mut decoded = 0;
        for path in &files {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            match decode(&bytes) {
                Ok(bitmap) => {
                    assert_eq!(
                        bitmap.pixels.len(),
                        (bitmap.width * bitmap.height) as usize,
                        "{path:?}"
                    );
                    decoded += 1;
                }
                // 16-bit icons are the one in-scope-directory file this
                // decoder declines by design; anything else is a real bug.
                Err(DecodeError::Unsupported { .. }) => {}
                Err(other) => panic!("{path:?}: {other}"),
            }
        }
        assert!(decoded > 0, "found real PNGs but decoded none of them");
    }

    fn collect_pngs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if out.len() >= limit {
                return;
            }
            let path = entry.path();
            if path.is_dir() {
                collect_pngs(&path, out, limit);
            } else if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
            {
                out.push(path);
            }
        }
    }
}
