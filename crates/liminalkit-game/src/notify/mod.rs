//! A notification stack: toasts that arrive, wait their turn, slide in,
//! stay a while and leave, laid out in one corner of the screen.
//!
//! [`NoticeQueue`] is the whole of it, and it draws nothing. The caller
//! pushes [`Notice`]s carrying any payload, calls
//! [`update`](NoticeQueue::update) once a frame with the time, the card under
//! the pointer and a way to measure a payload's height, and then draws each
//! [`Card`] from [`cards`](NoticeQueue::cards) however it likes: the queue
//! says where, how opaque and how large. Notices that are done come back
//! through [`take_retired`](NoticeQueue::take_retired) with their payload
//! and the reason, so the caller can act on an answer or log what was
//! missed.
//!
//! What it decides:
//!
//! - **Order.** Higher priority sits nearer the corner; within a priority,
//!   newer sits nearer.
//! - **Room.** At most [`StackStyle::max_visible`] cards are shown (not
//!   counting cards on their way out); the rest wait, highest priority and
//!   then oldest first, and come in as room frees up. A newcomer that
//!   outranks the lowest card shown when the stack is full sends that card
//!   back to wait: it slides out, keeps what was left of its lifetime, and
//!   comes back later. Nothing is dropped.
//! - **Time.** A notice's lifetime counts down only while it is fully shown
//!   and not under the pointer, so entering does not eat into it and
//!   reading it pauses it exactly.
//! - **Repeats.** A notice with a [`coalesce`](Notice::coalesce) key that
//!   matches one already live replaces that one's payload in place instead
//!   of stacking a duplicate.
//!
//! With no clock (`now` is `None`) every movement finishes at once and
//! lifetimes never run out, since there is no time to count; dismissing
//! still works.
//!
//! EXPERIMENTAL: the API is new, and the default style has not been tuned
//! on a screen.

#[cfg(feature = "surfaces")]
pub mod toast;

use std::cmp::Reverse;
use std::collections::VecDeque;
use std::time::Duration;

use crate::anim::{Ease, Tween};
use crate::geom::RectF;

/// Names one notice for as long as the queue knows about it. A coalesced
/// notice keeps the id of the one it replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NoticeId(u64);

impl NoticeId {
    /// The number behind the id: unique within one queue, and larger for
    /// notices pushed later.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// How much a notice matters. Higher priorities sit nearer the corner and
/// take a full stack's place from lower ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Background news.
    Low,
    /// The default.
    Normal,
    /// Wants attention.
    High,
    /// Must be seen.
    Critical,
}

/// Something to show: a payload and how to treat it.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice<T> {
    /// What the caller draws; the queue only measures and holds it.
    pub payload: T,
    /// Where it sits and what it displaces.
    pub priority: Priority,
    /// How long it is shown before it leaves on its own, counted only while
    /// it is fully shown and not hovered. `None`: until dismissed.
    pub lifetime: Option<Duration>,
    /// A key that makes a later notice with the same key replace this one
    /// in place ("3 new messages" updating "2 new messages") instead of
    /// stacking beside it.
    pub coalesce: Option<u64>,
}

impl<T> Notice<T> {
    /// A [`Normal`](Priority::Normal) notice that stays 5 seconds and does
    /// not coalesce.
    pub fn new(payload: T) -> Self {
        Notice {
            payload,
            priority: Priority::Normal,
            lifetime: Some(Duration::from_secs(5)),
            coalesce: None,
        }
    }

    /// With this priority.
    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    /// Shown for this long before leaving on its own.
    pub fn lifetime(mut self, lifetime: Duration) -> Self {
        self.lifetime = Some(lifetime);
        self
    }

    /// Shown until dismissed.
    pub fn sticky(mut self) -> Self {
        self.lifetime = None;
        self
    }

    /// Replacing any live notice pushed with the same key.
    pub fn coalesce(mut self, key: u64) -> Self {
        self.coalesce = Some(key);
        self
    }
}

/// The screen corner the stack grows from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    /// Top left: cards stack downwards and slide in from the left.
    TopLeft,
    /// Top right: cards stack downwards and slide in from the right.
    TopRight,
    /// Bottom left: cards stack upwards and slide in from the left.
    BottomLeft,
    /// Bottom right: cards stack upwards and slide in from the right.
    BottomRight,
}

impl Corner {
    fn is_bottom(self) -> bool {
        matches!(self, Corner::BottomLeft | Corner::BottomRight)
    }

    fn is_right(self) -> bool {
        matches!(self, Corner::TopRight | Corner::BottomRight)
    }
}

/// How the stack is laid out and animated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StackStyle {
    /// Which corner it grows from.
    pub corner: Corner,
    /// Every card's width, in pixels.
    pub width: f32,
    /// Space between cards, in pixels.
    pub gap: f32,
    /// How many cards may be shown at once, not counting cards on their way
    /// out. Zero holds everything back.
    pub max_visible: usize,
    /// How long a card takes to come in.
    pub enter: Duration,
    /// How long a card takes to go.
    pub exit: Duration,
    /// How long the cards take to close a gap or make room.
    pub reflow: Duration,
    /// How far, in pixels, a card travels sideways as it comes in or goes:
    /// from beyond the nearer side edge of the screen.
    pub slide: f32,
    /// The curve of every movement.
    pub ease: Ease,
    /// Whether the card under the pointer stops counting down, so that a
    /// notice being read does not leave under the reader.
    pub pause_on_hover: bool,
}

impl Default for StackStyle {
    /// Top right, 320 px wide with 8 px between cards, four at a time,
    /// 220 ms in, 180 ms out, 200 ms to reflow, a 24 px slide, cubic ease
    /// out, pausing on hover.
    fn default() -> StackStyle {
        StackStyle {
            corner: Corner::TopRight,
            width: 320.0,
            gap: 8.0,
            max_visible: 4,
            enter: Duration::from_millis(220),
            exit: Duration::from_millis(180),
            reflow: Duration::from_millis(200),
            slide: 24.0,
            ease: Ease::CubicOut,
            pause_on_hover: true,
        }
    }
}

