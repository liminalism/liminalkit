//! Textured triangle meshes: a premultiplied buffer drawn through a warp.
//!
//! Everything else the painter draws is axis-aligned. This is the one
//! primitive that is not: a mesh of screen-space triangles, each vertex naming
//! the texel it shows, so a panel painted flat into its own buffer can be
//! drawn bent over a sphere, tilted, or half way between a curve and a
//! rectangle. The mesh is the whole description of the warp — the rasterizer
//! knows nothing about what produced it — and it is the same eight floats per
//! vertex a GPU would be handed, so a host that has one draws the identical
//! mesh with a twenty-line shader instead.
//!
//! This function is its own specification; there is no faster twin to hold to
//! it. What it promises:
//!
//! * **Watertight.** Positions snap to 1/256 px and coverage is decided by
//!   integer edge functions under the top-left rule, so a pixel centre on an
//!   edge two triangles share belongs to exactly one of them. A translucent
//!   panel has no doubly blended diagonals and no pinholes.
//! * **Order-free.** Every attribute at a pixel is a closed-form function of
//!   that pixel and its triangle, never an accumulated step, so the result
//!   does not depend on how the triangle was walked.
//! * **Exact where it can be.** A mesh that maps texels onto pixels one to one
//!   reproduces the texture byte for byte (a test holds it to
//!   [`crate::blend::over_span`]).
//!
//! Interpolation is affine in screen space. That is deliberate: the mesh is
//! expected to be tessellated finely enough that a straight line between
//! vertices is within a fraction of a pixel of the true warp, which costs a
//! few hundred triangles and saves a division per pixel.
//!
//! The texture's edge is anti-aliased analytically rather than by the
//! triangles' own silhouette: coverage falls from one to zero across the pixel
//! that straddles the texture's boundary, using the vertex's `px_per_texel` to
//! turn a distance in texels into one in pixels. A mesh therefore extends one
//! texel beyond the texture on every side, and that ring draws nothing where
//! the mapping is one to one.

use crate::blend::over_pixel;
use crate::painter::{Painter, WindowBuffer};

/// Floats per vertex in a [`TexturedMesh`]: see [`MeshVertex`].
pub const VERTEX_FLOATS: usize = 8;

/// The largest coordinate, in pixels, a vertex may have. A triangle with any
/// vertex beyond it (or not finite) is skipped whole: it keeps the edge
/// functions inside 64 bits, and nothing real is half a million pixels wide.
pub const MAX_COORDINATE: f32 = 524_288.0;

/// One mesh vertex. In a vertex buffer it is these eight `f32`s in this order,
/// which is also the layout the reference shader reads.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MeshVertex {
    /// Where it is drawn: device pixels, top-left origin, pixel centres at
    /// `+0.5`.
    pub pos: [f32; 2],
    /// What it shows: texels of the texture's largest level, texel centres at
    /// `+0.5`. May lie outside the texture (the anti-aliasing ring).
    pub uv: [f32; 2],
    /// Opacity here, `0..=1`: how a panel fades toward a horizon.
    pub alpha: f32,
    /// Screen pixels one texel covers here, along its most compressed
    /// direction. Scales the edge anti-aliasing ramp.
    pub px_per_texel: f32,
    /// Mip level to sample, `0` the full texture. Ignored without mips.
    pub lod: f32,
    /// Below zero, darkens (`-1` black); above, adds a highlight along the
    /// texture's rim within [`MeshParams::glint_px`].
    pub light: f32,
}

impl MeshVertex {
    pub fn to_floats(self) -> [f32; VERTEX_FLOATS] {
        [
            self.pos[0],
            self.pos[1],
            self.uv[0],
            self.uv[1],
            self.alpha,
            self.px_per_texel,
            self.lod,
            self.light,
        ]
    }

    /// From the first [`VERTEX_FLOATS`] of `floats`. Panics on a shorter slice.
    pub fn from_floats(floats: &[f32]) -> MeshVertex {
        MeshVertex {
            pos: [floats[0], floats[1]],
            uv: [floats[2], floats[3]],
            alpha: floats[4],
            px_per_texel: floats[5],
            lod: floats[6],
            light: floats[7],
        }
    }
}

/// A triangle list over a flat vertex buffer of [`VERTEX_FLOATS`] floats per
/// vertex. Triangles are drawn in index order, both windings; an index past
/// the buffer skips its triangle.
#[derive(Debug, Clone, Copy)]
pub struct TexturedMesh<'a> {
    pub vertices: &'a [f32],
    pub indices: &'a [u32],
}

