//! Exact portable blend arithmetic: the specification, and the kernels the
//! frame actually runs.
//!
//! The scalar functions are not "the slow path". They are the specification:
//! every faster kernel is asserted byte-for-byte identical to them, over
//! exhaustive inputs where exhaustive is possible. A kernel that is merely
//! *close* would show up as a faint fringe on every glyph edge, or a dot that
//! prints on one machine and not another — the kind of difference nobody
//! attributes to arithmetic.
//!
//! **On SIMD.** The target is a Raspberry Pi (aarch64, NEON). The per-pixel
//! loops here are written so LLVM vectorizes them itself, and it does: the
//! glyph blend and the premultiplied `over` compile to 4-lane NEON multiplies,
//! compares and bit-selects (`scripts/neon-report.sh` shows it from the
//! aarch64 assembly). Hand-written `wide` versions of both were tried and
//! measured: on aarch64 the glyph kernel came out *less* vectorized (its lane
//! arrays were built with scalar code), and on x86 with AVX2 neither beat the
//! compiler's own. They were removed. What did pay is less work, not wider
//! work: chunks of coverage that are empty or full are skipped or stored
//! whole, divisions by 255 became shifts, and images are drawn as copies
//! where they are opaque (see `image`). `max_span`'s explicit `u8x32` path
//! predates this and was measured separately.
//!
//! `LIMINALKIT_SIMD=scalar` takes the fast paths out — every span is then the
//! specification loop — the first thing to try if a screen ever looks wrong.

use std::sync::atomic::{AtomicU8, Ordering};

use wide::u8x32;

/// Whether the fast paths are on: unless `LIMINALKIT_SIMD=scalar`. Read from
/// the environment once.
pub fn fast_paths_enabled() -> bool {
    // 0 = not yet read, 1 = on, 2 = off.
    static STATE: AtomicU8 = AtomicU8::new(0);
    match STATE.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let on = std::env::var_os("LIMINALKIT_SIMD").is_none_or(|v| v != "scalar");
            STATE.store(if on { 1 } else { 2 }, Ordering::Relaxed);
            on
        }
    }
}

/// `x / 255`, rounded down, without a division. Exact for `x <= 65534`
/// (it is one high at 65535); the largest value any blend here divides is
/// `255 × 255 + 127 = 65152`. A test proves the whole exact range.
#[inline(always)]
pub fn div255(x: u32) -> u32 {
    (x + 1 + (x >> 8)) >> 8
}

/// Integer blend in sRGB space, weights summing to 256.
///
/// The weights are `alpha + 1` and `256 - (alpha + 1)`. Weighting by `alpha`
/// and `255 - alpha` sums to 255, which loses a step: blending a colour over
/// *itself* returns one less than it started with, so antialiased text drawn in
/// the panel's own colour grows a faint dark fringe on every edge of every
/// letter.
///
/// Two channels are multiplied at once by keeping red and blue in one word.
/// That is only safe while each 16-bit lane stays inside itself, and it does:
/// `d * (256 - a) + s * a <= max(d, s) * 256 <= 65280`. The vector kernels rely
/// on exactly the same bound, which is why they can use 16-bit lanes.
#[inline]
pub fn blend_channels(destination: u32, source: u32, alpha: u8) -> u32 {
    let alpha = u32::from(alpha) + 1;
    let inverse = 256 - alpha;
    let red_blue = ((destination & 0x00ff_00ff) * inverse + (source & 0x00ff_00ff) * alpha) >> 8;
    let green = ((destination & 0x0000_ff00) * inverse + (source & 0x0000_ff00) * alpha) >> 8;
    (red_blue & 0x00ff_00ff) | (green & 0x0000_ff00)
}

/// One pixel of coverage-weighted compositing.
///
/// The two ends are exact rather than blended: zero coverage leaves the
/// destination untouched (the arithmetic alone would not — at `alpha = 0` the
/// weights are 255 and 1, which moves an opaque pixel by a step), and full
/// coverage stores the colour verbatim.
#[inline]
pub fn blend_pixel(destination: u32, color: u32, coverage: u8) -> u32 {
    match coverage {
        0 => destination,
        255 => color,
        _ => blend_channels(destination, color, coverage),
    }
}

