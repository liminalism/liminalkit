//! Widgets, without a widget tree.
//!
//! Immediate mode: the screen is rebuilt from scratch every frame, and a
//! widget is a function that draws itself and answers a question — was this
//! clicked, which tab is showing, which row is selected.
//!
//! # No identity system
//!
//! The usual cost of immediate mode is identity. A framework has to store
//! scroll offsets and cursor positions for callers it knows nothing about, so
//! it hashes a call path or a string into a map and hopes the hash is stable.
//! Every scheme has a failure: a counter shifts state to the wrong widget the
//! moment a conditional appears; a `&'static str` collides when two tables
//! share a screen; a call-path hash collides for every row of a loop.
//!
//! There is no such map here. A screen is a struct that owns its own widget
//! state — `scroll: ScrollState`, `selected: Option<CashSessionId>` — and the
//! widget functions take `&mut` to it. That is type-checked, has no collision
//! class at all, survives reordering, and makes every widget testable without
//! drawing anything: build the state, feed it input, assert on the state.
//!
//! It works because this is six screens somebody controls, not a framework for
//! callers who have not been written yet.
//!
//! # What is worth testing
//!
//! Behaviour, at the state level: scroll clamping at both ends, selection
//! surviving a resort, a click at a point landing on the right row given the
//! scroll offset, a text cursor that never lands inside a Thai cluster. Not
//! "pixel (417, 92) is #1a2b3c" — when that fails it says nothing about what
//! broke. Whether it *looks* right is what the golden screens are for.

use std::borrow::Cow;

use liminalkit_raster::{Painter, RasterKernel, Rect};
use liminalkit_shell::{Input, KeyInput, Scale};
use liminalkit_text::{Align, FaceId, TextCache, TextStyle};
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

/// Colours and sizes, in one place so a screen does not invent its own.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: u32,
    pub panel: u32,
    pub panel_alt: u32,
    pub panel_edge: u32,
    pub text: u32,
    pub text_dim: u32,
    /// For a figure that is bad news — short, negative, unexplained.
    pub alarm: u32,
    pub alarm_soft: u32,
    pub good: u32,
    pub good_soft: u32,
    pub accent: u32,
    pub accent_soft: u32,
    /// A row under the cursor.
    pub hover: u32,
    /// The selected row.
    pub selection: u32,
    pub row_height: i32,
    /// Body text.
    /// Logical em size on construction; `Ui::new` converts its theme copy to device pixels.
    pub body: TextStyle,
    /// Logical em size on construction; `Ui::new` converts its theme copy to device pixels.
    /// Headings.
    pub heading: TextStyle,
    pub padding: i32,
    pub corner_radius: i32,
    pub border_width: i32,
    /// Logical thickness of keyboard/assistive focus outlines.
    pub focus_width: i32,
}

impl Default for Theme {
    fn default() -> Theme {
        Theme {
            // Dark, because a back office is often a corner of a kitchen with
            // a window behind the screen.
            background: 0x0016_1a1f,
            panel: 0x001e_242b,
            panel_alt: 0x001a_2026,
            panel_edge: 0x002c_343d,
            text: 0x00e8_edf2,
            text_dim: 0x0090_9aa5,
            alarm: 0x00e0_6c5f,
            alarm_soft: 0x003b_2928,
            good: 0x006f_c28a,
            good_soft: 0x0024_3a2d,
            accent: 0x004d_9fd6,
            accent_soft: 0x0021_3847,
            hover: 0x0027_2f38,
            selection: 0x0032_4a5e,
            row_height: 30,
            body: TextStyle::new(FaceId(0), 17.0),
            heading: TextStyle::new(FaceId(0), 22.0),
            padding: 10,
            corner_radius: 7,
            border_width: 1,
            focus_width: 1,
        }
    }
}

/// What a widget draws into, and what it asks about input.
pub struct Ui<'a> {
    pub painter: Painter<'a>,
    pub text: &'a mut TextCache,
    pub input: &'a mut Input,
    /// Effective theme: font styles are device pixels; dimensional tokens remain logical.
    pub theme: Theme,
    /// Logical→physical; widgets that take logical sizes go through it.
    pub scale: Scale,
    /// Scratch for anti-aliased shapes.
    pub kernel: &'a mut RasterKernel,
    focus: Option<&'a mut crate::Focus>,
    #[cfg(feature = "accessibility")]
    pub(crate) semantics: Option<&'a mut crate::semantics::Semantics>,
}

impl<'a> Ui<'a> {
    pub fn new(
        painter: Painter<'a>,
        text: &'a mut TextCache,
        input: &'a mut Input,
        mut theme: Theme,
        scale: Scale,
        kernel: &'a mut RasterKernel,
    ) -> Ui<'a> {
        theme.body.size *= scale.0;
        theme.heading.size *= scale.0;
        Ui {
            painter,
            text,
            input,
            theme,
            scale,
            kernel,
            focus: None,
            #[cfg(feature = "accessibility")]
            semantics: None,
        }
    }

    /// Convert a custom logical text style once before measuring or drawing.
    /// `Ui::theme` font styles are already physical and must not pass through this.
    pub fn text_style(&self, mut logical: TextStyle) -> TextStyle {
        logical.size *= self.scale.0;
        logical
    }

    /// Opt into standard-control Tab traversal. Reset Focus when the page/row identity changes.
    pub fn with_focus(mut self, focus: &'a mut crate::Focus) -> Self {
        focus.handle_tab(self.input.keys(), self.input.modifiers());
        self.focus = Some(focus);
        self
    }

    /// Register a custom-painted actionable control in the existing frame focus order.
    pub fn control_focus(&mut self, area: Rect, explicit: bool) -> bool {
        match &mut self.focus {
            Some(focus) => focus.register_at(self.input, area),
            None => explicit,
        }
    }

    #[cfg(feature = "accessibility")]
    pub fn with_semantics(mut self, semantics: &'a mut crate::semantics::Semantics) -> Self {
        self.semantics = Some(semantics);
        self
    }
    /// Only the modal overlay, not the painted background, remains actionable.
    #[cfg(feature = "accessibility")]
    pub fn modal_semantics(&mut self) {
        if let Some(semantics) = &mut self.semantics {
            semantics.clear_controls();
        }
    }

    /// Describe a custom-painted control using the same current-frame action adapter.
    #[cfg(feature = "accessibility")]
    pub fn semantic(
        &mut self,
        role: crate::semantics::Role,
        label: &str,
        area: Rect,
        focused: bool,
        checked: Option<bool>,
        value: Option<&str>,
    ) {
        if let Some(semantics) = &mut self.semantics {
            semantics.add(role, label, area, focused, checked, value);
        }
    }
    /// Logical pixels to physical, through the frame's scale.
    #[inline]
    pub fn px(&self, logical: i32) -> i32 {
        self.scale.px(logical)
    }

    /// A rounded surface, returning the area inside its content padding.
    pub fn surface(&mut self, area: Rect, fill: u32, edge: u32) -> Rect {
        self.painter.rounded_rect(
            area,
            self.theme.corner_radius,
            self.theme.border_width,
            fill,
            edge,
        );
        area.inset(self.theme.padding)
    }

    /// A panel with a border, returning the area inside it.
    pub fn panel(&mut self, area: Rect) -> Rect {
        self.surface(area, self.theme.panel, self.theme.panel_edge)
    }

    /// One line of text, vertically centred in `area`.
    pub fn label(&mut self, area: Rect, value: &str, colour: u32, align: Align) {
        let style = self.theme.body;
        self.label_styled(area, value, style, colour, align);
    }

    /// One line of text in an explicit device-pixel style, vertically centred in `area`.
    pub fn label_styled(
        &mut self,
        area: Rect,
        value: &str,
        style: TextStyle,
        colour: u32,
        align: Align,
    ) {
        let size = style;
        let height = self.text.line_height(size);
        let y = area.y + (area.h - height) / 2;
        self.text
            .draw_fitted(&mut self.painter, value, area, y, size, colour, align);
    }

    pub fn heading(&mut self, area: Rect, value: &str) {
        let size = self.theme.heading;
        let height = self.text.line_height(size);
        let y = area.y + (area.h - height) / 2;
        let colour = self.theme.text;
        self.text
            .draw_fitted(&mut self.painter, value, area, y, size, colour, Align::Left);
    }

    /// A clickable button. Returns whether it was pressed this frame.
    pub fn button(&mut self, area: Rect, label: &str) -> bool {
        let focused = self.control_focus(area, false);
        self.button_focused(area, label, focused)
    }

    /// A button with caller-managed keyboard focus.
    pub fn button_focused(&mut self, area: Rect, label: &str, focused: bool) -> bool {
        #[cfg(feature = "accessibility")]
        self.semantic(
            crate::semantics::Role::Button,
            label,
            area,
            focused,
            None,
            None,
        );
        let pressed = self.input.pressing(area);
        let hovered = self.input.hovering(area);
        let fill = if pressed {
            self.theme.selection
        } else if hovered {
            self.theme.hover
        } else {
            self.theme.panel
        };
        self.painter.rounded_rect(
            area,
            self.theme.corner_radius,
            if focused {
                self.px(self.theme.focus_width)
            } else {
                self.theme.border_width
            },
            fill,
            if focused {
                self.theme.accent
            } else {
                self.theme.panel_edge
            },
        );
        self.label(area, label, self.theme.text, Align::Centre);
        self.input.take_click(area)
            || (focused
                && self.input.consume_keys(|keys| {
                    keys.iter()
                        .any(|key| matches!(key, KeyInput::Enter | KeyInput::Character(' ')))
                }))
    }

    /// A row of tabs. Returns true if the selection changed.
    ///
    /// The state is a plain index owned by the screen, so a screen with two
    /// tab strips has two fields and no way for them to be confused.
    pub fn tabs(&mut self, selected: &mut usize, area: Rect, labels: &[&str]) -> bool {
        if labels.is_empty() {
            return false;
        }
        // An index that has outlived the list it indexes — a screen rebuilt
        // with fewer tabs — snaps back rather than panicking.
        *selected = (*selected).min(labels.len() - 1);

        let width = area.w / labels.len() as i32;
        let mut changed = false;
        for (index, label) in labels.iter().enumerate() {
            let tab = Rect::new(area.x + width * index as i32, area.y, width, area.h);
            let active = index == *selected;
            let fill = if active {
                self.theme.selection
            } else if self.input.hovering(tab) {
                self.theme.hover
            } else {
                self.theme.panel
            };
            self.painter.rounded_rect(
                tab.inset(2),
                self.theme.corner_radius,
                self.theme.border_width,
                fill,
                self.theme.panel_edge,
            );
            let colour = if active {
                self.theme.text
            } else {
                self.theme.text_dim
            };
            self.label(tab, label, colour, Align::Centre);
            if self.input.take_click(tab) && !active {
                *selected = index;
                changed = true;
            }
        }
        changed
    }

    /// A horizontal bar, for a share of a total.
    ///
    /// `fraction` is clamped, because a share computed from figures that do
    /// not quite agree should draw a full bar rather than run off the panel.
    pub fn bar(&mut self, area: Rect, fraction: f64, colour: u32) {
        self.painter
            .fill_rounded_rect(area, area.h / 2, self.theme.hover);
        let fraction = fraction.clamp(0.0, 1.0);
        let width = (area.w as f64 * fraction).round() as i32;
        if width > 0 {
            self.painter.fill_rounded_rect(
                Rect::new(area.x, area.y, width, area.h),
                area.h / 2,
                colour,
            );
        }
    }

    /// A compact status label with a quiet tinted surface.
    pub fn badge(&mut self, area: Rect, label: &str, text: u32, fill: u32, edge: u32) {
        let badge = area.inset(4);
        self.painter.rounded_rect(
            badge,
            (self.theme.corner_radius - 1).max(0),
            self.theme.border_width,
            fill,
            edge,
        );
        self.label(badge.inset(4), label, text, Align::Centre);
    }

    /// A labelled progress meter. The track and label are drawn without an
    /// off-screen layer; callers can put exact quantities in `label`.
    pub fn meter(&mut self, area: Rect, fraction: f64, label: &str, colour: u32) {
        let meter = area.inset(5);
        self.bar(meter, fraction, colour);
        self.label(area, label, self.theme.text, Align::Centre);
    }
}