/// What a mesh shows: a premultiplied `0xAARRGGBB` buffer (one painted with
/// [`Painter::new_premultiplied`]) and, optionally, its smaller levels, each
/// half the one before, for texels that land smaller than a pixel.
#[derive(Debug, Clone, Copy)]
pub struct MeshTexture<'a> {
    pub base: &'a WindowBuffer,
    pub mips: &'a [WindowBuffer],
}

impl<'a> MeshTexture<'a> {
    pub fn new(base: &'a WindowBuffer) -> MeshTexture<'a> {
        MeshTexture { base, mips: &[] }
    }
}

/// Per-draw constants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshParams {
    /// Opacity of the whole draw, `0..=1`.
    pub opacity: f32,
    /// Width in pixels of the rim a positive [`MeshVertex::light`] brightens;
    /// zero or less brightens nothing.
    pub glint_px: f32,
}

impl Default for MeshParams {
    fn default() -> MeshParams {
        MeshParams {
            opacity: 1.0,
            glint_px: 0.0,
        }
    }
}

/// Blend two premultiplied pixels, `fraction / 256` of the way from `a` to
/// `b`, rounding each channel to nearest. Red and blue share a word, as do
/// alpha and green: each lane holds at most `255 × 256 + 128`, inside 16 bits.
#[inline]
fn lerp_pixel(a: u32, b: u32, fraction: u32) -> u32 {
    let keep = 256 - fraction;
    let red_blue = (((a & 0x00ff_00ff) * keep + (b & 0x00ff_00ff) * fraction + 0x0080_0080) >> 8)
        & 0x00ff_00ff;
    let alpha_green =
        (((a >> 8) & 0x00ff_00ff) * keep + ((b >> 8) & 0x00ff_00ff) * fraction + 0x0080_0080)
            & 0xff00_ff00;
    red_blue | alpha_green
}

/// Scale every channel by `by / 256`, rounded to nearest. `256` is exact.
#[inline]
fn scale_pixel(pixel: u32, by: u32) -> u32 {
    if by >= 256 {
        return pixel;
    }
    let red_blue = (((pixel & 0x00ff_00ff) * by + 0x0080_0080) >> 8) & 0x00ff_00ff;
    let alpha_green = (((pixel >> 8) & 0x00ff_00ff) * by + 0x0080_0080) & 0xff00_ff00;
    red_blue | alpha_green
}

/// Bilinear sample of a premultiplied buffer at `(u, v)` texels in 16.16
/// fixed point, texel centres at `+0.5`, clamped to the edge.
///
/// Integer throughout and never leaves premultiplied space, so a sample on a
/// texel centre is that texel exactly, weights are monotone, and no colour
/// channel can come out above its alpha. Fractions are kept to 8 bits, the
/// precision a GPU's sampler uses. An empty buffer samples as transparent.
pub fn sample_bilinear_premultiplied(texture: &WindowBuffer, u: i64, v: i64) -> u32 {
    let (width, height) = (i64::from(texture.width), i64::from(texture.height));
    if width == 0 || height == 0 {
        return 0;
    }
    let (su, sv) = (u - 32_768, v - 32_768);
    let (x0, y0) = (su >> 16, sv >> 16);
    let (fx, fy) = (((su >> 8) & 255) as u32, ((sv >> 8) & 255) as u32);
    let column = |x: i64| x.clamp(0, width - 1) as usize;
    let row = |y: i64| y.clamp(0, height - 1) as usize * width as usize;
    let (left, right) = (column(x0), column(x0 + 1));
    let (top, bottom) = (row(y0), row(y0 + 1));
    let pixels = &texture.pixels;
    if fx == 0 && fy == 0 {
        // On a texel centre: that texel, with nothing to weigh.
        return pixels[top + left];
    }
    let upper = lerp_pixel(pixels[top + left], pixels[top + right], fx);
    if fy == 0 {
        return upper;
    }
    let lower = lerp_pixel(pixels[bottom + left], pixels[bottom + right], fx);
    lerp_pixel(upper, lower, fy)
}

/// Round to nearest, halves away from zero: what `f64::round` gives, without
/// the library call it costs on targets that have no rounding instruction.
/// This runs twice a pixel.
#[inline]
fn round_to_i64(value: f64) -> i64 {
    (value + if value >= 0.0 { 0.5 } else { -0.5 }) as i64
}

