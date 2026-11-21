//! One frame, described once. **EXPERIMENTAL.**
//!
//! A [`GameFrame`] is a list of what to draw this frame, in order: borrowed
//! layers and the odd solid rectangle. It owns nothing but that list, lives
//! for one frame, and is the single description both renderers work from:
//!
//! * [`crate::composite::composite`] draws it in software into a buffer.
//! * A GPU host walks [`GameFrame::gpu`], which yields each draw as a texture
//!   to make current, a mesh, and a uniform block.
//!
//! The GPU contract, in full. Per [`GpuDraw::Textured`]:
//!
//! 1. Find or create the host's texture for `texture.key`
//!    (`Bgra8Unorm`, the size and mip count `texture` reports).
//! 2. Ask `texture.damage_since(revision last uploaded)` and upload that
//!    rectangle of `texture.pixels()` (and the mips) if it is not `None`.
//! 3. Write `mesh.vertex_floats()`, `mesh.indices()` and
//!    `uniforms.to_bytes()`.
//! 4. Draw with [`crate::gpu::layer_wgsl`], premultiplied blending
//!    (`ONE, ONE_MINUS_SRC_ALPHA`), no depth, no culling.
//!
//! Draws are in order; nothing else is needed for the result to match the
//! CPU compositor's.

use liminalkit_raster::WindowBuffer;

use crate::effect::LayerUniforms;
use crate::geom::{Argb, RectF};
use crate::layer::{Band, Layer, LayerKey};
use crate::surface::{Damage, Surface};
use crate::warp::WarpMesh;

/// One thing to draw.
#[derive(Debug, Clone, Copy)]
pub enum Item<'a> {
    Layer(&'a Layer),
    /// A translucent rectangle: the dimming behind a dialog.
    Scrim {
        band: Band,
        z: i32,
        rect: RectF,
        color: Argb,
        opacity: f32,
    },
}

impl Item<'_> {
    fn order(&self) -> (Band, i32) {
        match self {
            Item::Layer(layer) => (layer.band, layer.z),
            Item::Scrim { band, z, .. } => (*band, *z),
        }
    }
}

/// See the [module documentation](self).
#[derive(Debug)]
pub struct GameFrame<'a> {
    viewport: [u32; 2],
    items: Vec<Item<'a>>,
}