// --- scrolling --------------------------------------------------------------

/// How far a region has been scrolled, and how far it may be.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScrollState {
    /// Pixels hidden above the top of the viewport. Never negative.
    offset: i32,
}

impl ScrollState {
    pub fn new() -> ScrollState {
        ScrollState::default()
    }

    pub fn offset(&self) -> i32 {
        self.offset
    }

    /// The furthest this can scroll given a viewport and its content.
    ///
    /// Zero when everything fits, so a short list cannot be scrolled at all —
    /// content that drifts off the top and cannot be brought back is the most
    /// common bug in a hand-written scroll region.
    pub fn max_offset(viewport: i32, content: i32) -> i32 {
        (content - viewport).max(0)
    }

    /// Move by a delta and clamp. Returns the offset actually applied.
    pub fn scroll_by(&mut self, delta: i32, viewport: i32, content: i32) -> i32 {
        let before = self.offset;
        self.offset = (self.offset + delta).clamp(0, ScrollState::max_offset(viewport, content));
        self.offset - before
    }

    /// Bring a band of content into view, moving as little as possible.
    pub fn reveal(&mut self, top: i32, height: i32, viewport: i32, content: i32) {
        let max = ScrollState::max_offset(viewport, content);
        if top < self.offset {
            self.offset = top;
        } else if top + height > self.offset + viewport {
            self.offset = top + height - viewport;
        }
        self.offset = self.offset.clamp(0, max);
    }

    /// Clamp after the content changed underneath — a filter applied, a day
    /// with fewer rows. Without this a list scrolled to the bottom shows
    /// nothing at all when it gets shorter.
    pub fn clamp_to(&mut self, viewport: i32, content: i32) {
        self.offset = self
            .offset
            .clamp(0, ScrollState::max_offset(viewport, content));
    }
}

// --- tables -----------------------------------------------------------------

/// How wide a column is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    /// Exactly this many pixels — a quantity, a price, a date.
    Fixed(i32),
    /// A share of whatever is left, by weight. Names take these.
    Flex(i32),
}

#[derive(Debug, Clone)]
pub struct Column {
    pub title: Cow<'static, str>,
    pub width: Width,
    pub align: Align,
}

impl Column {
    pub fn fixed(title: &'static str, width: i32, align: Align) -> Column {
        Column {
            title: Cow::Borrowed(title),
            width: Width::Fixed(width),
            align,
        }
    }

    pub fn flex(title: &'static str, weight: i32, align: Align) -> Column {
        Column {
            title: Cow::Borrowed(title),
            width: Width::Flex(weight),
            align,
        }
    }
}

/// How a table cell communicates its value.
#[derive(Debug, Clone, Copy)]
pub enum CellAppearance {
    Text,
    Badge { fill: u32, edge: u32 },
    Meter { fraction: f64, fill: u32 },
}

/// One cell's text and, where it matters, its colour or compact visual.
#[derive(Debug, Clone)]
pub struct Cell<'a> {
    pub text: Cow<'a, str>,
    pub colour: Option<u32>,
    pub appearance: CellAppearance,
}

impl<'a> Cell<'a> {
    pub fn new(text: impl Into<Cow<'a, str>>) -> Cell<'a> {
        Cell {
            text: text.into(),
            colour: None,
            appearance: CellAppearance::Text,
        }
    }

    pub fn coloured(text: impl Into<Cow<'a, str>>, colour: u32) -> Cell<'a> {
        Cell {
            text: text.into(),
            colour: Some(colour),
            appearance: CellAppearance::Text,
        }
    }

    pub fn badge(text: impl Into<Cow<'a, str>>, colour: u32, fill: u32, edge: u32) -> Cell<'a> {
        Cell {
            text: text.into(),
            colour: Some(colour),
            appearance: CellAppearance::Badge { fill, edge },
        }
    }

    pub fn meter(text: impl Into<Cow<'a, str>>, fraction: f64, fill: u32) -> Cell<'a> {
        Cell {
            text: text.into(),
            colour: None,
            appearance: CellAppearance::Meter { fraction, fill },
        }
    }
}

/// A table's scroll and selection, owned by the screen that draws it.
#[derive(Debug, Clone, Default)]
pub struct TableState {
    pub scroll: ScrollState,
    /// The selected row's **key**, not its index.
    ///
    /// An index is wrong the moment the table is re-sorted or filtered: the
    /// selection silently moves to whatever row now sits at that position,
    /// which for a stock table means a manager acting on the wrong
    /// ingredient. A key follows its row.
    selected: Option<String>,
}

impl TableState {
    pub fn new() -> TableState {
        TableState::default()
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn select(&mut self, key: Option<String>) {
        self.selected = key;
    }

    pub fn is_selected(&self, key: &str) -> bool {
        self.selected.as_deref() == Some(key)
    }

    /// Which row is selected, by position, if it is still present.
    pub fn selected_index(&self, keys: &[String]) -> Option<usize> {
        let selected = self.selected.as_deref()?;
        keys.iter().position(|key| key == selected)
    }

    /// Move the selection by `delta` rows, clamped to the ends.
    ///
    /// With nothing selected, a downward move selects the first row and an
    /// upward move the last — so arrowing into an unfocused table does
    /// something obvious from either direction.
    pub fn move_by(&mut self, delta: i32, keys: &[String]) {
        if keys.is_empty() {
            self.selected = None;
            return;
        }
        let next = match self.selected_index(keys) {
            Some(index) => (index as i32 + delta).clamp(0, keys.len() as i32 - 1) as usize,
            None if delta >= 0 => 0,
            None => keys.len() - 1,
        };
        self.selected = Some(keys[next].clone());
    }

    /// Forget a selection whose row is gone — a filter typed, a day changed.
    pub fn prune(&mut self, keys: &[String]) {
        if self
            .selected
            .as_deref()
            .is_some_and(|selected| !keys.iter().any(|key| key == selected))
        {
            self.selected = None;
        }
    }

    fn prune_keyed<'k>(&mut self, row_count: usize, key_at: &impl Fn(usize) -> &'k str) {
        if self
            .selected
            .as_deref()
            .is_some_and(|selected| !(0..row_count).any(|index| key_at(index) == selected))
        {
            self.selected = None;
        }
    }
}

/// A row, built only when it is about to be drawn.
pub struct Row<'a> {
    pub cells: Vec<Cell<'a>>,
}

/// Lay columns out across a width.
///
/// Fixed columns take what they ask for; flex columns share what is left in
/// proportion to their weights, with the remainder going to the last one so
/// the row ends exactly at the right edge rather than a pixel short.
pub fn column_widths(columns: &[Column], total: i32, gap: i32) -> Vec<i32> {
    if columns.is_empty() {
        return Vec::new();
    }
    let gaps = gap * (columns.len() as i32 - 1);
    let fixed: i32 = columns
        .iter()
        .filter_map(|column| match column.width {
            Width::Fixed(width) => Some(width),
            Width::Flex(_) => None,
        })
        .sum();
    let weights: i32 = columns
        .iter()
        .filter_map(|column| match column.width {
            Width::Flex(weight) => Some(weight.max(0)),
            Width::Fixed(_) => None,
        })
        .sum();
    let spare = (total - gaps - fixed).max(0);

    let mut widths: Vec<i32> = columns
        .iter()
        .map(|column| match column.width {
            Width::Fixed(width) => width.max(0),
            Width::Flex(weight) if weights > 0 => spare * weight.max(0) / weights,
            Width::Flex(_) => 0,
        })
        .collect();

    // The remainder, to the last flexible column, so the row ends exactly at
    // the right edge rather than a pixel short of it.
    //
    // Clamped at zero: when the panel is narrower than its fixed columns and
    // gaps there is nothing to distribute, and the correction would be
    // negative. A window dragged very small should crowd its columns, not
    // hand one a negative width for some rectangle to trip over later.
    if weights > 0 {
        let assigned: i32 = widths.iter().sum::<i32>() + gaps;
        let last_flex = columns
            .iter()
            .rposition(|column| matches!(column.width, Width::Flex(_)));
        if let Some(index) = last_flex {
            widths[index] = (widths[index] + total - assigned).max(0);
        }
    }
    widths
}

/// The gap between one column and the next.
pub const COLUMN_GAP: i32 = 8;

/// Reserved down a table's right edge for the scrollbar.
///
/// Always, even when the list fits and no bar is drawn. Taking it only when
/// there is something to scroll would shift every column sideways the moment a
/// row was added, and would let the bar sit on top of the last column — which
/// reads as a rendering fault, because what it actually does is shave the last
/// digit off a price.
pub const TABLE_GUTTER: i32 = 10;

/// What a table did this frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableOutcome {
    /// A row was clicked, and is now selected.
    pub activated: Option<String>,
    /// How many rows the viewport can show.
    pub visible_rows: i32,
}

impl<'a> Ui<'a> {
    /// Draw a scrolling, selectable table.
    ///
    /// Rows are produced by `row`, called only for the ones on screen — a
    /// thousand movements draw thirty. `keys` must list every row in order,
    /// because selection and keyboard movement are by key rather than index.
    pub fn table<'k>(
        &mut self,
        state: &mut TableState,
        area: Rect,
        columns: &[Column],
        keys: &'k [String],
        row: impl FnMut(usize) -> Row<'k>,
    ) -> TableOutcome {
        self.table_keyed(
            state,
            area,
            columns,
            keys.len(),
            |index| keys[index].as_str(),
            row,
        )
    }

    /// Draw a table whose stable keys are borrowed directly from its data.
    ///
    /// This is the report-friendly form: a stock screen no longer has to
    /// clone every ingredient code into a temporary `Vec<String>` on every
    /// repaint merely to support selection.
    pub fn table_keyed<'k>(
        &mut self,
        state: &mut TableState,
        area: Rect,
        columns: &[Column],
        row_count: usize,
        key_at: impl Fn(usize) -> &'k str,
        mut row: impl FnMut(usize) -> Row<'k>,
    ) -> TableOutcome {
        let theme = self.theme;
        let content_width = (area.w - TABLE_GUTTER).max(0);
        let widths = column_widths(columns, content_width, COLUMN_GAP);
        let (header, body) = area.split_top(theme.row_height);

        // Header
        let mut x = area.x;
        for (column, width) in columns.iter().zip(&widths) {
            let cell = Rect::new(x, header.y, *width, header.h);
            self.label(cell, &column.title, theme.text_dim, column.align);
            x += width + COLUMN_GAP;
        }
        self.painter
            .horizontal_rule(area.x, header.bottom() - 1, area.w, theme.panel_edge);

        let content = row_count as i32 * theme.row_height;
        // Before anything is drawn: the list may have shrunk since last frame,
        // and a table scrolled past its own end shows nothing at all.
        state.scroll.clamp_to(body.h, content);
        state.prune_keyed(row_count, &key_at);

        let (_, scrolled) = self.input.take_scroll(body);
        if scrolled != 0.0 {
            // Negative wheel delta means the content moves up.
            state
                .scroll
                .scroll_by(-scrolled.round() as i32, body.h, content);
        }

        let offset = state.scroll.offset();
        let first = (offset / theme.row_height).max(0) as usize;
        let last = (((offset + body.h) / theme.row_height) as usize + 1).min(row_count);

        let mut outcome = TableOutcome {
            activated: None,
            visible_rows: body.h / theme.row_height,
        };

        self.painter.push_clip(body);
        #[allow(clippy::needless_range_loop)]
        // Indexed rather than iterated: the row closure is called by index,
        // and the range is the visible window rather than the whole slice.
        for index in first..last {
            let top = body.y + index as i32 * theme.row_height - offset;
            let line = Rect::new(body.x, top, body.w, theme.row_height);
            if !self.painter.is_visible(line) {
                continue;
            }

            let key = key_at(index);
            let selected = state.is_selected(key);
            // Hover is tested against the row clipped to the viewport, so the
            // half of a row hanging below the fold does not light up when the
            // cursor is over whatever is drawn beneath the table.
            let hit = line.intersect(body);
            if selected {
                self.painter.fill_rect(line, theme.selection);
            } else if self.input.hovering(hit) {
                self.painter.fill_rect(line, theme.hover);
            }

            // Built here, not before the loop: a table of a thousand rows
            // draws thirty, and materialising the rest would allocate a
            // string per cell per frame for content nobody can see.
            let drawn = row(index);
            let mut x = body.x;
            for ((column, width), cell) in columns.iter().zip(&widths).zip(drawn.cells.iter()) {
                let bounds = Rect::new(x, top, *width, theme.row_height);
                let colour = cell.colour.unwrap_or(theme.text);
                match cell.appearance {
                    CellAppearance::Text => {
                        self.label(bounds, &cell.text, colour, column.align);
                    }
                    CellAppearance::Badge { fill, edge } => {
                        self.badge(bounds, &cell.text, colour, fill, edge);
                    }
                    CellAppearance::Meter { fraction, fill } => {
                        self.meter(bounds, fraction, &cell.text, fill);
                    }
                }
                x += width + COLUMN_GAP;
            }

            if self.input.take_click(hit) {
                let key = key.to_owned();
                state.select(Some(key.clone()));
                outcome.activated = Some(key);
            }
        }
        self.painter.pop_clip();

        // A scrollbar, only when there is somewhere to scroll to.
        let max = ScrollState::max_offset(body.h, content);
        if max > 0 {
            let track = Rect::new(body.right() - TABLE_GUTTER + 3, body.y, 4, body.h);
            self.painter.fill_rect(track, theme.hover);
            let thumb_height = (body.h * body.h / content).max(20);
            let travel = body.h - thumb_height;
            let thumb_top = body.y + (travel * offset / max);
            self.painter.fill_rect(
                Rect::new(track.x, thumb_top, track.w, thumb_height),
                theme.text_dim,
            );
        }
        outcome
    }
}

