//! Routing the pointer to layers. **EXPERIMENTAL.**
//!
//! A game has two things under the pointer: its interface and its world. The
//! [`Router`] decides, event by event, which the pointer belongs to. It walks
//! the layers from the top, finds the first one the pointer is really on —
//! through that layer's mesh as drawn, so a window bent over a planet is hit
//! where it appears — and delivers the event to that layer's own
//! [`liminalkit_shell::Input`] in the layer's own coordinates. What it
//! reports back, [`Routed::consumed`], is whether the game's camera and
//! markers should ignore the event.
//!
//! This is the toolkit's rule for anything that floats, applied to layers:
//! input is claimed from the top before anything beneath sees it. Call the
//! router as events arrive, before painting.
//!
//! The router keeps three caller-chosen [`LayerKey`]s — hovered, captured,
//! focused — and the pointer's last position. That is all its state; it holds
//! no layers and no registry. The layers are handed to it on every call.
//!
//! A press on a layer's drag handle is not delivered to the layer: it starts
//! moving the layer's [`crate::sheet::Sheet`], and the router drives that
//! drag — along the surface, off it into a peel, back down — until release.

use std::time::Duration;

use liminalkit_shell::{CursorShape, KeyInput, Modifiers, MouseButton};

use crate::layer::{Band, Layer, LayerKey};
use crate::sheet::{DragOutcome, Scene};

/// A pointer event, in device pixels of the game's window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerEvent {
    Moved([f32; 2]),
    /// The pointer left the window.
    Left,
    Button {
        button: MouseButton,
        pressed: bool,
    },
    /// A wheel movement, in lines (positive `dy` scrolls content down).
    Wheel {
        dx: f32,
        dy: f32,
    },
    /// The window lost focus: every interaction in progress ends.
    FocusLost,
}

/// What the pointer is held on between a press and its release.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Capture {
    /// A layer's controls: moves keep going to it even off its edge.
    Content { key: LayerKey, button: MouseButton },
    /// A layer's drag handle: moves move the layer.
    Move { key: LayerKey },
}

impl Capture {
    pub fn key(self) -> LayerKey {
        match self {
            Capture::Content { key, .. } | Capture::Move { key } => key,
        }
    }
}

/// What became of an event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Routed {
    /// The interface took it: the world should not act on it.
    pub consumed: bool,
    /// The layer it went to.
    pub target: Option<LayerKey>,
    /// If it moved a layer, what that did.
    pub moved: Option<DragOutcome>,
}

impl Routed {
    const WORLD: Routed = Routed {
        consumed: false,
        target: None,
        moved: None,
    };
}

/// See the [module documentation](self).
#[derive(Debug, Default)]
pub struct Router {
    hover: Option<LayerKey>,
    capture: Option<Capture>,
    focus: Option<LayerKey>,
    cursor: Option<[f32; 2]>,
}

impl Router {
    pub fn new() -> Router {
        Router::default()
    }

    /// The layer under the pointer.
    pub fn hovered(&self) -> Option<LayerKey> {
        self.hover
    }

    /// The layer keys go to: the last one pressed.
    pub fn focused(&self) -> Option<LayerKey> {
        self.focus
    }

    pub fn captured(&self) -> Option<Capture> {
        self.capture
    }

    /// Whether the pointer currently belongs to the interface.
    pub fn owns_pointer(&self) -> bool {
        self.hover.is_some() || self.capture.is_some()
    }

    /// The pointer shape the interface wants, if the pointer is on it.
    pub fn cursor_shape(&self, layers: &[&mut Layer]) -> Option<CursorShape> {
        if let Some(Capture::Move { .. }) = self.capture {
            return Some(CursorShape::Grabbing);
        }
        let key = self.capture.map(Capture::key).or(self.hover)?;
        let layer = layers.iter().find(|l| l.key == key)?;
        let on_handle = self
            .cursor
            .and_then(|at| layer.hit(at))
            .is_some_and(|texel| {
                layer
                    .interaction
                    .drag_handle
                    .is_some_and(|handle| handle.contains(texel[0] as i32, texel[1] as i32))
            });
        Some(if on_handle {
            CursorShape::Grab
        } else {
            layer.surface.cursor_shape()
        })
    }

