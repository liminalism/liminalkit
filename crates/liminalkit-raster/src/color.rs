//! Colour helpers for the XRGB `u32` the buffer holds.

/// Pack `0xRRGGBB` (already the buffer's layout; exists for readability).
#[inline]
pub const fn rgb(hex: u32) -> u32 {
    hex & 0x00ff_ffff
}

#[inline]
pub const fn from_rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

#[inline]
pub const fn red(c: u32) -> u8 {
    ((c >> 16) & 0xff) as u8
}
#[inline]
pub const fn green(c: u32) -> u8 {
    ((c >> 8) & 0xff) as u8
}
#[inline]
pub const fn blue(c: u32) -> u8 {
    (c & 0xff) as u8
}

/// Linear interpolation between two colours in sRGB space, `t` in 0..=1.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    from_rgb(
        ch(red(a), red(b)),
        ch(green(a), green(b)),
        ch(blue(a), blue(b)),
    )
}

/// A CSS-style opacity (0..=1) as the coverage byte the blend kernels take.
#[inline]
pub fn alpha_byte(opacity: f32) -> u8 {
    (opacity.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Relative luminance in 0..=1, for picking a contrasting text colour.
pub fn luminance(c: u32) -> f32 {
    let lin = |v: u8| {
        let s = f32::from(v) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(red(c)) + 0.7152 * lin(green(c)) + 0.0722 * lin(blue(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_ends_are_exact() {
        assert_eq!(mix(0x102030, 0xf0e0d0, 0.0), 0x102030);
        assert_eq!(mix(0x102030, 0xf0e0d0, 1.0), 0xf0e0d0);
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
    }

    #[test]
    fn alpha_byte_rounds() {
        assert_eq!(alpha_byte(0.0), 0);
        assert_eq!(alpha_byte(1.0), 255);
        assert_eq!(alpha_byte(0.5), 128);
    }
}