/// Keyboard handling shared by any list: arrows, page keys, home and end.
///
/// Returns whether the selection moved, so a caller can scroll it into view.
pub fn navigate(state: &mut TableState, keys: &[String], page: i32, pressed: &[KeyInput]) -> bool {
    let before = state.selected().map(str::to_owned);
    for key in pressed {
        match key {
            KeyInput::Up => state.move_by(-1, keys),
            KeyInput::Down => state.move_by(1, keys),
            KeyInput::PageUp => state.move_by(-page.max(1), keys),
            KeyInput::PageDown => state.move_by(page.max(1), keys),
            KeyInput::Home => state.move_by(i32::MIN / 2, keys),
            KeyInput::End => state.move_by(i32::MAX / 2, keys),
            _ => {}
        }
    }
    before.as_deref() != state.selected()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("row-{index}")).collect()
    }

    // --- scrolling ------------------------------------------------------

    #[test]
    fn a_list_that_fits_cannot_be_scrolled() {
        // The commonest hand-written scroll bug: content drifts off the top
        // and cannot be brought back.
        let mut scroll = ScrollState::new();
        assert_eq!(scroll.scroll_by(500, 400, 200), 0);
        assert_eq!(scroll.offset(), 0);
        assert_eq!(ScrollState::max_offset(400, 200), 0);
    }

    #[test]
    fn scrolling_clamps_at_both_ends() {
        let mut scroll = ScrollState::new();
        assert_eq!(scroll.scroll_by(-100, 100, 500), 0, "already at the top");

        scroll.scroll_by(1_000, 100, 500);
        assert_eq!(scroll.offset(), 400, "the last screenful, not past it");

        scroll.scroll_by(50, 100, 500);
        assert_eq!(scroll.offset(), 400);

        scroll.scroll_by(-1_000, 100, 500);
        assert_eq!(scroll.offset(), 0);
    }

    #[test]
    fn a_shorter_list_pulls_the_view_back() {
        // A filter typed while scrolled to the bottom. Without clamping, the
        // table shows blank space and looks broken.
        let mut scroll = ScrollState::new();
        scroll.scroll_by(1_000, 100, 900);
        assert_eq!(scroll.offset(), 800);

        scroll.clamp_to(100, 150);
        assert_eq!(scroll.offset(), 50);

        scroll.clamp_to(100, 40);
        assert_eq!(scroll.offset(), 0, "everything fits now");
    }

    #[test]
    fn revealing_moves_as_little_as_it_must() {
        let mut scroll = ScrollState::new();
        // Below the fold: scroll just far enough that it is the bottom row.
        scroll.reveal(300, 30, 100, 1_000);
        assert_eq!(scroll.offset(), 230);

        // Already visible: nothing moves.
        scroll.reveal(250, 30, 100, 1_000);
        assert_eq!(scroll.offset(), 230);

        // Above: to the top of it, not past.
        scroll.reveal(100, 30, 100, 1_000);
        assert_eq!(scroll.offset(), 100);
    }

    // --- selection ------------------------------------------------------

    #[test]
    fn a_selection_follows_its_row_through_a_resort() {
        // The reason selection is by key. With an index, re-sorting moves the
        // selection to whatever now sits at that position — and on a stock
        // screen that is a manager acting on the wrong ingredient.
        let sorted_by_name = vec!["beef".to_owned(), "pork".to_owned(), "rice".to_owned()];
        let sorted_by_value = vec!["rice".to_owned(), "beef".to_owned(), "pork".to_owned()];

        let mut state = TableState::new();
        state.select(Some("pork".to_owned()));
        assert_eq!(state.selected_index(&sorted_by_name), Some(1));
        assert_eq!(state.selected_index(&sorted_by_value), Some(2));
        assert_eq!(state.selected(), Some("pork"), "still the pork");
    }

    #[test]
    fn a_selection_whose_row_is_gone_is_forgotten() {
        let mut state = TableState::new();
        state.select(Some("pork".to_owned()));
        state.prune(&["beef".to_owned(), "rice".to_owned()]);
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn moving_the_selection_stops_at_the_ends() {
        let keys = keys(3);
        let mut state = TableState::new();

        state.move_by(1, &keys);
        assert_eq!(state.selected(), Some("row-0"), "down selects the first");

        state.move_by(-1, &keys);
        assert_eq!(state.selected(), Some("row-0"), "and stays there");

        state.move_by(10, &keys);
        assert_eq!(state.selected(), Some("row-2"), "not past the end");
    }

    #[test]
    fn arrowing_up_into_an_unselected_table_lands_at_the_bottom() {
        let keys = keys(4);
        let mut state = TableState::new();
        state.move_by(-1, &keys);
        assert_eq!(state.selected(), Some("row-3"));
    }

    #[test]
    fn an_empty_table_has_nothing_to_select() {
        let mut state = TableState::new();
        state.select(Some("gone".to_owned()));
        state.move_by(1, &[]);
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn the_navigation_keys_all_move_the_selection() {
        let keys = keys(50);
        let mut state = TableState::new();

        assert!(navigate(&mut state, &keys, 10, &[KeyInput::Down]));
        assert_eq!(state.selected(), Some("row-0"));

        navigate(&mut state, &keys, 10, &[KeyInput::PageDown]);
        assert_eq!(state.selected(), Some("row-10"));

        navigate(&mut state, &keys, 10, &[KeyInput::End]);
        assert_eq!(state.selected(), Some("row-49"));

        navigate(&mut state, &keys, 10, &[KeyInput::Home]);
        assert_eq!(state.selected(), Some("row-0"));

        assert!(
            !navigate(&mut state, &keys, 10, &[KeyInput::Up]),
            "already at the top, so nothing moved"
        );
        assert!(
            !navigate(&mut state, &keys, 10, &[KeyInput::Character('x')]),
            "a key the table does not use"
        );
    }

    #[test]
    fn several_keys_in_one_frame_all_apply() {
        // Held arrow keys repeat faster than the screen redraws.
        let keys = keys(20);
        let mut state = TableState::new();
        navigate(
            &mut state,
            &keys,
            5,
            &[KeyInput::Down, KeyInput::Down, KeyInput::Down],
        );
        assert_eq!(state.selected(), Some("row-2"));
    }

    // --- columns --------------------------------------------------------

    #[test]
    fn columns_fill_their_width_exactly() {
        // A row that ends a pixel short of the panel looks like a rendering
        // fault, and one that ends a pixel over spills into the scrollbar.
        let columns = vec![
            Column::fixed("code", 100, Align::Left),
            Column::flex("name", 2, Align::Left),
            Column::fixed("value", 90, Align::Right),
            Column::flex("note", 1, Align::Left),
        ];
        for total in [400, 401, 402, 743, 1_000] {
            let widths = column_widths(&columns, total, 8);
            let used: i32 = widths.iter().sum::<i32>() + 8 * 3;
            assert_eq!(used, total, "at {total}px");
            assert!(widths.iter().all(|width| *width >= 0));
        }
    }

    #[test]
    fn flexible_columns_share_in_proportion() {
        let columns = vec![
            Column::flex("a", 1, Align::Left),
            Column::flex("b", 3, Align::Left),
        ];
        let widths = column_widths(&columns, 408, 8);
        assert_eq!(widths, vec![100, 300]);
    }

    #[test]
    fn a_narrow_panel_does_not_produce_negative_columns() {
        // A window dragged very small. Columns get nothing rather than a
        // negative width that would panic a rectangle somewhere later.
        let columns = vec![
            Column::fixed("code", 100, Align::Left),
            Column::flex("name", 1, Align::Left),
        ];
        let widths = column_widths(&columns, 20, 8);
        assert!(widths.iter().all(|width| *width >= 0), "{widths:?}");
        // The fixed column keeps its size and crowds the panel; the flexible
        // one gets nothing, which is the honest answer at 20 pixels.
        assert_eq!(widths, vec![100, 0]);
    }

    #[test]
    fn no_columns_is_not_a_crash() {
        assert!(column_widths(&[], 500, 8).is_empty());
    }

    #[test]
    fn only_fixed_columns_take_exactly_what_they_asked_for() {
        let columns = vec![
            Column::fixed("a", 100, Align::Left),
            Column::fixed("b", 50, Align::Right),
        ];
        assert_eq!(column_widths(&columns, 500, 8), vec![100, 50]);
    }
}

// --- text entry -------------------------------------------------------------

/// An editable line of text, and where the caret sits in it.
///
/// The caret is a **byte offset** on a Unicode extended-grapheme boundary.
/// Movement, selection, deletion and masking share UAX #29 segmentation,
/// including combining marks, emoji modifiers/ZWJ sequences and flag pairs.
/// The stored UTF-8 is never normalized.
#[derive(Default)]
pub struct TextFieldState {
    text: String,
    sensitive: bool,
    caret: usize,
    /// The other end of a selection, when one exists: the range is
    /// `anchor..caret`, normalised. `None` is the overwhelmingly common
    /// case — every caret move and every edit clears it — so unselected
    /// fields behave exactly as before.
    selection_anchor: Option<usize>,
    clipboard_edit: Option<Box<ClipboardEdit>>,
    clipboard_error: Option<String>,
    caret_area: Option<Rect>,
    masked_display: String,
    mask_dirty: bool,
}

#[derive(Debug)]
struct ClipboardEdit {
    task: liminalkit_shell::clipboard::ClipboardTask,
    text: String,
    sensitive: bool,
    caret: usize,
    selection: Option<usize>,
    paste: bool,
    cut: bool,
    interaction_epoch: u64,
}

impl Drop for ClipboardEdit {
    fn drop(&mut self) {
        if self.sensitive {
            zeroize::Zeroize::zeroize(&mut self.text);
        }
    }
}

impl std::fmt::Debug for TextFieldState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TextFieldState")
            .field(
                "text",
                &if self.sensitive {
                    "<redacted>"
                } else {
                    self.text.as_str()
                },
            )
            .field("caret", &self.caret)
            .finish_non_exhaustive()
    }
}