/// Why a notice left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Retired {
    /// Its lifetime ran out.
    Expired,
    /// The user (or the caller) closed it.
    Dismissed,
    /// The user chose an action on it; the string is the caller's own name
    /// for the action.
    Action(String),
    /// A coalescing notice took its place. The id stays live with the new
    /// payload; this is the old payload, handed back.
    Replaced,
}

/// Where a card is in its life on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Sliding and fading in.
    Entering,
    /// Fully shown.
    Shown,
    /// Sliding and fading out: retiring, or making way for a notice that
    /// outranks it and coming back later.
    Leaving,
}

/// One card to draw, as of the last [`update`](NoticeQueue::update).
///
/// `rect` is relative to the stack's corner: the origin is the corner
/// itself, `x` grows to the right and `y` downwards, in pixels, and the
/// stack lies in the quarter of the plane that extends into the screen. The
/// caller adds the corner's position on screen (for a right-hand corner the
/// screen's right edge, for a bottom one its bottom edge, less any margin).
/// The card nearest the corner comes first, and the sideways slide always
/// moves towards the nearer side edge of the screen:
///
/// - [`TopRight`](Corner::TopRight): `x` in `[-width, 0]`, `y >= 0` growing
///   downwards; cards slide out towards `+x`.
/// - [`TopLeft`](Corner::TopLeft): `x` in `[0, width]`, `y >= 0` growing
///   downwards; cards slide out towards `-x`.
/// - [`BottomRight`](Corner::BottomRight): `x` in `[-width, 0]`, `y <= 0`
///   growing upwards (the first card's bottom edge is at `y = 0`); cards
///   slide out towards `+x`.
/// - [`BottomLeft`](Corner::BottomLeft): `x` in `[0, width]`, `y <= 0`
///   growing upwards; cards slide out towards `-x`.
///
/// The `rect` is the card's layout box, slide included; `scale` is applied
/// by the caller about the box's centre.
#[derive(Debug)]
pub struct Card<'q, T> {
    /// Which notice.
    pub id: NoticeId,
    /// What to draw.
    pub payload: &'q T,
    /// Its priority, for styling.
    pub priority: Priority,
    /// Where, relative to the stack's corner; see the type documentation.
    pub rect: RectF,
    /// 0 to 1: how far in it is.
    pub opacity: f32,
    /// 0.96 to 1: a card grows slightly as it comes in.
    pub scale: f32,
    /// Coming, here, or going.
    pub phase: Phase,
    /// Whether the notice is new or was just replaced by a coalescing one,
    /// so the caller can flash it. It is set by the first
    /// [`update`](NoticeQueue::update) that finds the card on screen after
    /// it was pushed or coalesced, and cleared by the update after that:
    /// exactly one update-then-draw cycle sees it.
    pub fresh: bool,
}

/// What an [`update`](NoticeQueue::update) found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StackView {
    /// Whether any card is still moving, so another frame is worth drawing
    /// soon.
    pub animating: bool,
    /// Whether anything the cards show differs from the previous update: a
    /// position, an opacity, a card come or gone, a payload replaced or
    /// edited, or `fresh` turned on or off.
    pub changed: bool,
    /// The earliest host time at which a notice will run out of lifetime,
    /// so that a host which is otherwise idle knows when to wake and update.
    /// `None` when nothing will run out on its own (no clock, only sticky
    /// notices, or only the hovered one).
    pub next_deadline: Option<Duration>,
}

/// Why a card is leaving.
#[derive(Clone, Debug, PartialEq)]
enum Leave {
    /// For good, for this reason.
    Retire(Retired),
    /// Making way for a notice that outranks it: it goes back to wait.
    Requeue,
}

#[derive(Debug)]
struct Entry<T> {
    id: NoticeId,
    payload: T,
    priority: Priority,
    /// The lifetime it was pushed with, restored on coalescing.
    lifetime: Option<Duration>,
    /// What is left of it; `None` for sticky.
    remaining: Option<Duration>,
    coalesce: Option<u64>,
    /// The host time from which lifetime not yet counted starts: the end of
    /// the enter animation, then the time of each update that counted.
    /// `None` until a clock is first seen while it is shown.
    count_from: Option<Duration>,
    /// 0 off screen, 1 fully shown.
    presence: Tween<f32>,
    /// Distance from the corner along the stack, in pixels.
    offset: Tween<f32>,
    /// As last measured.
    height: f32,
    /// Whether `offset` has been put in a slot since it came on screen.
    placed: bool,
    leaving: Option<Leave>,
    /// Waiting to be reported fresh by the next update that shows it.
    fresh_pending: bool,
    /// Reported fresh by `cards` until the next update.
    fresh: bool,
}

impl<T> Entry<T> {
    /// Its order in the stack: higher priority, then newer, first.
    fn stack_key(&self) -> (Reverse<Priority>, Reverse<NoticeId>) {
        (Reverse(self.priority), Reverse(self.id))
    }

    /// Its order in the wait: higher priority, then older, first.
    fn wait_key(&self) -> (Reverse<Priority>, NoticeId) {
        (Reverse(self.priority), self.id)
    }

    fn is_live(&self) -> bool {
        self.leaving.is_none()
    }

    fn phase(&self) -> Phase {
        if self.leaving.is_some() {
            Phase::Leaving
        } else if self.presence.is_animating() || self.presence.value() < 1.0 {
            Phase::Entering
        } else {
            Phase::Shown
        }
    }

    fn card(&self, style: &StackStyle) -> Card<'_, T> {
        let presence = self.presence.value();
        let offset = self.offset.value();
        let away = style.slide * (1.0 - presence);
        let x = if style.corner.is_right() {
            -style.width + away
        } else {
            -away
        };
        let y = if style.corner.is_bottom() {
            -(offset + self.height)
        } else {
            offset
        };
        Card {
            id: self.id,
            payload: &self.payload,
            priority: self.priority,
            rect: RectF::new(x, y, style.width, self.height),
            opacity: presence,
            // Written so that it is exactly 1 when fully in.
            scale: 0.96 * (1.0 - presence) + presence,
            phase: self.phase(),
            fresh: self.fresh,
        }
    }
}

