# Architecture

liminalkit is an explicit-state, software-rendered GUI toolkit. This is the map of how a frame is produced, where state lives, and what each crate is responsible for. The README says why; this says how.

## The crates

```
liminalkit            front door: re-exports, prelude, App/Screen adapter, examples
├── liminalkit-ui          controls, focus, layout, popups     (raster, text, shell)
├── liminalkit-windowing   OS windows via winit + softbuffer, in-app panes and windows  (raster, shell)
├── liminalkit-kms         Linux DRM/KMS + evdev host          (raster, shell, windowing's PixelApp)
├── liminalkit-shell       Input, Scale, Presenter, Clipboard, touch contacts  (raster)
├── liminalkit-text        TrueType, OpenType shaping, glyph raster, string cache  (raster)
└── liminalkit-raster      WindowBuffer, Painter, Rect, analytic coverage, PNG, SVG icons

liminalkit-test-fonts      Noto Sans + Noto Sans Thai as a FontSet; dev-only, never published  (text)
```

Dependencies point downward only. `raster` and `text` know nothing about windows or input; `shell` is what hosts and applications share; `ui` is the controls over those three; the two hosts sit beside each other and both drive the same `PixelApp`. The facade at the top re-exports everything as `liminalkit::{raster, text, shell, ui, windowing, kms}`.

## A frame

A host (desktop or KMS) owns a `WindowBuffer` of XRGB pixels at the window's device size and calls `PixelApp::render(&mut buffer, scale)` when something needs drawing: input arrived, a `Waker` fired from another thread, or an animation interval elapsed. There is no frame loop while idle.

Inside `render` the application:

1. Lets any open popup claim its input (`Dropdown`, `Menu`, `Overlay::handle_input`) on the frame's `Input`, before anything draws.
2. Builds a `Painter` over the buffer and a `Ui` over the painter, its `TextCache`, the `Input`, its `Theme`, the `Scale` and a `RasterKernel`. `Ui::with_focus` attaches Tab traversal; `Ui::with_semantics` attaches the accessibility collector.
3. Computes rectangles (stateless layout) and calls controls. Each control draws itself and returns what happened: pressed, changed, picked.
4. Draws popups last (`draw`), then calls `Input::end_frame`, which discards unclaimed clicks and keys. Unclaimed input is never carried to the next frame.

`App<S: Screen>` in the facade crate performs steps 2 and 4 and the resource ownership for a `Screen`, whose `before_frame` is step 1 and whose `draw` is step 3. It is a thin `PixelApp`; its source is the reference for writing one by hand.

## Where state lives

In the application's own structs. A control is a function that takes a rectangle and `&mut` to its state. The toolkit keeps no widget tree, no identity map and no per-widget storage:

- Scroll offsets: `ScrollState`, `TableState`.
- Text: `TextFieldState`, `TextAreaState` (caret as a byte offset on a grapheme boundary; the stored UTF-8 is never normalised).
- Selection and tabs: a `usize` or an `Option<Key>` the screen owns.
- Keyboard focus: `Focus<K>`. With the default `K = usize` the draw-order index is the name and `Ui` registers standard controls automatically. With an application enum, `register_id` keeps focus with the named control across insertions and reorders.
- Popups: `Dropdown`, `Menu`, `Overlay` hold open/closed, placement and this frame's claimed keys. A dialog owns its own `Focus`.
- In-app layout: `Panes` and `Windows` hold fractions and rectangles; the application draws into what they report.

The rule for anything new: ownership, allocations and any state kept between frames are visible in the signature.

## Units

Every `Rect` a control or the painter takes is in device pixels: the pixels the buffer holds and the pointer reports. Layout is written in logical pixels and converted once at the call site (`Ui::px`, `Ui::rect`, `Scale::rect`). `Theme`'s sizes are logical and each control scales them itself. `Theme`'s two font styles are converted once by `Ui::new` so measuring and drawing share one cache key.

## Layout

Plain functions from a rectangle to rectangles, nothing retained: `vstack`/`hstack` with `Length::{Px, Flex, FlexMin}`, the `Flow` cursor, `Insets`, `place`/`centre`, and `grid`/`grid_visible_range` for identical cells. `Rect::split_*` and arithmetic mix freely with them. There is no constraint solver and no retained layout hierarchy.

## Text

`liminalkit-text` parses TrueType, applies the OpenType GSUB/GPOS lookups the bundled Latin and Thai faces use (ligatures, kerning, mark attachment), rasterises glyphs with the same analytic coverage kernel as vector paths, and caches shaped and rendered strings by `(text, style)`. The application embeds its faces and registers them in a `FontSet` with fallback chains. It is not a full Unicode shaper: unsupported lookups are skipped. Direction marks are never drawn. With the `bidi` feature, `shape` resolves each line with the Unicode Bidirectional Algorithm (`unicode-bidi`), shapes right-to-left runs in reverse with mirrored brackets, and places runs in display order; clusters still index the logical string, so fitting and wrapping are unchanged. Controls measure and edit in logical order, so a caret in right-to-left text sits at the logical prefix's width.

## Rendering

`Painter` draws fills, strokes, rounded rectangles, closed paths under non-zero or even-odd rules, bitmaps and prepared images into the XRGB buffer with a clip stack. Coverage for paths and glyphs comes from one analytic kernel. SIMD paths are checked byte for byte against the scalar specification and can be disabled with `LIMINALKIT_SIMD=scalar`. Cost scales with resolution and redraw area.

## The two hosts

`liminalkit-windowing::run_app` opens a winit window and presents the buffer through softbuffer (or any `Presenter` the application supplies). It tracks the scale factor, forwards keys, IME, pointer, scroll, touch, file drops and theme changes, sleeps until a wake, and with `accessibility` publishes the application's AccessKit tree. `run_windows` shares one event loop between several windows, each with its own `PixelApp`.

`liminalkit-kms::run_kms` runs the same `PixelApp` full screen on a bare Linux display: one copy into a DRM scan-out buffer per frame, paced by the page flip, with evdev multi-touch and no window system. It gives up the keyboard, IME, clipboard, cursor and multiple windows. The application cannot tell which host called it.

## Touch

The application owns hit testing and gesture policy. `on_touch` delivers contacts in device pixels; `TouchState` tracks them and reports pan and pinch; `TouchPointer` turns the first contact into a pointer press for controls that already understand one. The gesture recogniser and scroll physics in `shell` are a separate layer that compiles and is unit-tested but has not been run on a touchscreen.

## Accessibility

With the `accessibility` feature, `Ui` records every control it draws into a `Semantics` collector (role, label, bounds, focus, value, text selection). The host publishes it through AccessKit after each paint and delivers action requests back, which `Semantics::action` turns into ordinary input: a click at the control's centre, a focus request, a text edit. There is no second widget tree.
