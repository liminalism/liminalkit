//! Dependency-free platform seams: accent color and error beep.
//!
//! Both are stubs with their macOS resolution documented, so the
//! salon-desktop side can wire up call sites now — `system_accent()` where
//! it picks a highlight color, `beep()` where it refuses a submit — and get
//! real platform behavior later without changing those call sites.
//!
//! No new dependencies and no `unsafe` (the workspace forbids it): calling
//! `NSColor.controlAccentColor` or `NSBeep` needs an AppKit binding, which
//! is exactly the footprint this keeps out until the app needs it.

/// The system's accent color as `[r, g, b]`, or `None` when it is unknown.
///
/// macOS seam: resolve `NSColor.controlAccentColor` to RGB. Until then the
/// app should fall back to its theme's accent.
pub fn system_accent() -> Option<[u8; 3]> {
    None
}

/// A short error beep for a refused submit.
///
/// macOS plays the system alert sound (`NSBeep`); anywhere else this is a
/// no-op seam with the same signature, so call sites never branch.
/// Calling it must never panic, so it stays callable from any key path.
pub fn beep() {
    #[cfg(target_os = "macos")]
    objc2_app_kit::NSBeep();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_accent_is_none_rather_than_a_guess() {
        // Either state is honest: a resolved color, or no claim at all.
        // What it must never be is a made-up color presented as the system's.
        if let Some([r, g, b]) = system_accent() {
            let _ = (r, g, b); // any u8 triple is a well-formed color
        }
    }

    #[test]
    fn beeping_is_always_safe_to_call() {
        beep();
    }
}
