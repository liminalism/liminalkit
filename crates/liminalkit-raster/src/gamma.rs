//! Coverage curves: correcting anti-aliased coverage for the blend that
//! follows it.
//!
//! The blend kernels mix a colour into the buffer linearly in sRGB, which is
//! not linear in light. Half coverage of black on white lands at sRGB 128,
//! about a fifth of white's light rather than half of it, so dark text on a
//! light panel comes out heavy with grey fringes; half coverage of white on
//! black lands at the same 128, and light text on a dark panel comes out
//! thin. A *coverage curve* fixes this before the blend rather than in it:
//! the coverage byte goes through a 256-entry table chosen for the text and
//! background lightness, and the blend kernels stay what they are.
//!
//! This is the approach Skia (`SkMaskGamma`), WebRender and font-kit take for
//! grayscale text, with one difference: their table depends on the text
//! colour alone and guesses the background as its perceptual opposite
//! (white under black text). That guess is good for primary text and poor
//! for the grey of secondary text, which it assumes sits on mid-grey and so
//! hardly corrects. [`curve`] takes the background's lightness as well; a
//! caller that can read what it draws on passes it, one that cannot passes
//! [`opposite`] of the text and gets Skia's behaviour. Either way it is a
//! 256-entry table per pair, not a per-pixel conversion of the destination,
//! and only lightness enters it, so a coloured background is corrected as
//! the grey of its luminance.
//!
//! Two numbers shape the table. `gamma` is the exponent of the blend the
//! table emulates: 1.0 is the plain sRGB blend (the identity table), 2.2 is a
//! blend in linear light. `contrast` (Skia's `SK_GAMMA_CONTRAST`) then
//! steepens the curve on a light background, where gamma-correct blending
//! alone makes small dark text look washed out; it tapers to nothing as the
//! background approaches black, where the correction already adds weight.

/// A coverage remapping: `curve[coverage]` is the coverage to blend with.
///
/// Every curve built here maps 0 to 0 and 255 to 255, so an empty pixel
/// stays untouched and a covered one takes the colour verbatim.
pub type CoverageCurve = [u8; 256];

/// The identity curve: coverage blended as it is.
pub const IDENTITY: CoverageCurve = {
    let mut table = [0u8; 256];
    let mut index = 0;
    while index < 256 {
        table[index] = index as u8;
        index += 1;
    }
    table
};

