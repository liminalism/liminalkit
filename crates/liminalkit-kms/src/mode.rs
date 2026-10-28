//! Which of a connector's modes to drive. Pure, so the choice is a test.

/// What a mode offers, independent of the DRM types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeSummary {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
    pub preferred: bool,
}

/// The index of the mode to use. With a requested size, the fastest mode of
/// exactly that size; otherwise the panel's preferred (native) mode, which is
/// what a fixed panel should always be driven at; otherwise the largest,
/// fastest one.
pub fn choose_mode(modes: &[ModeSummary], want: Option<(u32, u32)>) -> Option<usize> {
    let best = |filter: &dyn Fn(&ModeSummary) -> bool| {
        modes
            .iter()
            .enumerate()
            .filter(|(_, m)| filter(m))
            .max_by_key(|(_, m)| (u64::from(m.width) * u64::from(m.height), m.refresh))
            .map(|(i, _)| i)
    };
    match want {
        Some((w, h)) => best(&|m| m.width == w && m.height == h),
        None => best(&|m| m.preferred).or_else(|| best(&|_| true)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: u32, height: u32, refresh: u32, preferred: bool) -> ModeSummary {
        ModeSummary {
            width,
            height,
            refresh,
            preferred,
        }
    }

    #[test]
    fn the_native_mode_wins_unless_a_size_is_asked_for() {
        let modes = [
            mode(1920, 1080, 60, false),
            mode(1280, 800, 60, true),
            mode(1280, 800, 50, false),
            mode(640, 480, 75, false),
        ];
        assert_eq!(choose_mode(&modes, None), Some(1));
        assert_eq!(choose_mode(&modes, Some((640, 480))), Some(3));
        assert_eq!(choose_mode(&modes, Some((800, 600))), None);
        let unflagged = [mode(1280, 800, 50, false), mode(1280, 800, 60, false)];
        assert_eq!(
            choose_mode(&unflagged, None),
            Some(1),
            "fastest of the largest"
        );
        assert_eq!(choose_mode(&[], None), None);
    }
}
