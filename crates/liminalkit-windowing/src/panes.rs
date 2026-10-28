//! Tiled panes with draggable dividers.
//!
//! A [`Panes`] tiles a rectangle with no gaps and no overlaps: every interior
//! edge is a divider, and dragging it grows the pane on one side while the
//! neighbour on the other side shrinks by exactly as much. Splits nest, so a
//! pane can be divided again in either direction, and every divider stays
//! draggable — to expand a pane towards the bottom-left, grab its left edge
//! or its bottom edge; both are live along their whole length, corners
//! included.
//!
//! Positions are fractions, not pixels: when the area itself resizes, every
//! pane keeps its share. A drag clamps at [`PaneMetrics::min`] on both sides,
//! counting nested panes, so no pane is ever squeezed past its minimum.
//!
//! Call [`Panes::update`] once a frame before drawing. It claims divider
//! presses through [`Input::take_click`], so a grabbed divider never also
//! fires the widget underneath — and a press on pane content is left strictly
//! alone for that content to claim.

use liminalkit_raster::Rect;
use liminalkit_shell::{Input, Scale};

use crate::host::CursorShape;

/// Identifies one pane. Stable until the pane is removed; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(u32);

/// Where a new pane goes when an existing one is split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// Divider behaviour, in physical pixels. See [`PaneMetrics::scaled`].
#[derive(Debug, Clone, Copy)]
pub struct PaneMetrics {
    /// Grab half-width around a divider: the press zone extends this far on
    /// each side of the divider line.
    pub grip: i32,
    /// Minimum pane width and height. A drag stops where either side would
    /// go under this, counting every nested pane on that side.
    pub min: i32,
}

impl PaneMetrics {
    /// Metrics for a scale factor: a 5px logical grab zone and a 64px
    /// logical minimum pane.
    pub fn scaled(scale: Scale) -> PaneMetrics {
        PaneMetrics {
            grip: scale.px(5),
            min: scale.px(64),
        }
    }
}

impl Default for PaneMetrics {
    fn default() -> PaneMetrics {
        PaneMetrics::scaled(Scale::ONE)
    }
}

/// Which way a divider runs: `X` divides left from right, `Y` divides top
/// from bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

impl Axis {
    fn cursor(self) -> CursorShape {
        match self {
            Axis::X => CursorShape::ResizeHorizontal,
            Axis::Y => CursorShape::ResizeVertical,
        }
    }
}

#[derive(Debug)]
enum Node<T> {
    Pane {
        id: PaneId,
        content: T,
    },
    Split {
        axis: Axis,
        /// Share of the node's extent the first child takes.
        frac: f32,
        first: Box<Node<T>>,
        second: Box<Node<T>>,
    },
}

#[derive(Debug, Clone)]
struct PaneDrag {
    /// Path to the split node: `false` steps into `first`, `true` into
    /// `second`. Revalidated every frame, so restructuring mid-drag cancels
    /// the drag instead of moving a stranger.
    path: Vec<bool>,
    axis: Axis,
    /// Press position minus divider position along the axis, so the divider
    /// follows the cursor without jumping to it.
    grab: i32,
}

/// A divider's grab zone, found by the layout walk.
struct Band {
    path: Vec<bool>,
    axis: Axis,
    /// Divider position along the axis, in physical pixels.
    line: i32,
    band: Rect,
}

/// Tiled panes over a caller-owned payload per pane. See the module docs.
pub struct Panes<T> {
    /// Always `Some`: a tiled region with no panes has no layout, so the
    /// last pane cannot be removed. Handled gracefully anyway.
    root: Option<Node<T>>,
    next_id: u32,
    drag: Option<PaneDrag>,
}

impl<T> Panes<T> {
    /// One pane, filling whatever area it is given.
    pub fn new(content: T) -> Panes<T> {
        Panes {
            root: Some(Node::Pane {
                id: PaneId(0),
                content,
            }),
            next_id: 1,
            drag: None,
        }
    }

