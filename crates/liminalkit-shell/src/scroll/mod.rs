//! Scrolling: what the wheel said, kept whole.
//!
//! EXPERIMENTAL. The types here compile and are tested headlessly; the
//! behaviour on real wheels and touchpads has not been calibrated on hardware.
//!
//! A scroll passes through three hands, and each used to drop something. The
//! host turned a wheel line into a fixed number of physical pixels whatever
//! the display scale or the system wheel setting. The per-frame collector
//! added a frame's events together, so their timing was gone. And each widget
//! rounded the sum to whole pixels every frame, so a touchpad moving a third
//! of a pixel at a time moved nothing at all.
//!
//! This module is the first half of the remedy: one event type,
//! [`WheelSample`], that carries a wheel movement from the host to whoever
//! scrolls with nothing rounded and nothing assumed. It keeps the delta in the
//! unit the platform reported ([`WheelUnit`]), because a Windows wheel
//! reporting 30/120ths of a notch, an X11 wheel reporting one line and a
//! macOS touchpad reporting four pixels are three different statements, and
//! deciding what each is worth is the reader's job, not the messenger's.
//!
//! The module is deliberately usable without the rest of the toolkit. An
//! application that owns its own window and its own scroll model can build
//! samples from primitives and take displacement back in `f64`; nothing here
//! holds a scroll position, and nothing here rounds to a pixel. That happens
//! where a pixel is finally drawn.
//!
//! [`trace`] writes samples as text and reads them back, so what a device
//! actually sent can be recorded once and replayed in a test for ever.

pub mod event;
pub mod interpret;
pub mod motion;
pub mod trace;

pub use event::{
    host_time, GesturePhase, ScrollSettings, SourceHint, Stamp, WheelLines, WheelSample, WheelUnit,
};
pub use interpret::{
    Cadence, CadenceTracker, Classifier, DeviceMode, InputClass, Interpreted, ModifierKey, Scroll,
    Units, WheelInterpreter, WheelProfile, Zoom,
};
pub use motion::{Ledger, MotionController, Smoothing};
pub use trace::{TraceError, WheelTrace};
