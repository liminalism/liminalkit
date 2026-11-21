//! The mesh a layer is drawn through. **EXPERIMENTAL.**
//!
//! A [`WarpMesh`] is a grid over a layer's texture with each grid point moved
//! to where it belongs on screen. It is the whole of a layer's geometry for
//! both renderers: the CPU compositor hands it to
//! [`liminalkit_raster::Painter::draw_textured_mesh`], and a GPU host uploads
//! [`WarpMesh::vertex_floats`] and [`WarpMesh::indices`] as they are.
//!
//! The mesh also answers the question input needs: which texel is under this
//! pixel ([`WarpMesh::locate`]). It answers by inverting the triangles that
//! were drawn, so whatever shape a window is in — curved, half peeled,
//! rippling — what is clicked is what was seen, with no second copy of the
//! warp's mathematics to keep in step.

use liminalkit_raster::mesh::{MeshVertex, VERTEX_FLOATS};

use crate::geom::RectF;

/// Where one grid point of a layer lands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// Device pixels.
    pub pos: [f32; 2],
    /// Opacity there, `0..=1` (a horizon fade).
    pub alpha: f32,
    /// See [`MeshVertex::light`].
    pub light: f32,
}

/// How a mesh is finished once its points are placed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshTuning {
    /// A texel squeezed below `minify_fade[0]` pixels is invisible, above
    /// `minify_fade[1]` fully drawn, and fades between: text disappears
    /// before it turns to noise. `[0, 0]` never fades.
    pub minify_fade: [f32; 2],
    /// Mip levels below the full texture that exist to be sampled.
    pub mip_levels: u8,
}

impl MeshTuning {
    /// For a texture without mips.
    pub const PLAIN: MeshTuning = MeshTuning {
        minify_fade: [0.2, 0.5],
        mip_levels: 0,
    };

    /// For a texture with `levels` mips: it can be squeezed further before it
    /// has to fade.
    pub const fn with_mips(levels: u8) -> MeshTuning {
        MeshTuning {
            minify_fade: [0.08, 0.2],
            mip_levels: levels,
        }
    }

    /// No fade at all: for a shadow, which is already a blur.
    pub const SOFT: MeshTuning = MeshTuning {
        minify_fade: [0.0, 0.0],
        mip_levels: 0,
    };
}

/// `0` below `low`, `1` above `high`, a smooth ramp between.
pub fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    if high <= low {
        return if x >= high { 1.0 } else { 0.0 };
    }
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A screen-space triangle mesh over a texture. Owned by its layer and
/// rebuilt in place, so a steady frame allocates nothing.
#[derive(Debug, Clone, Default)]
pub struct WarpMesh {
    vertices: Vec<f32>,
    indices: Vec<u32>,
    bounds: Option<RectF>,
    /// Scratch for the grid being built; kept for its allocation.
    grid: Vec<Option<Placed>>,
}

impl WarpMesh {
    pub fn new() -> WarpMesh {
        WarpMesh::default()
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.bounds = None;
    }

    /// The vertex buffer: [`VERTEX_FLOATS`] floats per vertex, in
    /// [`MeshVertex`] order.
    pub fn vertex_floats(&self) -> &[f32] {
        &self.vertices
    }

