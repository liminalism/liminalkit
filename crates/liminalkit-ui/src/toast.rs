//! A host-independent notification/toast card.
//!
//! Borrowed presentation data in, measured layout and a widget outcome out.
//! The card owns no D-Bus connection, no history, no classification, no
//! timers, no output choice, no Wayland privileges and no window: the host
//! (Templar's shell) owns the visible stack and its next deadline, and the
//! notification service owns lifetime policy. This module is layout, paint,
//! hit-testing, keyboard focus and accessible announcement, and nothing else.
//!
//! Paint and hit-testing share one geometry: [`toast_layout`] measures the
//! card, and [`Ui::toast`] draws exactly what it measured, returning the
//! same [`ToastLayout`] it painted. A host that sizes its window, declares
//! its input region and pauses on hover from that layout cannot disagree
//! with what is on screen.
//!
//! Colours, text styles and radii all come from [`Theme`]: a card drawn for
//! one host looks like that host, not like a second design language.

use liminalkit_raster::{Bitmap, Rect, ScaleFilter};
use liminalkit_shell::{Input, KeyInput, Scale};
use liminalkit_text::{Align, TextCache};

use crate::widget::{Theme, Ui};

/// Compact card width in logical pixels. Templar's shell sizes its
/// Notification-layer host to this; the card itself takes whatever physical
/// width the host measured it at.
pub const TOAST_WIDTH: i32 = 380;
/// Body lines drawn per card. The bound is what keeps a hostile 10-kilobyte
/// body a card rather than a document.
pub const TOAST_MAX_BODY_LINES: usize = 3;
/// Declared actions drawn per card. Further actions stay on the server row.
pub const TOAST_MAX_ACTIONS: usize = 3;

/// Logical size of the icon well, when the card carries an icon.
const ICON_WELL: i32 = 40;
/// Logical size of the dismiss control, top-right of the card.
const DISMISS_SIZE: i32 = 28;
/// Logical height of one action chip.
const ACTION_HEIGHT: i32 = 26;
/// Narrowest chip worth drawing. A chip that would be narrower (a late
/// action, a 100-pixel window) is left out rather than drawn unpressable.
const MIN_CHIP_WIDTH: i32 = 48;

/// One declared action, borrowed from the host's presentation row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToastAction<'a> {
    /// The key the host invokes on the service. Never shown.
    pub key: &'a str,
    /// The label drawn on the chip.
    pub label: &'a str,
}

/// Everything one card presents, borrowed from the host for the frame.
#[derive(Clone, Copy, Debug)]
pub struct ToastCard<'a> {
    /// Sending application, display only. The host verified it, if it did.
    pub app: &'a str,
    /// One fitted line, the card's headline.
    pub summary: &'a str,
    /// Optional wrapped body. Empty draws no body row at all.
    pub body: &'a str,
    /// Optional application icon, blitted into the well. `None` gives the
    /// text the full width rather than drawing a placeholder.
    pub icon: Option<&'a Bitmap>,
    /// Declared actions, in sender order. Only the first
    /// [`TOAST_MAX_ACTIONS`] that fit the row are drawn.
    pub actions: &'a [ToastAction<'a>],
    /// Accessible label for the dismiss control, e.g. `"Dismiss"`.
    pub dismiss_label: &'a str,
}

/// Measured geometry for one card, in physical pixels at an absolute origin.
/// [`toast_layout`] computes it; [`Ui::toast`] paints it and hands it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastLayout {
    /// Card height. The card is `(origin, width × height)`.
    pub height: i32,
    /// The dismiss control. Always present.
    pub dismiss: Rect,
    /// Drawn action chips, parallel to the card's leading actions.
    pub actions: Vec<Rect>,
    /// Body lines measured (and drawn), at most [`TOAST_MAX_BODY_LINES`].
    pub body_lines: usize,
    /// Physical width the app/summary/body lines were fitted into.
    pub text_width: i32,
}

