//! A layer's pixels and the input that belongs to them. **EXPERIMENTAL.**
//!
//! A [`Surface`] is a small window of its own: a premultiplied buffer the
//! ordinary immediate-mode [`Ui`] paints into, and an [`Input`] that sees the
//! pointer in the buffer's own coordinates. The second half is what lets a
//! panel be drawn bent over a planet and still be clicked: by the time a
//! button asks `take_click`, the pointer has already been carried back
//! through the warp to where the button was painted, and the button never
//! knows.
//!
//! A surface is repainted only when something asks ([`Surface::invalidate`],
//! routed input, or a control that asked for another frame). Moving, fading,
//! warping and effects are all done to the painted buffer afterwards, so a
//! window sliding across a turning planet costs no painting at all.
//!
//! Each paint bumps [`Surface::revision`] and records what it touched. A GPU
//! host keeps the revision it last uploaded and asks
//! [`Surface::damage_since`] for the rectangle to upload now; the surface
//! needs no acknowledgement back, so nothing here is mutated by being drawn.

use std::time::Instant;

use liminalkit_raster::{Painter, RasterKernel, Rect, WindowBuffer};
use liminalkit_shell::{CursorShape, Input, Scale};
use liminalkit_text::TextCache;
use liminalkit_ui::{Theme, Ui};

/// What a surface is painted with: the resources every surface shares. One
/// [`TextCache`] serves them all (it is not `Send`; neither is this).
pub struct PaintContext<'a> {
    pub text: &'a mut TextCache,
    pub kernel: &'a mut RasterKernel,
    pub theme: Theme,
    pub scale: Scale,
    /// The frame's time, handed to the surface's [`Input`] so controls on it
    /// can animate. `None`: they finish at once.
    pub now: Option<Instant>,
    /// Anti-aliased rounded corners ([`Ui::set_smooth_edges`]).
    pub smooth_edges: bool,
}

/// What changed in a surface's buffer since some earlier revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Damage {
    None,
    Rect(Rect),
    /// Everything, or the buffer changed size: upload it whole.
    Full,
}

/// What counts as being on a surface, for the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitTest {
    /// Anywhere in the buffer.
    Bounds,
    /// Inside a rectangle the last paint recorded as interactive.
    Regions,
    /// Where the painted alpha is at least this: clicks pass through the
    /// transparent corners and margins.
    Alpha(u8),
}

const HISTORY: usize = 8;

/// See the [module documentation](self).
pub struct Surface {
    buffer: WindowBuffer,
    /// The pointer and keys as this surface sees them: in its own texels.
    pub input: Input,
    mips: Vec<WindowBuffer>,
    mip_levels: u8,
    margin: u32,
    revision: u64,
    /// The last few paints, newest last: which revision, what it touched.
    history: [(u64, Rect); HISTORY],
    history_len: usize,
    /// Anything asked about a revision older than this gets `Full`.
    full_since: u64,
    /// What the next paint must redo. `None` and not `stale`: nothing.
    invalid: Option<Rect>,
    stale: bool,
    regions: Vec<Rect>,
    cursor: CursorShape,
}

impl std::fmt::Debug for Surface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Surface")
            .field("size", &self.size())
            .field("margin", &self.margin)
            .field("revision", &self.revision)
            .field("mip_levels", &self.mip_levels)
            .finish_non_exhaustive()
    }
}

impl Surface {
    /// A transparent surface of `width × height` texels, waiting for its
    /// first paint.
    pub fn new(width: u32, height: u32) -> Surface {
        Surface {
            buffer: WindowBuffer::new(width, height),
            input: Input::new(),
            mips: Vec::new(),
            mip_levels: 0,
            margin: 0,
            revision: 0,
            history: [(0, Rect::default()); HISTORY],
            history_len: 0,
            full_since: 0,
            invalid: None,
            stale: true,
            regions: Vec::new(),
            cursor: CursorShape::Default,
        }
    }

    pub fn size(&self) -> [u32; 2] {
        [self.buffer.width, self.buffer.height]
    }

