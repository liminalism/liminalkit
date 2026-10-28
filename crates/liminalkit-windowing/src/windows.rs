//! Floating in-app windows: move, resize, raise and close.
//!
//! A [`Windows`] is a tiny window manager inside one OS window. Each window
//! is a rect with a title bar, a close button and resize handles on every
//! edge and corner; a press on the title bar moves it, a press on an edge or
//! corner resizes it, and a press anywhere raises it above its neighbours.
//! Only the topmost window under a press can grab it — chrome hidden under
//! another window's content never steals the click.
//!
//! Geometry only: [`Windows`] tracks rects, z-order and drags, and reports
//! the chrome rects for the application to draw with its own widgets. Call
//! [`Windows::update`] once a frame before drawing; it claims chrome presses
//! through [`Input::take_click`] and leaves content presses for the content.
//!
//! These are windows *within* an OS window. For several OS windows sharing
//! one event loop, see [`run_windows`](crate::run_windows).

use liminalkit_raster::Rect;
use liminalkit_shell::{Input, Scale};

use crate::host::CursorShape;

/// Identifies one floating window. Stable until the window is closed; never
/// reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(u32);

/// Window chrome behaviour, in physical pixels. See [`WindowMetrics::scaled`].
#[derive(Debug, Clone, Copy)]
pub struct WindowMetrics {
    /// Title bar height. The bar is the move handle; the close button is a
    /// square of this height at its right end.
    pub title: i32,
    /// Edge grab width, measured inside the window.
    pub grip: i32,
    /// Corner grab square size. Corners take precedence over edges.
    pub corner: i32,
    /// Minimum window width and height. Minimums win over `bounds` when the
    /// two disagree.
    pub min_w: i32,
    pub min_h: i32,
}

impl WindowMetrics {
    /// Metrics for a scale factor: a 28px logical title bar, 6px logical
    /// edges, 14px logical corners, 120×80px logical minimum.
    pub fn scaled(scale: Scale) -> WindowMetrics {
        WindowMetrics {
            title: scale.px(28),
            grip: scale.px(6),
            corner: scale.px(14),
            min_w: scale.px(120),
            min_h: scale.px(80),
        }
    }
}

impl Default for WindowMetrics {
    fn default() -> WindowMetrics {
        WindowMetrics::scaled(Scale::ONE)
    }
}

#[derive(Debug)]
struct FloatWin<T> {
    id: WindowId,
    title: String,
    rect: Rect,
    content: T,
}

/// What part of a window a point is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Title,
    Close,
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
    Content,
}