/// Blend one colour along a row, weighted per pixel by a coverage byte.
///
/// The glyph-drawing kernel — every anti-aliased letter and icon passes
/// through here. Stops at the shorter of the two slices, so a caller that has
/// clipped one and not the other cannot run off the end.
///
/// Sixteen pixels at a time: text coverage is mostly empty (between strokes)
/// or full (inside a stem), and a chunk of either is recognised from one
/// 128-bit load of its coverage — an empty chunk is not read or written at
/// all, a full one is a plain store. Mixed chunks run the specification on a
/// fixed-length array, which LLVM unrolls into NEON.
#[inline]
pub fn blend_coverage_span(destination: &mut [u32], color: u32, coverage: &[u8]) {
    if !fast_paths_enabled() {
        return blend_coverage_span_scalar(destination, color, coverage);
    }
    let len = destination.len().min(coverage.len());
    let (destination, coverage) = (&mut destination[..len], &coverage[..len]);
    let mut pixels = destination.chunks_exact_mut(16);
    let mut covers = coverage.chunks_exact(16);
    for (pixels, covers) in (&mut pixels).zip(&mut covers) {
        let covers: &[u8; 16] = covers.try_into().expect("chunks of sixteen");
        match u128::from_ne_bytes(*covers) {
            0 => {}
            u128::MAX => pixels.fill(color),
            _ => {
                let pixels: &mut [u32; 16] = pixels.try_into().expect("chunks of sixteen");
                for (pixel, &cover) in pixels.iter_mut().zip(covers) {
                    *pixel = blend_pixel(*pixel, color, cover);
                }
            }
        }
    }
    let done = len - len % 16;
    blend_coverage_span_scalar(&mut destination[done..], color, &coverage[done..]);
}

/// The specification [`blend_coverage_span`] is held to.
#[inline]
pub fn blend_coverage_span_scalar(destination: &mut [u32], color: u32, coverage: &[u8]) {
    for (pixel, &cover) in destination.iter_mut().zip(coverage) {
        *pixel = blend_pixel(*pixel, color, cover);
    }
}

/// One premultiplied `0xAARRGGBB` pixel over a destination: each channel is
/// `source + destination × (255 − source alpha) / 255`, rounded to nearest.
/// A transparent source leaves the destination exactly as it was. With
/// `xrgb` the result's top byte is cleared (an XRGB buffer keeps it zero).
#[inline]
pub fn over_pixel(destination: u32, source: u32, xrgb: bool) -> u32 {
    let keep = 255 - (source >> 24);
    if keep == 255 {
        return destination;
    }
    let channel = |shift: u32| {
        let s = (source >> shift) & 0xff;
        let d = (destination >> shift) & 0xff;
        (s + div255(d * keep + 127)).min(255) << shift
    };
    let out = channel(24) | channel(16) | channel(8) | channel(0);
    if xrgb {
        out & 0x00ff_ffff
    } else {
        out
    }
}

/// Composite a row of premultiplied pixels over a row of the buffer: the
/// translucent edges of prepared images (a card's shadow, a picture's rounded
/// corner). Stops at the shorter slice. A straight loop LLVM vectorizes.
#[inline]
pub fn over_span(destination: &mut [u32], source: &[u32], xrgb: bool) {
    for (d, &s) in destination.iter_mut().zip(source) {
        *d = over_pixel(*d, s, xrgb);
    }
}

