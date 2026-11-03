# liminalkit

An explicit-state, software-rendered GUI toolkit for Rust.

liminalkit is for applications that need direct control over drawing, interaction, and resource use: desktop utilities, operational screens, kiosks, and embedded Linux displays. The application owns its state, its layout, and its pixels. The toolkit supplies drawing and interaction primitives. There is no browser, no retained widget tree, no widget identity map, and no mandatory GPU stack. The same application runs in a desktop window or directly on a Linux DRM/KMS display.

Source: <https://github.com/liminalism/liminalkit>. Not yet published to crates.io.

## Why this and not egui, Dioxus, or Freya

- **State lives in your structs.** A control is a function: it takes a rectangle and `&mut` to its state, draws, and returns what happened. Nothing is hashed, nothing is looked up by a name that could collide, and reordering a screen cannot move state to the wrong control. egui is also immediate-mode but keeps interaction state and widget ids inside the framework.
- **Software rendering is the architecture, not a fallback.** Every frame is drawn into an XRGB pixel buffer by a clipped painter with analytic anti-aliasing. Idle screens cost nothing: the host sleeps until input or a wake arrives. Cost scales with resolution and redraw area, which suits a 1080p dashboard or a Raspberry Pi panel better than a highly animated 4K interface.
- **Two hosts, one application.** `run_app` opens a winit window and presents through softbuffer. `run_kms` runs the same application full screen on a bare Linux display with evdev multi-touch and no window system.
- **A small number of understandable abstractions.** Every piece of state a control needs is a parameter. A convenient API is welcome; a hidden mechanism that undermines predictability needs a strong reason.

What it does not try to be: a universal application framework. Layout is written by the application with stateless solvers, not by a constraint engine. Text shaping covers the lookups the bundled Latin and Thai fonts use, not all of Unicode. The widget set is the one an operational screen is made of.

## Quick start

```toml
[dependencies]
liminalkit = { git = "https://github.com/liminalism/liminalkit", features = ["test-fonts"] }
```

```rust
use liminalkit::prelude::*;

struct Hello {
    clicks: u32,
}

impl Screen for Hello {
    fn draw(&mut self, ui: &mut Ui<'_>, ctx: &mut Context) {
        ui.painter.clear(ui.theme.background);
        // Layout in logical pixels, converted once; the controls take device pixels.
        let rows = vstack(ui.rect(16, 16, 240, 80), 8, &[Length::Px(28), Length::Px(36)]);
        ui.heading(rows[0], &format!("{} clicks", self.clicks));
        if ui.button(rows[1], "Click me") {
            self.clicks += 1;
        }
        if ctx.key_pressed(KeyInput::Escape) {
            ctx.exit = true;
        }
    }
}

fn main() {
    liminalkit::run(
        Hello { clicks: 0 },
        liminalkit::text::font::test_fonts::set(),
        WindowConfig::new("hello", 320.0, 200.0),
    )
    .expect("run");
}
```

That is `crates/liminalkit/examples/hello.rs`. `App` owns the text cache, raster kernel, per-frame `Input`, and keyboard `Focus`, forwards every host event, and ends the frame. A `Screen` keeps its own state as ordinary fields. An application that wants to own all of that itself implements `PixelApp` directly; `crates/liminalkit/examples/window_lowlevel.rs` is the same screen written that way, and `crates/liminalkit/src/app.rs` is the whole adapter.

`test-fonts` embeds two Noto faces so a first run has something to draw with. Application builds leave it off and register their own faces in a `FontSet`.

## How a screen is put together

A screen is a struct. Scroll offsets, the selected row, the text in a field, and which pane is which are ordinary fields. A control is a function: you give it a rectangle and `&mut` access to its state, it draws, and it returns what happened. Reordering controls cannot move state to the wrong one, because the toolkit never stored that state.

The host calls `PixelApp::render` with a `WindowBuffer` and a `Scale`. Build a `Painter` over the buffer and a `Ui` over the painter, the text cache, this frame's `Input`, the theme, and the scale. Place a panel, then draw into the inner rectangle it returns. Call `Input::end_frame()` once the frame's input has been consumed. `App` does all of this for a `Screen`.

### Units

Every rectangle a control takes is in device pixels: the pixels the painter writes and the pointer reports, so hit-testing compares what was drawn with where the pointer is. Layout is written in logical pixels and converted once, at the call site: `Ui::px` for a length, `Ui::rect` for a rectangle, or `Scale::rect` over a whole logical layout. `Theme`'s sizes (`padding`, `row_height`, `corner_radius`, `border_width`, `focus_width`) are logical and the controls scale them. Its two font styles are the exception: `Ui::new` converts its own copy to device pixels so measuring and drawing share one key into the text cache. Keep the unscaled theme and pass that into the next `Ui`; convert a custom style with `Ui::text_style`.