impl Zone {
    fn cursor(self) -> CursorShape {
        match self {
            Zone::North | Zone::South => CursorShape::ResizeVertical,
            Zone::East | Zone::West => CursorShape::ResizeHorizontal,
            Zone::NorthWest | Zone::SouthEast => CursorShape::ResizeDiagonalTlBr,
            Zone::NorthEast | Zone::SouthWest => CursorShape::ResizeDiagonalTrBl,
            Zone::Title | Zone::Close | Zone::Content => CursorShape::Default,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum WinDrag {
    Move {
        id: WindowId,
        /// Press position minus window position: the window keeps its offset
        /// from the cursor instead of jumping its corner to it.
        dx: i32,
        dy: i32,
    },
    Resize {
        id: WindowId,
        zone: Zone,
        /// Press position minus the moving edge's position, per axis.
        dx: i32,
        dy: i32,
    },
}

impl WinDrag {
    fn id(self) -> WindowId {
        match self {
            WinDrag::Move { id, .. } | WinDrag::Resize { id, .. } => id,
        }
    }
}

/// Floating windows over a caller-owned payload per window. Bottom-to-top in
/// [`Windows::order`]; the last one is focused. See the module docs.
pub struct Windows<T> {
    /// Bottom to top.
    order: Vec<FloatWin<T>>,
    next_id: u32,
    drag: Option<WinDrag>,
    closed: Vec<WindowId>,
}

impl<T> Windows<T> {
    /// No windows.
    pub fn new() -> Windows<T> {
        Windows {
            order: Vec::new(),
            next_id: 0,
            drag: None,
            closed: Vec::new(),
        }
    }

    /// Whether any window is open.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Open a window on top with a title, a payload and a rect. A degenerate
    /// rect is widened to 1×1 rather than opened invisible.
    pub fn open(&mut self, title: impl Into<String>, content: T, rect: Rect) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.order.push(FloatWin {
            id,
            title: title.into(),
            rect: Rect::new(rect.x, rect.y, rect.w.max(1), rect.h.max(1)),
            content,
        });
        id
    }

    /// Close a window, returning its payload. Unknown ids return `None`. A
    /// drag on the window is cancelled, and a pending close report for it is
    /// dropped — closing is idempotent.
    pub fn close(&mut self, id: WindowId) -> Option<T> {
        if self.drag.is_some_and(|drag| drag.id() == id) {
            self.drag = None;
        }
        self.closed.retain(|closed| *closed != id);
        let index = self.order.iter().position(|win| win.id == id)?;
        Some(self.order.remove(index).content)
    }

    /// The payload of one window, if it exists.
    pub fn get(&self, id: WindowId) -> Option<&T> {
        self.order
            .iter()
            .find(|win| win.id == id)
            .map(|win| &win.content)
    }

    /// The payload of one window, mutably, if it exists.
    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut T> {
        self.order
            .iter_mut()
            .find(|win| win.id == id)
            .map(|win| &mut win.content)
    }

    /// One window's rect, if it exists.
    pub fn rect(&self, id: WindowId) -> Option<Rect> {
        self.order
            .iter()
            .find(|win| win.id == id)
            .map(|win| win.rect)
    }

    /// One window's title, if it exists.
    pub fn title(&self, id: WindowId) -> Option<&str> {
        self.order
            .iter()
            .find(|win| win.id == id)
            .map(|win| win.title.as_str())
    }

    /// Rename a window. Unknown ids are ignored.
    pub fn set_title(&mut self, id: WindowId, title: impl Into<String>) {
        if let Some(win) = self.order.iter_mut().find(|win| win.id == id) {
            win.title = title.into();
        }
    }

    /// The focused window: the topmost one, if any is open.
    pub fn focused(&self) -> Option<WindowId> {
        self.order.last().map(|win| win.id)
    }

    /// Every window's id and rect, bottom to top — the order to paint them.
    pub fn order(&self) -> Vec<(WindowId, Rect)> {
        self.order.iter().map(|win| (win.id, win.rect)).collect()
    }

    /// The title bar to draw: the strip [`Windows::update`] moves the window
    /// by. `None` for an unknown window.
    pub fn title_bar(&self, id: WindowId, metrics: WindowMetrics) -> Option<Rect> {
        self.rect(id)
            .map(|rect| title_bar_rect(rect, metrics.title.max(0)))
    }

    /// The close button to draw: a square at the right end of the title bar.
    /// `None` for an unknown window.
    pub fn close_button(&self, id: WindowId, metrics: WindowMetrics) -> Option<Rect> {
        self.rect(id)
            .map(|rect| close_rect(rect, metrics.title.max(0)))
    }

    /// The rect below the title bar, where the window's content goes. `None`
    /// for an unknown window.
    pub fn content(&self, id: WindowId, metrics: WindowMetrics) -> Option<Rect> {
        self.rect(id)
            .map(|rect| content_rect(rect, metrics.title.max(0)))
    }

    /// Handle moves, resizes, raises and close presses against this frame's
    /// input. Call once a frame, before drawing.
    ///
    /// `bounds` is the region windows live in — usually the content area.
    /// Moves keep the title bar inside it; resizes keep the window inside
    /// it. Only the topmost window under a press acts on it.
    pub fn update(&mut self, input: &mut Input, bounds: Rect, metrics: WindowMetrics) {
        let metrics = WindowMetrics {
            title: metrics.title.max(0),
            grip: metrics.grip.max(0),
            corner: metrics.corner.max(0),
            min_w: metrics.min_w.max(1),
            min_h: metrics.min_h.max(1),
        };
        if let Some(drag) = self.drag {
            if !input.is_held() {
                self.drag = None;
                return;
            }
            let Some((cursor_x, cursor_y)) = input.cursor() else {
                return;
            };
            let Some(win) = self.order.iter_mut().find(|win| win.id == drag.id()) else {
                // Closed mid-drag: let go.
                self.drag = None;
                return;
            };
            match drag {
                WinDrag::Move { dx, dy, .. } => {
                    win.rect = moved_rect(
                        win.rect,
                        cursor_x - dx,
                        cursor_y - dy,
                        bounds,
                        metrics.title,
                    );
                }
                WinDrag::Resize { zone, dx, dy, .. } => {
                    win.rect = resized_rect(
                        win.rect,
                        zone,
                        (cursor_x, cursor_y),
                        (dx, dy),
                        bounds,
                        &metrics,
                    );
                }
            }
            return;
        }
        let Some((press_x, press_y)) = input.press_position() else {
            return;
        };
        let Some(index) = self
            .order
            .iter()
            .rposition(|win| win.rect.contains(press_x, press_y))
        else {
            return;
        };
        // Raise first: the press lands on the topmost window, whatever it
        // turns out to be for.
        let win = self.order.remove(index);
        let id = win.id;
        self.order.push(win);
        let rect = self.order.last().expect("just pushed").rect;
        // The press is inside this rect, so the claim below always succeeds;
        // it is written as a claim rather than assumed so a press on content
        // demonstrably survives for the content.
        match zone_at(rect, press_x, press_y, &metrics) {
            Zone::Content => {}
            Zone::Close => {
                if input.take_click(rect) {
                    self.closed.push(id);
                }
            }
            Zone::Title => {
                if input.take_click(rect) {
                    self.drag = Some(WinDrag::Move {
                        id,
                        dx: press_x - rect.x,
                        dy: press_y - rect.y,
                    });
                }
            }
            zone => {
                if input.take_click(rect) {
                    let (anchor_x, anchor_y) = zone_anchor(rect, zone, press_x, press_y);
                    self.drag = Some(WinDrag::Resize {
                        id,
                        zone,
                        dx: press_x - anchor_x,
                        dy: press_y - anchor_y,
                    });
                }
            }
        }
    }

    /// Drain one window whose close button was pressed, if any. Call in a
    /// loop and [`Windows::close`] each id it returns.
    pub fn take_closed(&mut self) -> Option<WindowId> {
        self.closed.pop()
    }

    /// The pointer shape for this frame: a resize arrow over (or while
    /// dragging) a window edge or corner, the default arrow everywhere else.
    pub fn hover_cursor(&self, input: &Input, metrics: WindowMetrics) -> CursorShape {
        if let Some(WinDrag::Resize { zone, .. }) = self.drag {
            return zone.cursor();
        }
        if self.drag.is_some() {
            return CursorShape::Default;
        }
        let metrics = WindowMetrics {
            title: metrics.title.max(0),
            grip: metrics.grip.max(0),
            corner: metrics.corner.max(0),
            min_w: metrics.min_w.max(1),
            min_h: metrics.min_h.max(1),
        };
        let Some((cursor_x, cursor_y)) = input.cursor() else {
            return CursorShape::Default;
        };
        self.order
            .iter()
            .rev()
            .find(|win| win.rect.contains(cursor_x, cursor_y))
            .map(|win| zone_at(win.rect, cursor_x, cursor_y, &metrics).cursor())
            .unwrap_or(CursorShape::Default)
    }
}

// Manual rather than derived: derive would demand `T: Default`, and a window
// payload has no business being defaultable.
#[allow(clippy::derivable_impl)]
impl<T> Default for Windows<T> {
    fn default() -> Windows<T> {
        Windows::new()
    }
}

/// The title strip: the window's full width, `title_h` tall.
fn title_bar_rect(rect: Rect, title_h: i32) -> Rect {
    Rect::new(rect.x, rect.y, rect.w, title_h.min(rect.h).max(0))
}

/// A square at the right end of the title bar.
fn close_rect(rect: Rect, title_h: i32) -> Rect {
    let height = title_h.min(rect.h).max(0);
    let side = height.min(rect.w).max(0);
    Rect::new(rect.right() - side, rect.y, side, height)
}

/// Everything below the title bar.
fn content_rect(rect: Rect, title_h: i32) -> Rect {
    rect.split_top(title_h.min(rect.h).max(0)).1
}

fn zone_at(rect: Rect, x: i32, y: i32, metrics: &WindowMetrics) -> Zone {
    if close_rect(rect, metrics.title).contains(x, y) {
        return Zone::Close;
    }
    let corner = metrics.corner.max(1);
    let in_left = x < rect.x + corner;
    let in_right = x >= rect.right() - corner;
    let in_top = y < rect.y + corner;
    let in_bottom = y >= rect.bottom() - corner;
    if in_top && in_left {
        return Zone::NorthWest;
    }
    if in_top && in_right {
        return Zone::NorthEast;
    }
    if in_bottom && in_left {
        return Zone::SouthWest;
    }
    if in_bottom && in_right {
        return Zone::SouthEast;
    }
    let grip = metrics.grip.max(1);
    if y < rect.y + grip {
        return Zone::North;
    }
    if y >= rect.bottom() - grip {
        return Zone::South;
    }
    if x < rect.x + grip {
        return Zone::West;
    }
    if x >= rect.right() - grip {
        return Zone::East;
    }
    if title_bar_rect(rect, metrics.title).contains(x, y) {
        return Zone::Title;
    }
    Zone::Content
}

/// The point of a resize zone that follows the cursor: the moving edge or
/// corner. The unused axis reports the press itself, for a zero offset.
fn zone_anchor(rect: Rect, zone: Zone, press_x: i32, press_y: i32) -> (i32, i32) {
    let x = match zone {
        Zone::West | Zone::NorthWest | Zone::SouthWest => rect.x,
        Zone::East | Zone::NorthEast | Zone::SouthEast => rect.right(),
        _ => press_x,
    };
    let y = match zone {
        Zone::North | Zone::NorthWest | Zone::NorthEast => rect.y,
        Zone::South | Zone::SouthWest | Zone::SouthEast => rect.bottom(),
        _ => press_y,
    };
    (x, y)
}

/// A moved window: the title bar — the grab handle — stays inside bounds.
fn moved_rect(rect: Rect, x: i32, y: i32, bounds: Rect, title_h: i32) -> Rect {
    let x = if rect.w >= bounds.w {
        bounds.x
    } else {
        x.clamp(bounds.x, bounds.right() - rect.w)
    };
    let title_h = title_h.max(1);
    let y = if title_h >= bounds.h {
        bounds.y
    } else {
        y.clamp(bounds.y, bounds.bottom() - title_h)
    };
    Rect::new(x, y, rect.w, rect.h)
}

/// A resized window: minimums first, then bounds, then minimums again so a
/// minimum wins when the bounds are smaller than it is.
fn resized_rect(
    rect: Rect,
    zone: Zone,
    cursor: (i32, i32),
    grab: (i32, i32),
    bounds: Rect,
    metrics: &WindowMetrics,
) -> Rect {
    let (cursor_x, cursor_y) = cursor;
    let (dx, dy) = grab;
    let moves_left = matches!(zone, Zone::West | Zone::NorthWest | Zone::SouthWest);
    let moves_top = matches!(zone, Zone::North | Zone::NorthWest | Zone::NorthEast);
    let moves_right = matches!(zone, Zone::East | Zone::NorthEast | Zone::SouthEast);
    let moves_bottom = matches!(zone, Zone::South | Zone::SouthWest | Zone::SouthEast);
    let mut x = rect.x;
    let mut y = rect.y;
    let mut right = rect.right();
    let mut bottom = rect.bottom();
    if moves_top {
        y = cursor_y - dy;
    }
    if moves_bottom {
        bottom = cursor_y - dy;
    }
    if moves_left {
        x = cursor_x - dx;
    }
    if moves_right {
        right = cursor_x - dx;
    }
    if right - x < metrics.min_w {
        if moves_left {
            x = right - metrics.min_w;
        } else {
            right = x + metrics.min_w;
        }
    }
    if bottom - y < metrics.min_h {
        if moves_top {
            y = bottom - metrics.min_h;
        } else {
            bottom = y + metrics.min_h;
        }
    }
    x = x.max(bounds.x);
    y = y.max(bounds.y);
    right = right.min(bounds.right());
    bottom = bottom.min(bounds.bottom());
    if right - x < metrics.min_w {
        if moves_left {
            x = right - metrics.min_w;
        } else {
            right = x + metrics.min_w;
        }
    }
    if bottom - y < metrics.min_h {
        if moves_top {
            y = bottom - metrics.min_h;
        } else {
            bottom = y + metrics.min_h;
        }
    }
    Rect::new(x, y, right - x, bottom - y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use liminalkit_shell::MouseButton;

    const BOUNDS: Rect = Rect {
        x: 0,
        y: 0,
        w: 800,
        h: 600,
    };
    const RECT: Rect = Rect {
        x: 100,
        y: 100,
        w: 200,
        h: 150,
    };

    fn metrics() -> WindowMetrics {
        WindowMetrics {
            title: 28,
            grip: 6,
            corner: 14,
            min_w: 120,
            min_h: 80,
        }
    }

    fn press(input: &mut Input, x: f32, y: f32) {
        input.cursor_moved(x, y);
        input.mouse(MouseButton::Left, true);
    }

    fn release(input: &mut Input) {
        input.mouse(MouseButton::Left, false);
    }

    fn ids(windows: &Windows<&str>) -> Vec<WindowId> {
        windows.order().iter().map(|(id, _)| *id).collect()
    }

    #[test]
    fn a_new_window_opens_on_top_and_takes_focus() {
        let mut windows = Windows::new();
        assert!(windows.is_empty());
        assert_eq!(windows.focused(), None);
        let a = windows.open("a", "a", RECT);
        let b = windows.open("b", "b", RECT);
        assert!(!windows.is_empty());
        assert_eq!(ids(&windows), vec![a, b], "bottom to top");
        assert_eq!(windows.focused(), Some(b));
        assert_eq!(windows.rect(b), Some(RECT));
        assert_eq!(windows.title(b), Some("b"));
    }

    #[test]
    fn pressing_a_window_raises_it_without_consuming_the_press() {
        let mut windows = Windows::new();
        let a = windows.open("a", "a", Rect::new(50, 50, 200, 150));
        let b = windows.open("b", "b", Rect::new(100, 100, 200, 150));
        let mut input = Input::new();

        // In b's content, under nothing: (280, 200) is past a's right edge.
        press(&mut input, 280.0, 200.0);
        windows.update(&mut input, BOUNDS, metrics());

        assert_eq!(ids(&windows), vec![a, b], "b was already top");
        // Now cover b and press a's visible content instead.
        let mut windows = Windows::new();
        let a = windows.open("a", "a", Rect::new(50, 50, 200, 150));
        windows.open("b", "b", Rect::new(100, 100, 200, 150));
        let mut input = Input::new();
        press(&mut input, 60.0, 180.0);
        windows.update(&mut input, BOUNDS, metrics());

        assert_eq!(windows.focused(), Some(a), "a rose to the top");
        assert_eq!(
            input.press_position(),
            Some((60, 180)),
            "a content press survives for the content"
        );
        assert!(input.take_click(Rect::new(50, 50, 200, 150)));
    }

    #[test]
    fn dragging_the_title_bar_moves_the_window_without_jumping() {
        let mut windows = Windows::new();
        let id = windows.open("win", "win", RECT);
        let mut input = Input::new();

        // 50px into the title bar: the window keeps that offset.
        press(&mut input, 150.0, 110.0);
        windows.update(&mut input, BOUNDS, metrics());
        input.end_frame();
        input.cursor_moved(400.0, 300.0);
        windows.update(&mut input, BOUNDS, metrics());

        assert_eq!(
            windows.rect(id),
            Some(Rect::new(350, 290, 200, 150)),
            "cursor minus the 50×10 grab offset"
        );

        release(&mut input);
        input.cursor_moved(500.0, 500.0);
        windows.update(&mut input, BOUNDS, metrics());
        assert_eq!(
            windows.rect(id),
            Some(Rect::new(350, 290, 200, 150)),
            "the release ended the drag"
        );
    }

    #[test]
    fn a_move_keeps_the_title_bar_inside_the_bounds() {
        let mut windows = Windows::new();
        let id = windows.open("win", "win", RECT);
        let mut input = Input::new();

        press(&mut input, 150.0, 110.0);
        windows.update(&mut input, BOUNDS, metrics());
        input.end_frame();
        input.cursor_moved(1000.0, 1000.0);
        windows.update(&mut input, BOUNDS, metrics());
        assert_eq!(
            windows.rect(id),
            Some(Rect::new(600, 572, 200, 150)),
            "pinned by the right and bottom edges"
        );

        input.cursor_moved(-100.0, -100.0);
        windows.update(&mut input, BOUNDS, metrics());
        assert_eq!(
            windows.rect(id),
            Some(Rect::new(0, 0, 200, 150)),
            "and by the top-left"
        );
    }

    #[test]
    fn edges_resize_one_axis_and_corners_resize_both() {
        // Each zone grabbed, then dragged (+20, +30): the rect delta follows
        // the zone, keeping the grab offset.
        let cases = [
            ((200.0, 102.0), Rect::new(100, 130, 200, 120)), // north
            ((200.0, 247.0), Rect::new(100, 100, 200, 180)), // south
            ((102.0, 175.0), Rect::new(120, 100, 180, 150)), // west
            ((297.0, 175.0), Rect::new(100, 100, 220, 150)), // east
            ((105.0, 105.0), Rect::new(120, 130, 180, 120)), // north-west
            ((105.0, 244.0), Rect::new(120, 100, 180, 180)), // south-west
            ((294.0, 244.0), Rect::new(100, 100, 220, 180)), // south-east
        ];
        for ((press_x, press_y), expected) in cases {
            let mut windows = Windows::new();
            let id = windows.open("win", "win", RECT);
            let mut input = Input::new();
            press(&mut input, press_x, press_y);
            windows.update(&mut input, BOUNDS, metrics());
            input.end_frame();
            input.cursor_moved(press_x + 20.0, press_y + 30.0);
            windows.update(&mut input, BOUNDS, metrics());
            assert_eq!(
                windows.rect(id),
                Some(expected),
                "press at ({press_x}, {press_y})"
            );
        }
    }

    #[test]
    fn the_close_button_wins_over_the_corner_behind_it() {
        // The close button covers the north-east corner grab: a press there
        // closes rather than resizing, since a missed close is worse than a
        // corner resized one axis at a time from its edges.
        let mut windows = Windows::new();
        let id = windows.open("win", "win", RECT);
        let mut input = Input::new();

        press(&mut input, 294.0, 105.0);
        windows.update(&mut input, BOUNDS, metrics());

        assert_eq!(windows.take_closed(), Some(id));
        assert_eq!(input.press_position(), None, "consumed");
    }

    #[test]
    fn resize_stops_at_the_minimum_size() {
        let mut windows = Windows::new();
        let id = windows.open("win", "win", RECT);
        let mut input = Input::new();

        press(&mut input, 297.0, 175.0);
        windows.update(&mut input, BOUNDS, metrics());
        input.end_frame();
        input.cursor_moved(150.0, 175.0);
        windows.update(&mut input, BOUNDS, metrics());
        assert_eq!(
            windows.rect(id),
            Some(Rect::new(100, 100, 120, 150)),
            "the 120px minimum, not 53px"
        );

        let mut windows = Windows::new();
        let id = windows.open("win", "win", RECT);
        let mut input = Input::new();
        press(&mut input, 102.0, 175.0);
        windows.update(&mut input, BOUNDS, metrics());
        input.end_frame();
        input.cursor_moved(290.0, 175.0);
        windows.update(&mut input, BOUNDS, metrics());
        assert_eq!(
            windows.rect(id),
            Some(Rect::new(180, 100, 120, 150)),
            "the moving edge stops, the fixed one stays"
        );
    }

    #[test]
    fn resize_stays_inside_the_bounds() {
        let bounds = Rect::new(0, 0, 400, 300);
        let mut windows = Windows::new();
        let id = windows.open("win", "win", RECT);
        let mut input = Input::new();

        press(&mut input, 297.0, 175.0);
        windows.update(&mut input, BOUNDS, metrics());
        input.end_frame();
        input.cursor_moved(600.0, 175.0);
        windows.update(&mut input, bounds, metrics());
        assert_eq!(windows.rect(id), Some(Rect::new(100, 100, 300, 150)));
    }

    #[test]
    fn minimums_win_when_the_bounds_are_smaller_than_them() {
        let bounds = Rect::new(0, 0, 50, 50);
        let mut windows = Windows::new();
        let id = windows.open("win", "win", RECT);
        let mut input = Input::new();

        press(&mut input, 297.0, 175.0);
        windows.update(&mut input, BOUNDS, metrics());
        input.end_frame();
        input.cursor_moved(600.0, 175.0);
        windows.update(&mut input, bounds, metrics());
        assert_eq!(
            windows.rect(id),
            Some(Rect::new(100, 100, 120, 80)),
            "120×80 past the 50×50 bounds: both minimums win"
        );
    }

    #[test]
    fn chrome_under_another_window_does_not_steal_its_press() {
        // b's title bar hides under a's content. The press raises a, claims
        // nothing, and b never moves.
        let mut windows = Windows::new();
        let b = windows.open("b", "b", Rect::new(100, 100, 200, 150));
        let a = windows.open("a", "a", Rect::new(50, 90, 200, 150));
        let mut input = Input::new();

        press(&mut input, 150.0, 120.0);
        windows.update(&mut input, BOUNDS, metrics());

        assert_eq!(windows.focused(), Some(a));
        assert_eq!(input.press_position(), Some((150, 120)));
        assert!(input.take_click(Rect::new(50, 118, 200, 122)));
        input.end_frame();
        input.cursor_moved(400.0, 400.0);
        windows.update(&mut input, BOUNDS, metrics());
        assert_eq!(
            windows.rect(b),
            Some(Rect::new(100, 100, 200, 150)),
            "b never grabbed"
        );
    }

    #[test]
    fn the_close_button_reports_and_consumes() {
        let mut windows = Windows::new();
        let id = windows.open("win", "win", RECT);
        let mut input = Input::new();
        assert_eq!(windows.take_closed(), None, "nothing yet");

        press(&mut input, 286.0, 114.0);
        windows.update(&mut input, BOUNDS, metrics());

        assert_eq!(windows.take_closed(), Some(id));
        assert_eq!(windows.take_closed(), None, "reported once");
        assert!(
            !input.take_click(RECT),
            "the press is gone for everyone else"
        );
        assert_eq!(windows.close(id), Some("win"));
        assert!(windows.is_empty());
    }

    #[test]
    fn closing_a_window_cancels_its_drag() {
        let mut windows = Windows::new();
        let id = windows.open("win", "win", RECT);
        let mut input = Input::new();

        press(&mut input, 150.0, 110.0);
        windows.update(&mut input, BOUNDS, metrics());
        assert_eq!(windows.close(id), Some("win"));
        input.end_frame();
        input.cursor_moved(400.0, 400.0);
        windows.update(&mut input, BOUNDS, metrics());
        assert!(windows.is_empty());
    }

    #[test]
    fn unknown_windows_are_left_alone() {
        let mut windows: Windows<&str> = Windows::new();
        let missing = WindowId(99);
        assert_eq!(windows.close(missing), None);
        assert_eq!(windows.rect(missing), None);
        assert_eq!(windows.title(missing), None);
        assert_eq!(windows.get(missing), None);
        assert_eq!(windows.get_mut(missing), None);
        assert_eq!(windows.title_bar(missing, metrics()), None);
        assert_eq!(windows.close_button(missing, metrics()), None);
        assert_eq!(windows.content(missing, metrics()), None);
        windows.set_title(missing, "nope");
        assert_eq!(windows.focused(), None);
        assert!(windows.order().is_empty());
    }

    #[test]
    fn chrome_geometry_matches_what_update_grabs() {
        let mut windows = Windows::new();
        let id = windows.open("win", "win", RECT);
        assert_eq!(
            windows.title_bar(id, metrics()),
            Some(Rect::new(100, 100, 200, 28))
        );
        assert_eq!(
            windows.close_button(id, metrics()),
            Some(Rect::new(272, 100, 28, 28))
        );
        assert_eq!(
            windows.content(id, metrics()),
            Some(Rect::new(100, 128, 200, 122))
        );
        // A press on each reported rect acts as that chrome.
        let mut input = Input::new();
        press(&mut input, 150.0, 110.0);
        windows.update(&mut input, BOUNDS, metrics());
        assert_eq!(
            windows.hover_cursor(&input, metrics()),
            CursorShape::Default,
            "a move drag shows the default arrow"
        );
    }

    #[test]
    fn hover_reports_zone_cursors() {
        let mut windows = Windows::new();
        windows.open("win", "win", RECT);
        let mut input = Input::new();
        for ((x, y), cursor) in [
            ((297.0, 175.0), CursorShape::ResizeHorizontal),
            ((200.0, 102.0), CursorShape::ResizeVertical),
            ((294.0, 244.0), CursorShape::ResizeDiagonalTlBr),
            ((105.0, 244.0), CursorShape::ResizeDiagonalTrBl),
            ((150.0, 110.0), CursorShape::Default),
            ((150.0, 200.0), CursorShape::Default),
            ((500.0, 500.0), CursorShape::Default),
        ] {
            input.cursor_moved(x, y);
            assert_eq!(
                windows.hover_cursor(&input, metrics()),
                cursor,
                "at ({x}, {y})"
            );
        }
        input.cursor_left();
        assert_eq!(
            windows.hover_cursor(&input, metrics()),
            CursorShape::Default
        );
    }

    #[test]
    fn a_resize_drag_keeps_its_cursor_off_the_handle() {
        let mut windows = Windows::new();
        windows.open("win", "win", RECT);
        let mut input = Input::new();
        press(&mut input, 297.0, 175.0);
        windows.update(&mut input, BOUNDS, metrics());
        input.end_frame();
        input.cursor_moved(500.0, 500.0);
        assert_eq!(
            windows.hover_cursor(&input, metrics()),
            CursorShape::ResizeHorizontal,
            "still resizing, wherever the cursor went"
        );
    }

    #[test]
    fn open_normalizes_a_degenerate_rect() {
        let mut windows = Windows::new();
        let id = windows.open("win", "win", Rect::new(5, 5, 0, -3));
        assert_eq!(windows.rect(id), Some(Rect::new(5, 5, 1, 1)));
    }

    #[test]
    fn windows_rename_and_hand_out_payloads() {
        let mut windows = Windows::new();
        let id = windows.open("old", 1, RECT);
        windows.set_title(id, "new");
        assert_eq!(windows.title(id), Some("new"));
        *windows.get_mut(id).unwrap() += 1;
        assert_eq!(windows.get(id), Some(&2));
    }
}
