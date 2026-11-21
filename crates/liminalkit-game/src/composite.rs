//! The CPU compositor: a [`GameFrame`] drawn in software. **EXPERIMENTAL.**
//!
//! This is the same frame a GPU host draws, drawn by the painter instead. It
//! is the path a host without a GPU uses, the reference a GPU host can be
//! compared against, and what the example runs on.
//!
//! Each layer takes the cheapest route that is still exact:
//!
//! * **Direct.** A flat layer on whole pixels at life size is a row-by-row
//!   composite of its buffer
//!   ([`Painter::draw_buffer`]): the cost of a blit, and byte for byte what
//!   the buffer holds.
//! * **Warped.** Anything else goes through its mesh
//!   ([`Painter::draw_textured_mesh`]).
//! * **Shaded first.** A layer with an effect active has
//!   [`crate::effect::shade`] applied to a scratch copy of its buffer, and
//!   that copy is what is then drawn. This is the one place the two renderers
//!   differ in order: the GPU shades after sampling, the CPU before. Every
//!   effect varies smoothly from texel to texel, so the difference is below
//!   a level of a channel except at a glow's outer edge under magnification.
//!
//! What this costs is per pixel and every frame for anything that moves: it
//! is a reference and a fallback, not a way to run a full-screen animated
//! interface at 4K. A pinned window on a turning planet is redrawn each
//! frame here, where on a GPU it is a few hundred vertices.

use liminalkit_raster::mesh::{MeshParams, MeshTexture, TexturedMesh};
use liminalkit_raster::{Painter, Rect, WindowBuffer};

use crate::effect::{scan_keep, shade_row, LayerUniforms};
use crate::frame::{GameFrame, Item};
use crate::layer::Layer;
use crate::sheet::Phase;

/// What kind of buffer is being composited into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// An opaque frame (`0x00RRGGBB`): the scene is already in it.
    Xrgb,
    /// A premultiplied overlay (`0xAARRGGBB`) to be composited over the
    /// scene later.
    Premultiplied,
}

/// Buffers the compositor reuses from frame to frame. Caller-owned so the
/// allocation is the caller's to see and to keep.
#[derive(Debug, Default)]
pub struct CompositeScratch {
    shaded: Option<WindowBuffer>,
}

/// What a composite did, for a frame-time readout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompositeStats {
    /// Layers drawn as a direct composite of their buffer.
    pub direct: u32,
    /// Layers drawn through their mesh.
    pub warped: u32,
    /// Layers whose effects were applied first.
    pub shaded: u32,
    /// Shadows drawn.
    pub shadows: u32,
    pub scrims: u32,
}

/// Draw `frame` into `dest`, inside `clip`.
pub fn composite(
    frame: &GameFrame<'_>,
    dest: &mut WindowBuffer,
    target: Target,
    clip: Rect,
    scratch: &mut CompositeScratch,
) -> CompositeStats {
    let mut stats = CompositeStats::default();
    let mut painter = match target {
        Target::Xrgb => Painter::new(dest),
        Target::Premultiplied => Painter::new_premultiplied(dest),
    };
    painter.push_clip(clip);
    for item in frame.items() {
        match *item {
            Item::Scrim {
                rect,
                color,
                opacity,
                ..
            } => {
                let alpha = (f32::from(color.alpha()) * opacity)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let rect = Rect::new(
                    rect.x.round() as i32,
                    rect.y.round() as i32,
                    rect.w.round() as i32,
                    rect.h.round() as i32,
                );
                painter.blend_rect(rect, color.0 & 0x00ff_ffff, alpha);
                stats.scrims += 1;
            }
            Item::Layer(layer) => draw_layer(&mut painter, layer, scratch, &mut stats),
        }
    }
    painter.pop_clip();
    stats
}

