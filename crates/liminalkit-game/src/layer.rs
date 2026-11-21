//! A layer: pixels, where they go, and how they look. **EXPERIMENTAL.**
//!
//! A [`Layer`] ties together the three things this crate keeps apart: a
//! [`Surface`] (what was painted), a [`Sheet`] (where it is placed: flat, on
//! a surface, or in between) and [`Effects`] (what is done to it on the way
//! to the screen). The application owns its layers, in whatever structure
//! suits it, and names each with a [`LayerKey`] of its own choosing; that
//! key is the only identity anything here ever stores.
//!
//! Layers are ordered by [`Band`] and then `z`. Immediate mode has no
//! z-order of its own, and the toolkit's rule for anything that floats — it
//! claims its input before what is beneath it and is drawn after — becomes,
//! with layers, simply the order they are walked in: the
//! [`crate::input::Router`] walks it from the top, the frame from the bottom.

use std::time::Duration;

use liminalkit_raster::Rect;

use crate::effect::{Effects, LayerUniforms, Resolved, Shape};
use crate::geom::RectF;
use crate::sheet::{MeshOptions, Scene, Sheet};
use crate::surface::{HitTest, Surface};
use crate::warp::MeshTuning;

/// The application's name for a layer: stable from frame to frame, and what a
/// GPU host keys the layer's texture by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerKey(pub u64);

/// Coarse stacking order, bottom to top. Within a band, `z` decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Band {
    /// Drawn on the world, under its annotations: windows lying on a planet.
    WorldUnderlay,
    /// Labels and popovers that follow points of the world.
    WorldAnnotation,
    /// The fixed interface: bars, panels, flat windows.
    Chrome,
    /// Transient things over the interface: menus, notifications.
    Overlay,
    /// A dialog that takes all input until it is answered.
    Modal,
    Debug,
}

/// How the pointer treats a layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interaction {
    pub hit: HitTest,
    /// The part of the layer, in its own texels, that moves it when dragged
    /// instead of being delivered to its controls: a title bar.
    pub drag_handle: Option<Rect>,
    /// Takes every pointer event while it is shown, even those that miss it.
    pub modal: bool,
    /// Whether dropping the layer on the surface lays it down there.
    pub lays_down: bool,
}

impl Default for Interaction {
    fn default() -> Interaction {
        Interaction {
            hit: HitTest::Alpha(8),
            drag_handle: None,
            modal: false,
            lays_down: true,
        }
    }
}

/// What [`Layer::prepare`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prepared {
    /// There is something to draw this frame.
    pub visible: bool,
    /// Something will be different next frame: ask for one.
    pub animating: bool,
}

/// See the [module documentation](self).
#[derive(Debug)]
pub struct Layer {
    pub key: LayerKey,
    pub band: Band,
    pub z: i32,
    pub surface: Surface,
    pub sheet: Sheet,
    pub effects: Effects,
    /// `0..=1`.
    pub opacity: f32,
    /// Corner radius of the painted content, in texels: the shape a glow
    /// follows.
    pub corner: f32,
    /// Width in pixels of the rim a light glints on while the layer lies on
    /// a surface.
    pub glint_px: f32,
    pub interaction: Interaction,
    /// Not drawn and not hit.
    pub hidden: bool,
    resolved: Resolved,
}

impl Layer {
    /// A flat layer showing `surface` with its top-left at the origin, and
    /// no shadow: a panel, a notification. A window that can lie on a surface
    /// is given its sheet with [`Layer::with_sheet`].
    pub fn new(key: LayerKey, band: Band, surface: Surface) -> Layer {
        let mut sheet = Sheet::flat(surface.size(), [0.0, 0.0]);
        sheet.tuning.shadow = None;
        Layer {
            key,
            band,
            z: 0,
            surface,
            sheet,
            effects: Effects::default(),
            opacity: 1.0,
            corner: 0.0,
            glint_px: 3.0,
            interaction: Interaction::default(),
            hidden: false,
            resolved: Effects::default().resolve(
                None,
                Shape {
                    size: [0.0, 0.0],
                    content: RectF::default(),
                    corner: 0.0,
                },
            ),
        }
    }

    /// Replace the sheet (to start a layer on a surface).
    #[must_use]
    pub fn with_sheet(mut self, sheet: Sheet) -> Layer {
        self.sheet = sheet;
        self
    }

    /// Settle this frame: advance the sheet, resolve the effects, and rebuild
    /// the mesh if anything it depends on moved. `now` is host time (`None`:
    /// no clock); `scene` is what a lying sheet lies on.
    pub fn prepare(&mut self, now: Option<Duration>, scene: Option<&Scene<'_>>) -> Prepared {
        let size = self.surface.size();
        self.sheet.set_size(size);
        let activity = self.sheet.advance(now);
        let content = self.surface.content();
        self.resolved = self.effects.resolve(
            now,
            Shape {
                size: [size[0] as f32, size[1] as f32],
                content: RectF::new(
                    content.x as f32,
                    content.y as f32,
                    content.w as f32,
                    content.h as f32,
                ),
                corner: self.corner,
            },
        );
        let mips = self.surface.mips().len() as u8;
        self.sheet.build(
            scene,
            &MeshOptions {
                tuning: if mips > 0 {
                    MeshTuning::with_mips(mips)
                } else {
                    MeshTuning::PLAIN
                },
                scale: self.resolved.scale,
            },
        );
        Prepared {
            visible: self.is_visible(),
            animating: activity.animating || self.resolved.animating,
        }
    }

