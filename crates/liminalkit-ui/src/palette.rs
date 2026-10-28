//! Shared appearance vocabulary for operational screens.
//!
//! This is intentionally semantic rather than widget-specific. Cashier,
//! kitchen, and dispatch screens can arrange their information differently
//! while still looking like terminals from the same installation.
//!
//! Provenance: `pos-client-ui::appearance` (restaurant-pos), see `ATTRIBUTION.md`.

use std::str::FromStr;

use liminalkit_text::TextStyle;

use crate::widget::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    IndustrialLight,
    IndustrialDark,
}

impl Appearance {
    pub const HELP: &str = "light or dark";

    pub fn palette(self) -> OperationalPalette {
        match self {
            Self::IndustrialLight => OperationalPalette {
                canvas: 0x00f7_f8f8,
                surface: 0x00ff_ffff,
                surface_raised: 0x00f0_f2f2,
                border: 0x00b8_bfc1,
                border_strong: 0x0077_8184,
                header: 0x00e8_ebeb,
                text: 0x001d_2426,
                text_muted: 0x005d_686b,
                accent: 0x002c_626c,
                information: 0x003e_6875,
                positive: 0x003f_6b55,
                caution: 0x0080_6328,
                danger: 0x0085_4542,
                selection: 0x00d9_e5e7,
                developer: 0x0038_6651,
            },
            Self::IndustrialDark => OperationalPalette {
                // Neutral graphite, deliberately neither black nor blue.
                canvas: 0x0029_2b2c,
                surface: 0x0034_3738,
                surface_raised: 0x003e_4243,
                border: 0x0059_5f61,
                border_strong: 0x0084_8b8d,
                header: 0x0030_3233,
                text: 0x00f0_f2f2,
                text_muted: 0x00b2_b8ba,
                accent: 0x007f_acb3,
                information: 0x008a_b2bd,
                positive: 0x008d_b39b,
                caution: 0x00cf_af70,
                danger: 0x00cc_8580,
                selection: 0x0047_5558,
                developer: 0x008e_b99f,
            },
        }
    }
}

impl Default for Appearance {
    fn default() -> Self {
        Self::IndustrialLight
    }
}

impl FromStr for Appearance {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "light" | "industrial-light" => Ok(Self::IndustrialLight),
            "dark" | "industrial-dark" => Ok(Self::IndustrialDark),
            other => Err(format!(
                "unknown appearance {other:?}; expected {}",
                Self::HELP
            )),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OperationalPalette {
    pub canvas: u32,
    pub surface: u32,
    pub surface_raised: u32,
    pub border: u32,
    pub border_strong: u32,
    pub header: u32,
    pub text: u32,
    pub text_muted: u32,
    pub accent: u32,
    pub information: u32,
    pub positive: u32,
    pub caution: u32,
    pub danger: u32,
    pub selection: u32,
    pub developer: u32,
}

impl OperationalPalette {
    /// A [`Theme`] carrying this palette's colours, for a screen built on
    /// `liminalkit-ui::Ui`. Sizing (row height, padding, corners) is not part
    /// of the palette, so it comes from `Theme::default()`.
    pub fn theme(self, body: TextStyle, heading: TextStyle) -> Theme {
        Theme {
            background: self.canvas,
            panel: self.surface,
            panel_alt: self.surface_raised,
            panel_edge: self.border,
            text: self.text,
            text_dim: self.text_muted,
            alarm: self.danger,
            good: self.positive,
            accent: self.accent,
            hover: self.selection,
            selection: self.selection,
            body,
            heading,
            ..Theme::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Appearance;

    #[test]
    fn modes_parse_with_short_and_explicit_names() {
        assert_eq!("light".parse(), Ok(Appearance::IndustrialLight));
        assert_eq!("industrial-dark".parse(), Ok(Appearance::IndustrialDark));
    }

    #[test]
    fn dark_canvas_is_graphite_and_light_canvas_is_near_white() {
        let light = Appearance::IndustrialLight.palette().canvas;
        let dark = Appearance::IndustrialDark.palette().canvas;
        assert!(light > 0x00f0_f0f0);
        let dark_r = (dark >> 16) & 0xff;
        let dark_g = (dark >> 8) & 0xff;
        let dark_b = dark & 0xff;
        assert!(dark_r.abs_diff(dark_g) <= 4 && dark_g.abs_diff(dark_b) <= 4);
    }

    #[test]
    fn theme_carries_the_palette_into_a_ui_theme() {
        use liminalkit_text::{FaceId, TextStyle};

        let theme = Appearance::IndustrialDark.palette().theme(
            TextStyle::new(FaceId(0), 17.0),
            TextStyle::new(FaceId(0), 22.0),
        );
        assert_eq!(
            theme.background,
            Appearance::IndustrialDark.palette().canvas
        );
        assert_eq!(theme.text, Appearance::IndustrialDark.palette().text);
    }
}