    /// Change size. The buffer is cleared and must be painted again.
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.size() == [width, height] {
            return;
        }
        self.buffer = WindowBuffer::new(width, height);
        self.reset();
    }

    /// Reserve `texels` of transparent border on every side: room for a glow
    /// ([`crate::effect::Effects::margin`]). The buffer keeps its size; the
    /// content rectangle shrinks.
    pub fn set_margin(&mut self, texels: u32) {
        if self.margin != texels {
            self.margin = texels;
            self.invalidate();
        }
    }

    pub fn margin(&self) -> u32 {
        self.margin
    }

    /// Where the content is painted: the buffer less its margin.
    pub fn content(&self) -> Rect {
        Rect::new(0, 0, self.buffer.width as i32, self.buffer.height as i32)
            .inset(self.margin as i32)
    }

    /// Keep `levels` smaller copies of the buffer, each half the last, for
    /// when the surface is drawn smaller than life (near a horizon). Rebuilt
    /// on every paint; a GPU host uploads them as the texture's mip levels.
    pub fn set_mip_levels(&mut self, levels: u8) {
        let levels = levels.min(8);
        if self.mip_levels != levels {
            self.mip_levels = levels;
            self.rebuild_mips();
            self.reset();
        }
    }

    /// The smaller levels, largest first.
    pub fn mips(&self) -> &[WindowBuffer] {
        &self.mips
    }

    /// The painted pixels: premultiplied `0xAARRGGBB`, sRGB-encoded.
    pub fn buffer(&self) -> &WindowBuffer {
        &self.buffer
    }

    /// Counts paints. Changes exactly when the buffer does.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The whole surface must be painted again.
    pub fn invalidate(&mut self) {
        self.stale = true;
        self.invalid = None;
    }

    /// Part of the surface must be painted again: the next paint is clipped
    /// to the union of the parts asked for.
    pub fn invalidate_rect(&mut self, rect: Rect) {
        if self.stale || rect.is_empty() {
            return;
        }
        self.invalid = Some(self.invalid.map_or(rect, |old| union(old, rect)));
    }

    /// Whether there is anything to paint: something was invalidated, or the
    /// last paint asked for another frame.
    pub fn needs_paint(&self) -> bool {
        self.stale || self.invalid.is_some() || self.input.frame_requested()
    }

    /// Paint. `draw` is given a [`Ui`] over the buffer, already cleared where
    /// it will be repainted, and a list to push interactive rectangles onto
    /// (for [`HitTest::Regions`]). Returns whatever `draw` returns.
    pub fn paint<R>(
        &mut self,
        cx: PaintContext<'_>,
        draw: impl FnOnce(&mut Ui<'_>, &mut Vec<Rect>) -> R,
    ) -> R {
        let bounds = Rect::new(0, 0, self.buffer.width as i32, self.buffer.height as i32);
        let region = match self.invalid {
            Some(part) if !self.stale && !self.input.frame_requested() => part.intersect(bounds),
            _ => bounds,
        };
        if let Some(now) = cx.now {
            self.input.begin_frame(now);
        }
        self.input.set_scale(cx.scale);
        self.regions.clear();
        let result = {
            let mut painter = Painter::new_premultiplied(&mut self.buffer);
            painter.push_clip(region);
            painter.erase_rect(region);
            let mut ui = Ui::new(
                painter,
                cx.text,
                &mut self.input,
                cx.theme,
                cx.scale,
                cx.kernel,
            )
            .with_smooth_edges(cx.smooth_edges);
            draw(&mut ui, &mut self.regions)
        };
        self.cursor = self
            .input
            .take_cursor_request()
            .unwrap_or(CursorShape::Default);
        self.input.end_frame();
        self.stale = false;
        self.invalid = None;
        self.revision += 1;
        if self.history_len == HISTORY {
            // The oldest entry falls off: revisions before it are unknown.
            self.full_since = self.history[0].0;
            self.history.rotate_left(1);
            self.history_len -= 1;
        }
        self.history[self.history_len] = (self.revision, region);
        self.history_len += 1;
        self.rebuild_mips();
        result
    }

    /// What to upload for a texture last uploaded at `revision` (`None`:
    /// never).
    pub fn damage_since(&self, revision: Option<u64>) -> Damage {
        let Some(revision) = revision else {
            return Damage::Full;
        };
        if revision >= self.revision {
            return Damage::None;
        }
        if revision < self.full_since {
            return Damage::Full;
        }
        let mut damage: Option<Rect> = None;
        for &(at, rect) in &self.history[..self.history_len] {
            if at > revision {
                damage = Some(damage.map_or(rect, |old| union(old, rect)));
            }
        }
        match damage {
            Some(rect) if !rect.is_empty() => Damage::Rect(rect),
            _ => Damage::None,
        }
    }

    /// The interactive rectangles the last paint recorded.
    pub fn regions(&self) -> &[Rect] {
        &self.regions
    }

    /// Whether texel position `local` is on the surface, by `test`.
    pub fn hit(&self, local: [f32; 2], test: HitTest) -> bool {
        let (x, y) = (local[0].floor() as i32, local[1].floor() as i32);
        let inside =
            x >= 0 && y >= 0 && x < self.buffer.width as i32 && y < self.buffer.height as i32;
        if !inside {
            return false;
        }
        match test {
            HitTest::Bounds => true,
            HitTest::Regions => self.regions.iter().any(|r| r.contains(x, y)),
            HitTest::Alpha(least) => {
                let pixel =
                    self.buffer.pixels[y as usize * self.buffer.width as usize + x as usize];
                pixel >> 24 >= u32::from(least)
            }
        }
    }

    /// The pointer shape the last paint asked for.
    pub fn cursor_shape(&self) -> CursorShape {
        self.cursor
    }

    /// The buffer was replaced: nothing a host uploaded is still right.
    fn reset(&mut self) {
        self.revision += 1;
        self.full_since = self.revision;
        self.history_len = 0;
        self.invalidate();
    }

    fn rebuild_mips(&mut self) {
        self.mips.truncate(usize::from(self.mip_levels));
        for level in 0..usize::from(self.mip_levels) {
            let (before, after) = self.mips.split_at_mut(level);
            let source = before.last().unwrap_or(&self.buffer);
            let (width, height) = (
                source.width.div_ceil(2).max(1),
                source.height.div_ceil(2).max(1),
            );
            if after.is_empty() {
                let mut half = WindowBuffer::new(width, height);
                halve(source, &mut half);
                self.mips.push(half);
            } else {
                if (after[0].width, after[0].height) != (width, height) {
                    after[0] = WindowBuffer::new(width, height);
                }
                halve(source, &mut after[0]);
            }
        }
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    Rect::new(
        x,
        y,
        a.right().max(b.right()) - x,
        a.bottom().max(b.bottom()) - y,
    )
}

/// Fill `half` with `source` averaged two by two. In premultiplied space the
/// average of valid pixels is a valid pixel, so nothing fringes; an odd edge
/// repeats its last texel.
fn halve(source: &WindowBuffer, half: &mut WindowBuffer) {
    if source.width == 0 || source.height == 0 {
        half.pixels.fill(0);
        return;
    }
    let at = |x: u32, y: u32| {
        let (x, y) = (x.min(source.width - 1), y.min(source.height - 1));
        source.pixels[(y * source.width + x) as usize]
    };
    for y in 0..half.height {
        for x in 0..half.width {
            let four = [
                at(2 * x, 2 * y),
                at(2 * x + 1, 2 * y),
                at(2 * x, 2 * y + 1),
                at(2 * x + 1, 2 * y + 1),
            ];
            let channel = |shift: u32| {
                ((four.iter().map(|p| (p >> shift) & 0xff).sum::<u32>() + 2) / 4) << shift
            };
            half.pixels[(y * half.width + x) as usize] =
                channel(24) | channel(16) | channel(8) | channel(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liminalkit_shell::MouseButton;

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

        fn cx(&mut self) -> PaintContext<'_> {
            PaintContext {
                text: &mut self.text,
                kernel: &mut self.kernel,
                theme: Theme::default(),
                scale: Scale(1.0),
                now: None,
                smooth_edges: false,
            }
        }
    }

    fn fill(surface: &mut Surface, rig: &mut Rig, color: u32) {
        surface.paint(rig.cx(), |ui, regions| {
            let all = ui.painter.bounds();
            ui.painter.fill_rect(all.inset(2), color);
            regions.push(Rect::new(2, 2, 4, 4));
        });
    }

    #[test]
    fn a_paint_bumps_the_revision_and_clears_what_it_does_not_draw() {
        let mut rig = Rig::new();
        let mut surface = Surface::new(12, 10);
        assert!(surface.needs_paint(), "never painted");
        assert_eq!(surface.damage_since(None), Damage::Full);
        fill(&mut surface, &mut rig, 0x00ff_0000);
        assert_eq!(surface.revision(), 1);
        assert!(!surface.needs_paint());
        assert_eq!(surface.buffer().pixels[0], 0, "transparent border");
        assert_eq!(
            surface.buffer().pixels[2 * 12 + 2],
            0xffff_0000,
            "premultiplied, opaque"
        );
        assert_eq!(surface.damage_since(Some(1)), Damage::None);
        assert_eq!(
            surface.damage_since(Some(0)),
            Damage::Rect(Rect::new(0, 0, 12, 10))
        );
    }

    #[test]
    fn a_part_repaint_touches_only_its_part_and_reports_only_that() {
        let mut rig = Rig::new();
        let mut surface = Surface::new(12, 10);
        fill(&mut surface, &mut rig, 0x00ff_0000);
        surface.invalidate_rect(Rect::new(4, 4, 3, 2));
        surface.invalidate_rect(Rect::new(6, 5, 2, 2));
        assert!(surface.needs_paint());
        fill(&mut surface, &mut rig, 0x0000_00ff);
        let pixel = |x: usize, y: usize| surface.buffer().pixels[y * 12 + x];
        assert_eq!(pixel(5, 5), 0xff00_00ff, "repainted");
        assert_eq!(pixel(3, 5), 0xffff_0000, "left alone");
        assert_eq!(pixel(8, 5), 0xffff_0000);
        assert_eq!(
            surface.damage_since(Some(1)),
            Damage::Rect(Rect::new(4, 4, 4, 3))
        );
        assert_eq!(
            surface.damage_since(Some(0)),
            Damage::Rect(Rect::new(0, 0, 12, 10))
        );
    }

    #[test]
    fn old_revisions_and_resizes_ask_for_everything() {
        let mut rig = Rig::new();
        let mut surface = Surface::new(8, 8);
        for _ in 0..12 {
            surface.invalidate_rect(Rect::new(1, 1, 2, 2));
            fill(&mut surface, &mut rig, 0x0000_ff00);
        }
        assert_eq!(
            surface.damage_since(Some(11)),
            Damage::Rect(Rect::new(1, 1, 2, 2))
        );
        assert_eq!(
            surface.damage_since(Some(2)),
            Damage::Full,
            "fell off the history"
        );
        let before = surface.revision();
        surface.resize(16, 8);
        assert!(surface.needs_paint());
        assert_eq!(surface.damage_since(Some(before)), Damage::Full);
        assert!(surface.buffer().pixels.iter().all(|&p| p == 0));
        surface.resize(16, 8);
        assert_eq!(surface.revision(), before + 1, "the same size is no change");
    }

    #[test]
    fn hit_tests_see_bounds_regions_and_alpha() {
        let mut rig = Rig::new();
        let mut surface = Surface::new(12, 10);
        fill(&mut surface, &mut rig, 0x00ff_ffff);
        assert!(surface.hit([0.5, 0.5], HitTest::Bounds));
        assert!(!surface.hit([-0.5, 3.0], HitTest::Bounds));
        assert!(!surface.hit([12.0, 3.0], HitTest::Bounds));
        assert!(
            !surface.hit([0.5, 0.5], HitTest::Alpha(8)),
            "the transparent border"
        );
        assert!(surface.hit([6.0, 5.0], HitTest::Alpha(8)));
        assert!(surface.hit([3.0, 3.0], HitTest::Regions));
        assert!(!surface.hit([9.0, 8.0], HitTest::Regions));
        assert_eq!(surface.regions(), &[Rect::new(2, 2, 4, 4)]);
    }

    #[test]
    fn a_control_sees_the_pointer_in_the_surfaces_own_coordinates() {
        let mut rig = Rig::new();
        let mut surface = Surface::new(120, 60);
        surface.input.cursor_moved(30.0, 20.0);
        surface.input.mouse(MouseButton::Left, true);
        surface.input.mouse(MouseButton::Left, false);
        surface.invalidate();
        let clicked = surface.paint(rig.cx(), |ui, _| ui.button(Rect::new(10, 10, 80, 30), "Go"));
        assert!(clicked);
        let again = surface.paint(rig.cx(), |ui, _| ui.button(Rect::new(10, 10, 80, 30), "Go"));
        assert!(!again, "a click is taken once");
    }

    #[test]
    fn mips_halve_by_averaging_and_follow_every_paint() {
        let mut rig = Rig::new();
        let mut surface = Surface::new(9, 6);
        surface.set_mip_levels(2);
        surface.paint(rig.cx(), |ui, _| {
            let all = ui.painter.bounds();
            ui.painter.fill_rect(all, 0x0080_4020);
        });
        let sizes: Vec<_> = surface.mips().iter().map(|m| (m.width, m.height)).collect();
        assert_eq!(sizes, [(5, 3), (3, 2)]);
        assert!(surface
            .mips()
            .iter()
            .all(|m| m.pixels.iter().all(|&p| p == 0xff80_4020)));
        // Half opaque, half clear, averages to half of everything.
        let mut source = WindowBuffer::new(2, 2);
        source.pixels = vec![0xffff_ffff, 0, 0xffff_ffff, 0];
        let mut half = WindowBuffer::new(1, 1);
        halve(&source, &mut half);
        assert_eq!(half.pixels[0], 0x8080_8080);
        surface.set_mip_levels(0);
        assert!(surface.mips().is_empty());
    }

    #[test]
    fn a_margin_shrinks_the_content_not_the_buffer() {
        let mut surface = Surface::new(40, 30);
        surface.set_margin(6);
        assert_eq!(surface.content(), Rect::new(6, 6, 28, 18));
        assert_eq!(surface.size(), [40, 30]);
    }
}