    /// The index buffer: a triangle list.
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / VERTEX_FLOATS
    }

    pub fn vertex(&self, index: usize) -> MeshVertex {
        MeshVertex::from_floats(&self.vertices[index * VERTEX_FLOATS..][..VERTEX_FLOATS])
    }

    /// Nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// The pixels the mesh can touch: its damage rectangle on a CPU frame.
    pub fn bounds(&self) -> Option<RectF> {
        self.bounds
    }

    /// The vertex buffer as little-endian bytes, appended to `out`: what a
    /// GPU buffer write takes. (Done by hand: the workspace forbids the
    /// `unsafe` a cast would need.)
    pub fn write_vertex_bytes(&self, out: &mut Vec<u8>) {
        out.reserve(self.vertices.len() * 4);
        for value in &self.vertices {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }

    /// The index buffer as little-endian `u32` bytes, appended to `out`.
    pub fn write_index_bytes(&self, out: &mut Vec<u8>) {
        out.reserve(self.indices.len() * 4);
        for value in &self.indices {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }

    /// Rebuild as a `cols × rows` grid over the texel rectangle `uv`, asking
    /// `place` where each grid point goes (`None`: nowhere, which drops the
    /// cells around it).
    ///
    /// Each vertex's `px_per_texel`, minification fade and `lod` are measured
    /// from the grid itself, by how far its neighbours landed. Cells that are
    /// invisible, or that came out mirrored — a surface folding back on
    /// itself at a horizon — are left out of the index buffer.
    pub fn build(
        &mut self,
        uv: RectF,
        cols: u16,
        rows: u16,
        tuning: &MeshTuning,
        mut place: impl FnMut([f32; 2]) -> Option<Placed>,
    ) {
        self.clear();
        let (cols, rows) = (usize::from(cols.max(1)), usize::from(rows.max(1)));
        if uv.is_empty() {
            return;
        }
        let across = cols + 1;
        let texel = |i: usize, j: usize| {
            [
                uv.x + uv.w * i as f32 / cols as f32,
                uv.y + uv.h * j as f32 / rows as f32,
            ]
        };
        self.grid.clear();
        for j in 0..=rows {
            for i in 0..=cols {
                let placed =
                    place(texel(i, j)).filter(|p| p.pos[0].is_finite() && p.pos[1].is_finite());
                self.grid.push(placed);
            }
        }

        let (step_u, step_v) = (uv.w / cols as f32, uv.h / rows as f32);
        for j in 0..=rows {
            for i in 0..=cols {
                let Some(placed) = self.grid[j * across + i] else {
                    // A placeholder keeps indices regular; no triangle uses it.
                    self.vertices.extend([0.0; VERTEX_FLOATS]);
                    continue;
                };
                // Screen movement per texel along each texture axis, from
                // whichever neighbours exist.
                let slope = |before: Option<usize>, after: Option<usize>, step: f32| {
                    let at = |index: Option<usize>| index.and_then(|n| self.grid[n]);
                    let (a, b, span) = match (at(before), at(after)) {
                        (Some(a), Some(b)) => (a, b, 2.0 * step),
                        (Some(a), None) => (a, placed, step),
                        (None, Some(b)) => (placed, b, step),
                        (None, None) => return None,
                    };
                    Some([(b.pos[0] - a.pos[0]) / span, (b.pos[1] - a.pos[1]) / span])
                };
                let here = j * across + i;
                let du = slope(
                    (i > 0).then(|| here - 1),
                    (i < cols).then(|| here + 1),
                    step_u,
                );
                let dv = slope(
                    (j > 0).then(|| here - across),
                    (j < rows).then(|| here + across),
                    step_v,
                );
                let squeeze = match (du, dv) {
                    (Some(du), Some(dv)) => least_stretch(du, dv),
                    _ => 1.0,
                };
                let fade = if tuning.minify_fade[1] > 0.0 {
                    smoothstep(tuning.minify_fade[0], tuning.minify_fade[1], squeeze)
                } else {
                    1.0
                };
                let lod = if squeeze > 0.0 {
                    (-squeeze.log2()).clamp(0.0, f32::from(tuning.mip_levels))
                } else {
                    f32::from(tuning.mip_levels)
                };
                let [u, v] = texel(i, j);
                self.vertices.extend(
                    MeshVertex {
                        pos: placed.pos,
                        uv: [u, v],
                        alpha: (placed.alpha * fade).clamp(0.0, 1.0),
                        px_per_texel: squeeze.clamp(0.0, 64.0),
                        lod,
                        light: placed.light.clamp(-1.0, 1.0),
                    }
                    .to_floats(),
                );
            }
        }

        let (mut low, mut high) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for j in 0..rows {
            for i in 0..cols {
                let corners = [
                    j * across + i,
                    j * across + i + 1,
                    (j + 1) * across + i + 1,
                    (j + 1) * across + i,
                ];
                for triangle in [[0, 1, 2], [0, 2, 3]] {
                    let index = triangle.map(|c| corners[c]);
                    let (Some(a), Some(b), Some(c)) = (
                        self.grid[index[0]],
                        self.grid[index[1]],
                        self.grid[index[2]],
                    ) else {
                        continue;
                    };
                    let alpha = |n: usize| self.vertices[n * VERTEX_FLOATS + 4];
                    if index.iter().all(|&n| alpha(n) <= 0.0) {
                        continue;
                    }
                    // Texels run right and down; so must the triangle.
                    let area = (b.pos[0] - a.pos[0]) * (c.pos[1] - a.pos[1])
                        - (b.pos[1] - a.pos[1]) * (c.pos[0] - a.pos[0]);
                    if area.is_nan() || area <= 0.0 {
                        continue;
                    }
                    for p in [a, b, c] {
                        for axis in 0..2 {
                            low[axis] = low[axis].min(p.pos[axis]);
                            high[axis] = high[axis].max(p.pos[axis]);
                        }
                    }
                    self.indices.extend(index.map(|n| n as u32));
                }
            }
        }
        if !self.indices.is_empty() {
            self.bounds = Some(RectF::new(
                low[0],
                low[1],
                high[0] - low[0],
                high[1] - low[1],
            ));
        }
    }

    /// Rebuild as one quad showing texels `uv` at `dest`: a flat layer.
    pub fn quad(&mut self, dest: RectF, uv: RectF) {
        let scale = [dest.w / uv.w, dest.h / uv.h];
        self.build(uv, 1, 1, &MeshTuning::SOFT, |t| {
            Some(Placed {
                pos: [
                    dest.x + (t[0] - uv.x) * scale[0],
                    dest.y + (t[1] - uv.y) * scale[1],
                ],
                alpha: 1.0,
                light: 0.0,
            })
        });
    }

    /// Scale every position about `pivot`: a pop.
    pub fn scale_about(&mut self, pivot: [f32; 2], scale: f32) {
        for vertex in self.vertices.chunks_exact_mut(VERTEX_FLOATS) {
            vertex[0] = pivot[0] + (vertex[0] - pivot[0]) * scale;
            vertex[1] = pivot[1] + (vertex[1] - pivot[1]) * scale;
        }
        if let Some(b) = self.bounds {
            self.bounds = Some(RectF::new(
                pivot[0] + (b.x - pivot[0]) * scale,
                pivot[1] + (b.y - pivot[1]) * scale,
                b.w * scale,
                b.h * scale,
            ));
        }
    }

    /// Where a texel is drawn: [`WarpMesh::locate`] the other way round.
    /// `None` if no triangle shows it (it is off the texture's edge, or that
    /// part of the layer is not drawn). For hanging something on a point of
    /// a warped layer — a tooltip on a button of a window that lies on a
    /// planet.
    pub fn screen_of(&self, texel: [f32; 2]) -> Option<[f32; 2]> {
        for triangle in self.indices.chunks_exact(3).rev() {
            let [a, b, c] =
                [triangle[0], triangle[1], triangle[2]].map(|n| self.vertex(n as usize));
            let area = (b.uv[0] - a.uv[0]) * (c.uv[1] - a.uv[1])
                - (b.uv[1] - a.uv[1]) * (c.uv[0] - a.uv[0]);
            if area == 0.0 {
                continue;
            }
            let wb = ((texel[0] - a.uv[0]) * (c.uv[1] - a.uv[1])
                - (texel[1] - a.uv[1]) * (c.uv[0] - a.uv[0]))
                / area;
            let wc = ((b.uv[0] - a.uv[0]) * (texel[1] - a.uv[1])
                - (b.uv[1] - a.uv[1]) * (texel[0] - a.uv[0]))
                / area;
            let wa = 1.0 - wb - wc;
            // A hair of slack: a texel on a shared edge belongs to both.
            if wa >= -1e-4 && wb >= -1e-4 && wc >= -1e-4 {
                return Some([
                    wa * a.pos[0] + wb * b.pos[0] + wc * c.pos[0],
                    wa * a.pos[1] + wb * b.pos[1] + wc * c.pos[1],
                ]);
            }
        }
        None
    }

    /// The texel under a screen position and the mesh's opacity there, from
    /// the topmost triangle that covers it (the last drawn). `None` off the
    /// mesh.
    pub fn locate(&self, screen: [f32; 2]) -> Option<([f32; 2], f32)> {
        let bounds = self.bounds?;
        if !bounds.inflate(1.0).contains(screen) {
            return None;
        }
        let (px, py) = (f64::from(screen[0]), f64::from(screen[1]));
        for triangle in self.indices.chunks_exact(3).rev() {
            let [a, b, c] =
                [triangle[0], triangle[1], triangle[2]].map(|n| self.vertex(n as usize));
            let at = |v: &MeshVertex| (f64::from(v.pos[0]), f64::from(v.pos[1]));
            let (pa, pb, pc) = (at(&a), at(&b), at(&c));
            let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pb.1 - pa.1) * (pc.0 - pa.0);
            if area <= 0.0 {
                continue;
            }
            let wb = ((px - pa.0) * (pc.1 - pa.1) - (py - pa.1) * (pc.0 - pa.0)) / area;
            let wc = ((pb.0 - pa.0) * (py - pa.1) - (pb.1 - pa.1) * (px - pa.0)) / area;
            let wa = 1.0 - wb - wc;
            if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                continue;
            }
            let mix = |get: fn(&MeshVertex) -> f32| {
                (wa * f64::from(get(&a)) + wb * f64::from(get(&b)) + wc * f64::from(get(&c))) as f32
            };
            return Some(([mix(|v| v.uv[0]), mix(|v| v.uv[1])], mix(|v| v.alpha)));
        }
        None
    }
}