/// An animated notification stack. See the module documentation.
#[derive(Debug)]
pub struct NoticeQueue<T> {
    /// On screen, in stack order, including cards on their way out.
    shown: Vec<Entry<T>>,
    /// Off screen, in the order they will come in.
    waiting: Vec<Entry<T>>,
    retired: VecDeque<(NoticeId, T, Retired)>,
    next_id: u64,
    /// The latest host time seen.
    clock: Option<Duration>,
    style: StackStyle,
    /// Something changed outside an update that the next one must report.
    dirty: bool,
}

impl<T> NoticeQueue<T> {
    /// An empty stack.
    pub fn new(style: StackStyle) -> Self {
        NoticeQueue {
            shown: Vec::new(),
            waiting: Vec::new(),
            retired: VecDeque::new(),
            next_id: 0,
            clock: None,
            style,
            dirty: false,
        }
    }

    /// The layout and timing in use.
    pub fn style(&self) -> &StackStyle {
        &self.style
    }

    /// Change the layout and timing. Cards move to the new layout, and a
    /// new [`max_visible`](StackStyle::max_visible) is applied, at the next
    /// update.
    pub fn set_style(&mut self, style: StackStyle) {
        self.style = style;
        self.dirty = true;
    }

    /// Note `now` and return the time to work at: the latest seen.
    fn observe(&mut self, now: Option<Duration>) -> Option<Duration> {
        let now = now?;
        let clock = self.clock.map_or(now, |clock| clock.max(now));
        self.clock = Some(clock);
        Some(clock)
    }

    /// Add a notice, and return its id.
    ///
    /// If its [`coalesce`](Notice::coalesce) key matches a live notice (one
    /// shown, waiting, or making way for a higher one; not one retiring),
    /// that notice takes the new payload and lifetime in place: same id,
    /// same place in the stack, same priority, its lifetime starting again,
    /// and `fresh` for the next update. The old payload is retired as
    /// [`Retired::Replaced`] under that id, and that id is returned.
    pub fn push(&mut self, now: Option<Duration>, notice: Notice<T>) -> NoticeId {
        let now = self.observe(now);
        self.dirty = true;
        if let Some(key) = notice.coalesce {
            let same = self
                .shown
                .iter_mut()
                .filter(|entry| !matches!(entry.leaving, Some(Leave::Retire(_))))
                .chain(self.waiting.iter_mut())
                .find(|entry| entry.coalesce == Some(key));
            if let Some(entry) = same {
                let old = std::mem::replace(&mut entry.payload, notice.payload);
                entry.lifetime = notice.lifetime;
                entry.remaining = notice.lifetime;
                // Shown already: the fresh lifetime counts from now. Still
                // entering: from when it is in, as before.
                if let (Some(now), Some(from)) = (now, entry.count_from) {
                    entry.count_from = Some(from.max(now));
                }
                entry.fresh_pending = true;
                let id = entry.id;
                self.retired.push_back((id, old, Retired::Replaced));
                return id;
            }
        }
        let id = NoticeId(self.next_id);
        self.next_id += 1;
        self.enqueue(Entry {
            id,
            payload: notice.payload,
            priority: notice.priority,
            lifetime: notice.lifetime,
            remaining: notice.lifetime,
            coalesce: notice.coalesce,
            count_from: None,
            presence: Tween::new(0.0),
            offset: Tween::new(0.0),
            height: 0.0,
            placed: false,
            leaving: None,
            fresh_pending: true,
            fresh: false,
        });
        self.sweep(now);
        id
    }

    /// Close a notice: [`resolve`](Self::resolve) with
    /// [`Retired::Dismissed`].
    pub fn dismiss(&mut self, now: Option<Duration>, id: NoticeId) -> bool {
        self.resolve(now, id, Retired::Dismissed)
    }

    /// Retire a notice for `how`. A card on screen starts to leave and is
    /// retired when it is gone; a waiting notice is retired at once. False
    /// if the id is unknown or the notice is already retiring.
    pub fn resolve(&mut self, now: Option<Duration>, id: NoticeId, how: Retired) -> bool {
        let now = self.observe(now);
        if let Some(index) = self.shown.iter().position(|entry| entry.id == id) {
            match self.shown[index].leaving {
                Some(Leave::Retire(_)) => return false,
                // Already on its way out to wait; now it goes for good.
                Some(Leave::Requeue) => self.shown[index].leaving = Some(Leave::Retire(how)),
                None => self.begin_leave(index, now, Leave::Retire(how)),
            }
            self.dirty = true;
            self.sweep(now);
            return true;
        }
        if let Some(index) = self.waiting.iter().position(|entry| entry.id == id) {
            let entry = self.waiting.remove(index);
            self.retired.push_back((entry.id, entry.payload, how));
            self.dirty = true;
            return true;
        }
        false
    }

    /// Dismiss everything: waiting notices at once, shown ones as they
    /// leave.
    pub fn clear(&mut self, now: Option<Duration>) {
        let now = self.observe(now);
        for entry in self.waiting.drain(..) {
            self.retired
                .push_back((entry.id, entry.payload, Retired::Dismissed));
        }
        for index in 0..self.shown.len() {
            match self.shown[index].leaving {
                Some(Leave::Retire(_)) => {}
                Some(Leave::Requeue) => {
                    self.shown[index].leaving = Some(Leave::Retire(Retired::Dismissed));
                }
                None => self.begin_leave(index, now, Leave::Retire(Retired::Dismissed)),
            }
        }
        self.dirty = true;
        self.sweep(now);
    }

