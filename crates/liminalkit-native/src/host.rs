//! The wrapper that ties native controls to an application's window and
//! frames. Everything a [`PixelApp`] can be asked is forwarded untouched;
//! three calls do extra work: window creation attaches the backend, a
//! rendered frame is reconciled with the platform, and exit detaches.

use std::path::PathBuf;
use std::time::Duration;

use liminalkit_raster::WindowBuffer;
use liminalkit_shell::{KeyInput, Modifiers, MouseButton, Scale, TouchEvent};
use liminalkit_windowing::{
    CursorShape, FrameInfo, GesturePhase, ImeCursorArea, ImeEvent, KeyEvent, PixelApp, ScrollEvent,
    ScrollSettings, Waker, WheelSample, WindowRequest,
};
use winit::window::Window;

use crate::NativeControls;

/// An application with native controls attached. Made by
/// [`NativeControls::host`]; run it in place of the application.
pub struct NativeHost<A: PixelApp> {
    controls: NativeControls,
    app: A,
}

impl<A: PixelApp> NativeHost<A> {
    pub(crate) fn new(controls: NativeControls, app: A) -> NativeHost<A> {
        NativeHost { controls, app }
    }

    pub fn app(&self) -> &A {
        &self.app
    }

    pub fn app_mut(&mut self) -> &mut A {
        &mut self.app
    }

    pub fn controls(&self) -> &NativeControls {
        &self.controls
    }
}

impl<A: PixelApp> PixelApp for NativeHost<A> {
    fn render(&mut self, buffer: &mut WindowBuffer, scale: Scale) {
        let shared = &self.controls.shared;
        shared.model.borrow_mut().begin_frame();
        self.app.render(buffer, scale);
        // The borrow ends before the platform is touched: changing a control
        // makes it notify its parent synchronously, and that lands back in
        // the model.
        let commands = shared.model.borrow_mut().end_frame();
        let events_read = shared.model.borrow_mut().take_events_read();
        #[cfg(target_os = "windows")]
        crate::win::apply(shared, commands, events_read);
        #[cfg(not(target_os = "windows"))]
        let _ = (commands, events_read);
    }

    fn on_native_window(&mut self, window: &Window) {
        self.app.on_native_window(window);
        #[cfg(target_os = "windows")]
        if self.controls.is_native() {
            if let Err(error) = crate::win::attach(&self.controls.shared, window) {
                self.app
                    .on_host_error(&format!("native controls unavailable: {error}"));
                self.controls.shared.model.borrow_mut().native = false;
            }
        }
    }

    fn on_exit(&mut self) {
        self.app.on_exit();
        #[cfg(target_os = "windows")]
        crate::win::detach(&self.controls.shared);
    }