/// Sample `texture` at `(u, v)` texels of its base level and mip level `lod`.
#[inline]
fn sample(texture: &MeshTexture<'_>, u: f64, v: f64, lod: f64) -> u32 {
    let fixed = |t: f64| round_to_i64(t * 65_536.0);
    if texture.mips.is_empty() || lod.is_nan() || lod <= 0.0 {
        return sample_bilinear_premultiplied(texture.base, fixed(u), fixed(v));
    }
    let lod = lod.min(texture.mips.len() as f64);
    let level = lod.floor() as usize;
    let fraction = ((lod - level as f64) * 256.0).round() as u32;
    let at = |level: usize| {
        let buffer = if level == 0 {
            texture.base
        } else {
            &texture.mips[level - 1]
        };
        let scale = 1.0 / f64::from(1u32 << level.min(30));
        sample_bilinear_premultiplied(buffer, fixed(u * scale), fixed(v * scale))
    };
    let near = at(level);
    if fraction == 0 || level >= texture.mips.len() {
        return near;
    }
    lerp_pixel(near, at(level + 1), fraction)
}

/// `(b - a) × (p - a)`: positive when `p` is on the inside of edge `a → b`
/// for a triangle wound clockwise on a y-down screen.
#[inline]
fn orient(a: (i64, i64), b: (i64, i64), p: (i64, i64)) -> i64 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

/// The top-left rule's tie-break for edge `a → b` of a clockwise triangle:
/// a pixel centre exactly on a top or a left edge is in, on any other out.
#[inline]
fn edge_bias(a: (i64, i64), b: (i64, i64)) -> i64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    if (dy == 0 && dx > 0) || dy < 0 {
        0
    } else {
        -1
    }
}

/// One interpolated attribute as a plane over screen pixels.
#[derive(Clone, Copy)]
struct Plane {
    base: f64,
    dx: f64,
    dy: f64,
}

impl Plane {
    #[inline]
    fn at(self, x: f64, y: f64) -> f64 {
        self.base + self.dx * x + self.dy * y
    }
}

