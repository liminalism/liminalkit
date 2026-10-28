//! A winit host: owns the windows and their pixel buffers, forwards input,
//! tracks the scale factor, paces redraws, and presents through a
//! [`Presenter`].
//!
//! One difference from a general-purpose shell, and it matters: **Escape is
//! delivered to the application, not treated as quit.** On a keypad, Escape is
//! a working key — it abandons what is being typed. A host that closed the
//! window on it would throw away a half-entered order at a busy counter.
//!
//! The counter client is keyboard-only by design — a cashier's hands belong on
//! the keypad — but the back office is a different job done sitting down, with
//! tables to scroll and rows to pick. So the host also reports the cursor,
//! the buttons, the wheel and the modifier keys. Every one of those arrives
//! through a defaulted trait method, which is what keeps the counter client
//! from having to know they exist.
//!
//! [`run_app`] runs one window; [`run_windows`] runs several, each with its
//! own application, buffer and presenter, all sharing one event loop. Either
//! way every window behaves like the single one always did — the multi-window
//! host is the same loop over a list of slots, not a second implementation.
//!
//! Provenance: `pos-client-ui::shell` (restaurant-pos), with HiDPI and the
//! presenter seam added; see `ATTRIBUTION.md`. Moved here from
//! `liminalkit-shell` when the crate grew panes and windows.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::present::SoftbufferPresenter;
use liminalkit_raster::WindowBuffer;
use liminalkit_shell::{
    FrameTimer, KeyInput, Modifiers, MouseButton, PresentError, Presenter, Scale, TouchEvent,
    TouchId, TouchState,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{
    ElementState, Ime as WinitIme, MouseButton as WinitButton, MouseScrollDelta, TouchPhase,
    WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{Theme as WinitTheme, Window, WindowId as WinitWindowId};

#[cfg(target_os = "macos")]
use winit::platform::macos::WindowAttributesExtMacOS;

/// What arrives on the event loop's user channel: a wake, and, with the
/// `accessibility` feature, a request from assistive technology.
#[derive(Debug)]
pub enum Host {
    Wake,
    #[cfg(feature = "accessibility")]
    Accessibility(accesskit_winit::Event),
}

#[cfg(feature = "accessibility")]
impl From<accesskit_winit::Event> for Host {
    fn from(event: accesskit_winit::Event) -> Host {
        Host::Accessibility(event)
    }
}

/// A handle that wakes the windows from another thread.
///
/// The reason this exists is latency. Without it a client that receives its
/// state over a socket has only one way to notice something arrived: ask on a
/// timer. A 33 ms timer costs an average of 16 ms between the daemon answering
/// a keystroke and the screen showing it — half a keystroke's worth of delay
/// that no amount of faster drawing recovers — and it costs thirty wakeups a
/// second forever, on a counter where nothing is happening.
///
/// Waking on arrival removes both at once: the redraw happens when there is
/// something new to draw, and an idle terminal sleeps.
///
/// Cloneable and `Send`, so the socket thread keeps one. Waking windows that
/// have already closed is a no-op rather than an error — a snapshot arriving
/// during shutdown is ordinary, not exceptional.
///
/// The winit host makes one from its event loop. A host that is not winit —
/// a direct KMS host, a test harness — makes one with [`Waker::from_fn`], so
/// the same [`PixelApp`] runs under either without knowing which.
#[derive(Clone)]
pub struct Waker {
    proxy: Option<EventLoopProxy<Host>>,
    custom: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for Waker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Waker")
            .field("winit", &self.proxy.is_some())
            .field("custom", &self.custom.is_some())
            .finish()
    }
}

/// The only user event: "look again". Deliberately carries nothing — the
/// application already owns its channels, and a payload here would be a second
/// path for state to arrive on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wake;

impl Waker {
    /// A waker attached to nothing, for tests and for the `screens` examples
    /// that render without an event loop.
    pub fn detached() -> Waker {
        Waker {
            proxy: None,
            custom: None,
        }
    }

    /// A waker for another host: `wake` calls `wake`. It must be cheap,
    /// non-blocking and callable from any thread.
    pub fn from_fn(wake: impl Fn() + Send + Sync + 'static) -> Waker {
        Waker {
            proxy: None,
            custom: Some(Arc::new(wake)),
        }
    }

    fn winit(proxy: EventLoopProxy<Host>) -> Waker {
        Waker {
            proxy: Some(proxy),
            custom: None,
        }
    }

    /// Ask the windows to run a tick and repaint as soon as they can.
    pub fn wake(&self) {
        if let Some(proxy) = &self.proxy {
            let _ = proxy.send_event(Host::Wake);
        }
        if let Some(wake) = &self.custom {
            wake();
        }
    }
}

/// A wheel notch, in pixels. Platforms that report scrolling in lines rather
/// than pixels get multiplied by this to land somewhere comfortable.
const PIXELS_PER_LINE: f32 = 24.0;

/// One key transition, including release, the whole committed string, and the
/// physical key. [`PixelApp::on_key`] still receives presses only, as the
/// first character, so existing callers keep working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: KeyInput,
    /// The whole string the key committed. Empty for navigation keys and for
    /// key-up. More than one scalar when the platform delivers one.
    pub text: String,
    /// Debug name of the physical key (`KeyA`, `F13`, …).
    pub physical: String,
    pub pressed: bool,
    pub repeat: bool,
    pub modifiers: Modifiers,
}

/// Touch or gesture phase. Momentum is [`GesturePhase::Moved`] after
/// [`GesturePhase::Ended`] is not invented here; the platform's phase is forwarded as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GesturePhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// A scroll. Line deltas stay distinct from the pixel deltas derived from them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollEvent {
    pub pixel_dx: f32,
    pub pixel_dy: f32,
    /// Set when the platform reported lines rather than pixels.
    pub line_dx: Option<f32>,
    pub line_dy: Option<f32>,
    pub phase: GesturePhase,
}

