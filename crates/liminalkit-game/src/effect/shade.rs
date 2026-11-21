//! The scalar specification of a layer's effects.

use super::LayerUniforms;

/// Signed distance from `p` to a rounded rectangle `left, top, right, bottom`
/// with corner radius `corner`: negative inside.
pub(crate) fn rounded_rect_distance(p: [f32; 2], rect: [f32; 4], corner: f32) -> f32 {
    let half = [(rect[2] - rect[0]) / 2.0, (rect[3] - rect[1]) / 2.0];
    let centre = [(rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0];
    let corner = corner.min(half[0]).min(half[1]).max(0.0);
    let q = [
        (p[0] - centre[0]).abs() - (half[0] - corner),
        (p[1] - centre[1]).abs() - (half[1] - corner),
    ];
    let outside = [q[0].max(0.0), q[1].max(0.0)];
    (outside[0] * outside[0] + outside[1] * outside[1]).sqrt() + q[0].max(q[1]).min(0.0) - corner
}

/// Apply a layer's effects to one premultiplied texel.
///
/// `src` is the layer's own pixel, premultiplied, `0..=1` per channel;
/// `texel` is where it is in the layer. The result is premultiplied and
/// never has a colour channel above its alpha. This function is the
/// specification: the reference shader's `lk_shade` is written to it line
/// for line, and the CPU compositor calls it directly.
///
/// In order: the glow goes *under* the layer's pixel; shimmer and flash
/// brighten the layer where it is opaque; scanlines dim everything. Opacity
/// is not applied here — it belongs to the draw.
pub fn shade(src: [f32; 4], texel: [f32; 2], u: &LayerUniforms) -> [f32; 4] {
    shade_row(src, texel, u, scan_keep(texel[1], u))
}

/// How much of a texel on row `y` the scanlines leave: 1 where there are
/// none.
pub(crate) fn scan_keep(y: f32, u: &LayerUniforms) -> f32 {
    if u.scan[1] > 0.0 && u.scan[0] > 0.0 {
        let turn = std::f32::consts::TAU * y / u.scan[0] + u.scan[2];
        1.0 - u.scan[1] * 0.5 * (1.0 + turn.cos())
    } else {
        1.0
    }
}

/// [`shade`] with the scanline factor for the texel's row already worked
/// out: it is the same along a row, and a cosine a texel is most of the cost.
pub(crate) fn shade_row(src: [f32; 4], texel: [f32; 2], u: &LayerUniforms, keep: f32) -> [f32; 4] {
    let mut p = src;

    // Glow: a halo outside the content, a line on its edge, both behind it.
    // Behind an opaque texel there is nothing of it to see, so none is worked
    // out: the sum below would add exactly zero.
    if u.glow_color[3] > 0.0 && p[3] < 1.0 {
        let distance = rounded_rect_distance(texel, u.glow_rect, u.glow_shape[0]);
        let (radius, outline) = (u.glow_shape[1], u.glow_shape[2]);
        let mut level = 0.0_f32;
        if radius > 0.0 && distance > 0.0 {
            let fall = (1.0 - distance / radius).clamp(0.0, 1.0);
            level = fall * fall;
        }
        if outline > 0.0 {
            level = level.max((1.0 - distance.abs() / (outline / 2.0)).clamp(0.0, 1.0));
        }
        let alpha = (level * u.glow_color[3]).clamp(0.0, 1.0);
        let under = 1.0 - p[3];
        p = [
            p[0] + under * u.glow_color[0] * alpha,
            p[1] + under * u.glow_color[1] * alpha,
            p[2] + under * u.glow_color[2] * alpha,
            p[3] + under * alpha,
        ];
    }

    // Shimmer: a band whose brightness falls off smoothly to its edges.
    if u.shimmer_color[3] > 0.0 {
        let along = u.shimmer_line[0] * texel[0] + u.shimmer_line[1] * texel[1] - u.shimmer_line[2];
        let across = along / u.shimmer_line[3];
        let bell = (1.0 - across * across).max(0.0);
        p = brighten(p, u.shimmer_color, u.shimmer_color[3] * bell * bell);
    }

    if u.flash[3] > 0.0 {
        p = brighten(p, u.flash, u.flash[3]);
    }

    if keep != 1.0 {
        p = p.map(|c| c * keep);
    }

    p
}

/// Move a premultiplied pixel toward `color` by `amount` of its own alpha,
/// never past it.
fn brighten(p: [f32; 4], color: [f32; 4], amount: f32) -> [f32; 4] {
    let lift = |c: f32, toward: f32| (c + toward * amount * p[3]).min(p[3]);
    [
        lift(p[0], color[0]),
        lift(p[1], color[1]),
        lift(p[2], color[2]),
        p[3],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPAQUE_GREY: [f32; 4] = [0.25, 0.25, 0.25, 1.0];

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    fn shimmer() -> LayerUniforms {
        LayerUniforms {
            shimmer_line: [1.0, 0.0, 100.0, 20.0],
            shimmer_color: [1.0, 1.0, 1.0, 0.4],
            ..LayerUniforms::IDENTITY
        }
    }

    #[test]
    fn identity_uniforms_return_the_pixel_untouched() {
        for src in [OPAQUE_GREY, [0.0; 4], [0.1, 0.2, 0.3, 0.5], [1.0; 4]] {
            assert_eq!(shade(src, [37.5, 12.5], &LayerUniforms::IDENTITY), src);
        }
    }

    #[test]
    fn a_shimmer_is_brightest_on_its_line_and_gone_at_its_edge() {
        let u = shimmer();
        let centre = shade(OPAQUE_GREY, [100.0, 7.0], &u);
        assert!(close(centre, [0.65, 0.65, 0.65, 1.0]));
        // Half way to the edge: (1 - 0.25)² of the intensity.
        let half = shade(OPAQUE_GREY, [110.0, 7.0], &u);
        let expected = 0.25 + 0.4 * 0.5625;
        assert!(close(half, [expected, expected, expected, 1.0]));
        assert_eq!(
            shade(OPAQUE_GREY, [120.0, 7.0], &u),
            OPAQUE_GREY,
            "the edge"
        );
        assert_eq!(shade(OPAQUE_GREY, [300.0, 7.0], &u), OPAQUE_GREY, "outside");
    }

    #[test]
    fn brightening_is_masked_by_the_layers_own_alpha_and_capped_at_it() {
        let u = shimmer();
        assert_eq!(
            shade([0.0; 4], [100.0, 0.0], &u),
            [0.0; 4],
            "nothing to brighten"
        );
        let half = shade([0.1, 0.1, 0.1, 0.5], [100.0, 0.0], &u);
        assert!(close(half, [0.3, 0.3, 0.3, 0.5]));
        let flash = LayerUniforms {
            flash: [1.0, 0.5, 0.0, 1.0],
            ..LayerUniforms::IDENTITY
        };
        let capped = shade([0.4, 0.4, 0.4, 0.5], [0.0, 0.0], &flash);
        assert!(close(capped, [0.5, 0.5, 0.4, 0.5]), "{capped:?}");
    }

    #[test]
    fn a_glow_fills_only_what_the_layer_left_transparent() {
        let u = LayerUniforms {
            glow_color: [0.0, 1.0, 1.0, 0.8],
            glow_rect: [20.0, 20.0, 180.0, 80.0],
            glow_shape: [0.0, 10.0, 0.0, 0.0],
            ..LayerUniforms::IDENTITY
        };
        // Five texels outside the left edge: (1 - 5/10)² of the intensity.
        let halo = shade([0.0; 4], [15.0, 50.0], &u);
        assert!(close(halo, [0.0, 0.2, 0.2, 0.2]));
        assert_eq!(
            shade([0.0; 4], [5.0, 50.0], &u),
            [0.0; 4],
            "beyond its reach"
        );
        assert_eq!(shade([0.0; 4], [100.0, 50.0], &u), [0.0; 4], "none inside");
        assert_eq!(
            shade(OPAQUE_GREY, [15.0, 50.0], &u),
            OPAQUE_GREY,
            "behind opaque content"
        );
        // An outline two texels wide is full on the edge and gone a texel away.
        let lined = LayerUniforms {
            glow_shape: [0.0, 0.0, 2.0, 0.0],
            ..u
        };
        assert!(close(
            shade([0.0; 4], [20.0, 50.0], &lined),
            [0.0, 0.8, 0.8, 0.8]
        ));
        assert_eq!(shade([0.0; 4], [21.0, 50.0], &lined), [0.0; 4]);
    }

    #[test]
    fn scanlines_dim_by_their_depth_at_a_crest_and_not_at_all_between() {
        let u = LayerUniforms {
            scan: [4.0, 0.5, 0.0, 0.0],
            ..LayerUniforms::IDENTITY
        };
        assert!(close(
            shade(OPAQUE_GREY, [0.0, 0.0], &u),
            [0.125, 0.125, 0.125, 0.5]
        ));
        assert!(close(shade(OPAQUE_GREY, [0.0, 2.0], &u), OPAQUE_GREY));
    }

    #[test]
    fn rounded_corners_measure_from_their_arc() {
        let rect = [0.0, 0.0, 100.0, 60.0];
        assert!((rounded_rect_distance([50.0, 30.0], rect, 10.0) + 30.0).abs() < 1e-5);
        assert!((rounded_rect_distance([110.0, 30.0], rect, 10.0) - 10.0).abs() < 1e-5);
        // The corner's arc is centred at (10, 10): the true corner is outside it.
        let corner = rounded_rect_distance([0.0, 0.0], rect, 10.0);
        assert!((corner - (200.0_f32.sqrt() - 10.0)).abs() < 1e-4);
        assert!(rounded_rect_distance([0.0, 0.0], rect, 0.0).abs() < 1e-6);
    }

    #[test]
    fn no_setting_lifts_a_colour_above_its_alpha() {
        // A small deterministic sweep over everything at once.
        let mut state = 0x1234_5678_u64;
        let mut unit = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 40) as f32) / (1u64 << 24) as f32
        };
        for _ in 0..4_000 {
            let alpha = unit();
            let src = [unit() * alpha, unit() * alpha, unit() * alpha, alpha];
            let u = LayerUniforms {
                shimmer_line: [0.8, 0.6, unit() * 200.0, 1.0 + unit() * 60.0],
                shimmer_color: [unit(), unit(), unit(), unit()],
                flash: [unit(), unit(), unit(), unit()],
                glow_color: [unit(), unit(), unit(), unit()],
                glow_rect: [20.0, 20.0, 180.0, 80.0],
                glow_shape: [unit() * 20.0, unit() * 30.0, unit() * 6.0, 0.0],
                scan: [1.0 + unit() * 8.0, unit(), unit() * 6.0, 0.0],
                ..LayerUniforms::IDENTITY
            };
            let out = shade(src, [unit() * 200.0, unit() * 100.0], &u);
            assert!(out[3] <= 1.0 + 1e-6 && out[3] >= 0.0);
            for c in &out[..3] {
                assert!(*c >= 0.0 && *c <= out[3] + 1e-6, "{out:?} from {src:?}");
            }
        }
    }
}
