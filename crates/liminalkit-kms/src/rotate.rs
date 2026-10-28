//! A panel mounted at a quarter turn: the application draws an upright
//! buffer, and the copy to scan-out turns it.
//!
//! Under a compositor the GPU rotates for free. Here the copy that has to
//! happen anyway (application buffer → scan-out buffer) does it, so rotation
//! costs the difference between a straight copy and a transposing one, not a
//! second pass. Touch is mapped back through the inverse, from the same
//! definition, and a test holds the two to each other.

use liminalkit_raster::WindowBuffer;

/// How far the *image* is turned clockwise to appear upright on the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl std::str::FromStr for Rotation {
    type Err = String;
    fn from_str(text: &str) -> Result<Rotation, String> {
        match text {
            "0" => Ok(Rotation::R0),
            "90" => Ok(Rotation::R90),
            "180" => Ok(Rotation::R180),
            "270" => Ok(Rotation::R270),
            other => Err(format!("rotation is 0, 90, 180 or 270, not {other}")),
        }
    }
}

impl Rotation {
    /// The application's buffer size for a panel of `panel` pixels.
    pub fn logical_size(self, panel: (u32, u32)) -> (u32, u32) {
        match self {
            Rotation::R0 | Rotation::R180 => panel,
            Rotation::R90 | Rotation::R270 => (panel.1, panel.0),
        }
    }

    /// The panel pixel that shows logical pixel `(lx, ly)`.
    #[inline]
    pub fn to_panel(self, panel: (u32, u32), (lx, ly): (u32, u32)) -> (u32, u32) {
        let (pw, ph) = panel;
        match self {
            Rotation::R0 => (lx, ly),
            Rotation::R90 => (pw - 1 - ly, lx),
            Rotation::R180 => (pw - 1 - lx, ph - 1 - ly),
            Rotation::R270 => (ly, ph - 1 - lx),
        }
    }

    /// A continuous panel position (a touch) in logical coordinates: the
    /// inverse of [`Rotation::to_panel`], on positions rather than pixels.
    pub fn to_logical(self, panel: (u32, u32), (px, py): (f64, f64)) -> (f64, f64) {
        let (pw, ph) = (f64::from(panel.0), f64::from(panel.1));
        match self {
            Rotation::R0 => (px, py),
            Rotation::R90 => (py, pw - px),
            Rotation::R180 => (pw - px, ph - py),
            Rotation::R270 => (ph - py, px),
        }
    }

    /// Copy `source` (logical, upright) into a panel-shaped XRGB8888 scan-out
    /// buffer with `pitch` bytes per row.
    ///
    /// This runs on every frame, over every pixel, so it is written for the
    /// vectorizer: when the buffer can be viewed as 32-bit pixels (aligned,
    /// whole-pixel pitch, a little-endian CPU — always true of a mapped dumb
    /// buffer on a Pi) the straight and half-turn copies are masked word
    /// copies, and quarter turns move whole 8×8 tiles through fixed-size
    /// arrays. Anything else takes [`Rotation::copy_bytes`], the reference
    /// the fast path is tested against. Rows are written in order, which is
    /// what write-combined scan-out memory wants.
    pub fn copy(self, source: &WindowBuffer, panel: (u32, u32), dest: &mut [u8], pitch: usize) {
        if cfg!(target_endian = "little") && pitch % 4 == 0 {
            if let Ok(words) = bytemuck::try_cast_slice_mut::<u8, u32>(dest) {
                return self.copy_words(source, panel, words, pitch / 4);
            }
        }
        self.copy_bytes(source, panel, dest, pitch);
    }

    /// The reference copy: one pixel at a time, through [`Rotation::to_panel`]'s
    /// mapping, byte by byte. Correct for any buffer.
    pub fn copy_bytes(
        self,
        source: &WindowBuffer,
        panel: (u32, u32),
        dest: &mut [u8],
        pitch: usize,
    ) {
        let lw = source.width as usize;
        for ly in 0..source.height {
            for lx in 0..source.width {
                let (px, py) = self.to_panel(panel, (lx, ly));
                let at = py as usize * pitch + px as usize * 4;
                let pixel = source.pixels[ly as usize * lw + lx as usize];
                dest[at..at + 4].copy_from_slice(&(pixel & 0x00ff_ffff).to_le_bytes());
            }
        }
    }