/// Where the IME candidate window should sit, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImeCursorArea {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub trait PixelApp {
    /// Paint the whole frame. `buffer` is the window's physical size;
    /// `scale` converts the logical units layout is written in.
    fn render(&mut self, buffer: &mut WindowBuffer, scale: Scale);

    /// The window moved to a display with a different scale factor. The
    /// next `render` receives it too; this is for caches keyed on scale.
    fn on_scale(&mut self, _scale: Scale) {}

    /// Layout geometry changes before the next render. Identity-sensitive
    /// surfaces should invalidate semantic targets here.
    fn on_layout_changed(&mut self) {}

    /// Take a handle that wakes this window from another thread.
    ///
    /// Called once, before the loop starts. An application whose state arrives
    /// on a socket should hand this to whatever owns the socket: it is the
    /// difference between the screen updating when the answer arrives and the
    /// screen updating on the next timer.
    fn attach(&mut self, _waker: Waker) {}

    /// Called once per native window creation, before initial presentation. Platform integration
    /// may borrow its raw handles here; it must retire associated resources in `on_exit`.
    fn on_native_window(&mut self, _window: &Window) {}

    /// How soon the application wants the next unprompted redraw. `None` waits
    /// for input or a wake; a duration ticks.
    ///
    /// Prefer `None` and a [`Waker`]. A tick is for something the application
    /// itself is animating — a flash that has to end on time — not for
    /// noticing that a message arrived, which is what a wake is for. An
    /// interval here is thirty wakeups a second on a machine that is supposed
    /// to be idle between customers.
    fn animation_interval(&self) -> Option<Duration> {
        None
    }

    fn tick(&mut self) {}
    fn on_key(&mut self, _key: &KeyInput) {}

    /// Press and release, with the full committed string and the physical key.
    fn on_key_event(&mut self, _event: &KeyEvent) {}

    /// IME composition: CJK, emoji-picker, dictation. `Commit` text belongs
    /// in the focused field (see `TextFieldState::insert_str`); `Preedit` is
    /// for showing the in-progress composition. Direct-key typing — Thai
    /// stacks included — still arrives through `on_key`, unchanged.
    fn on_ime(&mut self, _ime: &ImeEvent) {}
    /// Native window keyboard focus changed. Cancel composition and pending
    /// asynchronous field edits on loss, without destroying logical field focus.
    fn on_focus_changed(&mut self, _focused: bool) {}

    /// A new window title to apply, or `None` for no change. The shell drains
    /// this every frame and calls `Window::set_title`, so per-section
    /// subtitles need no window access. One-shot by convention: return the
    /// title once, then `None`.
    fn poll_title(&mut self) -> Option<String> {
        None
    }

    /// A change of full-screen state the application wants. Polled like
    /// [`PixelApp::poll_title`]: return `Some(true)` once to enter borderless
    /// full screen on the current monitor, `Some(false)` once to leave it.
    fn poll_fullscreen(&mut self) -> Option<bool> {
        None
    }

    /// Open another OS window, or close this one. Drained every frame, so an
    /// application that wants two windows returns `Some` twice (usually across
    /// two frames, one-shot like [`PixelApp::poll_title`]); the host keeps
    /// draining until it sees `None`.
    ///
    /// A window that closes itself and opens another in the same drain
    /// replaces itself: the close removes only the window that asked.
    fn poll_window(&mut self) -> Option<WindowRequest> {
        None
    }

    /// The pointer shape the application wants over its window now. Polled
    /// after input; the shell only touches the window when it changes.
    fn cursor_shape(&self) -> CursorShape {
        CursorShape::Default
    }

    fn on_exit(&mut self) {}

    /// Window creation or presentation failed. Called before this window is
    /// retired; applications with pending requests must report infrastructure
    /// failure rather than treating `on_exit` as user cancellation.
    fn on_host_error(&mut self, _error: &str) {}

    /// The modifier state changed. Delivered separately from the keys it
    /// modifies, because shift-clicking is a mouse event that needs to know.
    fn on_modifiers(&mut self, _modifiers: Modifiers) {}

    /// The cursor moved to a position in the buffer's pixel coordinates.
    fn on_cursor(&mut self, _x: f32, _y: f32) {}

    /// The cursor left the window, so nothing is hovered any more. Without
    /// this a highlight stays lit under a cursor that has gone.
    fn on_cursor_left(&mut self) {}

    fn on_mouse(&mut self, _button: MouseButton, _pressed: bool) {}

    /// Touchscreen contact in physical buffer pixels, timestamped on host receipt.
    /// Delivery is immediate, not frame-paced. No mouse events are synthesized
    /// here; opt into shell's TouchPointer for existing press-based controls.
    /// Focus loss, layout changes, suspension and exit cancel active contacts.
    fn on_touch(&mut self, _event: &TouchEvent) {}

    /// A file was dropped onto the window from the OS (a file-manager
    /// drag-and-drop). One call per file, in the order the platform
    /// reported them; a multi-file drop is N calls, not a batch, so the
    /// host keeps no drop state. Hover highlighting is intentionally not
    /// offered — drops either land or they do not.
    fn on_file_drop(&mut self, _path: PathBuf) {}

    /// Wheel or trackpad movement, in pixels. Positive `y` scrolls content
    /// down — the direction the wheel turns, not the direction the view moves.
    fn on_scroll(&mut self, _dx: f32, _dy: f32) {}

    /// Scroll with its phase and, when the platform sent lines, those lines
    /// kept separate from the pixel conversion.
    fn on_scroll_event(&mut self, _event: &ScrollEvent) {}

    /// Pinch / magnify. `delta` is the platform's magnification step.
    fn on_magnify(&mut self, _phase: GesturePhase, _delta: f64) {}

    /// Whether a text input currently owns IME. The default preserves legacy
    /// applications; focused-control surfaces should override it.
    fn ime_allowed(&self) -> bool {
        true
    }

    /// Caret rectangle for the IME candidate window, in physical pixels.
    /// `None` leaves the platform's last rectangle alone.
    fn ime_cursor_area(&self) -> Option<ImeCursorArea> {
        None
    }

    /// Whether the application wants the windows closed. Checked after every
    /// tick, so an application can quit itself. In a multi-window run one
    /// vote closes everything; a window that only wants itself gone returns
    /// [`WindowRequest::Close`] from [`PixelApp::poll_window`] instead.
    fn should_exit(&self) -> bool {
        false
    }

    /// The window's system theme is `dark`. Called once at startup (if the
    /// platform reports one) and again on every change, so a screen can
    /// switch light/dark colours without polling.
    fn on_theme(&mut self, _dark: bool) {}

    /// The application's semantic tree for assistive technology, or `None`
    /// for an application that has none.
    ///
    /// Asked for once when a screen reader attaches and again after every
    /// paint while one is attached, so it describes the frame that was just
    /// drawn: the same widgets, with the same bounds in physical pixels.
    /// The toolkit keeps no widget identity — that is the immediate-mode
    /// bargain — so the ids in the tree are the application's own, chosen
    /// from what a control means rather than where it sits.
    #[cfg(feature = "accessibility")]
    fn accessibility_tree(&mut self) -> Option<accesskit::TreeUpdate> {
        None
    }

    /// Assistive technology asked for an action on a node the application
    /// published, such as a click on a button.
    #[cfg(feature = "accessibility")]
    fn on_accessibility_action(&mut self, _request: accesskit::ActionRequest) {}
    #[cfg(feature = "accessibility")]
    fn on_accessibility_deactivated(&mut self) {}
}

/// A window an application wants opened or closed. See
/// [`PixelApp::poll_window`].
pub enum WindowRequest {
    /// Open a window with its own application. The new window gets its own
    /// buffer, presenter and scale; events route to it by its window id.
    Open {
        config: WindowConfig,
        app: Box<dyn PixelApp>,
    },
    /// Close the window that asked. Its application gets `on_exit`; when the
    /// last window closes, the event loop ends.
    Close,
}

impl std::fmt::Debug for WindowRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WindowRequest::Open { config, .. } => f
                .debug_struct("Open")
                .field("config", config)
                .field("app", &format_args!(".."))
                .finish(),
            WindowRequest::Close => write!(f, "Close"),
        }
    }
}

/// How the window is opened. Sizes are logical pixels.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub min_width: Option<f64>,
    pub min_height: Option<f64>,
    pub resizable: bool,
    /// macOS: draw the titlebar transparent, so content can show through it.
    /// Ignored on every other platform.
    pub titlebar_transparent: bool,
    /// macOS: hide the title text in the titlebar. Ignored elsewhere.
    pub title_hidden: bool,
    /// macOS: let content extend under the titlebar's full-size content
    /// view, the usual companion to `titlebar_transparent`. Ignored
    /// elsewhere.
    pub fullsize_content_view: bool,
    /// Per-window startup token; concurrent requests never race global env.
    pub activation_token: Option<String>,
    /// Linux: the Wayland app_id and the X11 `WM_CLASS` general/instance
    /// name (reverse-DNS, e.g. `org.example.App`). Without it the window
    /// carries no identity, so the taskbar cannot group it with its launcher
    /// icon. Ignored on every other platform.
    pub app_id: Option<String>,
}

impl WindowConfig {
    pub fn new(title: &str, width: f64, height: f64) -> WindowConfig {
        WindowConfig {
            title: title.to_owned(),
            width,
            height,
            min_width: None,
            min_height: None,
            resizable: true,
            titlebar_transparent: false,
            title_hidden: false,
            fullsize_content_view: false,
            activation_token: None,
            app_id: None,
        }
    }

    pub fn min_size(mut self, width: f64, height: f64) -> WindowConfig {
        self.min_width = Some(width);
        self.min_height = Some(height);
        self
    }
}

/// One window's worth of everything the loop needs: the application, its
/// window and presenter once created, its buffer, scale, timers and input
/// bookkeeping. A single-window run is one slot; [`run_windows`] is N.
struct Slot {
    app: Box<dyn PixelApp>,
    config: WindowConfig,
    window: Option<Arc<Window>>,
    winit_id: Option<WinitWindowId>,
    presenter: Option<Box<dyn Presenter>>,
    /// A one-shot factory for this slot's presenter, from
    /// [`run_app_with_presenter`]. Taken when the window is created.
    presenter_factory: Option<PresenterFactory>,
    buffer: WindowBuffer,
    scale: Scale,
    timer: FrameTimer,
    /// Duration of the most recent `tick`, recorded into the next present.
    pending_tick: Duration,
    /// First input since the last present. Cleared when that present is timed.
    input_at: Option<Instant>,
    touch_devices: Vec<winit::event::DeviceId>,
    touches: TouchState,
    modifiers: Modifiers,
    native_focused: bool,
    /// When the next unprompted repaint is due, for an app that asked for an
    /// interval. `None` until the first one is scheduled.
    next_tick: Option<Instant>,
    /// The pointer shape last set on the window.
    cursor_shape: CursorShape,
    #[cfg(feature = "accessibility")]
    accessibility: Option<accesskit_winit::Adapter>,
    /// Whether the application has published a tree, so updates are pushed
    /// only to a tree that exists.
    #[cfg(feature = "accessibility")]
    tree_published: bool,
}

