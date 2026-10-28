//! A decoded raster image: one straight-alpha word per pixel.
//!
//! Mirrors [`WindowBuffer`](crate::WindowBuffer)'s `0x00RRGGBB` layout with
//! an alpha channel added in the top byte, since that is what
//! [`crate::png::decode`] produces and what [`crate::Painter::blit`]
//! composites — the same packing on both sides of that boundary means
//! drawing an icon never has to shuffle channels first.

/// `width × height` pixels, row-major, each `0xAARRGGBB` with **straight**
/// (not premultiplied) alpha — the same convention PNG itself uses, so a
/// decoded pixel needs no further conversion to land here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl Bitmap {
    /// A fully transparent bitmap of the given size.
    pub fn new(width: u32, height: u32) -> Bitmap {
        Bitmap {
            width,
            height,
            pixels: vec![0; width as usize * height as usize],
        }
    }

    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> u32 {
        self.pixels[(y * self.width + x) as usize]
    }
}

/// Pack an alpha byte and an `0x00RRGGBB` colour into one bitmap pixel.
#[inline]
pub const fn argb(alpha: u8, rgb: u32) -> u32 {
    ((alpha as u32) << 24) | (rgb & 0x00ff_ffff)
}

#[inline]
pub const fn alpha_of(pixel: u32) -> u8 {
    (pixel >> 24) as u8
}

/// The colour, in the same `0x00RRGGBB` layout [`WindowBuffer`](crate::WindowBuffer) uses.
#[inline]
pub const fn rgb_of(pixel: u32) -> u32 {
    pixel & 0x00ff_ffff
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_and_unpacking_a_pixel_round_trips() {
        let pixel = argb(0x80, 0x00_11_22_33);
        assert_eq!(alpha_of(pixel), 0x80);
        assert_eq!(rgb_of(pixel), 0x00_11_22_33);
    }

    #[test]
    fn a_new_bitmap_is_fully_transparent() {
        let bitmap = Bitmap::new(3, 2);
        assert_eq!(bitmap.pixels.len(), 6);
        assert!(bitmap.pixels.iter().all(|&p| p == 0));
        assert_eq!(alpha_of(bitmap.pixel(1, 1)), 0);
    }

    #[test]
    fn pixel_indexes_row_major() {
        let mut bitmap = Bitmap::new(3, 2);
        bitmap.pixels[3 + 2] = argb(255, 0xabcdef); // row 1, column 2
        assert_eq!(bitmap.pixel(2, 1), argb(255, 0xabcdef));
    }
}
