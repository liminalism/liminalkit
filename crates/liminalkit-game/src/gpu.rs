//! The GPU half of the contract: shader source, layouts, and a Rust mirror.
//! **EXPERIMENTAL** — this shader has been validated as WGSL but has not been
//! run on a GPU from this workspace, which has none.
//!
//! liminalkit has no GPU dependency and this module adds none. What it holds
//! is text and numbers: the reference shader as a string
//! ([`layer_wgsl`]), the vertex and uniform layouts it expects, and
//! [`fragment_reference`], the shader's fragment stage written again in Rust
//! so that the arithmetic can be tested here and compared, by a host that has
//! a GPU, against what its GPU produces ([`PARITY`]).
//!
//! # What a host sets up
//!
//! * **Vertex buffer:** [`VERTEX_STRIDE`] bytes per vertex, three attributes
//!   ([`VERTEX_ATTRIBUTES`]): position, texel, and four per-vertex floats.
//! * **Index buffer:** `u32`, triangle list.
//! * **Bind group 0:** binding 0 the uniform block
//!   ([`crate::effect::LayerUniforms::to_bytes`]), binding 1 the layer's
//!   texture (`Bgra8Unorm`, with the layer's mips if it has any), binding 2 a
//!   sampler: linear in every direction, clamp to edge.
//! * **Pipeline:** `vs_layer` with `fs_layer_encoded` for a UNORM target or
//!   `fs_layer_linear` for an sRGB one; premultiplied blending; no depth; no
//!   culling.

use crate::effect::{shade, LayerUniforms};

/// Bytes per vertex: eight `f32`.
pub const VERTEX_STRIDE: u64 = 32;

/// The vertex attributes: shader location, byte offset, format.
pub const VERTEX_ATTRIBUTES: [(u32, u64, &str); 3] = [
    (0, 0, "Float32x2"),
    (1, 8, "Float32x2"),
    (2, 16, "Float32x4"),
];

/// The shader's entry points.
pub const VERTEX_ENTRY: &str = "vs_layer";
pub const FRAGMENT_ENTRY_ENCODED: &str = "fs_layer_encoded";
pub const FRAGMENT_ENTRY_LINEAR: &str = "fs_layer_linear";

const BODY: &str = include_str!("shaders/layer.wgsl");

/// The `Layer` uniform struct, written from [`LayerUniforms::LAYOUT`] so the
/// shader's idea of the block and the bytes Rust packs cannot part company.
pub fn uniforms_struct() -> String {
    let mut out = String::from("struct Layer {\n");
    for (name, kind) in LayerUniforms::LAYOUT {
        out.push_str(&format!("    {name}: {kind},\n"));
    }
    out.push_str("};\n");
    out
}

/// The whole reference shader module: the uniform struct, then the vertex
/// stage, the effects and the two fragment stages.
pub fn layer_wgsl() -> String {
    format!("{}\n{BODY}", uniforms_struct())
}

/// What a linear, clamp-to-edge sampler returns for a premultiplied texture
/// at `uv` texels: the four nearest texels, weighted.
pub fn sample_linear(pixels: &[u32], size: [u32; 2], uv: [f32; 2]) -> [f32; 4] {
    let (width, height) = (size[0] as i64, size[1] as i64);
    if width == 0 || height == 0 || pixels.len() < (width * height) as usize {
        return [0.0; 4];
    }
    let (x, y) = (uv[0] - 0.5, uv[1] - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let texel = |dx: i64, dy: i64| {
        let column = (x0 as i64 + dx).clamp(0, width - 1);
        let row = (y0 as i64 + dy).clamp(0, height - 1);
        let pixel = pixels[(row * width + column) as usize];
        let channel = |shift: u32| ((pixel >> shift) & 0xff) as f32 / 255.0;
        [channel(16), channel(8), channel(0), channel(24)]
    };
    let (a, b, c, d) = (texel(0, 0), texel(1, 0), texel(0, 1), texel(1, 1));
    std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = c[i] + (d[i] - c[i]) * fx;
        top + (bottom - top) * fy
    })
}