fn draw_layer(
    painter: &mut Painter<'_>,
    layer: &Layer,
    scratch: &mut CompositeScratch,
    stats: &mut CompositeStats,
) {
    let opacity = layer.draw_opacity();
    if let Some((mesh, texture, shadow)) = layer.sheet.shadow() {
        painter.draw_textured_mesh(
            TexturedMesh {
                vertices: mesh.vertex_floats(),
                indices: mesh.indices(),
            },
            MeshTexture::new(texture),
            MeshParams {
                opacity: shadow * opacity,
                glint_px: 0.0,
            },
        );
        stats.shadows += 1;
    }

    let resolved = layer.resolved();
    let sheet = &layer.sheet;
    let buffer = layer.surface.buffer();
    let origin = sheet.flat_origin();
    // A flat layer at life size on whole pixels needs no mesh: its buffer, or
    // the shaded copy of it, goes down row by row.
    let flat = resolved.scale == 1.0
        && sheet.phase() == Phase::Flat
        && sheet.flat_scale() == 1.0
        && origin[0].fract() == 0.0
        && origin[1].fract() == 0.0;
    let source = if resolved.identity {
        buffer
    } else {
        let shaded = scratch
            .shaded
            .get_or_insert_with(|| WindowBuffer::new(0, 0));
        shade_buffer(buffer, &resolved.uniforms, shaded);
        stats.shaded += 1;
        &*shaded
    };
    if flat {
        painter.draw_buffer(
            source,
            Rect::new(0, 0, source.width as i32, source.height as i32),
            origin[0] as i32,
            origin[1] as i32,
            (opacity * 255.0).round() as u8,
        );
        stats.direct += 1;
        return;
    }
    painter.draw_textured_mesh(
        TexturedMesh {
            vertices: sheet.mesh().vertex_floats(),
            indices: sheet.mesh().indices(),
        },
        MeshTexture {
            base: source,
            // The smaller levels are of the unshaded buffer.
            mips: if resolved.identity {
                layer.surface.mips()
            } else {
                &[]
            },
        },
        MeshParams {
            opacity,
            glint_px: layer.glint_px,
        },
    );
    stats.warped += 1;
}

/// A premultiplied pixel as floats, `0..=1`.
pub fn unpack(pixel: u32) -> [f32; 4] {
    let channel = |shift: u32| ((pixel >> shift) & 0xff) as f32 * (1.0 / 255.0);
    [channel(16), channel(8), channel(0), channel(24)]
}

/// Floats back to a premultiplied pixel, rounded to nearest, with no colour
/// channel allowed above alpha.
pub fn pack(pixel: [f32; 4]) -> u32 {
    // Clamped to be non-negative, so adding a half and truncating rounds; a
    // NaN clamps to itself and casts to zero.
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    let alpha = byte(pixel[3]);
    let colour = |value: f32| byte(value).min(alpha);
    (alpha << 24) | (colour(pixel[0]) << 16) | (colour(pixel[1]) << 8) | colour(pixel[2])
}

