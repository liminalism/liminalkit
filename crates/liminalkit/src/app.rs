//! The short road: a [`Screen`] draws into a [`Ui`]; [`App`] owns the rest.
//!
//! A [`PixelApp`] is given a bare pixel buffer and raw events, and
//! an application that implements it directly keeps its own text cache,
//! raster kernel, per-frame [`Input`], keyboard [`Focus`], and calls
//! [`Input::end_frame`] itself. That is the right amount of control for a
//! kiosk that also owns a touch recognizer, and too much ceremony for a
//! settings dialog.
//!
//! [`App`] holds those resources, forwards every host event to the
//! [`Input`], builds the [`Ui`] with Tab traversal attached, calls the
//! screen's [`Screen::draw`], and ends the frame. The screen keeps every
//! piece of its own state as ordinary fields, exactly as it would under
//! `PixelApp`; nothing here stores anything on the screen's behalf. The
//! per-frame [`Context`] is how a screen talks back to the host: quit,
//! retitle, change the cursor, place the IME caret.
//!
//! Popups own input before the background draws. A dropdown, menu or
//! overlay that is open claims its clicks and keys in
//! [`Screen::before_frame`], which runs on the frame's [`Input`] before the
//! `Ui` is built and before Tab traversal looks at the keys; the same popup
//! is then drawn last at the end of [`Screen::draw`].
//!
//! With the `accessibility` feature, `App` publishes the controls each frame
//! drew to assistive technology through AccessKit and routes action requests
//! back into the input, so a screen gets a screen-reader tree without
//! implementing `PixelApp`. What `App` does not do: arbitrate touch gestures
//! (the first contact is forwarded as a pointer press through
//! [`TouchPointer`], which is activation on press, not a tap recognizer).
//! A screen that needs that implements `PixelApp` instead; `App`'s source is
//! the worked example of how.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use liminalkit_raster::{Painter, RasterKernel, Rect, WindowBuffer};
use liminalkit_shell::{
    Input, KeyInput, Modifiers, MouseButton, Scale, TouchEvent, TouchPointer, WheelSample,
};
use liminalkit_text::{FontSet, TextCache};
use liminalkit_ui::{Focus, Theme, Ui};
use liminalkit_windowing::{
    run_app, CursorShape, FrameInfo, ImeCursorArea, ImeEvent, PixelApp, Waker, WindowConfig,
    WindowRequest,
};

/// What a screen can ask of the host during one frame, and what the host
/// tells it. Rebuilt every frame; nothing in it persists except what
/// [`App`] copies out after `draw`.
#[derive(Debug)]
pub struct Context {
    /// The window's scale. The same value as `ui.scale`; here so a screen
    /// can size things before it borrows the `Ui`.
    pub scale: Scale,
    /// The window's size in device pixels.
    pub size: (i32, i32),
    /// Set to close the window and end `run`.
    pub exit: bool,
    /// A new window title, applied once.
    pub title: Option<String>,
    /// `Some(true)` to enter borderless full screen, `Some(false)` to leave.
    pub fullscreen: Option<bool>,
    /// The pointer shape over the window after this frame.
    pub cursor: CursorShape,
    /// The caret rectangle of the focused text field, in device pixels
    /// (`TextFieldState::caret_area`), so the platform's IME candidate
    /// window can sit next to it. `None` leaves the last placement alone.
    pub ime_caret: Option<Rect>,
    /// Forget the keyboard focus and its tab order, for a page change where
    /// none of the old controls will be drawn again.
    pub reset_focus: bool,
    /// Whether the host reported a dark system theme; `None` until it has.
    pub dark: Option<bool>,
    /// Windows to open, or `WindowRequest::Close` for this one, drained by
    /// the host after the frame. A new window needs its own application:
    /// `Box::new(App::new(other_screen, fonts))`.
    pub windows: Vec<WindowRequest>,
    /// When this frame is being drawn. `None` when the host supplied no
    /// clock; see [`Input::now`].
    pub now: Option<Instant>,
    /// One frame of the display the window is on.
    pub frame_interval: Duration,
    frame_wanted: bool,
    keys: Vec<KeyInput>,
}