impl Slot {
    fn new(
        app: Box<dyn PixelApp>,
        config: WindowConfig,
        presenter_factory: Option<PresenterFactory>,
    ) -> Slot {
        Slot {
            app,
            config,
            window: None,
            winit_id: None,
            presenter: None,
            presenter_factory,
            buffer: WindowBuffer::new(1, 1),
            scale: Scale::ONE,
            timer: FrameTimer::from_env(),
            pending_tick: Duration::ZERO,
            input_at: None,
            touch_devices: Vec::new(),
            touches: TouchState::default(),
            modifiers: Modifiers::default(),
            native_focused: true,
            next_tick: None,
            cursor_shape: CursorShape::Default,
            #[cfg(feature = "accessibility")]
            accessibility: None,
            #[cfg(feature = "accessibility")]
            tree_published: false,
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn run_tick(&mut self) {
        let started = Instant::now();
        self.app.tick();
        self.pending_tick = started.elapsed();
    }

    fn note_input(&mut self) {
        if self.input_at.is_none() {
            self.input_at = Some(Instant::now());
        }
    }

    fn cancel_touches(&mut self) {
        for event in self.touches.cancel(Instant::now()) {
            self.app.on_touch(&event);
        }
    }

    fn note_touch(&mut self, touch: winit::event::Touch) {
        let received_at = Instant::now();
        self.input_at.get_or_insert(received_at);
        let device = self
            .touch_devices
            .iter()
            .position(|id| *id == touch.device_id)
            .unwrap_or_else(|| {
                self.touch_devices.push(touch.device_id);
                self.touch_devices.len() - 1
            }) as u64;
        let event = TouchEvent {
            id: TouchId {
                device,
                contact: touch.id,
            },
            phase: match touch.phase {
                TouchPhase::Started => liminalkit_shell::TouchPhase::Started,
                TouchPhase::Moved => liminalkit_shell::TouchPhase::Moved,
                TouchPhase::Ended => liminalkit_shell::TouchPhase::Ended,
                TouchPhase::Cancelled => liminalkit_shell::TouchPhase::Cancelled,
            },
            position: (touch.location.x, touch.location.y),
            pressure: touch
                .force
                .map(|force| force.normalized())
                .filter(|p| p.is_finite())
                .map(|p| p.clamp(0.0, 1.0)),
            received_at,
        };
        self.touches.update(event);
        self.app.on_touch(&event);
        self.request_redraw();
    }

    /// A file was dropped onto the window: hand it to the application
    /// immediately and repaint, like any other input event.
    fn note_dropped_file(&mut self, path: PathBuf) {
        self.app.on_file_drop(path);
        self.note_input();
        self.request_redraw();
    }

    fn redraw(&mut self) {
        let Some(window) = &self.window else { return };
        let Some(presenter) = &mut self.presenter else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        if self.buffer.width != size.width || self.buffer.height != size.height {
            self.buffer.resize(size.width, size.height);
            if let Err(error) = presenter.resize(size.width, size.height) {
                self.app.on_host_error(&error.to_string());
                return;
            }
        }
        let started = Instant::now();
        self.app.render(&mut self.buffer, self.scale);
        let painted = Instant::now();
        if self.app.ime_allowed() {
            apply_ime(window, self.app.ime_cursor_area());
        } else {
            window.set_ime_allowed(false);
        }
        if let Err(error) = presenter.present(&self.buffer) {
            self.app.on_host_error(&error.to_string());
            return;
        }
        let presented = Instant::now();
        let input_to_present = self
            .input_at
            .take()
            .map(|at| presented.saturating_duration_since(at));
        let tick = self.pending_tick;
        self.pending_tick = Duration::ZERO;
        self.timer.record_full(
            tick,
            painted - started,
            presented - painted,
            input_to_present,
        );
        // After the present, so a screen reader is never told about a frame
        // the eyes cannot see yet.
        self.publish_accessibility();
    }
}

impl Slot {
    /// Push the frame just drawn to assistive technology, if any is listening
    /// and the application has a tree to give.
    #[cfg(feature = "accessibility")]
    fn publish_accessibility(&mut self) {
        if !self.tree_published {
            return;
        }
        let Some(adapter) = &mut self.accessibility else {
            return;
        };
        let app = &mut self.app;
        let mut tree = None;
        adapter.update_if_active(|| {
            tree = app.accessibility_tree();
            tree.take().unwrap_or_else(|| accesskit::TreeUpdate {
                nodes: Vec::new(),
                tree: None,
                tree_id: accesskit::TreeId::ROOT,
                focus: accesskit::NodeId(0),
            })
        });
    }

    #[cfg(not(feature = "accessibility"))]
    fn publish_accessibility(&mut self) {}
}

struct Shell {
    slots: Vec<Slot>,
    /// The presenter factory shared by every window in a multi-window run.
    /// Taken per window but never consumed, so windows opened later —
    /// including ones requested at runtime — get the same presenter.
    shared_presenter: Option<MultiPresenterFactory>,
    /// A persistent service delivers new applications on the main thread.
    incoming: Option<Box<dyn FnMut() -> Vec<(WindowConfig, Box<dyn PixelApp>)>>>,
    waker: Waker,
    failure: Option<String>,
    #[cfg(feature = "accessibility")]
    proxy: EventLoopProxy<Host>,
}

impl Shell {
    /// Create the winit window for a slot that has none: the initial slots
    /// in `resumed`, runtime requests on the next pass, everything again
    /// after a suspend.
    fn ensure_window(&mut self, event_loop: &ActiveEventLoop, index: usize) {
        if self.slots[index].window.is_some() {
            return;
        }
        let slot = &mut self.slots[index];
        let mut attributes = Window::default_attributes()
            .with_title(slot.config.title.clone())
            .with_resizable(slot.config.resizable)
            .with_inner_size(LogicalSize::new(slot.config.width, slot.config.height));
        if let (Some(w), Some(h)) = (slot.config.min_width, slot.config.min_height) {
            attributes = attributes.with_min_inner_size(LogicalSize::new(w, h));
        }
        #[cfg(target_os = "linux")]
        if let Some(token) = &slot.config.activation_token {
            use winit::platform::startup_notify::WindowAttributesExtStartupNotify;
            attributes = attributes
                .with_activation_token(winit::window::ActivationToken::from_raw(token.clone()));
        }
        #[cfg(target_os = "linux")]
        if let Some(app_id) = &slot.config.app_id {
            // One field feeds both backends: the Wayland app_id and the X11
            // WM_CLASS pair. The x11 trait's `with_name` writes the same
            // field, so importing one of the two is enough.
            use winit::platform::wayland::WindowAttributesExtWayland;
            attributes = attributes.with_name(app_id.clone(), app_id.clone());
        }
        #[cfg(target_os = "macos")]
        {
            attributes = attributes
                .with_titlebar_transparent(slot.config.titlebar_transparent)
                .with_title_hidden(slot.config.title_hidden)
                .with_fullsize_content_view(slot.config.fullsize_content_view);
        }
        #[cfg(feature = "accessibility")]
        {
            // The adapter attaches to the view before it is ever shown, so
            // the first thing assistive technology sees is a described window.
            attributes = attributes.with_visible(false);
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                let error = error.to_string();
                self.failure = Some(error);
                event_loop.exit();
                return;
            }
        };
        slot.app.on_native_window(&window);
        #[cfg(feature = "accessibility")]
        {
            slot.accessibility = Some(accesskit_winit::Adapter::with_event_loop_proxy(
                event_loop,
                &window,
                self.proxy.clone(),
            ));
            window.set_visible(true);
        }
        if slot.app.ime_allowed() {
            apply_ime(&window, slot.app.ime_cursor_area());
        } else {
            window.set_ime_allowed(false);
        }
        let presenter = match slot.presenter_factory.take() {
            Some(factory) => factory(window.clone()),
            None => match &mut self.shared_presenter {
                Some(factory) => factory(window.clone()),
                None => SoftbufferPresenter::new(window.clone())
                    .map(|presenter| Box::new(presenter) as Box<dyn Presenter>),
            },
        };
        let presenter = match presenter {
            Ok(presenter) => presenter,
            Err(error) => {
                let error = error.to_string();
                self.failure = Some(error);
                event_loop.exit();
                return;
            }
        };
        slot.scale = Scale::new(window.scale_factor());
        slot.app.on_scale(slot.scale);
        if let Some(theme) = window.theme() {
            slot.app.on_theme(theme == WinitTheme::Dark);
        }
        slot.winit_id = Some(window.id());
        slot.window = Some(window);
        slot.presenter = Some(presenter);
    }
}

/// Which slot an event belongs to. A linear scan: a run holds a handful of
/// windows, and a map would only move the bookkeeping somewhere else.
fn find_slot(slots: &[Slot], id: WinitWindowId) -> Option<usize> {
    slots
        .iter()
        .position(|slot| slot.winit_id.as_ref() == Some(&id))
}

/// Queue a freshly requested window. It gets its winit window on the next
/// pass through the loop, like the initial ones do in `resumed`.
fn open_slot(slots: &mut Vec<Slot>, config: WindowConfig, app: Box<dyn PixelApp>) {
    slots.push(Slot::new(app, config, None));
}

/// Close one window. Its application gets `on_exit`; the loop ends when the
/// last slot goes, which the caller checks. An unknown index is a no-op.
fn close_slot(slots: &mut Vec<Slot>, index: usize) {
    if index < slots.len() {
        slots[index].cancel_touches();
        slots[index].app.on_exit();
        slots.remove(index);
    }
}

/// Drain every slot's window requests: opens append, closes remove. Opens
/// only ever append, so a close's index still points at the window that
/// asked even when an earlier slot opened new ones first. Returns whether
/// any slots remain.
fn drain_requests(slots: &mut Vec<Slot>) -> bool {
    let mut opens = Vec::new();
    let mut closes = Vec::new();
    for (index, slot) in slots.iter_mut().enumerate() {
        while let Some(request) = slot.app.poll_window() {
            match request {
                WindowRequest::Open { config, app } => opens.push((config, app)),
                WindowRequest::Close => closes.push(index),
            }
        }
    }
    for (config, app) in opens {
        open_slot(slots, config, app);
    }
    // Highest first so removals never shift a pending index, and deduplicated
    // so an app that asks twice does not take its neighbour with it.
    closes.sort_unstable();
    closes.dedup();
    for index in closes.into_iter().rev() {
        close_slot(slots, index);
    }
    !slots.is_empty()
}

/// Translate a winit key into something a counter understands.
fn translate(logical: &Key, text: Option<&str>) -> Option<KeyInput> {
    match logical {
        Key::Named(NamedKey::Enter) => Some(KeyInput::Enter),
        Key::Named(NamedKey::Escape) => Some(KeyInput::Escape),
        Key::Named(NamedKey::Backspace) => Some(KeyInput::Backspace),
        Key::Named(NamedKey::Tab) => Some(KeyInput::Tab),
        Key::Named(NamedKey::Delete) => Some(KeyInput::Delete),
        Key::Named(NamedKey::Home) => Some(KeyInput::Home),
        Key::Named(NamedKey::End) => Some(KeyInput::End),
        Key::Named(NamedKey::PageUp) => Some(KeyInput::PageUp),
        Key::Named(NamedKey::PageDown) => Some(KeyInput::PageDown),
        Key::Named(NamedKey::ArrowLeft) => Some(KeyInput::Left),
        Key::Named(NamedKey::ArrowRight) => Some(KeyInput::Right),
        Key::Named(NamedKey::ArrowUp) => Some(KeyInput::Up),
        Key::Named(NamedKey::ArrowDown) => Some(KeyInput::Down),
        Key::Named(named) => {
            if let Some(number) = function_key(*named) {
                Some(KeyInput::Function(number))
            } else {
                text.and_then(|text| text.chars().next())
                    .map(KeyInput::Character)
            }
        }
        Key::Character(characters) => characters.chars().next().map(KeyInput::Character),
        // The numpad reports its keys as text rather than as named keys on
        // some platforms; taking the text is what makes a numpad and the top
        // row indistinguishable to the application.
        _ => text
            .and_then(|text| text.chars().next())
            .map(KeyInput::Character),
    }
}

/// IME composition, as the application sees it: a dependency-free mirror of
/// `winit::event::Ime`, so tests never need a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeEvent {
    /// The input method attached; expect `Preedit`/`Commit` to follow.
    Enabled,
    /// In-progress composition and an optional byte-indexed cursor range.
    /// An empty string clears the composition.
    Preedit(String, Option<(usize, usize)>),
    /// Finished text to insert into the focused field.
    Commit(String),
    /// The input method detached; drop any pending composition.
    Disabled,
}

