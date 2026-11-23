//! A window as a sheet: lying on a surface, flat on the screen, or on its way
//! between the two. **EXPERIMENTAL.**
//!
//! A [`Sheet`] knows where a layer's texture goes. It owns no pixels and no
//! input — only the placement: a [`Frame`] on a surface while it lies there, a
//! position on screen while it is flat, and one number, [`Sheet::flatness`],
//! that runs from the first to the second while it is peeled off or laid
//! down.
//!
//! The picture is a sticker on a globe. Drag it and it slides over the
//! surface, the point you hold staying under the cursor
//! ([`Sheet::drag_to`]). Drag it past the edge of the globe and it peels:
//! the part near your finger comes away first and the far edge follows, until
//! it is an ordinary flat window you are carrying. Drop a flat window on the
//! globe and it lays back down, the far edge landing first, with a ripple
//! where you pressed it.
//!
//! All of it is caller-owned state advanced by host time
//! ([`Sheet::advance`]); with no clock every transition completes at once.
//! The sheet never looks at the camera or the planet between calls: both
//! arrive as a [`Scene`] when they are needed.

use std::time::Duration;

use liminalkit_raster::WindowBuffer;

use crate::anim::{Ease, Tween};
use crate::geom::RectF;
use crate::math::{mix, mix_v3, V3};
use crate::view::View;
use crate::warp::{smoothstep, MeshTuning, Placed, WarpMesh};
use crate::world::{Frame, Surface3};

/// What a sheet is laid on and seen through, for one call.
#[derive(Clone, Copy)]
pub struct Scene<'a> {
    pub surface: &'a dyn Surface3,
    pub view: &'a dyn View,
    /// Unit direction *toward* the light, in world space, for the glint on a
    /// lying sheet's rim. `None`: no glint.
    pub light: Option<V3>,
}

impl std::fmt::Debug for Scene<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scene")
            .field("surface", &self.surface.fingerprint())
            .field("view", &self.view.fingerprint())
            .field("light", &self.light)
            .finish()
    }
}

/// Where a sheet is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Lying on the surface.
    Surface,
    /// Coming off it.
    Peeling,
    /// A flat window on the screen.
    Flat,
    /// Going back down.
    Laying,
}

/// How large a lying sheet is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Extent {
    /// Sized for the screen: a texel covers `scale` pixels where the sheet
    /// faces the viewer, wherever it lies and however far the camera is.
    Screen { scale: f64 },
    /// Sized for the surface: a texel is `m_per_texel` metres of ground, so
    /// the sheet shrinks as the camera pulls away.
    Surface { m_per_texel: f64 },
}

/// The soft shadow under a lifted or flat sheet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowStyle {
    /// Blur radius, in texels of the sheet.
    pub radius: f32,
    /// How far it falls below the sheet when the sheet is flat, in pixels.
    pub offset: f32,
    /// Corner radius of the shape casting it, in texels.
    pub corner: f32,
    /// Opacity under a fully lifted lying sheet, and under a flat one.
    pub opacity: [f32; 2],
}

impl Default for ShadowStyle {
    fn default() -> ShadowStyle {
        ShadowStyle {
            radius: 18.0,
            offset: 6.0,
            corner: 8.0,
            opacity: [0.35, 0.45],
        }
    }
}

/// The ripple that runs out from where a sheet was pressed down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RippleStyle {
    /// Height of the first crest, as a fraction of the full lift.
    pub amplitude: f32,
    /// Texels from crest to crest.
    pub wavelength: f32,
    /// Texels per second the front travels.
    pub speed: f32,
    pub duration: Duration,
}

impl Default for RippleStyle {
    fn default() -> RippleStyle {
        RippleStyle {
            amplitude: 0.6,
            wavelength: 90.0,
            speed: 600.0,
            duration: Duration::from_millis(650),
        }
    }
}

/// The numbers that shape a sheet's behaviour. The defaults are a starting
/// point judged on a desktop screen, not a calibration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SheetTuning {
    pub peel: Duration,
    pub lay: Duration,
    /// How much later the far edge follows the held point, `0..=1`.
    pub stagger: f32,
    /// A dragged sheet peels when the surface under the cursor faces the
    /// viewer by less than this (the cosine of the angle between them).
    pub facing_peel: f64,
    /// A dropped sheet lays down only where the surface faces the viewer by
    /// at least this. The gap between the two is hysteresis.
    pub facing_lay: f64,
    /// Facing below `[0]` is invisible, above `[1]` fully drawn: the fade at
    /// the horizon.
    pub horizon_fade: [f64; 2],
    /// Lift while hovered and while held, as fractions of `lift_px`.
    pub hover_lift: f32,
    pub drag_lift: f32,
    /// A full lift, in texels of the sheet above the surface.
    pub lift_px: f32,
    /// Time constant of the lift's approach.
    pub lift_time: Duration,
    /// Largest distance, in pixels, a straight mesh edge may stray from the
    /// true curve.
    pub tolerance_px: f32,
    /// Most cells along one side of the mesh.
    pub max_cells: u16,
    pub shadow: Option<ShadowStyle>,
    pub ripple: Option<RippleStyle>,
}

impl Default for SheetTuning {
    fn default() -> SheetTuning {
        SheetTuning {
            peel: Duration::from_millis(240),
            lay: Duration::from_millis(300),
            stagger: 0.5,
            facing_peel: 0.06,
            facing_lay: 0.20,
            horizon_fade: [0.04, 0.30],
            hover_lift: 0.35,
            drag_lift: 1.0,
            lift_px: 10.0,
            lift_time: Duration::from_millis(60),
            tolerance_px: 0.2,
            max_cells: 48,
            shadow: Some(ShadowStyle::default()),
            ripple: Some(RippleStyle::default()),
        }
    }
}

/// What [`Sheet::advance`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    /// Something is still moving: ask for another frame.
    pub animating: bool,
}

/// What a drag step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOutcome {
    /// Slid over the surface.
    Sliding,
    /// Left the surface on this step.
    Peeled,
    /// Moved as a flat window.
    FlatMoving,
    /// Was dropped on the surface and is laying down.
    Laid,
    /// Nothing to do (no drag in progress, or mid-landing).
    Ignored,
}

/// What a mesh build needs beyond the scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshOptions {
    pub tuning: MeshTuning,
    /// Scale about the sheet's centre: a pop.
    pub scale: f32,
}