impl Context {
    /// Ask for another frame after this one, because the screen is drawing
    /// something that is still moving. Lasts one frame: ask again in the
    /// next for as long as it moves. A control that animates asks through
    /// `ui.input` by itself; this is for the screen's own drawing.
    pub fn request_frame(&mut self) {
        self.frame_wanted = true;
    }

    /// Whether `key` arrived this frame. A convenience over `ui.input.keys()`
    /// for a screen that wants a shortcut without consuming the key from the
    /// controls, which still see it.
    pub fn key_pressed(&self, key: KeyInput) -> bool {
        self.keys.contains(&key)
    }
}

/// Something that can draw a screen. One frame is one call to
/// [`draw`](Screen::draw); every other method has a default.
pub trait Screen {
    /// Paint the whole frame into `ui`. `ui.painter` is the window,
    /// `ui.input` is this frame's events, and the controls on `ui` draw and
    /// answer. Rectangles are device pixels: `ui.rect(..)` converts a
    /// logical one.
    fn draw(&mut self, ui: &mut Ui<'_>, ctx: &mut Context);

    /// Claim input before anything draws. An open popup goes here:
    /// `self.menu.handle_input(input, bounds, row_h)`,
    /// `self.overlay.handle_input(input, bounds, ctx.scale)`. `bounds` is
    /// the window in device pixels. Runs before the `Ui` is built, so the
    /// keys a popup takes are never seen by the background's controls or by
    /// Tab traversal.
    fn before_frame(&mut self, _input: &mut Input, _bounds: Rect, _ctx: &Context) {}

    /// The window title, for the accessibility tree's root and the title
    /// bar when `Context::title` has not set one.
    fn title(&self) -> &str {
        ""
    }

    /// A key arrived. Return `true` to keep it from the controls. The
    /// default hands every key on.
    fn on_key(&mut self, _key: &KeyInput, _modifiers: Modifiers) -> bool {
        false
    }

    /// The input method committed finished text, which belongs in the
    /// focused text field: `field.insert_str(text)`. In-progress
    /// composition is painted from `ui.input` without any help from here.
    fn on_text_commit(&mut self, _text: &str) {}

    /// A touch contact, in device pixels. Return `true` to keep it from the
    /// default handling, which forwards the first contact to the controls as
    /// a pointer press.
    fn on_touch(&mut self, _event: &TouchEvent) -> bool {
        false
    }

    /// A file dropped onto the window; one call per file.
    fn on_file_drop(&mut self, _path: PathBuf) {}

    /// The system theme changed, or was first reported.
    fn on_theme(&mut self, _dark: bool) {}

    /// Native keyboard focus came or went. Logical field focus is not
    /// touched; composition and the held pointer are cancelled by `App`.
    fn on_focus_changed(&mut self, _focused: bool) {}

    /// A handle that wakes the window from another thread, for a screen
    /// whose state arrives on a socket. Called once, before the loop starts.
    fn attach(&mut self, _waker: Waker) {}

    /// How soon the screen wants the next unprompted redraw. `None`, the
    /// default, waits for input or a wake; see
    /// [`PixelApp::animation_interval`] for why that is the right default.
    fn animation_interval(&self) -> Option<Duration> {
        None
    }

    /// Called on each tick of [`animation_interval`](Screen::animation_interval).
    fn tick(&mut self) {}

    /// The window is closing.
    fn on_exit(&mut self) {}

    /// Window creation or presentation failed; the window is about to be
    /// retired.
    fn on_host_error(&mut self, _error: &str) {}
}

/// A [`Screen`] with the resources a frame needs, as a [`PixelApp`].
pub struct App<S: Screen> {
    screen: S,
    text: TextCache,
    kernel: RasterKernel,
    input: Input,
    focus: Focus,
    touch: TouchPointer,
    theme: Theme,
    exit: bool,
    title: Option<String>,
    fullscreen: Option<bool>,
    cursor: CursorShape,
    /// The shape the screen last chose, kept apart from what a control asked
    /// for. The screen's choice carries over from frame to frame; a control's
    /// request is for the frame it was made in and no longer.
    screen_cursor: CursorShape,
    ime_caret: Option<Rect>,
    dark: Option<bool>,
    windows: std::collections::VecDeque<WindowRequest>,
    window_title: String,
    /// One frame of the display, as the host last reported it.
    frame_interval: Duration,
    #[cfg(feature = "accessibility")]
    semantics: liminalkit_ui::semantics::Semantics,
}

impl<S: Screen> App<S> {
    /// `fonts` is the application's own set; face `0` is the theme's body
    /// face unless [`theme`](App::theme) says otherwise.
    pub fn new(screen: S, fonts: &'static FontSet) -> App<S> {
        App {
            screen,
            text: TextCache::new(fonts),
            kernel: RasterKernel::new(),
            input: Input::new(),
            focus: Focus::new(),
            touch: TouchPointer::default(),
            theme: Theme::default(),
            exit: false,
            title: None,
            fullscreen: None,
            cursor: CursorShape::Default,
            screen_cursor: CursorShape::Default,
            ime_caret: None,
            dark: None,
            windows: std::collections::VecDeque::new(),
            window_title: String::new(),
            frame_interval: liminalkit_windowing::host::frame_interval(None),
            #[cfg(feature = "accessibility")]
            semantics: liminalkit_ui::semantics::Semantics::default(),
        }
    }