    /// Split `id`, putting the new pane on `side` with `fraction` of the
    /// node's extent (clamped to 0.05–0.95). Returns the new pane's id, or
    /// `None` when `id` is unknown.
    pub fn split(&mut self, id: PaneId, side: Side, content: T, fraction: f32) -> Option<PaneId> {
        let new_id = PaneId(self.next_id);
        let root = self.root.take()?;
        let mut content = Some(content);
        let (root, placed) = split_node(
            root,
            id,
            side,
            &mut content,
            fraction.clamp(0.05, 0.95),
            new_id,
        );
        self.root = Some(root);
        if placed {
            self.next_id = self.next_id.wrapping_add(1);
            Some(new_id)
        } else {
            None
        }
    }

    /// Remove a pane, returning its payload. The sibling subtree absorbs its
    /// space. Returns `None` for an unknown id — and for the last pane,
    /// which stays.
    pub fn remove(&mut self, id: PaneId) -> Option<T> {
        let root = self.root.take()?;
        if matches!(root, Node::Pane { id: pid, .. } if pid == id) {
            self.root = Some(root);
            return None;
        }
        let (root, removed) = remove_node(root, id);
        self.root = root;
        removed
    }

    /// The payload of one pane, if it exists.
    pub fn get(&self, id: PaneId) -> Option<&T> {
        self.root.as_ref().and_then(|root| find_node(root, id))
    }

    /// The payload of one pane, mutably, if it exists.
    pub fn get_mut(&mut self, id: PaneId) -> Option<&mut T> {
        self.root.as_mut().and_then(|root| find_node_mut(root, id))
    }

    /// One pane's rect within `area`, if it exists.
    pub fn rect(&self, id: PaneId, area: Rect) -> Option<Rect> {
        self.root.as_ref().and_then(|root| rect_in(root, id, area))
    }

    /// Every pane's id and rect within `area`, parents before children.
    pub fn panes(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let (panes, _) = self.collect(area, 0);
        panes
    }

    /// Handle divider drags against this frame's input. Call once a frame,
    /// before drawing, with the same `area` the rects are drawn in.
    pub fn update(&mut self, input: &mut Input, area: Rect, metrics: PaneMetrics) {
        let grip = metrics.grip.max(1);
        let min = metrics.min.max(1);
        if self.drag.is_some() {
            if !input.is_held() {
                self.drag = None;
                return;
            }
            let Some((cursor_x, cursor_y)) = input.cursor() else {
                return;
            };
            let drag = self.drag.clone().expect("checked above");
            let Some(root) = self.root.as_mut() else {
                self.drag = None;
                return;
            };
            let Some((node, rect)) = split_at_path_mut(root, area, &drag.path) else {
                // Restructured mid-drag: let go rather than move a stranger.
                self.drag = None;
                return;
            };
            let Node::Split {
                axis,
                frac,
                first,
                second,
            } = node
            else {
                self.drag = None;
                return;
            };
            if *axis != drag.axis {
                self.drag = None;
                return;
            }
            let (start, len, pos) = match axis {
                Axis::X => (rect.x, rect.w, cursor_x),
                Axis::Y => (rect.y, rect.h, cursor_y),
            };
            if len <= 0 {
                return;
            }
            let lo = subtree_min(first, *axis, min);
            let hi = len - subtree_min(second, *axis, min);
            if lo <= hi {
                let at = (pos - start - drag.grab).clamp(lo, hi);
                *frac = (at as f32 / len as f32).clamp(0.0, 1.0);
            }
            return;
        }
        let Some((press_x, press_y)) = input.press_position() else {
            return;
        };
        let (_, mut bands) = self.collect(area, grip);
        bands.retain(|band| band.band.contains(press_x, press_y));
        // At a junction two dividers overlap. The one whose line is nearer
        // the press wins, so grabbing near a corner grabs the edge the
        // cursor is actually on.
        bands.sort_by_key(|band| match band.axis {
            Axis::X => (press_x - band.line).abs(),
            Axis::Y => (press_y - band.line).abs(),
        });
        for band in &bands {
            if input.take_click(band.band) {
                let along = match band.axis {
                    Axis::X => press_x,
                    Axis::Y => press_y,
                };
                self.drag = Some(PaneDrag {
                    path: band.path.clone(),
                    axis: band.axis,
                    grab: along - band.line,
                });
                break;
            }
        }
    }

