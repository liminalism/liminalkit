//! A scroll region's state: where it is, and how it gets there.
//!
//! [`ScrollState`] is the ordinary field a screen keeps for anything that
//! scrolls. It holds a position on two axes and, since a position that only
//! ever jumped was hard to follow, the means of moving it: the wheel
//! interpreter that decides how far a wheel movement goes, and a motion
//! controller per axis that decides how it looks arriving. A control calls
//! [`ScrollState::update`] once a frame and draws at the offset it is given.
//!
//! Three things are worth knowing.
//!
//! **The offset is whole pixels, the position is not.** Text and images are
//! drawn at whole device pixels and a click is tested against what was
//! drawn, so [`ScrollState::offset`] is an integer, and it is the same
//! integer for the whole of a frame. The position underneath is fractional,
//! which is how a touchpad moving a third of a pixel at a time gets anywhere.
//!
//! **It only moves over time if it is told the time.** With a clock
//! ([`Input::begin_frame`]) a wheel notch eases in over a few frames and the
//! region asks for those frames itself. Without one there is no later frame
//! to finish in, so everything arrives at once; that is also what makes a
//! test that never mentions time behave exactly as it always did.
//!
//! **A press stops it.** Whatever is under the pointer when the button goes
//! down stays under it, so a click during a scroll lands on what was seen.
//!
//! EXPERIMENTAL: the smoothing and the wheel profile are starting points.

use std::time::Duration;

use liminalkit_raster::Rect;
use liminalkit_shell::scroll::{
    host_time, InputClass, Interpreted, MotionController, Smoothing, Units, WheelInterpreter,
    WheelProfile,
};
use liminalkit_shell::{CanScroll, CursorShape, Input, MouseButton, ScrollBoundary};

/// What the middle mouse button does in a scroll region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MiddleButton {
    /// Nothing; the press is left for the application.
    Off,
    /// Hold and drag to move the content with the pointer.
    #[default]
    Pan,
    /// Click to drop an anchor; the pointer's distance from it sets a
    /// steady scroll, until the next click.
    Autoscroll,
    /// A click starts autoscroll; holding and dragging pans.
    Both,
}

/// How a region responds to input. Kept in the [`ScrollState`], so that the
/// controls built on it need no extra arguments.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollTuning {
    /// How far a wheel movement goes.
    pub wheel: WheelProfile,
    /// How a wheel notch is shown arriving.
    pub notch: Smoothing,
    /// How continuous wheel input (a touchpad reported as a wheel) is shown
    /// arriving. It comes about fifty times a second, slower than most
    /// displays draw, so a little easing fills the frames in between.
    pub fine: Smoothing,
    /// The share of a notch shown at once, 0 to 1.
    pub lead: f64,
    /// What happens to the wheel at the end of a region inside another.
    pub boundary: ScrollBoundary,
    pub middle: MiddleButton,
    /// Whether the left button drags the content, as a hand tool does. The
    /// region then takes every left press on it, so nothing drawn inside it
    /// can be clicked: for a page or a canvas, not for a list of buttons.
    pub drag_pan: bool,
    /// Whether content let go of while it is moving carries on and coasts
    /// to a stop. Off by default: with a mouse, a drag that ends is over.
    pub fling: Option<Fling>,
    /// How long a page key or a click in the scrollbar's track takes.
    pub page_time: Duration,
}

/// How content coasts after a drag lets go of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fling {
    /// How quickly it slows: the rate at which speed is lost. 2 glides a
    /// long way, 10 stops short.
    pub friction: f64,
    /// Slower than this, in logical pixels a second, and letting go just
    /// stops.
    pub min_speed: f64,
    /// The fastest a fling starts, in logical pixels a second.
    pub max_speed: f64,
}

impl Default for Fling {
    fn default() -> Fling {
        Fling {
            friction: 5.0,
            min_speed: 100.0,
            max_speed: 6_000.0,
        }
    }
}

impl Default for ScrollTuning {
    fn default() -> ScrollTuning {
        ScrollTuning {
            wheel: WheelProfile::natural(),
            notch: Smoothing::Exponential {
                time: Duration::from_millis(40),
            },
            fine: Smoothing::Exponential {
                time: Duration::from_millis(12),
            },
            lead: 0.2,
            boundary: ScrollBoundary::Auto,
            middle: MiddleButton::Pan,
            drag_pan: false,
            fling: None,
            page_time: Duration::from_millis(140),
        }
    }
}

/// Which way a scrollbar runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    fn index(self) -> usize {
        match self {
            Axis::Horizontal => 0,
            Axis::Vertical => 1,
        }
    }

    fn of(self, point: (i32, i32)) -> i32 {
        match self {
            Axis::Horizontal => point.0,
            Axis::Vertical => point.1,
        }
    }
}

/// Something the pointer has hold of, from one frame to the next.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Grab {
    #[default]
    None,
    /// The scrollbar thumb, held `offset` pixels from its leading edge.
    Thumb { axis: Axis, offset: i32 },
    /// The scrollbar track, paging towards the pointer while held.
    Track { axis: Axis, next: Duration },
    /// The content, under `button`. `moved` once it has been dragged far
    /// enough to be a pan and not a click; `velocity` is how fast it has
    /// lately been going, for letting go, and `moved_at` when it last went
    /// anywhere.
    Pan {
        button: MouseButton,
        origin: (i32, i32),
        last: (i32, i32),
        since: Duration,
        moved: bool,
        velocity: (f64, f64),
        at: Duration,
        moved_at: Duration,
    },
    /// Scrolling steadily, at a speed set by the pointer's distance from
    /// `anchor`. `at` is when it was last moved along.
    Auto { anchor: (i32, i32), at: Duration },
}

/// A scrollbar's thumb this frame, for drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollThumb {
    pub rect: Rect,
    pub hovered: bool,
    /// The pointer has hold of it.
    pub held: bool,
}

/// What a region should draw this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollView {
    /// Pixels hidden to the left of and above the viewport. Draw content at
    /// its own position minus this, and test clicks against what was drawn.
    pub offset: (i32, i32),
    /// The offset changed since the last frame.
    pub moved: bool,
    /// Still moving: another frame has been asked for.
    pub animating: bool,
}