### Layout

Layout is plain functions from a rectangle to rectangles, with nothing retained between frames. `vstack` and `hstack` divide an area by a list of `Length`s: fixed pixels, a weighted share of the remainder, or a share with a minimum. `Flow` is a running cursor for a screen that takes a title here and a status line there and gives the rest to the body. `Insets` and `place` handle margins and alignment. `grid` and `grid_visible_range` lay out identical cells and say which are on screen. Manual arithmetic over `Rect` mixes freely with all of them because they produce the same thing.

### Keyboard focus

`Focus` is one small value a screen owns. Standard controls register with it in draw order when a `Ui` is built `with_focus`, and Tab moves between them. A screen whose controls come and go, or move, names them with its own enum instead: `Focus<Field>` with `register_id`, so focus follows the name rather than the slot. See `crates/liminalkit/examples/window.rs`.

In-app layout is the same idea, one level up, and still owned by the caller:

- `Panes` tiles a region. Every interior edge is a divider. Dragging one side grows a pane and shrinks its neighbour by the same amount. Positions are fractions, so a resize keeps each pane's share.
- `Windows` is a window manager inside one OS window. Windows move by the title bar, resize from the edges and corners, raise on press, and close. Only the topmost window under a press can take it.

Update panes or windows once, before drawing, so chrome claims its press ahead of the content underneath. Several operating-system windows are a different arrangement: `run_windows` shares one event loop, and each window has its own `PixelApp`.

The live shape of this is `crates/liminalkit/examples/window.rs`.

## Two hosts, one application

`run_app` opens a winit window and presents through softbuffer. `run_kms` runs the same `PixelApp` on a bare Linux display: one copy into a DRM/KMS scan-out buffer, frames paced by the page flip, evdev multi-touch, and no window system. The application cannot tell which host called it.

The KMS host gives up the keyboard, IME, clipboard, multiple windows, and a cursor. It needs DRM master, which means a virtual terminal with no desktop and membership in the `video` and `input` groups. `probe` lists the displays this process can see without taking one. The touch decoder, rotation, and frame pacing are unit-tested. Setting a mode belongs on a real panel.

A different present path implements the shell's `Presenter` and is started with `run_app_with_presenter`. The default host uses `SoftbufferPresenter`.

Raster and text do not know about either host. The shell is what they share with the hosts: per-frame input, logical-to-physical scale, the presenter seam, and the clipboard. The application embeds its own fonts.

## Crates

| Crate | Role |
|---|---|
| [`liminalkit`](crates/liminalkit) | The front door: re-exports the crates below as `liminalkit::{raster, text, shell, ui, windowing, kms}`, plus `App` and `Screen`. Features: `test-fonts`, `accessibility`, `kms`. |
| [`liminalkit-raster`](crates/liminalkit-raster) | An XRGB buffer, a clipped painter, and analytic coverage for paths and glyphs. PNG, images prepared once for the size they are drawn at, and SVG icons rasterized once and tinted. |
| [`liminalkit-text`](crates/liminalkit-text) | TrueType faces, the OpenType substitutions and positioning this stack implements, wrapping, and a string cache. |
| [`liminalkit-shell`](crates/liminalkit-shell) | `Input`, `Scale`, `Presenter`, the clipboard, and touch contacts. |
| [`liminalkit-ui`](crates/liminalkit-ui) | Immediate-mode controls over those three, keyboard focus, and stateless layout. |
| [`liminalkit-windowing`](crates/liminalkit-windowing) | OS windows, draggable panes, and floating in-app windows. `winit` and `softbuffer` stay here. |
| [`liminalkit-kms`](crates/liminalkit-kms) | The Linux DRM/KMS and evdev host for a `PixelApp`. |

The workspace forbids `unsafe`. Rust 1.85 or newer.

Drawing is fills, strokes, and closed paths under the non-zero or even-odd rule, plus bitmaps and prepared images. Coverage for vectors and glyphs is the same analytic kernel. Fast paths are checked byte for byte against a scalar specification. `LIMINALKIT_SIMD=scalar` turns them off. Time them on the device.

Text shaping covers the lookups used by the bundled Latin and Thai sample fonts, including ligatures, kerning, and mark attachment. It is not a full Unicode shaper. Unsupported lookups are skipped. A text field edits on extended-grapheme boundaries and keeps the stored UTF-8 un-normalized. Committed key text is applied to `TextFieldState`. IME preedit is painted from `Input`.