fn function_key(named: NamedKey) -> Option<u8> {
    Some(match named {
        NamedKey::F1 => 1,
        NamedKey::F2 => 2,
        NamedKey::F3 => 3,
        NamedKey::F4 => 4,
        NamedKey::F5 => 5,
        NamedKey::F6 => 6,
        NamedKey::F7 => 7,
        NamedKey::F8 => 8,
        NamedKey::F9 => 9,
        NamedKey::F10 => 10,
        NamedKey::F11 => 11,
        NamedKey::F12 => 12,
        NamedKey::F13 => 13,
        NamedKey::F14 => 14,
        NamedKey::F15 => 15,
        NamedKey::F16 => 16,
        NamedKey::F17 => 17,
        NamedKey::F18 => 18,
        NamedKey::F19 => 19,
        NamedKey::F20 => 20,
        NamedKey::F21 => 21,
        NamedKey::F22 => 22,
        NamedKey::F23 => 23,
        NamedKey::F24 => 24,
        _ => return None,
    })
}

/// The whole string a key committed. Navigation keys contribute nothing, so
/// Tab stays a key instead of the `"\t"` winit attaches to it.
pub fn committed_text(logical: &Key, text: Option<&str>) -> String {
    // Space and some keypad/IME platform keys are named or unidentified but still
    // translate to text. Use the same classification as on_key; otherwise the
    // full-text path silently drops characters the legacy path accepts.
    if !matches!(translate(logical, text), Some(KeyInput::Character(_))) {
        return String::new();
    }
    match logical {
        Key::Character(characters) => {
            if let Some(text) = text.filter(|text| !text.is_empty()) {
                text.to_string()
            } else {
                characters.to_string()
            }
        }
        _ => text.unwrap_or_default().to_owned(),
    }
}

pub fn physical_label(key: PhysicalKey) -> String {
    match key {
        PhysicalKey::Code(code) => format!("{code:?}"),
        PhysicalKey::Unidentified(native) => format!("unidentified:{native:?}"),
    }
}

pub fn interpret_key(
    logical: &Key,
    text: Option<&str>,
    physical: &str,
    pressed: bool,
    repeat: bool,
    modifiers: Modifiers,
) -> Option<KeyEvent> {
    let key = translate(logical, text)?;
    Some(KeyEvent {
        key,
        text: if pressed {
            committed_text(logical, text)
        } else {
            String::new()
        },
        physical: physical.to_string(),
        pressed,
        repeat,
        modifiers,
    })
}

pub fn gesture_phase(phase: TouchPhase) -> GesturePhase {
    match phase {
        TouchPhase::Started => GesturePhase::Started,
        TouchPhase::Moved => GesturePhase::Moved,
        TouchPhase::Ended => GesturePhase::Ended,
        TouchPhase::Cancelled => GesturePhase::Cancelled,
    }
}

/// Pixel deltas for existing callers, plus the original line deltas when the
/// platform reported lines.
pub fn scroll_event(delta: MouseScrollDelta, phase: TouchPhase) -> ScrollEvent {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => ScrollEvent {
            pixel_dx: x * PIXELS_PER_LINE,
            pixel_dy: y * PIXELS_PER_LINE,
            line_dx: Some(x),
            line_dy: Some(y),
            phase: gesture_phase(phase),
        },
        MouseScrollDelta::PixelDelta(position) => ScrollEvent {
            pixel_dx: position.x as f32,
            pixel_dy: position.y as f32,
            line_dx: None,
            line_dy: None,
            phase: gesture_phase(phase),
        },
    }
}

/// Turn IME on and, when the app has a caret rectangle, park the candidate
/// window on it.
pub fn apply_ime(window: &Window, area: Option<ImeCursorArea>) {
    window.set_ime_allowed(true);
    if let Some(area) = area {
        window.set_ime_cursor_area(
            PhysicalPosition::new(area.x, area.y),
            PhysicalSize::new(area.width.max(1.0), area.height.max(1.0)),
        );
    }
}

fn translate_ime(ime: &WinitIme) -> ImeEvent {
    match ime {
        WinitIme::Enabled => ImeEvent::Enabled,
        WinitIme::Preedit(text, cursor) => ImeEvent::Preedit(text.clone(), *cursor),
        WinitIme::Commit(text) => ImeEvent::Commit(text.clone()),
        WinitIme::Disabled => ImeEvent::Disabled,
    }
}

/// Apply a pending title request, if any. Split out so the one-shot
/// convention is testable without a window: with no window yet, the app is
/// not even asked, so an early title is never lost.
fn drain_title<A: PixelApp + ?Sized>(app: &mut A, window: Option<&Window>) {
    let Some(window) = window else { return };
    if let Some(title) = app.poll_title() {
        window.set_title(&title);
    }
}