impl Default for MeshOptions {
    fn default() -> MeshOptions {
        MeshOptions {
            tuning: MeshTuning::PLAIN,
            scale: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Grab {
    /// The held point, in texels.
    texel: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Ripple {
    origin: [f32; 2],
    started: Duration,
}

/// The shadow's texture is drawn at a quarter of the sheet's resolution: it
/// is a blur.
const SHADOW_DOWNSCALE: f32 = 4.0;

/// A layer's placement: see the [module documentation](self).
#[derive(Debug, Clone)]
pub struct Sheet {
    size: [u32; 2],
    /// The anchor on the surface: the sheet's centre. `None` for a sheet that
    /// has only ever been flat.
    pub frame: Option<Frame>,
    pub extent: Extent,
    pub tuning: SheetTuning,
    phase: Phase,
    /// Top-left of the flat sheet, device pixels, and pixels per texel.
    flat_origin: [f32; 2],
    flat_scale: f32,
    /// 0 lying … 1 flat.
    morph: Tween<f32>,
    lift: f32,
    hovered: bool,
    grab: Option<Grab>,
    /// The point the last peel or lay pivots about, kept after release so
    /// the motion finishes about the same point.
    pivot: [f32; 2],
    ripple: Option<Ripple>,
    clock: Option<Duration>,
    mesh: WarpMesh,
    shadow_mesh: WarpMesh,
    shadow_texture: WindowBuffer,
    shadow_revision: u64,
    shadow_style: Option<(ShadowStyle, [u32; 2])>,
    key: Option<u64>,
}

impl Sheet {
    /// A sheet of `size` texels lying on a surface at `frame`.
    pub fn on_surface(size: [u32; 2], frame: Frame, extent: Extent) -> Sheet {
        Sheet {
            frame: Some(frame),
            extent,
            phase: Phase::Surface,
            morph: Tween::new(0.0),
            ..Sheet::flat(size, [0.0, 0.0])
        }
    }

    /// A flat sheet of `size` texels with its top-left at `origin`.
    pub fn flat(size: [u32; 2], origin: [f32; 2]) -> Sheet {
        Sheet {
            size,
            frame: None,
            extent: Extent::Screen { scale: 1.0 },
            tuning: SheetTuning::default(),
            phase: Phase::Flat,
            flat_origin: origin,
            flat_scale: 1.0,
            morph: Tween::new(1.0),
            lift: 0.0,
            hovered: false,
            grab: None,
            pivot: [size[0] as f32 / 2.0, size[1] as f32 / 2.0],
            ripple: None,
            clock: None,
            mesh: WarpMesh::new(),
            shadow_mesh: WarpMesh::new(),
            shadow_texture: WindowBuffer::new(0, 0),
            shadow_revision: 0,
            shadow_style: None,
            key: None,
        }
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// The texture changed size. The sheet keeps its centre.
    pub fn set_size(&mut self, size: [u32; 2]) {
        if size == self.size {
            return;
        }
        let grow = |axis: usize| (size[axis] as f32 - self.size[axis] as f32) / 2.0;
        self.flat_origin = [
            self.flat_origin[0] - grow(0) * self.flat_scale,
            self.flat_origin[1] - grow(1) * self.flat_scale,
        ];
        self.pivot = [self.pivot[0] + grow(0), self.pivot[1] + grow(1)];
        self.size = size;
        self.key = None;
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// 0 lying on the surface … 1 flat. For the application's own emphasis: a
    /// stronger border or a more opaque backing as a window comes off.
    pub fn flatness(&self) -> f32 {
        self.morph.value()
    }

    /// How far the sheet is raised, `0..=1` of a full lift.
    pub fn lift(&self) -> f32 {
        self.lift
    }

    pub fn is_dragging(&self) -> bool {
        self.grab.is_some()
    }

    /// Where the flat sheet's top-left is, in device pixels.
    pub fn flat_origin(&self) -> [f32; 2] {
        self.flat_origin
    }

    /// Move the flat sheet. A lying sheet remembers it for when it peels.
    pub fn set_flat_origin(&mut self, origin: [f32; 2]) {
        self.flat_origin = origin;
    }

    /// Pixels per texel when flat (1 unless set).
    pub fn flat_scale(&self) -> f32 {
        self.flat_scale
    }

    pub fn set_flat_scale(&mut self, scale: f32) {
        if scale.is_finite() && scale > 0.0 {
            self.flat_scale = scale;
        }
    }

    /// The flat sheet's rectangle on screen.
    pub fn flat_rect(&self) -> RectF {
        RectF::new(
            self.flat_origin[0],
            self.flat_origin[1],
            self.size[0] as f32 * self.flat_scale,
            self.size[1] as f32 * self.flat_scale,
        )
    }

    /// The pointer is over the sheet (it rises a little) or has left.
    pub fn set_hovered(&mut self, hovered: bool) {
        self.hovered = hovered;
    }

    /// Advance to host time `now`. `None`: this host has no clock, and every
    /// transition lands at once.
    pub fn advance(&mut self, now: Option<Duration>) -> Activity {
        let previous = self.clock;
        if let Some(now) = now {
            self.clock = Some(self.clock.map_or(now, |clock| clock.max(now)));
        }
        let clock = if now.is_some() { self.clock } else { None };

        let morph = self.morph.update(clock);
        if !morph.animating {
            match self.phase {
                Phase::Peeling => self.phase = Phase::Flat,
                Phase::Laying => {
                    self.phase = Phase::Surface;
                    if let (Some(started), Some(_)) = (clock, self.tuning.ripple) {
                        self.ripple = Some(Ripple {
                            origin: self.pivot,
                            started,
                        });
                    }
                }
                Phase::Surface | Phase::Flat => {}
            }
        }

        let target = if self.grab.is_some() {
            self.tuning.drag_lift
        } else if self.hovered {
            self.tuning.hover_lift
        } else {
            0.0
        };
        let mut lifting = false;
        if (self.lift - target).abs() > f32::EPSILON {
            match (clock, previous) {
                (Some(clock), Some(previous)) if !self.tuning.lift_time.is_zero() => {
                    let elapsed = clock.saturating_sub(previous).as_secs_f32();
                    let rate = 1.0 - (-elapsed / self.tuning.lift_time.as_secs_f32()).exp();
                    self.lift += (target - self.lift) * rate;
                    if (self.lift - target).abs() < 0.002 {
                        self.lift = target;
                    } else {
                        lifting = true;
                    }
                }
                // The first frame with a clock has no interval to step by.
                (Some(_), None) if !self.tuning.lift_time.is_zero() => lifting = true,
                _ => self.lift = target,
            }
        }

        let mut rippling = false;
        if let Some(ripple) = self.ripple {
            let duration = self.tuning.ripple.map_or(Duration::ZERO, |r| r.duration);
            match clock {
                Some(clock) if clock.saturating_sub(ripple.started) < duration => rippling = true,
                _ => self.ripple = None,
            }
        }

        Activity {
            animating: morph.animating || lifting || rippling,
        }
    }

    /// Take the sheet off the surface where it is, without a drag: it
    /// flattens about its centre.
    pub fn peel(&mut self, scene: &Scene<'_>, now: Option<Duration>) {
        if self.phase != Phase::Surface {
            return;
        }
        let centre = self.centre();
        if let Some(at) = self.conformer(scene).and_then(|c| c.at(centre, true)) {
            self.flat_origin = self.origin_holding(centre, [at.px[0] as f32, at.px[1] as f32]);
        }
        self.start_peel(centre, now);
    }

    /// Lay a flat sheet down with its centre at screen position `px`.
    /// `false` if the surface is not there, or faces away too steeply.
    pub fn lay_at(&mut self, scene: &Scene<'_>, px: [f64; 2], now: Option<Duration>) -> bool {
        let centre = self.centre();
        self.lay_holding(scene, centre, px, now)
    }

    /// A press landed on the part of the sheet the application treats as its
    /// handle. `texel` is the point held, `cursor` where the pointer is.
    pub fn begin_drag(&mut self, texel: [f32; 2], cursor: [f64; 2]) {
        if self.phase == Phase::Laying {
            return;
        }
        self.grab = Some(Grab { texel });
        self.pivot = texel;
        if self.phase != Phase::Surface {
            self.flat_origin = self.origin_holding(texel, [cursor[0] as f32, cursor[1] as f32]);
        }
    }

    /// The pointer moved while holding the sheet.
    pub fn drag_to(
        &mut self,
        scene: &Scene<'_>,
        cursor: [f64; 2],
        now: Option<Duration>,
    ) -> DragOutcome {
        let Some(grab) = self.grab else {
            return DragOutcome::Ignored;
        };
        let cursor_px = [cursor[0] as f32, cursor[1] as f32];
        match self.phase {
            Phase::Laying => DragOutcome::Ignored,
            Phase::Peeling | Phase::Flat => {
                self.flat_origin = self.origin_holding(grab.texel, cursor_px);
                DragOutcome::FlatMoving
            }
            Phase::Surface => {
                let slid = self.frame.and_then(|_| {
                    let (hit, facing) = pick(scene, cursor)?;
                    if facing < self.tuning.facing_peel {
                        return None;
                    }
                    self.upright_frame_holding(scene, grab.texel, hit)
                });
                match slid {
                    Some(frame) => {
                        self.frame = Some(frame);
                        DragOutcome::Sliding
                    }
                    None => {
                        self.flat_origin = self.origin_holding(grab.texel, cursor_px);
                        self.start_peel(grab.texel, now);
                        DragOutcome::Peeled
                    }
                }
            }
        }
    }

    /// The pointer let go. With `lay`, a flat sheet released over the surface
    /// lays down there; without, it stays flat (the application's policy: a
    /// modifier key, a mode).
    pub fn end_drag(
        &mut self,
        scene: &Scene<'_>,
        cursor: [f64; 2],
        lay: bool,
        now: Option<Duration>,
    ) -> DragOutcome {
        let Some(grab) = self.grab.take() else {
            return DragOutcome::Ignored;
        };
        match self.phase {
            Phase::Surface => DragOutcome::Sliding,
            Phase::Laying => DragOutcome::Ignored,
            Phase::Peeling | Phase::Flat => {
                if lay && self.lay_holding(scene, grab.texel, cursor, now) {
                    return DragOutcome::Laid;
                }
                // At rest a flat window sits on whole pixels: crisp text, and
                // the compositor's exact path.
                self.flat_origin = self.flat_origin.map(f32::round);
                DragOutcome::FlatMoving
            }
        }
    }

    /// Abandon a drag where it is (the window lost focus mid-drag).
    pub fn cancel_drag(&mut self) {
        self.grab = None;
    }

    /// The texel under a screen position, through the mesh as last built.
    /// `None` off the sheet or where it has all but faded out.
    pub fn hit(&self, px: [f32; 2]) -> Option<[f32; 2]> {
        let (texel, alpha) = self.mesh.locate(px)?;
        let inside = texel[0] >= 0.0
            && texel[1] >= 0.0
            && texel[0] < self.size[0] as f32
            && texel[1] < self.size[1] as f32;
        (inside && alpha >= 0.25).then_some(texel)
    }

    /// Where a texel of the sheet is on screen, through the mesh as last
    /// built. `None` where that part of the sheet is not drawn.
    pub fn screen_of(&self, texel: [f32; 2]) -> Option<[f32; 2]> {
        self.mesh.screen_of(texel)
    }

    /// The texel a screen position corresponds to even off the sheet's edge:
    /// what a slider being dragged outside its window needs. Analytic where
    /// the sheet is at rest, through the mesh while it is in between.
    pub fn to_local(&self, scene: Option<&Scene<'_>>, px: [f64; 2]) -> Option<[f32; 2]> {
        match (self.phase, scene, self.frame) {
            (Phase::Surface, Some(scene), Some(_)) => {
                let frame = self.active_frame(scene)?;
                let ray = scene.view.ray(px)?;
                let pose = scene.surface.pose();
                let body = crate::view::Ray {
                    origin: pose.to_body(ray.origin),
                    dir: pose.dir_to_body(ray.dir),
                };
                let t = scene.surface.intersect(body, 0.0)?;
                let d = scene.surface.log(&frame, body.at(t))?;
                let scale = self.metres_per_texel(scene, &frame)?;
                Some([
                    (d[0] / scale) as f32 + self.size[0] as f32 / 2.0,
                    (-d[1] / scale) as f32 + self.size[1] as f32 / 2.0,
                ])
            }
            (Phase::Flat, _, _) => Some([
                (px[0] as f32 - self.flat_origin[0]) / self.flat_scale,
                (px[1] as f32 - self.flat_origin[1]) / self.flat_scale,
            ]),
            _ => self
                .mesh
                .locate([px[0] as f32, px[1] as f32])
                .map(|(texel, _)| texel),
        }
    }

    /// The mesh as last built by [`Sheet::build`].
    pub fn mesh(&self) -> &WarpMesh {
        &self.mesh
    }

    /// The shadow's mesh, texture and opacity, when there is one to draw.
    pub fn shadow(&self) -> Option<(&WarpMesh, &WindowBuffer, f32)> {
        let style = self.tuning.shadow?;
        let flatness = self.flatness();
        let opacity = style.opacity[0] * self.lift * (1.0 - flatness) + style.opacity[1] * flatness;
        (opacity > 0.003 && !self.shadow_mesh.is_empty()).then_some((
            &self.shadow_mesh,
            &self.shadow_texture,
            opacity,
        ))
    }

    /// Changes when the shadow's texture does: what a GPU host re-uploads on.
    pub fn shadow_revision(&self) -> u64 {
        self.shadow_revision
    }

    /// Rebuild the meshes if anything they depend on changed since the last
    /// call. `true` if they were rebuilt. Without a scene, a sheet that is not
    /// flat has nowhere to be and its mesh is empty.
    pub fn build(&mut self, scene: Option<&Scene<'_>>, options: &MeshOptions) -> bool {
        let key = self.key_for(scene, options);
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        self.refresh_shadow_texture();

        let (w, h) = (self.size[0] as f32, self.size[1] as f32);
        if w <= 0.0 || h <= 0.0 {
            self.mesh.clear();
            self.shadow_mesh.clear();
            return true;
        }
        // One texel of margin all round carries the edge's anti-aliasing.
        let content = RectF::new(-1.0, -1.0, w + 2.0, h + 2.0);
        let flatness = self.flatness();
        let conformer = if flatness < 1.0 {
            scene.and_then(|scene| self.conformer(scene))
        } else {
            None
        };
        let morph = Morph::new(self, conformer.as_ref(), flatness);
        let Some(morph) = morph else {
            self.mesh.clear();
            self.shadow_mesh.clear();
            return true;
        };

        let (cols, rows) = morph.cells(self, content);
        let mut mesh = std::mem::take(&mut self.mesh);
        mesh.build(content, cols, rows, &options.tuning, |texel| {
            morph.place(self, texel, true)
        });
        let centre = morph.place(self, self.centre(), true).map(|p| p.pos);
        if let (Some(centre), true) = (centre, options.scale != 1.0) {
            mesh.scale_about(centre, options.scale);
        }
        self.mesh = mesh;

        let mut shadow = std::mem::take(&mut self.shadow_mesh);
        shadow.clear();
        if let Some(style) = self.tuning.shadow {
            let texture = RectF::new(
                0.0,
                0.0,
                self.shadow_texture.width as f32,
                self.shadow_texture.height as f32,
            );
            // The shadow lies on the surface under a lifted sheet and falls
            // below a flat one.
            let drop = style.offset / self.flat_scale;
            shadow.build(
                texture,
                cols.min(24),
                rows.min(24),
                &MeshTuning::SOFT,
                |t| {
                    let texel = [
                        t[0] * SHADOW_DOWNSCALE - style.radius,
                        t[1] * SHADOW_DOWNSCALE - style.radius + drop * morph.flatness_at(self, t),
                    ];
                    morph.place(self, texel, false)
                },
            );
            if let (Some(centre), true) = (centre, options.scale != 1.0) {
                shadow.scale_about(centre, options.scale);
            }
        }
        self.shadow_mesh = shadow;
        true
    }

    fn centre(&self) -> [f32; 2] {
        [self.size[0] as f32 / 2.0, self.size[1] as f32 / 2.0]
    }

    /// The flat origin that puts `texel` at screen position `at`.
    fn origin_holding(&self, texel: [f32; 2], at: [f32; 2]) -> [f32; 2] {
        [
            at[0] - texel[0] * self.flat_scale,
            at[1] - texel[1] * self.flat_scale,
        ]
    }

    /// A texel's offset from the anchor in metres: x right, y up.
    fn offset(&self, texel: [f32; 2], metres_per_texel: f64) -> [f64; 2] {
        [
            f64::from(texel[0] - self.size[0] as f32 / 2.0) * metres_per_texel,
            -f64::from(texel[1] - self.size[1] as f32 / 2.0) * metres_per_texel,
        ]
    }

    fn metres_per_texel(&self, scene: &Scene<'_>, frame: &Frame) -> Option<f64> {
        let scale = match self.extent {
            Extent::Screen { scale } => {
                let anchor = scene.surface.pose().to_world(frame.origin);
                scale * scene.view.footprint_m(anchor)
            }
            Extent::Surface { m_per_texel } => m_per_texel,
        };
        if !(scale.is_finite() && scale > 0.0) {
            return None;
        }
        // No part of a sheet may reach further round than the surface allows.
        let reach = f64::from(self.size[0]).hypot(f64::from(self.size[1])) / 2.0;
        let limit = scene.surface.max_extent_m();
        Some(if limit.is_finite() && reach > 0.0 {
            scale.min(limit / reach)
        } else {
            scale
        })
    }

    /// The sheet's surface frame, aligned upright on screen at its anchor
    /// whenever the anchor faces the viewer. Keeping only the anchor point
    /// pinned to the surface prevents parallel-transport twist during drags
    /// and keeps text level as the surface turns under the camera.
    fn active_frame(&self, scene: &Scene<'_>) -> Option<Frame> {
        let frame = self.frame?;
        Some(upright_frame(scene, frame.origin).unwrap_or(frame))
    }

    /// Find the upright surface frame whose `texel` lands on `hit`.
    ///
    /// A screen-sized sheet's scale and upright orientation both vary slightly
    /// between `hit` and the sheet's centre anchor, so settle them together.
    fn upright_frame_holding(&self, scene: &Scene<'_>, texel: [f32; 2], hit: V3) -> Option<Frame> {
        let mut frame = upright_frame(scene, hit)?;
        for _ in 0..6 {
            let scale = self.metres_per_texel(scene, &frame)?;
            let offset = self.offset(texel, scale);
            let at = scene.surface.exp(&frame, offset, 0.0).pos;
            let moved = scene.surface.slide(&frame, at, hit)?;
            frame = upright_frame(scene, moved.origin).unwrap_or(moved);
        }
        Some(frame)
    }

    fn conformer<'a>(&self, scene: &'a Scene<'a>) -> Option<Conformer<'a>> {
        let frame = self.active_frame(scene)?;
        let scale = self.metres_per_texel(scene, &frame)?;
        let ripple = match (self.ripple, self.tuning.ripple, self.clock) {
            (Some(ripple), Some(style), Some(clock)) => Some((
                ripple.origin,
                clock.saturating_sub(ripple.started).as_secs_f32(),
                style,
            )),
            _ => None,
        };
        Some(Conformer {
            scene,
            frame,
            scale,
            size: self.size,
            lift_m: f64::from(self.lift * self.tuning.lift_px) * scale,
            lift_unit_m: f64::from(self.tuning.lift_px) * scale,
            ripple,
            fade: self.tuning.horizon_fade,
        })
    }

    fn start_peel(&mut self, pivot: [f32; 2], now: Option<Duration>) {
        self.pivot = pivot;
        self.phase = Phase::Peeling;
        self.ripple = None;
        self.morph.start(now, 1.0, self.tuning.peel, Ease::CubicOut);
        if !self.morph.is_animating() {
            self.phase = Phase::Flat;
        }
    }

    /// Lay the sheet down so that `texel` lands under screen position `px`.
    fn lay_holding(
        &mut self,
        scene: &Scene<'_>,
        texel: [f32; 2],
        px: [f64; 2],
        now: Option<Duration>,
    ) -> bool {
        if !matches!(self.phase, Phase::Flat | Phase::Peeling) {
            return false;
        }
        let Some((hit, facing)) = pick(scene, px) else {
            return false;
        };
        if facing < self.tuning.facing_lay {
            return false;
        }
        let Some(frame) = self.upright_frame_holding(scene, texel, hit) else {
            return false;
        };
        self.flat_origin = self.origin_holding(texel, [px[0] as f32, px[1] as f32]);
        self.frame = Some(frame);
        self.pivot = texel;
        self.phase = Phase::Laying;
        self.morph
            .start(now, 0.0, self.tuning.lay, Ease::CubicInOut);
        true
    }

    fn key_for(&self, scene: Option<&Scene<'_>>, options: &MeshOptions) -> u64 {
        let bits32 = |seed: u64, values: &[f32]| {
            values
                .iter()
                .fold(seed, |s, v| mix(s, u64::from(v.to_bits())))
        };
        let mut key = mix(
            0x0053_4845_4554,
            u64::from(self.size[0]) << 32 | u64::from(self.size[1]),
        );
        key = bits32(
            key,
            &[
                self.flat_origin[0],
                self.flat_origin[1],
                self.flat_scale,
                self.morph.value(),
                self.lift,
                self.pivot[0],
                self.pivot[1],
                options.scale,
                options.tuning.minify_fade[0],
                options.tuning.minify_fade[1],
                self.tuning.stagger,
                self.tuning.tolerance_px,
            ],
        );
        key = mix(key, u64::from(options.tuning.mip_levels));
        key = mix(key, self.phase as u64);
        key = match self.tuning.shadow {
            Some(style) => bits32(mix(key, 1), &[style.radius, style.offset, style.corner]),
            None => mix(key, 0),
        };
        if self.flatness() >= 1.0 {
            return key;
        }
        if let Some(frame) = self.frame {
            for v in [frame.origin, frame.x, frame.y] {
                key = mix_v3(key, v);
            }
        }
        key = match self.extent {
            Extent::Screen { scale } => mix(mix(key, 1), scale.to_bits()),
            Extent::Surface { m_per_texel } => mix(mix(key, 2), m_per_texel.to_bits()),
        };
        if let Some(scene) = scene {
            key = mix(key, scene.surface.fingerprint());
            key = mix(key, scene.view.fingerprint());
            if let Some(light) = scene.light {
                key = mix_v3(key, light);
            }
        } else {
            key = mix(key, 0xdead);
        }
        if let (Some(ripple), Some(clock)) = (self.ripple, self.clock) {
            key = mix(key, clock.saturating_sub(ripple.started).as_nanos() as u64);
        }
        key
    }

    fn refresh_shadow_texture(&mut self) {
        let Some(style) = self.tuning.shadow else {
            if self.shadow_style.take().is_some() {
                self.shadow_texture = WindowBuffer::new(0, 0);
                self.shadow_revision += 1;
            }
            return;
        };
        if self.shadow_style == Some((style, self.size)) {
            return;
        }
        self.shadow_style = Some((style, self.size));
        self.shadow_revision += 1;
        let (w, h) = (self.size[0] as f32, self.size[1] as f32);
        let across = |extent: f32| ((extent + 2.0 * style.radius) / SHADOW_DOWNSCALE).ceil() as u32;
        let mut texture = WindowBuffer::new(across(w).max(1), across(h).max(1));
        let corner = style.corner.min(w / 2.0).min(h / 2.0).max(0.0);
        for y in 0..texture.height {
            for x in 0..texture.width {
                // This texel's centre, in sheet texels from the sheet's centre.
                let px = (x as f32 + 0.5) * SHADOW_DOWNSCALE - style.radius - w / 2.0;
                let py = (y as f32 + 0.5) * SHADOW_DOWNSCALE - style.radius - h / 2.0;
                let qx = px.abs() - (w / 2.0 - corner);
                let qy = py.abs() - (h / 2.0 - corner);
                let outside = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - corner;
                let fall = (1.0 - outside / style.radius.max(1.0)).clamp(0.0, 1.0);
                let alpha = (fall * fall * 255.0).round() as u32;
                texture.pixels[(y * texture.width + x) as usize] = alpha << 24;
            }
        }
        self.shadow_texture = texture;
    }
}

/// The surface point under a screen position, in body coordinates, and how
/// squarely the surface there faces the viewer.
fn pick(scene: &Scene<'_>, px: [f64; 2]) -> Option<(V3, f64)> {
    let ray = scene.view.ray(px)?;
    let pose = scene.surface.pose();
    let body = crate::view::Ray {
        origin: pose.to_body(ray.origin),
        dir: pose.dir_to_body(ray.dir),
    };
    let hit = body.at(scene.surface.intersect(body, 0.0)?);
    let normal = any_frame(scene.surface, hit)?.n;
    let world = pose.to_world(hit);
    let facing = pose
        .dir_to_world(normal)
        .dot(scene.view.toward_viewer(world));
    Some((hit, facing))
}

/// Some frame at `p`: whichever axis is not along the normal serves as up.
fn any_frame(surface: &dyn Surface3, p: V3) -> Option<Frame> {
    [V3::Y, V3::Z, V3::X]
        .into_iter()
        .find_map(|hint| surface.frame_at(p, hint))
}

/// The frame at `p` whose x axis runs level across the screen, left to
/// right: text on a sheet laid there has a level baseline. (Up on the sheet
/// is then as near up on the screen as the tilt of the ground allows; a
/// surface seen at an angle cannot have both.)
fn upright_frame(scene: &Scene<'_>, p: V3) -> Option<Frame> {
    let frame = any_frame(scene.surface, p)?;
    let pose = scene.surface.pose();
    let world = pose.to_world(p);
    let facing = pose
        .dir_to_world(frame.n)
        .dot(scene.view.toward_viewer(world));
    if facing <= 1e-6 {
        return None;
    }
    let project = |d: [f64; 2]| {
        let at = scene.surface.exp(&frame, d, 0.0);
        scene.view.project(pose.to_world(at.pos)).map(|p| p.px)
    };
    // A step small enough to be straight, large enough to survive rounding.
    let step = scene.view.footprint_m(world).max(1e-9) * 4.0;
    let (origin, along_x, along_y) = (
        project([0.0, 0.0])?,
        project([step, 0.0])?,
        project([0.0, step])?,
    );
    let (a, b) = (along_x[0] - origin[0], along_y[0] - origin[0]);
    let (c, d) = (along_x[1] - origin[1], along_y[1] - origin[1]);
    let det = a * d - b * c;
    if det.abs() < 1e-12 {
        return None;
    }
    // The tangent direction that moves straight right on screen, and the
    // direction square to it in the surface: the sheet's up.
    let right = frame.x * (d / det) + frame.y * (-c / det);
    scene.surface.frame_at(p, frame.n.cross(right))
}

/// A texel carried onto the surface and on to the screen.
#[derive(Debug, Clone, Copy)]
struct Conformed {
    px: [f64; 2],
    alpha: f32,
    light: f32,
}

/// Everything needed to conform texels of one sheet for one build.
struct Conformer<'a> {
    scene: &'a Scene<'a>,
    frame: Frame,
    scale: f64,
    size: [u32; 2],
    lift_m: f64,
    lift_unit_m: f64,
    ripple: Option<([f32; 2], f32, RippleStyle)>,
    fade: [f64; 2],
}

impl Conformer<'_> {
    /// Where `texel` is on screen. With `raised`, at the sheet's lift (and
    /// riding any ripple); without, on the surface itself, as a shadow is.
    fn at(&self, texel: [f32; 2], raised: bool) -> Option<Conformed> {
        let d = [
            f64::from(texel[0] - self.size[0] as f32 / 2.0) * self.scale,
            -f64::from(texel[1] - self.size[1] as f32 / 2.0) * self.scale,
        ];
        let (mut lift, mut light) = (0.0, 0.0);
        if raised {
            lift = self.lift_m;
            if let Some((origin, elapsed, style)) = self.ripple {
                let distance = (texel[0] - origin[0]).hypot(texel[1] - origin[1]);
                let front = style.speed * elapsed;
                if distance < front && style.wavelength > 0.0 {
                    let phase = std::f32::consts::TAU * (distance - front) / style.wavelength;
                    let decay = style.amplitude * (-elapsed / 0.22).exp();
                    lift += f64::from(decay * phase.sin()).max(-1.0) * self.lift_unit_m;
                    // The slope, as shading: what makes a ripple read from
                    // straight above.
                    light -= 0.25 * decay * phase.cos();
                }
            }
        }
        let surface = self.scene.surface;
        let pose = surface.pose();
        let point = surface.exp(&self.frame, d, lift.max(0.0));
        let world = pose.to_world(point.pos);
        let normal = pose.dir_to_world(point.normal);
        let projected = self.scene.view.project(world)?;
        let toward = self.scene.view.toward_viewer(world);
        let facing = normal.dot(toward);
        let mut alpha = smoothstep(self.fade[0] as f32, self.fade[1] as f32, facing as f32);
        if alpha > 0.0 && lift > 0.0 && surface.occludes(world, toward) {
            alpha = 0.0;
        }
        if let (true, Some(sun)) = (raised, self.scene.light) {
            if let Some(half) = (sun + toward).try_normalize() {
                light += 0.6 * (normal.dot(half).max(0.0) as f32).powi(32);
            }
        }
        Some(Conformed {
            px: projected.px,
            alpha,
            light: light.clamp(-1.0, 1.0),
        })
    }
}

/// One build's blend between lying and flat.
struct Morph<'a> {
    conformer: Option<&'a Conformer<'a>>,
    flatness: f32,
    /// The pivot texel, where it is conformed, and where it is drawn.
    pivot: [f32; 2],
    pivot_conformed: [f64; 2],
    pivot_drawn: [f64; 2],
    /// Screen angle of the sheet's x axis at the pivot while lying.
    angle: f64,
    reach: f32,
}

impl<'a> Morph<'a> {
    fn new(
        sheet: &Sheet,
        conformer: Option<&'a Conformer<'a>>,
        flatness: f32,
    ) -> Option<Morph<'a>> {
        let pivot = sheet.pivot;
        let flat_pivot = [
            f64::from(sheet.flat_origin[0] + pivot[0] * sheet.flat_scale),
            f64::from(sheet.flat_origin[1] + pivot[1] * sheet.flat_scale),
        ];
        let reach = [0.0, sheet.size[0] as f32]
            .into_iter()
            .flat_map(|x| [0.0, sheet.size[1] as f32].map(|y| (x - pivot[0]).hypot(y - pivot[1])))
            .fold(1.0_f32, f32::max);
        let flat = Morph {
            conformer: None,
            flatness: 1.0,
            pivot,
            pivot_conformed: flat_pivot,
            pivot_drawn: flat_pivot,
            angle: 0.0,
            reach,
        };
        let Some(conformer) = conformer else {
            // Flat, or nothing to lie on: a lying sheet with no scene has no
            // place at all.
            return (flatness >= 1.0).then_some(flat);
        };
        if flatness <= 0.0 {
            return Some(Morph {
                conformer: Some(conformer),
                flatness: 0.0,
                ..flat
            });
        }
        // In between, everything is measured from the pivot.
        let Some(held) = conformer.at(pivot, true) else {
            return Some(flat);
        };
        let side = |dx: f32| conformer.at([pivot[0] + dx, pivot[1]], true).map(|c| c.px);
        let angle = match (side(-4.0), side(4.0)) {
            (Some(a), Some(b)) => (b[1] - a[1]).atan2(b[0] - a[0]),
            _ => 0.0,
        };
        // Peeling, the pivot is already where the flat sheet has it (in the
        // hand); laying, it travels down to the surface.
        let travel = if sheet.phase == Phase::Laying {
            f64::from(smoothstep(0.0, 1.0, flatness))
        } else {
            1.0
        };
        Some(Morph {
            conformer: Some(conformer),
            flatness,
            pivot_conformed: held.px,
            pivot_drawn: [
                held.px[0] + (flat_pivot[0] - held.px[0]) * travel,
                held.px[1] + (flat_pivot[1] - held.px[1]) * travel,
            ],
            angle,
            ..flat
        })
    }

    /// How flat the sheet is at `texel`: the held point leads, the far edge
    /// follows.
    fn flatness_at(&self, sheet: &Sheet, texel: [f32; 2]) -> f32 {
        if self.flatness <= 0.0 || self.flatness >= 1.0 {
            return self.flatness.clamp(0.0, 1.0);
        }
        let lag = sheet.tuning.stagger.clamp(0.0, 1.0);
        let far =
            ((texel[0] - self.pivot[0]).hypot(texel[1] - self.pivot[1]) / self.reach).min(1.0);
        smoothstep(0.0, 1.0, self.flatness * (1.0 + lag) - lag * far)
    }

    fn place(&self, sheet: &Sheet, texel: [f32; 2], raised: bool) -> Option<Placed> {
        let flat = |e: f64| {
            [
                f64::from((texel[0] - self.pivot[0]) * sheet.flat_scale) * e,
                f64::from((texel[1] - self.pivot[1]) * sheet.flat_scale) * e,
            ]
        };
        let Some(conformer) = self.conformer else {
            let offset = flat(1.0);
            return Some(Placed {
                pos: [
                    (self.pivot_drawn[0] + offset[0]) as f32,
                    (self.pivot_drawn[1] + offset[1]) as f32,
                ],
                alpha: 1.0,
                light: 0.0,
            });
        };
        if self.flatness <= 0.0 {
            let c = conformer.at(texel, raised)?;
            return Some(Placed {
                pos: [c.px[0] as f32, c.px[1] as f32],
                alpha: c.alpha,
                light: c.light,
            });
        }
        let e = f64::from(self.flatness_at(sheet, texel));
        let conformed = conformer.at(texel, raised);
        if conformed.is_none() && e < 1.0 {
            return None;
        }
        // The lying offset from the pivot, turned upright; the flat offset;
        // a blend of the two; then turned back by however much lying is left.
        let (sin, cos) = self.angle.sin_cos();
        let lying = conformed.map_or([0.0, 0.0], |c| {
            let (dx, dy) = (
                c.px[0] - self.pivot_conformed[0],
                c.px[1] - self.pivot_conformed[1],
            );
            [
                (dx * cos + dy * sin) * (1.0 - e),
                (-dx * sin + dy * cos) * (1.0 - e),
            ]
        });
        let target = flat(e);
        // A slight swell mid-way, so the sheet seems to come toward you.
        let swell = 1.0 + 0.03 * (std::f64::consts::PI * e).sin();
        let (bx, by) = (
            (lying[0] + target[0]) * swell,
            (lying[1] + target[1]) * swell,
        );
        let (sin, cos) = (self.angle * (1.0 - e)).sin_cos();
        let e = e as f32;
        Some(Placed {
            pos: [
                (self.pivot_drawn[0] + bx * cos - by * sin) as f32,
                (self.pivot_drawn[1] + bx * sin + by * cos) as f32,
            ],
            alpha: conformed.map_or(1.0, |c| c.alpha + (1.0 - c.alpha) * e),
            light: conformed.map_or(0.0, |c| c.light * (1.0 - e)),
        })
    }

    /// How many cells the mesh needs along each side for a straight edge to
    /// stay within tolerance of the curve.
    fn cells(&self, sheet: &Sheet, content: RectF) -> (u16, u16) {
        if self.conformer.is_none() {
            return (1, 1);
        }
        const PROBES: usize = 9;
        let mut points = [[None; PROBES]; PROBES];
        for (j, row) in points.iter_mut().enumerate() {
            for (i, point) in row.iter_mut().enumerate() {
                let texel = [
                    content.x + content.w * i as f32 / (PROBES - 1) as f32,
                    content.y + content.h * j as f32 / (PROBES - 1) as f32,
                ];
                *point = self.place(sheet, texel, true).map(|p| p.pos);
            }
        }
        // How far the middle of three probes strays from the chord of the
        // outer two: curvature and perspective together.
        let stray = |a: Option<[f32; 2]>, b: Option<[f32; 2]>, c: Option<[f32; 2]>| {
            let (a, b, c) = (a?, b?, c?);
            Some((b[0] - (a[0] + c[0]) / 2.0).hypot(b[1] - (a[1] + c[1]) / 2.0))
        };
        let (mut along_x, mut along_y) = (0.0_f32, 0.0_f32);
        for row in &points {
            for three in row.windows(3) {
                if let Some(e) = stray(three[0], three[1], three[2]) {
                    along_x = along_x.max(e);
                }
            }
        }
        for three in points.windows(3) {
            for ((a, b), c) in three[0].iter().zip(&three[1]).zip(&three[2]) {
                if let Some(e) = stray(*a, *b, *c) {
                    along_y = along_y.max(e);
                }
            }
        }
        let tolerance = sheet.tuning.tolerance_px.max(0.01);
        let most = sheet.tuning.max_cells.max(1);
        // A sheet in motion bends in ways the probes are too coarse to see.
        let least = if sheet.ripple.is_some() {
            24
        } else if self.flatness > 0.0 {
            12
        } else {
            1
        };
        let count = |stray: f32| {
            // The error of a chord falls with the square of its length.
            let needed = (0.5 * (PROBES - 1) as f32 * (stray / tolerance).sqrt()).ceil();
            (needed.max(1.0) as u16).max(least).min(most)
        };
        (count(along_x), count(along_y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::M3;
    use crate::view::Pinhole;
    use crate::world::{Pose, Sphere};

    const R: f64 = 6_371_000.0;

    /// A planet turned by `spin` radians about its y axis, seen from 800 km up
    /// over its +z pole with the camera pitched so the horizon is in frame.
    struct Rig {
        globe: Sphere,
        view: Pinhole,
    }

    impl Rig {
        fn new(spin: f64) -> Rig {
            let eye = V3::new(0.0, 0.0, R + 800_000.0);
            let pitch = M3::rodrigues(V3::X, 0.65);
            Rig {
                globe: Sphere {
                    radius_m: R,
                    pose: Pose {
                        origin: V3::ZERO,
                        rotation: M3::rodrigues(V3::Y, spin),
                    },
                },
                view: Pinhole {
                    eye,
                    right: pitch.mul_vec(V3::X),
                    up: pitch.mul_vec(V3::Y),
                    back: pitch.mul_vec(V3::Z),
                    tan_half_fov_y: (27.5_f64).to_radians().tan(),
                    viewport: [1280, 720],
                },
            }
        }

        fn scene(&self) -> Scene<'_> {
            Scene {
                surface: &self.globe,
                view: &self.view,
                light: None,
            }
        }

        /// A frame under screen position `px`, upright on screen.
        fn frame_under(&self, px: [f64; 2]) -> Frame {
            let scene = self.scene();
            let (hit, _) = pick(&scene, px).expect("on the planet");
            upright_frame(&scene, hit).expect("a frame")
        }
    }

    fn sheet_at(rig: &Rig, px: [f64; 2]) -> Sheet {
        let mut sheet = Sheet::on_surface(
            [240, 160],
            rig.frame_under(px),
            Extent::Screen { scale: 1.0 },
        );
        sheet.tuning.shadow = None;
        sheet
    }

    fn ms(n: u64) -> Option<Duration> {
        Some(Duration::from_millis(n))
    }

    fn screen_of(sheet: &Sheet, texel: [f32; 2]) -> [f32; 2] {
        sheet.screen_of(texel).expect("the texel is drawn")
    }

    #[test]
    fn a_lying_sheet_is_centred_on_its_anchor_upright_and_about_life_size() {
        let rig = Rig::new(0.0);
        let mut sheet = sheet_at(&rig, [640.0, 420.0]);
        assert!(sheet.build(Some(&rig.scene()), &MeshOptions::default()));
        let centre = screen_of(&sheet, [120.0, 80.0]);
        assert!(
            (centre[0] - 640.0).abs() < 0.5 && (centre[1] - 420.0).abs() < 0.5,
            "{centre:?}"
        );
        let (left, right) = (
            screen_of(&sheet, [20.0, 80.0]),
            screen_of(&sheet, [220.0, 80.0]),
        );
        assert!(
            right[0] - left[0] > 150.0 && right[0] - left[0] < 220.0,
            "about 200 px wide"
        );
        assert!((right[1] - left[1]).abs() < 3.0, "level on screen");
        let (top, bottom) = (
            screen_of(&sheet, [120.0, 20.0]),
            screen_of(&sheet, [120.0, 140.0]),
        );
        assert!(bottom[1] > top[1], "its top is up");
        assert!(
            bottom[1] - top[1] < 120.0,
            "foreshortened by the tilt of the ground"
        );
    }

    #[test]
    fn nothing_is_rebuilt_until_something_moves() {
        let mut rig = Rig::new(0.0);
        let mut sheet = sheet_at(&rig, [640.0, 420.0]);
        let options = MeshOptions::default();
        assert!(sheet.build(Some(&rig.scene()), &options));
        assert!(!sheet.build(Some(&rig.scene()), &options));
        rig.globe.pose.rotation = M3::rodrigues(V3::Y, 0.001);
        assert!(
            sheet.build(Some(&rig.scene()), &options),
            "the planet turned"
        );
        assert!(!sheet.build(Some(&rig.scene()), &options));
        assert!(sheet.build(
            Some(&rig.scene()),
            &MeshOptions {
                scale: 1.1,
                ..options
            }
        ));
    }

    #[test]
    fn a_sheet_turns_with_its_planet() {
        let mut sheet = sheet_at(&Rig::new(0.0), [640.0, 420.0]);
        let options = MeshOptions::default();
        let still = Rig::new(0.0);
        sheet.build(Some(&still.scene()), &options);
        let before = screen_of(&sheet, [120.0, 80.0]);
        let turned = Rig::new(0.01);
        sheet.build(Some(&turned.scene()), &options);
        let after = screen_of(&sheet, [120.0, 80.0]);
        assert!(
            after[0] - before[0] > 20.0,
            "carried east: {before:?} -> {after:?}"
        );
    }

    #[test]
    fn dragging_keeps_the_held_point_under_the_cursor_as_the_planet_turns() {
        let mut sheet = sheet_at(&Rig::new(0.0), [640.0, 420.0]);
        let options = MeshOptions::default();
        let rig = Rig::new(0.0);
        sheet.build(Some(&rig.scene()), &options);
        let grab = [60.0_f32, 30.0];
        let start = screen_of(&sheet, grab);
        sheet.begin_drag(grab, [f64::from(start[0]), f64::from(start[1])]);
        for step in 1..=40 {
            let rig = Rig::new(f64::from(step) * 0.0004);
            let cursor = [
                f64::from(start[0]) + f64::from(step) * 6.0,
                f64::from(start[1]) + f64::from(step) * 3.0,
            ];
            assert_eq!(
                sheet.drag_to(&rig.scene(), cursor, None),
                DragOutcome::Sliding
            );
            sheet.build(Some(&rig.scene()), &options);
            let held = screen_of(&sheet, grab);
            let miss = (f64::from(held[0]) - cursor[0]).hypot(f64::from(held[1]) - cursor[1]);
            assert!(miss < 0.5, "step {step}: {miss} px");
        }
        assert_eq!(sheet.phase(), Phase::Surface);
    }

    #[test]
    fn dragging_off_the_planet_peels_and_the_sheet_ends_flat_in_the_hand() {
        let rig = Rig::new(0.0);
        let mut sheet = sheet_at(&rig, [640.0, 300.0]);
        let options = MeshOptions::default();
        sheet.advance(ms(0));
        sheet.build(Some(&rig.scene()), &options);
        let grab = [120.0_f32, 10.0];
        let start = screen_of(&sheet, grab);
        sheet.begin_drag(grab, [f64::from(start[0]), f64::from(start[1])]);
        // Up the screen until the cursor is over space.
        let mut outcome = DragOutcome::Sliding;
        let mut y = f64::from(start[1]);
        while outcome == DragOutcome::Sliding && y > 0.0 {
            y -= 8.0;
            outcome = sheet.drag_to(&rig.scene(), [640.0, y], ms(0));
        }
        assert_eq!(outcome, DragOutcome::Peeled, "left the disc at y = {y}");
        assert_eq!(sheet.phase(), Phase::Peeling);
        // Half way: the held point is at the cursor, and no cell is inside out.
        sheet.advance(ms(100));
        assert!(sheet.flatness() > 0.0 && sheet.flatness() < 1.0);
        sheet.build(Some(&rig.scene()), &options);
        let held = screen_of(&sheet, grab);
        assert!(
            (held[0] - 640.0).abs() < 1.0 && (f64::from(held[1]) - y).abs() < 1.0,
            "{held:?}"
        );
        // Moving on carries it, flat or not.
        assert_eq!(
            sheet.drag_to(&rig.scene(), [700.0, 60.0], ms(120)),
            DragOutcome::FlatMoving
        );
        assert!(sheet.advance(ms(120)).animating);
        sheet.advance(ms(900));
        assert_eq!(sheet.phase(), Phase::Flat);
        assert_eq!(sheet.flatness(), 1.0);
        sheet.build(Some(&rig.scene()), &options);
        let held = screen_of(&sheet, grab);
        assert!((held[0] - 700.0).abs() < 0.01 && (held[1] - 60.0).abs() < 0.01);
        // Released over space (no lay): it stays flat, on whole pixels.
        assert_eq!(
            sheet.end_drag(&rig.scene(), [700.3, 60.4], true, ms(900)),
            DragOutcome::FlatMoving
        );
        assert_eq!(sheet.flat_origin(), [700.0 - 120.0, 60.0 - 10.0]);
        assert!(!sheet.is_dragging());
    }

    #[test]
    fn a_flat_sheet_crosses_the_planet_and_lays_down_only_when_dropped() {
        let rig = Rig::new(0.0);
        let mut sheet = Sheet::flat([240, 160], [100.0, 20.0]);
        sheet.tuning.shadow = None;
        let options = MeshOptions::default();
        sheet.advance(ms(0));
        let grab = [40.0_f32, 12.0];
        sheet.begin_drag(grab, [140.0, 32.0]);
        for (x, y) in [(300.0, 200.0), (500.0, 380.0), (640.0, 430.0)] {
            assert_eq!(
                sheet.drag_to(&rig.scene(), [x, y], ms(10)),
                DragOutcome::FlatMoving
            );
            assert_eq!(sheet.phase(), Phase::Flat, "over the planet and still flat");
        }
        // Not asked to lay: stays flat.
        let mut kept = sheet.clone();
        assert_eq!(
            kept.end_drag(&rig.scene(), [640.0, 430.0], false, ms(20)),
            DragOutcome::FlatMoving
        );
        assert_eq!(kept.phase(), Phase::Flat);
        // Asked: it lays, and the held point lands on the drop.
        assert_eq!(
            sheet.end_drag(&rig.scene(), [640.0, 430.0], true, ms(20)),
            DragOutcome::Laid
        );
        assert_eq!(sheet.phase(), Phase::Laying);
        assert!(sheet.advance(ms(100)).animating);
        sheet.build(Some(&rig.scene()), &options);
        assert!(!sheet.mesh().is_empty());
        sheet.advance(ms(400));
        assert_eq!(sheet.phase(), Phase::Surface);
        assert_eq!(sheet.flatness(), 0.0);
        sheet.advance(ms(2_000));
        sheet.build(Some(&rig.scene()), &options);
        let held = screen_of(&sheet, grab);
        assert!(
            (held[0] - 640.0).abs() < 0.5 && (held[1] - 430.0).abs() < 0.5,
            "{held:?}"
        );
    }

    #[test]
    fn a_drop_on_the_limb_or_on_space_does_not_lay() {
        let rig = Rig::new(0.0);
        let scene = rig.scene();
        // Find the horizon on the centre line: the last row that hits.
        let mut horizon = 0.0;
        for y in 0..720 {
            if pick(&scene, [640.0, f64::from(y)]).is_some() {
                horizon = f64::from(y);
                break;
            }
        }
        assert!(
            horizon > 5.0 && horizon < 400.0,
            "the limb is in frame: {horizon}"
        );
        let mut sheet = Sheet::flat([240, 160], [0.0, 0.0]);
        assert!(!sheet.lay_at(&scene, [640.0, horizon - 4.0], None), "space");
        assert!(
            !sheet.lay_at(&scene, [640.0, horizon + 1.0], None),
            "the limb"
        );
        assert_eq!(sheet.phase(), Phase::Flat);
        assert!(sheet.lay_at(&scene, [640.0, 500.0], None));
        // With no clock it is down at once.
        sheet.advance(None);
        assert_eq!(sheet.phase(), Phase::Surface);
    }

    #[test]
    fn no_cell_turns_inside_out_anywhere_through_a_peel() {
        let rig = Rig::new(0.0);
        let options = MeshOptions::default();
        for (grab, to) in [
            ([10.0_f32, 10.0], [900.0, 80.0]),
            ([230.0, 150.0], [200.0, 40.0]),
        ] {
            let mut sheet = sheet_at(&rig, [640.0, 380.0]);
            sheet.advance(ms(0));
            sheet.build(Some(&rig.scene()), &options);
            let start = screen_of(&sheet, grab);
            sheet.begin_drag(grab, [f64::from(start[0]), f64::from(start[1])]);
            sheet.drag_to(&rig.scene(), [640.0, -50.0], ms(0));
            assert_eq!(sheet.phase(), Phase::Peeling);
            sheet.drag_to(&rig.scene(), to, ms(0));
            let cells_when_flat = {
                let mut flat = sheet.clone();
                flat.advance(ms(5_000));
                flat.build(Some(&rig.scene()), &options);
                flat.mesh().indices().len()
            };
            assert_eq!(cells_when_flat, 6);
            for t in (0..=240).step_by(8) {
                sheet.advance(ms(t));
                sheet.build(Some(&rig.scene()), &options);
                let mesh = sheet.mesh();
                // Every grid cell that could be drawn was: none was dropped
                // as folded. (Cells are dropped only for facing away.)
                let grid = mesh.vertex_count();
                assert!(grid >= 4);
                let visible = (0..grid).filter(|&i| mesh.vertex(i).alpha > 0.0).count();
                assert!(visible > grid / 2, "t = {t}: {visible} of {grid}");
                for triangle in mesh.indices().chunks_exact(3) {
                    let [a, b, c] = [0, 1, 2].map(|k| mesh.vertex(triangle[k] as usize).pos);
                    let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                    assert!(area > 0.0, "t = {t}");
                }
            }
        }
    }

    #[test]
    fn hits_agree_with_the_analytic_inverse_and_respect_the_edge() {
        let rig = Rig::new(0.0);
        let mut sheet = sheet_at(&rig, [640.0, 420.0]);
        sheet.build(Some(&rig.scene()), &MeshOptions::default());
        for texel in [[30.0_f32, 30.0], [120.0, 80.0], [200.0, 140.0]] {
            let px = screen_of(&sheet, texel);
            let hit = sheet.hit(px).expect("on the sheet");
            let exact = sheet
                .to_local(Some(&rig.scene()), [f64::from(px[0]), f64::from(px[1])])
                .expect("on the planet");
            assert!((hit[0] - exact[0]).abs() < 0.3 && (hit[1] - exact[1]).abs() < 0.3);
            assert!((hit[0] - texel[0]).abs() < 0.05);
        }
        // Off the sheet: no hit, but the analytic inverse still answers, with
        // a texel outside the sheet, as a captured drag needs.
        assert!(sheet.hit([30.0, 700.0]).is_none());
        let outside = sheet.to_local(Some(&rig.scene()), [30.0, 700.0]).unwrap();
        assert!(outside[0] < 0.0);
        let flat = Sheet::flat([240, 160], [100.0, 50.0]);
        assert_eq!(flat.to_local(None, [110.0, 70.0]), Some([10.0, 20.0]));
    }

    #[test]
    fn a_sheet_behind_the_planet_or_with_no_scene_draws_nothing() {
        let rig = Rig::new(0.0);
        let options = MeshOptions::default();
        let far_side = rig.globe.frame_at(-V3::Z * R, V3::Y).unwrap();
        let mut hidden =
            Sheet::on_surface([240, 160], far_side, Extent::Surface { m_per_texel: 500.0 });
        hidden.build(Some(&rig.scene()), &options);
        assert!(hidden.mesh().is_empty());
        assert!(hidden.hit([640.0, 360.0]).is_none());

        let mut lost = sheet_at(&rig, [640.0, 420.0]);
        lost.build(None, &options);
        assert!(lost.mesh().is_empty(), "nothing to lie on");

        let mut empty = Sheet::on_surface([0, 0], far_side, Extent::Screen { scale: 1.0 });
        empty.build(Some(&rig.scene()), &options);
        assert!(empty.mesh().is_empty());

        let mut bad = sheet_at(&rig, [640.0, 420.0]);
        bad.extent = Extent::Surface {
            m_per_texel: f64::NAN,
        };
        bad.build(Some(&rig.scene()), &options);
        assert!(bad.mesh().is_empty());
        bad.extent = Extent::Surface { m_per_texel: 1.0e9 };
        bad.build(Some(&rig.scene()), &options);
        // Clamped to the surface's reach rather than wrapping the planet.
        for i in 0..bad.mesh().vertex_count() {
            assert!(bad.mesh().vertex(i).pos.iter().all(|p| p.is_finite()));
        }
    }

    #[test]
    fn hover_and_hold_lift_the_sheet_and_cast_a_shadow() {
        let rig = Rig::new(0.0);
        let mut sheet = sheet_at(&rig, [640.0, 420.0]);
        sheet.tuning.shadow = Some(ShadowStyle::default());
        let options = MeshOptions::default();
        sheet.advance(ms(0));
        sheet.build(Some(&rig.scene()), &options);
        assert!(sheet.shadow().is_none(), "resting: no shadow");
        let resting = screen_of(&sheet, [120.0, 80.0]);
        sheet.set_hovered(true);
        assert!(sheet.advance(ms(0)).animating);
        assert!(sheet.advance(ms(30)).animating);
        sheet.advance(ms(2_000));
        assert_eq!(sheet.lift(), sheet.tuning.hover_lift);
        assert!(!sheet.advance(ms(2_016)).animating);
        sheet.build(Some(&rig.scene()), &options);
        let lifted = screen_of(&sheet, [120.0, 80.0]);
        assert!(
            lifted[1] < resting[1],
            "raised toward the camera: up the screen"
        );
        let (shadow, texture, opacity) = sheet.shadow().expect("a shadow");
        assert!(!shadow.is_empty() && opacity > 0.05 && opacity < 0.2);
        assert_eq!(texture.width, (240 + 36) / 4);
        assert_eq!(
            texture.pixels[(texture.height / 2 * texture.width + texture.width / 2) as usize],
            0xff00_0000
        );
        assert_eq!(texture.pixels[0], 0);
        sheet.set_hovered(false);
        sheet.advance(None);
        assert_eq!(sheet.lift(), 0.0, "no clock: at once");
    }

    #[test]
    fn a_ripple_runs_after_a_lay_and_then_stops() {
        let rig = Rig::new(0.0);
        let mut sheet = Sheet::flat([240, 160], [500.0, 300.0]);
        sheet.tuning.shadow = None;
        let options = MeshOptions::default();
        sheet.advance(ms(0));
        assert!(sheet.lay_at(&rig.scene(), [640.0, 430.0], ms(0)));
        sheet.advance(ms(300));
        assert_eq!(sheet.phase(), Phase::Surface);
        assert!(sheet.advance(ms(400)).animating, "rippling");
        sheet.build(Some(&rig.scene()), &options);
        let lit = (0..sheet.mesh().vertex_count()).any(|i| sheet.mesh().vertex(i).light != 0.0);
        assert!(lit, "the ripple shades the sheet");
        assert!(sheet.advance(ms(450)).animating);
        assert!(sheet.build(Some(&rig.scene()), &options), "it moved");
        assert!(!sheet.advance(ms(1_000)).animating);
        sheet.build(Some(&rig.scene()), &options);
        let lit = (0..sheet.mesh().vertex_count()).any(|i| sheet.mesh().vertex(i).light != 0.0);
        assert!(!lit, "at rest");
    }

    #[test]
    fn peeling_in_place_flattens_about_the_centre() {
        let rig = Rig::new(0.0);
        let mut sheet = sheet_at(&rig, [640.0, 420.0]);
        sheet.advance(ms(0));
        sheet.peel(&rig.scene(), ms(0));
        assert_eq!(sheet.phase(), Phase::Peeling);
        sheet.advance(ms(1_000));
        assert_eq!(sheet.phase(), Phase::Flat);
        let rect = sheet.flat_rect();
        assert!((rect.x + rect.w / 2.0 - 640.0).abs() < 0.5);
        assert!((rect.y + rect.h / 2.0 - 420.0).abs() < 0.5);
        // With no clock a peel is immediate.
        let mut instant = sheet_at(&rig, [640.0, 420.0]);
        instant.peel(&rig.scene(), None);
        assert_eq!(instant.phase(), Phase::Flat);
    }

    #[test]
    fn resizing_keeps_the_centre() {
        let mut sheet = Sheet::flat([240, 160], [100.0, 50.0]);
        sheet.set_size([300, 200]);
        assert_eq!(sheet.flat_rect(), RectF::new(70.0, 30.0, 300.0, 200.0));
    }

    /// A whole globe in view from 2.5 radii out, looking down -z.
    fn globe_view() -> Pinhole {
        Pinhole {
            eye: V3::new(0.0, 0.0, R * 2.5),
            right: V3::X,
            up: V3::Y,
            back: V3::Z,
            tan_half_fov_y: (27.5_f64).to_radians().tan(),
            viewport: [1280, 720],
        }
    }

    fn assert_upright(sheet: &Sheet, label: &str) {
        let [w, h] = sheet.size();
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        // Horizontal baseline at the centre and along the top edge: +X on screen.
        let (left, right) = (
            screen_of(sheet, [cx - 40.0, cy]),
            screen_of(sheet, [cx + 40.0, cy]),
        );
        let dx = right[0] - left[0];
        let dy = right[1] - left[1];
        assert!(
            dx > 20.0,
            "{label}: centre x goes right ({left:?} -> {right:?})"
        );
        assert!(
            dy.abs() / dx < 0.02,
            "{label}: centre baseline level ({left:?} -> {right:?})"
        );
        let (top_l, top_r) = (
            screen_of(sheet, [20.0, 10.0]),
            screen_of(sheet, [w as f32 - 20.0, 10.0]),
        );
        let tdx = top_r[0] - top_l[0];
        let tdy = top_r[1] - top_l[1];
        assert!(
            tdx > 40.0,
            "{label}: top edge goes right ({top_l:?} -> {top_r:?})"
        );
        assert!(
            tdy.abs() / tdx < 0.10,
            "{label}: top edge level ({top_l:?} -> {top_r:?})"
        );
        // Vertical axis: +Y on screen (downward in device pixels).
        let (top, bottom) = (
            screen_of(sheet, [cx, cy - 30.0]),
            screen_of(sheet, [cx, cy + 30.0]),
        );
        let vdx = bottom[0] - top[0];
        let vdy = bottom[1] - top[1];
        assert!(
            vdy > 10.0,
            "{label}: vertical axis goes down ({top:?} -> {bottom:?})"
        );
        assert!(
            vdx.abs() < vdy,
            "{label}: vertical axis stays upright ({top:?} -> {bottom:?})"
        );
    }

    #[test]
    fn dragging_around_a_closed_loop_and_across_latitudes_keeps_the_sheet_upright() {
        let globe = Sphere {
            radius_m: R,
            pose: Pose::IDENTITY,
        };
        let view = globe_view();
        let scene = Scene {
            surface: &globe,
            view: &view,
            light: None,
        };
        let options = MeshOptions::default();
        let mut sheet = Sheet::flat([200, 120], [540.0, 300.0]);
        sheet.tuning.shadow = None;
        assert!(sheet.lay_at(&scene, [640.0, 360.0], None));
        sheet.advance(None);
        sheet.build(Some(&scene), &options);
        assert_upright(&sheet, "initial");

        // Grab off-centre and drag around a wide loop spanning high/low
        // latitudes and east/west longitudes, then back to the start.
        let grab = [40.0_f32, 20.0];
        let start = screen_of(&sheet, grab);
        sheet.begin_drag(grab, [f64::from(start[0]), f64::from(start[1])]);

        let waypoints = [
            [600.0, 220.0],
            [700.0, 250.0],
            [700.0, 420.0],
            [600.0, 450.0],
            [480.0, 420.0],
            [480.0, 250.0],
            [f64::from(start[0]), f64::from(start[1])],
        ];
        let mut prev = [f64::from(start[0]), f64::from(start[1])];
        for (leg, &target) in waypoints.iter().enumerate() {
            for step in 1..=12 {
                let t = f64::from(step) / 12.0;
                let cursor = [
                    prev[0] + (target[0] - prev[0]) * t,
                    prev[1] + (target[1] - prev[1]) * t,
                ];
                assert_eq!(
                    sheet.drag_to(&scene, cursor, None),
                    DragOutcome::Sliding,
                    "leg {leg} step {step}"
                );
                sheet.build(Some(&scene), &options);
                let held = screen_of(&sheet, grab);
                let miss = (f64::from(held[0]) - cursor[0]).hypot(f64::from(held[1]) - cursor[1]);
                assert!(miss < 0.5, "leg {leg} step {step}: {miss} px");
                assert_upright(&sheet, &format!("leg {leg} step {step}"));
            }
            prev = target;
        }
    }

    #[test]
    fn a_lying_sheet_stays_upright_as_the_globe_rotates_and_over_the_pole() {
        let view = globe_view();
        let options = MeshOptions::default();

        // Anchor at a mid-latitude point; rolling the globe about the view
        // axis (+Z) would spin a body-fixed tangent frame by 90 degrees.
        let initial_globe = Sphere {
            radius_m: R,
            pose: Pose::IDENTITY,
        };
        let initial_scene = Scene {
            surface: &initial_globe,
            view: &view,
            light: None,
        };
        let mut sheet = Sheet::flat([200, 120], [540.0, 300.0]);
        sheet.tuning.shadow = None;
        assert!(sheet.lay_at(&initial_scene, [720.0, 280.0], None));
        sheet.advance(None);

        for deg in [0.0_f64, 30.0, 60.0, 90.0, 135.0, -45.0] {
            let globe = Sphere {
                radius_m: R,
                pose: Pose {
                    origin: V3::ZERO,
                    rotation: M3::rodrigues(V3::Z, deg.to_radians()),
                },
            };
            let scene = Scene {
                surface: &globe,
                view: &view,
                light: None,
            };
            sheet.build(Some(&scene), &options);
            assert_upright(&sheet, &format!("roll {deg} deg"));
        }

        // Looking straight down at the north pole (V3::Y * R): no singularity
        // flip even when spinning around the pole axis.
        let pole_frame = initial_globe
            .frame_at(V3::Y * R, V3::X)
            .expect("frame at pole");
        let mut pole_sheet =
            Sheet::on_surface([200, 120], pole_frame, Extent::Screen { scale: 1.0 });
        pole_sheet.tuning.shadow = None;
        let pole_view = Pinhole {
            eye: V3::new(0.0, R * 3.5, 0.0),
            right: V3::X,
            up: -V3::Z,
            back: V3::Y,
            tan_half_fov_y: (27.5_f64).to_radians().tan(),
            viewport: [1280, 720],
        };
        for deg in [0.0_f64, 45.0, 90.0, 180.0, 270.0] {
            let globe = Sphere {
                radius_m: R,
                pose: Pose {
                    origin: V3::ZERO,
                    rotation: M3::rodrigues(V3::Y, deg.to_radians()),
                },
            };
            let scene = Scene {
                surface: &globe,
                view: &pole_view,
                light: None,
            };
            pole_sheet.build(Some(&scene), &options);
            assert_upright(&pole_sheet, &format!("pole spin {deg} deg"));
        }
    }
}