    /// The pointer shape for this frame: a resize arrow over (or while
    /// dragging) a divider, the default arrow everywhere else.
    pub fn hover_cursor(&self, input: &Input, area: Rect, metrics: PaneMetrics) -> CursorShape {
        if let Some(drag) = &self.drag {
            return drag.axis.cursor();
        }
        let Some((cursor_x, cursor_y)) = input.cursor() else {
            return CursorShape::Default;
        };
        let (_, bands) = self.collect(area, metrics.grip.max(1));
        bands
            .iter()
            .filter(|band| band.band.contains(cursor_x, cursor_y))
            .min_by_key(|band| match band.axis {
                Axis::X => (cursor_x - band.line).abs(),
                Axis::Y => (cursor_y - band.line).abs(),
            })
            .map(|band| band.axis.cursor())
            .unwrap_or(CursorShape::Default)
    }

    /// Pane rects and divider bands for an area. One walk serves
    /// [`Panes::panes`], [`Panes::update`] and [`Panes::hover_cursor`].
    fn collect(&self, area: Rect, grip: i32) -> (Vec<(PaneId, Rect)>, Vec<Band>) {
        let mut panes = Vec::new();
        let mut bands = Vec::new();
        if let Some(root) = &self.root {
            let mut path = Vec::new();
            layout(root, area, &mut path, grip, &mut panes, &mut bands);
        }
        (panes, bands)
    }
}

/// Split a node's extent into its children's rects.
fn child_rects(rect: Rect, axis: Axis, frac: f32) -> (Rect, Rect) {
    match axis {
        Axis::X => {
            let at = ((rect.w as f32 * frac).round() as i32).clamp(0, rect.w);
            (
                Rect::new(rect.x, rect.y, at, rect.h),
                Rect::new(rect.x + at, rect.y, rect.w - at, rect.h),
            )
        }
        Axis::Y => {
            let at = ((rect.h as f32 * frac).round() as i32).clamp(0, rect.h);
            (
                Rect::new(rect.x, rect.y, rect.w, at),
                Rect::new(rect.x, rect.y + at, rect.w, rect.h - at),
            )
        }
    }
}

/// The smallest extent a subtree can take along an axis: leaves take the
/// minimum, same-axis splits add up, cross-axis splits take the largest.
fn subtree_min<T>(node: &Node<T>, axis: Axis, min: i32) -> i32 {
    match node {
        Node::Pane { .. } => min,
        Node::Split {
            axis: inner,
            first,
            second,
            ..
        } => {
            let a = subtree_min(first, axis, min);
            let b = subtree_min(second, axis, min);
            if *inner == axis {
                a + b
            } else {
                a.max(b)
            }
        }
    }
}

fn find_node<T>(node: &Node<T>, id: PaneId) -> Option<&T> {
    match node {
        Node::Pane { id: pid, content } if *pid == id => Some(content),
        Node::Pane { .. } => None,
        Node::Split { first, second, .. } => find_node(first, id).or_else(|| find_node(second, id)),
    }
}

fn find_node_mut<T>(node: &mut Node<T>, id: PaneId) -> Option<&mut T> {
    match node {
        Node::Pane { id: pid, content } if *pid == id => Some(content),
        Node::Pane { .. } => None,
        Node::Split { first, second, .. } => {
            find_node_mut(first, id).or_else(|| find_node_mut(second, id))
        }
    }
}

fn rect_in<T>(node: &Node<T>, id: PaneId, rect: Rect) -> Option<Rect> {
    match node {
        Node::Pane { id: pid, .. } if *pid == id => Some(rect),
        Node::Pane { .. } => None,
        Node::Split {
            axis,
            frac,
            first,
            second,
        } => {
            let (a, b) = child_rects(rect, *axis, *frac);
            rect_in(first, id, a).or_else(|| rect_in(second, id, b))
        }
    }
}

