//! Notification cards drawn with the toolkit's own toast.
//! **EXPERIMENTAL.**
//!
//! [`liminalkit_ui::toast`] lays out and paints a card and owns no timers;
//! [`super::NoticeQueue`] owns the timers and the stack and paints nothing.
//! These three functions are the join: measure a card for the queue, paint it
//! into a layer's surface, and hand what the user did back to the queue.
//!
//! The pattern they are for: the queue's payload holds a card's text, and the
//! application keeps one [`crate::layer::Layer`] per notice on screen. A card
//! is painted once, into its own small surface; from then on entering,
//! leaving and making room are changes to where that layer is placed and how
//! opaque it is, and paint nothing. `crates/liminalkit/examples/planet.rs` is
//! that pattern written out.

use std::time::Duration;

use liminalkit_shell::Scale;
use liminalkit_text::TextCache;
use liminalkit_ui::toast::{toast_layout, ToastCard, ToastOutcome};
use liminalkit_ui::Theme;

use super::{NoticeId, NoticeQueue, Retired};
use crate::surface::{PaintContext, Surface};

/// The height, in device pixels, a card needs at `width`. `theme` is the
/// application's theme as it is given to a paint (logical font sizes).
pub fn toast_height(
    text: &mut TextCache,
    theme: &Theme,
    scale: Scale,
    width: i32,
    card: &ToastCard<'_>,
) -> i32 {
    // `toast_layout` wants device-pixel font sizes, as a `Ui` holds them.
    let mut device = *theme;
    device.body.size *= scale.0;
    device.heading.size *= scale.0;
    toast_layout(text, &device, scale, (0, 0), width, card).height
}

/// Paint `card` to fill the surface's content rectangle and report what its
/// controls were asked to do. Its dismiss control and action chips are
/// recorded as the surface's interactive regions.
pub fn paint_toast(
    surface: &mut Surface,
    cx: PaintContext<'_>,
    card: &ToastCard<'_>,
) -> ToastOutcome {
    let content = surface.content();
    surface.paint(cx, |ui, regions| {
        let (outcome, layout) = ui.toast((content.x, content.y), content.w, card);
        regions.push(layout.dismiss);
        regions.extend(layout.actions);
        outcome
    })
}

/// Retire notice `id` as its card asked: dismissed, or by one of its
/// actions. `false` if the card asked for nothing or the notice is gone.
pub fn apply_outcome<T>(
    queue: &mut NoticeQueue<T>,
    now: Option<Duration>,
    id: NoticeId,
    outcome: &ToastOutcome,
) -> bool {
    match (&outcome.action, outcome.dismissed) {
        (Some(action), _) => queue.resolve(now, id, Retired::Action(action.clone())),
        (None, true) => queue.dismiss(now, id),
        (None, false) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::{Notice, StackStyle};
    use liminalkit_raster::RasterKernel;
    use liminalkit_shell::MouseButton;
    use liminalkit_ui::toast::ToastAction;

    fn card<'a>(actions: &'a [ToastAction<'a>]) -> ToastCard<'a> {
        ToastCard {
            app: "Survey",
            summary: "Signal acquired",
            body: "A repeating pattern on the night side.",
            icon: None,
            actions,
            dismiss_label: "Dismiss",
        }
    }

    #[test]
    fn a_card_measures_as_it_paints_and_its_controls_reach_the_queue() {
        let mut text = TextCache::new(liminalkit_test_fonts::set());
        let mut kernel = RasterKernel::new();
        let actions = [ToastAction {
            key: "open",
            label: "Open",
        }];
        let card = card(&actions);
        let (theme, scale) = (Theme::default(), Scale(1.5));
        let height = toast_height(&mut text, &theme, scale, 360, &card);
        assert!(height > 60 && height < 400, "{height}");

        let mut surface = Surface::new(360, height as u32);
        fn cx<'a>(text: &'a mut TextCache, kernel: &'a mut RasterKernel) -> PaintContext<'a> {
            PaintContext {
                text,
                kernel,
                theme: Theme::default(),
                scale: Scale(1.5),
                now: None,
                smooth_edges: true,
            }
        }
        let outcome = paint_toast(&mut surface, cx(&mut text, &mut kernel), &card);
        assert!(outcome.is_none());
        assert_eq!(surface.regions().len(), 2, "dismiss and one action");
        // The card fills the surface: its last row is painted.
        let last_row = &surface.buffer().pixels[(height as usize - 2) * 360..][..360];
        assert!(last_row.iter().any(|&p| p >> 24 != 0));

        // Press the action chip, then the dismiss control.
        let mut queue = NoticeQueue::new(StackStyle::default());
        let id = queue.push(None, Notice::new("payload"));
        let chip = surface.regions()[1];
        surface
            .input
            .cursor_moved((chip.x + 2) as f32, (chip.y + 2) as f32);
        surface.input.mouse(MouseButton::Left, true);
        surface.invalidate();
        let pressed = paint_toast(&mut surface, cx(&mut text, &mut kernel), &card);
        assert_eq!(pressed.action.as_deref(), Some("open"));
        assert!(apply_outcome(&mut queue, None, id, &pressed));
        queue.update(None, None, |_| 80.0);
        assert_eq!(
            queue.take_retired().map(|(_, _, how)| how),
            Some(Retired::Action("open".into()))
        );
        assert!(!apply_outcome(
            &mut queue,
            None,
            id,
            &ToastOutcome::default()
        ));
    }
}