    /// Route a pointer event. `layers` is every layer that can be touched, in
    /// any order; `scene` is what lying sheets lie on; `now` is host time,
    /// for a peel or a lay the event may start.
    pub fn pointer(
        &mut self,
        event: PointerEvent,
        layers: &mut [&mut Layer],
        scene: Option<&Scene<'_>>,
        now: Option<Duration>,
    ) -> Routed {
        match event {
            PointerEvent::Moved(at) => {
                self.cursor = Some(at);
                self.moved(at, layers, scene, now)
            }
            PointerEvent::Left => {
                self.cursor = None;
                if self.capture.is_some() {
                    // A held button keeps reporting from outside the window.
                    return self.captured_result();
                }
                self.set_hover(None, layers);
                Routed::WORLD
            }
            PointerEvent::Button { button, pressed } => {
                if pressed {
                    self.pressed(button, layers)
                } else {
                    self.released(button, layers, scene, now)
                }
            }
            PointerEvent::Wheel { dx, dy } => {
                let Some(layer) = self.hover.and_then(|key| find(layers, key)) else {
                    return self.modal_or_world(layers);
                };
                layer.surface.input.scrolled(dx, dy);
                layer.surface.invalidate();
                Routed {
                    consumed: true,
                    target: Some(layer.key),
                    moved: None,
                }
            }
            PointerEvent::FocusLost => {
                self.cancel(layers);
                Routed::WORLD
            }
        }
    }

    /// Deliver a key to the focused layer. `false` if there is none: the key
    /// is the game's.
    pub fn key(&mut self, key: KeyInput, layers: &mut [&mut Layer]) -> bool {
        let Some(layer) = self.focus.and_then(|focus| find(layers, focus)) else {
            return false;
        };
        layer.surface.input.key(key);
        layer.surface.invalidate();
        true
    }

    /// Tell every layer the modifier keys changed.
    pub fn modifiers(&mut self, modifiers: Modifiers, layers: &mut [&mut Layer]) {
        for layer in layers.iter_mut() {
            layer.surface.input.modifiers_changed(modifiers);
        }
    }

    /// Give keyboard focus to a layer, or to none.
    pub fn set_focus(&mut self, key: Option<LayerKey>) {
        self.focus = key;
    }

    /// End every interaction in progress: a held drag is abandoned where it
    /// is, nothing is hovered.
    pub fn cancel(&mut self, layers: &mut [&mut Layer]) {
        if let Some(capture) = self.capture.take() {
            if let Some(layer) = find(layers, capture.key()) {
                layer.sheet.cancel_drag();
                layer.surface.input.blur();
                layer.surface.invalidate();
            }
        }
        self.set_hover(None, layers);
        self.cursor = None;
    }

    fn moved(
        &mut self,
        at: [f32; 2],
        layers: &mut [&mut Layer],
        scene: Option<&Scene<'_>>,
        now: Option<Duration>,
    ) -> Routed {
        let precise = [f64::from(at[0]), f64::from(at[1])];
        match self.capture {
            Some(Capture::Move { key }) => {
                let moved = find(layers, key)
                    .zip(scene)
                    .map(|(layer, scene)| layer.sheet.drag_to(scene, precise, now));
                Routed {
                    consumed: true,
                    target: Some(key),
                    moved,
                }
            }
            Some(Capture::Content { key, .. }) => {
                if let Some(layer) = find(layers, key) {
                    // Off the layer's edge the mesh has no answer; the
                    // sheet's own inverse does.
                    if let Some(local) = layer.sheet.to_local(scene, precise) {
                        layer.surface.input.cursor_moved(local[0], local[1]);
                        layer.surface.invalidate();
                    }
                }
                self.captured_result()
            }
            None => {
                let hit = topmost(layers, at);
                self.set_hover(hit.map(|(key, _)| key), layers);
                match hit {
                    Some((key, texel)) => {
                        if let Some(layer) = find(layers, key) {
                            layer.surface.input.cursor_moved(texel[0], texel[1]);
                            layer.surface.invalidate();
                        }
                        Routed {
                            consumed: true,
                            target: Some(key),
                            moved: None,
                        }
                    }
                    None => self.modal_or_world(layers),
                }
            }
        }
    }