impl Painter<'_> {
    /// Draw `texture` through `mesh`, source-over, inside the clip. See the
    /// [module documentation](self) for what is guaranteed.
    pub fn draw_textured_mesh(
        &mut self,
        mesh: TexturedMesh<'_>,
        texture: MeshTexture<'_>,
        params: MeshParams,
    ) {
        let opacity = f64::from(params.opacity);
        if opacity.is_nan() || opacity <= 0.0 || texture.base.pixels.is_empty() {
            return;
        }
        let opacity = opacity.min(1.0);
        let count = mesh.vertices.len() / VERTEX_FLOATS;
        let vertex = |index: u32| {
            let index = index as usize;
            (index < count).then(|| {
                MeshVertex::from_floats(&mesh.vertices[index * VERTEX_FLOATS..][..VERTEX_FLOATS])
            })
        };
        for triangle in mesh.indices.chunks_exact(3) {
            let (Some(a), Some(b), Some(c)) = (
                vertex(triangle[0]),
                vertex(triangle[1]),
                vertex(triangle[2]),
            ) else {
                continue;
            };
            self.draw_triangle([a, b, c], &texture, opacity, f64::from(params.glint_px));
        }
    }

    fn draw_triangle(
        &mut self,
        mut vertices: [MeshVertex; 3],
        texture: &MeshTexture<'_>,
        opacity: f64,
        glint_px: f64,
    ) {
        let snap = |v: &MeshVertex| -> Option<(i64, i64)> {
            let ok = |p: f32| p.is_finite() && p.abs() <= MAX_COORDINATE;
            (ok(v.pos[0]) && ok(v.pos[1])).then(|| {
                (
                    (f64::from(v.pos[0]) * 256.0).round() as i64,
                    (f64::from(v.pos[1]) * 256.0).round() as i64,
                )
            })
        };
        let (Some(a), Some(mut b), Some(mut c)) =
            (snap(&vertices[0]), snap(&vertices[1]), snap(&vertices[2]))
        else {
            return;
        };
        let area = orient(a, b, c);
        if area == 0 {
            return;
        }
        if area < 0 {
            // Two-sided: wind it clockwise and carry on.
            std::mem::swap(&mut b, &mut c);
            vertices.swap(1, 2);
        }

        // Pixels whose centres can be inside: centre `x * 256 + 128`.
        let clip = self.clip();
        let first = |min: i64| (min - 128 + 255).div_euclid(256);
        let last = |max: i64| (max - 128).div_euclid(256);
        let x0 = first(a.0.min(b.0).min(c.0)).max(i64::from(clip.x));
        let x1 = last(a.0.max(b.0).max(c.0)).min(i64::from(clip.right()) - 1);
        let y0 = first(a.1.min(b.1).min(c.1)).max(i64::from(clip.y));
        let y1 = last(a.1.max(b.1).max(c.1)).min(i64::from(clip.bottom()) - 1);
        if x0 > x1 || y0 > y1 {
            return;
        }

        // Attribute planes, in pixels relative to vertex `a` as snapped.
        let px = |p: (i64, i64)| (p.0 as f64 / 256.0, p.1 as f64 / 256.0);
        let (pa, pb, pc) = (px(a), px(b), px(c));
        let (abx, aby, acx, acy) = (pb.0 - pa.0, pb.1 - pa.1, pc.0 - pa.0, pc.1 - pa.1);
        let denominator = abx * acy - aby * acx;
        let plane = |get: fn(&MeshVertex) -> f32| {
            let (va, vb, vc) = (
                f64::from(get(&vertices[0])),
                f64::from(get(&vertices[1])),
                f64::from(get(&vertices[2])),
            );
            Plane {
                base: va,
                dx: ((vb - va) * acy - (vc - va) * aby) / denominator,
                dy: ((vc - va) * abx - (vb - va) * acx) / denominator,
            }
        };
        let u = plane(|v| v.uv[0]);
        let v = plane(|v| v.uv[1]);
        let alpha = plane(|v| v.alpha);
        let scale = plane(|v| v.px_per_texel);
        let lod = plane(|v| v.lod);
        let light = plane(|v| v.light);
        let lit = vertices.iter().any(|v| v.light != 0.0);
        if [u, v, alpha, scale, lod, light]
            .iter()
            .any(|p| !(p.base.is_finite() && p.dx.is_finite() && p.dy.is_finite()))
        {
            return;
        }

        let (width, height) = (
            f64::from(texture.base.width),
            f64::from(texture.base.height),
        );
        let xrgb = !self.is_premultiplied();
        let biases = [edge_bias(b, c), edge_bias(c, a), edge_bias(a, b)];
        // Moving one pixel right changes `orient(a, b, p)` by `-(b.y - a.y) * 256`.
        let steps = [-(c.1 - b.1) * 256, -(a.1 - c.1) * 256, -(b.1 - a.1) * 256];
        let run = (x1 - x0 + 1) as usize;
        for y in y0..=y1 {
            let Some((skip, row)) = self.clipped_run(x0 as i32, y as i32, run) else {
                continue;
            };
            let start = (x0 + skip as i64) * 256 + 128;
            let centre = (start, y * 256 + 128);
            let mut w = [
                orient(b, c, centre) + biases[0],
                orient(c, a, centre) + biases[1],
                orient(a, b, centre) + biases[2],
            ];
            let dy = (centre.1 - a.1) as f64 / 256.0;
            for (i, pixel) in row.iter_mut().enumerate() {
                let inside = (w[0] | w[1] | w[2]) >= 0;
                w[0] += steps[0];
                w[1] += steps[1];
                w[2] += steps[2];
                if !inside {
                    continue;
                }
                let dx = (start + i as i64 * 256 - a.0) as f64 / 256.0;
                let (tu, tv) = (u.at(dx, dy), v.at(dx, dy));
                // Signed distance, in texels, inside the texture's edge.
                let inset = tu.min(width - tu).min(tv.min(height - tv));
                let per_texel = scale.at(dx, dy);
                let coverage = (0.5 + inset * per_texel).clamp(0.0, 1.0);
                let weight = alpha.at(dx, dy).clamp(0.0, 1.0) * coverage * opacity;
                // Never negative, so adding a half and truncating rounds.
                let weight = (weight * 256.0 + 0.5) as u32;
                if weight == 0 {
                    continue;
                }
                let mut source = sample(texture, tu, tv, lod.at(dx, dy));
                if source >> 24 == 0 {
                    continue;
                }
                if weight >= 256 && source >> 24 == 0xff && !lit {
                    // Fully covered by an opaque texel: it replaces what was
                    // there, which is all source-over would have done.
                    *pixel = if xrgb { source & 0x00ff_ffff } else { source };
                    continue;
                }
                source = scale_pixel(source, weight);
                if lit {
                    let rim = if glint_px > 0.0 {
                        (1.0 - inset * per_texel / glint_px).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    source = relight(source, light.at(dx, dy), rim);
                }
                *pixel = over_pixel(*pixel, source, xrgb);
            }
        }
    }
}