    /// Advance to `now`: count lifetimes down, expire what has run out, move
    /// every card, bring waiting notices in, and lay the stack out.
    ///
    /// `hovered` is the card under the pointer (from [`hit`](Self::hit));
    /// with [`pause_on_hover`](StackStyle::pause_on_hover) its countdown
    /// stops for the time since the previous update. `height` measures a
    /// payload, in pixels, as the caller will draw it at the style's width;
    /// it is asked for every card on screen, every update, so a card whose
    /// content grows makes room for itself.
    pub fn update(
        &mut self,
        now: Option<Duration>,
        hovered: Option<NoticeId>,
        mut height: impl FnMut(&T) -> f32,
    ) -> StackView {
        let now = self.observe(now);
        let hovered = hovered.filter(|_| self.style.pause_on_hover);

        if let Some(now) = now {
            self.count_down(now, hovered);
        }

        let mut changed = false;
        for entry in &mut self.shown {
            changed |= entry.presence.update(now).changed;
        }
        self.sweep(now);
        changed |= std::mem::take(&mut self.dirty);

        let style = self.style;
        let mut along = 0.0;
        for entry in &mut self.shown {
            let measured = height(&entry.payload);
            let measured = if measured.is_finite() && measured > 0.0 {
                measured
            } else {
                0.0
            };
            if measured != entry.height {
                entry.height = measured;
                changed = true;
            }
            if !entry.placed {
                // A card coming in appears in its slot; only its presence
                // animates.
                entry.offset.jump(along);
                entry.placed = true;
            } else if entry.offset.target() != along {
                // From wherever it has got to, so a reflow that changes its
                // mind part way is still continuous.
                entry.offset.start(now, along, style.reflow, style.ease);
            }
            changed |= entry.offset.update(now).changed;
            along += measured + style.gap;
        }

        for entry in &mut self.shown {
            changed |= entry.fresh != entry.fresh_pending;
            entry.fresh = std::mem::take(&mut entry.fresh_pending);
        }

        let animating = self
            .shown
            .iter()
            .any(|entry| entry.presence.is_animating() || entry.offset.is_animating());
        let next_deadline = now.and_then(|_| {
            self.shown
                .iter()
                .filter(|entry| entry.is_live() && Some(entry.id) != hovered)
                .filter_map(|entry| Some(entry.count_from? + entry.remaining?))
                .min()
        });
        StackView {
            animating,
            changed,
            next_deadline,
        }
    }

    /// Count the time since each shown card was last counted against its
    /// lifetime, and start the leave of any that ran out, at the moment it
    /// ran out.
    fn count_down(&mut self, now: Duration, hovered: Option<NoticeId>) {
        for index in 0..self.shown.len() {
            let entry = &mut self.shown[index];
            if !entry.is_live() {
                continue;
            }
            let Some(from) = entry.count_from else {
                // Came on screen with no clock: count from here.
                entry.count_from = Some(now);
                continue;
            };
            if now <= from {
                // Not yet fully in.
                continue;
            }
            entry.count_from = Some(now);
            if Some(entry.id) == hovered {
                continue;
            }
            let Some(remaining) = entry.remaining else {
                continue;
            };
            let counted = now - from;
            if counted < remaining {
                entry.remaining = Some(remaining - counted);
            } else {
                entry.remaining = Some(Duration::ZERO);
                self.begin_leave(
                    index,
                    Some(from + remaining),
                    Leave::Retire(Retired::Expired),
                );
            }
        }
    }

    fn begin_leave(&mut self, index: usize, at: Option<Duration>, why: Leave) {
        let (exit, ease) = (self.style.exit, self.style.ease);
        let entry = &mut self.shown[index];
        entry.leaving = Some(why);
        entry.presence.start(at, 0.0, exit, ease);
    }

    /// Put an entry in the wait, in order.
    fn enqueue(&mut self, entry: Entry<T>) {
        let key = entry.wait_key();
        let at = self.waiting.partition_point(|other| other.wait_key() < key);
        self.waiting.insert(at, entry);
    }

    /// Bring an entry on screen, in order, and start it coming in.
    fn promote(&mut self, mut entry: Entry<T>, now: Option<Duration>) {
        entry
            .presence
            .start(now, 1.0, self.style.enter, self.style.ease);
        entry.count_from = now.map(|now| now + self.style.enter);
        entry.placed = false;
        entry.leaving = None;
        let key = entry.stack_key();
        let at = self.shown.partition_point(|other| other.stack_key() < key);
        self.shown.insert(at, entry);
        self.dirty = true;
    }

    /// Make the shown, live counts agree with `max_visible` and with
    /// priority: displace extras and outranked cards, bring waiting ones in.
    fn rebalance(&mut self, now: Option<Duration>) {
        loop {
            let live = self.shown.iter().filter(|entry| entry.is_live()).count();
            let lowest = self.shown.iter().rposition(Entry::is_live);
            if live > self.style.max_visible {
                if let Some(lowest) = lowest {
                    self.begin_leave(lowest, now, Leave::Requeue);
                    self.dirty = true;
                    continue;
                }
            }
            let Some(next) = self.waiting.first() else {
                break;
            };
            if live < self.style.max_visible {
                let entry = self.waiting.remove(0);
                self.promote(entry, now);
                continue;
            }
            match lowest {
                Some(lowest) if next.priority > self.shown[lowest].priority => {
                    self.begin_leave(lowest, now, Leave::Requeue);
                    let entry = self.waiting.remove(0);
                    self.promote(entry, now);
                }
                _ => break,
            }
        }
    }

    /// Take cards that have finished leaving off screen: retired, or back
    /// to wait. Whether there were any.
    fn remove_finished(&mut self) -> bool {
        let mut removed = false;
        let mut index = 0;
        while index < self.shown.len() {
            let entry = &self.shown[index];
            if entry.leaving.is_none() || entry.presence.is_animating() {
                index += 1;
                continue;
            }
            let mut entry = self.shown.remove(index);
            removed = true;
            match entry.leaving.take() {
                Some(Leave::Retire(why)) => self.retired.push_back((entry.id, entry.payload, why)),
                Some(Leave::Requeue) | None => {
                    entry.presence = Tween::new(0.0);
                    entry.count_from = None;
                    entry.placed = false;
                    entry.fresh = false;
                    self.enqueue(entry);
                }
            }
        }
        if removed {
            self.dirty = true;
        }
        removed
    }

    /// Settle who is on screen: rebalance, and repeat while that finished
    /// anything (with no clock a displaced card is gone at once).
    fn sweep(&mut self, now: Option<Duration>) {
        loop {
            self.rebalance(now);
            if !self.remove_finished() {
                break;
            }
        }
    }

    /// The cards to draw, nearest the corner first. Cards on their way out
    /// are included until they are gone.
    pub fn cards(&self) -> impl Iterator<Item = Card<'_, T>> + '_ {
        let style = self.style;
        self.shown.iter().map(move |entry| entry.card(&style))
    }

    /// The payload of a notice that is shown or waiting, to change in place.
    /// The next update reports the stack changed.
    pub fn payload_mut(&mut self, id: NoticeId) -> Option<&mut T> {
        let entry = self
            .shown
            .iter_mut()
            .chain(self.waiting.iter_mut())
            .find(|entry| entry.id == id)?;
        self.dirty = true;
        Some(&mut entry.payload)
    }