impl<'a> GameFrame<'a> {
    /// An empty frame for a target of `viewport` pixels.
    pub fn new(viewport: [u32; 2]) -> GameFrame<'a> {
        GameFrame {
            viewport,
            items: Vec::new(),
        }
    }

    /// As [`GameFrame::new`], with room for `items` so that pushing them does
    /// not allocate again.
    pub fn with_capacity(viewport: [u32; 2], items: usize) -> GameFrame<'a> {
        GameFrame {
            viewport,
            items: Vec::with_capacity(items),
        }
    }

    pub fn viewport(&self) -> [u32; 2] {
        self.viewport
    }

    /// Add a prepared layer. One that draws nothing is left out.
    pub fn push(&mut self, layer: &'a Layer) {
        if layer.is_visible() {
            self.items.push(Item::Layer(layer));
        }
    }

    /// Add a translucent rectangle at `(band, z)`.
    pub fn scrim(&mut self, band: Band, z: i32, rect: RectF, color: Argb, opacity: f32) {
        if opacity > 0.0 && !rect.is_empty() {
            self.items.push(Item::Scrim {
                band,
                z,
                rect,
                color,
                opacity: opacity.min(1.0),
            });
        }
    }

    /// Put the items in drawing order: by band, then `z`, then as pushed.
    pub fn finish(&mut self) {
        self.items.sort_by_key(Item::order);
    }

    /// The items, in the order they stand ([`GameFrame::finish`] sorts them).
    pub fn items(&self) -> &[Item<'a>] {
        &self.items
    }

    /// The frame as GPU draws, in order. A layer with a shadow yields the
    /// shadow first.
    pub fn gpu(&self) -> impl Iterator<Item = GpuDraw<'a>> + '_ {
        let viewport = self.viewport;
        self.items.iter().flat_map(move |item| {
            let (first, second) = match *item {
                Item::Scrim {
                    band,
                    rect,
                    color,
                    opacity,
                    ..
                } => {
                    let [r, g, b, a] = color.premultiplied();
                    let color = [r * opacity, g * opacity, b * opacity, a * opacity];
                    (Some(GpuDraw::Solid { band, rect, color }), None)
                }
                Item::Layer(layer) => {
                    let shadow =
                        layer
                            .sheet
                            .shadow()
                            .map(|(mesh, buffer, opacity)| GpuDraw::Textured {
                                band: layer.band,
                                texture: TextureRef {
                                    key: TextureKey::Shadow(layer.key),
                                    buffer,
                                    mips: &[],
                                    revision: layer.sheet.shadow_revision(),
                                    surface: None,
                                },
                                mesh,
                                uniforms: LayerUniforms {
                                    size: [0.0, 0.0, buffer.width as f32, buffer.height as f32],
                                    ..LayerUniforms::IDENTITY
                                }
                                .for_draw(
                                    viewport,
                                    opacity * layer.draw_opacity(),
                                    0.0,
                                    0,
                                ),
                            });
                    let content = GpuDraw::Textured {
                        band: layer.band,
                        texture: TextureRef {
                            key: TextureKey::Content(layer.key),
                            buffer: layer.surface.buffer(),
                            mips: layer.surface.mips(),
                            revision: layer.surface.revision(),
                            surface: Some(&layer.surface),
                        },
                        mesh: layer.sheet.mesh(),
                        uniforms: layer.uniforms(viewport),
                    };
                    (shadow, Some(content))
                }
            };
            first.into_iter().chain(second)
        })
    }
}

/// Which of a layer's textures a draw uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureKey {
    /// What the layer painted.
    Content(LayerKey),
    /// The soft shadow under it: a small blur, the same until the layer
    /// changes size.
    Shadow(LayerKey),
}

/// A texture a draw needs current, and what changed in it.
#[derive(Debug, Clone, Copy)]
pub struct TextureRef<'a> {
    pub key: TextureKey,
    buffer: &'a WindowBuffer,
    mips: &'a [WindowBuffer],
    revision: u64,
    surface: Option<&'a Surface>,
}

impl<'a> TextureRef<'a> {
    pub fn size(&self) -> [u32; 2] {
        [self.buffer.width, self.buffer.height]
    }

    /// Premultiplied `0xAARRGGBB`, sRGB-encoded, row after row with no
    /// padding: the bytes of a `Bgra8Unorm` texture on a little-endian
    /// machine.
    pub fn pixels(&self) -> &'a [u32] {
        &self.buffer.pixels
    }

    /// The smaller mip levels, largest first. Empty for most layers.
    pub fn mips(&self) -> &'a [WindowBuffer] {
        self.mips
    }

    /// Changes exactly when the pixels do.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// What to upload for a texture last uploaded at `uploaded` (`None`:
    /// never).
    pub fn damage_since(&self, uploaded: Option<u64>) -> Damage {
        match (self.surface, uploaded) {
            (Some(surface), _) => surface.damage_since(uploaded),
            (None, Some(at)) if at == self.revision => Damage::None,
            (None, _) => Damage::Full,
        }
    }
}