    fn pressed(&mut self, button: MouseButton, layers: &mut [&mut Layer]) -> Routed {
        if self.capture.is_some() {
            // A second button during a drag changes nothing.
            return self.captured_result();
        }
        let Some((key, texel, at)) = self
            .cursor
            .and_then(|at| topmost(layers, at).map(|(key, texel)| (key, texel, at)))
        else {
            return self.modal_or_world(layers);
        };
        self.focus = Some(key);
        let Some(layer) = find(layers, key) else {
            return Routed::WORLD;
        };
        let on_handle = button == MouseButton::Left
            && layer
                .interaction
                .drag_handle
                .is_some_and(|handle| handle.contains(texel[0] as i32, texel[1] as i32));
        if on_handle {
            layer
                .sheet
                .begin_drag(texel, [f64::from(at[0]), f64::from(at[1])]);
            self.capture = Some(Capture::Move { key });
        } else {
            layer.surface.input.cursor_moved(texel[0], texel[1]);
            layer.surface.input.mouse(button, true);
            layer.surface.invalidate();
            self.capture = Some(Capture::Content { key, button });
        }
        Routed {
            consumed: true,
            target: Some(key),
            moved: None,
        }
    }

    fn released(
        &mut self,
        button: MouseButton,
        layers: &mut [&mut Layer],
        scene: Option<&Scene<'_>>,
        now: Option<Duration>,
    ) -> Routed {
        let routed = match self.capture {
            Some(Capture::Move { key }) if button == MouseButton::Left => {
                self.capture = None;
                let cursor = self.cursor;
                let moved = find(layers, key).map(|layer| match (scene, cursor) {
                    (Some(scene), Some(at)) => layer.sheet.end_drag(
                        scene,
                        [f64::from(at[0]), f64::from(at[1])],
                        layer.interaction.lays_down,
                        now,
                    ),
                    _ => {
                        layer.sheet.cancel_drag();
                        DragOutcome::Ignored
                    }
                });
                Routed {
                    consumed: true,
                    target: Some(key),
                    moved,
                }
            }
            Some(Capture::Content { key, button: held }) if held == button => {
                self.capture = None;
                if let Some(layer) = find(layers, key) {
                    layer.surface.input.mouse(button, false);
                    layer.surface.invalidate();
                }
                Routed {
                    consumed: true,
                    target: Some(key),
                    moved: None,
                }
            }
            Some(_) => return self.captured_result(),
            None => return self.modal_or_world(layers),
        };
        // The pointer may have ended somewhere else than it started.
        if let Some(at) = self.cursor {
            let hit = topmost(layers, at).map(|(key, _)| key);
            self.set_hover(hit, layers);
        }
        routed
    }

    fn captured_result(&self) -> Routed {
        Routed {
            consumed: true,
            target: self.capture.map(Capture::key),
            moved: None,
        }
    }

    /// An event that hit no layer: the world's, unless a modal layer is up.
    fn modal_or_world(&self, layers: &[&mut Layer]) -> Routed {
        let modal = layers
            .iter()
            .any(|l| !l.hidden && l.band >= Band::Modal && l.interaction.modal);
        Routed {
            consumed: modal,
            ..Routed::WORLD
        }
    }

    fn set_hover(&mut self, to: Option<LayerKey>, layers: &mut [&mut Layer]) {
        if self.hover == to {
            return;
        }
        if let Some(old) = self.hover.and_then(|key| find(layers, key)) {
            old.surface.input.cursor_left();
            old.surface.invalidate();
            old.sheet.set_hovered(false);
        }
        if let Some(new) = to.and_then(|key| find(layers, key)) {
            new.sheet.set_hovered(true);
        }
        self.hover = to;
    }
}

fn find<'l>(layers: &'l mut [&mut Layer], key: LayerKey) -> Option<&'l mut Layer> {
    layers
        .iter_mut()
        .find(|layer| layer.key == key)
        .map(|layer| &mut **layer)
}

