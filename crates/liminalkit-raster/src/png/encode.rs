//! A minimal PNG writer: 8-bit RGB, stored (uncompressed) deflate blocks.
//!
//! Exists so headless screenshots for design review need no dependency. The
//! files are large but every viewer opens them; a screenshot is not an asset.

use std::io::{self, Write};

use super::checksum::{adler32, crc32};
use crate::painter::WindowBuffer;

/// Encode `buffer` (XRGB) as an RGB PNG.
pub fn encode(buffer: &WindowBuffer) -> Vec<u8> {
    let width = buffer.width as usize;
    let height = buffer.height as usize;
    // Filter byte 0 + 3 bytes per pixel per row.
    let mut raw = Vec::with_capacity(height * (1 + width * 3));
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            let p = buffer.pixels[y * width + x];
            raw.push(((p >> 16) & 0xff) as u8);
            raw.push(((p >> 8) & 0xff) as u8);
            raw.push((p & 0xff) as u8);
        }
    }

    let mut out = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 64);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(buffer.width).to_be_bytes());
    ihdr.extend_from_slice(&(buffer.height).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // depth 8, colour type RGB
    chunk(&mut out, b"IHDR", &ihdr);

    let mut idat = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 6);
    idat.extend_from_slice(&[0x78, 0x01]); // zlib header, no compression preset
    let mut blocks = raw.chunks(65535).peekable();
    if raw.is_empty() {
        idat.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        let last = blocks.peek().is_none();
        idat.push(u8::from(last));
        let len = block.len() as u16;
        idat.extend_from_slice(&len.to_le_bytes());
        idat.extend_from_slice(&(!len).to_le_bytes());
        idat.extend_from_slice(block);
    }
    idat.extend_from_slice(&adler32(&raw).to_be_bytes());
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Write `buffer` as a PNG file.
pub fn write<P: AsRef<std::path::Path>>(path: P, buffer: &WindowBuffer) -> io::Result<()> {
    let bytes = encode(buffer);
    let mut file = std::fs::File::create(path)?;
    file.write_all(&bytes)?;
    file.flush()
}

/// Shared with the decoder's tests, which build small PNGs by hand the same
/// way this writer does.
pub(crate) fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode our own stored-block stream back to raw scanlines.
    fn inflate_stored(zlib: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 2;
        loop {
            let last = zlib[i] & 1 == 1;
            let len = u16::from_le_bytes([zlib[i + 1], zlib[i + 2]]) as usize;
            let nlen = u16::from_le_bytes([zlib[i + 3], zlib[i + 4]]);
            assert_eq!(nlen, !(len as u16));
            out.extend_from_slice(&zlib[i + 5..i + 5 + len]);
            i += 5 + len;
            if last {
                break;
            }
        }
        assert_eq!(adler32(&out).to_be_bytes(), zlib[i..i + 4]);
        out
    }

    #[test]
    fn a_small_image_round_trips() {
        let mut buffer = WindowBuffer::new(3, 2);
        buffer
            .pixels
            .copy_from_slice(&[0xff0000, 0x00ff00, 0x0000ff, 0x102030, 0xffffff, 0]);
        let png = encode(&buffer);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        // IHDR at 8: len(4) type(4) data(13) crc(4) = 25 bytes; IDAT follows.
        let idat_len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let zlib = &png[41..41 + idat_len];
        let raw = inflate_stored(zlib);
        assert_eq!(
            raw,
            vec![0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0x10, 0x20, 0x30, 255, 255, 255, 0, 0, 0]
        );
    }

    #[test]
    fn a_large_image_splits_into_stored_blocks() {
        let buffer = WindowBuffer::new(300, 100); // 90,100 raw bytes → 2 blocks
        let png = encode(&buffer);
        let idat_len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        let raw = inflate_stored(&png[41..41 + idat_len]);
        assert_eq!(raw.len(), 100 * (1 + 300 * 3));
    }
}
