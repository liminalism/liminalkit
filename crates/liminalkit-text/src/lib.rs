//! liminalkit-text: TrueType parsing, OpenType shaping (GSUB/GPOS incl. marks
//! and kerning), exact-area glyph rasterization and a string-level cache.
//!
//! No external dependencies by default. The `bidi` feature adds Unicode
//! bidirectional reordering and bracket mirroring for right-to-left runs
//! through `unicode-bidi`; see [`text::shape_face`]. Faces are embedded by the application and
//! registered in a [`FontSet`]; a [`TextStyle`] (face, size, tracking) is what
//! everything else is keyed on, and a [`TextRendering`] on the cache says how
//! glyphs are fitted to the pixel grid and blended. Provenance: `thai-text` and `pos-client-ui`
//! from restaurant-pos; see `ATTRIBUTION.md`.

pub mod cache;
pub mod font;
pub mod layout;
pub mod rendering;
pub mod sfnt;
pub mod text;

pub use cache::{Align, TextCache};
pub use font::{Face, FaceId, FontSet, TextStyle};
pub use rendering::{Hinting, TextRendering};
pub use text::{
    line_height, render, render_shaped, render_shaped_with, render_with, shape, shape_face, wrap,
    RenderedText, ShapedText,
};