    /// Whether the layer draws anything, as last prepared.
    pub fn is_visible(&self) -> bool {
        !self.hidden && self.draw_opacity() > 0.0 && !self.sheet.mesh().is_empty()
    }

    /// This frame's effects, settled by the last [`Layer::prepare`].
    pub fn resolved(&self) -> &Resolved {
        &self.resolved
    }

    /// The opacity the layer is drawn at: its own, times any flicker.
    pub fn draw_opacity(&self) -> f32 {
        (self.opacity * self.resolved.flicker).clamp(0.0, 1.0)
    }

    /// The uniform block for drawing this layer into a `viewport`-sized
    /// target.
    pub fn uniforms(&self, viewport: [u32; 2]) -> LayerUniforms {
        self.resolved.uniforms.for_draw(
            viewport,
            self.draw_opacity(),
            self.glint_px,
            self.surface.mips().len() as u8,
        )
    }

    /// The pixels the layer can touch, as last prepared.
    pub fn screen_bounds(&self) -> Option<RectF> {
        let own = self.sheet.mesh().bounds();
        match (
            own,
            self.sheet.shadow().and_then(|(mesh, _, _)| mesh.bounds()),
        ) {
            (Some(a), Some(b)) => Some(a.union(b)),
            (a, b) => a.or(b),
        }
    }

    /// The texel of the surface under screen position `px`, if the layer is
    /// there to be touched: through the mesh as drawn, then the surface's own
    /// hit test.
    pub fn hit(&self, px: [f32; 2]) -> Option<[f32; 2]> {
        if self.hidden {
            return None;
        }
        let texel = self.sheet.hit(px)?;
        self.surface
            .hit(texel, self.interaction.hit)
            .then_some(texel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::Ease;
    use crate::effect::{Glow, Pop};
    use crate::geom::Argb;

    fn layer() -> Layer {
        let mut surface = Surface::new(80, 40);
        // Mark it painted without a font: fill the buffer through a paint
        // that draws one rectangle.
        let mut text = liminalkit_text::TextCache::new(liminalkit_test_fonts::set());
        let mut kernel = liminalkit_raster::RasterKernel::new();
        surface.paint(
            crate::surface::PaintContext {
                text: &mut text,
                kernel: &mut kernel,
                theme: liminalkit_ui::Theme::default(),
                scale: liminalkit_shell::Scale(1.0),
                now: None,
                smooth_edges: false,
            },
            |ui, _| {
                let content = ui.painter.bounds().inset(10);
                ui.painter.fill_rect(content, 0x0030_6090);
            },
        );
        let mut layer = Layer::new(LayerKey(7), Band::Chrome, surface);
        layer.sheet.set_flat_origin([100.0, 50.0]);
        layer
    }

    #[test]
    fn a_flat_layer_is_where_its_sheet_says_and_hit_through_its_alpha() {
        let mut layer = layer();
        let prepared = layer.prepare(None, None);
        assert_eq!(
            prepared,
            Prepared {
                visible: true,
                animating: false
            }
        );
        // The mesh carries a one-texel ring for the edge.
        assert_eq!(
            layer.screen_bounds(),
            Some(RectF::new(99.0, 49.0, 82.0, 42.0))
        );
        assert_eq!(layer.hit([130.5, 70.5]), Some([30.5, 20.5]));
        assert!(layer.hit([102.0, 52.0]).is_none(), "the transparent border");
        assert!(layer.hit([10.0, 10.0]).is_none());
        layer.interaction.hit = HitTest::Bounds;
        assert!(layer.hit([102.0, 52.0]).is_some());
        layer.hidden = true;
        assert!(layer.hit([130.5, 70.5]).is_none());
        assert!(!layer.is_visible());
    }

    #[test]
    fn effects_reach_the_uniforms_and_a_pop_reaches_the_mesh() {
        let mut layer = layer();
        layer.surface.set_margin(10);
        layer.corner = 4.0;
        layer.opacity = 0.5;
        layer.effects.glow = Some(Glow {
            color: Argb::WHITE,
            intensity: 1.0,
            radius: 8.0,
            outline: 0.0,
            pulse: None,
        });
        layer.effects.pop = Some(Pop {
            fired: Duration::ZERO,
            duration: Duration::from_millis(200),
            from: 0.5,
            ease: Ease::Linear,
        });
        let prepared = layer.prepare(Some(Duration::ZERO), None);
        assert!(prepared.animating, "the pop is running");
        let uniforms = layer.uniforms([1280, 720]);
        assert_eq!(uniforms.size, [1280.0, 720.0, 80.0, 40.0]);
        assert_eq!(uniforms.params[0], 0.5);
        assert_eq!(uniforms.glow_rect, [10.0, 10.0, 70.0, 30.0]);
        assert_eq!(uniforms.glow_shape[0], 4.0);
        // Half size about the centre (140, 70).
        assert_eq!(
            layer.screen_bounds(),
            Some(RectF::new(119.5, 59.5, 41.0, 21.0))
        );
        let settled = layer.prepare(Some(Duration::from_millis(400)), None);
        assert!(!settled.animating);
        assert_eq!(
            layer.screen_bounds(),
            Some(RectF::new(99.0, 49.0, 82.0, 42.0))
        );
    }

    #[test]
    fn bands_order_before_z() {
        assert!(Band::WorldUnderlay < Band::Chrome && Band::Chrome < Band::Modal);
        assert!((Band::Chrome, 100) < (Band::Overlay, -5));
    }
}
