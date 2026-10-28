# liminalkit

liminalkit is a software-rendered UI toolkit for desktop and touchscreen applications. The application owns the screen and draws every frame into a pixel buffer. There is no retained widget tree and no identity map.

Source: <https://github.com/liminalism/liminalkit>

## How a screen is put together

A screen is a struct. Scroll offsets, the selected row, the text in a field, and which pane is which are ordinary fields. A control is a function: you give it a rectangle and `&mut` access to its state, it draws, and it returns what happened. Reordering controls cannot move state to the wrong one, because the toolkit never stored that state.

The host calls `PixelApp::render` with a `WindowBuffer` and a `Scale`. Build a `Painter` over the buffer and a `Ui` over the painter, the text cache, this frame's `Input`, the theme, and the scale. Place a panel, then draw into the inner rectangle it returns. Coordinates passed to `Ui` are logical pixels. Call `Input::end_frame()` once the frame's input has been consumed.

`Theme` body and heading sizes are logical. `Ui::new` converts its own copy to device pixels. Keep the unscaled theme and pass that into the next `Ui`.

In-app layout is the same idea, one level up, and still owned by the caller:

- `Panes` tiles a region. Every interior edge is a divider. Dragging one side grows a pane and shrinks its neighbour by the same amount. Positions are fractions, so a resize keeps each pane's share.
- `Windows` is a window manager inside one OS window. Windows move by the title bar, resize from the edges and corners, raise on press, and close. Only the topmost window under a press can take it.

Update panes or windows once, before drawing, so chrome claims its press ahead of the content underneath. Several operating-system windows are a different arrangement: `run_windows` shares one event loop, and each window has its own `PixelApp`.

The live shape of this is `crates/liminalkit-ui/examples/window.rs`.

## Two hosts, one application

`run_app` opens a winit window and presents through softbuffer. `run_kms` runs the same `PixelApp` on a bare Linux display: one copy into a DRM/KMS scan-out buffer, frames paced by the page flip, evdev multi-touch, and no window system. The application cannot tell which host called it.

The KMS host gives up the keyboard, IME, clipboard, multiple windows, and a cursor. It needs DRM master, which means a virtual terminal with no desktop and membership in the `video` and `input` groups. `probe` lists the displays this process can see without taking one. The touch decoder, rotation, and frame pacing are unit-tested. Setting a mode belongs on a real panel.

A different present path implements the shell's `Presenter` and is started with `run_app_with_presenter`. The default host uses `SoftbufferPresenter`.

Raster and text do not know about either host. The shell is what they share with the hosts: per-frame input, logical-to-physical scale, the presenter seam, and the clipboard. The application embeds its own fonts.

## Crates

| Crate | Role |
|---|---|
| [`liminalkit-raster`](crates/liminalkit-raster) | An XRGB buffer, a clipped painter, and analytic coverage for paths and glyphs. PNG, images prepared once for the size they are drawn at, and SVG icons rasterized once and tinted. |
| [`liminalkit-text`](crates/liminalkit-text) | TrueType faces, the OpenType substitutions and positioning this stack implements, wrapping, and a string cache. |
| [`liminalkit-shell`](crates/liminalkit-shell) | `Input`, `Scale`, `Presenter`, the clipboard, and touch contacts. |
| [`liminalkit-ui`](crates/liminalkit-ui) | Immediate-mode controls over those three. |
| [`liminalkit-windowing`](crates/liminalkit-windowing) | OS windows, draggable panes, and floating in-app windows. `winit` and `softbuffer` stay here. |
| [`liminalkit-kms`](crates/liminalkit-kms) | The Linux DRM/KMS and evdev host for a `PixelApp`. |

The workspace forbids `unsafe`. Rust 1.85 or newer.

Drawing is fills, strokes, and closed paths under the non-zero or even-odd rule, plus bitmaps and prepared images. Coverage for vectors and glyphs is the same analytic kernel. Fast paths are checked byte for byte against a scalar specification. `LIMINALKIT_SIMD=scalar` turns them off. Time them on the device.

Text shaping covers the lookups used by the bundled Latin and Thai sample fonts, including ligatures, kerning, and mark attachment. It is not a full Unicode shaper. Unsupported lookups are skipped. A text field edits on extended-grapheme boundaries and keeps the stored UTF-8 un-normalized. Committed key text is applied to `TextFieldState`. IME preedit is painted from `Input`.

The controls are the ones an operational screen is made of: labels, buttons, tabs, checkboxes, toggles, radios, dropdowns, text fields, and tables and lists that draw only the visible rows. `Theme` holds semantic colours and sizes. `OperationalPalette` is a shared light and dark vocabulary, so screens can differ in layout and still belong to one installation. Chrome that mixes several meanings on one screen takes its colours as arguments. Tooltips are requested while drawing and painted after the rest of the frame, so they sit above it.

`accessibility` on `liminalkit-ui` and `liminalkit-windowing` can expose the controls that were drawn, through AccessKit. It is off unless the application opts in.

## Touch

The application owns hit testing and gesture policy. `PixelApp::on_touch` receives each contact: a device and contact id, a phase, physical buffer coordinates, and optional pressure. `TouchState` tracks the contacts that are down and reports pan and pinch. `TouchPointer` can give the first finger to controls that already understand a press. That is activation on press, not a tap recognizer, and it will not choose between a scroll and a tap. Feed each contact to one owner. Some platforms also synthesize mouse events for a touch.

`gesture`, `scroll_physics`, and `touch_replay` are a separate layer. They compile, and their behaviour has not been run on a touchscreen. The defaults are a starting point, not a calibrated profile. One recognizer belongs to one surface the application captured on contact start. There is no global gesture arena. The playground is `touch_tuning`, and it labels itself untested.

## Build

```sh
cargo test --workspace

# Gallery to target/gallery.png, with no window
cargo run -p liminalkit-ui --example gallery --features test-fonts

# Interactive demo, tiled panes, two OS windows
cargo run -p liminalkit-ui --example window --features test-fonts
cargo run -p liminalkit-windowing --example panes
cargo run -p liminalkit-windowing --example multi

# Contacts, and the untested tuning playground
cargo run -p liminalkit-windowing --example touch
cargo run -p liminalkit-windowing --example touch_tuning

# Displays this process can see, without taking one
cargo run -p liminalkit-kms --example probe
```

`test-fonts` embeds Noto Sans and Noto Sans Thai for examples and tests. Application builds leave it off. Font details are in [`crates/liminalkit-text/test-fonts/README.md`](crates/liminalkit-text/test-fonts/README.md).

`LIMINALKIT_FRAME_LOG=1` prints p50, p95, and maximum timings for the phases the shell was given.

## License

`MIT OR Apache-2.0`. Provenance, and the license for the sample fonts, is in [`ATTRIBUTION.md`](ATTRIBUTION.md).