fn layout<T>(
    node: &Node<T>,
    rect: Rect,
    path: &mut Vec<bool>,
    grip: i32,
    panes: &mut Vec<(PaneId, Rect)>,
    bands: &mut Vec<Band>,
) {
    match node {
        Node::Pane { id, .. } => panes.push((*id, rect)),
        Node::Split {
            axis,
            frac,
            first,
            second,
        } => {
            let (a, b) = child_rects(rect, *axis, *frac);
            let (line, band) = match axis {
                Axis::X => {
                    let x = a.right();
                    (x, Rect::new(x - grip, rect.y, grip * 2, rect.h))
                }
                Axis::Y => {
                    let y = a.bottom();
                    (y, Rect::new(rect.x, y - grip, rect.w, grip * 2))
                }
            };
            bands.push(Band {
                path: path.clone(),
                axis: *axis,
                line,
                band,
            });
            path.push(false);
            layout(first, a, path, grip, panes, bands);
            path.pop();
            path.push(true);
            layout(second, b, path, grip, panes, bands);
            path.pop();
        }
    }
}

/// Walk a drag path to its split node and that node's current rect.
fn split_at_path_mut<'a, T>(
    node: &'a mut Node<T>,
    area: Rect,
    path: &[bool],
) -> Option<(&'a mut Node<T>, Rect)> {
    let Some((&bit, rest)) = path.split_first() else {
        return Some((node, area));
    };
    let Node::Split {
        axis,
        frac,
        first,
        second,
    } = node
    else {
        return None;
    };
    let (a, b) = child_rects(area, *axis, *frac);
    if bit {
        split_at_path_mut(second, b, rest)
    } else {
        split_at_path_mut(first, a, rest)
    }
}

fn split_node<T>(
    node: Node<T>,
    id: PaneId,
    side: Side,
    content: &mut Option<T>,
    frac: f32,
    new_id: PaneId,
) -> (Node<T>, bool) {
    match node {
        Node::Pane {
            id: pid,
            content: old,
        } if pid == id => {
            let content = content.take().expect("a pane is placed exactly once");
            let (axis, new_first) = match side {
                Side::Left | Side::Right => (Axis::X, side == Side::Left),
                Side::Top | Side::Bottom => (Axis::Y, side == Side::Top),
            };
            let old_leaf = Node::Pane { id, content: old };
            let new_leaf = Node::Pane {
                id: new_id,
                content,
            };
            let (first, second) = if new_first {
                (Box::new(new_leaf), Box::new(old_leaf))
            } else {
                (Box::new(old_leaf), Box::new(new_leaf))
            };
            // `frac` is the new pane's share; the node stores the first
            // child's share.
            let first_frac = if new_first { frac } else { 1.0 - frac };
            (
                Node::Split {
                    axis,
                    frac: first_frac,
                    first,
                    second,
                },
                true,
            )
        }
        leaf @ Node::Pane { .. } => (leaf, false),
        Node::Split {
            axis,
            frac,
            first,
            second,
        } => {
            let (first, placed) = split_node(*first, id, side, content, frac, new_id);
            if placed {
                (
                    Node::Split {
                        axis,
                        frac,
                        first: Box::new(first),
                        second,
                    },
                    true,
                )
            } else {
                let (second, placed) = split_node(*second, id, side, content, frac, new_id);
                (
                    Node::Split {
                        axis,
                        frac,
                        first: Box::new(first),
                        second: Box::new(second),
                    },
                    placed,
                )
            }
        }
    }
}