The controls are the ones an operational screen is made of: labels, buttons, tabs, checkboxes, toggles, radios, dropdowns, context menus, single- and multi-line text fields, modal dialogs and popovers, drag and drop, and tables and lists that draw only the visible rows. `Theme` holds semantic colours and sizes. `OperationalPalette` is a shared light and dark vocabulary, so screens can differ in layout and still belong to one installation. Chrome that mixes several meanings on one screen takes its colours as arguments. Tooltips are requested while drawing and painted after the rest of the frame, so they sit above it.

`accessibility` on `liminalkit-ui` and `liminalkit-windowing` can expose the controls that were drawn, through AccessKit. It is off unless the application opts in. Under `App` the same feature publishes the tree and routes actions without any work in the screen; this has been exercised in tests, not yet with a screen reader.

### Popups and overlays

Immediate mode has no z-order, so anything that floats above the screen is drawn last and claims its input first. `Dropdown`, `Menu` (a context menu opened by a right-click or at a point), and `Overlay` (a modal dialog centred in the window, or a popover hung from a control) all work the same way: a value the screen owns; a `handle_input` call at the start of the frame, before the `Ui` is built, that takes the popup's clicks and keys so the background cannot act on them (`Screen::before_frame` under `App`); and a `draw` call at the end of the frame that paints it above everything and reports what happened. A dialog keeps its own `Focus`, so Tab stays inside it. `Drag` carries a payload from a source rectangle to a target one and draws its ghost last. `TextAreaState` is the multi-line field: wrapped lines, Up and Down by line, Home and End by line, Enter for a line break, over the same grapheme-safe editing as the single-line field.

## Touch

The application owns hit testing and gesture policy. `PixelApp::on_touch` receives each contact: a device and contact id, a phase, physical buffer coordinates, and optional pressure. `TouchState` tracks the contacts that are down and reports pan and pinch. `TouchPointer` can give the first finger to controls that already understand a press. That is activation on press, not a tap recognizer, and it will not choose between a scroll and a tap. Feed each contact to one owner. Some platforms also synthesize mouse events for a touch.

`gesture`, `scroll_physics`, and `touch_replay` are a separate layer. They compile, and their behaviour has not been run on a touchscreen. The defaults are a starting point, not a calibrated profile. One recognizer belongs to one surface the application captured on contact start. There is no global gesture arena. The playground is `touch_tuning`, and it labels itself untested.

`Gesture::TransformStarted` ends single-contact recognition while retaining
capture for a multi-contact transform. `Gesture::Cancelled` ends the entire
interaction without a fling; it is followed by `Released`. Returning from a
transform to one contact rebases drag recognition and never restores tap eligibility.

The desktop host retains its window and presenter while suspended, pauses native
input and rendering, cancels touches and modifiers, and notifies the application
of focus loss and IME disablement.
Presenter resize readiness is recorded only after the backend succeeds, allowing
a failed resize to retry on the next redraw. KMS owns one frame's timing until
its matching flip completes, fails with an error after a two-second missing-flip
deadline, and reconciles touchscreen hotplug every two seconds without replacing
existing device readers.

## Build

```sh
cargo test --workspace --features liminalkit/test-fonts

# Gallery to target/gallery@1x.png (or @2x.png), with no window; an optional scale factor
cargo run -p liminalkit-ui --example gallery --features test-fonts
cargo run -p liminalkit-ui --example gallery --features test-fonts -- 2.0

# The quick start, the interactive demo through App, and the same demo over PixelApp
cargo run -p liminalkit --features test-fonts --example hello
cargo run -p liminalkit --features test-fonts --example window
cargo run -p liminalkit --features test-fonts --example window_lowlevel

# Tiled panes with floating windows, and two OS windows
cargo run -p liminalkit --features test-fonts --example panes
cargo run -p liminalkit --features test-fonts --example multi

# Contacts, and the untested tuning playground
cargo run -p liminalkit --features test-fonts --example touch
cargo run -p liminalkit --features test-fonts --example touch_tuning

# Displays this process can see, without taking one
cargo run -p liminalkit-kms --example probe
```

`test-fonts` embeds Noto Sans and Noto Sans Thai for examples and tests. Application builds leave it off. Font details are in [`crates/liminalkit-text/test-fonts/README.md`](crates/liminalkit-text/test-fonts/README.md).

`LIMINALKIT_FRAME_LOG=1` prints p50, p95, and maximum timings for the phases the shell was given.

## Contributing

[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) is the map: crates, the frame, units, focus, popups, the two hosts. [`CONTRIBUTING.md`](CONTRIBUTING.md) has the build, test, and review expectations. `AGENTS.md` describes the project's knowledge ledger, which contributors do not need to use.

## License

`MIT OR Apache-2.0`. Provenance, and the license for the sample fonts, is in [`ATTRIBUTION.md`](ATTRIBUTION.md).