/// Apply a full-screen request and a pointer-shape change, if any.
fn drain_window_state<A: PixelApp + ?Sized>(
    app: &mut A,
    window: Option<&Window>,
    shape: &mut CursorShape,
) {
    let Some(window) = window else { return };
    if let Some(full) = app.poll_fullscreen() {
        window.set_fullscreen(full.then_some(winit::window::Fullscreen::Borderless(None)));
    }
    let wanted = app.cursor_shape();
    if wanted != *shape {
        *shape = wanted;
        window.set_cursor(match wanted {
            CursorShape::Default => winit::window::CursorIcon::Default,
            CursorShape::Text => winit::window::CursorIcon::Text,
            CursorShape::Pointer => winit::window::CursorIcon::Pointer,
            CursorShape::ResizeHorizontal => winit::window::CursorIcon::EwResize,
            CursorShape::ResizeVertical => winit::window::CursorIcon::NsResize,
            CursorShape::ResizeDiagonalTlBr => winit::window::CursorIcon::NwseResize,
            CursorShape::ResizeDiagonalTrBl => winit::window::CursorIcon::NeswResize,
        });
    }
}

/// Pointer shapes an application can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// The platform arrow.
    #[default]
    Default,
    /// An I-beam over editable text.
    Text,
    /// A hand over something clickable.
    Pointer,
    /// A left-right arrow over a draggable vertical edge.
    ResizeHorizontal,
    /// An up-down arrow over a draggable horizontal edge.
    ResizeVertical,
    /// A top-left to bottom-right arrow over a `\` corner handle.
    ResizeDiagonalTlBr,
    /// A top-right to bottom-left arrow over a `/` corner handle.
    ResizeDiagonalTrBl,
}

/// Whether an interval-driven repaint is due, and when the next one is.
///
/// Split out from the event loop because getting it wrong does not look like a
/// bug — it looks like a working application. Requesting a redraw on every
/// pass through `about_to_wait` does not tick, it *spins*: a pending redraw is
/// work, so `WaitUntil` never blocks, and the loop dispatches the redraw,
/// comes straight back, asks for another, and repaints as fast as the
/// compositor allows. On a desktop that is an idle-looking client quietly
/// burning a core. On a Raspberry Pi it is the machine, permanently,
/// repainting a screen nobody is looking at.
///
/// Returns `(redraw_now, next_deadline)`.
fn schedule(now: Instant, next_tick: Option<Instant>, interval: Duration) -> (bool, Instant) {
    match next_tick {
        // Due, or overdue because a slow frame or a suspended laptop took us
        // past it. Either way the next deadline is measured from now rather
        // than from the missed one, so a backlog cannot accumulate into a
        // burst of catch-up frames.
        Some(due) if now < due => (false, due),
        _ => (true, now + interval),
    }
}

fn translate_button(button: WinitButton) -> Option<MouseButton> {
    match button {
        WinitButton::Left => Some(MouseButton::Left),
        WinitButton::Right => Some(MouseButton::Right),
        WinitButton::Middle => Some(MouseButton::Middle),
        // Back, forward and the extra buttons on a gaming mouse. Ignored
        // rather than guessed at, for the same reason an unknown key is.
        _ => None,
    }
}

impl ApplicationHandler<Host> for Shell {
    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        for slot in &mut self.slots {
            slot.cancel_touches();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        for index in 0..self.slots.len() {
            self.ensure_window(event_loop, index);
        }
    }

    /// Something arrived on a thread that is not this one.
    ///
    /// Ticking here rather than leaving it to `about_to_wait` is deliberate:
    /// the drain has to happen before the paint, and this is the one place
    /// that ordering is guaranteed regardless of how the platform schedules
    /// the two. A tick is a channel drain, so running it twice costs nothing.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: Host) {
        match event {
            Host::Wake => {
                for slot in &mut self.slots {
                    slot.run_tick();
                    slot.request_redraw();
                }
            }
            #[cfg(feature = "accessibility")]
            Host::Accessibility(event) => {
                use accesskit_winit::WindowEvent as Access;
                let Some(index) = find_slot(&self.slots, event.window_id) else {
                    return;
                };
                match event.window_event {
                    Access::InitialTreeRequested => {
                        let slot = &mut self.slots[index];
                        if let Some(tree) = slot.app.accessibility_tree() {
                            slot.tree_published = true;
                            if let Some(adapter) = &mut slot.accessibility {
                                adapter.update_if_active(|| tree);
                            }
                        }
                        slot.request_redraw();
                    }
                    Access::ActionRequested(request) => {
                        let slot = &mut self.slots[index];
                        slot.app.on_accessibility_action(request);
                        slot.request_redraw();
                    }
                    Access::AccessibilityDeactivated => {
                        let slot = &mut self.slots[index];
                        slot.tree_published = false;
                        slot.app.on_accessibility_deactivated();
                    }
                }
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        for slot in &mut self.slots {
            slot.timer.flush();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WinitWindowId,
        event: WindowEvent,
    ) {
        let Some(index) = find_slot(&self.slots, window_id) else {
            return;
        };
        #[cfg(feature = "accessibility")]
        {
            let slot = &mut self.slots[index];
            if let (Some(adapter), Some(window)) = (&mut slot.accessibility, &slot.window) {
                adapter.process_event(window, &event);
            }
        }
        if matches!(event, WindowEvent::CloseRequested) {
            // One window's close box closes that window. The loop ends when
            // the last one goes, not before.
            close_slot(&mut self.slots, index);
            if self.slots.is_empty() && self.incoming.is_none() {
                event_loop.exit();
            }
            return;
        }
        let slot = &mut self.slots[index];
        match event {
            WindowEvent::CloseRequested => unreachable!("handled above"),
            WindowEvent::Focused(focused) => {
                slot.native_focused = focused;
                if !focused {
                    slot.cancel_touches();
                    slot.modifiers = Modifiers::default();
                    slot.app.on_modifiers(slot.modifiers);
                    slot.app.on_ime(&ImeEvent::Disabled);
                }
                slot.app.on_focus_changed(focused);
                slot.request_redraw();
            }
            WindowEvent::Resized(_) => {
                slot.cancel_touches();
                slot.app.on_layout_changed();
                slot.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                slot.cancel_touches();
                slot.scale = Scale::new(scale_factor);
                slot.app.on_layout_changed();
                slot.app.on_scale(slot.scale);
                // winit follows this with a `Resized` carrying the new
                // physical size, which triggers the repaint.
                slot.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if !slot.native_focused {
                    return;
                }
                let physical = physical_label(event.physical_key);
                if let Some(decoded) = interpret_key(
                    &event.logical_key,
                    event.text.as_deref(),
                    &physical,
                    event.state == ElementState::Pressed,
                    event.repeat,
                    slot.modifiers,
                ) {
                    if decoded.pressed {
                        slot.app.on_key(&decoded.key);
                    }
                    slot.app.on_key_event(&decoded);
                    slot.note_input();
                    slot.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                slot.modifiers = Modifiers {
                    shift: state.shift_key(),
                    control: state.control_key(),
                    alt: state.alt_key(),
                    logo: state.super_key(),
                };
                slot.app.on_modifiers(slot.modifiers);
            }
            WindowEvent::ThemeChanged(theme) => {
                slot.app.on_theme(theme == WinitTheme::Dark);
                slot.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                slot.app.on_cursor(position.x as f32, position.y as f32);
                // A redraw on every cursor move is what makes hover feedback
                // work at all under `ControlFlow::Wait`. It costs a full
                // repaint, which for a screen of panels and text is cheap
                // enough that measuring it was not worth the complexity of
                // tracking which region changed.
                slot.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                slot.app.on_cursor_left();
                slot.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = translate_button(button) {
                    slot.app.on_mouse(button, state == ElementState::Pressed);
                    slot.request_redraw();
                }
            }
            WindowEvent::Touch(touch) => slot.note_touch(touch),
            WindowEvent::DroppedFile(path) => slot.note_dropped_file(path),
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let scrolled = scroll_event(delta, phase);
                slot.app.on_scroll(scrolled.pixel_dx, scrolled.pixel_dy);
                slot.app.on_scroll_event(&scrolled);
                slot.note_input();
                slot.request_redraw();
            }
            WindowEvent::PinchGesture { delta, phase, .. } => {
                slot.app.on_magnify(gesture_phase(phase), delta);
                slot.note_input();
                slot.request_redraw();
            }
            WindowEvent::Ime(ime) => {
                if (slot.native_focused && slot.app.ime_allowed())
                    || matches!(ime, WinitIme::Disabled)
                {
                    slot.app.on_ime(&translate_ime(&ime));
                }
                slot.note_input();
                slot.request_redraw();
            }
            WindowEvent::RedrawRequested => slot.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(incoming) = &mut self.incoming {
            for (config, mut app) in incoming() {
                app.attach(self.waker.clone());
                open_slot(&mut self.slots, config, app);
            }
        }
        if !drain_requests(&mut self.slots) && self.incoming.is_none() {
            event_loop.exit();
            return;
        }
        for index in 0..self.slots.len() {
            self.ensure_window(event_loop, index);
        }
        for slot in &mut self.slots {
            slot.run_tick();
        }
        for slot in &mut self.slots {
            drain_title(&mut *slot.app, slot.window.as_deref());
            drain_window_state(
                &mut *slot.app,
                slot.window.as_deref(),
                &mut slot.cursor_shape,
            );
        }
        if self.slots.iter().any(|slot| slot.app.should_exit()) {
            event_loop.exit();
            return;
        }
        // One loop, several intervals: polling wins over waiting, and waiting
        // waits for the earliest deadline. Each slot still schedules through
        // `schedule`, so the no-spin guarantee holds per window.
        let now = Instant::now();
        let mut poll = false;
        let mut earliest: Option<Instant> = None;
        for slot in &mut self.slots {
            match slot.app.animation_interval() {
                Some(interval) if interval.is_zero() => {
                    poll = true;
                    slot.request_redraw();
                }
                Some(interval) => {
                    let (redraw, deadline) = schedule(now, slot.next_tick, interval);
                    slot.next_tick = Some(deadline);
                    if redraw {
                        slot.request_redraw();
                    }
                    earliest = Some(earliest.map_or(deadline, |best| best.min(deadline)));
                }
                None => {
                    slot.next_tick = None;
                }
            }
        }
        event_loop.set_control_flow(if poll {
            ControlFlow::Poll
        } else if let Some(deadline) = earliest {
            ControlFlow::WaitUntil(deadline)
        } else {
            ControlFlow::Wait
        });
    }
}

pub type PresenterFactory =
    Box<dyn FnOnce(Arc<Window>) -> Result<Box<dyn Presenter>, PresentError>>;

/// Builds the presenter for every window in a multi-window run. Unlike
/// [`PresenterFactory`] it is called once per window rather than once, so
/// windows opened later — including at runtime — get the same presenter.
pub type MultiPresenterFactory =
    Box<dyn Fn(Arc<Window>) -> Result<Box<dyn Presenter>, PresentError>>;

pub fn run_app<A: PixelApp + 'static>(
    app: A,
    config: WindowConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    run_app_with_presenter(app, config, None)
}

/// `presenter` replaces the softbuffer default. The closure receives the
/// window once, when it is created.
pub fn run_app_with_presenter<A: PixelApp + 'static>(
    app: A,
    config: WindowConfig,
    presenter: Option<PresenterFactory>,
) -> Result<(), Box<dyn std::error::Error>> {
    run_inner(vec![Slot::new(Box::new(app), config, presenter)], None)
}

/// Run several windows, each with its own application, sharing one event
/// loop. Every window behaves like [`run_app`]'s: its own buffer, presenter,
/// scale and timers, with input routed to it by its window id.
///
/// Closing one window's close box closes that window; the loop ends when the
/// last one goes, or when any application votes [`PixelApp::should_exit`].
/// An empty list returns `Ok` immediately: there is nothing to show.
pub fn run_windows(
    apps: Vec<(WindowConfig, Box<dyn PixelApp>)>,
) -> Result<(), Box<dyn std::error::Error>> {
    run_windows_with_presenter(apps, None)
}

/// [`run_windows`] with a presenter factory shared by every window.
/// `presenter` replaces the softbuffer default for the initial windows and
/// for any window an application opens later at runtime.
pub fn run_windows_with_presenter(
    apps: Vec<(WindowConfig, Box<dyn PixelApp>)>,
    presenter: Option<MultiPresenterFactory>,
) -> Result<(), Box<dyn std::error::Error>> {
    let slots = apps
        .into_iter()
        .map(|(config, app)| Slot::new(app, config, None))
        .collect();
    run_inner(slots, presenter)
}

fn run_inner(
    slots: Vec<Slot>,
    shared_presenter: Option<MultiPresenterFactory>,
) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::<Host>::with_user_event().build()?;
    let mut shell = Shell {
        slots,
        shared_presenter,
        incoming: None,
        waker: Waker::winit(event_loop.create_proxy()),
        failure: None,
        #[cfg(feature = "accessibility")]
        proxy: event_loop.create_proxy(),
    };
    for slot in &mut shell.slots {
        slot.app.attach(Waker::winit(event_loop.create_proxy()));
    }
    let result = event_loop.run_app(&mut shell);
    if let Err(error) = &result {
        shell.failure = Some(error.to_string());
    }
    finish_host(&mut shell);
    result?;
    if let Some(error) = shell.failure {
        return Err(std::io::Error::other(error).into());
    }
    Ok(())
}