fn remove_node<T>(node: Node<T>, id: PaneId) -> (Option<Node<T>>, Option<T>) {
    match node {
        Node::Pane { id: pid, content } if pid == id => (None, Some(content)),
        leaf @ Node::Pane { .. } => (Some(leaf), None),
        Node::Split {
            axis,
            frac,
            first,
            second,
        } => {
            let (rebuilt, removed) = remove_node(*first, id);
            if let Some(content) = removed {
                let node = match rebuilt {
                    Some(kept) => Node::Split {
                        axis,
                        frac,
                        first: Box::new(kept),
                        second,
                    },
                    None => *second,
                };
                return (Some(node), Some(content));
            }
            // Untouched subtrees rebuild: `None` only comes back with a
            // removal.
            let first = Box::new(rebuilt.expect("an untouched subtree rebuilds"));
            let (rebuilt, removed) = remove_node(*second, id);
            if let Some(content) = removed {
                let node = match rebuilt {
                    Some(kept) => Node::Split {
                        axis,
                        frac,
                        first,
                        second: Box::new(kept),
                    },
                    None => *first,
                };
                return (Some(node), Some(content));
            }
            let second = Box::new(rebuilt.expect("an untouched subtree rebuilds"));
            (
                Some(Node::Split {
                    axis,
                    frac,
                    first,
                    second,
                }),
                None,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liminalkit_shell::MouseButton;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 400,
        h: 300,
    };

    fn metrics() -> PaneMetrics {
        PaneMetrics { grip: 5, min: 40 }
    }

    fn press(input: &mut Input, x: f32, y: f32) {
        input.cursor_moved(x, y);
        input.mouse(MouseButton::Left, true);
    }

    fn release(input: &mut Input) {
        input.mouse(MouseButton::Left, false);
    }

    fn rects(panes: &Panes<&str>) -> Vec<Rect> {
        panes.panes(AREA).iter().map(|(_, rect)| *rect).collect()
    }

    #[test]
    fn a_single_pane_fills_the_area() {
        let panes = Panes::new("only");
        let all = panes.panes(AREA);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].1, AREA);
        assert_eq!(panes.rect(all[0].0, AREA), Some(AREA));
        assert_eq!(panes.get(all[0].0), Some(&"only"));
    }

    #[test]
    fn splits_tile_without_gaps_or_overlaps() {
        // Each side on a fresh layout: the two rects exactly cover the area.
        for (side, first, second) in [
            (
                Side::Right,
                Rect::new(0, 0, 200, 300),
                Rect::new(200, 0, 200, 300),
            ),
            (
                Side::Left,
                Rect::new(0, 0, 200, 300),
                Rect::new(200, 0, 200, 300),
            ),
            (
                Side::Bottom,
                Rect::new(0, 0, 400, 150),
                Rect::new(0, 150, 400, 150),
            ),
            (
                Side::Top,
                Rect::new(0, 0, 400, 150),
                Rect::new(0, 150, 400, 150),
            ),
        ] {
            let mut panes = Panes::new("old");
            let old = panes.panes(AREA)[0].0;
            let new = panes
                .split(old, side, "new", 0.5)
                .expect("a known pane splits");
            assert_eq!(rects(&panes), vec![first, second], "{side:?}");
            assert_eq!(
                panes.rect(old, AREA).unwrap().w * panes.rect(old, AREA).unwrap().h
                    + panes.rect(new, AREA).unwrap().w * panes.rect(new, AREA).unwrap().h,
                AREA.w * AREA.h,
                "no gaps, no overlaps"
            );
            // The new pane is on the side it was asked for.
            let placed = panes.rect(new, AREA).unwrap();
            match side {
                Side::Left => assert_eq!(placed.x, 0),
                Side::Right => assert_eq!(placed.right(), AREA.right()),
                Side::Top => assert_eq!(placed.y, 0),
                Side::Bottom => assert_eq!(placed.bottom(), AREA.bottom()),
            }
        }
    }

    #[test]
    fn the_new_pane_gets_the_fraction_it_was_promised() {
        let mut panes = Panes::new("old");
        let old = panes.panes(AREA)[0].0;
        let new = panes.split(old, Side::Right, "new", 0.25).unwrap();
        assert_eq!(panes.rect(new, AREA), Some(Rect::new(300, 0, 100, 300)));
        assert_eq!(panes.rect(old, AREA), Some(Rect::new(0, 0, 300, 300)));
    }

    #[test]
    fn fractions_survive_an_area_resize() {
        let mut panes = Panes::new("old");
        let old = panes.panes(AREA)[0].0;
        panes.split(old, Side::Right, "new", 0.25);
        let grown = Rect::new(0, 0, 800, 600);
        let all = panes.panes(grown);
        assert_eq!(all[0].1, Rect::new(0, 0, 600, 600));
        assert_eq!(all[1].1, Rect::new(600, 0, 200, 600));
    }

    #[test]
    fn unknown_panes_split_remove_and_read_nothing() {
        let mut panes = Panes::new("old");
        let missing = PaneId(99);
        assert_eq!(panes.split(missing, Side::Right, "x", 0.5), None);
        assert_eq!(panes.remove(missing), None);
        assert_eq!(panes.rect(missing, AREA), None);
        assert_eq!(panes.get(missing), None);
        assert_eq!(panes.get_mut(missing), None);
        assert_eq!(rects(&panes), vec![AREA], "untouched");
    }

    #[test]
    fn removing_a_pane_gives_its_space_to_its_sibling() {
        let mut panes = Panes::new("old");
        let old = panes.panes(AREA)[0].0;
        let new = panes.split(old, Side::Right, "new", 0.5).unwrap();
        assert_eq!(panes.remove(new), Some("new"));
        assert_eq!(rects(&panes), vec![AREA]);
        assert_eq!(panes.rect(old, AREA), Some(AREA));
    }

    #[test]
    fn the_last_pane_cannot_be_removed() {
        let mut panes = Panes::new("old");
        let old = panes.panes(AREA)[0].0;
        assert_eq!(panes.remove(old), None);
        assert_eq!(rects(&panes), vec![AREA], "still there");
    }

    #[test]
    fn removing_from_a_nested_split_collapses_one_level() {
        let mut panes = Panes::new("a");
        let a = panes.panes(AREA)[0].0;
        let b = panes.split(a, Side::Right, "b", 0.5).unwrap();
        let b2 = panes.split(b, Side::Bottom, "b2", 0.5).unwrap();
        assert_eq!(panes.remove(b), Some("b"));
        // The right half is b2's alone now; the left half never moved.
        assert_eq!(panes.rect(a, AREA), Some(Rect::new(0, 0, 200, 300)));
        assert_eq!(panes.rect(b2, AREA), Some(Rect::new(200, 0, 200, 300)));
    }

    #[test]
    fn payloads_are_reachable_mutably() {
        let mut panes = Panes::new(1);
        let id = panes.panes(AREA)[0].0;
        *panes.get_mut(id).unwrap() += 1;
        assert_eq!(panes.get(id), Some(&2));
    }

    #[test]
    fn a_split_clamps_wild_fractions() {
        let mut panes = Panes::new("old");
        let old = panes.panes(AREA)[0].0;
        let zero = panes.split(old, Side::Right, "z", 0.0).unwrap();
        assert_eq!(
            panes.rect(zero, AREA),
            Some(Rect::new(380, 0, 20, 300)),
            "5% of 400, not nothing"
        );
        let mut panes = Panes::new("old");
        let old = panes.panes(AREA)[0].0;
        let huge = panes.split(old, Side::Right, "h", 2.0).unwrap();
        assert_eq!(
            panes.rect(huge, AREA),
            Some(Rect::new(20, 0, 380, 300)),
            "95% of 400, not everything"
        );
    }

    #[test]
    fn dragging_a_divider_grows_one_pane_and_shrinks_the_other() {
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        let right = panes.split(left, Side::Right, "right", 0.5).unwrap();
        let mut input = Input::new();

        press(&mut input, 200.0, 150.0);
        panes.update(&mut input, AREA, metrics());
        input.end_frame();

        input.cursor_moved(250.0, 150.0);
        panes.update(&mut input, AREA, metrics());
        assert_eq!(
            panes.rect(left, AREA),
            Some(Rect::new(0, 0, 250, 300)),
            "grew by 50"
        );
        assert_eq!(
            panes.rect(right, AREA),
            Some(Rect::new(250, 0, 150, 300)),
            "shrank by exactly as much"
        );

        release(&mut input);
        input.cursor_moved(300.0, 150.0);
        panes.update(&mut input, AREA, metrics());
        assert_eq!(
            panes.rect(left, AREA),
            Some(Rect::new(0, 0, 250, 300)),
            "the release ended the drag"
        );
    }

    #[test]
    fn a_divider_grab_does_not_jump_to_the_cursor() {
        // Pressed 3px right of the line: the divider keeps that offset while
        // it follows the cursor.
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        panes.split(left, Side::Right, "right", 0.5).unwrap();
        let mut input = Input::new();

        press(&mut input, 203.0, 150.0);
        panes.update(&mut input, AREA, metrics());
        input.end_frame();
        input.cursor_moved(250.0, 150.0);
        panes.update(&mut input, AREA, metrics());

        assert_eq!(
            panes.rect(left, AREA),
            Some(Rect::new(0, 0, 247, 300)),
            "250 minus the 3px grab offset"
        );
    }

    #[test]
    fn a_divider_stops_where_either_side_hits_its_minimum() {
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        let right = panes.split(left, Side::Right, "right", 0.5).unwrap();
        let mut input = Input::new();

        press(&mut input, 200.0, 150.0);
        panes.update(&mut input, AREA, metrics());
        input.end_frame();
        input.cursor_moved(10.0, 150.0);
        panes.update(&mut input, AREA, metrics());
        assert_eq!(
            panes.rect(left, AREA),
            Some(Rect::new(0, 0, 40, 300)),
            "clamped at the 40px minimum, not dragged to 10"
        );
        assert_eq!(panes.rect(right, AREA), Some(Rect::new(40, 0, 360, 300)));

        input.cursor_moved(399.0, 150.0);
        panes.update(&mut input, AREA, metrics());
        assert_eq!(
            panes.rect(right, AREA),
            Some(Rect::new(360, 0, 40, 300)),
            "the other side clamps too"
        );
    }

    #[test]
    fn a_nested_minimum_counts_every_pane_behind_it() {
        // The right half holds two panes stacked: squeezing it from the left
        // stops where the stack — 40px wide, both panes tall — still fits.
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        let right = panes.split(left, Side::Right, "right", 0.5).unwrap();
        panes.split(right, Side::Bottom, "bottom", 0.5).unwrap();
        let mut input = Input::new();

        press(&mut input, 200.0, 150.0);
        panes.update(&mut input, AREA, metrics());
        input.end_frame();
        input.cursor_moved(390.0, 150.0);
        panes.update(&mut input, AREA, metrics());

        assert_eq!(
            panes.rect(left, AREA),
            Some(Rect::new(0, 0, 360, 300)),
            "the stacked pair kept its 40px of width"
        );

        // Side by side instead of stacked, the pair needs twice the width.
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        let right = panes.split(left, Side::Right, "right", 0.5).unwrap();
        panes.split(right, Side::Right, "far", 0.5).unwrap();
        let mut input = Input::new();

        press(&mut input, 200.0, 150.0);
        panes.update(&mut input, AREA, metrics());
        input.end_frame();
        input.cursor_moved(390.0, 150.0);
        panes.update(&mut input, AREA, metrics());

        assert_eq!(
            panes.rect(left, AREA),
            Some(Rect::new(0, 0, 320, 300)),
            "two minima along the drag axis"
        );
    }

    #[test]
    fn a_drag_survives_leaving_the_grab_zone() {
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        panes.split(left, Side::Right, "right", 0.5).unwrap();
        let mut input = Input::new();

        press(&mut input, 200.0, 150.0);
        panes.update(&mut input, AREA, metrics());
        input.end_frame();
        // Far from the 10px grab band, but the button is still down.
        input.cursor_moved(350.0, 290.0);
        panes.update(&mut input, AREA, metrics());

        assert_eq!(panes.rect(left, AREA), Some(Rect::new(0, 0, 350, 300)));
    }

    #[test]
    fn a_grabbed_divider_consumes_its_press() {
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        panes.split(left, Side::Right, "right", 0.5).unwrap();
        let mut input = Input::new();

        press(&mut input, 200.0, 150.0);
        panes.update(&mut input, AREA, metrics());

        assert_eq!(input.press_position(), None, "claimed, not peeked");
        assert!(
            !input.take_click(Rect::new(195, 0, 10, 300)),
            "no widget under the divider fires"
        );
    }

    #[test]
    fn a_press_on_content_is_left_for_the_content() {
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        panes.split(left, Side::Right, "right", 0.5).unwrap();
        let mut input = Input::new();

        press(&mut input, 50.0, 50.0);
        panes.update(&mut input, AREA, metrics());

        assert_eq!(input.press_position(), Some((50, 50)));
        assert!(input.take_click(Rect::new(0, 0, 100, 100)));
    }

    #[test]
    fn hovering_reports_each_dividers_axis() {
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        let right = panes.split(left, Side::Right, "right", 0.5).unwrap();
        panes.split(right, Side::Bottom, "bottom", 0.5).unwrap();
        let mut input = Input::new();

        input.cursor_moved(200.0, 40.0);
        assert_eq!(
            panes.hover_cursor(&input, AREA, metrics()),
            CursorShape::ResizeHorizontal,
            "the vertical divider"
        );
        input.cursor_moved(300.0, 150.0);
        assert_eq!(
            panes.hover_cursor(&input, AREA, metrics()),
            CursorShape::ResizeVertical,
            "the horizontal divider in the right half"
        );
        input.cursor_moved(50.0, 50.0);
        assert_eq!(
            panes.hover_cursor(&input, AREA, metrics()),
            CursorShape::Default,
            "pane content"
        );
        input.cursor_left();
        assert_eq!(
            panes.hover_cursor(&input, AREA, metrics()),
            CursorShape::Default,
            "no cursor"
        );
    }

    #[test]
    fn at_a_junction_the_nearer_divider_wins() {
        // A + junction at (200, 150). Pressed 3px right of the vertical line
        // and 4px below the horizontal one: the vertical divider grabs.
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        let right = panes.split(left, Side::Right, "right", 0.5).unwrap();
        let bottom = panes.split(right, Side::Bottom, "bottom", 0.5).unwrap();
        let mut input = Input::new();
        input.cursor_moved(203.0, 154.0);
        assert_eq!(
            panes.hover_cursor(&input, AREA, metrics()),
            CursorShape::ResizeHorizontal,
            "hover agrees with the grab"
        );

        press(&mut input, 203.0, 154.0);
        panes.update(&mut input, AREA, metrics());
        input.end_frame();
        input.cursor_moved(260.0, 154.0);
        panes.update(&mut input, AREA, metrics());

        assert_eq!(
            panes.rect(left, AREA),
            Some(Rect::new(0, 0, 257, 300)),
            "the vertical divider moved, keeping the 3px grab"
        );
        assert_eq!(
            panes.rect(bottom, AREA).unwrap().h,
            150,
            "the horizontal divider never moved"
        );
    }

    #[test]
    fn restructuring_mid_drag_cancels_the_drag() {
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        let right = panes.split(left, Side::Right, "right", 0.5).unwrap();
        let mut input = Input::new();

        press(&mut input, 200.0, 150.0);
        panes.update(&mut input, AREA, metrics());
        input.end_frame();
        panes.remove(right);
        input.cursor_moved(300.0, 150.0);
        panes.update(&mut input, AREA, metrics());

        assert_eq!(rects(&panes), vec![AREA]);
        assert_eq!(
            panes.hover_cursor(&input, AREA, metrics()),
            CursorShape::Default,
            "nothing left to drag"
        );
    }

    #[test]
    fn a_degenerate_area_does_not_panic() {
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        panes.split(left, Side::Right, "right", 0.5).unwrap();
        let mut input = Input::new();
        let empty = Rect::new(0, 0, 0, 0);

        press(&mut input, 0.0, 0.0);
        panes.update(&mut input, empty, metrics());
        assert_eq!(
            panes.hover_cursor(&input, empty, metrics()),
            CursorShape::Default
        );
        for (_, rect) in panes.panes(empty) {
            assert!(rect.w <= 0 || rect.h <= 0);
        }
        // A drag in progress while the area collapses holds its fraction.
        let mut panes = Panes::new("left");
        let left = panes.panes(AREA)[0].0;
        panes.split(left, Side::Right, "right", 0.5).unwrap();
        press(&mut input, 200.0, 150.0);
        panes.update(&mut input, AREA, metrics());
        input.end_frame();
        input.cursor_moved(250.0, 150.0);
        panes.update(&mut input, empty, metrics());
        assert_eq!(
            rects(&panes),
            vec![Rect::new(0, 0, 200, 300), Rect::new(200, 0, 200, 300)]
        );
    }
}