    /// The theme every frame's `Ui` starts from, in logical units.
    pub fn theme(mut self, theme: Theme) -> App<S> {
        self.theme = theme;
        self
    }

    /// A text cache with explicit bounds, replacing the default one.
    pub fn text_cache(mut self, text: TextCache) -> App<S> {
        self.text = text;
        self
    }

    pub fn screen(&self) -> &S {
        &self.screen
    }

    pub fn screen_mut(&mut self) -> &mut S {
        &mut self.screen
    }

    /// Open a window and run until the screen asks to exit or the window is
    /// closed.
    pub fn run(mut self, config: WindowConfig) -> Result<(), Box<dyn std::error::Error>>
    where
        S: 'static,
    {
        self.window_title = config.title.clone();
        run_app(self, config)
    }

    /// Run full screen on a bare Linux display through DRM/KMS.
    #[cfg(feature = "kms")]
    pub fn run_kms(
        self,
        config: liminalkit_kms::KmsConfig,
    ) -> Result<(), liminalkit_kms::KmsError> {
        liminalkit_kms::run_kms(self, config)
    }
}

/// [`App::new`] then [`App::run`]: a screen, its fonts, a window.
pub fn run<S: Screen + 'static>(
    screen: S,
    fonts: &'static FontSet,
    config: WindowConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    App::new(screen, fonts).run(config)
}

impl<S: Screen> PixelApp for App<S> {
    fn render(&mut self, buffer: &mut WindowBuffer, scale: Scale) {
        let size = (buffer.width as i32, buffer.height as i32);
        let mut ctx = Context {
            scale,
            size,
            exit: false,
            title: None,
            fullscreen: None,
            cursor: self.screen_cursor,
            ime_caret: None,
            reset_focus: false,
            dark: self.dark,
            windows: Vec::new(),
            now: self.input.now(),
            frame_interval: self.frame_interval,
            frame_wanted: false,
            keys: Vec::new(),
        };
        self.input.set_scale(scale);
        self.screen
            .before_frame(&mut self.input, Rect::new(0, 0, size.0, size.1), &ctx);
        ctx.keys = self.input.keys().to_vec();
        {
            let title = if self.screen.title().is_empty() {
                self.window_title.clone()
            } else {
                self.screen.title().to_owned()
            };
            #[cfg(feature = "accessibility")]
            self.semantics.begin(&title);
            #[cfg(not(feature = "accessibility"))]
            let _ = title;
            let ui = Ui::new(
                Painter::new(buffer),
                &mut self.text,
                &mut self.input,
                self.theme,
                scale,
                &mut self.kernel,
            )
            .with_focus(&mut self.focus);
            #[cfg(feature = "accessibility")]
            let mut ui = ui.with_semantics(&mut self.semantics);
            #[cfg(not(feature = "accessibility"))]
            let mut ui = ui;
            self.screen.draw(&mut ui, &mut ctx);
            if ctx.frame_wanted {
                ui.input.request_frame();
            }
            ui.input.end_frame();
        }
        self.exit |= ctx.exit;
        if let Some(title) = &ctx.title {
            self.window_title.clone_from(title);
        }
        self.title = ctx.title.or(self.title.take());
        self.fullscreen = ctx.fullscreen.or(self.fullscreen.take());
        // A control that asked for a shape while drawing (a scrollbar thumb
        // being dragged) is more specific than the screen's choice.
        // The request is not what the next frame starts from: seeding the
        // screen's choice with it left the closed hand showing after a
        // middle-button pan had ended, with nothing to put the arrow back.
        self.screen_cursor = ctx.cursor;
        self.cursor = self.input.take_cursor_request().unwrap_or(ctx.cursor);
        self.ime_caret = ctx.ime_caret;
        self.windows.extend(ctx.windows);
        if ctx.reset_focus {
            self.focus.reset();
            #[cfg(feature = "accessibility")]
            self.semantics.invalidate();
        }
    }

