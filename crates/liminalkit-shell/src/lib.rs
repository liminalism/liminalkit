//! liminalkit-shell: the pieces a window host and its application share.
//!
//! Per-frame [`Input`], logical-to-physical [`Scale`], the [`Presenter`]
//! seam that carries a finished buffer to the screen, frame timing and the
//! clipboard. Window hosting itself — the event loop,
//! `liminalkit_windowing::PixelApp`, `WindowConfig`, `run_app` — lives in
//! `liminalkit-windowing`.

pub mod clipboard;
pub mod frame_log;
/// Experimental, behavior and hardware untested.
pub mod gesture;
pub mod input;
pub mod platform;
pub mod present;
pub mod scale;
/// Experimental, behavior and hardware untested.
pub mod scroll_physics;
pub mod touch;
/// Experimental, behavior and hardware untested.
pub mod touch_replay;

pub use clipboard::Clipboard;
pub use frame_log::{FrameSnapshot, FrameTimer, PhaseStats};
pub use input::{Input, KeyInput, Modifiers, MouseButton};
pub use platform::{beep, system_accent};
pub use present::{PresentError, Presenter, PresenterBackend};
pub use scale::Scale;

pub use touch::{
    TouchDelta, TouchEvent, TouchId, TouchPhase, TouchPointer, TouchState, TouchTransform,
};
