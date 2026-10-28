//! Native software presenter. The generic presenter trait stays window-system free.
use liminalkit_raster::WindowBuffer;
use liminalkit_shell::{PresentError, Presenter, PresenterBackend};
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::sync::Arc;
use winit::window::Window;

/// softbuffer: a CPU buffer handed to the window system, no GPU involved.
pub struct SoftbufferPresenter {
    surface: Surface<Arc<Window>, Arc<Window>>,
    size: (u32, u32),
}

impl SoftbufferPresenter {
    pub fn new(window: Arc<Window>) -> Result<SoftbufferPresenter, PresentError> {
        let context =
            Context::new(window.clone()).map_err(|e| PresentError::Backend(e.to_string()))?;
        let surface =
            Surface::new(&context, window).map_err(|e| PresentError::Backend(e.to_string()))?;
        Ok(SoftbufferPresenter {
            surface,
            size: (0, 0),
        })
    }
}

impl Presenter for SoftbufferPresenter {
    fn resize(&mut self, width: u32, height: u32) -> Result<(), PresentError> {
        if self.size == (width, height) {
            return Ok(());
        }
        let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) else {
            return Err(PresentError::InvalidSize);
        };
        self.surface
            .resize(w, h)
            .map_err(|e| PresentError::Backend(e.to_string()))?;
        self.size = (width, height);
        Ok(())
    }

    fn present(&mut self, buffer: &WindowBuffer) -> Result<(), PresentError> {
        if (buffer.width, buffer.height) != self.size {
            self.resize(buffer.width, buffer.height)?;
        }
        let mut frame = self
            .surface
            .buffer_mut()
            .map_err(|e| PresentError::Backend(e.to_string()))?;
        frame.copy_from_slice(&buffer.pixels);
        frame
            .present()
            .map_err(|e| PresentError::Backend(e.to_string()))
    }

    fn backend(&self) -> PresenterBackend {
        PresenterBackend::Software
    }
}