    fn on_layout_changed(&mut self) {
        #[cfg(feature = "accessibility")]
        self.semantics.invalidate();
    }

    fn poll_window(&mut self) -> Option<WindowRequest> {
        self.windows.pop_front()
    }

    #[cfg(feature = "accessibility")]
    fn accessibility_tree(&mut self) -> Option<accesskit::TreeUpdate> {
        Some(self.semantics.update())
    }

    #[cfg(feature = "accessibility")]
    fn on_accessibility_action(&mut self, request: accesskit::ActionRequest) {
        self.semantics.action(&mut self.input, request);
    }

    fn attach(&mut self, waker: Waker) {
        self.screen.attach(waker);
    }

    fn on_frame(&mut self, frame: &FrameInfo) {
        self.frame_interval = frame.interval;
        self.input.begin_frame(frame.now);
    }

    fn animation_interval(&self) -> Option<Duration> {
        let screen = self.screen.animation_interval();
        if !self.input.frame_requested() {
            return screen;
        }
        // Something drawn last frame is still moving: one display frame from
        // now, or sooner if the screen wanted sooner anyway.
        Some(screen.map_or(self.frame_interval, |own| own.min(self.frame_interval)))
    }

    fn tick(&mut self) {
        self.screen.tick();
    }

    fn on_key(&mut self, key: &KeyInput) {
        if !self.screen.on_key(key, self.input.modifiers()) {
            self.input.key(key.clone());
        }
    }

    fn on_modifiers(&mut self, modifiers: Modifiers) {
        self.input.modifiers_changed(modifiers);
    }

    fn on_ime(&mut self, ime: &ImeEvent) {
        match ime {
            ImeEvent::Preedit(text, _) => self.input.set_composition(text),
            ImeEvent::Commit(text) => {
                self.input.set_composition("");
                self.screen.on_text_commit(text);
            }
            ImeEvent::Enabled | ImeEvent::Disabled => self.input.set_composition(""),
        }
    }

    fn ime_cursor_area(&self) -> Option<ImeCursorArea> {
        self.ime_caret.map(|caret| ImeCursorArea {
            x: caret.x as f64,
            y: caret.y as f64,
            width: caret.w as f64,
            height: caret.h as f64,
        })
    }

    fn on_focus_changed(&mut self, focused: bool) {
        if !focused {
            self.input.blur();
        }
        self.screen.on_focus_changed(focused);
    }

    fn on_cursor(&mut self, x: f32, y: f32) {
        self.input.cursor_moved(x, y);
    }

    fn on_cursor_left(&mut self) {
        self.input.cursor_left();
    }

    fn on_mouse(&mut self, button: MouseButton, pressed: bool) {
        self.input.mouse(button, pressed);
    }

    fn on_wheel(&mut self, sample: &WheelSample) {
        self.input.wheel(*sample);
    }

    fn on_scroll_settings(&mut self, settings: &liminalkit_shell::ScrollSettings) {
        self.input.set_scroll_settings(*settings);
    }

    fn on_touch(&mut self, event: &TouchEvent) {
        if !self.screen.on_touch(event) {
            self.touch.update(&mut self.input, *event);
        }
    }

    fn on_file_drop(&mut self, path: PathBuf) {
        self.screen.on_file_drop(path);
    }

    fn on_theme(&mut self, dark: bool) {
        self.dark = Some(dark);
        self.screen.on_theme(dark);
    }

    fn poll_title(&mut self) -> Option<String> {
        self.title.take()
    }

