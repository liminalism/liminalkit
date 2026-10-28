//! liminalkit-text: TrueType parsing, OpenType shaping (GSUB/GPOS incl. marks
//! and kerning), exact-area glyph rasterization and a string-level cache.
//!
//! No external dependencies. Faces are embedded by the application and
//! registered in a [`FontSet`]; a [`TextStyle`] (face, size, tracking) is what
//! everything else is keyed on. Provenance: `thai-text` and `pos-client-ui`
//! from restaurant-pos; see `ATTRIBUTION.md`.

pub mod cache;
pub mod font;
pub mod layout;
pub mod sfnt;
pub mod text;

pub use cache::{Align, TextCache};
pub use font::{Face, FaceId, FontSet, TextStyle};
pub use text::{
    line_height, render, render_shaped, shape, shape_face, wrap, RenderedText, ShapedText,
};