/// The shader's `lk_premul`, in Rust: the premultiplied colour one fragment
/// produces for interpolated texel `uv` and per-vertex floats `attrs`
/// (alpha, pixels per texel, level of detail, light), sampling level 0.
///
/// Nothing draws with this. It exists so the shader's arithmetic has a
/// second, testable statement, and so a host can check its GPU against it.
pub fn fragment_reference(
    pixels: &[u32],
    uv: [f32; 2],
    attrs: [f32; 4],
    uniforms: &LayerUniforms,
) -> [f32; 4] {
    let size = [uniforms.size[2], uniforms.size[3]];
    let inset = uv[0].min(size[0] - uv[0]).min(uv[1].min(size[1] - uv[1]));
    let coverage = (0.5 + inset * attrs[1]).clamp(0.0, 1.0);
    let weight = attrs[0].clamp(0.0, 1.0) * coverage * uniforms.params[0];
    let sampled = sample_linear(pixels, [size[0] as u32, size[1] as u32], uv);
    let mut p = shade(sampled, uv, uniforms).map(|c| c * weight);
    let light = attrs[3];
    if light < 0.0 {
        let keep = 1.0 + light.max(-1.0);
        p = [p[0] * keep, p[1] * keep, p[2] * keep, p[3]];
    } else if light > 0.0 && uniforms.params[1] > 0.0 {
        let rim = (1.0 - inset * attrs[1] / uniforms.params[1]).clamp(0.0, 1.0);
        let add = light.min(1.0) * rim * p[3];
        p = [
            (p[0] + add).min(p[3]),
            (p[1] + add).min(p[3]),
            (p[2] + add).min(p[3]),
            p[3],
        ];
    }
    p
}

/// One row of the parity table: an input to the effects and the output they
/// must give.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParityCase {
    pub name: &'static str,
    /// The layer's own pixel, premultiplied.
    pub src: [f32; 4],
    pub texel: [f32; 2],
    pub uniforms: LayerUniforms,
    /// Worked out by hand from the definitions, not by running the code.
    pub expected: [f32; 4],
}

const fn with(
    shimmer_line: [f32; 4],
    shimmer_color: [f32; 4],
    flash: [f32; 4],
    glow_color: [f32; 4],
    glow_shape: [f32; 4],
    scan: [f32; 4],
) -> LayerUniforms {
    LayerUniforms {
        size: [0.0, 0.0, 200.0, 100.0],
        params: [1.0, 0.0, 0.0, 0.0],
        shimmer_line,
        shimmer_color,
        flash,
        glow_color,
        glow_rect: [20.0, 20.0, 180.0, 80.0],
        glow_shape,
        scan,
    }
}

const NONE: [f32; 4] = [0.0; 4];
const LINE: [f32; 4] = [1.0, 0.0, 100.0, 20.0];
const GREY: [f32; 4] = [0.25, 0.25, 0.25, 1.0];

