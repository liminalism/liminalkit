//! liminalkit-raster: a dependency-free software rasterizer.
//!
//! A `u32` XRGB pixel buffer, a clip-stack painter, an exact-area
//! anti-aliasing kernel that fills any closed path, a small path builder on
//! top of it, a PNG reader and writer, and a bitmap blit for compositing
//! decoded images (icons, thumbnails) onto the buffer. For screens that are
//! more than text and outlines: SVG path data ([`svg`]), vector icons
//! rasterized once per size and tinted when drawn ([`icon`]), and images
//! resampled once to their drawing size with a copy-speed draw path, plus
//! gradients ([`image`]).
//!
//! Provenance: the painter and blend kernels come from `pos-client-ui` /
//! `pos-simd` (restaurant-pos), the coverage kernel from `thai-text`, itself a
//! trim of the lege-pdf CPU renderer. See `ATTRIBUTION.md`.

pub mod bitmap;
pub mod blend;
mod blit;
pub mod color;
pub mod icon;
pub mod image;
pub mod kernel;
pub mod painter;
pub mod path;
pub mod png;
mod shapes;
pub mod svg;

pub use bitmap::Bitmap;
pub use blit::ScaleFilter;
pub use icon::{Icon, Mask};
pub use image::{Fit, PreparedImage};
pub use kernel::{CoverageBitmap, FillRule, RasterKernel};
pub use painter::{Painter, Rect, WindowBuffer};
pub use path::Path;
pub use svg::{SvgPathError, SvgTransform};
