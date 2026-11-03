//! A multi-line text field: wrapped lines, a caret that moves by line, and
//! vertical scrolling, over the same grapheme-safe editing state as the
//! single-line [`TextFieldState`].
//!
//! The text is one `String` with `\n` between paragraphs; each paragraph is
//! wrapped to the field's width with the text cache's wrapper. The caret is
//! a byte offset, exactly as in the single-line field, so every edit, the
//! selection, the clipboard and accessibility come from `TextFieldState`
//! unchanged. What this adds is geometry: which line a byte is on, which
//! byte an x-position on a line is nearest to, and keeping the caret's line
//! in view.

use std::ops::Range;

use crate::widget::{ScrollState, TextFieldState, Ui};
use liminalkit_raster::Rect;
use liminalkit_shell::{KeyInput, Modifiers};
use liminalkit_text::TextStyle;
use unicode_segmentation::UnicodeSegmentation;

/// A multi-line field's state, owned by the screen.
#[derive(Debug, Default)]
pub struct TextAreaState {
    field: TextFieldState,
    scroll: ScrollState,
    /// The x the caret was last placed at by typing or a click, kept across
    /// Up/Down so moving through a short line does not lose the column.
    preferred_x: Option<i32>,
    caret_area: Option<Rect>,
}

impl TextAreaState {
    pub fn new() -> TextAreaState {
        TextAreaState::default()
    }

    pub fn with_text(text: impl Into<String>) -> TextAreaState {
        TextAreaState {
            field: TextFieldState::with_text(text),
            ..TextAreaState::default()
        }
    }

    pub fn text(&self) -> &str {
        self.field.text()
    }

    /// The editing state underneath, for anything the single-line field
    /// can do: selection, caret, clipboard errors.
    pub fn field(&self) -> &TextFieldState {
        &self.field
    }

    pub fn field_mut(&mut self) -> &mut TextFieldState {
        self.preferred_x = None;
        &mut self.field
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.field.set_text(text);
        self.preferred_x = None;
    }

    /// Insert at the caret, replacing any selection: what an IME `Commit`
    /// delivers.
    pub fn insert_str(&mut self, text: &str) {
        self.field.insert_str(text);
        self.preferred_x = None;
    }

    pub fn is_empty(&self) -> bool {
        self.field.is_empty()
    }

    /// Device-pixel caret rectangle from the last focused render, for IME
    /// candidate placement.
    pub fn caret_area(&self) -> Option<Rect> {
        self.caret_area
    }

    pub fn scroll(&self) -> &ScrollState {
        &self.scroll
    }
}

/// The wrapped lines of a text, as byte ranges that exclude the `\n` that
/// ends a paragraph. `paragraph_end` says whether a line is the last of its
/// paragraph, which decides where a caret exactly at a wrap sits.
struct Lines {
    ranges: Vec<Range<usize>>,
    paragraph_end: Vec<bool>,
}

impl Lines {
    fn line_of(&self, caret: usize) -> usize {
        for (index, range) in self.ranges.iter().enumerate() {
            if caret < range.end || (caret == range.end && self.paragraph_end[index]) {
                return index;
            }
        }
        self.ranges.len().saturating_sub(1)
    }
}

