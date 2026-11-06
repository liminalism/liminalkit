//! Native software presenter. The generic presenter trait stays window-system free.
use liminalkit_raster::{Rect, WindowBuffer};
use liminalkit_shell::{PresentError, Presenter, PresenterBackend};
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::sync::Arc;
use winit::window::Window;

/// softbuffer: a CPU buffer handed to the window system, no GPU involved.
///
/// The application draws straight into softbuffer's buffer. The copy that
/// used to sit between the two — a whole frame, every frame, 8 MiB at 1080p
/// and four times that at 4K — is gone, and the memory traffic with it.
///
/// softbuffer reports the age of the buffer it lends: 0 when its contents
/// are undefined, 1 when it holds the frame presented last, *n* when it
/// holds the frame presented *n* frames ago (Wayland alternates between
/// two). The presenter turns that into [`WindowBuffer::stale`] from the
/// damage of the frames in between, which it keeps for that purpose, so an
/// application that repaints only what changed is right on every backend
/// without knowing any of this.
pub struct SoftbufferPresenter {
    surface: Surface<Arc<Window>, Arc<Window>>,
    size: (u32, u32),
    /// Damage of recent frames, newest first, for buffers older than one
    /// frame. Longer than any backend's buffer count; an older buffer is
    /// simply stale all over.
    history: [Rect; 4],
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
            history: [Rect::default(); 4],
        })
    }
}

/// The part of a buffer of `age` that is not the last frame: nothing for
/// age 1, the union of the frames it missed otherwise, and everything when
/// its contents are undefined or older than the history kept.
fn stale(age: u8, history: &[Rect], whole: Rect) -> Rect {
    match usize::from(age) {
        0 => whole,
        age if age - 1 > history.len() => whole,
        age => history[..age - 1]
            .iter()
            .fold(Rect::default(), |stale, &damage| stale.union(damage)),
    }
}

/// softbuffer's damage rectangle for `rect`, which must be non-empty and
/// inside the buffer.
fn damage_rect(rect: Rect) -> Option<softbuffer::Rect> {
    Some(softbuffer::Rect {
        x: u32::try_from(rect.x).ok()?,
        y: u32::try_from(rect.y).ok()?,
        width: NonZeroU32::new(u32::try_from(rect.w).ok()?)?,
        height: NonZeroU32::new(u32::try_from(rect.h).ok()?)?,
    })
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
        self.history = [Rect::default(); 4];
        Ok(())
    }

    fn present(
        &mut self,
        paint: &mut dyn FnMut(&mut WindowBuffer<'_>),
    ) -> Result<(), PresentError> {
        let (width, height) = self.size;
        if width == 0 || height == 0 {
            return Err(PresentError::InvalidSize);
        }
        let whole = Rect::new(0, 0, width as i32, height as i32);
        let mut frame = self
            .surface
            .buffer_mut()
            .map_err(|e| PresentError::Backend(e.to_string()))?;
        let age = frame.age();
        let mut buffer = WindowBuffer::over(width, height, &mut frame);
        buffer.stale = stale(age, &self.history, whole);
        paint(&mut buffer);
        let damage = buffer.damage.intersect(whole);
        // An undefined buffer is shown whole even when nothing was drawn, so
        // a window never shows garbage; a kept one with nothing new stays as
        // it is, which costs nothing at all.
        let shown = if damage.is_empty() && age != 0 {
            return Ok(());
        } else if age == 0 {
            whole
        } else {
            damage
        };
        self.history.rotate_right(1);
        self.history[0] = shown;
        let rects: Vec<softbuffer::Rect> = damage_rect(shown).into_iter().collect();
        frame
            .present_with_damage(&rects)
            .map_err(|e| PresentError::Backend(e.to_string()))
    }

    fn backend(&self) -> PresenterBackend {
        PresenterBackend::Software
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_follows_the_buffer_age() {
        let whole = Rect::new(0, 0, 100, 100);
        let history = [
            Rect::new(10, 10, 5, 5),
            Rect::new(50, 50, 20, 20),
            Rect::new(0, 0, 1, 1),
            Rect::default(),
        ];
        assert_eq!(stale(0, &history, whole), whole, "undefined contents");
        assert_eq!(stale(1, &history, whole), Rect::default(), "the last frame");
        assert_eq!(
            stale(2, &history, whole),
            history[0],
            "missed exactly the last frame's damage"
        );
        assert_eq!(
            stale(3, &history, whole),
            history[0].union(history[1]),
            "missed the last two"
        );
        assert_eq!(
            stale(5, &history, whole),
            history.iter().fold(Rect::default(), |a, &b| a.union(b))
        );
        assert_eq!(
            stale(6, &history, whole),
            whole,
            "older than the history kept"
        );
    }

    #[test]
    fn damage_rects_are_whole_pixels_inside_the_buffer() {
        let rect = damage_rect(Rect::new(3, 4, 10, 20)).unwrap();
        assert_eq!((rect.x, rect.y), (3, 4));
        assert_eq!((rect.width.get(), rect.height.get()), (10, 20));
        assert!(damage_rect(Rect::default()).is_none());
        assert!(damage_rect(Rect::new(-1, 0, 2, 2)).is_none());
    }
}
