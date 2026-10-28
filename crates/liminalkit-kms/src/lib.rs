//! liminalkit-kms: run a [`PixelApp`] on the bare display — DRM/KMS for the
//! picture, evdev for touch, no window system at all.
//!
//! For appliances: one program, one panel, booted straight into. The same
//! application runs under the winit host on a desktop for development and
//! under this host on the device; it cannot tell which (the host gives it a
//! [`Waker`](liminalkit_windowing::Waker) built with `Waker::from_fn`).
//!
//! What it does differently from running under a kiosk compositor (cage):
//!
//! - **One copy to the glass.** The software-rendered frame is copied once,
//!   into a mapped scan-out buffer, and page-flipped. Under a compositor the
//!   same frame is copied into shared memory, then uploaded into a GPU
//!   texture (on a Pi's V3D, re-tiled by the CPU on the way), then composited.
//! - **Vertical-blank pacing.** The page-flip event drives animation: one
//!   frame per refresh, drawn as the last one lands. The winit host paces
//!   animation with a timer, which beats against the panel's refresh.
//! - **No compositor frame.** A frame is on the glass at the first vertical
//!   blank after it is drawn, not the one after the compositor redraws.
//! - **Fewer parts.** No Wayland, wlroots, compositor process, or GL stack in
//!   the display path.
//!
//! What it gives up, deliberately: keyboards, IME, clipboard, multiple
//! windows, cursor, and accessibility (a lobby panel has no screen reader).
//! Rotation is done in the copy ([`Rotation`]), not by a GPU.
//!
//! Needs DRM master: run it on a VT with no desktop (a systemd service on
//! `tty1`), as a user in the `video` and `input` groups.
//!
//! The pure parts — the multi-touch decoder, rotation, mode choice, frame
//! pacing — are tested anywhere. The display and input paths need a real
//! panel and are checked on one.

use std::path::PathBuf;

use liminalkit_shell::TouchTransform;
use liminalkit_windowing::PixelApp;

pub mod mode;
pub mod mt;
pub mod pacing;
pub mod rotate;

#[cfg(target_os = "linux")]
mod display;
#[cfg(target_os = "linux")]
mod host;
#[cfg(target_os = "linux")]
mod input;

pub use rotate::Rotation;

#[derive(Debug, Clone)]
pub struct KmsConfig {
    /// A `/dev/dri/card*` node; the first with a connected display if `None`.
    pub card: Option<PathBuf>,
    /// A mode size; the panel's preferred (native) mode if `None`.
    pub mode: Option<(u32, u32)>,
    pub rotation: Rotation,
    /// A `/dev/input/event*` node; every direct multi-touch device if `None`.
    pub touch: Option<PathBuf>,
    /// For a touch controller mounted differently from the glass. Applied in
    /// panel pixels, before rotation.
    pub calibration: TouchTransform,
    /// The scale passed to `render`. Applications that fit their own layout to
    /// the buffer can leave this at 1.
    pub scale: f32,
}

impl Default for KmsConfig {
    fn default() -> KmsConfig {
        KmsConfig {
            card: None,
            mode: None,
            rotation: Rotation::R0,
            touch: None,
            calibration: TouchTransform::IDENTITY,
            scale: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KmsError {
    Display(String),
    Io(String),
    Unsupported,
}

impl std::fmt::Display for KmsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KmsError::Display(message) => write!(f, "display: {message}"),
            KmsError::Io(message) => write!(f, "{message}"),
            KmsError::Unsupported => write!(f, "the KMS host runs on Linux only"),
        }
    }
}

impl std::error::Error for KmsError {}

/// Run `app` full screen on the display until it asks to exit. Errors are
/// fatal by design: an appliance's supervisor (systemd) restarts the process,
/// which rediscovers the display and the touchscreen from scratch.
pub fn run_kms<A: PixelApp>(app: A, config: KmsConfig) -> Result<(), KmsError> {
    #[cfg(target_os = "linux")]
    {
        host::run(app, config)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, config);
        Err(KmsError::Unsupported)
    }
}

/// What displays this process can see, without taking any of them. Safe to
/// run under a desktop.
pub fn probe() -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        display::probe()
    }
    #[cfg(not(target_os = "linux"))]
    {
        vec!["the KMS host runs on Linux only".into()]
    }
}