impl Ui<'_> {
    fn wrap_lines(&mut self, text: &str, width: i32, style: TextStyle) -> Lines {
        let mut ranges = Vec::new();
        let mut paragraph_end = Vec::new();
        let mut start = 0;
        for paragraph in text.split('\n') {
            let wrapped = self.text.wrap(paragraph, width.max(1), style);
            let count = wrapped.len();
            for (index, range) in wrapped.iter().enumerate() {
                ranges.push(start + range.start..start + range.end);
                paragraph_end.push(index + 1 == count);
            }
            start += paragraph.len() + 1;
        }
        Lines {
            ranges,
            paragraph_end,
        }
    }

    /// The byte offset within `line` nearest to `x` pixels from its start.
    fn byte_at_x(&mut self, text: &str, line: Range<usize>, x: i32, style: TextStyle) -> usize {
        let slice = &text[line.clone()];
        let mut best = line.start;
        let mut best_distance = x.abs();
        for (offset, _) in slice.grapheme_indices(true).skip(1) {
            let width = self.text.measure(&slice[..offset], style);
            let distance = (width - x).abs();
            if distance < best_distance {
                best = line.start + offset;
                best_distance = distance;
            }
        }
        let full = self.text.measure(slice, style);
        if (full - x).abs() < best_distance {
            best = line.end;
        }
        best
    }

    /// A multi-line field. Returns whether its text changed this frame.
    ///
    /// Enter inserts a line break; Up and Down move by visual line keeping
    /// the column; Home and End go to the ends of the current line; Shift
    /// with any of those extends the selection. A click places the caret.
    /// Everything else, including the clipboard shortcuts, is the
    /// single-line field's behaviour.
    pub fn text_area(
        &mut self,
        state: &mut TextAreaState,
        area: Rect,
        placeholder: &str,
        focused: bool,
    ) -> bool {
        let focused = self.control_focus(area, focused);
        let theme = self.theme;
        let style = theme.body;
        let inner = area.inset(self.px(theme.padding / 2));
        let line_h = self.text.line_height(style).max(1);
        state.caret_area = None;

        // --- input --------------------------------------------------------
        let mut changed = false;
        while let Some(edit) = self.input.take_accessible_text_edit(area) {
            changed |= state.field.apply_accessible_edit(edit);
        }
        let epoch = self.input.interaction_epoch();
        changed |= state.field.finish_clipboard(focused, epoch);

        let (_, dy) = self.input.take_scroll(area);
        if dy != 0.0 {
            let lines = self.wrap_lines(state.field.text(), inner.w, style);
            let content = lines.ranges.len() as i32 * line_h;
            state.scroll.scroll_by(-dy.round() as i32, inner.h, content);
        }

        if focused {
            if let Some((px, py)) = self.input.press_position() {
                if area.contains(px, py) && self.input.take_click(area) {
                    let lines = self.wrap_lines(state.field.text(), inner.w, style);
                    let row = ((py - inner.y + state.scroll.offset()) / line_h)
                        .clamp(0, lines.ranges.len() as i32 - 1)
                        as usize;
                    let line = lines.ranges[row].clone();
                    let x = px - inner.x;
                    let text = state.field.text().to_owned();
                    let at = self.byte_at_x(&text, line, x, style);
                    if self.input.modifiers().shift {
                        state.field.select_to(at);
                    } else {
                        state.field.set_caret(at);
                    }
                    state.preferred_x = Some(x);
                }
            }

            let modifiers = self.input.modifiers();
            let worker = self.input.clipboard().cloned();
            let keys = self.input.take_keys();
            for key in &keys {
                match key {
                    KeyInput::Enter if modifiers.none() || modifiers.shift => {
                        state.field.insert('\n');
                        state.preferred_x = None;
                        changed = true;
                    }
                    KeyInput::Up | KeyInput::Down => {
                        let text = state.field.text().to_owned();
                        let lines = self.wrap_lines(&text, inner.w, style);
                        let row = lines.line_of(state.field.caret());
                        let target = if *key == KeyInput::Up {
                            row.checked_sub(1)
                        } else {
                            (row + 1 < lines.ranges.len()).then_some(row + 1)
                        };
                        let Some(target) = target else {
                            // Past the first or last line: the ends of the text.
                            let at = if *key == KeyInput::Up { 0 } else { text.len() };
                            move_caret(&mut state.field, at, modifiers);
                            continue;
                        };
                        let current = lines.ranges[row].clone();
                        let x = match state.preferred_x {
                            Some(x) => x,
                            None => self
                                .text
                                .measure(&text[current.start..state.field.caret()], style),
                        };
                        let at = self.byte_at_x(&text, lines.ranges[target].clone(), x, style);
                        move_caret(&mut state.field, at, modifiers);
                        state.preferred_x = Some(x);
                    }
                    KeyInput::Home | KeyInput::End => {
                        let text = state.field.text().to_owned();
                        let lines = self.wrap_lines(&text, inner.w, style);
                        let line = lines.ranges[lines.line_of(state.field.caret())].clone();
                        let at = if *key == KeyInput::Home {
                            line.start
                        } else {
                            line.end
                        };
                        move_caret(&mut state.field, at, modifiers);
                        state.preferred_x = None;
                    }
                    _ => {
                        let before = state.field.caret();
                        changed |= state.field.clipboard_keys(
                            std::slice::from_ref(key),
                            modifiers,
                            false,
                            worker.as_ref(),
                            epoch,
                        );
                        if state.field.caret() != before {
                            state.preferred_x = None;
                        }
                    }
                }
            }
        }

        // --- paint ----------------------------------------------------------
        #[cfg(feature = "accessibility")]
        if let Some(semantics) = self.semantics.as_deref_mut() {
            semantics.add_text_input(
                placeholder,
                area,
                focused,
                state.field.text(),
                state.field.selection_anchor(),
                state.field.caret(),
            );
        }
        self.painter.rounded_rect(
            area,
            self.px(theme.corner_radius),
            if focused {
                self.px(theme.focus_width)
            } else {
                self.px(theme.border_width)
            },
            theme.background,
            if focused {
                theme.accent
            } else {
                theme.panel_edge
            },
        );

        let text = state.field.text().to_owned();
        let lines = self.wrap_lines(&text, inner.w, style);
        let content = lines.ranges.len() as i32 * line_h;
        state.scroll.clamp_to(inner.h, content);
        let caret_row = lines.line_of(state.field.caret());
        if focused {
            // Keep the caret's line in view.
            let top = caret_row as i32 * line_h;
            let offset = state.scroll.offset();
            if top < offset {
                state.scroll.scroll_by(top - offset, inner.h, content);
            } else if top + line_h > offset + inner.h {
                state
                    .scroll
                    .scroll_by(top + line_h - inner.h - offset, inner.h, content);
            }
        }
        let offset = state.scroll.offset();

        self.painter.push_clip(inner);
        if text.is_empty() && !focused {
            self.text.draw(
                &mut self.painter,
                placeholder,
                inner.x,
                inner.y,
                style,
                theme.text_dim,
            );
        }
        let selection = state.field.selection_range().filter(|(s, e)| s != e);
        for (row, line) in lines.ranges.iter().enumerate() {
            let y = inner.y + row as i32 * line_h - offset;
            if y + line_h <= inner.y || y >= inner.bottom() {
                continue;
            }
            if let Some((start, end)) = selection {
                let from = start.max(line.start);
                let to = end.min(line.end);
                if from < to || (start <= line.start && end > line.end) {
                    let x0 = inner.x
                        + self
                            .text
                            .measure(&text[line.start..from.max(line.start)], style);
                    let x1 = if end > line.end {
                        inner.right()
                    } else {
                        inner.x + self.text.measure(&text[line.start..to], style)
                    };
                    self.painter
                        .fill_rect(Rect::new(x0, y, (x1 - x0).max(0), line_h), theme.selection);
                }
            }
            self.text.draw(
                &mut self.painter,
                &text[line.clone()],
                inner.x,
                y,
                style,
                theme.text,
            );
        }
        if focused {
            let line = lines.ranges[caret_row].clone();
            let x = inner.x
                + self
                    .text
                    .measure(&text[line.start..state.field.caret()], style);
            let y = inner.y + caret_row as i32 * line_h - offset;
            let caret = Rect::new(x, y, self.px(2).max(1), line_h);
            state.caret_area = Some(caret);
            let composition = self.input.composition();
            if !composition.is_empty() {
                self.text
                    .draw(&mut self.painter, composition, x, y, style, theme.text);
            }
            self.painter.fill_rect(caret, theme.accent);
        }
        self.painter.pop_clip();
        changed
    }
}