/// What a card's frame asked for. The host dismisses, invokes and retires.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToastOutcome {
    /// The dismiss control was activated.
    pub dismissed: bool,
    /// A declared action was activated: its service key.
    pub action: Option<String>,
}

impl ToastOutcome {
    /// Whether the frame asked for nothing.
    pub fn is_none(&self) -> bool {
        !self.dismissed && self.action.is_none()
    }
}

/// Measure one card: its height, its dismiss rect and its action-chip rects.
/// `theme` carries device-pixel font styles (as [`Ui::theme`] does);
/// dimensional tokens stay logical and go through `scale`. All rects are
/// physical pixels at `origin`, so paint and hit-testing share them exactly.
pub fn toast_layout(
    text: &mut TextCache,
    theme: &Theme,
    scale: Scale,
    origin: (i32, i32),
    width: i32,
    card: &ToastCard<'_>,
) -> ToastLayout {
    let (x, y) = origin;
    let width = width.max(1);
    let pad = scale.px(theme.padding);
    let gap = scale.px(8);

    let dismiss_side = scale.px(DISMISS_SIZE);
    let dismiss = Rect::new(
        x + width - pad - dismiss_side,
        y + pad,
        dismiss_side,
        dismiss_side,
    );

    let icon_side = if card.icon.is_some() {
        scale.px(ICON_WELL)
    } else {
        0
    };
    let text_x = x
        + pad
        + if card.icon.is_some() {
            icon_side + gap
        } else {
            0
        };
    let text_width = (dismiss.x - gap - text_x).max(1);

    let line = text.line_height(theme.body).max(1);
    let body_lines = if card.body.is_empty() {
        0
    } else {
        text.wrap(card.body, text_width, theme.body)
            .len()
            .min(TOAST_MAX_BODY_LINES)
    };
    let mut bottom = y + pad + line + line + body_lines as i32 * line;

    let mut actions = Vec::new();
    if !card.actions.is_empty() {
        let chip_h = scale.px(ACTION_HEIGHT);
        let chip_gap = scale.px(6);
        let row_y = bottom + scale.px(4);
        let mut chip_x = x + pad;
        for action in card.actions.iter().take(TOAST_MAX_ACTIONS) {
            let measured = text.measure(action.label, theme.body) + scale.px(16);
            let room = x + width - pad - chip_x;
            let chip_w = measured.min(room);
            if chip_w < scale.px(MIN_CHIP_WIDTH) {
                break;
            }
            actions.push(Rect::new(chip_x, row_y, chip_w, chip_h));
            chip_x += chip_w + chip_gap;
        }
        if !actions.is_empty() {
            bottom = row_y + chip_h;
        }
    }

    let icon_bottom = if card.icon.is_some() {
        y + pad + icon_side
    } else {
        y
    };
    let height = bottom.max(icon_bottom).max(dismiss.bottom()) + pad - y;
    ToastLayout {
        height: height.max(1),
        dismiss,
        actions,
        body_lines,
        text_width,
    }
}

/// A press or an Enter/Space on a focused control. The same rule every
/// button in this crate answers to.
fn activated(input: &mut Input, area: Rect, focused: bool) -> bool {
    input.take_click(area)
        || (focused
            && input.consume_keys(|keys| {
                keys.iter()
                    .any(|key| matches!(key, KeyInput::Enter | KeyInput::Character(' ')))
            }))
}