/// `source` with `uniforms`' effects applied to every texel, into `out`
/// (resized to match).
pub fn shade_buffer(source: &WindowBuffer, uniforms: &LayerUniforms, out: &mut WindowBuffer) {
    if (out.width, out.height) != (source.width, source.height) {
        out.resize(source.width, source.height);
    }
    // Only a glow draws where the layer is transparent.
    let fills = uniforms.glow_color[3] > 0.0;
    let (flashing, shimmering) = (uniforms.flash[3] > 0.0, uniforms.shimmer_color[3] > 0.0);
    let line = uniforms.shimmer_line;
    let width = source.width.max(1) as usize;
    for (y, (to_row, from_row)) in out
        .pixels
        .chunks_mut(width)
        .zip(source.pixels.chunks(width))
        .enumerate()
    {
        let row = y as f32 + 0.5;
        let keep = scan_keep(row, uniforms);
        let along_row = line[1] * row - line[2];
        for (x, (to, &from)) in to_row.iter_mut().zip(from_row).enumerate() {
            let column = x as f32 + 0.5;
            // Most texels of most effects are untouched: outside the
            // shimmer's band, with no flash, between scanlines' reach.
            let untouched = keep == 1.0
                && !flashing
                && (!fills || from >> 24 == 0xff)
                && (!shimmering || ((line[0] * column + along_row) / line[3]).abs() >= 1.0);
            *to = if untouched || (from == 0 && !fills) {
                from
            } else {
                pack(shade_row(unpack(from), [column, row], uniforms, keep))
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::Envelope;
    use crate::effect::{Flash, Glow};
    use crate::geom::{Argb, RectF};
    use crate::layer::{Band, LayerKey};
    use crate::surface::{PaintContext, Surface};
    use std::time::Duration;

    /// A layer painted with an opaque block inside a transparent border.
    fn layer(key: u64, band: Band, color: u32, at: [f32; 2]) -> Layer {
        let mut surface = Surface::new(24, 16);
        let mut text = liminalkit_text::TextCache::new(liminalkit_test_fonts::set());
        let mut kernel = liminalkit_raster::RasterKernel::new();
        surface.paint(
            PaintContext {
                text: &mut text,
                kernel: &mut kernel,
                theme: liminalkit_ui::Theme::default(),
                scale: liminalkit_shell::Scale(1.0),
                now: None,
                smooth_edges: false,
            },
            |ui, _| {
                let block = ui.painter.bounds().inset(4);
                ui.painter.fill_rect(block, color);
            },
        );
        let mut layer = Layer::new(LayerKey(key), band, surface);
        layer.sheet.set_flat_origin(at);
        layer.prepare(None, None);
        layer
    }

    fn ground() -> WindowBuffer {
        let mut buffer = WindowBuffer::new(64, 48);
        buffer.pixels.fill(0x0010_2030);
        buffer
    }

    fn draw(layers: &[&Layer], target: Target) -> (WindowBuffer, CompositeStats) {
        let mut frame = GameFrame::new([64, 48]);
        for layer in layers {
            frame.push(layer);
        }
        frame.finish();
        let mut dest = match target {
            Target::Xrgb => ground(),
            Target::Premultiplied => WindowBuffer::new(64, 48),
        };
        let stats = composite(
            &frame,
            &mut dest,
            target,
            Rect::new(0, 0, 64, 48),
            &mut CompositeScratch::default(),
        );
        (dest, stats)
    }

    #[test]
    fn a_resting_flat_layer_is_composited_directly_and_exactly() {
        let layer = layer(1, Band::Chrome, 0x00ff_8000, [10.0, 8.0]);
        let (dest, stats) = draw(&[&layer], Target::Xrgb);
        assert_eq!((stats.direct, stats.warped), (1, 0));
        assert_eq!(dest.pixels[(8 + 6) * 64 + 10 + 6], 0x00ff_8000);
        assert_eq!(
            dest.pixels[(8 + 1) * 64 + 10 + 1],
            0x0010_2030,
            "its clear border"
        );
        assert_eq!(dest.pixels[0], 0x0010_2030);
        // As an overlay: the block is opaque, the rest stays clear.
        let (overlay, _) = draw(&[&layer], Target::Premultiplied);
        assert_eq!(overlay.pixels[(8 + 6) * 64 + 10 + 6], 0xffff_8000);
        assert_eq!(overlay.pixels[0], 0);
    }

    #[test]
    fn the_mesh_route_draws_the_same_pixels_as_the_direct_one() {
        for opacity in [1.0, 0.5] {
            let mut direct = layer(1, Band::Chrome, 0x0040_c0ff, [10.0, 8.0]);
            direct.opacity = opacity;
            direct.prepare(None, None);
            // A scale a hair off one sends the same layer through its mesh.
            let mut meshed = layer(1, Band::Chrome, 0x0040_c0ff, [10.0, 8.0]);
            meshed.opacity = opacity;
            meshed.sheet.set_flat_scale(1.000_001);
            meshed.prepare(None, None);
            let (a, direct_stats) = draw(&[&direct], Target::Xrgb);
            let (b, meshed_stats) = draw(&[&meshed], Target::Xrgb);
            assert_eq!((direct_stats.direct, meshed_stats.warped), (1, 1));
            for (i, (x, y)) in a.pixels.iter().zip(&b.pixels).enumerate() {
                for shift in [16, 8, 0] {
                    let (x, y) = ((x >> shift) & 0xff, (y >> shift) & 0xff);
                    assert!(
                        x.abs_diff(y) <= 1,
                        "opacity {opacity}, pixel {i}: {x} vs {y}"
                    );
                }
            }
        }
    }

    #[test]
    fn bands_decide_what_is_on_top_whatever_the_push_order() {
        let over = layer(1, Band::Overlay, 0x00ff_0000, [10.0, 8.0]);
        let under = layer(2, Band::Chrome, 0x0000_00ff, [14.0, 10.0]);
        let (dest, _) = draw(&[&over, &under], Target::Xrgb);
        // Where both blocks overlap, the overlay's red wins.
        assert_eq!(dest.pixels[(8 + 9) * 64 + 10 + 12], 0x00ff_0000);
        // Where only the lower one is, its blue shows.
        assert_eq!(dest.pixels[(10 + 11) * 64 + 14 + 18], 0x0000_00ff);
    }

    #[test]
    fn a_flash_brightens_the_block_and_leaves_the_border_clear() {
        let mut flashing = layer(1, Band::Chrome, 0x0040_4040, [10.0, 8.0]);
        flashing.effects.flash = Some(Flash {
            color: Argb::WHITE,
            peak: 0.5,
            envelope: Envelope::flash(
                Duration::from_millis(100),
                Duration::from_millis(100),
                Duration::from_millis(100),
            ),
            fired: Duration::ZERO,
        });
        flashing.prepare(Some(Duration::from_millis(150)), None);
        let (dest, stats) = draw(&[&flashing], Target::Premultiplied);
        assert_eq!(
            (stats.shaded, stats.direct),
            (1, 1),
            "shaded, then laid down flat"
        );
        let block = dest.pixels[(8 + 8) * 64 + 10 + 12];
        assert_eq!(block >> 24, 0xff);
        // 0x40 plus half of full, give or take rounding.
        assert!(((block & 0xff) as i32 - 0xc0).abs() <= 1, "{block:#010x}");
        assert_eq!(
            dest.pixels[(8 + 1) * 64 + 10 + 1],
            0,
            "nothing to brighten there"
        );
    }

    #[test]
    fn a_glow_spills_into_the_margin_around_the_content() {
        let mut glowing = layer(1, Band::Chrome, 0x0020_2020, [10.0, 8.0]);
        glowing.surface.set_margin(4);
        glowing.effects.glow = Some(Glow {
            color: Argb::rgb(0x00ffff),
            intensity: 1.0,
            radius: 4.0,
            outline: 0.0,
            pulse: None,
        });
        glowing.prepare(None, None);
        let (dest, _) = draw(&[&glowing], Target::Premultiplied);
        let beside = dest.pixels[(8 + 8) * 64 + 10 + 3];
        assert!(
            beside >> 24 > 0x40,
            "a halo beside the block: {beside:#010x}"
        );
        assert_eq!(beside & 0x00ff_0000, 0, "cyan has no red");
        let far = dest.pixels[(8 + 8) * 64 + 10];
        assert!(far >> 24 < beside >> 24, "fading outward");
        assert_eq!(
            dest.pixels[(8 + 8) * 64 + 10 + 12],
            0xff20_2020,
            "the block itself"
        );
    }

    #[test]
    fn a_scrim_dims_what_is_below_it_and_the_clip_holds() {
        let panel = layer(1, Band::Chrome, 0x00ff_ffff, [10.0, 8.0]);
        let mut frame = GameFrame::new([64, 48]);
        frame.push(&panel);
        frame.scrim(
            Band::Modal,
            0,
            RectF::new(0.0, 0.0, 64.0, 48.0),
            Argb::BLACK,
            0.5,
        );
        frame.finish();
        let mut dest = ground();
        let stats = composite(
            &frame,
            &mut dest,
            Target::Xrgb,
            Rect::new(0, 0, 32, 48),
            &mut CompositeScratch::default(),
        );
        assert_eq!(stats.scrims, 1);
        let dimmed = dest.pixels[(8 + 8) * 64 + 10 + 8];
        assert!(
            (dimmed & 0xff) > 0x70 && (dimmed & 0xff) < 0x90,
            "{dimmed:#x}"
        );
        assert_eq!(dest.pixels[20 * 64 + 40], 0x0010_2030, "outside the clip");
    }

    #[test]
    fn packing_rounds_and_keeps_colour_inside_alpha() {
        assert_eq!(pack(unpack(0x8040_2010)), 0x8040_2010);
        assert_eq!(pack([1.0, 0.5, 0.0, 0.5]), 0x8080_8000);
        assert_eq!(pack([2.0, -1.0, f32::NAN, 1.0]) >> 24, 0xff);
    }
}