/// One GPU draw.
#[derive(Debug, Clone, Copy)]
pub enum GpuDraw<'a> {
    /// A texture through a mesh, with the layer shader.
    Textured {
        band: Band,
        texture: TextureRef<'a>,
        mesh: &'a WarpMesh,
        uniforms: LayerUniforms,
    },
    /// A rectangle of one premultiplied colour.
    Solid {
        band: Band,
        rect: RectF,
        color: [f32; 4],
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::LayerKey;
    use crate::surface::PaintContext;

    fn painted(key: u64, band: Band, z: i32) -> Layer {
        let mut surface = Surface::new(20, 10);
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
                let all = ui.painter.bounds();
                ui.painter.fill_rect(all, 0x00ff_ffff);
            },
        );
        let mut layer = Layer::new(LayerKey(key), band, surface);
        layer.z = z;
        layer.prepare(None, None);
        layer
    }

    #[test]
    fn items_are_drawn_by_band_then_z_then_as_pushed() {
        let layers = [
            painted(1, Band::Overlay, 0),
            painted(2, Band::Chrome, 5),
            painted(3, Band::Chrome, -1),
            painted(4, Band::Chrome, 5),
        ];
        let mut frame = GameFrame::with_capacity([640, 480], 8);
        for layer in &layers {
            frame.push(layer);
        }
        frame.scrim(
            Band::Modal,
            0,
            RectF::new(0.0, 0.0, 640.0, 480.0),
            Argb::BLACK,
            0.5,
        );
        frame.scrim(
            Band::Modal,
            0,
            RectF::new(0.0, 0.0, 640.0, 480.0),
            Argb::BLACK,
            0.0,
        );
        frame.finish();
        let keys: Vec<u64> = frame
            .items()
            .iter()
            .map(|item| match item {
                Item::Layer(layer) => layer.key.0,
                Item::Scrim { .. } => 0,
            })
            .collect();
        assert_eq!(keys, [3, 2, 4, 1, 0]);
    }

    #[test]
    fn a_hidden_layer_is_left_out() {
        let mut hidden = painted(1, Band::Chrome, 0);
        hidden.hidden = true;
        let mut clear = painted(2, Band::Chrome, 0);
        clear.opacity = 0.0;
        let mut frame = GameFrame::new([640, 480]);
        frame.push(&hidden);
        frame.push(&clear);
        assert!(frame.items().is_empty());
    }

    #[test]
    fn the_gpu_walk_names_textures_and_says_what_to_upload() {
        let mut layer = painted(9, Band::Chrome, 0);
        layer.opacity = 0.75;
        layer.sheet.tuning.shadow = Some(crate::sheet::ShadowStyle::default());
        layer.prepare(None, None);
        let mut frame = GameFrame::new([640, 480]);
        frame.push(&layer);
        frame.scrim(
            Band::Modal,
            0,
            RectF::new(0.0, 0.0, 10.0, 10.0),
            Argb::rgb(0xff0000),
            0.5,
        );
        frame.finish();
        let draws: Vec<_> = frame.gpu().collect();
        // A flat sheet with a shadow style casts it at its flat opacity.
        assert_eq!(draws.len(), 3);
        let GpuDraw::Textured {
            texture, uniforms, ..
        } = draws[0]
        else {
            panic!("the shadow first");
        };
        assert_eq!(texture.key, TextureKey::Shadow(LayerKey(9)));
        assert_eq!(texture.damage_since(None), Damage::Full);
        assert_eq!(texture.damage_since(Some(texture.revision())), Damage::None);
        assert!((uniforms.params[0] - 0.45 * 0.75).abs() < 1e-6);
        let GpuDraw::Textured {
            texture,
            mesh,
            uniforms,
            ..
        } = draws[1]
        else {
            panic!("then the content");
        };
        assert_eq!(texture.key, TextureKey::Content(LayerKey(9)));
        assert_eq!((texture.size(), texture.pixels().len()), ([20, 10], 200));
        assert_eq!(texture.damage_since(Some(texture.revision())), Damage::None);
        assert_eq!(mesh.indices().len(), 6);
        assert_eq!(uniforms.size, [640.0, 480.0, 20.0, 10.0]);
        assert_eq!(uniforms.params[0], 0.75);
        let GpuDraw::Solid { color, .. } = draws[2] else {
            panic!("the scrim last");
        };
        assert_eq!(color, [0.5, 0.0, 0.0, 0.5]);
    }
}