/// Inputs and hand-derived outputs for the effects. [`crate::effect::shade`]
/// is tested against it here; a GPU host runs the same rows through
/// `lk_shade` in its own headless test and expects the same numbers to
/// within a level of a channel (1/255).
pub const PARITY: [ParityCase; 10] = [
    ParityCase {
        name: "no effect",
        src: [0.1, 0.2, 0.3, 0.5],
        texel: [50.0, 50.0],
        uniforms: with(LINE, NONE, NONE, NONE, NONE, NONE),
        expected: [0.1, 0.2, 0.3, 0.5],
    },
    ParityCase {
        name: "shimmer, on the line",
        src: GREY,
        texel: [100.0, 50.0],
        uniforms: with(LINE, [1.0, 1.0, 1.0, 0.4], NONE, NONE, NONE, NONE),
        expected: [0.65, 0.65, 0.65, 1.0],
    },
    ParityCase {
        name: "shimmer, half way to the band's edge",
        src: GREY,
        texel: [110.0, 50.0],
        uniforms: with(LINE, [1.0, 1.0, 1.0, 0.4], NONE, NONE, NONE, NONE),
        expected: [0.475, 0.475, 0.475, 1.0],
    },
    ParityCase {
        name: "shimmer, outside the band",
        src: GREY,
        texel: [130.0, 50.0],
        uniforms: with(LINE, [1.0, 1.0, 1.0, 0.4], NONE, NONE, NONE, NONE),
        expected: GREY,
    },
    ParityCase {
        name: "shimmer over a transparent texel",
        src: NONE,
        texel: [100.0, 50.0],
        uniforms: with(LINE, [1.0, 1.0, 1.0, 0.4], NONE, NONE, NONE, NONE),
        expected: NONE,
    },
    ParityCase {
        name: "flash, capped at alpha",
        src: [0.4, 0.4, 0.4, 0.5],
        texel: [50.0, 50.0],
        uniforms: with(LINE, NONE, [1.0, 0.5, 0.0, 1.0], NONE, NONE, NONE),
        expected: [0.5, 0.5, 0.4, 0.5],
    },
    ParityCase {
        name: "glow halo, half its radius out",
        src: NONE,
        texel: [15.0, 50.0],
        uniforms: with(
            LINE,
            NONE,
            NONE,
            [0.0, 1.0, 1.0, 0.8],
            [0.0, 10.0, 0.0, 0.0],
            NONE,
        ),
        expected: [0.0, 0.2, 0.2, 0.2],
    },
    ParityCase {
        name: "glow behind an opaque texel",
        src: GREY,
        texel: [15.0, 50.0],
        uniforms: with(
            LINE,
            NONE,
            NONE,
            [0.0, 1.0, 1.0, 0.8],
            [0.0, 10.0, 0.0, 0.0],
            NONE,
        ),
        expected: GREY,
    },
    ParityCase {
        name: "scanline crest",
        src: GREY,
        texel: [50.0, 8.0],
        uniforms: with(LINE, NONE, NONE, NONE, NONE, [4.0, 0.5, 0.0, 0.0]),
        expected: [0.125, 0.125, 0.125, 0.5],
    },
    ParityCase {
        name: "scanline trough",
        src: GREY,
        texel: [50.0, 10.0],
        uniforms: with(LINE, NONE, NONE, NONE, NONE, [4.0, 0.5, 0.0, 0.0]),
        expected: GREY,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scalar_specification_gives_the_table() {
        for case in PARITY {
            let got = shade(case.src, case.texel, &case.uniforms);
            for (g, e) in got.iter().zip(case.expected) {
                assert!((g - e).abs() < 1e-5, "{}: {got:?}", case.name);
            }
        }
    }

    #[test]
    fn the_shader_struct_is_the_layout_and_the_block_is_its_size() {
        let text = uniforms_struct();
        let fields: Vec<&str> = text
            .lines()
            .filter_map(|line| line.trim().strip_suffix(": vec4f,"))
            .collect();
        let names: Vec<&str> = LayerUniforms::LAYOUT.iter().map(|(n, _)| *n).collect();
        assert_eq!(fields, names);
        assert_eq!(LayerUniforms::SIZE, names.len() * 16);
        assert_eq!(VERTEX_STRIDE, 8 * 4);
    }

    #[test]
    fn the_module_has_its_stages_and_uses_every_uniform() {
        let module = layer_wgsl();
        for entry in [VERTEX_ENTRY, FRAGMENT_ENTRY_ENCODED, FRAGMENT_ENTRY_LINEAR] {
            assert!(module.contains(&format!("fn {entry}(")), "{entry}");
        }
        assert!(module.starts_with("struct Layer {"));
        for (name, _) in LayerUniforms::LAYOUT {
            assert!(
                module.contains(&format!("layer.{name}")),
                "{name} is unused"
            );
        }
        for (location, _, _) in VERTEX_ATTRIBUTES {
            assert!(module.contains(&format!("@location({location})")));
        }
    }

    #[test]
    fn sampling_is_exact_on_texel_centres_and_blends_between() {
        let pixels = [0xffff_0000, 0x0000_0000, 0xff00_ff00, 0xff00_00ff];
        assert_eq!(
            sample_linear(&pixels, [2, 2], [0.5, 0.5]),
            [1.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(
            sample_linear(&pixels, [2, 2], [1.5, 1.5]),
            [0.0, 0.0, 1.0, 1.0]
        );
        // Between the opaque red and the clear texel: half of each, and the
        // colour stays inside the alpha.
        assert_eq!(
            sample_linear(&pixels, [2, 2], [1.0, 0.5]),
            [0.5, 0.0, 0.0, 0.5]
        );
        // Clamped, not wrapped.
        assert_eq!(
            sample_linear(&pixels, [2, 2], [-5.0, -5.0]),
            [1.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(sample_linear(&pixels, [0, 0], [0.5, 0.5]), [0.0; 4]);
        assert_eq!(
            sample_linear(&pixels[..1], [2, 2], [0.5, 0.5]),
            [0.0; 4],
            "a short buffer"
        );
    }

    #[test]
    fn a_fragment_weighs_by_alpha_coverage_and_opacity() {
        let pixels = vec![0xff80_8080_u32; 16];
        let uniforms = LayerUniforms {
            size: [640.0, 480.0, 4.0, 4.0],
            ..LayerUniforms::IDENTITY
        }
        .for_draw([640, 480], 0.5, 2.0, 0);
        let grey = 128.0 / 255.0;
        let inside = fragment_reference(&pixels, [2.0, 2.0], [1.0, 1.0, 0.0, 0.0], &uniforms);
        assert!((inside[3] - 0.5).abs() < 1e-6 && (inside[0] - grey * 0.5).abs() < 1e-6);
        // On the texture's edge: half covered.
        let edge = fragment_reference(&pixels, [0.0, 2.0], [1.0, 1.0, 0.0, 0.0], &uniforms);
        assert!((edge[3] - 0.25).abs() < 1e-6);
        // In the ring outside it: nothing.
        let ring = fragment_reference(&pixels, [-0.5, 2.0], [1.0, 1.0, 0.0, 0.0], &uniforms);
        assert_eq!(ring[3], 0.0);
        let faded = fragment_reference(&pixels, [2.0, 2.0], [0.5, 1.0, 0.0, 0.0], &uniforms);
        assert!((faded[3] - 0.25).abs() < 1e-6);
        let dark = fragment_reference(&pixels, [2.0, 2.0], [1.0, 1.0, 0.0, -0.5], &uniforms);
        assert!((dark[0] - grey * 0.25).abs() < 1e-6 && (dark[3] - 0.5).abs() < 1e-6);
        // A glint reaches two pixels in from the rim and no further.
        let rim = fragment_reference(&pixels, [0.5, 2.0], [1.0, 1.0, 0.0, 1.0], &uniforms);
        assert!(rim[0] > inside[0] && rim[0] <= rim[3]);
        let middle = fragment_reference(&pixels, [2.0, 2.0], [1.0, 1.0, 0.0, 1.0], &uniforms);
        assert_eq!(middle, inside);
    }
}

#[cfg(all(test, feature = "surfaces"))]
mod against_the_rasterizer {
    use super::*;
    use liminalkit_raster::mesh::{MeshParams, MeshTexture, MeshVertex, TexturedMesh};
    use liminalkit_raster::{Painter, WindowBuffer};

    /// The CPU's integer pixel and the shader's float one are two statements
    /// of the same thing: over a warped, faded, lit quad they must agree to
    /// within a level of a channel.
    #[test]
    fn the_mirror_agrees_with_the_cpu_rasterizer_on_a_warped_quad() {
        let mut texture = WindowBuffer::new(16, 12);
        for (i, pixel) in texture.pixels.iter_mut().enumerate() {
            let alpha = 255 - (i as u32 * 7) % 120;
            let grey = (i as u32 * 29) % (alpha + 1);
            *pixel = (alpha << 24) | (grey << 16) | ((grey / 3) << 8) | (grey / 2);
        }
        // An affine warp (a sheared, scaled quad): one plane per attribute,
        // so a fragment's interpolated values can be computed directly.
        let at = |u: f32, v: f32| [6.0 + u * 1.75 + v * 0.5, 4.0 + v * 1.5 + u * 0.25];
        let corner = |u: f32, v: f32| MeshVertex {
            pos: at(u, v),
            uv: [u, v],
            alpha: 0.9 - u / 40.0,
            px_per_texel: 1.5,
            lod: 0.0,
            light: (v - 6.0) / 12.0,
        };
        let vertices: Vec<f32> = [
            corner(-1.0, -1.0),
            corner(17.0, -1.0),
            corner(17.0, 13.0),
            corner(-1.0, 13.0),
        ]
        .iter()
        .flat_map(|v| v.to_floats())
        .collect();
        let mut dest = WindowBuffer::new(48, 32);
        Painter::new_premultiplied(&mut dest).draw_textured_mesh(
            TexturedMesh {
                vertices: &vertices,
                indices: &[0, 1, 2, 0, 2, 3],
            },
            MeshTexture::new(&texture),
            MeshParams {
                opacity: 0.8,
                glint_px: 3.0,
            },
        );
        let uniforms = LayerUniforms {
            size: [48.0, 32.0, 16.0, 12.0],
            ..LayerUniforms::IDENTITY
        }
        .for_draw([48, 32], 0.8, 3.0, 0);
        // Invert the affine warp for each pixel centre.
        let det = 1.75 * 1.5 - 0.5 * 0.25;
        let mut compared = 0;
        for y in 0..32 {
            for x in 0..48 {
                let (dx, dy) = (x as f32 + 0.5 - 6.0, y as f32 + 0.5 - 4.0);
                let u = (dx * 1.5 - dy * 0.5) / det;
                let v = (dy * 1.75 - dx * 0.25) / det;
                if !(-0.9..16.9).contains(&u) || !(-0.9..12.9).contains(&v) {
                    continue;
                }
                let attrs = [0.9 - u / 40.0, 1.5, 0.0, (v - 6.0) / 12.0];
                let expected = fragment_reference(&texture.pixels, [u, v], attrs, &uniforms);
                let got = dest.pixels[y * 48 + x];
                let channels = [(got >> 16) & 0xff, (got >> 8) & 0xff, got & 0xff, got >> 24];
                for (g, e) in channels.iter().zip(expected) {
                    // One level for each of the two roundings on the CPU
                    // side (the sampler's 8-bit weights, the modulation).
                    assert!(
                        (*g as f32 - e * 255.0).abs() <= 2.0,
                        "{x},{y}: {channels:?} vs {expected:?}"
                    );
                }
                compared += 1;
            }
        }
        assert!(compared > 400, "{compared}");
    }
}
