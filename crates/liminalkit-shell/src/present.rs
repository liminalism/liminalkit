//! The presenter seam: how a frame reaches the window.
//!
//! A [`Presenter`] owns the platform surface. Each frame it lends the
//! application a [`WindowBuffer`] to paint into and then shows the result.
//! When the surface's memory can be mapped — softbuffer's can, on every
//! desktop — the buffer *is* that memory, so a frame is rasterized once and
//! copied never; a presenter that cannot lend its surface (a texture
//! upload, say) paints into a buffer it owns and uploads from that. The
//! application cannot tell which it was given.
//!
//! The buffer carries two rectangles with its pixels. Going in, the
//! presenter sets [`WindowBuffer::stale`]: the part of the buffer that does
//! not hold the application's previous frame, which is everything after a
//! resize or on a surface that recycles buffers, and nothing on a surface
//! that keeps the last frame. Coming out, [`WindowBuffer::damage`] is what
//! the painter wrote, and the presenter shows that and nothing else. An
//! application that repaints only what changed therefore needs no second
//! contract: it clips its painter to `stale` plus its own changes, draws as
//! it always has, and the damage is exact.

use liminalkit_raster::WindowBuffer;

#[derive(Debug)]
pub enum PresentError {
    /// The platform surface refused; the message is the backend's.
    Backend(String),
    /// A zero-sized window; nothing to present to.
    InvalidSize,
}

impl std::fmt::Display for PresentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresentError::Backend(message) => write!(f, "presenter backend failed: {message}"),
            PresentError::InvalidSize => write!(f, "invalid surface size"),
        }
    }
}

impl std::error::Error for PresentError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenterBackend {
    Software,
    Gpu,
}

pub trait Presenter {
    /// The surface now has this physical size.
    fn resize(&mut self, width: u32, height: u32) -> Result<(), PresentError>;

    /// Paint one frame and show it. `paint` receives a buffer of the size
    /// from the last [`resize`](Presenter::resize), with
    /// [`WindowBuffer::stale`] set and [`WindowBuffer::damage`] empty; the
    /// presenter then shows what `damage` says was written. Nothing written
    /// means nothing shown, except on a surface whose contents are undefined,
    /// which is shown whole so the window is never left with garbage.
    fn present(&mut self, paint: &mut dyn FnMut(&mut WindowBuffer<'_>))
        -> Result<(), PresentError>;

    fn backend(&self) -> PresenterBackend;
}