fn move_caret(field: &mut TextFieldState, at: usize, modifiers: Modifiers) {
    if modifiers.shift {
        field.select_to(at);
    } else {
        field.set_caret(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::Theme;
    use liminalkit_raster::{Painter, RasterKernel, WindowBuffer};
    use liminalkit_shell::{Input, MouseButton, Scale};
    use liminalkit_text::{font::test_fonts::set, TextCache};

    struct Harness {
        buffer: WindowBuffer,
        text: TextCache,
        kernel: RasterKernel,
        input: Input,
    }

    impl Harness {
        fn new() -> Harness {
            Harness {
                buffer: WindowBuffer::new(300, 300),
                text: TextCache::new(set()),
                kernel: RasterKernel::new(),
                input: Input::new(),
            }
        }

        fn frame<R>(&mut self, run: impl FnOnce(&mut Ui<'_>) -> R) -> R {
            self.buffer.pixels.fill(0);
            let mut ui = Ui::new(
                Painter::new(&mut self.buffer),
                &mut self.text,
                &mut self.input,
                Theme::default(),
                Scale::ONE,
                &mut self.kernel,
            );
            let result = run(&mut ui);
            ui.input.end_frame();
            result
        }

        fn keys(&mut self, keys: &[KeyInput]) {
            for key in keys {
                self.input.key(key.clone());
            }
        }
    }

    const AREA: Rect = Rect::new(10, 10, 120, 100);

    fn typed(text: &str) -> Vec<KeyInput> {
        text.chars().map(KeyInput::Character).collect()
    }

    #[test]
    fn enter_breaks_the_line_and_the_lines_wrap_to_the_width() {
        let mut h = Harness::new();
        let mut state = TextAreaState::new();
        h.keys(&typed("one two"));
        h.keys(&[KeyInput::Enter]);
        h.keys(&typed("three"));
        let changed = h.frame(|ui| ui.text_area(&mut state, AREA, "", true));
        assert!(changed);
        assert_eq!(state.text(), "one two\nthree");
        // A long paragraph wraps: more lines than paragraphs.
        state.set_text("alpha beta gamma delta epsilon zeta eta theta iota kappa");
        let rows = h.frame(|ui| {
            let lines = ui.wrap_lines(state.text(), AREA.inset(5).w, ui.theme.body);
            lines.ranges.len()
        });
        assert!(rows > 1, "{rows} rows");
    }

    #[test]
    fn up_and_down_move_between_lines_keeping_the_column() {
        let mut h = Harness::new();
        let mut state = TextAreaState::with_text("abcdef\nab\nabcdef");
        // Caret at the end: last line, column 6.
        h.keys(&[KeyInput::Up]);
        h.frame(|ui| ui.text_area(&mut state, AREA, "", true));
        assert_eq!(
            state.field().caret(),
            "abcdef\nab".len(),
            "short line: its end"
        );
        h.keys(&[KeyInput::Up]);
        h.frame(|ui| ui.text_area(&mut state, AREA, "", true));
        assert_eq!(
            state.field().caret(),
            6,
            "the column comes back on a long line"
        );
        h.keys(&[KeyInput::Down, KeyInput::Down]);
        h.frame(|ui| ui.text_area(&mut state, AREA, "", true));
        assert_eq!(state.field().caret(), state.text().len());
        h.keys(&[KeyInput::Down]);
        h.frame(|ui| ui.text_area(&mut state, AREA, "", true));
        assert_eq!(
            state.field().caret(),
            state.text().len(),
            "past the last line stays at the end"
        );
        h.keys(&[KeyInput::Up, KeyInput::Up, KeyInput::Up]);
        h.frame(|ui| ui.text_area(&mut state, AREA, "", true));
        assert_eq!(
            state.field().caret(),
            0,
            "past the first line goes to the start"
        );
    }

    #[test]
    fn home_and_end_work_on_the_current_line_and_shift_extends_the_selection() {
        let mut h = Harness::new();
        let mut state = TextAreaState::with_text("first\nsecond");
        h.keys(&[KeyInput::Home]);
        h.frame(|ui| ui.text_area(&mut state, AREA, "", true));
        assert_eq!(state.field().caret(), "first\n".len());
        h.input.modifiers_changed(Modifiers {
            shift: true,
            ..Modifiers::default()
        });
        h.keys(&[KeyInput::End]);
        h.frame(|ui| ui.text_area(&mut state, AREA, "", true));
        assert_eq!(state.field().selected_text(), Some("second"));
        h.keys(&[KeyInput::Up]);
        h.frame(|ui| ui.text_area(&mut state, AREA, "", true));
        assert_eq!(
            state.field().selected_text(),
            Some("\n"),
            "anchor at the start of 'second', caret up to the end of 'first'"
        );
    }

    #[test]
    fn a_click_places_the_caret_on_the_clicked_line() {
        let mut h = Harness::new();
        let mut state = TextAreaState::with_text("first\nsecond\nthird");
        let line_h = h.text.line_height(Theme::default().body);
        let inner = AREA.inset(5);
        h.input
            .cursor_moved(inner.x as f32 + 1.0, inner.y as f32 + line_h as f32 * 1.5);
        h.input.mouse(MouseButton::Left, true);
        h.input.mouse(MouseButton::Left, false);
        h.frame(|ui| ui.text_area(&mut state, AREA, "", true));
        assert_eq!(state.field().caret(), "first\n".len());
        let caret = state.caret_area().expect("focused caret");
        assert_eq!(caret.y, inner.y + line_h);
    }

    #[test]
    fn the_caret_line_is_scrolled_into_view() {
        let mut h = Harness::new();
        let text: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let mut state = TextAreaState::with_text(text);
        h.frame(|ui| ui.text_area(&mut state, AREA, "", true));
        assert!(
            state.scroll().offset() > 0,
            "the caret is at the end, far below the field"
        );
        let caret = state.caret_area().unwrap();
        assert!(
            caret.y >= AREA.y && caret.bottom() <= AREA.bottom(),
            "{caret:?}"
        );
        h.keys(&[KeyInput::Home]);
        h.input.modifiers_changed(Modifiers {
            control: true,
            ..Modifiers::default()
        });
        h.keys(&[KeyInput::Character('a')]);
        h.frame(|ui| ui.text_area(&mut state, AREA, "", true));
        assert!(state.field().has_selection(), "ctrl-a still selects all");
    }

    #[test]
    fn an_unfocused_empty_area_shows_its_placeholder_and_changes_nothing() {
        let mut h = Harness::new();
        let mut state = TextAreaState::new();
        h.keys(&typed("ignored"));
        let changed = h.frame(|ui| ui.text_area(&mut state, AREA, "notes", false));
        assert!(!changed);
        assert!(state.is_empty());
        assert_eq!(state.caret_area(), None);
    }
}