/// How far a region has been scrolled, and how far it may be.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScrollState {
    /// Horizontal, then vertical.
    motion: [MotionController; 2],
    wheel: WheelInterpreter,
    tuning: ScrollTuning,
    /// The time of the last frame that had one.
    clock: Duration,
    grab: Grab,
}

fn button_held(input: &Input, button: MouseButton) -> bool {
    match button {
        MouseButton::Left => input.is_held(),
        MouseButton::Middle => input.is_middle_held(),
        MouseButton::Right => false,
    }
}

fn whole(position: f64) -> i32 {
    (position + 0.5)
        .floor()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

/// How far from its start a middle press may move and still be a click.
const CLICK_SLOP: i32 = 4;
/// How long a middle press may last and still be a click.
const CLICK_TIME: Duration = Duration::from_millis(400);
/// Around an autoscroll anchor, the distance within which nothing moves.
const AUTOSCROLL_DEAD_ZONE: f64 = 12.0;
/// The fastest autoscroll goes, in logical pixels a second.
const AUTOSCROLL_LIMIT: f64 = 3_000.0;
/// A held track pages again after this long, then at the shorter interval.
const TRACK_DELAY: Duration = Duration::from_millis(400);
const TRACK_REPEAT: Duration = Duration::from_millis(60);

impl ScrollState {
    pub fn new() -> ScrollState {
        ScrollState::default()
    }

    /// A region that responds as `tuning` says.
    pub fn with_tuning(tuning: ScrollTuning) -> ScrollState {
        let mut state = ScrollState::default();
        state.set_tuning(tuning);
        state
    }

    pub fn tuning(&self) -> &ScrollTuning {
        &self.tuning
    }

    pub fn set_tuning(&mut self, tuning: ScrollTuning) {
        self.tuning = tuning;
        self.wheel.set_profile(tuning.wheel);
    }

    /// Pixels hidden above the top of the viewport.
    pub fn offset(&self) -> i32 {
        whole(self.motion[1].position())
    }

    /// Pixels hidden to the left of the viewport.
    pub fn offset_x(&self) -> i32 {
        whole(self.motion[0].position())
    }

    /// Where the vertical offset is heading; equal to it at rest.
    pub fn target(&self) -> i32 {
        whole(self.motion[1].target())
    }

    /// Whether the region is still moving.
    pub fn is_animating(&self) -> bool {
        self.motion.iter().any(MotionController::animating)
            || matches!(self.grab, Grab::Auto { .. } | Grab::Track { .. })
    }

    /// The furthest this can scroll given a viewport and its content.
    ///
    /// Zero when everything fits, so a short list cannot be scrolled at all —
    /// content that drifts off the top and cannot be brought back is the most
    /// common bug in a hand-written scroll region.
    pub fn max_offset(viewport: i32, content: i32) -> i32 {
        (content - viewport).max(0)
    }

    fn bound(&mut self, axis: usize, viewport: i32, content: i32) {
        self.motion[axis].set_bounds(0.0, f64::from(ScrollState::max_offset(viewport, content)));
    }

    /// Move by a delta, at once, and clamp. Returns the offset actually
    /// applied.
    pub fn scroll_by(&mut self, delta: i32, viewport: i32, content: i32) -> i32 {
        let before = self.offset();
        self.bound(1, viewport, content);
        self.wheel.interrupt();
        self.motion[1].jump_to(self.clock, f64::from(before) + f64::from(delta));
        self.offset() - before
    }

    /// Move by a fractional distance on both axes, in device pixels, at
    /// once, and clamp. Positive moves the view right and down. Returns the
    /// whole pixels the offsets changed by.
    ///
    /// The fraction that does not make a pixel yet is kept, so ten moves of
    /// 0.3 px are three pixels, where rounding each one would make them
    /// none. Movement refused at an end is dropped rather than kept, or
    /// scrolling back would have to use it up first.
    pub fn scroll_by_precise(
        &mut self,
        dx: f64,
        dy: f64,
        viewport: (i32, i32),
        content: (i32, i32),
    ) -> (i32, i32) {
        let before = (self.offset_x(), self.offset());
        self.bound(0, viewport.0, content.0);
        self.bound(1, viewport.1, content.1);
        self.motion[0].nudge(self.clock, dx);
        self.motion[1].nudge(self.clock, dy);
        (self.offset_x() - before.0, self.offset() - before.1)
    }

    /// Bring a band of content into view, at once, moving as little as
    /// possible. Measured from where the view is heading, so that revealing
    /// row after row while a scroll is in progress does not fight it.
    pub fn reveal(&mut self, top: i32, height: i32, viewport: i32, content: i32) {
        self.bound(1, viewport, content);
        let current = self.target();
        let wanted = if top < current {
            top
        } else if top + height > current + viewport {
            top + height - viewport
        } else {
            return;
        };
        self.wheel.interrupt();
        self.motion[1].jump_to(self.clock, f64::from(wanted));
    }

    /// As [`reveal`](Self::reveal), but the view travels there over the
    /// tuning's page time when there is a clock to travel by.
    pub fn reveal_animated(&mut self, top: i32, height: i32, viewport: i32, content: i32) {
        self.bound(1, viewport, content);
        let current = self.target();
        let wanted = if top < current {
            top
        } else if top + height > current + viewport {
            top + height - viewport
        } else {
            return;
        };
        self.wheel.interrupt();
        self.motion[1].animate_to(self.clock, f64::from(wanted), self.tuning.page_time);
    }

    /// Go to an offset over the tuning's page time: a page key, a jump to a
    /// search result. Call [`update`](Self::update) afterwards as usual; it
    /// is what moves the view along.
    pub fn animate_to(&mut self, axis: Axis, offset: i32) {
        self.wheel.interrupt();
        self.motion[axis.index()].animate_to(self.clock, f64::from(offset), self.tuning.page_time);
    }

    /// Go to an offset at once.
    pub fn jump_to(&mut self, axis: Axis, offset: i32) {
        self.wheel.interrupt();
        self.motion[axis.index()].jump_to(self.clock, f64::from(offset));
    }

    /// Clamp after the content changed underneath — a filter applied, a day
    /// with fewer rows. Without this a list scrolled to the bottom shows
    /// nothing at all when it gets shorter.
    pub fn clamp_to(&mut self, viewport: i32, content: i32) {
        self.bound(1, viewport, content);
    }

    /// One frame of a scroll region: take the input that belongs to it, move
    /// along, and say where to draw.
    ///
    /// `area` is what the pointer has to be over; `viewport` and `content`
    /// are sizes in device pixels, `(width, height)`; `line` is what one
    /// wheel line is worth on each axis (a table passes its row height).
    /// Call it once a frame, before drawing the content, and after any
    /// [`scrollbar`](Self::scrollbar) so that a press on the bar is the
    /// bar's.
    pub fn update(
        &mut self,
        input: &mut Input,
        area: Rect,
        viewport: (i32, i32),
        content: (i32, i32),
        line: (f64, f64),
    ) -> ScrollView {
        let before = (self.offset_x(), self.offset());
        self.bound(0, viewport.0, content.0);
        self.bound(1, viewport.1, content.1);
        let timed = input.now().map(host_time);
        if let Some(now) = timed {
            self.clock = self.clock.max(now);
        }
        let scale = f64::from(input.scale().0);

        // A press on the content stops it where it is, so the click lands on
        // what was on screen. It also ends a steady autoscroll.
        if input
            .press_position()
            .is_some_and(|(x, y)| area.contains(x, y))
        {
            self.halt();
        }
        self.pointer_pan(input, area, scale);

        if let Some((dx, dy)) = input.take_returned_scroll(area) {
            self.motion[0].nudge(self.clock, dx);
            self.motion[1].nudge(self.clock, dy);
        }

        let can = CanScroll {
            up: self.motion[1].target() > 0.0,
            down: self.motion[1].target() < self.motion[1].bounds().1,
            left: self.motion[0].target() > 0.0,
            right: self.motion[0].target() < self.motion[0].bounds().1,
        };
        let units = Units {
            line,
            page: (f64::from(viewport.0), f64::from(viewport.1)),
            pixel: 1.0,
            settings: *input.scroll_settings(),
        };
        let tuning = self.tuning;
        let clock = self.clock;
        let (wheel, motion) = (&mut self.wheel, &mut self.motion);
        let unused = input.route_wheel(area, can, tuning.boundary, |samples| {
            let mut unused = (0.0, 0.0);
            for sample in samples {
                let Interpreted::Scroll(scroll) = wheel.interpret(sample, &units) else {
                    continue;
                };
                // A sample stamped before the clock (a test's, or one from a
                // host with no clock of its own) is applied now.
                let at = sample.at.max(clock);
                for (axis, asked) in [scroll.displacement.0, scroll.displacement.1]
                    .into_iter()
                    .enumerate()
                {
                    let took = match scroll.class {
                        InputClass::Notched => {
                            motion[axis].wheel(at, asked, tuning.notch, tuning.lead)
                        }
                        InputClass::Fine => motion[axis].wheel(at, asked, tuning.fine, 0.0),
                        InputClass::Precise => motion[axis].nudge(at, asked),
                    };
                    if axis == 0 {
                        unused.0 += asked - took;
                    } else {
                        unused.1 += asked - took;
                    }
                }
            }
            unused
        });
        if let (Some(unused), ScrollBoundary::Chain) = (unused, tuning.boundary) {
            input.return_scroll(area, unused);
        }

        if timed.is_some() {
            for motion in &mut self.motion {
                motion.advance(self.clock);
            }
        } else {
            // No clock, so no later frame to arrive in.
            for motion in &mut self.motion {
                motion.settle();
            }
        }
        let animating = self.is_animating();
        if animating {
            input.request_frame();
        }
        let offset = (self.offset_x(), self.offset());
        ScrollView {
            offset,
            moved: offset != before,
            animating,
        }
    }

    /// Stop where it is, and let go of anything the pointer was doing.
    pub fn halt(&mut self) {
        for motion in &mut self.motion {
            motion.halt();
        }
        if matches!(self.grab, Grab::Auto { .. }) {
            self.grab = Grab::None;
        }
        self.wheel.interrupt();
    }

    /// Dragging the content with a button: the middle one, as its mode
    /// says, and the left one when the region is a hand tool.
    fn pointer_pan(&mut self, input: &mut Input, area: Rect, scale: f64) {
        let mode = self.tuning.middle;
        let middle_pans = matches!(mode, MiddleButton::Pan | MiddleButton::Both);
        let autoscrolls = matches!(mode, MiddleButton::Autoscroll | MiddleButton::Both);
        let Some(cursor) = input.cursor() else {
            if let Grab::Pan { button, .. } = self.grab {
                if !button_held(input, button) {
                    self.grab = Grab::None;
                }
            }
            return;
        };
        let clock = self.clock;
        let begin = move |button| Grab::Pan {
            button,
            origin: cursor,
            last: cursor,
            since: clock,
            moved: false,
            velocity: (0.0, 0.0),
            at: clock,
            moved_at: clock,
        };
        if mode != MiddleButton::Off && input.take_middle_click(area) {
            // A second click ends an autoscroll; otherwise this starts
            // whatever the middle button does here.
            self.grab = if matches!(self.grab, Grab::Auto { .. }) {
                Grab::None
            } else {
                let grab = begin(MouseButton::Middle);
                self.catch();
                grab
            };
        }
        if self.tuning.drag_pan {
            if input.take_click(area) {
                let grab = begin(MouseButton::Left);
                self.catch();
                self.grab = grab;
            }
            if input.hovering(area) && self.grab == Grab::None {
                input.request_cursor(CursorShape::Grab);
            }
        }
        match self.grab {
            Grab::Pan {
                button,
                origin,
                last,
                since,
                moved,
                velocity,
                at,
                moved_at,
            } => {
                let pans = button == MouseButton::Left || middle_pans;
                let slop = (f64::from(CLICK_SLOP) * scale).round() as i32;
                let moved = moved
                    || (cursor.0 - origin.0).abs() > slop
                    || (cursor.1 - origin.1).abs() > slop;
                if button_held(input, button) {
                    let mut velocity = velocity;
                    let mut moved_at = moved_at;
                    if pans && moved {
                        // The content follows the pointer: dragging down
                        // brings what was above into view.
                        let step = (f64::from(last.0 - cursor.0), f64::from(last.1 - cursor.1));
                        self.motion[0].nudge(self.clock, step.0);
                        self.motion[1].nudge(self.clock, step.1);
                        input.request_cursor(CursorShape::Grabbing);
                        // A running average of the speed, weighted to the
                        // last sixty milliseconds: what the hand was doing
                        // as it let go, not at the start of the drag.
                        let elapsed = self.clock.saturating_sub(at).as_secs_f64();
                        if elapsed > 0.0 {
                            let weight = 1.0 - (-elapsed / 0.06).exp();
                            velocity.0 += weight * (step.0 / elapsed - velocity.0);
                            velocity.1 += weight * (step.1 / elapsed - velocity.1);
                        }
                        if step != (0.0, 0.0) {
                            moved_at = self.clock;
                        }
                    }
                    self.grab = Grab::Pan {
                        button,
                        origin,
                        last: cursor,
                        since,
                        moved,
                        velocity,
                        at: self.clock,
                        moved_at,
                    };
                } else {
                    let clicked = !moved && self.clock.saturating_sub(since) <= CLICK_TIME;
                    let middle = button == MouseButton::Middle;
                    self.grab = if middle && autoscrolls && (clicked || !middle_pans) {
                        Grab::Auto {
                            anchor: origin,
                            at: self.clock,
                        }
                    } else {
                        Grab::None
                    };
                    // Let go while moving: carry on, if the region flings.
                    // A hand that had stopped before letting go has stopped.
                    let recent = self.clock.saturating_sub(moved_at) <= Duration::from_millis(60);
                    if let (Some(fling), true, true) = (self.tuning.fling, pans && moved, recent) {
                        let limit = fling.max_speed * scale;
                        for (motion, speed) in self.motion.iter_mut().zip([velocity.0, velocity.1])
                        {
                            if speed.abs() >= fling.min_speed * scale {
                                motion.fling(
                                    self.clock,
                                    speed.clamp(-limit, limit),
                                    fling.friction,
                                );
                            }
                        }
                    }
                }
            }
            Grab::Auto { anchor, at } => {
                let elapsed = self.clock.saturating_sub(at).as_secs_f64();
                let speed = |offset: i32| {
                    let dead = AUTOSCROLL_DEAD_ZONE * scale;
                    let past = (f64::from(offset).abs() - dead).max(0.0) / dead;
                    f64::from(offset.signum())
                        * (past * past * 6.0 * dead).min(AUTOSCROLL_LIMIT * scale)
                };
                self.motion[0].nudge(self.clock, speed(cursor.0 - anchor.0) * elapsed);
                self.motion[1].nudge(self.clock, speed(cursor.1 - anchor.1) * elapsed);
                self.grab = Grab::Auto {
                    anchor,
                    at: self.clock,
                };
                if input
                    .released()
                    .is_some_and(|(button, _)| button != MouseButton::Middle)
                {
                    self.grab = Grab::None;
                }
            }
            _ => {}
        }
    }

    /// Take hold of the content: whatever it was doing, it stops and waits
    /// for the pointer.
    fn catch(&mut self) {
        for motion in &mut self.motion {
            motion.halt();
        }
        self.wheel.interrupt();
    }

    /// Where an autoscroll's anchor is, while one is running, for drawing a
    /// mark there.
    pub fn autoscroll_anchor(&self) -> Option<(i32, i32)> {
        match self.grab {
            Grab::Auto { anchor, .. } => Some(anchor),
            _ => None,
        }
    }

    /// One frame of a scrollbar along `track`: let the pointer drag the
    /// thumb or page by clicking beside it, and say where the thumb is.
    ///
    /// Call it before [`update`](Self::update). `viewport` and `content` are
    /// lengths along the bar's axis in device pixels. Returns `None` when
    /// everything fits and there is no thumb.
    ///
    /// Dragging the thumb puts the view exactly where the thumb is, with no
    /// easing and no acceleration, and goes on following the pointer when it
    /// strays off the bar or out of the window. A press beside the thumb
    /// pages towards the pointer, and goes on paging while it is held until
    /// the thumb reaches the pointer.
    pub fn scrollbar(
        &mut self,
        input: &mut Input,
        track: Rect,
        axis: Axis,
        viewport: i32,
        content: i32,
        min_thumb: i32,
    ) -> Option<ScrollThumb> {
        let index = axis.index();
        self.bound(index, viewport, content);
        if let Some(now) = input.now().map(host_time) {
            self.clock = self.clock.max(now);
        }
        let (start, length) = match axis {
            Axis::Horizontal => (track.x, track.w),
            Axis::Vertical => (track.y, track.h),
        };
        let max = ScrollState::max_offset(viewport, content);
        let geometry = |position: f64| {
            crate::widget::scrollbar_thumb(length, viewport, content, whole(position), min_thumb)
        };
        let (mut thumb_at, thumb_len) = geometry(self.motion[index].position())?;
        let thumb_rect = |at: i32| match axis {
            Axis::Horizontal => Rect::new(start + at, track.y, thumb_len, track.h),
            Axis::Vertical => Rect::new(track.x, start + at, track.w, thumb_len),
        };
        let page = f64::from((viewport - viewport / 8).max(1));

        if let Some(press) = input
            .press_position()
            .filter(|(x, y)| track.contains(*x, *y))
        {
            input.take_click(track);
            let along = axis.of(press) - start;
            if (thumb_at..thumb_at + thumb_len).contains(&along) {
                self.grab = Grab::Thumb {
                    axis,
                    offset: along - thumb_at,
                };
            } else {
                let direction = if along < thumb_at { -1.0 } else { 1.0 };
                self.wheel.interrupt();
                let to = self.motion[index].target() + direction * page;
                self.motion[index].animate_to(self.clock, to, self.tuning.page_time);
                self.grab = Grab::Track {
                    axis,
                    next: self.clock + TRACK_DELAY,
                };
            }
        }

        match self.grab {
            Grab::Thumb { axis: held, offset } if held == axis => {
                if let (true, Some(cursor)) = (input.is_held(), input.cursor()) {
                    let travel = (length - thumb_len).max(1);
                    let wanted = (axis.of(cursor) - start - offset).clamp(0, travel);
                    let position = f64::from(wanted) * f64::from(max) / f64::from(travel);
                    self.wheel.interrupt();
                    self.motion[index].jump_to(self.clock, position);
                    thumb_at = wanted;
                } else {
                    self.grab = Grab::None;
                }
            }
            Grab::Track { axis: held, next } if held == axis => {
                let cursor = input.cursor().map(|cursor| axis.of(cursor) - start);
                match (input.is_held(), cursor) {
                    (true, Some(along)) => {
                        let (target_at, _) = geometry(self.motion[index].target())?;
                        let reached = (target_at..target_at + thumb_len).contains(&along);
                        if !reached && self.clock >= next {
                            let direction = if along < target_at { -1.0 } else { 1.0 };
                            let to = self.motion[index].target() + direction * page;
                            self.motion[index].animate_to(self.clock, to, self.tuning.page_time);
                            self.grab = Grab::Track {
                                axis,
                                next: self.clock + TRACK_REPEAT,
                            };
                        }
                        // Held on the track: keep waking to page again.
                        input.request_frame();
                    }
                    _ => self.grab = Grab::None,
                }
            }
            _ => {}
        }

        let rect = thumb_rect(thumb_at);
        let held = matches!(self.grab, Grab::Thumb { axis: held, .. } if held == axis);
        Some(ScrollThumb {
            rect,
            hovered: held || input.hovering(rect),
            held,
        })
    }
}

/// Scrollbar gutter width for [`Ui::scroll_area`](crate::widget::Ui::scroll_area), in
/// logical pixels.
pub const AREA_GUTTER: i32 = 10;

impl crate::widget::Ui<'_> {
    /// A region that scrolls whatever is drawn in it, on both axes.
    ///
    /// `content` is the size of what there is to show, in device pixels.
    /// `draw` is given the viewport (the area less any scrollbars) and the
    /// offset; it draws its content at the content's own position, plus the
    /// viewport's corner, minus the offset. Scrollbars appear on the edges
    /// that need them, and can be dragged.
    ///
    /// The table, the list and the text area have this built in. This is for
    /// anything else: a page, a canvas, a form taller than its window.
    pub fn scroll_area<R>(
        &mut self,
        state: &mut ScrollState,
        area: Rect,
        content: (i32, i32),
        draw: impl FnOnce(&mut crate::widget::Ui<'_>, Rect, (i32, i32)) -> R,
    ) -> R {
        let gutter = self.px(AREA_GUTTER);
        // A bar on one edge takes room from the other axis, which can be
        // what makes the other bar necessary.
        let mut viewport = area;
        let mut tall = content.1 > viewport.h;
        if tall {
            viewport.w = (viewport.w - gutter).max(0);
        }
        let wide = content.0 > viewport.w;
        if wide {
            viewport.h = (viewport.h - gutter).max(0);
            if !tall && content.1 > viewport.h {
                tall = true;
                viewport.w = (viewport.w - gutter).max(0);
            }
        }
        let min_thumb = self.px(crate::widget::MIN_THUMB);
        let vertical = Rect::new(viewport.right(), area.y, gutter, viewport.h);
        let horizontal = Rect::new(area.x, viewport.bottom(), viewport.w, gutter);
        let thumb_y = tall
            .then(|| {
                state.scrollbar(
                    self.input,
                    vertical,
                    Axis::Vertical,
                    viewport.h,
                    content.1,
                    min_thumb,
                )
            })
            .flatten();
        let thumb_x = wide
            .then(|| {
                state.scrollbar(
                    self.input,
                    horizontal,
                    Axis::Horizontal,
                    viewport.w,
                    content.0,
                    min_thumb,
                )
            })
            .flatten();
        let line = self
            .input
            .scroll_settings()
            .default_line_px(f64::from(self.scale.0));
        let view = state.update(
            self.input,
            viewport,
            (viewport.w, viewport.h),
            content,
            (line, line),
        );

        self.painter.push_clip(viewport);
        let result = draw(self, viewport, view.offset);
        self.painter.pop_clip();

        let inset = self.px(3);
        if let Some(thumb) = thumb_y {
            let rect = Rect::new(
                vertical.x + inset,
                thumb.rect.y,
                (gutter - inset * 2).max(1),
                thumb.rect.h,
            );
            self.scroll_thumb(thumb, rect);
        }
        if let Some(thumb) = thumb_x {
            let rect = Rect::new(
                thumb.rect.x,
                horizontal.y + inset,
                thumb.rect.w,
                (gutter - inset * 2).max(1),
            );
            self.scroll_thumb(thumb, rect);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liminalkit_shell::scroll::{GesturePhase, WheelSample};
    use std::time::Instant;

    const AREA: Rect = Rect::new(0, 0, 200, 100);
    const VIEW: (i32, i32) = (200, 100);
    const CONTENT: (i32, i32) = (200, 2_000);
    const LINE: (f64, f64) = (20.0, 20.0);

    /// A window with a clock, the pointer over the region.
    struct Rig {
        input: Input,
        start: Instant,
        state: ScrollState,
    }

    impl Rig {
        fn new() -> Rig {
            let mut input = Input::new();
            input.cursor_moved(50.0, 50.0);
            let start = Instant::now();
            // Fix the shared epoch no later than this rig's own start.
            host_time(start);
            Rig {
                input,
                start,
                state: ScrollState::new(),
            }
        }

        fn notch(&mut self, ms: u64, raw: f32) {
            let at = host_time(self.start + Duration::from_millis(ms));
            self.input.wheel(WheelSample::from_lines(
                0.0,
                raw,
                true,
                GesturePhase::Moved,
                at,
            ));
        }

        /// Draw a frame at `ms`; the view, and whether another was asked for.
        fn frame(&mut self, ms: u64) -> (ScrollView, bool) {
            self.input
                .begin_frame(self.start + Duration::from_millis(ms));
            let view = self
                .state
                .update(&mut self.input, AREA, VIEW, CONTENT, LINE);
            self.input.end_frame();
            (view, self.input.frame_requested())
        }
    }

    #[test]
    fn a_notch_eases_in_over_a_few_frames_and_lands_exactly() {
        let mut rig = Rig::new();
        rig.frame(0);
        rig.notch(10, -1.0);
        let (first, wants_more) = rig.frame(16);
        // Three 20 px rows in all; the first frame already shows some of it.
        assert!(first.offset.1 > 0 && first.offset.1 < 60, "{:?}", first);
        assert!(first.moved && first.animating && wants_more);

        let mut last = first.offset.1;
        let mut frames = 0;
        for step in 2..40_u64 {
            let (view, wants_more) = rig.frame(step * 16);
            assert!(view.offset.1 >= last, "never backwards");
            last = view.offset.1;
            if !wants_more {
                frames = step;
                break;
            }
        }
        assert_eq!(last, 60, "exactly three rows");
        assert!(frames > 2 && frames < 30, "settled after {frames} frames");
        // At rest it asks for nothing and moves nowhere.
        let (rested, wants_more) = rig.frame(5_000);
        assert_eq!(
            (rested.offset.1, rested.moved, wants_more),
            (60, false, false)
        );
    }

    #[test]
    fn without_a_clock_everything_arrives_at_once() {
        let mut input = Input::new();
        input.cursor_moved(50.0, 50.0);
        let mut state = ScrollState::new();
        input.wheel(WheelSample::from_lines(
            0.0,
            -1.0,
            true,
            GesturePhase::Moved,
            Duration::ZERO,
        ));
        let view = state.update(&mut input, AREA, VIEW, CONTENT, LINE);
        assert_eq!((view.offset.1, view.animating), (60, false));
        input.end_frame();
        assert!(!input.frame_requested());
    }

    #[test]
    fn the_offset_is_one_whole_number_for_the_whole_frame() {
        // Whatever the position is underneath, a frame draws and hit-tests
        // at the offset `update` returned.
        let mut rig = Rig::new();
        rig.frame(0);
        rig.notch(5, -1.0);
        let (view, _) = rig.frame(16);
        assert_eq!(rig.state.offset(), view.offset.1);
        assert_eq!(rig.state.offset(), view.offset.1, "and it does not drift");
    }

    #[test]
    fn a_press_stops_the_scroll_where_it_was_drawn() {
        let mut rig = Rig::new();
        rig.frame(0);
        rig.notch(5, -1.0);
        rig.notch(8, -1.0);
        let (drawn, _) = rig.frame(16);
        assert!(drawn.animating);
        rig.input.mouse(MouseButton::Left, true);
        let (pressed, _) = rig.frame(32);
        assert_eq!(
            pressed.offset, drawn.offset,
            "the click lands on what was seen"
        );
        let (after, wants_more) = rig.frame(500);
        assert_eq!(after.offset, drawn.offset);
        assert!(!wants_more);
    }

    #[test]
    fn a_touchpad_fraction_is_carried_until_it_makes_a_pixel() {
        let mut rig = Rig::new();
        rig.frame(0);
        // Three 120ths of a notch: a pixel and a half of 60, every 18 ms.
        let mut total = 0;
        for step in 1..=40_u64 {
            rig.notch(step * 18, -3.0 / 120.0);
            total = rig.frame(step * 18 + 2).0.offset.1;
        }
        for step in 0..30_u64 {
            total = rig.frame(800 + step * 16).0.offset.1;
        }
        assert_eq!(total, 60, "forty of them are one notch");
    }

    #[test]
    fn the_thumb_drags_one_to_one_and_keeps_following_outside_the_window() {
        let mut rig = Rig::new();
        let track = Rect::new(190, 0, 10, 100);
        let bar = |rig: &mut Rig, ms: u64| {
            rig.input.begin_frame(rig.start + Duration::from_millis(ms));
            let thumb = rig
                .state
                .scrollbar(&mut rig.input, track, Axis::Vertical, 100, 2_000, 20)
                .expect("there is something to scroll");
            rig.state.update(&mut rig.input, AREA, VIEW, CONTENT, LINE);
            rig.input.end_frame();
            thumb
        };
        let resting = bar(&mut rig, 0);
        assert_eq!(
            (resting.rect.y, resting.rect.h, resting.held),
            (0, 20, false)
        );

        // Take hold of the thumb 5 px from its top and pull it down 40 px.
        rig.input.cursor_moved(195.0, 5.0);
        rig.input.mouse(MouseButton::Left, true);
        assert!(bar(&mut rig, 16).held);
        rig.input.cursor_moved(195.0, 45.0);
        let dragged = bar(&mut rig, 32);
        assert_eq!(dragged.rect.y, 40, "the thumb is where the pointer put it");
        // 40 of the track's 80 px of travel: half of the 1900 px range.
        assert_eq!(rig.state.offset(), 950);
        assert!(!rig.state.is_animating(), "no easing on a drag");

        // Off the bar, then out of the window altogether.
        rig.input.cursor_moved(20.0, 65.0);
        assert_eq!(bar(&mut rig, 48).rect.y, 60);
        rig.input.cursor_left();
        rig.input.cursor_moved(-300.0, 500.0);
        let outside = bar(&mut rig, 64);
        assert_eq!(outside.rect.y, 80, "pinned at the end, still held");
        assert_eq!(rig.state.offset(), 1_900);

        rig.input.mouse(MouseButton::Left, false);
        assert!(!bar(&mut rig, 80).held);
        assert_eq!(rig.state.offset(), 1_900, "and it stays where it was left");
    }

    #[test]
    fn a_press_beside_the_thumb_pages_and_goes_on_paging_while_held() {
        let mut rig = Rig::new();
        let track = Rect::new(190, 0, 10, 100);
        let bar = |rig: &mut Rig, ms: u64| {
            rig.input.begin_frame(rig.start + Duration::from_millis(ms));
            rig.state
                .scrollbar(&mut rig.input, track, Axis::Vertical, 100, 2_000, 20);
            rig.state.update(&mut rig.input, AREA, VIEW, CONTENT, LINE);
            rig.input.end_frame();
        };
        bar(&mut rig, 0);
        rig.input.cursor_moved(195.0, 90.0);
        rig.input.mouse(MouseButton::Left, true);
        bar(&mut rig, 10);
        let page = 100 - 100 / 8;
        assert_eq!(rig.state.target(), page, "one page, less a little overlap");
        // Still held after the delay: it pages again, and again.
        for step in 0..80_u64 {
            bar(&mut rig, 20 + step * 16);
        }
        assert!(rig.state.target() >= page * 3, "{}", rig.state.target());
        let reached = rig.state.target();
        rig.input.mouse(MouseButton::Left, false);
        for step in 0..40_u64 {
            bar(&mut rig, 1_400 + step * 16);
        }
        assert_eq!(rig.state.offset(), reached, "released, it stops paging");
        assert!(!rig.input.frame_requested());
    }

    #[test]
    fn the_middle_button_drags_the_content_with_the_pointer() {
        let mut rig = Rig::new();
        rig.state.scroll_by(500, 100, 2_000);
        rig.frame(0);
        rig.input.mouse(MouseButton::Middle, true);
        rig.frame(16);
        // Pull the content down 30 px: what was above comes into view.
        rig.input.cursor_moved(50.0, 80.0);
        let (view, _) = rig.frame(32);
        assert_eq!(view.offset.1, 470);
        assert_eq!(rig.input.take_cursor_request(), Some(CursorShape::Grabbing));
        rig.input.mouse(MouseButton::Middle, false);
        rig.input.cursor_moved(50.0, 10.0);
        assert_eq!(rig.frame(48).0.offset.1, 470, "let go, it stays");
    }

    #[test]
    fn a_middle_click_can_start_a_steady_scroll_that_the_next_press_ends() {
        let mut rig = Rig::new();
        rig.state.set_tuning(ScrollTuning {
            middle: MiddleButton::Both,
            ..ScrollTuning::default()
        });
        rig.frame(0);
        rig.input.mouse(MouseButton::Middle, true);
        rig.frame(16);
        rig.input.mouse(MouseButton::Middle, false);
        rig.frame(32);
        assert_eq!(rig.state.autoscroll_anchor(), Some((50, 50)));
        // Inside the dead zone nothing moves.
        rig.input.cursor_moved(50.0, 56.0);
        assert_eq!(rig.frame(200).0.offset.1, 0);
        // Further from the anchor it scrolls, and faster the further.
        rig.input.cursor_moved(50.0, 90.0);
        rig.frame(216);
        let (near, wants_more) = rig.frame(1_216);
        assert!(near.offset.1 > 100, "{:?}", near);
        assert!(wants_more, "it keeps itself going");
        // Any press ends it.
        rig.input.mouse(MouseButton::Left, true);
        let (stopped, _) = rig.frame(1_232);
        assert_eq!(rig.state.autoscroll_anchor(), None);
        rig.input.mouse(MouseButton::Left, false);
        assert_eq!(rig.frame(3_000).0.offset.1, stopped.offset.1);
    }

    #[test]
    fn content_let_go_while_moving_coasts_if_the_region_flings() {
        let mut rig = Rig::new();
        rig.state.set_tuning(ScrollTuning {
            fling: Some(Fling::default()),
            ..ScrollTuning::default()
        });
        rig.state.scroll_by(1_000, 100, 2_000);
        rig.frame(0);
        rig.input.mouse(MouseButton::Middle, true);
        rig.frame(16);
        // Drag the content up at a steady 10 px a frame: 625 px a second.
        for step in 1..=10_u64 {
            rig.input.cursor_moved(50.0, 50.0 - 10.0 * step as f32);
            rig.frame(16 + step * 16);
        }
        let let_go = rig.state.offset();
        assert_eq!(let_go, 1_090 + 10, "the drag itself is one to one");
        rig.input.mouse(MouseButton::Middle, false);
        let (_, wants_more) = rig.frame(192);
        assert!(wants_more, "it is still going");
        assert!(
            rig.state.target() > let_go + 50,
            "and knows where it will stop"
        );
        let rest = rig.state.target();
        assert_eq!(rig.frame(5_000).0.offset.1, rest);
        assert!(!rig.state.is_animating());
    }

    #[test]
    fn a_hand_that_stopped_before_letting_go_does_not_fling() {
        let mut rig = Rig::new();
        rig.state.set_tuning(ScrollTuning {
            fling: Some(Fling::default()),
            ..ScrollTuning::default()
        });
        rig.state.scroll_by(1_000, 100, 2_000);
        rig.frame(0);
        rig.input.mouse(MouseButton::Middle, true);
        rig.frame(16);
        for step in 1..=5_u64 {
            rig.input.cursor_moved(50.0, 50.0 - 10.0 * step as f32);
            rig.frame(16 + step * 16);
        }
        // Held still for half a second, then released.
        rig.frame(600);
        let held = rig.state.offset();
        rig.input.mouse(MouseButton::Middle, false);
        let (view, wants_more) = rig.frame(616);
        assert_eq!((view.offset.1, wants_more), (held, false));
    }

    #[test]
    fn a_hand_tool_region_drags_with_the_left_button_and_keeps_its_presses() {
        let mut rig = Rig::new();
        rig.state.set_tuning(ScrollTuning {
            drag_pan: true,
            ..ScrollTuning::default()
        });
        rig.state.scroll_by(500, 100, 2_000);
        rig.frame(0);
        assert_eq!(rig.input.take_cursor_request(), Some(CursorShape::Grab));
        rig.input.mouse(MouseButton::Left, true);
        rig.input.begin_frame(rig.start + Duration::from_millis(16));
        rig.state.update(&mut rig.input, AREA, VIEW, CONTENT, LINE);
        assert!(
            !rig.input.take_click(AREA),
            "the press is the region's, not a click on what is in it"
        );
        rig.input.end_frame();
        rig.input.cursor_moved(50.0, 20.0);
        assert_eq!(rig.frame(32).0.offset.1, 530);
        assert_eq!(rig.input.take_cursor_request(), Some(CursorShape::Grabbing));
        rig.input.mouse(MouseButton::Left, false);
        assert_eq!(rig.frame(48).0.offset.1, 530);
    }

    #[test]
    fn with_the_middle_button_off_the_press_is_left_for_the_application() {
        let mut rig = Rig::new();
        rig.state.set_tuning(ScrollTuning {
            middle: MiddleButton::Off,
            ..ScrollTuning::default()
        });
        rig.input.mouse(MouseButton::Middle, true);
        rig.input.begin_frame(rig.start + Duration::from_millis(16));
        rig.state.update(&mut rig.input, AREA, VIEW, CONTENT, LINE);
        assert!(rig.input.take_middle_click(AREA), "still there to claim");
    }

    #[test]
    fn a_list_inside_a_page_takes_the_wheel_until_it_runs_out() {
        // The page is drawn, and asks for the wheel, before the list in it.
        let mut input = Input::new();
        input.cursor_moved(50.0, 50.0);
        let start = Instant::now();
        let mut page = ScrollState::new();
        let mut list = ScrollState::new();
        let inner = Rect::new(20, 20, 100, 60);
        let mut frame = |input: &mut Input, ms: u64| {
            input.begin_frame(start + Duration::from_millis(ms));
            page.update(input, AREA, VIEW, CONTENT, LINE);
            list.update(input, inner, (100, 60), (100, 120), LINE);
            input.end_frame();
            (page.target(), list.target())
        };
        let notch = |input: &mut Input, ms: u64| {
            input.wheel(WheelSample::from_lines(
                0.0,
                -1.0,
                true,
                GesturePhase::Moved,
                host_time(start + Duration::from_millis(ms)),
            ));
        };
        frame(&mut input, 0);
        notch(&mut input, 10);
        assert_eq!(frame(&mut input, 16), (0, 60), "the list, not the page");
        // The list is at its end now. The same roll does not spill over.
        notch(&mut input, 100);
        assert_eq!(frame(&mut input, 116), (0, 60));
        // A new roll, after a pause, moves the page.
        notch(&mut input, 2_000);
        assert_eq!(frame(&mut input, 2_016), (60, 60));
    }

    #[test]
    fn a_chained_list_hands_on_what_it_cannot_use() {
        let mut input = Input::new();
        input.cursor_moved(50.0, 50.0);
        let mut page = ScrollState::new();
        let mut list = ScrollState::with_tuning(ScrollTuning {
            boundary: ScrollBoundary::Chain,
            ..ScrollTuning::default()
        });
        let inner = Rect::new(20, 20, 100, 60);
        let mut frame = |input: &mut Input| {
            page.update(input, AREA, VIEW, CONTENT, LINE);
            list.update(input, inner, (100, 60), (100, 90), LINE);
            input.end_frame();
            (page.offset(), list.offset())
        };
        frame(&mut input);
        // A notch is 60 px; the list has 30 to give.
        input.wheel(WheelSample::from_lines(
            0.0,
            -1.0,
            true,
            GesturePhase::Moved,
            Duration::ZERO,
        ));
        assert_eq!(frame(&mut input), (0, 30));
        assert_eq!(frame(&mut input), (30, 30), "the rest reaches the page");
    }

    #[test]
    fn scrolling_sideways_moves_only_content_that_is_wider() {
        let mut input = Input::new();
        input.cursor_moved(50.0, 50.0);
        let mut state = ScrollState::new();
        let sideways = WheelSample::from_pixels(-40.0, 0.0, GesturePhase::Moved, Duration::ZERO);
        input.wheel(sideways);
        let view = state.update(&mut input, AREA, VIEW, (600, 2_000), LINE);
        assert_eq!(view.offset, (40, 0));
        input.end_frame();
        let mut narrow = ScrollState::new();
        input.wheel(sideways);
        assert_eq!(
            narrow.update(&mut input, AREA, VIEW, CONTENT, LINE).offset,
            (0, 0)
        );
    }

    #[test]
    fn a_scroll_area_gives_its_content_a_viewport_and_bars_only_where_needed() {
        use crate::widget::{Theme, Ui};
        use liminalkit_raster::{Painter, RasterKernel, WindowBuffer};
        use liminalkit_shell::Scale;
        use liminalkit_text::TextCache;

        let mut buffer = WindowBuffer::new(300, 200);
        let mut text = TextCache::new(liminalkit_test_fonts::set());
        let mut kernel = RasterKernel::new();
        let mut input = Input::new();
        input.cursor_moved(50.0, 50.0);
        input.scrolled(-30.0, -70.0);
        let mut state = ScrollState::new();
        let area = Rect::new(0, 0, 300, 200);
        let mut ui = Ui::new(
            Painter::new(&mut buffer),
            &mut text,
            &mut input,
            Theme::default(),
            Scale::ONE,
            &mut kernel,
        );
        // Taller and wider than the area: both bars, each taking a gutter.
        let (viewport, offset) =
            ui.scroll_area(&mut state, area, (900, 1_200), |_, viewport, offset| {
                (viewport, offset)
            });
        assert_eq!(viewport, Rect::new(0, 0, 290, 190));
        assert_eq!(offset, (30, 70));
        // Only taller: one bar, and the full height.
        let mut tall = ScrollState::new();
        let viewport = ui.scroll_area(&mut tall, area, (200, 1_200), |_, viewport, _| viewport);
        assert_eq!(viewport, Rect::new(0, 0, 290, 200));
        // It all fits: the whole area, and nothing to scroll.
        let mut fits = ScrollState::new();
        let viewport = ui.scroll_area(&mut fits, area, (200, 100), |_, viewport, _| viewport);
        assert_eq!(viewport, area);
    }

    #[test]
    fn a_programmed_move_travels_and_the_view_ends_exactly_there() {
        let mut rig = Rig::new();
        rig.frame(0);
        rig.state.animate_to(Axis::Vertical, 800);
        assert_eq!(rig.state.target(), 800);
        let (mid, wants_more) = rig.frame(60);
        assert!(mid.offset.1 > 0 && mid.offset.1 < 800 && wants_more);
        assert_eq!(rig.frame(400).0.offset.1, 800);
        rig.state.jump_to(Axis::Vertical, 5);
        assert_eq!(rig.frame(416).0.offset.1, 5);
    }
}