/// Darken a premultiplied pixel (`light < 0`) or add a white highlight
/// weighted by `rim` (`light > 0`), never taking a channel above its alpha.
#[inline]
fn relight(pixel: u32, light: f64, rim: f64) -> u32 {
    let alpha = pixel >> 24;
    if light < 0.0 {
        let keep = ((1.0 + light.max(-1.0)) * 256.0).round() as u32;
        let dimmed = scale_pixel(pixel & 0x00ff_ffff, keep);
        return (alpha << 24) | (dimmed & 0x00ff_ffff);
    }
    let add = (light.min(1.0) * rim * f64::from(alpha)).round() as u32;
    if add == 0 {
        return pixel;
    }
    let channel = |shift: u32| (((pixel >> shift) & 0xff) + add).min(alpha) << shift;
    (alpha << 24) | channel(16) | channel(8) | channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::over_span;
    use crate::painter::Rect;

    /// A small deterministic generator: tests must not depend on a clock.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as u32
        }
        fn unit(&mut self) -> f32 {
            (self.next() % 10_000) as f32 / 10_000.0
        }
    }

    /// A premultiplied texture with a different valid pixel in every texel.
    fn texture(width: u32, height: u32, seed: u64) -> WindowBuffer {
        let mut random = Lcg(seed);
        let mut buffer = WindowBuffer::new(width, height);
        for pixel in &mut buffer.pixels {
            let alpha = match random.next() % 4 {
                0 => 255,
                1 => 0,
                _ => random.next() % 256,
            };
            let channel = |r: u32| (r % 256) * alpha / 255;
            *pixel = (alpha << 24)
                | (channel(random.next()) << 16)
                | (channel(random.next()) << 8)
                | channel(random.next());
        }
        buffer
    }

    fn vertex(pos: [f32; 2], uv: [f32; 2]) -> MeshVertex {
        MeshVertex {
            pos,
            uv,
            alpha: 1.0,
            px_per_texel: 1.0,
            lod: 0.0,
            light: 0.0,
        }
    }

    fn floats(vertices: &[MeshVertex]) -> Vec<f32> {
        vertices.iter().flat_map(|v| v.to_floats()).collect()
    }

    /// The texture mapped one texel to one pixel at `(x, y)`, with the
    /// one-texel ring a real mesh carries.
    fn identity(texture: &WindowBuffer, x: f32, y: f32) -> (Vec<f32>, Vec<u32>) {
        let (w, h) = (texture.width as f32, texture.height as f32);
        let corner = |u: f32, v: f32| vertex([x + u, y + v], [u, v]);
        (
            floats(&[
                corner(-1.0, -1.0),
                corner(w + 1.0, -1.0),
                corner(w + 1.0, h + 1.0),
                corner(-1.0, h + 1.0),
            ]),
            vec![0, 1, 2, 0, 2, 3],
        )
    }

    fn ground(width: u32, height: u32) -> WindowBuffer {
        let mut buffer = WindowBuffer::new(width, height);
        for (i, pixel) in buffer.pixels.iter_mut().enumerate() {
            *pixel = 0x0020_4060 + (i as u32 % 7) * 0x0001_0101;
        }
        buffer
    }

    #[test]
    fn a_one_to_one_mesh_is_the_texture_byte_for_byte() {
        for seed in 0..6 {
            let source = texture(13, 9, seed);
            let (vertices, indices) = identity(&source, 4.0, 3.0);
            for premultiplied in [false, true] {
                let mut actual = ground(24, 16);
                if premultiplied {
                    for pixel in &mut actual.pixels {
                        *pixel |= 0xff00_0000;
                    }
                }
                let mut expected = actual.clone();
                let mut painter = if premultiplied {
                    Painter::new_premultiplied(&mut actual)
                } else {
                    Painter::new(&mut actual)
                };
                painter.draw_textured_mesh(
                    TexturedMesh {
                        vertices: &vertices,
                        indices: &indices,
                    },
                    MeshTexture::new(&source),
                    MeshParams::default(),
                );
                for row in 0..9usize {
                    let at = (row + 3) * 24 + 4;
                    over_span(
                        &mut expected.pixels[at..at + 13],
                        &source.pixels[row * 13..(row + 1) * 13],
                        !premultiplied,
                    );
                }
                assert_eq!(actual.pixels, expected.pixels, "seed {seed}");
            }
        }
    }

    #[test]
    fn winding_does_not_matter() {
        let source = texture(8, 8, 3);
        let (vertices, _) = identity(&source, 2.0, 2.0);
        let draw = |indices: &[u32]| {
            let mut buffer = ground(14, 14);
            Painter::new(&mut buffer).draw_textured_mesh(
                TexturedMesh {
                    vertices: &vertices,
                    indices,
                },
                MeshTexture::new(&source),
                MeshParams::default(),
            );
            buffer.pixels
        };
        assert_eq!(draw(&[0, 1, 2, 0, 2, 3]), draw(&[0, 2, 1, 0, 3, 2]));
    }

    /// A grid over `[2, 2 + cells * 5]²` with its inner vertices jittered,
    /// every vertex showing the middle of the texture so coverage is full.
    fn jittered_grid(cells: usize, random: &mut Lcg) -> (Vec<f32>, Vec<u32>) {
        let mut vertices = Vec::new();
        for j in 0..=cells {
            for i in 0..=cells {
                let inner = i > 0 && i < cells && j > 0 && j < cells;
                // Under a fifth of a cell each way: no triangle can fold over.
                let jitter = |r: f32| if inner { (r - 0.5) * 1.8 } else { 0.0 };
                let mut v = vertex(
                    [
                        2.0 + i as f32 * 5.0 + jitter(random.unit()),
                        2.0 + j as f32 * 5.0 + jitter(random.unit()),
                    ],
                    [8.0, 8.0],
                );
                v.px_per_texel = 8.0;
                vertices.push(v);
            }
        }
        let mut indices = Vec::new();
        let at = |i: usize, j: usize| (j * (cells + 1) + i) as u32;
        for j in 0..cells {
            for i in 0..cells {
                // Alternate the diagonal and the winding.
                if (i + j) % 2 == 0 {
                    indices.extend([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                    indices.extend([at(i, j), at(i, j + 1), at(i + 1, j + 1)]);
                } else {
                    indices.extend([at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]);
                    indices.extend([at(i, j), at(i + 1, j), at(i, j + 1)]);
                }
            }
        }
        (floats(&vertices), indices)
    }

    #[test]
    fn every_pixel_of_a_mesh_is_drawn_exactly_once() {
        // A half-transparent texture: a pixel blended twice is darker than one
        // blended once, and a missed one is still the ground.
        let mut source = WindowBuffer::new(16, 16);
        source.pixels.fill(0x8040_4040);
        let once = over_pixel(0xff00_0000, 0x8040_4040, false);
        let mut random = Lcg(11);
        for cells in [1usize, 2, 3, 5, 8] {
            for _ in 0..12 {
                let (vertices, indices) = jittered_grid(cells, &mut random);
                let size = 4 + cells as u32 * 5;
                let mut buffer = WindowBuffer::new(size, size);
                buffer.pixels.fill(0xff00_0000);
                Painter::new_premultiplied(&mut buffer).draw_textured_mesh(
                    TexturedMesh {
                        vertices: &vertices,
                        indices: &indices,
                    },
                    MeshTexture::new(&source),
                    MeshParams::default(),
                );
                let covered = Rect::new(2, 2, cells as i32 * 5, cells as i32 * 5);
                for y in 0..size as i32 {
                    for x in 0..size as i32 {
                        let pixel = buffer.pixels[(y * size as i32 + x) as usize];
                        let expected = if covered.contains(x, y) {
                            once
                        } else {
                            0xff00_0000
                        };
                        assert_eq!(pixel, expected, "cells {cells} at {x},{y}");
                    }
                }
            }
        }
    }

    #[test]
    fn nothing_is_written_outside_the_clip() {
        let source = texture(20, 20, 5);
        let (vertices, indices) = identity(&source, -3.0, -3.0);
        let mut buffer = ground(16, 16);
        let before = buffer.clone();
        let mut painter = Painter::new(&mut buffer);
        painter.push_clip(Rect::new(5, 6, 4, 3));
        painter.draw_textured_mesh(
            TexturedMesh {
                vertices: &vertices,
                indices: &indices,
            },
            MeshTexture::new(&source),
            MeshParams::default(),
        );
        painter.pop_clip();
        for y in 0..16 {
            for x in 0..16 {
                if !Rect::new(5, 6, 4, 3).contains(x, y) {
                    let at = (y * 16 + x) as usize;
                    assert_eq!(buffer.pixels[at], before.pixels[at], "{x},{y}");
                }
            }
        }
    }

    #[test]
    fn the_ring_fades_the_edge_across_one_pixel() {
        // An opaque white texture drawn half a pixel off the grid: the pixels
        // straddling its left and right edges are half covered.
        let mut source = WindowBuffer::new(4, 4);
        source.pixels.fill(0xffff_ffff);
        let (vertices, indices) = identity(&source, 2.5, 2.0);
        let mut buffer = WindowBuffer::new(10, 8);
        Painter::new_premultiplied(&mut buffer).draw_textured_mesh(
            TexturedMesh {
                vertices: &vertices,
                indices: &indices,
            },
            MeshTexture::new(&source),
            MeshParams::default(),
        );
        let row = &buffer.pixels[3 * 10..4 * 10];
        assert_eq!(row[1], 0, "outside");
        assert_eq!(row[2] >> 24, 0x80, "half over the left edge");
        assert_eq!(row[3], 0xffff_ffff);
        assert_eq!(row[5], 0xffff_ffff);
        assert_eq!(row[6] >> 24, 0x80, "half over the right edge");
        assert_eq!(row[7], 0, "outside");
    }

    #[test]
    fn a_warped_draw_never_lifts_colour_above_alpha() {
        let mut random = Lcg(29);
        for seed in 0..20 {
            let source = texture(12, 10, 100 + seed);
            let mut corner = |u: f32, v: f32| {
                let mut c = vertex(
                    [random.unit() * 30.0 + 1.0, random.unit() * 30.0 + 1.0],
                    [u, v],
                );
                c.alpha = random.unit();
                c.px_per_texel = random.unit() * 3.0;
                c.light = random.unit() * 2.0 - 1.0;
                c
            };
            let vertices = floats(&[
                corner(-1.0, -1.0),
                corner(13.0, -1.0),
                corner(13.0, 11.0),
                corner(-1.0, 11.0),
            ]);
            let mut buffer = WindowBuffer::new(32, 32);
            Painter::new_premultiplied(&mut buffer).draw_textured_mesh(
                TexturedMesh {
                    vertices: &vertices,
                    indices: &[0, 1, 2, 0, 2, 3],
                },
                MeshTexture::new(&source),
                MeshParams {
                    opacity: 0.8,
                    glint_px: 3.0,
                },
            );
            for &pixel in &buffer.pixels {
                let alpha = pixel >> 24;
                for shift in [16, 8, 0] {
                    assert!((pixel >> shift) & 0xff <= alpha, "{pixel:#010x}");
                }
            }
        }
    }

    #[test]
    fn degenerate_and_hostile_input_draws_nothing() {
        let source = texture(4, 4, 1);
        let draw = |vertices: &[MeshVertex], indices: &[u32], opacity: f32| {
            let mut buffer = ground(12, 12);
            let before = buffer.clone();
            Painter::new(&mut buffer).draw_textured_mesh(
                TexturedMesh {
                    vertices: &floats(vertices),
                    indices,
                },
                MeshTexture::new(&source),
                MeshParams {
                    opacity,
                    glint_px: 0.0,
                },
            );
            assert_eq!(buffer.pixels, before.pixels);
        };
        let good = [
            vertex([1.0, 1.0], [0.0, 0.0]),
            vertex([9.0, 1.0], [4.0, 0.0]),
            vertex([9.0, 9.0], [4.0, 4.0]),
        ];
        draw(&good, &[0, 1, 2], 0.0);
        draw(&good, &[0, 1, 2], f32::NAN);
        draw(&good, &[0, 1, 7], 1.0); // an index past the buffer
        draw(&good, &[0, 1], 1.0); // not a whole triangle
        draw(&good, &[0, 0, 1], 1.0); // zero area
        for bad in [f32::NAN, f32::INFINITY, 2.0e6] {
            let mut vertices = good;
            vertices[1].pos[0] = bad;
            draw(&vertices, &[0, 1, 2], 1.0);
            let mut vertices = good;
            vertices[2].uv[1] = if bad.is_finite() { 1.0 } else { bad };
            if !bad.is_finite() {
                draw(&vertices, &[0, 1, 2], 1.0);
            }
        }
        let mut transparent = good;
        for v in &mut transparent {
            v.alpha = 0.0;
        }
        draw(&transparent, &[0, 1, 2], 1.0);

        // An empty texture, and a huge triangle that must still be clipped.
        let empty = WindowBuffer::new(0, 0);
        let mut buffer = ground(4, 4);
        let before = buffer.clone();
        Painter::new(&mut buffer).draw_textured_mesh(
            TexturedMesh {
                vertices: &floats(&good),
                indices: &[0, 1, 2],
            },
            MeshTexture::new(&empty),
            MeshParams::default(),
        );
        assert_eq!(buffer.pixels, before.pixels);
        let far = [
            vertex([-500_000.0, -500_000.0], [2.0, 2.0]),
            vertex([500_000.0, -500_000.0], [2.0, 2.0]),
            vertex([0.0, 500_000.0], [2.0, 2.0]),
        ];
        let mut opaque = WindowBuffer::new(4, 4);
        opaque.pixels.fill(0xff12_3456);
        let mut buffer = WindowBuffer::new(4, 4);
        Painter::new(&mut buffer).draw_textured_mesh(
            TexturedMesh {
                vertices: &floats(&far),
                indices: &[0, 1, 2],
            },
            MeshTexture::new(&opaque),
            MeshParams::default(),
        );
        assert!(buffer.pixels.iter().all(|&p| p == 0x0012_3456));
    }

    #[test]
    fn sampling_is_exact_on_centres_and_clamps_at_the_edge() {
        let source = texture(5, 4, 9);
        for y in 0..4i64 {
            for x in 0..5i64 {
                let at =
                    sample_bilinear_premultiplied(&source, (x << 16) + 32_768, (y << 16) + 32_768);
                assert_eq!(at, source.pixels[(y * 5 + x) as usize]);
            }
        }
        // Far outside is the nearest edge texel, not a wrap or a panic.
        assert_eq!(
            sample_bilinear_premultiplied(&source, -9 << 16, -9 << 16),
            source.pixels[0]
        );
        assert_eq!(
            sample_bilinear_premultiplied(&source, 99 << 16, 99 << 16),
            source.pixels[19]
        );
        // Half way between an opaque and a transparent texel: half of each
        // channel, colour still inside alpha (no dark or bright fringe).
        let mut pair = WindowBuffer::new(2, 1);
        pair.pixels = vec![0xffff_8000, 0];
        let middle = sample_bilinear_premultiplied(&pair, 1 << 16, 32_768);
        assert_eq!(middle, 0x8080_4000);
    }

    #[test]
    fn light_darkens_or_brightens_the_rim_only() {
        let mut source = WindowBuffer::new(8, 8);
        source.pixels.fill(0xff40_4040);
        let draw = |light: f32| {
            let (mut vertices, indices) = identity(&source, 1.0, 1.0);
            for v in vertices.chunks_exact_mut(VERTEX_FLOATS) {
                v[7] = light;
            }
            let mut buffer = WindowBuffer::new(10, 10);
            Painter::new_premultiplied(&mut buffer).draw_textured_mesh(
                TexturedMesh {
                    vertices: &vertices,
                    indices: &indices,
                },
                MeshTexture::new(&source),
                MeshParams {
                    opacity: 1.0,
                    glint_px: 2.0,
                },
            );
            buffer
        };
        let dark = draw(-0.5);
        assert_eq!(dark.pixels[5 * 10 + 5], 0xff20_2020);
        let bright = draw(1.0);
        assert_eq!(
            bright.pixels[5 * 10 + 5],
            0xff40_4040,
            "the middle is untouched"
        );
        let edge = bright.pixels[10 + 1];
        assert!(edge & 0xff > 0x40, "the rim is brighter: {edge:#010x}");
        assert_eq!(edge >> 24, 0xff);
    }

    #[test]
    fn a_smaller_level_is_used_where_the_mesh_asks_for_it() {
        // Base: a one-texel checkerboard. Level 1: its average, flat grey.
        let mut base = WindowBuffer::new(8, 8);
        for (i, pixel) in base.pixels.iter_mut().enumerate() {
            *pixel = if (i % 8 + i / 8) % 2 == 0 {
                0xffff_ffff
            } else {
                0xff00_0000
            };
        }
        let mut half = WindowBuffer::new(4, 4);
        half.pixels.fill(0xff80_8080);
        let mips = [half];
        let (mut vertices, indices) = identity(&base, 1.0, 1.0);
        let draw = |vertices: &[f32]| {
            let mut buffer = WindowBuffer::new(10, 10);
            Painter::new_premultiplied(&mut buffer).draw_textured_mesh(
                TexturedMesh {
                    vertices,
                    indices: &indices,
                },
                MeshTexture {
                    base: &base,
                    mips: &mips,
                },
                MeshParams::default(),
            );
            buffer
        };
        assert_eq!(draw(&vertices).pixels[2 * 10 + 2], 0xffff_ffff, "level 0");
        for v in vertices.chunks_exact_mut(VERTEX_FLOATS) {
            v[6] = 1.0;
        }
        assert_eq!(draw(&vertices).pixels[2 * 10 + 2], 0xff80_8080, "level 1");
        for v in vertices.chunks_exact_mut(VERTEX_FLOATS) {
            v[6] = 9.0;
        }
        assert_eq!(
            draw(&vertices).pixels[2 * 10 + 2],
            0xff80_8080,
            "clamped to the last level"
        );
    }
}
