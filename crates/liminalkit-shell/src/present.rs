//! The presenter seam: how a finished pixel buffer reaches the window.
//!
//! The application paints into a [`WindowBuffer`]; a [`Presenter`] owns the
//! platform surface and copies the buffer onto it. Today that is softbuffer.
//! A GPU presenter would implement the same trait — first as a texture upload
//! of the same buffer (vsync'd presents for free), later as a compositor if
//! profiling ever asks for one — without touching the application.

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
    /// Copy the whole buffer to the window. `buffer` matches the last resize.
    fn present(&mut self, buffer: &WindowBuffer) -> Result<(), PresentError>;
    fn backend(&self) -> PresenterBackend;
}