impl Ui<'_> {
    /// Draw one notification card at `origin`, `width` physical pixels wide,
    /// and answer what the frame asked of it.
    ///
    /// Tab order is draw order: dismiss first, then the action chips. The
    /// host wires [`Focus`](crate::Focus) through [`Ui::with_focus`] exactly
    /// as for any other control; without it the card is pointer-only, which
    /// is the honest state for a popup that never stole keyboard focus.
    pub fn toast(
        &mut self,
        origin: (i32, i32),
        width: i32,
        card: &ToastCard<'_>,
    ) -> (ToastOutcome, ToastLayout) {
        let theme = self.theme;
        let scale = self.scale;
        let layout = toast_layout(self.text, &theme, scale, origin, width, card);
        let area = Rect::new(origin.0, origin.1, width.max(1), layout.height);
        self.painter.rounded_rect(
            area,
            theme.corner_radius,
            theme.border_width,
            theme.panel,
            theme.panel_edge,
        );
        #[cfg(feature = "accessibility")]
        {
            let label = format!("{}: {}", card.app, card.summary);
            let body = (!card.body.is_empty()).then_some(card.body);
            self.semantic(
                crate::semantics::Role::Alert,
                &label,
                area,
                false,
                None,
                body,
            );
        }

        let pad = self.px(theme.padding);
        let gap = self.px(8);
        let line = self.text.line_height(theme.body).max(1);
        let mut text_y = origin.1 + pad;
        let text_x = origin.0
            + pad
            + if card.icon.is_some() {
                self.px(ICON_WELL) + gap
            } else {
                0
            };
        if let Some(icon) = card.icon {
            let side = self.px(ICON_WELL);
            self.painter.blit(
                icon,
                Rect::new(origin.0 + pad, origin.1 + pad, side, side),
                ScaleFilter::Bilinear,
            );
        }
        let lines = Rect::new(text_x, text_y, layout.text_width, line);
        self.label_styled(lines, card.app, theme.body, theme.text_dim, Align::Left);
        text_y += line;
        let lines = Rect::new(text_x, text_y, layout.text_width, line);
        self.label_styled(lines, card.summary, theme.body, theme.text, Align::Left);
        text_y += line;
        if layout.body_lines > 0 {
            let wrapped = self.text.wrap(card.body, layout.text_width, theme.body);
            for (index, range) in wrapped.iter().take(layout.body_lines).enumerate() {
                let top = text_y + index as i32 * line;
                self.text.draw(
                    &mut self.painter,
                    &card.body[range.clone()],
                    text_x,
                    top,
                    theme.body,
                    theme.text_dim,
                );
            }
        }

        let mut outcome = ToastOutcome::default();
        let dismiss_focused = self.control_focus(layout.dismiss, false);
        #[cfg(feature = "accessibility")]
        self.semantic(
            crate::semantics::Role::Button,
            card.dismiss_label,
            layout.dismiss,
            dismiss_focused,
            None,
            None,
        );
        let dismiss_fill = if self.input.pressing(layout.dismiss) {
            theme.selection
        } else if self.input.hovering(layout.dismiss) {
            theme.hover
        } else {
            theme.panel
        };
        self.painter.rounded_rect(
            layout.dismiss,
            theme.corner_radius,
            if dismiss_focused {
                self.px(theme.focus_width)
            } else {
                theme.border_width
            },
            dismiss_fill,
            if dismiss_focused {
                theme.accent
            } else {
                theme.panel_edge
            },
        );
        self.label(layout.dismiss, "×", theme.text, Align::Centre);
        if activated(&mut self.input, layout.dismiss, dismiss_focused) {
            outcome.dismissed = true;
        }

        for (index, chip) in layout.actions.iter().enumerate() {
            let Some(action) = card.actions.get(index) else {
                continue;
            };
            let focused = self.control_focus(*chip, false);
            #[cfg(feature = "accessibility")]
            self.semantic(
                crate::semantics::Role::Button,
                action.label,
                *chip,
                focused,
                None,
                None,
            );
            let fill = if self.input.pressing(*chip) {
                theme.selection
            } else if self.input.hovering(*chip) {
                theme.hover
            } else {
                theme.panel
            };
            self.painter.rounded_rect(
                *chip,
                theme.corner_radius,
                if focused {
                    self.px(theme.focus_width)
                } else {
                    theme.border_width
                },
                fill,
                if focused {
                    theme.accent
                } else {
                    theme.panel_edge
                },
            );
            self.label(*chip, action.label, theme.text, Align::Centre);
            if activated(&mut self.input, *chip, focused) {
                outcome.action = Some(action.key.to_owned());
            }
        }
        (outcome, layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liminalkit_raster::WindowBuffer;
    use liminalkit_text::font::test_fonts;

    fn card<'a>(
        body: &'a str,
        icon: Option<&'a Bitmap>,
        actions: &'a [ToastAction<'a>],
    ) -> ToastCard<'a> {
        ToastCard {
            app: "Terminal",
            summary: "Job finished",
            body,
            icon,
            actions,
            dismiss_label: "Dismiss",
        }
    }

    fn cache() -> TextCache {
        TextCache::new(test_fonts::set())
    }

    struct Harness {
        buffer: WindowBuffer,
        text: TextCache,
        input: Input,
        kernel: liminalkit_raster::RasterKernel,
        theme: Theme,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                buffer: WindowBuffer::new(480, 800),
                text: cache(),
                input: Input::new(),
                kernel: liminalkit_raster::RasterKernel::new(),
                theme: Theme::default(),
            }
        }

        fn frame(
            &mut self,
            focus: Option<&mut crate::Focus>,
            keys: &[KeyInput],
            click: Option<(i32, i32)>,
            card: &ToastCard<'_>,
        ) -> (ToastOutcome, ToastLayout) {
            for key in keys {
                self.input.key(key.clone());
            }
            if let Some((x, y)) = click {
                #[allow(clippy::cast_precision_loss)]
                self.input.cursor_moved(x as f32, y as f32);
                self.input.mouse(liminalkit_shell::MouseButton::Left, true);
                self.input.mouse(liminalkit_shell::MouseButton::Left, false);
            }
            let theme = self.theme;
            let ui = Ui::new(
                liminalkit_raster::Painter::new(&mut self.buffer),
                &mut self.text,
                &mut self.input,
                theme,
                Scale::ONE,
                &mut self.kernel,
            );
            let (outcome, layout) = match focus {
                Some(focus) => {
                    let mut ui = ui.with_focus(focus);
                    ui.toast((10, 10), 380, card)
                }
                None => {
                    let mut ui = ui;
                    ui.toast((10, 10), 380, card)
                }
            };
            self.input.end_frame();
            (outcome, layout)
        }

        fn pixel(&self, x: i32, y: i32) -> u32 {
            self.buffer.pixels[y as usize * self.buffer.width as usize + x as usize]
        }
    }

    #[test]
    fn body_lines_cap_the_measured_height() {
        let mut text = cache();
        let theme = Theme::default();
        let tall_body = "word ".repeat(200);
        let taller_body = "word ".repeat(2000);
        let short = card("done", None, &[]);
        let tall = card(&tall_body, None, &[]);
        let taller = card(&taller_body, None, &[]);
        let none = card("", None, &[]);
        let short = toast_layout(&mut text, &theme, Scale::ONE, (0, 0), 380, &short);
        let tall = toast_layout(&mut text, &theme, Scale::ONE, (0, 0), 380, &tall);
        let taller = toast_layout(&mut text, &theme, Scale::ONE, (0, 0), 380, &taller);
        let none = toast_layout(&mut text, &theme, Scale::ONE, (0, 0), 380, &none);
        assert_eq!(tall.body_lines, TOAST_MAX_BODY_LINES);
        assert_eq!(taller.body_lines, TOAST_MAX_BODY_LINES);
        assert_eq!(tall.height, taller.height, "a longer body draws no taller");
        assert!(none.height < short.height && short.height < tall.height);
        assert!(
            taller.height < Scale::ONE.px(240),
            "a hostile body stays a card, not a document: {}px",
            taller.height
        );
    }

    #[test]
    fn actions_are_capped_and_stay_inside_the_card() {
        let mut text = cache();
        let theme = Theme::default();
        let many: Vec<ToastAction> = (0..20)
            .map(|_| ToastAction {
                key: "key",
                label: "Action",
            })
            .collect();
        let card = card("", None, &many);
        let layout = toast_layout(&mut text, &theme, Scale::ONE, (0, 0), 380, &card);
        assert!(
            layout.actions.len() <= TOAST_MAX_ACTIONS,
            "twenty declared actions draw at most {TOAST_MAX_ACTIONS}"
        );
        let area = Rect::new(0, 0, 380, layout.height);
        for chip in &layout.actions {
            assert!(
                chip.x >= area.x
                    && chip.y >= area.y
                    && chip.right() <= area.right()
                    && chip.bottom() <= area.bottom(),
                "chip {chip:?} escapes the card {area:?}"
            );
        }
    }

    #[test]
    fn layout_rects_stay_inside_the_card_and_apart() {
        let mut text = cache();
        let theme = Theme::default();
        let actions = [
            ToastAction {
                key: "reply",
                label: "Reply",
            },
            ToastAction {
                key: "open",
                label: "Open",
            },
        ];
        let icon = Bitmap::new(16, 16);
        let card = card("a short body", Some(&icon), &actions);
        let layout = toast_layout(&mut text, &theme, Scale::ONE, (0, 0), 380, &card);
        let area = Rect::new(0, 0, 380, layout.height);
        let dismiss = layout.dismiss;
        assert!(
            dismiss.x >= area.x
                && dismiss.y >= area.y
                && dismiss.right() <= area.right()
                && dismiss.bottom() <= area.bottom(),
            "dismiss {dismiss:?} escapes the card {area:?}"
        );
        assert_eq!(
            (dismiss.right(), dismiss.y),
            (area.right() - theme.padding, area.y + theme.padding),
            "dismiss sits top-right"
        );
        for pair in layout.actions.windows(2) {
            assert!(
                pair[0].right() <= pair[1].x,
                "chips overlap: {:?} vs {:?}",
                pair[0],
                pair[1]
            );
        }
        for chip in &layout.actions {
            let below_dismiss = chip.y >= dismiss.bottom();
            let left_of_dismiss = chip.right() <= dismiss.x;
            assert!(
                chip.bottom() <= area.bottom() && (below_dismiss || left_of_dismiss),
                "chip {chip:?} overlaps dismiss {dismiss:?}"
            );
        }
    }

    #[test]
    fn a_missing_icon_gives_the_text_the_full_width() {
        let mut text = cache();
        let theme = Theme::default();
        let icon = Bitmap::new(16, 16);
        let with = card("body", Some(&icon), &[]);
        let without = card("body", None, &[]);
        let with = toast_layout(&mut text, &theme, Scale::ONE, (0, 0), 380, &with);
        let without = toast_layout(&mut text, &theme, Scale::ONE, (0, 0), 380, &without);
        assert!(
            without.text_width > with.text_width,
            "no well means wider text: {} vs {}",
            without.text_width,
            with.text_width
        );
    }

    #[test]
    fn clicking_dismiss_reports_dismissal_and_nothing_else() {
        let mut harness = Harness::new();
        let actions = [ToastAction {
            key: "reply",
            label: "Reply",
        }];
        let card = card("body", None, &actions);
        let (_, layout) = harness.frame(None, &[], None, &card);
        let dismiss = layout.dismiss;
        let click = (dismiss.x + dismiss.w / 2, dismiss.y + dismiss.h / 2);
        let (outcome, _) = harness.frame(None, &[], Some(click), &card);
        assert_eq!(
            outcome,
            ToastOutcome {
                dismissed: true,
                action: None,
            }
        );
    }

    #[test]
    fn clicking_an_action_reports_its_service_key() {
        let mut harness = Harness::new();
        let actions = [
            ToastAction {
                key: "reply",
                label: "Reply",
            },
            ToastAction {
                key: "open",
                label: "Open",
            },
        ];
        let card = card("body", None, &actions);
        let (_, layout) = harness.frame(None, &[], None, &card);
        assert_eq!(layout.actions.len(), 2);
        let chip = layout.actions[1];
        let click = (chip.x + chip.w / 2, chip.y + chip.h / 2);
        let (outcome, _) = harness.frame(None, &[], Some(click), &card);
        assert_eq!(
            outcome,
            ToastOutcome {
                dismissed: false,
                action: Some("open".to_owned()),
            }
        );
    }

    #[test]
    fn a_click_on_the_body_asks_for_nothing() {
        let mut harness = Harness::new();
        let card = card("body", None, &[]);
        let (_, layout) = harness.frame(None, &[], None, &card);
        // Left edge, vertically centred: on the card, on no control.
        let click = (10 + 4, 10 + layout.height / 2);
        let (outcome, _) = harness.frame(None, &[], Some(click), &card);
        assert!(outcome.is_none());
    }

    #[test]
    fn enter_activates_the_focused_control_in_tab_order() {
        let mut harness = Harness::new();
        let mut focus = crate::Focus::new();
        let actions = [ToastAction {
            key: "reply",
            label: "Reply",
        }];
        let card = card("body", None, &actions);
        harness.frame(Some(&mut focus), &[], None, &card);
        // One Tab from nothing focused lands on dismiss, the first control.
        harness.frame(Some(&mut focus), &[KeyInput::Tab], None, &card);
        let (outcome, _) = harness.frame(Some(&mut focus), &[KeyInput::Enter], None, &card);
        assert!(outcome.dismissed, "Enter on focused dismiss dismisses");
        // Two Tabs reach the first action chip instead.
        focus.clear();
        harness.frame(Some(&mut focus), &[], None, &card);
        harness.frame(
            Some(&mut focus),
            &[KeyInput::Tab, KeyInput::Tab],
            None,
            &card,
        );
        let (outcome, _) = harness.frame(Some(&mut focus), &[KeyInput::Enter], None, &card);
        assert_eq!(outcome.action.as_deref(), Some("reply"));
    }

    #[test]
    fn theme_tokens_drive_the_cards_fill_and_edge() {
        let mut harness = Harness::new();
        harness.theme.panel = 0x00aa_bbcc;
        harness.theme.panel_edge = 0x0011_2233;
        let card = card("", None, &[]);
        let (_, layout) = harness.frame(None, &[], None, &card);
        let mid_x = 10 + 380 / 2;
        assert_eq!(
            harness.pixel(mid_x, 10),
            0x0011_2233,
            "the card's top edge wears panel_edge"
        );
        assert_eq!(
            harness.pixel(mid_x, 10 + layout.height - 2),
            0x00aa_bbcc,
            "the bottom padding band wears panel"
        );
    }

    #[test]
    fn a_full_card_draws_ink_and_an_empty_one_asks_for_nothing() {
        let mut harness = Harness::new();
        let mut icon = Bitmap::new(16, 16);
        for pixel in icon.pixels.iter_mut() {
            *pixel = (255u32 << 24) | 0x00ff_0000;
        }
        let actions = [
            ToastAction {
                key: "a",
                label: "First",
            },
            ToastAction {
                key: "b",
                label: "Second",
            },
        ];
        let full = card("a body with several words in it", Some(&icon), &actions);
        let (outcome, layout) = harness.frame(None, &[], None, &full);
        assert!(outcome.is_none());
        let ink = harness
            .buffer
            .pixels
            .iter()
            .filter(|pixel| **pixel != 0)
            .count();
        assert!(
            ink > 1_000,
            "a full card paints its summary, body and chips"
        );
        assert!(layout.height > 0);

        let mut empty = Harness::new();
        let card = card("", None, &[]);
        let (outcome, _) = empty.frame(None, &[], None, &card);
        assert!(outcome.is_none(), "an empty card asks for nothing");
    }

    #[test]
    fn toast_width_matches_the_shell_compact_card() {
        assert_eq!(TOAST_WIDTH, 380);
    }
}