/// Build once on the process main thread. `start` hands the wake handle to an
/// asynchronous producer; `incoming` constructs windows on this thread only.
/// Unlike `run_windows`, closing the last window leaves this service asleep,
/// ready for the next request. Use `WindowRequest::Close` for per-window exit.
pub fn run_window_service(
    start: impl FnOnce(Waker),
    incoming: impl FnMut() -> Vec<(WindowConfig, Box<dyn PixelApp>)> + 'static,
) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::<Host>::with_user_event().build()?;
    let waker = Waker::winit(event_loop.create_proxy());
    let mut shell = Shell {
        slots: Vec::new(),
        shared_presenter: None,
        incoming: Some(Box::new(incoming)),
        waker: waker.clone(),
        failure: None,
        #[cfg(feature = "accessibility")]
        proxy: event_loop.create_proxy(),
    };
    start(waker);
    let result = event_loop.run_app(&mut shell);
    if let Err(error) = &result {
        shell.failure = Some(error.to_string());
    }
    finish_host(&mut shell);
    result?;
    if let Some(error) = shell.failure {
        return Err(std::io::Error::other(error).into());
    }
    Ok(())
}

fn finish_host(shell: &mut Shell) {
    for slot in &mut shell.slots {
        if let Some(error) = shell.failure.as_deref() {
            slot.app.on_host_error(error);
        } else if shell.incoming.is_some() {
            slot.app.on_host_error("graphical host stopped");
        }
        slot.cancel_touches();
        slot.app.on_exit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::rc::Rc;
    use winit::keyboard::SmolStr;

    #[test]
    fn escape_reaches_the_application_rather_than_closing_the_window() {
        // The whole reason this shell is not the general-purpose one.
        assert_eq!(
            translate(&Key::Named(NamedKey::Escape), None),
            Some(KeyInput::Escape)
        );
    }

    #[test]
    fn digits_arrive_the_same_from_the_numpad_and_the_top_row() {
        // Top row: reported as a character.
        assert_eq!(
            translate(&Key::Character(SmolStr::new("7")), Some("7")),
            Some(KeyInput::Character('7'))
        );
        // Numpad on platforms that report it as an unnamed key with text.
        assert_eq!(
            translate(&Key::Named(NamedKey::Space), Some("7")),
            Some(KeyInput::Character('7'))
        );
    }

    #[test]
    fn the_keypad_operators_all_translate() {
        for symbol in ['+', '-', '*', '/', '.'] {
            assert_eq!(
                translate(&Key::Character(SmolStr::new(symbol.to_string())), None),
                Some(KeyInput::Character(symbol)),
                "{symbol} should reach the application"
            );
        }
    }

    #[test]
    fn function_keys_are_distinguished_from_characters() {
        assert_eq!(
            translate(&Key::Named(NamedKey::F1), None),
            Some(KeyInput::Function(1))
        );
        assert_ne!(
            translate(&Key::Named(NamedKey::F1), None),
            translate(&Key::Named(NamedKey::F2), None)
        );
    }

    #[test]
    fn a_key_with_no_meaning_is_ignored_rather_than_guessed_at() {
        assert_eq!(translate(&Key::Named(NamedKey::Alt), None), None);
    }

    #[test]
    fn the_navigation_keys_a_table_needs_all_arrive() {
        for (named, expected) in [
            (NamedKey::Tab, KeyInput::Tab),
            (NamedKey::Delete, KeyInput::Delete),
            (NamedKey::Home, KeyInput::Home),
            (NamedKey::End, KeyInput::End),
            (NamedKey::PageUp, KeyInput::PageUp),
            (NamedKey::PageDown, KeyInput::PageDown),
        ] {
            assert_eq!(translate(&Key::Named(named), None), Some(expected));
        }
    }

    #[test]
    fn tab_is_a_key_rather_than_a_character() {
        // Winit reports Tab with text of "\t". Falling through to the text
        // branch would make it an unprintable character in a search field
        // instead of moving the focus.
        assert_eq!(
            translate(&Key::Named(NamedKey::Tab), Some("\t")),
            Some(KeyInput::Tab)
        );
    }

    #[test]
    fn the_buttons_a_mouse_has_translate_and_the_extra_ones_do_not() {
        assert_eq!(translate_button(WinitButton::Left), Some(MouseButton::Left));
        assert_eq!(
            translate_button(WinitButton::Right),
            Some(MouseButton::Right)
        );
        assert_eq!(
            translate_button(WinitButton::Middle),
            Some(MouseButton::Middle)
        );
        assert_eq!(translate_button(WinitButton::Back), None);
        assert_eq!(translate_button(WinitButton::Other(9)), None);
    }

    #[test]
    fn an_interval_ticks_rather_than_spinning() {
        // The regression this exists for: the first pass repaints, and every
        // pass before the deadline must not. A version that returned `true`
        // here repainted flat out while claiming a 33 ms tick.
        let interval = Duration::from_millis(33);
        let start = Instant::now();

        let (redraw, deadline) = schedule(start, None, interval);
        assert!(redraw, "the first pass paints");
        assert_eq!(deadline, start + interval);

        // Every pass in between — and there are thousands, because input and
        // the redraw itself both bring us back here.
        for micros in [1, 100, 16_000, 32_999] {
            let now = start + Duration::from_micros(micros);
            let (redraw, unchanged) = schedule(now, Some(deadline), interval);
            assert!(!redraw, "repainted {micros}µs into a 33ms interval");
            assert_eq!(unchanged, deadline, "the deadline must not drift");
        }

        // And at the deadline it paints again.
        let (redraw, next) = schedule(deadline, Some(deadline), interval);
        assert!(redraw);
        assert_eq!(next, deadline + interval);
    }

    #[test]
    fn a_missed_deadline_does_not_become_a_burst_of_catch_up_frames() {
        // A suspended laptop or a slow frame can leave us far past due.
        // Measuring the next deadline from the missed one would queue every
        // frame that should have happened while nothing was watching.
        let interval = Duration::from_millis(33);
        let start = Instant::now();
        let missed = start + interval;
        let late = start + Duration::from_secs(60);

        let (redraw, next) = schedule(late, Some(missed), interval);
        assert!(redraw, "one frame, to catch up");
        assert_eq!(
            next,
            late + interval,
            "measured from now, not from the miss"
        );
    }

    struct TitleApp {
        polls: usize,
        pending: Option<String>,
    }

    impl PixelApp for TitleApp {
        fn render(&mut self, _buffer: &mut WindowBuffer, _scale: Scale) {}

        fn poll_title(&mut self) -> Option<String> {
            self.polls += 1;
            self.pending.take()
        }
    }

    struct BareApp;

    impl PixelApp for BareApp {
        fn render(&mut self, _buffer: &mut WindowBuffer, _scale: Scale) {}
    }

    #[test]
    fn hooks_an_app_does_not_override_are_quiet() {
        // New trait methods must not disturb existing apps: the defaults are
        // a no-op IME hook and no title request.
        let mut app = BareApp;
        app.on_ime(&ImeEvent::Commit("x".to_owned()));
        app.on_key_event(&KeyEvent {
            key: KeyInput::Character('a'),
            text: "a".to_owned(),
            physical: "KeyA".to_owned(),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        });
        app.on_scroll_event(&ScrollEvent {
            pixel_dx: 0.0,
            pixel_dy: 1.0,
            line_dx: None,
            line_dy: None,
            phase: GesturePhase::Moved,
        });
        app.on_magnify(GesturePhase::Ended, 0.1);
        assert!(app.ime_cursor_area().is_none());
        assert_eq!(app.poll_title(), None);
        assert!(
            app.poll_window().is_none(),
            "an app that never heard of windows opens none"
        );
    }

    #[test]
    fn ime_events_all_reach_the_app_rather_than_being_dropped() {
        assert_eq!(translate_ime(&WinitIme::Enabled), ImeEvent::Enabled);
        assert_eq!(
            translate_ime(&WinitIme::Preedit("あ".to_owned(), Some((3, 3)))),
            ImeEvent::Preedit("あ".to_owned(), Some((3, 3)))
        );
        assert_eq!(
            translate_ime(&WinitIme::Preedit(String::new(), None)),
            ImeEvent::Preedit(String::new(), None),
            "the empty preedit that clears a composition survives too"
        );
        assert_eq!(
            translate_ime(&WinitIme::Commit("あ不".to_owned())),
            ImeEvent::Commit("あ不".to_owned())
        );
        assert_eq!(translate_ime(&WinitIme::Disabled), ImeEvent::Disabled);
    }

    #[test]
    fn a_title_waits_for_the_window_rather_than_being_lost() {
        // `about_to_wait` runs before the first window exists. Asking the
        // app then would consume a one-shot title nobody could apply.
        let mut app = TitleApp {
            polls: 0,
            pending: Some("Salon \u{2014} Checkout".to_owned()),
        };
        drain_title(&mut app, None);
        assert_eq!(app.polls, 0, "the app must not be asked with no window");
        assert!(app.pending.is_some(), "the title is still there for later");
    }

    #[test]
    fn a_title_is_one_shot() {
        let mut app = TitleApp {
            polls: 0,
            pending: Some("Salon \u{2014} Checkout".to_owned()),
        };
        assert!(app.poll_title().is_some());
        assert_eq!(app.poll_title(), None, "the second drain finds nothing");
    }

    #[test]
    fn a_committed_string_is_delivered_whole_and_key_up_is_separate() {
        let pressed = interpret_key(
            &Key::Character(SmolStr::new("abc")),
            Some("abc"),
            "KeyA",
            true,
            false,
            Modifiers::default(),
        )
        .expect("character");
        assert_eq!(pressed.key, KeyInput::Character('a'));
        assert_eq!(pressed.text, "abc");
        assert!(pressed.pressed);

        let released = interpret_key(
            &Key::Character(SmolStr::new("a")),
            Some("a"),
            "KeyA",
            false,
            false,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        )
        .expect("release");
        assert!(!released.pressed);
        assert!(released.text.is_empty());
        assert!(released.modifiers.shift);
        assert_eq!(released.physical, "KeyA");
    }

    #[test]
    fn named_space_commits_text_without_turning_navigation_into_text() {
        let event = interpret_key(
            &Key::Named(NamedKey::Space),
            Some(" "),
            "Space",
            true,
            false,
            Modifiers::default(),
        )
        .unwrap();
        assert_eq!(event.key, KeyInput::Character(' '));
        assert_eq!(event.text, " ");
        for (key, text) in [(NamedKey::Tab, "\t"), (NamedKey::Enter, "\r")] {
            let event = interpret_key(
                &Key::Named(key),
                Some(text),
                "navigation",
                true,
                false,
                Modifiers::default(),
            )
            .unwrap();
            assert_eq!(event.text, "");
        }
        assert_eq!(
            committed_text(
                &Key::Unidentified(winit::keyboard::NativeKey::Unidentified),
                Some("7")
            ),
            "7",
        );
    }

    #[test]
    fn function_keys_cover_the_whole_range() {
        assert_eq!(
            translate(&Key::Named(NamedKey::F13), None),
            Some(KeyInput::Function(13))
        );
        assert_eq!(
            translate(&Key::Named(NamedKey::F24), None),
            Some(KeyInput::Function(24))
        );
        assert_eq!(
            translate(&Key::Named(NamedKey::F6), None),
            Some(KeyInput::Function(6))
        );
    }

    #[test]
    fn scroll_keeps_line_deltas_and_forwards_the_phase() {
        let lines = scroll_event(MouseScrollDelta::LineDelta(1.0, -2.0), TouchPhase::Ended);
        assert_eq!(lines.line_dx, Some(1.0));
        assert_eq!(lines.line_dy, Some(-2.0));
        assert_eq!(lines.pixel_dy, -2.0 * PIXELS_PER_LINE);
        assert_eq!(lines.phase, GesturePhase::Ended);

        let pixels = scroll_event(
            MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(4.0, 5.0)),
            TouchPhase::Moved,
        );
        assert_eq!(pixels.line_dx, None);
        assert_eq!((pixels.pixel_dx, pixels.pixel_dy), (4.0, 5.0));
        assert_eq!(pixels.phase, GesturePhase::Moved);
        assert_eq!(
            gesture_phase(TouchPhase::Cancelled),
            GesturePhase::Cancelled
        );
        assert_eq!(gesture_phase(TouchPhase::Started), GesturePhase::Started);
    }

    #[test]
    fn native_touch_reaches_app_and_timing_and_cancels_on_teardown() {
        use std::cell::RefCell;
        struct TouchApp(Rc<RefCell<Vec<TouchEvent>>>);
        impl PixelApp for TouchApp {
            fn render(&mut self, _: &mut WindowBuffer, _: Scale) {}
            fn on_touch(&mut self, event: &TouchEvent) {
                self.0.borrow_mut().push(*event);
            }
        }
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut slot = Slot::new(
            Box::new(TouchApp(seen.clone())),
            WindowConfig::new("touch", 800.0, 600.0),
            None,
        );
        let mut native = winit::event::Touch {
            device_id: winit::event::DeviceId::dummy(),
            phase: TouchPhase::Started,
            location: PhysicalPosition::new(12.5, 24.75),
            force: Some(winit::event::Force::Normalized(0.5)),
            id: 42,
        };
        slot.note_touch(native);
        let first = seen.borrow()[0];
        assert_eq!(first.position, (12.5, 24.75));
        assert_eq!(first.pressure, Some(0.5));
        assert_eq!(first.id.contact, 42);
        assert_eq!(slot.input_at, Some(first.received_at));
        native.phase = TouchPhase::Moved;
        native.location.x = 15.0;
        slot.note_touch(native);
        assert_eq!(slot.input_at, Some(first.received_at));
        slot.cancel_touches();
        let seen = seen.borrow();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[2].id, first.id);
        assert_eq!(seen[2].phase, liminalkit_shell::TouchPhase::Cancelled);
        assert_eq!(seen[2].position, (15.0, 24.75));
        assert!(slot.touches.contacts().is_empty());
    }

    // --- multi-window slots -------------------------------------------------

    struct StubApp {
        requests: VecDeque<WindowRequest>,
        exited: Rc<Cell<bool>>,
    }

    impl PixelApp for StubApp {
        fn render(&mut self, _buffer: &mut WindowBuffer, _scale: Scale) {}

        fn poll_window(&mut self) -> Option<WindowRequest> {
            self.requests.pop_front()
        }

        fn on_exit(&mut self) {
            self.exited.set(true);
        }
    }

    fn stub(requests: Vec<WindowRequest>) -> (Box<dyn PixelApp>, Rc<Cell<bool>>) {
        let exited = Rc::new(Cell::new(false));
        let app = StubApp {
            requests: requests.into(),
            exited: exited.clone(),
        };
        (Box::new(app), exited)
    }

    fn slot(app: Box<dyn PixelApp>) -> Slot {
        Slot::new(app, WindowConfig::new("stub", 100.0, 100.0), None)
    }

    fn open_request(title: &str) -> WindowRequest {
        let (app, _) = stub(Vec::new());
        WindowRequest::Open {
            config: WindowConfig::new(title, 100.0, 100.0),
            app,
        }
    }

    #[test]
    fn a_runtime_open_appends_a_windowless_slot() {
        let (app, _) = stub(vec![open_request("second")]);
        let mut slots = vec![slot(app)];

        assert!(drain_requests(&mut slots), "slots remain");
        assert_eq!(slots.len(), 2);
        assert!(
            slots[1].window.is_none(),
            "the window is created on the next pass, not in the drain"
        );
        assert_eq!(slots[1].config.title, "second");
    }

    #[test]
    fn a_window_can_close_itself_and_gets_on_exit() {
        let (quitter, exited) = stub(vec![WindowRequest::Close]);
        let (keeper, kept) = stub(Vec::new());
        let mut slots = vec![slot(quitter), slot(keeper)];

        assert!(drain_requests(&mut slots));
        assert_eq!(slots.len(), 1);
        assert!(exited.get(), "the closed app was told");
        assert!(!kept.get(), "the other one was not");
    }

    #[test]
    fn closing_the_last_window_empties_the_run() {
        let (app, exited) = stub(vec![WindowRequest::Close]);
        let mut slots = vec![slot(app)];

        assert!(!drain_requests(&mut slots), "nothing remains");
        assert!(slots.is_empty());
        assert!(exited.get());
    }

    #[test]
    fn a_close_removes_only_the_window_that_asked() {
        let (first, first_out) = stub(Vec::new());
        let (middle, middle_out) = stub(vec![WindowRequest::Close]);
        let (last, last_out) = stub(Vec::new());
        let mut slots = vec![slot(first), slot(middle), slot(last)];

        drain_requests(&mut slots);

        assert_eq!(slots.len(), 2);
        assert!(middle_out.get());
        assert!(!first_out.get() && !last_out.get());
    }

    #[test]
    fn opens_do_not_shift_a_close_index() {
        // Slot 0 opens two windows while slot 1 closes itself. The opens
        // append at the end, so the close's index still points at slot 1.
        let (opener, opener_out) = stub(vec![open_request("a"), open_request("b")]);
        let (closer, closer_out) = stub(vec![WindowRequest::Close]);
        let mut slots = vec![slot(opener), slot(closer)];

        drain_requests(&mut slots);

        assert_eq!(slots.len(), 3);
        assert!(closer_out.get());
        assert!(!opener_out.get(), "the opener was not closed by mistake");
        assert_eq!(slots[1].config.title, "a");
        assert_eq!(slots[2].config.title, "b");
    }

    #[test]
    fn a_window_can_replace_itself() {
        // Open and Close in one drain: the new window appears and only the
        // window that asked goes away.
        let (app, exited) = stub(vec![open_request("replacement"), WindowRequest::Close]);
        let mut slots = vec![slot(app)];

        assert!(drain_requests(&mut slots));
        assert_eq!(slots.len(), 1);
        assert!(exited.get());
        assert_eq!(slots[0].config.title, "replacement");
    }

    #[test]
    fn a_double_close_takes_only_one_window() {
        let (app, exited) = stub(vec![WindowRequest::Close, WindowRequest::Close]);
        let (keeper, kept) = stub(Vec::new());
        let mut slots = vec![slot(app), slot(keeper)];

        drain_requests(&mut slots);

        assert_eq!(slots.len(), 1, "the neighbour survived");
        assert!(exited.get());
        assert!(!kept.get());
    }

    #[test]
    fn closing_an_unknown_slot_is_a_no_op() {
        let (app, exited) = stub(Vec::new());
        let mut slots = vec![slot(app)];

        close_slot(&mut slots, 5);

        assert_eq!(slots.len(), 1);
        assert!(!exited.get());
    }

    #[test]
    fn a_drain_with_no_requests_changes_nothing() {
        let (app, _) = stub(Vec::new());
        let mut slots = vec![slot(app)];

        assert!(drain_requests(&mut slots));
        assert_eq!(slots.len(), 1);
    }

    #[test]
    fn window_requests_describe_themselves() {
        assert_eq!(format!("{:?}", WindowRequest::Close), "Close");
        let debug = format!("{:?}", open_request("second"));
        assert!(debug.contains("second"), "the config shows: {debug}");
    }

    struct DropRecorder {
        seen: Rc<RefCell<Vec<PathBuf>>>,
    }

    impl PixelApp for DropRecorder {
        fn render(&mut self, _buffer: &mut WindowBuffer, _scale: Scale) {}
        fn on_file_drop(&mut self, path: PathBuf) {
            self.seen.borrow_mut().push(path);
        }
    }

    #[test]
    fn dropped_files_reach_the_application_in_platform_order() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let app = DropRecorder { seen: seen.clone() };
        let mut slot = slot(Box::new(app));

        // `note_dropped_file` is exactly what the `DroppedFile` event arm
        // calls; the arm itself needs a live event loop, so the wiring is
        // covered by inspection and the owner's hand test instead.
        slot.note_dropped_file(PathBuf::from("/tmp/b.png"));
        slot.note_dropped_file(PathBuf::from("/tmp/a.png"));

        assert_eq!(
            *seen.borrow(),
            vec![PathBuf::from("/tmp/b.png"), PathBuf::from("/tmp/a.png")]
        );
    }

    #[test]
    fn file_drop_defaults_to_a_no_op() {
        // Existing applications (jxl-batch-studio and older screens) do not
        // override the method; a drop must pass through silently.
        struct Plain;
        impl PixelApp for Plain {
            fn render(&mut self, _buffer: &mut WindowBuffer, _scale: Scale) {}
        }
        let mut app = Plain;
        app.on_file_drop(PathBuf::from("/tmp/a.png"));
    }
}