    /// The card at `point`, in the same corner-relative space as
    /// [`Card::rect`], as of the last update. Cards on their way out are
    /// not hit: what is leaving cannot be clicked.
    pub fn hit(&self, point: [f32; 2]) -> Option<NoticeId> {
        self.cards()
            .filter(|card| card.phase != Phase::Leaving)
            .find(|card| card.rect.contains(point))
            .map(|card| card.id)
    }

    /// The next retired notice, oldest first, with its payload and why it
    /// went.
    pub fn take_retired(&mut self) -> Option<(NoticeId, T, Retired)> {
        self.retired.pop_front()
    }

    /// Notices on screen (cards on their way out included) and waiting.
    pub fn len(&self) -> usize {
        self.shown.len() + self.waiting.len()
    }

    /// Whether there is nothing on screen and nothing waiting.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: f32 = 50.0;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn at(value: u64) -> Option<Duration> {
        Some(ms(value))
    }

    fn queue() -> NoticeQueue<&'static str> {
        NoticeQueue::new(StackStyle::default())
    }

    fn tall(_: &&str) -> f32 {
        H
    }

    /// Update every 10 ms from `from` to `to`, both included.
    fn run(queue: &mut NoticeQueue<&'static str>, from: u64, to: u64) -> StackView {
        let mut view = queue.update(at(from), None, tall);
        let mut now = from;
        while now < to {
            now = (now + 10).min(to);
            view = queue.update(at(now), None, tall);
        }
        view
    }

    fn ids(queue: &NoticeQueue<&'static str>) -> Vec<&'static str> {
        queue.cards().map(|card| *card.payload).collect()
    }

    fn phase(queue: &NoticeQueue<&'static str>, id: NoticeId) -> Option<Phase> {
        queue
            .cards()
            .find(|card| card.id == id)
            .map(|card| card.phase)
    }

    fn lifetime(text: &'static str, millis: u64) -> Notice<&'static str> {
        Notice::new(text).lifetime(ms(millis))
    }

    #[test]
    fn a_notice_comes_in_stays_its_lifetime_and_expires() {
        let mut queue = queue();
        let id = queue.push(at(0), lifetime("a", 1_000));
        run(&mut queue, 0, 100);
        assert_eq!(phase(&queue, id), Some(Phase::Entering));
        run(&mut queue, 100, 220);
        assert_eq!(phase(&queue, id), Some(Phase::Shown));
        // Entering took 220 ms, which did not count against the lifetime.
        run(&mut queue, 220, 1_210);
        assert_eq!(phase(&queue, id), Some(Phase::Shown));
        run(&mut queue, 1_210, 1_220);
        assert_eq!(phase(&queue, id), Some(Phase::Leaving));
        assert_eq!(queue.take_retired(), None, "not until it is gone");
        run(&mut queue, 1_220, 1_400);
        assert_eq!(queue.take_retired(), Some((id, "a", Retired::Expired)));
        assert!(queue.is_empty());
        assert_eq!(queue.cards().count(), 0);
    }

    #[test]
    fn hovering_pauses_the_countdown_for_exactly_as_long_as_it_lasts() {
        let mut queue = queue();
        let id = queue.push(at(0), lifetime("a", 1_000));
        let mut now = 0;
        while now < 1_600 {
            now += 10;
            let hovered = (500 < now && now <= 800).then_some(id);
            let view = queue.update(at(now), hovered, tall);
            if now == 900 {
                assert_eq!(view.next_deadline, Some(ms(1_520)));
            }
            if now == 1_510 {
                assert_eq!(phase(&queue, id), Some(Phase::Shown));
            }
            if now == 1_520 {
                assert_eq!(phase(&queue, id), Some(Phase::Leaving));
            }
        }
    }

    #[test]
    fn hover_does_not_pause_when_the_style_says_not_to() {
        let mut queue = NoticeQueue::new(StackStyle {
            pause_on_hover: false,
            ..StackStyle::default()
        });
        let id = queue.push(at(0), lifetime("a", 1_000));
        let mut now = 0;
        while now < 1_220 {
            now += 10;
            queue.update(at(now), Some(id), tall);
        }
        assert_eq!(phase(&queue, id), Some(Phase::Leaving));
    }

    #[test]
    fn a_sticky_notice_never_expires() {
        let mut queue = queue();
        let id = queue.push(at(0), Notice::new("s").sticky());
        let view = run(&mut queue, 0, 400);
        assert_eq!(view.next_deadline, None);
        let view = queue.update(at(3_600_000), None, tall);
        assert_eq!(phase(&queue, id), Some(Phase::Shown));
        assert_eq!(view.next_deadline, None);
        assert!(!view.animating);
    }

    #[test]
    fn at_most_max_visible_are_shown_and_the_rest_come_in_as_room_frees() {
        let mut queue = queue();
        let pushed: Vec<_> = ["a", "b", "c", "d", "e", "f"]
            .into_iter()
            .map(|text| queue.push(at(0), Notice::new(text)))
            .collect();
        run(&mut queue, 0, 300);
        assert_eq!(queue.len(), 6);
        assert_eq!(ids(&queue), ["d", "c", "b", "a"], "newest nearest");
        queue.dismiss(at(300), pushed[2]);
        let view = queue.update(at(300), None, tall);
        assert!(view.animating && view.changed);
        // The oldest waiting notice starts in at once; the dismissed one
        // keeps its place until it is out.
        assert_eq!(ids(&queue), ["e", "d", "c", "b", "a"]);
        assert_eq!(phase(&queue, pushed[4]), Some(Phase::Entering));
        assert_eq!(phase(&queue, pushed[2]), Some(Phase::Leaving));
        run(&mut queue, 300, 600);
        assert_eq!(ids(&queue), ["e", "d", "b", "a"]);
        assert_eq!(queue.len(), 5);
        assert_eq!(
            queue.take_retired(),
            Some((pushed[2], "c", Retired::Dismissed))
        );
    }

    #[test]
    fn higher_priority_sits_nearer_the_corner_and_newer_within_a_priority() {
        let mut queue = NoticeQueue::new(StackStyle {
            max_visible: 10,
            ..StackStyle::default()
        });
        queue.push(at(0), Notice::new("low").priority(Priority::Low));
        queue.push(at(0), Notice::new("critical").priority(Priority::Critical));
        queue.push(at(0), Notice::new("normal 1"));
        queue.push(at(0), Notice::new("high").priority(Priority::High));
        queue.push(at(0), Notice::new("normal 2"));
        run(&mut queue, 0, 500);
        assert_eq!(
            ids(&queue),
            ["critical", "high", "normal 2", "normal 1", "low"]
        );
        let tops: Vec<f32> = queue.cards().map(|card| card.rect.y).collect();
        assert_eq!(tops, [0.0, 58.0, 116.0, 174.0, 232.0]);
    }

    #[test]
    fn a_notice_that_outranks_a_full_stack_sends_the_lowest_back_to_wait() {
        let mut queue = NoticeQueue::new(StackStyle {
            max_visible: 2,
            ..StackStyle::default()
        });
        let a = queue.push(at(0), lifetime("a", 1_000));
        let b = queue.push(at(0), lifetime("b", 1_000));
        run(&mut queue, 0, 500);
        let c = queue.push(at(500), lifetime("c", 1_000).priority(Priority::High));
        queue.update(at(500), None, tall);
        assert_eq!(ids(&queue), ["c", "b", "a"]);
        assert_eq!(phase(&queue, a), Some(Phase::Leaving));
        assert_eq!(phase(&queue, c), Some(Phase::Entering));
        run(&mut queue, 500, 600);
        queue.dismiss(at(600), c);
        run(&mut queue, 600, 680);
        // `a` finished leaving at 680, went back to wait, and came straight
        // back in, since `c` left room.
        assert_eq!(phase(&queue, a), Some(Phase::Entering));
        assert_eq!(queue.take_retired(), None, "a was never retired");
        run(&mut queue, 680, 800);
        assert_eq!(queue.take_retired(), Some((c, "c", Retired::Dismissed)));
        // `b` was never displaced: in at 220, out at 1220.
        run(&mut queue, 800, 1_220);
        assert_eq!(phase(&queue, b), Some(Phase::Leaving));
        // `a` had 720 ms left; it was fully back in at 900, so it runs out
        // at 1620.
        run(&mut queue, 1_220, 1_610);
        assert_eq!(phase(&queue, a), Some(Phase::Shown));
        run(&mut queue, 1_610, 1_620);
        assert_eq!(phase(&queue, a), Some(Phase::Leaving));
        run(&mut queue, 1_620, 2_000);
        assert_eq!(queue.take_retired(), Some((b, "b", Retired::Expired)));
        assert_eq!(queue.take_retired(), Some((a, "a", Retired::Expired)));
        assert!(queue.is_empty());
    }

    #[test]
    fn a_notice_that_does_not_outrank_a_full_stack_waits() {
        let mut queue = NoticeQueue::new(StackStyle {
            max_visible: 1,
            ..StackStyle::default()
        });
        queue.push(at(0), Notice::new("a").priority(Priority::High));
        let b = queue.push(at(0), Notice::new("b").priority(Priority::High));
        queue.update(at(0), None, tall);
        assert_eq!(ids(&queue), ["a"]);
        assert_eq!(queue.len(), 2);
        assert_eq!(phase(&queue, b), None);
    }

    #[test]
    fn a_coalescing_notice_replaces_the_live_one_in_place() {
        let mut queue = queue();
        let a = queue.push(at(0), lifetime("2 messages", 1_000).coalesce(7));
        let b = queue.push(at(100), lifetime("other", 10_000));
        run(&mut queue, 0, 500);
        assert_eq!(ids(&queue), ["other", "2 messages"]);
        let again = queue.push(at(500), lifetime("3 messages", 1_000).coalesce(7));
        assert_eq!(again, a, "the same id");
        assert_eq!(
            queue.take_retired(),
            Some((a, "2 messages", Retired::Replaced))
        );
        assert_eq!(queue.len(), 2);
        let view = queue.update(at(500), None, tall);
        assert!(view.changed);
        assert_eq!(ids(&queue), ["other", "3 messages"], "the same place");
        let fresh: Vec<bool> = queue.cards().map(|card| card.fresh).collect();
        assert_eq!(fresh, [false, true]);
        assert_eq!(phase(&queue, a), Some(Phase::Shown), "no re-entry");
        queue.update(at(510), None, tall);
        assert!(queue.cards().all(|card| !card.fresh));
        // The lifetime started again at 500.
        run(&mut queue, 510, 1_490);
        assert_eq!(phase(&queue, a), Some(Phase::Shown));
        run(&mut queue, 1_490, 1_500);
        assert_eq!(phase(&queue, a), Some(Phase::Leaving));
        assert_eq!(phase(&queue, b), Some(Phase::Shown));
    }

    #[test]
    fn a_waiting_notice_coalesces_too_but_a_retiring_one_does_not() {
        let mut queue = NoticeQueue::new(StackStyle {
            max_visible: 1,
            ..StackStyle::default()
        });
        queue.push(at(0), Notice::new("front"));
        let waiting = queue.push(at(0), Notice::new("v1").coalesce(1));
        assert_eq!(queue.push(at(5), Notice::new("v2").coalesce(1)), waiting);
        assert_eq!(
            queue.take_retired(),
            Some((waiting, "v1", Retired::Replaced))
        );
        assert_eq!(queue.payload_mut(waiting).map(|p| *p), Some("v2"));

        let mut queue = queue_with_one_leaving();
        let fresh = queue.push(at(150), Notice::new("new").coalesce(9));
        assert_ne!(fresh.get(), 0, "a retiring notice is not live");
    }

    fn queue_with_one_leaving() -> NoticeQueue<&'static str> {
        let mut queue = queue();
        let old = queue.push(at(0), Notice::new("old").coalesce(9));
        queue.update(at(100), None, tall);
        queue.dismiss(at(100), old);
        queue
    }

    #[test]
    fn dismissing_a_card_lets_it_leave_and_then_retires_it() {
        let mut queue = queue();
        let id = queue.push(at(0), Notice::new("a"));
        run(&mut queue, 0, 300);
        assert!(queue.dismiss(at(300), id));
        assert!(!queue.dismiss(at(300), id), "already going");
        queue.update(at(300), None, tall);
        assert_eq!(phase(&queue, id), Some(Phase::Leaving));
        run(&mut queue, 300, 390);
        let card = queue.cards().next().expect("still on screen");
        assert!(card.opacity > 0.0 && card.opacity < 1.0);
        assert!(card.rect.x > -320.0, "sliding out to the right");
        run(&mut queue, 390, 480);
        assert_eq!(queue.take_retired(), Some((id, "a", Retired::Dismissed)));
        assert!(!queue.dismiss(at(500), id), "unknown now");
    }

    #[test]
    fn resolving_with_an_action_hands_the_action_back() {
        let mut queue = queue();
        let id = queue.push(at(0), Notice::new("invite"));
        run(&mut queue, 0, 300);
        assert!(queue.resolve(at(300), id, Retired::Action("accept".into())));
        run(&mut queue, 300, 500);
        assert_eq!(
            queue.take_retired(),
            Some((id, "invite", Retired::Action("accept".into())))
        );
        assert_eq!(queue.take_retired(), None);
    }

    #[test]
    fn a_waiting_notice_is_retired_at_once() {
        let mut queue = NoticeQueue::new(StackStyle {
            max_visible: 0,
            ..StackStyle::default()
        });
        let id = queue.push(at(0), Notice::new("held"));
        assert_eq!(queue.cards().count(), 0);
        assert!(queue.dismiss(at(10), id));
        assert_eq!(queue.take_retired(), Some((id, "held", Retired::Dismissed)));
        assert!(queue.is_empty());
    }

    #[test]
    fn the_stack_closes_a_gap_continuously_even_when_it_changes_its_mind() {
        let mut queue = queue();
        let mut height = |_: &&str| 100.0;
        let a = queue.push(at(0), Notice::new("a").sticky());
        let b = queue.push(at(0), Notice::new("b").sticky());
        let c = queue.push(at(0), Notice::new("c").sticky());
        queue.update(at(0), None, &mut height);
        let y_of_a = |queue: &NoticeQueue<&str>| {
            queue
                .cards()
                .find(|card| card.id == a)
                .map(|card| card.rect.y)
                .expect("a is shown")
        };
        let frame = Duration::from_nanos(16_666_667);
        let mut now = Duration::ZERO;
        let mut last = None;
        let mut largest_step = 0.0_f32;
        for index in 0..120 {
            now += frame;
            if index == 30 {
                queue.dismiss(Some(now), c);
            }
            if index == 36 {
                // c is still leaving; b goes too, so a's reflow is retargeted
                // while it is under way.
                queue.dismiss(Some(now), b);
            }
            queue.update(Some(now), None, &mut height);
            let y = y_of_a(&queue);
            if let Some(last) = last {
                largest_step = largest_step.max(f32::abs(y - last));
            }
            last = Some(y);
        }
        assert_eq!(last, Some(0.0), "a ends nearest the corner");
        // Cubic ease out covers at most 3 * distance / duration a second:
        // about 30 px in a frame for the 122 px a still had to go when it was
        // retargeted. A jump would be at least one 108 px slot.
        assert!(largest_step > 1.0 && largest_step < 32.0, "{largest_step}");
    }

    #[test]
    fn with_no_clock_everything_settles_at_once_and_nothing_expires() {
        let mut queue = queue();
        let a = queue.push(None, lifetime("a", 10));
        let b = queue.push(None, lifetime("b", 10));
        let view = queue.update(None, None, tall);
        assert!(!view.animating && view.changed);
        assert_eq!(view.next_deadline, None);
        for card in queue.cards() {
            assert_eq!(
                (card.phase, card.opacity, card.scale),
                (Phase::Shown, 1.0, 1.0)
            );
        }
        let tops: Vec<f32> = queue.cards().map(|card| card.rect.y).collect();
        assert_eq!(tops, [0.0, 58.0]);
        for _ in 0..10 {
            queue.update(None, None, tall);
        }
        assert_eq!(queue.cards().count(), 2, "no clock, no expiry");
        assert!(queue.dismiss(None, b));
        assert_eq!(queue.take_retired(), Some((b, "b", Retired::Dismissed)));
        let view = queue.update(None, None, tall);
        assert!(!view.animating);
        let card = queue.cards().next().expect("a");
        assert_eq!((card.id, card.rect.y), (a, 0.0), "reflowed at once");
    }

    #[test]
    fn with_no_clock_a_freed_slot_is_filled_at_once() {
        let mut queue = NoticeQueue::new(StackStyle {
            max_visible: 1,
            ..StackStyle::default()
        });
        let a = queue.push(None, Notice::new("a"));
        let b = queue.push(None, Notice::new("b"));
        queue.dismiss(None, a);
        queue.update(None, None, tall);
        assert_eq!(ids(&queue), ["b"]);
        assert_eq!(phase(&queue, b), Some(Phase::Shown));
        // And a notice that outranks it displaces it at once.
        queue.push(None, Notice::new("c").priority(Priority::Critical));
        queue.update(None, None, tall);
        assert_eq!(ids(&queue), ["c"]);
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn the_next_deadline_is_the_earliest_expiry_of_an_unhovered_card() {
        let mut queue = queue();
        let view = queue.update(at(0), None, tall);
        assert_eq!(view.next_deadline, None, "nothing pending");
        let long = queue.push(at(0), lifetime("long", 3_000));
        let short = queue.push(at(100), lifetime("short", 1_000));
        let view = queue.update(at(100), None, tall);
        // Counting starts once each is in: 220 and 320.
        assert_eq!(view.next_deadline, Some(ms(1_320)));
        let view = run(&mut queue, 100, 500);
        assert!(!view.animating);
        assert_eq!(view.next_deadline, Some(ms(1_320)));
        let view = queue.update(at(510), Some(short), tall);
        assert_eq!(view.next_deadline, Some(ms(3_220)), "the hovered one waits");
        let view = queue.update(at(520), Some(long), tall);
        assert_eq!(view.next_deadline, Some(ms(1_330)), "paused 10 ms");
    }

    #[test]
    fn hit_finds_the_card_under_a_point_but_not_one_leaving() {
        let mut queue = queue();
        let older = queue.push(at(0), Notice::new("older"));
        let newer = queue.push(at(0), Notice::new("newer"));
        run(&mut queue, 0, 300);
        assert_eq!(queue.hit([-10.0, 10.0]), Some(newer));
        assert_eq!(queue.hit([-319.0, 49.0]), Some(newer));
        assert_eq!(queue.hit([-10.0, 54.0]), None, "in the gap");
        assert_eq!(queue.hit([-10.0, 70.0]), Some(older));
        assert_eq!(queue.hit([10.0, 10.0]), None, "off the screen edge");
        assert_eq!(queue.hit([-330.0, 10.0]), None);
        queue.dismiss(at(300), newer);
        queue.update(at(310), None, tall);
        assert_eq!(queue.hit([-100.0, 10.0]), None);
    }

    #[test]
    fn clearing_dismisses_everything() {
        let mut queue = NoticeQueue::new(StackStyle {
            max_visible: 1,
            ..StackStyle::default()
        });
        let a = queue.push(at(0), Notice::new("a"));
        let b = queue.push(at(0), Notice::new("b"));
        run(&mut queue, 0, 300);
        queue.clear(at(300));
        assert_eq!(queue.take_retired(), Some((b, "b", Retired::Dismissed)));
        assert_eq!(queue.take_retired(), None, "a is still leaving");
        run(&mut queue, 300, 500);
        assert_eq!(queue.take_retired(), Some((a, "a", Retired::Dismissed)));
        assert!(queue.is_empty());
    }

    #[test]
    fn a_card_is_fresh_for_exactly_one_update() {
        let mut queue = queue();
        queue.push(at(0), Notice::new("a"));
        assert!(
            queue.cards().all(|card| !card.fresh),
            "not before an update"
        );
        queue.update(at(0), None, tall);
        assert!(queue.cards().all(|card| card.fresh));
        let view = queue.update(at(16), None, tall);
        assert!(view.changed);
        assert!(queue.cards().all(|card| !card.fresh));
    }

    #[test]
    fn a_waiting_card_is_fresh_when_it_first_comes_on_screen() {
        let mut queue = NoticeQueue::new(StackStyle {
            max_visible: 1,
            ..StackStyle::default()
        });
        let a = queue.push(at(0), Notice::new("a"));
        queue.push(at(0), Notice::new("b"));
        run(&mut queue, 0, 100);
        queue.dismiss(at(100), a);
        queue.update(at(100), None, tall);
        let fresh: Vec<(&str, bool)> = queue.cards().map(|c| (*c.payload, c.fresh)).collect();
        assert_eq!(fresh, [("b", true), ("a", false)]);
    }

    #[test]
    fn each_corner_lays_the_stack_into_the_screen_and_slides_towards_its_edge() {
        let cases = [
            (Corner::TopRight, [-320.0, 0.0], [-320.0, 58.0], -317.0),
            (Corner::TopLeft, [0.0, 0.0], [0.0, 58.0], -3.0),
            (
                Corner::BottomRight,
                [-320.0, -50.0],
                [-320.0, -108.0],
                -317.0,
            ),
            (Corner::BottomLeft, [0.0, -50.0], [0.0, -108.0], -3.0),
        ];
        for (corner, first, second, half_in_x) in cases {
            let mut queue = NoticeQueue::new(StackStyle {
                corner,
                ..StackStyle::default()
            });
            queue.push(at(0), Notice::new("older"));
            queue.push(at(0), Notice::new("newer"));
            queue.update(at(0), None, tall);
            // Half way through coming in: cubic ease out is seven eighths
            // in, so an eighth of the 24 px slide is left.
            queue.update(at(110), None, tall);
            let card = queue.cards().next().expect("a card");
            assert!((card.rect.x - half_in_x).abs() < 1e-3, "{corner:?}");
            assert!((card.opacity - 0.875).abs() < 1e-6);
            assert!((card.scale - 0.995).abs() < 1e-6);
            run(&mut queue, 110, 400);
            let rects: Vec<RectF> = queue.cards().map(|card| card.rect).collect();
            assert_eq!(*queue.cards().next().unwrap().payload, "newer");
            assert_eq!([rects[0].x, rects[0].y], first, "{corner:?}");
            assert_eq!([rects[1].x, rects[1].y], second, "{corner:?}");
            assert_eq!((rects[0].w, rects[0].h), (320.0, 50.0));
            // A point just inside the corner hits the nearest card.
            let inside = [
                if first[0] < 0.0 { -1.0 } else { 1.0 },
                if first[1] < 0.0 { -1.0 } else { 1.0 },
            ];
            assert_eq!(queue.hit(inside).map(|id| id.get()), Some(1), "{corner:?}");
        }
    }

    #[test]
    fn a_payload_can_be_edited_in_place_and_its_card_grows() {
        let mut queue = NoticeQueue::new(StackStyle::default());
        let a = queue.push(at(0), Notice::new(String::from("short")));
        queue.push(at(0), Notice::new(String::from("x")));
        let measure = |text: &String| 10.0 * text.len() as f32;
        run_strings(&mut queue, 300, measure);
        queue.payload_mut(a).expect("a").push_str(" and longer");
        let view = queue.update(at(310), None, measure);
        assert!(view.changed);
        let a_card = queue.cards().find(|card| card.id == a).unwrap();
        assert_eq!(a_card.rect.h, 160.0);
        assert_eq!(queue.payload_mut(NoticeId(99)), None);
    }

    fn run_strings(queue: &mut NoticeQueue<String>, to: u64, measure: impl Fn(&String) -> f32) {
        let mut now = 0;
        while now <= to {
            queue.update(at(now), None, &measure);
            now += 10;
        }
    }

    #[test]
    fn lowering_max_visible_sends_the_extra_cards_back_to_wait() {
        let mut queue = queue();
        for text in ["a", "b", "c"] {
            queue.push(at(0), Notice::new(text));
        }
        run(&mut queue, 0, 300);
        queue.set_style(StackStyle {
            max_visible: 1,
            ..StackStyle::default()
        });
        run(&mut queue, 300, 600);
        assert_eq!(ids(&queue), ["c"]);
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.take_retired(), None);
    }
}