    fn poll_fullscreen(&mut self) -> Option<bool> {
        self.fullscreen.take()
    }

    fn cursor_shape(&self) -> CursorShape {
        self.cursor
    }

    fn should_exit(&self) -> bool {
        self.exit
    }

    fn on_exit(&mut self) {
        self.screen.on_exit();
    }

    fn on_host_error(&mut self, error: &str) {
        self.screen.on_host_error(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Counter {
        frames: u32,
        clicks: u32,
        committed: String,
        keys_seen: Vec<KeyInput>,
    }

    impl Screen for Counter {
        fn draw(&mut self, ui: &mut Ui<'_>, ctx: &mut Context) {
            self.frames += 1;
            let button = ui.rect(10, 10, 100, 30);
            if ui.button(button, "go") {
                self.clicks += 1;
            }
            if ctx.key_pressed(KeyInput::Escape) {
                ctx.exit = true;
            }
            ctx.title = Some(format!("frame {}", self.frames));
        }

        fn on_key(&mut self, key: &KeyInput, _modifiers: Modifiers) -> bool {
            self.keys_seen.push(key.clone());
            matches!(key, KeyInput::Character('q'))
        }

        fn on_text_commit(&mut self, text: &str) {
            self.committed.push_str(text);
        }
    }

    fn fonts() -> &'static FontSet {
        liminalkit_test_fonts::set()
    }

    fn frame(app: &mut App<Counter>) {
        let mut buffer = WindowBuffer::new(200, 100);
        app.render(&mut buffer, Scale::ONE);
    }

    #[test]
    fn a_click_reaches_the_button_and_the_frame_is_ended_for_the_screen() {
        let mut app = App::new(
            Counter {
                frames: 0,
                clicks: 0,
                committed: String::new(),
                keys_seen: Vec::new(),
            },
            fonts(),
        );
        app.on_cursor(20.0, 20.0);
        app.on_mouse(MouseButton::Left, true);
        app.on_mouse(MouseButton::Left, false);
        frame(&mut app);
        assert_eq!(app.screen().clicks, 1);
        // The click was consumed and cleared: the next frame sees nothing.
        frame(&mut app);
        assert_eq!(app.screen().clicks, 1);
        assert_eq!(app.poll_title().as_deref(), Some("frame 2"));
        assert_eq!(app.poll_title(), None, "one-shot");
    }

    struct Scroller {
        scroll: liminalkit_ui::ScrollState,
    }

    impl Screen for Scroller {
        fn draw(&mut self, ui: &mut Ui<'_>, _ctx: &mut Context) {
            let area = Rect::new(0, 0, 200, 100);
            ui.scroll_list(&mut self.scroll, area, &[30; 40], |_, _, _| {});
        }
    }

    #[test]
    fn a_wheel_sample_reaches_a_scrolling_control_whole_and_at_the_window_scale() {
        use liminalkit_shell::scroll::GesturePhase;
        let mut app = App::new(
            Scroller {
                scroll: liminalkit_ui::ScrollState::new(),
            },
            fonts(),
        );
        let notch =
            |raw: f32| WheelSample::from_lines(0.0, raw, true, GesturePhase::Moved, Duration::ZERO);
        app.on_cursor(50.0, 50.0);
        // Four quarter notches, a frame apart. Each is 25 px of the hundred a
        // notch moves; the adapter used to see none of them, because it only
        // ever heard the rounded pixel form of the event.
        for _ in 0..4 {
            app.on_wheel(&notch(-0.25));
            let mut buffer = WindowBuffer::new(200, 100);
            app.render(&mut buffer, Scale::ONE);
        }
        assert_eq!(app.screen().scroll.offset(), 100);

        // One notch back, on a display at twice the scale: the same distance
        // on screen is twice the pixels.
        app.on_wheel(&notch(1.0));
        let mut buffer = WindowBuffer::new(200, 100);
        app.render(&mut buffer, Scale(2.0));
        assert_eq!(app.screen().scroll.offset(), 0);
        app.on_wheel(&notch(-1.0));
        let mut buffer = WindowBuffer::new(200, 100);
        app.render(&mut buffer, Scale(2.0));
        assert_eq!(app.screen().scroll.offset(), 200);
    }

    struct Mover {
        /// Frames left in which the screen is still drawing something moving.
        moving: u32,
        seen: Vec<(Option<Instant>, Duration)>,
    }

    impl Screen for Mover {
        fn draw(&mut self, _ui: &mut Ui<'_>, ctx: &mut Context) {
            self.seen.push((ctx.now, ctx.frame_interval));
            if self.moving > 0 {
                self.moving -= 1;
                ctx.request_frame();
            }
        }
    }

    #[test]
    fn a_screen_is_told_the_time_and_gets_frames_only_while_it_asks_for_them() {
        let mut app = App::new(
            Mover {
                moving: 2,
                seen: Vec::new(),
            },
            fonts(),
        );
        assert_eq!(
            app.animation_interval(),
            None,
            "asleep until something moves"
        );

        let start = Instant::now();
        let interval = Duration::from_micros(6_944);
        let mut buffer = WindowBuffer::new(200, 100);
        for frame in 0..4_u32 {
            app.on_frame(&FrameInfo {
                now: start + interval * frame,
                interval,
            });
            app.render(&mut buffer, Scale::ONE);
            let expected = if frame < 2 { Some(interval) } else { None };
            assert_eq!(app.animation_interval(), expected, "after frame {frame}");
        }
        assert_eq!(app.screen().seen[1], (Some(start + interval), interval));
    }

    #[test]
    fn without_a_clock_a_screen_is_told_there_is_none() {
        let mut app = App::new(
            Mover {
                moving: 0,
                seen: Vec::new(),
            },
            fonts(),
        );
        let mut buffer = WindowBuffer::new(200, 100);
        app.render(&mut buffer, Scale::ONE);
        assert_eq!(app.screen().seen[0].0, None);
    }

    #[test]
    fn the_grab_hand_goes_away_when_a_middle_button_pan_ends() {
        let mut app = App::new(
            Scroller {
                scroll: liminalkit_ui::ScrollState::new(),
            },
            fonts(),
        );
        let frame = |app: &mut App<Scroller>| {
            let mut buffer = WindowBuffer::new(200, 100);
            app.render(&mut buffer, Scale::ONE);
            app.cursor_shape()
        };
        app.on_cursor(50.0, 20.0);
        assert_eq!(frame(&mut app), CursorShape::Default);
        app.on_mouse(MouseButton::Middle, true);
        frame(&mut app);
        app.on_cursor(50.0, 60.0);
        assert_eq!(
            frame(&mut app),
            CursorShape::Grabbing,
            "while it is dragged"
        );
        app.on_mouse(MouseButton::Middle, false);
        assert_eq!(frame(&mut app), CursorShape::Default, "and not afterwards");
        // Nor on any later frame, whatever is pressed.
        app.on_mouse(MouseButton::Left, true);
        app.on_mouse(MouseButton::Left, false);
        assert_eq!(frame(&mut app), CursorShape::Default);
    }

    #[test]
    fn the_screen_sees_keys_first_and_can_keep_them_from_the_controls() {
        let mut app = App::new(
            Counter {
                frames: 0,
                clicks: 0,
                committed: String::new(),
                keys_seen: Vec::new(),
            },
            fonts(),
        );
        app.on_key(&KeyInput::Character('q'));
        app.on_key(&KeyInput::Escape);
        assert_eq!(app.input.keys(), &[KeyInput::Escape], "'q' was kept");
        frame(&mut app);
        assert!(app.should_exit(), "Escape reached the screen through ctx");
        assert_eq!(
            app.screen().keys_seen,
            vec![KeyInput::Character('q'), KeyInput::Escape]
        );
    }

    #[test]
    fn ime_commit_goes_to_the_screen_and_preedit_to_the_input() {
        let mut app = App::new(
            Counter {
                frames: 0,
                clicks: 0,
                committed: String::new(),
                keys_seen: Vec::new(),
            },
            fonts(),
        );
        app.on_ime(&ImeEvent::Preedit("ก".to_owned(), None));
        assert_eq!(app.input.composition(), "ก");
        app.on_ime(&ImeEvent::Commit("กา".to_owned()));
        assert_eq!(app.screen().committed, "กา");
        assert_eq!(app.input.composition(), "");
    }
}