/// The topmost layer under `at`, and the texel of it there. Beneath a modal
/// layer nothing is reachable.
fn topmost(layers: &[&mut Layer], at: [f32; 2]) -> Option<(LayerKey, [f32; 2])> {
    /// Stacking order: band, then `z`, then position in the slice.
    type Order = (Band, i32, usize);
    let mut best: Option<(Order, LayerKey, [f32; 2])> = None;
    let mut floor: Option<Order> = None;
    for (index, layer) in layers.iter().enumerate() {
        if layer.hidden {
            continue;
        }
        let order = (layer.band, layer.z, index);
        if layer.band >= Band::Modal && layer.interaction.modal {
            floor = Some(floor.map_or(order, |f| f.max(order)));
        }
        if best.as_ref().is_some_and(|(top, _, _)| *top > order) {
            continue;
        }
        if let Some(texel) = layer.hit(at) {
            best = Some((order, layer.key, texel));
        }
    }
    let (order, key, texel) = best?;
    // Anything below the topmost modal layer is behind it.
    (floor.is_none_or(|floor| order >= floor)).then_some((key, texel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::{HitTest, PaintContext, Surface};
    use liminalkit_raster::{RasterKernel, Rect};
    use liminalkit_shell::Scale;
    use liminalkit_text::TextCache;
    use liminalkit_ui::Theme;

    struct Rig {
        text: TextCache,
        kernel: RasterKernel,
    }

    impl Rig {
        fn new() -> Rig {
            Rig {
                text: TextCache::new(liminalkit_test_fonts::set()),
                kernel: RasterKernel::new(),
            }
        }

        /// Paint a layer as one button filling it below a 10-texel title
        /// strip; returns whether the button was clicked.
        fn paint(&mut self, layer: &mut Layer) -> bool {
            let clicked = layer.surface.paint(
                PaintContext {
                    text: &mut self.text,
                    kernel: &mut self.kernel,
                    theme: Theme::default(),
                    scale: Scale(1.0),
                    now: None,
                    smooth_edges: false,
                },
                |ui, _| {
                    let all = ui.painter.bounds();
                    ui.painter.fill_rect(all, 0x0020_2020);
                    let (_, body) = all.split_top(10);
                    ui.button(body, "Go")
                },
            );
            layer.prepare(None, None);
            clicked
        }

        fn layer(&mut self, key: u64, band: Band, at: [f32; 2]) -> Layer {
            let mut layer = Layer::new(LayerKey(key), band, Surface::new(100, 60));
            layer.sheet.set_flat_origin(at);
            self.paint(&mut layer);
            layer
        }
    }

    fn press() -> PointerEvent {
        PointerEvent::Button {
            button: MouseButton::Left,
            pressed: true,
        }
    }

    fn release() -> PointerEvent {
        PointerEvent::Button {
            button: MouseButton::Left,
            pressed: false,
        }
    }

    #[test]
    fn a_click_reaches_the_button_in_the_layers_own_coordinates() {
        let mut rig = Rig::new();
        let mut layer = rig.layer(1, Band::Chrome, [200.0, 100.0]);
        let mut router = Router::new();
        let moved = router.pointer(
            PointerEvent::Moved([250.0, 130.0]),
            &mut [&mut layer],
            None,
            None,
        );
        assert_eq!((moved.consumed, moved.target), (true, Some(LayerKey(1))));
        assert_eq!(layer.surface.input.cursor(), Some((50, 30)));
        assert!(
            router
                .pointer(press(), &mut [&mut layer], None, None)
                .consumed
        );
        assert!(
            router
                .pointer(release(), &mut [&mut layer], None, None)
                .consumed
        );
        assert!(layer.surface.needs_paint(), "input makes it stale");
        assert!(rig.paint(&mut layer), "the button saw its click");
        assert_eq!(router.focused(), Some(LayerKey(1)));
    }

    #[test]
    fn what_misses_every_layer_is_the_worlds() {
        let mut rig = Rig::new();
        let mut layer = rig.layer(1, Band::Chrome, [200.0, 100.0]);
        let mut router = Router::new();
        let layers = &mut [&mut layer];
        for event in [
            PointerEvent::Moved([20.0, 20.0]),
            press(),
            release(),
            PointerEvent::Wheel { dx: 0.0, dy: 1.0 },
        ] {
            let routed = router.pointer(event, layers, None, None);
            assert_eq!(routed, Routed::WORLD, "{event:?}");
        }
        assert!(!router.owns_pointer());
        assert!(
            !router.key(KeyInput::Enter, layers),
            "no focus: the game's key"
        );
    }

    #[test]
    fn the_top_layer_claims_and_the_one_beneath_never_hears() {
        let mut rig = Rig::new();
        let mut below = rig.layer(1, Band::Chrome, [200.0, 100.0]);
        let mut above = rig.layer(2, Band::Overlay, [240.0, 120.0]);
        let mut router = Router::new();
        let layers = &mut [&mut above, &mut below];
        router.pointer(PointerEvent::Moved([260.0, 140.0]), layers, None, None);
        assert_eq!(router.hovered(), Some(LayerKey(2)));
        router.pointer(press(), layers, None, None);
        router.pointer(release(), layers, None, None);
        assert!(rig.paint(&mut above));
        assert!(!rig.paint(&mut below), "covered: it saw nothing");
        assert_eq!(below.surface.input.cursor(), None);
        // Moving off the upper layer onto the lower: the upper is told.
        let layers = &mut [&mut above, &mut below];
        router.pointer(PointerEvent::Moved([210.0, 115.0]), layers, None, None);
        assert_eq!(router.hovered(), Some(LayerKey(1)));
        assert_eq!(above.surface.input.cursor(), None, "the pointer left it");
        assert_eq!(below.surface.input.cursor(), Some((10, 15)));
    }

    #[test]
    fn a_press_keeps_the_pointer_until_release_even_off_the_layer() {
        let mut rig = Rig::new();
        let mut layer = rig.layer(1, Band::Chrome, [200.0, 100.0]);
        let mut router = Router::new();
        let layers = &mut [&mut layer];
        router.pointer(PointerEvent::Moved([250.0, 130.0]), layers, None, None);
        router.pointer(press(), layers, None, None);
        let away = router.pointer(PointerEvent::Moved([500.0, 400.0]), layers, None, None);
        assert_eq!((away.consumed, away.target), (true, Some(LayerKey(1))));
        assert!(
            router
                .pointer(PointerEvent::Left, layers, None, None)
                .consumed
        );
        assert_eq!(
            layer.surface.input.cursor(),
            Some((300, 300)),
            "tracked off its edge"
        );
        let layers = &mut [&mut layer];
        router.pointer(PointerEvent::Moved([500.0, 400.0]), layers, None, None);
        assert!(router.pointer(release(), layers, None, None).consumed);
        assert_eq!(router.captured(), None);
        assert_eq!(router.hovered(), None, "released over the world");
        // The toolkit's controls act on the press, so the button did fire.
        assert!(rig.paint(&mut layer));
    }

    #[test]
    fn clicks_fall_through_transparent_parts() {
        let mut rig = Rig::new();
        let mut layer = Layer::new(LayerKey(1), Band::Chrome, Surface::new(100, 60));
        // Painted with nothing: wholly transparent.
        layer.surface.paint(
            PaintContext {
                text: &mut rig.text,
                kernel: &mut rig.kernel,
                theme: Theme::default(),
                scale: Scale(1.0),
                now: None,
                smooth_edges: false,
            },
            |_, _| {},
        );
        layer.prepare(None, None);
        let mut router = Router::new();
        let through = router.pointer(
            PointerEvent::Moved([50.0, 30.0]),
            &mut [&mut layer],
            None,
            None,
        );
        assert!(!through.consumed);
        layer.interaction.hit = HitTest::Bounds;
        let caught = router.pointer(
            PointerEvent::Moved([50.0, 30.0]),
            &mut [&mut layer],
            None,
            None,
        );
        assert!(caught.consumed);
    }

    #[test]
    fn a_modal_layer_takes_everything_and_hides_what_is_under_it() {
        let mut rig = Rig::new();
        let mut panel = rig.layer(1, Band::Chrome, [200.0, 100.0]);
        let mut dialog = rig.layer(2, Band::Modal, [400.0, 300.0]);
        dialog.interaction.modal = true;
        let mut router = Router::new();
        let layers = &mut [&mut panel, &mut dialog];
        let on_panel = router.pointer(PointerEvent::Moved([250.0, 130.0]), layers, None, None);
        assert_eq!(
            (on_panel.consumed, on_panel.target),
            (true, None),
            "swallowed"
        );
        assert!(router.pointer(press(), layers, None, None).consumed);
        assert!(router.pointer(release(), layers, None, None).consumed);
        assert!(!rig.paint(&mut panel));
        let layers = &mut [&mut panel, &mut dialog];
        let on_dialog = router.pointer(PointerEvent::Moved([450.0, 330.0]), layers, None, None);
        assert_eq!(on_dialog.target, Some(LayerKey(2)));
        dialog.hidden = true;
        let layers = &mut [&mut panel, &mut dialog];
        let again = router.pointer(PointerEvent::Moved([250.0, 130.0]), layers, None, None);
        assert_eq!(
            again.target,
            Some(LayerKey(1)),
            "dismissed: the panel is back"
        );
    }

    #[test]
    fn the_handle_moves_the_layer_and_is_not_delivered() {
        let mut rig = Rig::new();
        let mut layer = rig.layer(1, Band::Chrome, [200.0, 100.0]);
        layer.interaction.drag_handle = Some(Rect::new(0, 0, 100, 10));
        let mut router = Router::new();
        // A scene is needed to drag; a plane under a pixel-exact camera will do.
        let view = crate::view::Pinhole::css([800, 600], 1000.0);
        let plane = crate::world::Plane {
            pose: crate::world::Pose::IDENTITY,
        };
        let scene = Scene {
            surface: &plane,
            view: &view,
            light: None,
        };
        let layers = &mut [&mut layer];
        router.pointer(
            PointerEvent::Moved([230.0, 104.0]),
            layers,
            Some(&scene),
            None,
        );
        assert_eq!(router.cursor_shape(layers), Some(CursorShape::Grab));
        router.pointer(press(), layers, Some(&scene), None);
        assert_eq!(router.captured(), Some(Capture::Move { key: LayerKey(1) }));
        assert_eq!(router.cursor_shape(layers), Some(CursorShape::Grabbing));
        let moved = router.pointer(
            PointerEvent::Moved([330.0, 154.0]),
            layers,
            Some(&scene),
            None,
        );
        assert_eq!(moved.moved, Some(DragOutcome::FlatMoving));
        layer.interaction.lays_down = false;
        let layers = &mut [&mut layer];
        let dropped = router.pointer(release(), layers, Some(&scene), None);
        assert_eq!(dropped.moved, Some(DragOutcome::FlatMoving));
        assert_eq!(layer.sheet.flat_origin(), [300.0, 150.0]);
        assert!(
            !layer.surface.input.is_held(),
            "the press never reached the controls"
        );
        assert!(!rig.paint(&mut layer));
    }

    #[test]
    fn losing_focus_ends_everything() {
        let mut rig = Rig::new();
        let mut layer = rig.layer(1, Band::Chrome, [200.0, 100.0]);
        let mut router = Router::new();
        let layers = &mut [&mut layer];
        router.pointer(PointerEvent::Moved([250.0, 130.0]), layers, None, None);
        router.pointer(press(), layers, None, None);
        router.pointer(PointerEvent::FocusLost, layers, None, None);
        assert_eq!((router.captured(), router.hovered()), (None, None));
        assert!(!layer.surface.input.is_held());
        assert!(!rig.paint(&mut layer), "an abandoned press is not a click");
    }

    #[test]
    fn keys_and_the_wheel_go_to_the_layer_they_belong_to() {
        let mut rig = Rig::new();
        let mut layer = rig.layer(1, Band::Chrome, [200.0, 100.0]);
        let mut router = Router::new();
        let layers = &mut [&mut layer];
        router.pointer(PointerEvent::Moved([250.0, 130.0]), layers, None, None);
        let wheel = router.pointer(PointerEvent::Wheel { dx: 0.0, dy: 2.0 }, layers, None, None);
        assert!(wheel.consumed);
        router.set_focus(Some(LayerKey(1)));
        assert!(router.key(KeyInput::Enter, layers));
        assert_eq!(layer.surface.input.keys(), &[KeyInput::Enter]);
    }
}