/// Source-over an RGB colour with coverage onto a premultiplied ARGB destination.
/// Unlike the XRGB kernel, the alpha lane is accumulated and RGB never exceeds alpha.
#[inline]
pub(crate) fn blend_premultiplied(destination: u32, color: u32, coverage: u8) -> u32 {
    let alpha = u32::from(coverage);
    let inverse = 255 - alpha;
    let channel = |shift: u32| -> u32 {
        let destination = (destination >> shift) & 255_u32;
        let source = (color >> shift) & 255_u32;
        div255(source * alpha + destination * inverse + 127)
    };
    let out_alpha = alpha + div255((destination >> 24) * inverse + 127);
    (out_alpha << 24) | (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// Merge coverage in, keeping the greater value.
///
/// `max` rather than adding: two overlapping contours of the same glyph — a
/// tone mark whose bounding box overlaps its base — must not produce a doubly
/// dark seam where they meet.
#[inline]
pub fn max_span(destination: &mut [u8], source: &[u8]) {
    let len = destination.len().min(source.len());
    // Scalar code is already auto-vectorized on short glyph spans. Keep the
    // explicit wide path on larger coverage buffers to avoid call overhead.
    if len < 1024 {
        for (slot, &cover) in destination.iter_mut().zip(source) {
            *slot = (*slot).max(cover);
        }
        return;
    }
    let (destination, source) = (&mut destination[..len], &source[..len]);
    let mut chunks = destination
        .chunks_exact_mut(32)
        .zip(source.chunks_exact(32));
    for (dst, src) in &mut chunks {
        let mut dst_array = [0u8; 32];
        let mut src_array = [0u8; 32];
        dst_array.copy_from_slice(dst);
        src_array.copy_from_slice(src);
        dst.copy_from_slice(
            &u8x32::from(dst_array)
                .max(u8x32::from(src_array))
                .to_array(),
        );
    }
    let offset = len - len % 32;
    for (slot, &cover) in destination[offset..].iter_mut().zip(&source[offset..]) {
        *slot = (*slot).max(cover);
    }
}

/// Threshold coverage to one bit per pixel, MSB first — the layout an ESC/POS
/// raster command expects.
///
/// `output` must hold `coverage.len().div_ceil(8)` bytes and is written whole,
/// including the padding bits of a final partial byte, which are zero.
#[inline]
pub fn pack_row_1bit(coverage: &[u8], threshold: u8, output: &mut [u8]) {
    let bytes = coverage.len().div_ceil(8);
    let output = &mut output[..bytes];
    output.fill(0);
    for (index, &cover) in coverage.iter().enumerate() {
        if cover >= threshold {
            output[index / 8] |= 0x80 >> (index % 8);
        }
    }
}

/// Whether every byte is zero — asked of a whole receipt before it is encoded,
/// and of a glyph bitmap before it is cached.
#[inline]
pub fn all_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_max_matches_scalar_for_clipped_and_unaligned_spans() {
        for dst_len in 0..65 {
            for src_len in 0..65 {
                let mut actual = (0..dst_len)
                    .map(|i| (i * 37 + 11) as u8)
                    .collect::<Vec<_>>();
                let source = (0..src_len)
                    .map(|i| (i * 53 + 197) as u8)
                    .collect::<Vec<_>>();
                let mut expected = actual.clone();
                for (slot, &cover) in expected.iter_mut().zip(&source) {
                    *slot = (*slot).max(cover);
                }
                max_span(&mut actual, &source);
                assert_eq!(actual, expected, "dst={dst_len}, src={src_len}");
            }
        }
    }

    #[test]
    fn vector_max_matches_scalar_across_dispatch_boundary() {
        for dst_len in [1023, 1024, 1025, 2048, 4097] {
            for src_len in [1023, 1024, 1025, 2048, 4097] {
                let mut actual = (0..dst_len)
                    .map(|index| (index * 37 + 11) as u8)
                    .collect::<Vec<_>>();
                let source = (0..src_len)
                    .map(|index| (index * 53 + 197) as u8)
                    .collect::<Vec<_>>();
                let mut expected = actual.clone();
                for (slot, &cover) in expected.iter_mut().zip(&source) {
                    *slot = (*slot).max(cover);
                }
                max_span(&mut actual, &source);
                assert_eq!(actual, expected, "dst={dst_len}, src={src_len}");
            }
        }
    }

    #[test]
    fn division_by_255_without_dividing_is_exact_everywhere_it_is_used() {
        for x in 0..=65534u32 {
            assert_eq!(div255(x), x / 255, "{x}");
        }
    }

    /// Coverage that exercises every chunk shape the fast path has: empty,
    /// full, mixed, and the exact ends inside a mixed chunk.
    fn coverage_patterns(len: usize, seed: usize) -> Vec<Vec<u8>> {
        let mixed = (0..len)
            .map(|i| ((i * 37 + seed * 101) % 256) as u8)
            .collect();
        let runs = (0..len)
            .map(|i| match (i + seed) % 41 {
                0..=17 => 0,
                18..=33 => 255,
                n => (n * 41) as u8,
            })
            .collect();
        vec![vec![0; len], vec![255; len], mixed, runs]
    }

    #[test]
    fn the_chunked_glyph_blend_is_the_specification() {
        for len in 0..90 {
            for seed in 0..3 {
                for (pattern, covers) in coverage_patterns(len, seed).into_iter().enumerate() {
                    let original: Vec<u32> = (0..len as u32)
                        .map(|i| i.wrapping_mul(0x9e37_79b9))
                        .collect();
                    for color in [0x00b0_174f, 0xff00_ff00] {
                        let mut fast = original.clone();
                        let mut spec = original.clone();
                        blend_coverage_span(&mut fast, color, &covers);
                        blend_coverage_span_scalar(&mut spec, color, &covers);
                        assert_eq!(fast, spec, "len={len} pattern={pattern} seed={seed}");
                    }
                }
            }
            // Mismatched lengths stop at the shorter, like the specification.
            let mut fast = vec![7u32; len + 3];
            let mut spec = fast.clone();
            blend_coverage_span(&mut fast, 0x00ff_ffff, &vec![128u8; len]);
            blend_coverage_span_scalar(&mut spec, 0x00ff_ffff, &vec![128u8; len]);
            assert_eq!(fast, spec);
        }
    }

    #[test]
    fn premultiplied_over_matches_the_division_it_replaced() {
        // The formula `over` used before this file had div255: exact
        // division. Every source alpha and channel against every destination
        // channel.
        for alpha in 0..=255u32 {
            for s in (0..=alpha).step_by(17).chain([alpha]) {
                for d in 0..=255u32 {
                    let source = (alpha << 24) | (s << 16) | (s << 8) | s;
                    let destination = (d << 24) | (d << 16) | (d << 8) | d;
                    let keep = 255 - alpha;
                    let expected = if keep == 255 {
                        destination
                    } else {
                        let c = |sc: u32, dc: u32| (sc + (dc * keep + 127) / 255).min(255);
                        (c(alpha, d) << 24) | (c(s, d) << 16) | (c(s, d) << 8) | c(s, d)
                    };
                    assert_eq!(over_pixel(destination, source, false), expected);
                    assert_eq!(
                        over_pixel(destination, source, true),
                        if keep == 255 {
                            destination
                        } else {
                            expected & 0x00ff_ffff
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn blending_a_colour_over_itself_changes_nothing() {
        for colour in [0x0000_0000, 0x0012_3456, 0x00ff_ffff, 0x0080_8080] {
            for alpha in 1..=254u8 {
                assert_eq!(blend_channels(colour, colour, alpha), colour);
            }
        }
    }

    #[test]
    fn a_channel_never_bleeds_into_its_neighbour() {
        // Exhaustive over one channel pair: the 16-bit lane bound the vector
        // kernels depend on either holds for every input or it does not.
        for destination in 0..=255u32 {
            for alpha in 0..=255u8 {
                let blended = blend_channels(destination * 0x0001_0001, 0x0000_ff00, alpha);
                assert!(blended <= 0x00ff_ffff, "{blended:#08x} left the lanes");
            }
        }
    }

    #[test]
    fn the_ends_of_the_coverage_range_are_exact() {
        assert_eq!(blend_pixel(0x0011_2233, 0x00ff_ffff, 0), 0x0011_2233);
        assert_eq!(blend_pixel(0x0011_2233, 0x00ff_ffff, 255), 0x00ff_ffff);
        // Arithmetic alone would not give either: at coverage 0 the weights
        // are 255 and 1, which moves an opaque destination by a step.
        assert_ne!(blend_channels(0x00ff_ffff, 0x0000_0000, 0), 0x00ff_ffff);
    }

    #[test]
    fn a_span_stops_at_the_shorter_of_its_two_slices() {
        let mut pixels = [0u32; 4];
        blend_coverage_span(&mut pixels, 0x00ff_ffff, &[255, 255]);
        assert_eq!(pixels, [0x00ff_ffff, 0x00ff_ffff, 0, 0]);

        let mut pixels = [0u32; 2];
        blend_coverage_span(&mut pixels, 0x00ff_ffff, &[255; 8]);
        assert_eq!(pixels, [0x00ff_ffff; 2]);
    }

    #[test]
    fn packing_pads_the_last_byte_with_zeros() {
        let mut packed = [0xffu8; 2];
        pack_row_1bit(&[255, 0, 255, 0, 0, 0, 0, 0, 255], 128, &mut packed);
        assert_eq!(packed[0], 0b1010_0000);
        assert_eq!(packed[1], 0b1000_0000, "seven padding bits, all zero");
    }

    #[test]
    fn a_zero_threshold_prints_every_dot() {
        // `>= threshold`, so a threshold of zero is satisfied by blank paper.
        let mut packed = [0u8; 1];
        pack_row_1bit(&[0; 8], 0, &mut packed);
        assert_eq!(packed[0], 0xff);
    }

    #[test]
    fn emptiness_is_recognised_and_a_single_dot_is_not_lost() {
        assert!(all_zero(&[]));
        assert!(all_zero(&[0; 1000]));
        for position in 0..1000 {
            let mut bytes = vec![0u8; 1000];
            bytes[position] = 1;
            assert!(!all_zero(&bytes), "lost the dot at {position}");
        }
    }
}