    fn copy_words(self, source: &WindowBuffer, panel: (u32, u32), dest: &mut [u32], stride: usize) {
        const MASK: u32 = 0x00ff_ffff;
        let (pw, ph) = (panel.0 as usize, panel.1 as usize);
        let lw = source.width as usize;
        match self {
            Rotation::R0 => {
                for (y, row) in source.pixels.chunks_exact(lw).take(ph).enumerate() {
                    for (d, &s) in dest[y * stride..y * stride + pw].iter_mut().zip(row) {
                        *d = s & MASK;
                    }
                }
            }
            Rotation::R180 => {
                // Not vectorized on aarch64: LLVM will not form a lane
                // reversal from safe Rust here (a fixed-block version was
                // tried; scripts/neon-report.sh shows no `rev64`). Mounting
                // the panel the other way up makes this the straight copy.
                for (y, row) in source.pixels.chunks_exact(lw).rev().take(ph).enumerate() {
                    for (d, &s) in dest[y * stride..y * stride + pw]
                        .iter_mut()
                        .zip(row.iter().rev())
                    {
                        *d = s & MASK;
                    }
                }
            }
            Rotation::R90 | Rotation::R270 => self.copy_turned(source, panel, dest, stride),
        }
    }

    /// Quarter turns, by 8×8 tiles of the source: eight contiguous row loads
    /// into a fixed array, eight contiguous row stores out of it, with no
    /// per-pixel index arithmetic or bounds checks. The transpose itself is
    /// scalar: a NEON one needs shuffle intrinsics (`trn`/`zip`), which means
    /// `unsafe` or nightly `core::simd`, and this workspace forbids the first
    /// and builds on stable. The edges that do not fill a tile go one pixel
    /// at a time.
    fn copy_turned(
        self,
        source: &WindowBuffer,
        panel: (u32, u32),
        dest: &mut [u32],
        stride: usize,
    ) {
        const T: usize = 8;
        const MASK: u32 = 0x00ff_ffff;
        let (lw, lh) = (source.width as usize, source.height as usize);
        let (pw, ph) = (panel.0 as usize, panel.1 as usize);
        let (full_w, full_h) = (lw - lw % T, lh - lh % T);
        let mut tile = [[0u32; T]; T];
        for ly0 in (0..full_h).step_by(T) {
            for lx0 in (0..full_w).step_by(T) {
                for (j, row) in tile.iter_mut().enumerate() {
                    let start = (ly0 + j) * lw + lx0;
                    row.copy_from_slice(&source.pixels[start..start + T]);
                }
                // `i` walks a column across the tile's rows, so it indexes
                // rather than iterates.
                #[allow(clippy::needless_range_loop)]
                for i in 0..T {
                    // Source column lx0 + i becomes one panel row; its pixels
                    // run along that row in ly order (R270) or reversed (R90).
                    let (py, px0) = if self == Rotation::R90 {
                        (lx0 + i, pw - (ly0 + T))
                    } else {
                        (ph - 1 - (lx0 + i), ly0)
                    };
                    let out: &mut [u32; T] = (&mut dest[py * stride + px0..py * stride + px0 + T])
                        .try_into()
                        .expect("eight pixels");
                    for (k, o) in out.iter_mut().enumerate() {
                        let j = if self == Rotation::R90 { T - 1 - k } else { k };
                        *o = tile[j][i] & MASK;
                    }
                }
            }
        }
        // The right and bottom edges of the source that do not fill a tile.
        let mut edge = |lx: usize, ly: usize| {
            let (px, py) = self.to_panel(panel, (lx as u32, ly as u32));
            dest[py as usize * stride + px as usize] = source.pixels[ly * lw + lx] & MASK;
        };
        for ly in 0..lh {
            for lx in full_w..lw {
                edge(lx, ly);
            }
        }
        for ly in full_h..lh {
            for lx in 0..full_w {
                edge(lx, ly);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Rotation; 4] = [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270];

    /// Every logical pixel distinct, so a mis-mapped copy cannot hide.
    fn numbered(width: u32, height: u32) -> WindowBuffer {
        let mut buffer = WindowBuffer::new(width, height);
        for (i, p) in buffer.pixels.iter_mut().enumerate() {
            *p = i as u32 + 1;
        }
        buffer
    }

    /// Check a panel-shaped copy against [`Rotation::to_panel`], pixel by pixel.
    fn assert_mapped(
        rotation: Rotation,
        panel: (u32, u32),
        source: &WindowBuffer,
        dest: &[u8],
        pitch: usize,
    ) {
        let (lw, lh) = rotation.logical_size(panel);
        for ly in 0..lh {
            for lx in 0..lw {
                let (px, py) = rotation.to_panel(panel, (lx, ly));
                let at = py as usize * pitch + px as usize * 4;
                let got = u32::from_le_bytes(dest[at..at + 4].try_into().unwrap());
                assert_eq!(
                    got,
                    source.pixels[(ly * lw + lx) as usize],
                    "{rotation:?} at {lx},{ly}"
                );
            }
        }
    }

    #[test]
    fn the_fast_copy_and_the_reference_both_put_each_pixel_where_to_panel_says() {
        // Odd sizes, exact multiples of the tile, and a padded pitch, so tile
        // edges, row ends and the pitch gap are all exercised.
        for panel in [(37, 21), (32, 16), (41, 24), (8, 8), (1, 1)] {
            for rotation in ALL {
                let (lw, lh) = rotation.logical_size(panel);
                let source = numbered(lw, lh);
                let pitch = panel.0 as usize * 4 + 12;
                // Aligned: the word path.
                let mut words = vec![0u32; pitch / 4 * panel.1 as usize];
                rotation.copy(
                    &source,
                    panel,
                    bytemuck::cast_slice_mut(&mut words[..]),
                    pitch,
                );
                assert_mapped(
                    rotation,
                    panel,
                    &source,
                    bytemuck::cast_slice(&words[..]),
                    pitch,
                );
                // Misaligned by one byte: the reference path, same answer.
                let mut bytes = vec![0u8; pitch * panel.1 as usize + 1];
                rotation.copy(&source, panel, &mut bytes[1..], pitch);
                assert_mapped(rotation, panel, &source, &bytes[1..], pitch);
                // The gap at the end of each row is never written.
                for y in 0..panel.1 as usize {
                    let gap = &bytemuck::cast_slice::<u32, u8>(&words[..])
                        [y * pitch + panel.0 as usize * 4..(y + 1) * pitch];
                    assert!(gap.iter().all(|&b| b == 0), "{rotation:?} row {y}");
                }
            }
        }
    }

    #[test]
    fn a_touch_on_a_pixel_maps_back_to_that_pixel() {
        let panel = (1280, 800);
        for rotation in ALL {
            let (lw, lh) = rotation.logical_size(panel);
            for (lx, ly) in [
                (0, 0),
                (lw - 1, 0),
                (0, lh - 1),
                (lw - 1, lh - 1),
                (lw / 3, lh / 2),
            ] {
                let (px, py) = rotation.to_panel(panel, (lx, ly));
                let centre = (f64::from(px) + 0.5, f64::from(py) + 0.5);
                let (x, y) = rotation.to_logical(panel, centre);
                assert_eq!(
                    (x.floor() as u32, y.floor() as u32),
                    (lx, ly),
                    "{rotation:?}"
                );
            }
        }
    }

    #[test]
    fn quarter_turns_swap_the_shape() {
        assert_eq!(Rotation::R90.logical_size((1280, 800)), (800, 1280));
        assert_eq!(Rotation::R180.logical_size((1280, 800)), (1280, 800));
        assert_eq!("270".parse::<Rotation>(), Ok(Rotation::R270));
        assert!("45".parse::<Rotation>().is_err());
    }

    #[test]
    fn the_top_byte_never_reaches_scan_out() {
        let mut source = WindowBuffer::new(1, 1);
        source.pixels[0] = 0xff12_3456;
        let mut dest = [0xaau8; 4];
        Rotation::R0.copy(&source, (1, 1), &mut dest, 4);
        assert_eq!(dest, [0x56, 0x34, 0x12, 0x00]);
    }
}