/// The smaller singular value of the 2×2 map with columns `du` and `dv`: how
/// many pixels a texel covers along the direction it is squeezed most.
fn least_stretch(du: [f32; 2], dv: [f32; 2]) -> f32 {
    let (a, b, c, d) = (
        f64::from(du[0]),
        f64::from(dv[0]),
        f64::from(du[1]),
        f64::from(dv[1]),
    );
    let sum = a * a + b * b + c * c + d * d;
    let det = a * d - b * c;
    let greater = ((sum + (sum * sum - 4.0 * det * det).max(0.0).sqrt()) / 2.0).sqrt();
    if greater > 0.0 {
        (det.abs() / greater) as f32
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(mesh: &mut WarpMesh, scale: f32) {
        mesh.build(
            RectF::new(-1.0, -1.0, 42.0, 22.0),
            4,
            3,
            &MeshTuning::PLAIN,
            |t| {
                Some(Placed {
                    pos: [10.0 + t[0] * scale, 20.0 + t[1] * scale],
                    alpha: 1.0,
                    light: 0.0,
                })
            },
        );
    }

    #[test]
    fn a_flat_grid_measures_its_own_scale() {
        let mut mesh = WarpMesh::new();
        flat(&mut mesh, 1.0);
        assert_eq!(mesh.vertex_count(), 5 * 4);
        assert_eq!(mesh.indices().len(), 4 * 3 * 6);
        for index in 0..mesh.vertex_count() {
            let v = mesh.vertex(index);
            assert!((v.px_per_texel - 1.0).abs() < 1e-5);
            assert_eq!((v.alpha, v.lod), (1.0, 0.0));
        }
        assert_eq!(mesh.bounds(), Some(RectF::new(9.0, 19.0, 42.0, 22.0)));
        flat(&mut mesh, 2.0);
        assert!((mesh.vertex(7).px_per_texel - 2.0).abs() < 1e-5);
    }

    #[test]
    fn squeezed_texels_fade_out_and_ask_for_a_smaller_level() {
        let squeezed = |tuning: MeshTuning, squeeze: f32| {
            let mut mesh = WarpMesh::new();
            mesh.build(RectF::new(0.0, 0.0, 64.0, 64.0), 2, 2, &tuning, |t| {
                Some(Placed {
                    pos: [t[0], t[1] * squeeze],
                    alpha: 1.0,
                    light: 0.0,
                })
            });
            mesh.vertex(4)
        };
        let v = squeezed(MeshTuning::PLAIN, 0.25);
        assert!((v.px_per_texel - 0.25).abs() < 1e-5);
        assert!(v.alpha > 0.0 && v.alpha < 0.2, "{}", v.alpha);
        assert_eq!(v.lod, 0.0, "no mips to ask for");
        assert_eq!(squeezed(MeshTuning::PLAIN, 0.1).alpha, 0.0);
        let mipped = squeezed(MeshTuning::with_mips(3), 0.25);
        assert_eq!(mipped.alpha, 1.0, "mips keep it legible");
        assert!((mipped.lod - 2.0).abs() < 1e-5);
        assert_eq!(squeezed(MeshTuning::with_mips(1), 0.01).lod, 1.0, "clamped");
    }

    #[test]
    fn a_missing_point_drops_only_the_cells_that_touch_it() {
        let mut mesh = WarpMesh::new();
        mesh.build(
            RectF::new(0.0, 0.0, 30.0, 30.0),
            3,
            3,
            &MeshTuning::SOFT,
            |t| {
                (t != [0.0, 0.0]).then_some(Placed {
                    pos: t,
                    alpha: 1.0,
                    light: 0.0,
                })
            },
        );
        assert_eq!(mesh.indices().len(), (9 - 1) * 6);
        assert!(mesh.locate([2.0, 2.0]).is_none(), "the dropped cell");
        assert!(mesh.locate([15.0, 15.0]).is_some());
    }

    #[test]
    fn folded_and_invisible_cells_are_left_out() {
        // Mirrored left to right: every cell is back to front.
        let mut mesh = WarpMesh::new();
        mesh.build(
            RectF::new(0.0, 0.0, 30.0, 30.0),
            2,
            2,
            &MeshTuning::SOFT,
            |t| {
                Some(Placed {
                    pos: [-t[0], t[1]],
                    alpha: 1.0,
                    light: 0.0,
                })
            },
        );
        assert!(mesh.is_empty() && mesh.bounds().is_none());
        // Transparent on the left half.
        mesh.build(
            RectF::new(0.0, 0.0, 30.0, 30.0),
            3,
            1,
            &MeshTuning::SOFT,
            |t| {
                Some(Placed {
                    pos: t,
                    alpha: if t[0] < 15.0 { 0.0 } else { 1.0 },
                    light: 0.0,
                })
            },
        );
        assert_eq!(
            mesh.indices().len(),
            2 * 6,
            "the all-transparent cell is gone"
        );
        // A point at infinity is a missing point, not a crash.
        mesh.build(
            RectF::new(0.0, 0.0, 30.0, 30.0),
            1,
            1,
            &MeshTuning::SOFT,
            |t| {
                Some(Placed {
                    pos: [t[0] * f32::INFINITY, t[1]],
                    alpha: 1.0,
                    light: 0.0,
                })
            },
        );
        assert!(mesh.is_empty());
        mesh.build(
            RectF::new(0.0, 0.0, 0.0, 30.0),
            1,
            1,
            &MeshTuning::SOFT,
            |_| None,
        );
        assert!(mesh.is_empty());
    }

    #[test]
    fn locating_inverts_a_warp_through_the_triangles_drawn() {
        // A bulge: not affine, so the answer is only as good as the grid.
        let warp = |t: [f32; 2]| {
            let (u, v) = (t[0] / 100.0 - 0.5, t[1] / 100.0 - 0.5);
            [
                300.0 + t[0] * 1.5 + 30.0 * v * v,
                200.0 + t[1] * 1.2 + 20.0 * u * u,
            ]
        };
        let mut mesh = WarpMesh::new();
        mesh.build(
            RectF::new(0.0, 0.0, 100.0, 100.0),
            24,
            24,
            &MeshTuning::SOFT,
            |t| {
                Some(Placed {
                    pos: warp(t),
                    alpha: 0.5 + t[0] / 200.0,
                    light: 0.0,
                })
            },
        );
        for (u, v) in [(10.0, 10.0), (50.0, 50.0), (87.5, 12.25), (33.0, 91.0)] {
            let there = mesh.screen_of([u, v]).expect("drawn");
            let exact = warp([u, v]);
            assert!((there[0] - exact[0]).abs() < 0.1 && (there[1] - exact[1]).abs() < 0.1);
            let (found, alpha) = mesh.locate(warp([u, v])).expect("on the mesh");
            assert!(
                (found[0] - u).abs() < 0.1 && (found[1] - v).abs() < 0.1,
                "{found:?}"
            );
            assert!((alpha - (0.5 + u / 200.0)).abs() < 0.01);
        }
        assert!(mesh.locate([0.0, 0.0]).is_none());
        assert!(mesh.screen_of([150.0, 50.0]).is_none(), "off the texture");
    }

    #[test]
    fn a_quad_and_a_pop_place_texels_where_they_say() {
        let mut mesh = WarpMesh::new();
        mesh.quad(
            RectF::new(100.0, 50.0, 80.0, 40.0),
            RectF::new(0.0, 0.0, 40.0, 20.0),
        );
        assert_eq!(mesh.indices().len(), 6);
        let (texel, _) = mesh.locate([140.0, 70.0]).unwrap();
        assert!((texel[0] - 20.0).abs() < 1e-4 && (texel[1] - 10.0).abs() < 1e-4);
        mesh.scale_about([140.0, 70.0], 0.5);
        assert_eq!(mesh.bounds(), Some(RectF::new(120.0, 60.0, 40.0, 20.0)));
        let (texel, _) = mesh.locate([121.0, 61.0]).unwrap();
        assert!((texel[0] - 1.0).abs() < 1e-3, "{texel:?}");
    }

    #[test]
    fn buffers_pack_as_little_endian_words() {
        let mut mesh = WarpMesh::new();
        mesh.quad(
            RectF::new(1.0, 2.0, 4.0, 4.0),
            RectF::new(0.0, 0.0, 4.0, 4.0),
        );
        let (mut vertices, mut indices) = (Vec::new(), Vec::new());
        mesh.write_vertex_bytes(&mut vertices);
        mesh.write_index_bytes(&mut indices);
        assert_eq!(vertices.len(), 4 * VERTEX_FLOATS * 4);
        assert_eq!(&vertices[..4], &1.0f32.to_le_bytes());
        assert_eq!(&vertices[4..8], &2.0f32.to_le_bytes());
        assert_eq!(indices.len(), 6 * 4);
        assert_eq!(&indices[4..8], &1u32.to_le_bytes());
    }

    #[test]
    fn smoothstep_is_flat_outside_its_ramp() {
        assert_eq!(smoothstep(0.2, 0.5, 0.1), 0.0);
        assert_eq!(smoothstep(0.2, 0.5, 0.9), 1.0);
        assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(
            smoothstep(0.3, 0.3, 0.3),
            1.0,
            "a step when the ramp is empty"
        );
    }
}