impl Clone for TextFieldState {
    fn clone(&self) -> Self {
        Self {
            text: self.text.clone(),
            caret: self.caret,
            sensitive: self.sensitive,
            selection_anchor: self.selection_anchor,
            mask_dirty: true,
            ..Self::default()
        }
    }
}
impl PartialEq for TextFieldState {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && self.caret == other.caret
            && self.selection_anchor == other.selection_anchor
    }
}
impl Eq for TextFieldState {}

impl TextFieldState {
    pub fn new() -> TextFieldState {
        TextFieldState::default()
    }

    pub fn with_text(text: impl Into<String>) -> TextFieldState {
        let text = text.into();
        TextFieldState {
            caret: text.len(),
            text,
            selection_anchor: None,
            mask_dirty: true,
            ..Self::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Physical caret rectangle from the last focused render, for the host's
    /// IME candidate placement. Layout is converted to physical pixels once.
    pub fn caret_area(&self) -> Option<Rect> {
        self.caret_area
    }

    pub fn apply_with_modifiers(
        &mut self,
        keys: &[KeyInput],
        modifiers: liminalkit_shell::Modifiers,
    ) -> bool {
        let mut changed = false;
        for key in keys {
            if modifiers.shift
                && matches!(
                    key,
                    KeyInput::Left | KeyInput::Right | KeyInput::Home | KeyInput::End
                )
            {
                let anchor = self.selection_anchor.unwrap_or(self.caret);
                self.apply(std::slice::from_ref(key));
                self.selection_anchor = Some(anchor);
            } else {
                changed |= self.apply(std::slice::from_ref(key));
            }
        }
        changed
    }

    /// Last clipboard failure; contains no field contents. Surfaces may show
    /// this inline instead of claiming a copy or paste succeeded.
    pub fn clipboard_error(&self) -> Option<&str> {
        self.clipboard_error.as_deref()
    }

    fn clipboard_keys(
        &mut self,
        keys: &[KeyInput],
        modifiers: liminalkit_shell::Modifiers,
        masked: bool,
        worker: Option<&liminalkit_shell::clipboard::ClipboardWorker>,
        interaction_epoch: u64,
    ) -> bool {
        let mut changed = false;
        for key in keys {
            if (modifiers.control || modifiers.logo) && !modifiers.alt {
                let KeyInput::Character(character) = key else {
                    continue;
                };
                let character = character.to_ascii_lowercase();
                if character == 'a' {
                    self.clipboard_edit = None;
                    self.select_all();
                } else if matches!(character, 'c' | 'x' | 'v') {
                    if masked && character != 'v' {
                        continue;
                    }
                    self.clipboard_edit = None;
                    let operation = match worker {
                        Some(worker) if character == 'v' => worker.paste(),
                        Some(worker) => {
                            let Some(text) = self.selected_text() else {
                                continue;
                            };
                            worker.copy(text.to_owned())
                        }
                        None => Err(liminalkit_shell::clipboard::ClipboardError(
                            "system clipboard not attached".into(),
                        )),
                    };
                    match operation {
                        Ok(task) => {
                            self.clipboard_error = None;
                            self.clipboard_edit = Some(Box::new(ClipboardEdit {
                                task,
                                text: self.text.clone(),
                                caret: self.caret,
                                sensitive: self.sensitive,
                                interaction_epoch,
                                selection: self.selection_anchor,
                                paste: character == 'v',
                                cut: character == 'x',
                            }));
                        }
                        Err(error) => self.clipboard_error = Some(error.to_string()),
                    }
                }
                // Ctrl shortcuts must never insert their letters into a field.
            } else {
                self.clipboard_edit = None;
                changed |= self.apply_with_modifiers(std::slice::from_ref(key), modifiers);
            }
        }
        changed
    }

    fn finish_clipboard(&mut self, focused: bool, interaction_epoch: u64) -> bool {
        if !focused
            || self
                .clipboard_edit
                .as_ref()
                .is_some_and(|edit| edit.interaction_epoch != interaction_epoch)
        {
            self.clipboard_edit = None;
            return false;
        }
        let Some(result) = self
            .clipboard_edit
            .as_ref()
            .and_then(|edit| edit.task.try_result())
        else {
            return false;
        };
        let edit = self
            .clipboard_edit
            .take()
            .expect("completed clipboard edit");
        match result {
            Err(error) => {
                self.clipboard_error = Some(error.to_string());
                false
            }
            Ok(text)
                if edit.text == self.text
                    && edit.caret == self.caret
                    && edit.selection == self.selection_anchor =>
            {
                if edit.paste {
                    if let Some(mut text) = text {
                        self.insert_str(&text);
                        if self.sensitive {
                            zeroize::Zeroize::zeroize(&mut text);
                        }
                        return true;
                    }
                } else if edit.cut {
                    return self.delete_selection();
                }
                false
            }
            Ok(mut text) => {
                if self.sensitive {
                    if let Some(text) = &mut text {
                        zeroize::Zeroize::zeroize(text);
                    }
                }
                false
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn clear(&mut self) {
        if self.sensitive {
            zeroize::Zeroize::zeroize(&mut self.text);
        }
        self.clipboard_edit = None;
        self.text.clear();
        self.masked_display.clear();
        self.mask_dirty = false;
        self.caret = 0;
        self.selection_anchor = None;
    }

    /// Clear owned sensitive buffers, including a pending clipboard edit snapshot.
    /// Marks this field sensitive for Debug/replacement. Its owner must call this on
    /// sensitive page leave/drop; this is not an OS credential or clipboard policy.
    pub fn clear_sensitive(&mut self) {
        self.sensitive = true;
        zeroize::Zeroize::zeroize(&mut self.text);
        self.masked_display.clear();
        self.mask_dirty = false;
        if let Some(edit) = &mut self.clipboard_edit {
            zeroize::Zeroize::zeroize(&mut edit.text);
        }
        self.clipboard_edit = None;
        self.caret = 0;
        self.selection_anchor = None;
        self.caret_area = None;
    }

    /// Move the field value without cloning; cancel and clear any edit snapshot.
    pub fn take_text(&mut self) -> String {
        if let Some(edit) = &mut self.clipboard_edit {
            zeroize::Zeroize::zeroize(&mut edit.text);
        }
        self.clipboard_edit = None;
        self.caret = 0;
        self.selection_anchor = None;
        self.caret_area = None;
        self.masked_display.clear();
        self.mask_dirty = false;
        std::mem::take(&mut self.text)
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        if self.sensitive {
            self.clear_sensitive();
        }
        self.clipboard_edit = None;
        self.text = text.into();
        self.mask_dirty = true;
        self.caret = self.text.len();
        self.selection_anchor = None;
    }

    fn apply_accessible_edit(
        &mut self,
        mut edit: liminalkit_shell::input::AccessibleTextEdit,
    ) -> bool {
        match &mut edit {
            liminalkit_shell::input::AccessibleTextEdit::SetValue(value) => {
                let changed = self.text != *value;
                self.set_text(std::mem::take(value));
                changed
            }
            liminalkit_shell::input::AccessibleTextEdit::SetSelection { anchor, focus } => {
                self.clipboard_edit = None;
                self.selection_anchor = Some(self.snap_grapheme_boundary(*anchor));
                self.caret = self.snap_grapheme_boundary(*focus);
                false
            }
        }
    }

    fn snap_grapheme_boundary(&self, at: usize) -> usize {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        let mut cursor = GraphemeCursor::new(at, self.text.len(), true);
        if cursor
            .is_boundary(&self.text, 0)
            .expect("complete field text")
        {
            at
        } else {
            cursor
                .prev_boundary(&self.text, 0)
                .expect("complete field text")
                .unwrap_or(0)
        }
    }

    /// Select everything, for copy-out or type-over. The caret moves to the
    /// end, matching a native field; [`TextFieldState::clear_selection`]
    /// drops it again.
    pub fn select_all(&mut self) {
        self.clipboard_edit = None;
        self.selection_anchor = Some(0);
        self.caret = self.text.len();
    }

    /// Drop the selection, keeping caret and text.
    pub fn clear_selection(&mut self) {
        self.clipboard_edit = None;
        self.selection_anchor = None;
    }

    /// The selected byte range, ordered, while a selection exists — `None`
    /// once it is dropped. An empty range (select-all of an empty field)
    /// still reports `Some((n, n))`; use [`TextFieldState::has_selection`]
    /// to ask whether anything is actually selected.
    pub fn selection_range(&self) -> Option<(usize, usize)> {
        self.selection_anchor.map(|anchor| {
            if anchor < self.caret {
                (anchor, self.caret)
            } else {
                (self.caret, anchor)
            }
        })
    }

    /// Whether any text is selected: a range exists and is non-empty.
    pub fn has_selection(&self) -> bool {
        self.selection_range()
            .is_some_and(|(start, end)| start != end)
    }

    /// The selected text, or `None` when nothing is selected.
    pub fn selected_text(&self) -> Option<&str> {
        self.selection_range()
            .filter(|(start, end)| start != end)
            .map(|(start, end)| &self.text[start..end])
    }

    /// Remove the selected text, keeping the caret on the resulting cluster boundary. Returns
    /// whether text was removed. Always drops the selection, even an empty
    /// one, so every edit below can call it unconditionally.
    pub fn delete_selection(&mut self) -> bool {
        self.clipboard_edit = None;
        let Some(anchor) = self.selection_anchor.take() else {
            return false;
        };
        let (start, end) = if anchor < self.caret {
            (anchor, self.caret)
        } else {
            (self.caret, anchor)
        };
        self.caret = start;
        if start == end {
            return false;
        }
        self.text.replace_range(start..end, "");
        self.mask_dirty = true;
        self.caret = self.caret_after_edit(self.caret);
        true
    }

    fn previous_cluster(&self, at: usize) -> usize {
        GraphemeCursor::new(at, self.text.len(), true)
            .prev_boundary(&self.text, 0)
            .expect("complete field text")
            .unwrap_or(0)
    }

    fn next_cluster(&self, at: usize) -> usize {
        GraphemeCursor::new(at, self.text.len(), true)
            .next_boundary(&self.text, 0)
            .expect("complete field text")
            .unwrap_or(self.text.len())
    }

    /// Editing can join neighboring clusters; keep the caret after the whole result.
    fn caret_after_edit(&self, at: usize) -> usize {
        let mut cursor = GraphemeCursor::new(at, self.text.len(), true);
        if cursor
            .is_boundary(&self.text, 0)
            .expect("complete field text")
        {
            at
        } else {
            cursor
                .next_boundary(&self.text, 0)
                .expect("complete field text")
                .unwrap_or(self.text.len())
        }
    }

    pub fn move_left(&mut self) {
        self.clear_selection();
        if self.caret == 0 {
            return;
        }
        self.caret = self.previous_cluster(self.caret);
    }

    pub fn move_right(&mut self) {
        self.clear_selection();
        if self.caret >= self.text.len() {
            return;
        }
        self.caret = self.next_cluster(self.caret);
    }

    pub fn home(&mut self) {
        self.clear_selection();
        self.caret = 0;
    }

    pub fn end(&mut self) {
        self.clear_selection();
        self.caret = self.text.len();
    }

    pub fn insert(&mut self, character: char) {
        let mut encoded = [0; 4];
        self.insert_str(character.encode_utf8(&mut encoded));
    }

    /// Insert a whole string — what an IME `Commit` delivers — replacing any
    /// selection first, exactly as single-character [`TextFieldState::insert`]
    /// does. The caret lands after the inserted text.
    pub fn insert_str(&mut self, text: &str) {
        self.clipboard_edit = None;
        let (start, end) = self.selection_range().unwrap_or((self.caret, self.caret));
        self.selection_anchor = None;
        self.text.replace_range(start..end, text);
        self.mask_dirty = true;
        self.caret = self.caret_after_edit(start + text.len());
    }

    /// Delete backwards — the selection if there is one, otherwise the
    /// whole cluster, base and marks together.
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.caret == 0 {
            return;
        }
        let start = self.previous_cluster(self.caret);
        self.text.replace_range(start..self.caret, "");
        self.mask_dirty = true;
        self.caret = self.caret_after_edit(start);
    }

    /// Delete forwards — the selection if there is one, otherwise a
    /// whole cluster, likewise.
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.caret >= self.text.len() {
            return;
        }
        let end = self.next_cluster(self.caret);
        self.text.replace_range(self.caret..end, "");
        self.mask_dirty = true;
        self.caret = self.caret_after_edit(self.caret);
    }

    fn refresh_mask(&mut self) {
        if !self.mask_dirty {
            return;
        }
        self.masked_display.clear();
        self.masked_display
            .extend(std::iter::repeat_n(MASK_GLYPH, cluster_count(&self.text)));
        self.mask_dirty = false;
    }

    /// Apply a frame's keys. Returns whether the text changed.
    pub fn apply(&mut self, keys: &[KeyInput]) -> bool {
        let mut changed = false;
        for key in keys {
            match key {
                KeyInput::Character(character) if !character.is_control() => {
                    self.insert(*character);
                    changed = true;
                }
                KeyInput::Backspace if self.caret > 0 || self.has_selection() => {
                    self.backspace();
                    changed = true;
                }
                KeyInput::Delete if self.caret < self.text.len() || self.has_selection() => {
                    self.delete();
                    changed = true;
                }
                KeyInput::Left => self.move_left(),
                KeyInput::Right => self.move_right(),
                KeyInput::Home => self.home(),
                KeyInput::End => self.end(),
                _ => {}
            }
        }
        changed
    }
}

/// The glyph a masked field shows in place of one grapheme cluster. ASCII, so
/// it renders in any embedded face without a fallback lookup — the one place
/// a password field cannot afford to show a `.notdef` box instead of a dot.
const MASK_GLYPH: char = '*';

/// Count the same extended graphemes used by movement and deletion.
fn cluster_count(text: &str) -> usize {
    text.graphemes(true).count()
}

impl Ui<'_> {
    /// An editable field. Returns whether its text changed this frame.
    ///
    /// `focused` is the screen's own idea of what has focus — another piece of
    /// state a screen owns rather than the toolkit guessing at. For a single
    /// field or two, a plain `bool` the screen tracks is enough. For a form
    /// with many, hand-rolling that per field stops scaling and gives up
    /// keyboard reachability (no way to Tab between them); reach for
    /// [`crate::Focus`] instead and pass it `focus.register()` here.
    ///
    /// `masked` swaps the displayed text (and the caret's measured position)
    /// for mask glyphs and marks [`TextFieldState`] sensitive for Debug and
    /// buffer replacement without changing its stored text. The caret is still a byte offset into the
    /// real text and still only ever lands on a cluster boundary, so masking
    /// cannot desynchronise it from what backspace or the arrow keys do. The
    /// placeholder is shown in the clear either way; it is a label ("PIN"),
    /// not a secret.
    pub fn text_field(
        &mut self,
        state: &mut TextFieldState,
        area: Rect,
        placeholder: &str,
        focused: bool,
        masked: bool,
    ) -> bool {
        let (changed, focused) = self.text_field_input(state, area, focused, masked);
        self.paint_text_field(state, area, placeholder, focused, masked);
        changed
    }

    /// Claim editing/clipboard input before background widgets draw a modal overlay.
    /// Returns `(changed, focused)`; pass that focus to `paint_text_field` once.
    pub fn text_field_input(
        &mut self,
        state: &mut TextFieldState,
        area: Rect,
        focused: bool,
        masked: bool,
    ) -> (bool, bool) {
        state.sensitive |= masked;
        let focused = self.control_focus(area, focused);
        state.caret_area = None;
        let protected = masked || state.sensitive;
        let mut changed = false;
        while let Some(edit) = self.input.take_accessible_text_edit(area) {
            if !protected {
                changed |= state.apply_accessible_edit(edit);
            }
        }
        let interaction_epoch = self.input.interaction_epoch();
        changed |= state.finish_clipboard(focused, interaction_epoch);
        if focused {
            let modifiers = self.input.modifiers();
            let worker = self.input.clipboard().cloned();
            changed |= self.input.consume_keys(|keys| {
                state.clipboard_keys(
                    keys,
                    modifiers,
                    protected,
                    worker.as_ref(),
                    interaction_epoch,
                )
            });
        }
        (changed, focused)
    }

    /// Paint an already-claimed text field without processing its input twice.
    pub fn paint_text_field(
        &mut self,
        state: &mut TextFieldState,
        area: Rect,
        placeholder: &str,
        focused: bool,
        masked: bool,
    ) {
        #[cfg(feature = "accessibility")]
        if masked || state.sensitive {
            self.semantic(
                crate::semantics::Role::PasswordInput,
                placeholder,
                area,
                focused,
                None,
                None,
            );
        } else if let Some(semantics) = self.semantics.as_deref_mut() {
            semantics.add_text_input(
                placeholder,
                area,
                focused,
                state.text(),
                state.selection_anchor.unwrap_or(state.caret),
                state.caret,
            );
        }
        let theme = self.theme;
        self.painter.rounded_rect(
            area,
            theme.corner_radius,
            if focused {
                self.px(theme.focus_width)
            } else {
                theme.border_width
            },
            theme.background,
            if focused {
                theme.accent
            } else {
                theme.panel_edge
            },
        );

        let inner = area.inset(theme.padding / 2);
        if masked {
            state.refresh_mask();
        }
        let display = if masked {
            state.masked_display.as_str()
        } else {
            state.text.as_str()
        };
        if focused {
            // Paint below the glyphs, using their displayed positions even in masked fields.
            if let Some((start, end)) = state.selection_range() {
                if start != end {
                    let (start, end) = if masked {
                        (
                            cluster_count(&state.text[..start]),
                            cluster_count(&state.text[..end]),
                        )
                    } else {
                        (start, end)
                    };
                    let x0 = inner.x + self.text.measure(&display[..start], theme.body);
                    let width = self.text.measure(&display[start..end], theme.body);
                    let left = x0.clamp(inner.x, inner.right());
                    let right = (x0 + width).clamp(left, inner.right());
                    let height = self.text.line_height(theme.body);
                    let top = inner.y + (inner.h - height) / 2;
                    self.painter
                        .fill_rect(Rect::new(left, top, right - left, height), theme.selection);
                }
            }
        }
        if state.is_empty() && !focused {
            self.label(inner, placeholder, theme.text_dim, Align::Left);
        } else {
            self.label(inner, display, theme.text, Align::Left);
        }

        if focused {
            // A caret drawn at the measured width of the text before it, so
            // it sits where the next glyph will land rather than at a guess.
            // Masked, that means the width of *its* dots, not of the real
            // (possibly much narrower or wider) characters they stand for.
            let before = &state.text()[..state.caret()];
            let offset = if masked {
                self.text
                    .measure(&display[..cluster_count(before)], theme.body)
            } else {
                self.text.measure(before, theme.body)
            };
            let height = self.text.line_height(theme.body);
            let top = inner.y + (inner.h - height) / 2;
            state.caret_area = Some(Rect::new(inner.x + offset, top, self.px(2), height));
            let composition = self.input.composition();
            if !composition.is_empty() && !masked {
                self.text.draw_fitted(
                    &mut self.painter,
                    composition,
                    Rect::new(inner.x + offset, top, (inner.w - offset).max(0), height),
                    top,
                    theme.body,
                    theme.text,
                    Align::Left,
                );
            }
            self.painter
                .fill_rect(Rect::new(inner.x + offset, top, 2, height), theme.accent);
        }
    }
}

#[cfg(test)]
mod text_field_tests {
    use super::*;

    #[test]
    fn movement_selection_and_deletion_keep_extended_graphemes_whole() {
        for grapheme in ["e\u{301}", "ก็", "น้ำ", "🧑🏽‍💻", "🇹🇭", "ن\u{651}", "\r\n"]
        {
            let mut field = TextFieldState::with_text(format!("L{grapheme}R"));
            field.home();
            field.move_right();
            field.move_right();
            assert_eq!(field.caret(), 1 + grapheme.len(), "{grapheme:?}");
            field.apply_with_modifiers(
                &[KeyInput::Left],
                liminalkit_shell::Modifiers {
                    shift: true,
                    ..Default::default()
                },
            );
            assert_eq!(field.selected_text(), Some(grapheme), "{grapheme:?}");
            field.delete();
            assert_eq!(field.text(), "LR", "{grapheme:?}");
            let mut field = TextFieldState::with_text(format!("L{grapheme}"));
            field.backspace();
            assert_eq!(field.text(), "L", "{grapheme:?}");
            let mut field = TextFieldState::with_text(format!("{grapheme}R"));
            field.home();
            field.delete();
            assert_eq!(field.text(), "R", "{grapheme:?}");
        }
    }

    #[test]
    fn deletion_and_insertion_keep_caret_valid_when_neighboring_graphemes_merge() {
        let mut field = TextFieldState::with_text("🇦x🇧");
        field.home();
        field.move_right();
        field.apply_with_modifiers(
            &[KeyInput::Right],
            liminalkit_shell::Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        field.delete_selection();
        assert_eq!(field.text(), "🇦🇧");
        assert_eq!(field.caret(), "🇦🇧".len());
        field.backspace();
        assert_eq!(field.text(), "");

        let mut field = TextFieldState::with_text("🇦x🇧");
        field.home();
        field.move_right();
        field.apply_with_modifiers(
            &[KeyInput::Right],
            liminalkit_shell::Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        field.insert_str("y");
        assert_eq!(
            field.text(),
            "🇦y🇧",
            "replacement must not move after a temporary merged flag"
        );
        assert_eq!(field.caret(), "🇦y".len());

        let mut field = TextFieldState::with_text("\u{301}R");
        field.home();
        field.insert('e');
        assert_eq!(field.text(), "e\u{301}R");
        assert_eq!(field.caret(), "e\u{301}".len());
        field.backspace();
        assert_eq!(field.text(), "R");
    }

    #[test]
    fn sensitive_text_transfer_redacts_and_clears_edit_state() {
        let mut field = TextFieldState::with_text("fixture-password");
        field.sensitive = true;
        field.select_all();
        assert!(!format!("{field:?}").contains("fixture-password"));
        let mut value = field.take_text();
        assert!(field.text().is_empty());
        assert_eq!(field.caret(), 0);
        assert_eq!(field.selected_text(), None);
        zeroize::Zeroize::zeroize(&mut value);
        field.set_text("replacement");
        field.select_all();
        field.clear_sensitive();
        assert!(field.text().is_empty());
        assert_eq!(field.caret(), 0);
        assert_eq!(field.selected_text(), None);
        assert!(field.caret_area().is_none());
    }

    #[test]
    fn delete_forwards_removes_a_whole_cluster_too() {
        let mut field = TextFieldState::with_text("ก็ข");
        field.home();
        field.delete();
        assert_eq!(field.text(), "ข", "ก and its mark went together");
        field.delete();
        assert_eq!(field.text(), "");
        field.delete();
        assert_eq!(field.text(), "", "and deleting past the end is harmless");
    }

    #[test]
    fn backspacing_from_the_start_is_harmless() {
        let mut field = TextFieldState::with_text("ข้าว");
        field.home();
        field.backspace();
        assert_eq!(field.text(), "ข้าว");
        assert_eq!(field.caret(), 0);
    }

    #[test]
    fn typing_a_thai_word_reads_back_as_it_was_typed() {
        let mut field = TextFieldState::new();
        for character in "ข้าวผัด".chars() {
            field.insert(character);
        }
        assert_eq!(field.text(), "ข้าวผัด");
        assert_eq!(field.caret(), field.text().len());
    }

    #[test]
    fn a_frame_of_keys_applies_in_order_and_reports_a_change() {
        let mut field = TextFieldState::new();
        let typed: Vec<KeyInput> = "rice".chars().map(KeyInput::Character).collect();
        assert!(field.apply(&typed));
        assert_eq!(field.text(), "rice");

        assert!(field.apply(&[KeyInput::Backspace, KeyInput::Backspace]));
        assert_eq!(field.text(), "ri");

        // Movement is not a change.
        assert!(!field.apply(&[KeyInput::Home, KeyInput::Right, KeyInput::End]));
        assert_eq!(field.text(), "ri");
    }

    #[test]
    fn control_characters_are_not_typed_into_the_field() {
        // Tab arrives with text "\t" on some platforms, and a literal tab in
        // a search box is a character nobody can see or delete.
        let mut field = TextFieldState::new();
        assert!(!field.apply(&[
            KeyInput::Character('\t'),
            KeyInput::Character('\n'),
            KeyInput::Enter,
            KeyInput::Escape,
        ]));
        assert!(field.is_empty());
    }

    #[test]
    fn inserting_in_the_middle_lands_where_the_caret_is() {
        let mut field = TextFieldState::with_text("ac");
        field.move_left();
        field.insert('b');
        assert_eq!(field.text(), "abc");
        assert_eq!(field.caret(), 2);
    }

    #[test]
    fn an_empty_field_moves_nowhere() {
        let mut field = TextFieldState::new();
        field.move_left();
        field.move_right();
        field.backspace();
        field.delete();
        assert!(field.is_empty());
        assert_eq!(field.caret(), 0);
    }

    #[test]
    fn select_all_reports_the_whole_text() {
        let mut field = TextFieldState::with_text("ข้าว");
        assert_eq!(field.selected_text(), None, "nothing selected yet");
        field.select_all();
        assert!(field.has_selection());
        assert_eq!(field.selected_text(), Some("ข้าว"));
        assert_eq!(field.selection_range(), Some((0, "ข้าว".len())));
        assert_eq!(
            field.caret(),
            "ข้าว".len(),
            "caret to the end, like a native field"
        );
    }

    #[test]
    fn select_all_of_an_empty_field_selects_nothing() {
        let mut field = TextFieldState::new();
        field.select_all();
        assert!(!field.has_selection());
        assert_eq!(field.selected_text(), None);
        field.backspace();
        assert!(field.is_empty(), "still harmless");
    }

    #[test]
    fn clear_selection_drops_it_without_moving_the_caret() {
        let mut field = TextFieldState::with_text("rice");
        field.select_all();
        field.clear_selection();
        assert!(!field.has_selection());
        assert_eq!(field.selected_text(), None);
        assert_eq!(field.selection_range(), None);
        assert_eq!(field.text(), "rice");
        assert_eq!(field.caret(), 4);
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut field = TextFieldState::with_text("ac");
        field.select_all();
        field.insert('b');
        assert_eq!(field.text(), "b");
        assert_eq!(field.caret(), 1);
        assert!(!field.has_selection(), "the edit consumed the selection");
    }

    #[test]
    fn committing_a_string_over_a_selection_replaces_it() {
        // What an IME Commit delivers: several characters at once.
        let mut field = TextFieldState::with_text("ac");
        field.select_all();
        field.insert_str("あ不");
        assert_eq!(field.text(), "あ不");
        assert_eq!(field.caret(), "あ不".len());
    }

    #[test]
    fn backspace_with_a_selection_deletes_the_range() {
        let mut field = TextFieldState::with_text("น้ำดี");
        field.select_all();
        field.backspace();
        assert_eq!(field.text(), "");
        assert_eq!(field.caret(), 0);
    }

    #[test]
    fn delete_with_a_selection_deletes_the_range() {
        let mut field = TextFieldState::with_text("ab");
        field.select_all();
        field.delete();
        assert_eq!(field.text(), "");
        assert_eq!(field.caret(), 0);
    }

    #[test]
    fn moving_the_caret_drops_the_selection() {
        let mut field = TextFieldState::with_text("rice");
        field.select_all();
        field.move_left();
        assert!(!field.has_selection());
        field.select_all();
        field.home();
        assert!(!field.has_selection());
        assert_eq!(field.caret(), 0);
    }

    #[test]
    fn plain_edits_leave_no_selection_behind() {
        // The caret semantics existing callers rely on, stated outright:
        // without select_all there is never a selection to trip over.
        let mut field = TextFieldState::with_text("ac");
        field.move_left();
        field.insert('b');
        field.backspace();
        assert_eq!(field.text(), "ac");
        assert!(!field.has_selection());
        assert_eq!(field.caret(), 1);
    }
}

#[cfg(test)]
mod masked_text_field_tests {
    use super::*;
    use liminalkit_raster::WindowBuffer;
    use liminalkit_text::font::test_fonts::set;

    fn area() -> Rect {
        Rect::new(0, 0, 200, 30)
    }

    fn edit_frame(state: &mut TextFieldState, input: &mut Input, masked: bool) -> bool {
        let mut buffer = WindowBuffer::new(200, 30);
        let mut text = TextCache::new(set());
        let mut kernel = RasterKernel::new();
        Ui::new(
            Painter::new(&mut buffer),
            &mut text,
            input,
            Theme::default(),
            Scale::ONE,
            &mut kernel,
        )
        .text_field(state, area(), "Fixture field", true, masked)
    }

    #[test]
    fn accessible_value_and_selection_batch_replaces_only_whole_graphemes() {
        use liminalkit_shell::input::AccessibleTextEdit;
        let value = "Le\u{301}🧑🏽‍💻R";
        let end = value.len() - 1;
        for reverse in [false, true] {
            let mut state = TextFieldState::with_text("old");
            let mut input = Input::new();
            input.set_accessible_text_edit(area(), AccessibleTextEdit::SetValue(value.into()));
            let (anchor, focus) = if reverse { (end, 3) } else { (3, end) };
            input.set_accessible_text_edit(
                area(),
                AccessibleTextEdit::SetSelection { anchor, focus },
            );
            assert!(edit_frame(&mut state, &mut input, false));
            assert_eq!(state.selected_text(), Some("e\u{301}🧑🏽‍💻"));
            assert_eq!(state.caret(), if reverse { 1 } else { end });
            input.end_frame();
            input.key(KeyInput::Character('X'));
            assert!(edit_frame(&mut state, &mut input, false));
            assert_eq!(state.text(), "LXR");
            assert_eq!(state.caret(), 2);
        }
    }

    #[test]
    fn expired_accessible_edit_cannot_modify_same_position_replacement_field() {
        use liminalkit_shell::input::AccessibleTextEdit;
        for boundary in 0..3 {
            let mut input = Input::new();
            input.set_accessible_text_edit(
                area(),
                AccessibleTextEdit::SetValue("retired field".into()),
            );
            match boundary {
                0 => input.end_frame(),
                1 => input.blur(),
                _ => input.clear_accessible_text_edit(),
            }
            let mut replacement = TextFieldState::with_text("current field");
            assert!(!edit_frame(&mut replacement, &mut input, false));
            assert_eq!(replacement.text(), "current field");
            assert_eq!(replacement.caret(), "current field".len());
        }
    }

    #[test]
    fn password_rejects_plain_accessible_edits_even_while_visually_revealed() {
        use liminalkit_shell::input::AccessibleTextEdit;
        let mut state = TextFieldState::with_text("secret fixture");
        let mut input = Input::new();
        edit_frame(&mut state, &mut input, true);
        for masked in [true, false] {
            input.set_accessible_text_edit(
                area(),
                AccessibleTextEdit::SetValue("replacement".into()),
            );
            input.set_accessible_text_edit(
                area(),
                AccessibleTextEdit::SetSelection {
                    anchor: 0,
                    focus: usize::MAX,
                },
            );
            assert!(!edit_frame(&mut state, &mut input, masked));
            assert_eq!(state.text(), "secret fixture");
            assert!(!state.has_selection());
            assert_eq!(state.caret(), "secret fixture".len());
            input.end_frame();
        }
    }

    #[test]
    fn scaled_text_glyphs_and_caret_follow_the_same_device_density() {
        let mut baseline_height = None;
        for factor in [1.0, 1.25, 1.5, 2.0] {
            let scale = Scale(factor);
            let area = Rect::new(0, 0, scale.px(200), scale.px(36));
            let mut buffer = WindowBuffer::new(area.w as u32, area.h as u32);
            let mut text = TextCache::new(set());
            let mut kernel = RasterKernel::new();
            let mut input = Input::new();
            let mut field = TextFieldState::with_text("Density");
            let theme = Theme {
                background: 0,
                text: 0xff_ffff,
                accent: 0x22_4466,
                body: TextStyle::new(FaceId(0), 18.0),
                ..Theme::default()
            };
            Ui::new(
                Painter::new(&mut buffer),
                &mut text,
                &mut input,
                theme,
                scale,
                &mut kernel,
            )
            .text_field(&mut field, area, "", true, false);
            let expected = theme.body.with_size(18.0 * factor);
            let caret = field.caret_area().expect("focused text caret");
            assert_eq!(
                caret.x,
                area.inset(theme.padding / 2).x + text.measure("Density", expected)
            );
            assert_eq!(caret.h, text.line_height(expected));
            let bright_rows: Vec<_> = buffer
                .pixels
                .chunks(buffer.width as usize)
                .enumerate()
                .filter(|(_, row)| {
                    row.iter().any(|pixel| {
                        (pixel >> 16 & 0xff) > 200
                            && (pixel >> 8 & 0xff) > 200
                            && (pixel & 0xff) > 200
                    })
                })
                .map(|(y, _)| y as i32)
                .collect();
            let height = bright_rows.last().expect("visible glyphs") - bright_rows[0] + 1;
            let baseline = *baseline_height.get_or_insert(height);
            assert!(
                (height as f32 - baseline as f32 * factor).abs() <= 2.0,
                "glyph height {height} did not follow density {factor} from {baseline}"
            );
        }
    }

    #[test]
    fn selected_plain_text_and_password_mask_remain_readable() {
        for masked in [false, true] {
            let mut buffer = WindowBuffer::new(200, 30);
            let mut text = TextCache::new(set());
            let mut kernel = RasterKernel::new();
            let mut input = Input::new();
            let mut state = TextFieldState::with_text("Readable");
            state.select_all();
            let theme = Theme {
                background: 0x11_2233,
                text: 0xff_ffff,
                selection: 0x22_4466,
                accent: 0x22_4466,
                ..Theme::default()
            };
            Ui::new(
                Painter::new(&mut buffer),
                &mut text,
                &mut input,
                theme,
                Scale::ONE,
                &mut kernel,
            )
            .text_field(&mut state, area(), "", true, masked);
            assert!(
                buffer.pixels.iter().any(|pixel| (pixel >> 16 & 0xff) > 200
                    && (pixel >> 8 & 0xff) > 200
                    && (pixel & 0xff) > 200),
                "selected {} glyphs were covered by their highlight",
                if masked { "mask" } else { "text" }
            );
        }
    }

    fn render(text: &str, masked: bool, focused: bool) -> WindowBuffer {
        let area = area();
        let mut buffer = WindowBuffer::new(area.w as u32, area.h as u32);
        let mut text_cache = TextCache::new(set());
        let mut kernel = RasterKernel::new();
        let mut input = Input::new();
        let mut state = TextFieldState::with_text(text);
        {
            let mut ui = Ui::new(
                Painter::new(&mut buffer),
                &mut text_cache,
                &mut input,
                Theme::default(),
                Scale::ONE,
                &mut kernel,
            );
            ui.painter.clear(0);
            ui.text_field(&mut state, area, "placeholder", focused, masked);
        }
        buffer
    }

    /// Column of the leftmost pixel matching `colour`, searched only inside
    /// `rect` — the field's focused border is drawn in the same colour as
    /// the caret, so a search of the whole buffer would just find the left
    /// edge of the border every time regardless of where the caret is.
    fn first_column_in(buffer: &WindowBuffer, rect: Rect, colour: u32) -> Option<i32> {
        for x in rect.x..rect.right() {
            for y in rect.y..rect.bottom() {
                if buffer.pixels[y as usize * buffer.width as usize + x as usize] == colour {
                    return Some(x);
                }
            }
        }
        None
    }

    fn caret_x_after_clusters(text: &str, clusters: usize) -> Option<i32> {
        let area = area();
        let mut buffer = WindowBuffer::new(area.w as u32, area.h as u32);
        let mut text_cache = TextCache::new(set());
        let mut kernel = RasterKernel::new();
        let mut input = Input::new();
        let mut state = TextFieldState::with_text(text);
        state.home();
        for _ in 0..clusters {
            state.move_right();
        }
        {
            let mut ui = Ui::new(
                Painter::new(&mut buffer),
                &mut text_cache,
                &mut input,
                Theme::default(),
                Scale::ONE,
                &mut kernel,
            );
            ui.painter.clear(0);
            ui.text_field(&mut state, area, "", true, true);
        }
        // Inside the field's content padding, clear of the focused border.
        let inner = area.inset(Theme::default().padding / 2);
        first_column_in(&buffer, inner, Theme::default().accent)
    }

    #[test]
    fn masking_hides_the_actual_characters() {
        // Two different strings, same length: masked, they must be
        // pixel-for-pixel indistinguishable, or the mask is leaking content.
        let a = render("secret", true, false);
        let b = render("xxxxxx", true, false);
        assert_eq!(a.pixels, b.pixels);
    }

    #[test]
    fn masking_counts_clusters_not_bytes() {
        // ก็ is six bytes and one grapheme cluster (base + a combining tone
        // mark); "a" is one byte and one cluster. Masked, both are a single
        // dot — the same test `deleting` already relies on for backspace.
        let a = render("ก็", true, false);
        let b = render("a", true, false);
        assert_eq!(a.pixels, b.pixels);
    }

    #[test]
    fn masking_actually_changes_what_is_drawn() {
        let masked = render("secret", true, false);
        let plain = render("secret", false, false);
        assert_ne!(masked.pixels, plain.pixels);
    }

    #[test]
    fn an_empty_masked_field_still_shows_its_placeholder_in_the_clear() {
        // The placeholder is a label ("PIN"), not a secret, whether or not
        // the field itself is a masked one.
        let masked_empty = render("", true, false);
        let plain_empty = render("", false, false);
        assert_eq!(masked_empty.pixels, plain_empty.pixels);
    }

    #[test]
    fn the_caret_in_a_masked_field_advances_per_cluster_not_per_byte() {
        let latin_one = caret_x_after_clusters("aXYZ", 1).unwrap();
        let thai_one = caret_x_after_clusters("ก็XYZ", 1).unwrap();
        assert_eq!(
            latin_one, thai_one,
            "one cluster is one dot, whatever its byte length"
        );

        let latin_two = caret_x_after_clusters("aXYZ", 2).unwrap();
        assert!(
            latin_two > latin_one,
            "the caret keeps moving right with each cluster"
        );
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;
    use liminalkit_raster::WindowBuffer;
    use liminalkit_shell::MouseButton;
    use liminalkit_text::font::test_fonts::set;

    const ROW_HEIGHT: i32 = 30;

    fn keys(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("row-{index}")).collect()
    }

    fn columns() -> Vec<Column> {
        vec![
            Column::flex("name", 1, Align::Left),
            Column::fixed("value", 80, Align::Right),
        ]
    }

    /// Draw a table once at a fixed size, with the given input, and report
    /// what it did. The buffer is real, so the drawing code runs.
    fn draw(
        state: &mut TableState,
        input: &mut Input,
        keys: &[String],
        area: Rect,
    ) -> TableOutcome {
        let mut buffer = WindowBuffer::new(400, 400);
        let mut text = TextCache::new(set());
        let mut kernel = RasterKernel::new();
        let theme = Theme {
            row_height: ROW_HEIGHT,
            ..Theme::default()
        };
        let mut ui = Ui::new(
            Painter::new(&mut buffer),
            &mut text,
            input,
            theme,
            Scale::ONE,
            &mut kernel,
        );
        ui.table(state, area, &columns(), keys, |index| Row {
            cells: vec![Cell::new(format!("ข้าว {index}")), Cell::new("฿70.00")],
        })
    }

    #[test]
    fn clicking_a_row_selects_the_one_under_the_cursor() {
        // The area is at y=0 with a header of one row height, so the body
        // starts at 30 and the first row occupies 30..60.
        let area = Rect::new(0, 0, 300, 330);
        let keys = keys(20);
        let mut state = TableState::new();

        let mut input = Input::new();
        input.cursor_moved(50.0, 75.0); // 45px into the body: row 1
        input.mouse(MouseButton::Left, true);

        let outcome = draw(&mut state, &mut input, &keys, area);
        assert_eq!(outcome.activated.as_deref(), Some("row-1"));
        assert_eq!(state.selected(), Some("row-1"));
    }

    #[test]
    fn a_click_lands_on_the_right_row_after_scrolling() {
        // The bug this exists for: hit-testing that ignores the scroll offset
        // selects a different row from the one under the cursor, and only
        // once the list has been scrolled — so it survives every test that
        // starts at the top.
        let area = Rect::new(0, 0, 300, 330);
        let keys = keys(50);
        let mut state = TableState::new();

        // Scroll down by four rows, then click the first visible one.
        let mut input = Input::new();
        input.cursor_moved(50.0, 100.0);
        input.scrolled(0.0, -(4.0 * ROW_HEIGHT as f32));
        draw(&mut state, &mut input, &keys, area);
        input.end_frame();
        assert_eq!(state.scroll.offset(), 4 * ROW_HEIGHT);

        let mut input = Input::new();
        input.cursor_moved(50.0, 45.0); // 15px into the body
        input.mouse(MouseButton::Left, true);
        let outcome = draw(&mut state, &mut input, &keys, area);

        assert_eq!(
            outcome.activated.as_deref(),
            Some("row-4"),
            "the row under the cursor, not the fourth from the top of the data"
        );
    }

    #[test]
    fn clicking_below_the_last_row_selects_nothing() {
        let area = Rect::new(0, 0, 300, 330);
        let keys = keys(2);
        let mut state = TableState::new();

        let mut input = Input::new();
        input.cursor_moved(50.0, 250.0);
        input.mouse(MouseButton::Left, true);

        let outcome = draw(&mut state, &mut input, &keys, area);
        assert_eq!(outcome.activated, None);
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn a_click_on_the_header_is_not_a_click_on_a_row() {
        let area = Rect::new(0, 0, 300, 330);
        let keys = keys(10);
        let mut state = TableState::new();

        let mut input = Input::new();
        input.cursor_moved(50.0, 10.0);
        input.mouse(MouseButton::Left, true);

        assert_eq!(draw(&mut state, &mut input, &keys, area).activated, None);
    }

    #[test]
    fn a_table_of_a_thousand_rows_builds_only_what_it_draws() {
        // The whole reason rows come from a closure. Building a thousand
        // rows' worth of strings per frame is what makes a software-rendered
        // table unusable on a Pi.
        let area = Rect::new(0, 0, 300, 330);
        let keys = keys(1_000);
        let mut state = TableState::new();
        let mut input = Input::new();

        let mut buffer = WindowBuffer::new(400, 400);
        let mut text = TextCache::new(set());
        let mut kernel = RasterKernel::new();
        let theme = Theme {
            row_height: ROW_HEIGHT,
            ..Theme::default()
        };
        let mut built = 0usize;
        {
            let mut ui = Ui::new(
                Painter::new(&mut buffer),
                &mut text,
                &mut input,
                theme,
                Scale::ONE,
                &mut kernel,
            );
            ui.table(&mut state, area, &columns(), &keys, |_index| {
                built += 1;
                Row {
                    cells: vec![Cell::new("ข้าว"), Cell::new("฿70.00")],
                }
            });
        }
        // A 300px body at 30px a row is ten, plus one straddling the edge.
        assert!(built <= 12, "built {built} rows to draw about ten");
        assert!(built >= 10, "built {built}, which cannot fill the viewport");
    }

    #[test]
    fn scrolling_stops_at_the_bottom_of_the_data() {
        let area = Rect::new(0, 0, 300, 330);
        let keys = keys(15);
        let mut state = TableState::new();

        let mut input = Input::new();
        input.cursor_moved(50.0, 100.0);
        input.scrolled(0.0, -10_000.0);
        draw(&mut state, &mut input, &keys, area);

        // 15 rows of 30 is 450; a 300px body leaves 150 to scroll.
        assert_eq!(state.scroll.offset(), 150);
    }

    #[test]
    fn a_table_that_fits_does_not_scroll_at_all() {
        let area = Rect::new(0, 0, 300, 330);
        let keys = keys(3);
        let mut state = TableState::new();

        let mut input = Input::new();
        input.cursor_moved(50.0, 100.0);
        input.scrolled(0.0, -500.0);
        draw(&mut state, &mut input, &keys, area);

        assert_eq!(state.scroll.offset(), 0);
    }

    #[test]
    fn a_selection_that_leaves_the_data_is_dropped_when_the_table_redraws() {
        let area = Rect::new(0, 0, 300, 330);
        let mut state = TableState::new();
        state.select(Some("row-40".to_owned()));

        let mut input = Input::new();
        draw(&mut state, &mut input, &keys(5), area);
        assert_eq!(state.selected(), None, "a filter removed that row");
    }

    #[test]
    fn an_empty_table_draws_without_complaint() {
        let area = Rect::new(0, 0, 300, 330);
        let mut state = TableState::new();
        let mut input = Input::new();
        let outcome = draw(&mut state, &mut input, &[], area);
        assert_eq!(outcome.activated, None);
        assert_eq!(state.scroll.offset(), 0);
    }

    #[test]
    fn rows_are_clipped_to_the_table_rather_than_drawn_over_what_is_below() {
        // A row half past the bottom edge must be cut, not painted across
        // whatever panel comes next.
        let mut buffer = WindowBuffer::new(400, 400);
        let mut text = TextCache::new(set());
        let mut kernel = RasterKernel::new();
        let mut input = Input::new();
        let mut state = TableState::new();
        let keys = keys(50);
        let area = Rect::new(0, 0, 300, 200);
        state.select(Some("row-40".to_owned()));

        {
            let theme = Theme {
                row_height: ROW_HEIGHT,
                ..Theme::default()
            };
            let mut ui = Ui::new(
                Painter::new(&mut buffer),
                &mut text,
                &mut input,
                theme,
                Scale::ONE,
                &mut kernel,
            );
            ui.table(&mut state, area, &columns(), &keys, |index| Row {
                cells: vec![Cell::new(format!("ข้าวสาร {index}")), Cell::new("฿70.00")],
            });
        }

        // Nothing below the table's own bounds was touched.
        let below = (area.bottom() as usize..400)
            .any(|y| (0..400).any(|x| buffer.pixels[y * 400 + x] != 0));
        assert!(!below, "the table drew past its own bottom edge");
    }
}

#[cfg(test)]
mod gutter_tests {
    use super::*;
    use liminalkit_raster::WindowBuffer;
    use liminalkit_text::font::test_fonts::set;

    /// A table always leaves room for its scrollbar, scrollable or not.
    ///
    /// The bug this fixes was visible rather than logical: the bar was drawn
    /// over the last column and shaved the final digit off every price, so
    /// ฿143.50 read as ฿143.5. Reserving only when scrollable would instead
    /// have shifted every column the moment a row was added.
    #[test]
    fn columns_never_reach_under_the_scrollbar() {
        let columns = vec![
            Column::flex("name", 1, Align::Left),
            Column::fixed("value", 120, Align::Right),
        ];
        for total in [300, 500, 743, 1_100] {
            let widths = column_widths(&columns, total - TABLE_GUTTER, COLUMN_GAP);
            let right_edge: i32 = widths.iter().sum::<i32>() + COLUMN_GAP;
            assert!(
                right_edge <= total - TABLE_GUTTER,
                "at {total}px the columns reach {right_edge}, into the gutter"
            );
        }
    }

    #[test]
    fn a_short_table_and_a_long_one_lay_out_their_columns_identically() {
        // Two frames of the same screen, one row apart. Columns that moved
        // between them would make a list twitch as it filled.
        let columns = vec![
            Column::flex("name", 1, Align::Left),
            Column::fixed("value", 120, Align::Right),
        ];
        let area = Rect::new(0, 0, 400, 200);
        let mut text = TextCache::new(set());
        let mut kernel = RasterKernel::new();

        let mut cell_left = Vec::new();
        for count in [2usize, 200usize] {
            let keys: Vec<String> = (0..count).map(|index| format!("k{index}")).collect();
            let mut buffer = WindowBuffer::new(500, 300);
            let mut input = Input::new();
            let mut state = TableState::new();
            let widths;
            {
                let mut ui = Ui::new(
                    Painter::new(&mut buffer),
                    &mut text,
                    &mut input,
                    Theme::default(),
                    Scale::ONE,
                    &mut kernel,
                );
                ui.table(&mut state, area, &columns, &keys, |_index| Row {
                    cells: vec![Cell::new("ข้าวสาร"), Cell::new("฿143.50")],
                });
                widths = column_widths(&columns, area.w - TABLE_GUTTER, COLUMN_GAP);
            }
            cell_left.push(widths);
        }
        assert_eq!(cell_left[0], cell_left[1]);
    }
}

#[cfg(test)]
mod clipboard_interaction_tests {
    use super::*;
    use liminalkit_shell::Modifiers;

    #[test]
    fn clipboard_shortcut_never_inserts_its_letter_on_failure() {
        let mut state = TextFieldState::with_text("ชื่อ");
        assert!(!state.clipboard_keys(
            &[KeyInput::Character('v')],
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
            false,
            None,
            0
        ));
        assert_eq!(state.text(), "ชื่อ");
        assert_eq!(
            state.clipboard_error(),
            Some("system clipboard not attached")
        );
    }

    #[test]
    fn masked_copy_and_cut_never_reach_clipboard_or_delete_text() {
        let mut state = TextFieldState::with_text("secret");
        state.select_all();
        for key in ['c', 'x'] {
            assert!(!state.clipboard_keys(
                &[KeyInput::Character(key)],
                Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
                true,
                None,
                0
            ));
        }
        assert_eq!(state.text(), "secret");
        assert_eq!(state.clipboard_error(), None);
    }

    #[test]
    fn shift_selection_preserves_thai_cluster_boundaries() {
        let mut state = TextFieldState::with_text("aก็");
        state.apply_with_modifiers(
            &[KeyInput::Left],
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(state.selected_text(), Some("ก็"));
        state.insert_str("ชื่อ😀.txt");
        assert_eq!(state.text(), "aชื่อ😀.txt");
        assert_eq!(state.caret(), state.text().len());
    }
}
