#![allow(clippy::too_many_arguments)]
//! Small flat-design controls: text buttons, underline tabs, segmented
//! controls, steppers, tracks, dots and pills.
//!
//! These take their colours as arguments rather than from [`Theme`], because
//! a terminal's chrome mixes several palettes on one screen (bid/ask, ok/
//! warning/danger) and a theme field per combination would be a worse API
//! than passing the colour.

use crate::widget::Ui;
use liminalkit_raster::Rect;
use liminalkit_text::{Align, TextStyle};

/// Colours for a flat button in its three states.
#[derive(Debug, Clone, Copy)]
pub struct ButtonStyle {
    pub text: u32,
    pub text_active: u32,
    pub fill: u32,
    pub fill_hover: u32,
    pub fill_active: u32,
    pub edge: u32,
    pub edge_active: u32,
    /// Logical corner radius; 0 for square.
    pub radius: i32,
}

impl Ui<'_> {
    /// A flat, bordered text button. `active` draws the pressed/selected
    /// look. Returns whether it was clicked this frame.
    pub fn text_button(
        &mut self,
        area: Rect,
        label: &str,
        style: TextStyle,
        colours: ButtonStyle,
        active: bool,
        enabled: bool,
    ) -> bool {
        let hovered = enabled && self.input.hovering(area);
        let fill = if active {
            colours.fill_active
        } else if hovered {
            colours.fill_hover
        } else {
            colours.fill
        };
        let edge = if active {
            colours.edge_active
        } else {
            colours.edge
        };
        let radius = self.px(colours.radius);
        if radius > 0 {
            self.painter
                .rounded_rect(area, radius, self.scale.hairline(), fill, edge);
        } else {
            self.painter.fill_rect(area, fill);
            self.painter.stroke_rect(area, self.scale.hairline(), edge);
        }
        let text = if active {
            colours.text_active
        } else {
            colours.text
        };
        self.label_styled(area, label, style, text, Align::Centre);
        enabled && self.input.take_click(area)
    }

    /// Tabs drawn as text with an underline bar under the active one, laid
    /// out at equal widths. Returns true if the selection changed.
    pub fn underline_tabs(
        &mut self,
        selected: &mut usize,
        area: Rect,
        labels: &[&str],
        style: TextStyle,
        text: u32,
        text_dim: u32,
        bar: u32,
        bar_height: i32,
    ) -> bool {
        if labels.is_empty() {
            return false;
        }
        *selected = (*selected).min(labels.len() - 1);
        let width = area.w / labels.len() as i32;
        let mut changed = false;
        let bar_h = self.px(bar_height).max(1);
        let inset = self.px(18);
        for (index, label) in labels.iter().enumerate() {
            let tab = Rect::new(area.x + width * index as i32, area.y, width, area.h);
            let active = index == *selected;
            let colour = if active || self.input.hovering(tab) {
                text
            } else {
                text_dim
            };
            self.label_styled(tab, label, style, colour, Align::Centre);
            if active {
                let w = (tab.w - inset * 2).max(self.px(24));
                let x = tab.x + (tab.w - w) / 2;
                self.painter
                    .fill_rect(Rect::new(x, tab.bottom() - bar_h, w, bar_h), bar);
            }
            if self.input.take_click(tab) && !active {
                *selected = index;
                changed = true;
            }
        }
        changed
    }

    /// A row of small buttons of which one is active. Returns the index
    /// clicked this frame, if any.
    pub fn segmented(
        &mut self,
        area: Rect,
        labels: &[&str],
        active: usize,
        style: TextStyle,
        colours: ButtonStyle,
        gap: i32,
    ) -> Option<usize> {
        if labels.is_empty() {
            return None;
        }
        let gap = self.px(gap);
        let width = (area.w - gap * (labels.len() as i32 - 1)) / labels.len() as i32;
        let mut clicked = None;
        for (index, label) in labels.iter().enumerate() {
            let rect = Rect::new(area.x + (width + gap) * index as i32, area.y, width, area.h);
            if self.text_button(rect, label, style, colours, index == active, true) {
                clicked = Some(index);
            }
        }
        clicked
    }

    /// `−  value  +`. Returns the delta clicked this frame (−1, 0, +1).
    pub fn stepper(
        &mut self,
        area: Rect,
        value: &str,
        caption: &str,
        value_style: TextStyle,
        caption_style: TextStyle,
        colours: ButtonStyle,
        text: u32,
        text_dim: u32,
    ) -> i32 {
        let button_w = area.h;
        let minus = Rect::new(area.x, area.y, button_w, area.h);
        let plus = Rect::new(area.right() - button_w, area.y, button_w, area.h);
        let middle = Rect::new(
            minus.right(),
            area.y,
            (plus.x - minus.right()).max(0),
            area.h,
        );
        let mut delta = 0;
        if self.text_button(minus, "−", value_style, colours, false, true) {
            delta -= 1;
        }
        if self.text_button(plus, "+", value_style, colours, false, true) {
            delta += 1;
        }
        let cap_h = self.text.line_height(caption_style);
        let val_h = self.text.line_height(value_style);
        let total = cap_h + val_h;
        let top = middle.y + (middle.h - total) / 2;
        self.text.draw_aligned(
            &mut self.painter,
            caption,
            middle,
            top,
            caption_style,
            text_dim,
            Align::Centre,
        );
        self.text.draw_aligned(
            &mut self.painter,
            value,
            middle,
            top + cap_h,
            value_style,
            text,
            Align::Centre,
        );
        delta
    }

    /// A thin track with a filled fraction, square-cornered.
    pub fn track(&mut self, area: Rect, fraction: f32, track: u32, fill: u32) {
        self.painter.fill_rect(area, track);
        let w = (area.w as f32 * fraction.clamp(0.0, 1.0)).round() as i32;
        if w > 0 {
            self.painter
                .fill_rect(Rect::new(area.x, area.y, w, area.h), fill);
        }
    }

    /// An anti-aliased dot, optionally with a soft ring around it.
    pub fn dot(&mut self, cx: f32, cy: f32, radius: f32, colour: u32, ring: Option<(f32, u8)>) {
        if let Some((ring_width, ring_alpha)) = ring {
            self.painter.fill_circle_aa(
                self.kernel,
                cx,
                cy,
                radius + ring_width,
                colour,
                ring_alpha,
            );
        }
        self.painter
            .fill_circle_aa(self.kernel, cx, cy, radius, colour, 255);
    }

    /// A small square-cornered pill: tinted fill, border, optional leading
    /// dot, text.
    pub fn pill(
        &mut self,
        area: Rect,
        label: &str,
        style: TextStyle,
        text: u32,
        fill: u32,
        edge: u32,
        dot: bool,
    ) {
        self.painter.fill_rect(area, fill);
        self.painter.stroke_rect(area, self.scale.hairline(), edge);
        let pad = self.px(10);
        let mut inner = Rect::new(area.x + pad, area.y, (area.w - pad * 2).max(0), area.h);
        if dot {
            let r = self.scale.f(3.5);
            let cx = inner.x as f32 + r;
            let cy = area.y as f32 + area.h as f32 / 2.0;
            self.dot(cx, cy, r, text, Some((self.scale.f(3.0), 30)));
            let shift = self.px(14);
            inner = Rect::new(inner.x + shift, inner.y, (inner.w - shift).max(0), inner.h);
        }
        self.label_styled(inner, label, style, text, Align::Left);
    }

    /// A bordered, square-cornered panel with an optional soft shadow.
    /// Returns the area inside the border.
    pub fn flat_panel(&mut self, area: Rect, fill: u32, edge: u32, shadow: bool) -> Rect {
        if shadow {
            self.painter
                .shadow_rect(area, self.px(2), self.px(4), 0x1f262a, 24);
        }
        self.painter.fill_rect(area, fill);
        let hair = self.scale.hairline();
        self.painter.stroke_rect(area, hair, edge);
        area.inset(hair)
    }

    /// A small caption above a value, both left-aligned, filling `area`.
    pub fn stat(
        &mut self,
        area: Rect,
        caption: &str,
        value: &str,
        caption_style: TextStyle,
        value_style: TextStyle,
        caption_colour: u32,
        value_colour: u32,
    ) {
        let cap_h = self.text.line_height(caption_style);
        let val_h = self.text.line_height(value_style);
        let gap = self.px(3);
        let top = area.y + (area.h - cap_h - val_h - gap) / 2;
        self.text.draw_fitted(
            &mut self.painter,
            caption,
            area,
            top,
            caption_style,
            caption_colour,
            Align::Left,
        );
        self.text.draw_fitted(
            &mut self.painter,
            value,
            area,
            top + cap_h + gap,
            value_style,
            value_colour,
            Align::Left,
        );
    }
}