    fn on_scale(&mut self, scale: Scale) {
        self.app.on_scale(scale);
    }
    fn on_layout_changed(&mut self) {
        self.app.on_layout_changed();
    }
    fn attach(&mut self, waker: Waker) {
        self.app.attach(waker);
    }
    fn animation_interval(&self) -> Option<Duration> {
        self.app.animation_interval()
    }
    fn tick(&mut self) {
        self.app.tick();
    }
    fn on_key(&mut self, key: &KeyInput) {
        self.app.on_key(key);
    }
    fn on_key_event(&mut self, event: &KeyEvent) {
        self.app.on_key_event(event);
    }
    fn on_ime(&mut self, ime: &ImeEvent) {
        self.app.on_ime(ime);
    }
    fn on_focus_changed(&mut self, focused: bool) {
        self.app.on_focus_changed(focused);
    }
    fn poll_title(&mut self) -> Option<String> {
        self.app.poll_title()
    }
    fn poll_fullscreen(&mut self) -> Option<bool> {
        self.app.poll_fullscreen()
    }
    fn poll_window(&mut self) -> Option<WindowRequest> {
        self.app.poll_window()
    }
    fn cursor_shape(&self) -> CursorShape {
        self.app.cursor_shape()
    }
    fn on_host_error(&mut self, error: &str) {
        self.app.on_host_error(error);
    }
    fn on_modifiers(&mut self, modifiers: Modifiers) {
        self.app.on_modifiers(modifiers);
    }
    fn on_cursor(&mut self, x: f32, y: f32) {
        self.app.on_cursor(x, y);
    }
    fn on_cursor_left(&mut self) {
        self.app.on_cursor_left();
    }
    fn on_mouse(&mut self, button: MouseButton, pressed: bool) {
        self.app.on_mouse(button, pressed);
    }
    fn on_touch(&mut self, event: &TouchEvent) {
        self.app.on_touch(event);
    }
    fn on_file_drop(&mut self, path: PathBuf) {
        self.app.on_file_drop(path);
    }
    fn on_frame(&mut self, frame: &FrameInfo) {
        // The system's wheel settings, read when the window was attached and
        // again whenever the user changed them.
        #[cfg(target_os = "windows")]
        if let Some(settings) = crate::win::take_scroll_settings(&self.controls.shared) {
            self.app.on_scroll_settings(&settings);
        }
        self.app.on_frame(frame);
    }
    fn on_wheel(&mut self, sample: &WheelSample) {
        #[cfg(target_os = "windows")]
        let sample = &crate::win::hinted(&self.controls.shared, *sample);
        self.app.on_wheel(sample);
    }
    fn on_scroll_settings(&mut self, settings: &ScrollSettings) {
        self.app.on_scroll_settings(settings);
    }
    fn on_scroll(&mut self, dx: f32, dy: f32) {
        self.app.on_scroll(dx, dy);
    }
    fn on_scroll_event(&mut self, event: &ScrollEvent) {
        self.app.on_scroll_event(event);
    }
    fn on_magnify(&mut self, phase: GesturePhase, delta: f64) {
        self.app.on_magnify(phase, delta);
    }
    fn ime_allowed(&self) -> bool {
        self.app.ime_allowed()
    }
    fn ime_cursor_area(&self) -> Option<ImeCursorArea> {
        self.app.ime_cursor_area()
    }
    fn should_exit(&self) -> bool {
        self.app.should_exit()
    }
    fn on_theme(&mut self, dark: bool) {
        self.app.on_theme(dark);
    }
    #[cfg(feature = "accessibility")]
    fn accessibility_tree(&mut self) -> Option<accesskit::TreeUpdate> {
        self.app.accessibility_tree()
    }
    #[cfg(feature = "accessibility")]
    fn on_accessibility_action(&mut self, request: accesskit::ActionRequest) {
        self.app.on_accessibility_action(request);
    }
    #[cfg(feature = "accessibility")]
    fn on_accessibility_deactivated(&mut self) {
        self.app.on_accessibility_deactivated();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ControlKind, NativeEvent};
    use liminalkit_raster::Rect;
    use std::cell::Cell;
    use std::rc::Rc;

    struct Probe {
        native: NativeControls,
        button: crate::ControlId,
        place: Rc<Cell<bool>>,
        keys: u32,
        exited: bool,
    }

    impl PixelApp for Probe {
        fn render(&mut self, _buffer: &mut WindowBuffer, scale: Scale) {
            if self.place.get() {
                self.native
                    .place(self.button, scale.rect(Rect::new(1, 2, 30, 10)));
            }
        }
        fn on_key(&mut self, _key: &KeyInput) {
            self.keys += 1;
        }
        fn on_exit(&mut self) {
            self.exited = true;
        }
        fn should_exit(&self) -> bool {
            self.keys > 1
        }
    }

    fn probe() -> (NativeHost<Probe>, Rc<Cell<bool>>) {
        // Drawn, so the test needs no window on any platform.
        let native = NativeControls::drawn();
        let button = native.button("Go");
        let place = Rc::new(Cell::new(true));
        let app = Probe {
            native: native.clone(),
            button,
            place: place.clone(),
            keys: 0,
            exited: false,
        };
        (native.host(app), place)
    }

    #[test]
    fn the_application_behind_the_host_still_gets_everything() {
        let (mut host, _) = probe();
        host.on_key(&KeyInput::Enter);
        assert!(!host.should_exit());
        host.on_key(&KeyInput::Enter);
        assert!(host.should_exit());
        host.on_exit();
        assert!(host.app().exited);
        assert_eq!(host.cursor_shape(), CursorShape::Default);
        assert_eq!(host.animation_interval(), None);
    }

    #[test]
    fn a_frame_without_a_window_renders_and_keeps_the_model_consistent() {
        let (mut host, place) = probe();
        let mut buffer = WindowBuffer::new(8, 8);
        host.render(&mut buffer, Scale::ONE);
        host.render(&mut buffer, Scale::ONE);
        place.set(false);
        host.render(&mut buffer, Scale::ONE);
        let controls = host.controls();
        assert_eq!(controls.kind(host.app().button), Some(ControlKind::Button));
        assert_eq!(controls.poll_event(), None::<NativeEvent>);
    }
}