/// The perceived lightness of an `0xRRGGBB` colour under a power-law
/// `gamma`, as a byte: the channels are taken to linear light, weighted by
/// their Rec. 709 luminance, and brought back. Grey `n` gives `n` (to
/// rounding) for any gamma; this is the value [`curve`] is built from, and a
/// natural cache key for its tables.
pub fn luma(color: u32, gamma: f32) -> u8 {
    let gamma = gamma.max(f32::MIN_POSITIVE);
    let linear = |shift: u32| (((color >> shift) & 0xff) as f32 / 255.0).powf(gamma);
    let light = 0.2126 * linear(16) + 0.7152 * linear(8) + 0.0722 * linear(0);
    (light.powf(1.0 / gamma) * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Skia's guess at the background under text of lightness `luma`, for a
/// caller that cannot read it: the perceptual opposite. Its smooth
/// dependence on the text colour avoids visible jumps between neighbouring
/// tables (chromium bug 141425).
pub fn opposite(luma: u8) -> u8 {
    255 - luma
}

/// Whether `gamma` and `contrast` describe the identity curve, so a caller
/// can skip the table entirely and keep the plain blend's fast path.
pub fn is_identity(gamma: f32, contrast: f32) -> bool {
    gamma == 1.0 && contrast == 0.0
}

/// The coverage curve for text of lightness `text` over a background of
/// lightness `background` (both from [`luma`]).
///
/// For each coverage `a`, the table holds the coverage that makes the plain
/// sRGB blend of the text over the background produce what a blend in light
/// of exponent `gamma` would have, after `contrast` has steepened `a`.
/// `gamma` is clamped to 0.1..=10 and `contrast` to 0..=1.
pub fn curve(text: u8, background: u8, gamma: f32, contrast: f32) -> CoverageCurve {
    let gamma = gamma.clamp(0.1, 10.0);
    let contrast = contrast.clamp(0.0, 1.0);
    let to_linear = |value: f32| value.powf(gamma);
    let from_linear = |value: f32| value.powf(1.0 / gamma);

    let source = f32::from(text) / 255.0;
    let destination = f32::from(background) / 255.0;
    let linear_source = to_linear(source);
    let linear_destination = to_linear(destination);
    // Contrast fades out as the background darkens: it is there to keep dark
    // text on a light ground from washing out, and light text on a dark
    // ground needs the opposite.
    let contrast = contrast * linear_destination;
    let steepen = |alpha: f32| alpha + (1.0 - alpha) * contrast * alpha;

    let mut table = [0u8; 256];
    for (index, slot) in table.iter_mut().enumerate() {
        let alpha = steepen(index as f32 / 255.0);
        let corrected = if (source - destination).abs() < 1.0 / 256.0 {
            // Text and background indistinguishable: there is nothing to
            // undo, and the division below would be unstable.
            alpha
        } else {
            let wanted = from_linear(linear_source * alpha + linear_destination * (1.0 - alpha));
            // Undo what the sRGB blend will do with the coverage.
            (wanted - destination) / (source - destination)
        };
        *slot = (corrected * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    table[0] = 0;
    table[255] = 255;
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamma_one_without_contrast_is_the_identity_for_every_colour() {
        for text in (0..=255u8).step_by(3) {
            for background in (0..=255u8).step_by(5) {
                assert_eq!(
                    curve(text, background, 1.0, 0.0),
                    IDENTITY,
                    "{text} on {background}"
                );
            }
        }
        assert!(is_identity(1.0, 0.0));
        assert!(!is_identity(2.2, 0.0));
    }

    #[test]
    fn every_curve_keeps_its_ends_and_never_decreases() {
        for text in (0..=255u8).step_by(15) {
            for background in (0..=255u8).step_by(15) {
                for gamma in [1.0, 1.4, 1.8, 2.2, 3.0] {
                    for contrast in [0.0, 0.25, 0.5, 1.0] {
                        let table = curve(text, background, gamma, contrast);
                        assert_eq!(table[0], 0);
                        assert_eq!(table[255], 255);
                        assert!(
                            table.windows(2).all(|pair| pair[0] <= pair[1]),
                            "{text} on {background}, gamma {gamma} contrast {contrast}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn linear_light_thins_dark_text_and_thickens_light_text() {
        // The point of the exercise, at half coverage.
        let dark = curve(0, 255, 2.2, 0.0);
        let light = curve(255, 0, 2.2, 0.0);
        assert!(dark[128] < 100, "black on white: {}", dark[128]);
        assert!(light[128] > 160, "white on black: {}", light[128]);
        // Grey on white is corrected like black on white, not left alone
        // the way a guessed mid-grey background would leave it.
        let grey = curve(106, 246, 2.2, 0.0);
        assert!(grey[128] < 110, "grey on white: {}", grey[128]);
        assert!(curve(106, opposite(106), 2.2, 0.0)[128] > grey[128]);
    }

    #[test]
    fn a_blend_through_the_curve_is_a_blend_in_light() {
        // Without contrast the table is exactly the linear-light blend, to
        // within the rounding of the table and of the blend.
        let (text, background) = (40u8, 230u8);
        let table = curve(text, background, 2.2, 0.0);
        let light = |v: f32| (v / 255.0).powf(2.2);
        for coverage in [32usize, 64, 128, 192, 224] {
            let a = coverage as f32 / 255.0;
            let wanted = (light(f32::from(text)) * a + light(f32::from(background)) * (1.0 - a))
                .powf(1.0 / 2.2)
                * 255.0;
            let mapped = f32::from(table[coverage]) / 255.0;
            let blended = f32::from(text) * mapped + f32::from(background) * (1.0 - mapped);
            assert!(
                (blended - wanted).abs() < 1.0,
                "{coverage}: {blended} vs {wanted}"
            );
        }
    }

    #[test]
    fn contrast_steepens_text_on_a_light_ground_and_not_on_black() {
        assert!(curve(0, 255, 2.2, 0.5)[128] > curve(0, 255, 2.2, 0.0)[128]);
        assert_eq!(curve(255, 0, 2.2, 0.5), curve(255, 0, 2.2, 0.0));
    }

    #[test]
    fn luma_of_a_grey_is_that_grey() {
        for value in [0u32, 17, 128, 200, 255] {
            let grey = value * 0x0001_0101;
            for gamma in [1.0, 1.8, 2.2] {
                assert!((i32::from(luma(grey, gamma)) - value as i32).abs() <= 1);
            }
        }
        // Green carries most of the luminance, blue the least.
        assert!(luma(0x0000_ff00, 2.2) > luma(0x00ff_0000, 2.2));
        assert!(luma(0x00ff_0000, 2.2) > luma(0x0000_00ff, 2.2));
    }
}
