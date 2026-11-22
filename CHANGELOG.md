# Changelog

One version for the whole workspace. The method is the "Versioning" section
of `CONTRIBUTING.md`: before 1.0 the minor number counts API generations, and
a generation closes when a change lands after which an application had to
edit its own code. None of the versions below were published to crates.io;
applications consumed them from git, which is why they count. The dates are
the commits that closed each generation.

## 0.5.0 (unreleased)

The adoptability generation: the toolkit became something a project other
than the one it was extracted from can pick up.

### Breaking

- Interaction and host lifecycle transitions are explicit (`6ba1d36`):
  `Input` has interaction epochs and `PixelApp` has lifecycle callbacks. A
  host or test that drove `Input` by hand follows the new sequence.
- Every rectangle a control takes is in device pixels; `Theme` sizes are
  logical and the controls scale them (`2d8fbfd`, `ee8b35d`). A layout that
  pre-scaled theme tokens itself now double-scales; use `Ui::rect`,
  `Ui::px` or `Scale::rect` once and pass the result.
- `Focus` is `Focus<K>`; `Focus::new()` with no `Ui` nearby needs a type.
- Popups (`Dropdown`, `Menu`, `Overlay`) claim input in a pre-frame step
  before the `Ui` is built and draw last; under `App` that step is
  `Screen::before_frame`.
- The `test-fonts` feature is gone from `liminalkit-text`, `liminalkit-ui`
  and `liminalkit`. The Noto fixtures live in `liminalkit-test-fonts`, an
  unpublished dev-only crate, so no published crate and no application
  carries them. Applications embed their own faces, as they always should
  have.
- Cross-crate examples moved to the `liminalkit` facade crate;
  `liminalkit-ui` and `liminalkit-windowing` no longer dev-depend on each
  other.
- Direction marks and embeddings (U+200E, U+200F, U+202A..U+202E,
  U+2066..U+2069, U+061C) are never drawn; they used to render as `.notdef`
  boxes.
- Text is drawn with gamma-corrected coverage by default
  (`TextRendering::DEFAULT`: gamma 1.8, contrast 0.5), so glyph edges land
  on different pixel values than before; layout and measurements do not
  change. An application that compares pixels against renders from earlier
  versions, or wants the old look, sets
  `TextCache::set_rendering(TextRendering::UNCORRECTED)`.

- A wheel line is no longer 24 physical pixels. `PixelApp::on_scroll` and
  `ScrollEvent`'s pixel fields now give a notch three lines of 100/3
  logical pixels at the window's scale: 100 logical pixels a notch, the
  same distance on screen at any display scale. An application that
  tuned its own multiplier against the old 24 moves about four times as far
  per notch at 100% scale.
- `App` takes wheel input through `PixelApp::on_wheel` and no longer
  implements `on_scroll`. A host that drove an `App` by calling
  `on_scroll` by hand calls `on_wheel` with a `WheelSample`.
- `ScrollState` is no longer `Eq` (it carries a fraction of a pixel), and
  the table, scroll list and text area move by a wheel *line* of their own
  content: a table by rows, a text area by lines of text, three to a notch.
- `GesturePhase` lives in `liminalkit_shell::scroll`;
  `liminalkit_windowing::GesturePhase` is the same type re-exported.
- A drag no longer ends when the pointer leaves the window.
  `Input::cursor_left` with a button held keeps the button held and the
  cursor reported (the platform goes on delivering both while it has the
  pointer captured); `Input::hovering` is false until the release, after
  which the cursor is gone. Focus loss (`Input::blur`) and
  `Input::cancel_pointer` still end it. Code that relied on leaving the
  window to cancel a drag watches for the release.
- `ScrollState` is no longer plain data: it holds the wheel interpreter and
  a motion controller for each axis, and lives in `liminalkit_ui::scroll`
  (still re-exported from the crate root). With a frame clock
  (`Input::begin_frame`, which `App` supplies) the table, scroll list and
  text area ease a wheel notch in over a few frames, accelerate a spun
  wheel, and ask for those frames themselves; a host that never calls
  `begin_frame` gets the old immediate movement. `ScrollState::reveal`
  measures from where the view is heading.
- `CursorShape` lives in `liminalkit_shell`; `liminalkit_windowing`
  re-exports it. Under `App`, a control's `Input::request_cursor` overrides
  `Context::cursor` for that frame.
- A focused `text_area` follows its caret only when the caret moved, the
  text changed or it has just taken focus. It used to bring the caret back
  into view on every frame, which undid any wheel scroll away from it.

### Added

- **Window icons.** `WindowConfig::icon` takes a `WindowIcon`: straight
  RGBA pixels (X11 and Windows), or `WindowIcon::WindowsResource(ordinal)`,
  which loads an icon compiled into the executable at the small and large
  sizes for the window's scale factor, so Windows picks the matching image
  instead of stretching one. Wayland and macOS take the icon from the desktop
  entry or app bundle and ignore it. The field is additive; code that builds
  `WindowConfig` with a struct literal adds `icon: None`.

- **Experimental: game UI, as a new crate.** `liminalkit-game`, re-exported as
  `liminalkit::game` behind the facade's `game` feature, is an interface over
  a rendered scene. A `Layer` is a `Surface` (a premultiplied buffer the
  ordinary `Ui` paints into, with its own `Input` in its own coordinates), a
  `Sheet` (where that picture goes this frame) and `Effects` (shimmer, flash,
  glow, scanlines, pop). Moving, fading and every effect are done to the
  painted picture, so nothing is repainted to animate. `anim` has `Tween`,
  `Spring`, `Ease` and `Envelope`, driven by host time and settling at once
  when there is no clock; `notify::NoticeQueue` is a notification stack with
  priorities, lifetimes that pause on hover, coalescing and animated
  enter, leave and make-room; `anchor::place` puts a popover beside a moving
  screen point. `input::Router` gives each pointer event to the topmost layer
  it is really on and reports whether the interface took it.
  A `Sheet` can lie on a `Sphere`, `Cylinder` or `Plane` seen through the
  host's camera (`view::View`, with a ready `Pinhole`): a window pinned to a
  planet turns with it, slides over the surface when dragged with the held
  point under the cursor, peels off into a flat window past the horizon, and
  lays back down when dropped. Its controls work throughout, because input is
  carried back through the mesh that was drawn.
  One `GameFrame` is drawn by `composite::composite` on the CPU or by a host's
  GPU from `GameFrame::gpu` and the reference WGSL in `gpu`: textures with
  revisions and damage, meshes, and a 144-byte uniform block. The crate has no
  GPU dependency. With `default-features = false` it needs only `std` and
  keeps animation, effect parameters, notifications, anchoring and geometry.
  Not known: the shader is validated and mirrored in Rust but has not run on a
  GPU from this workspace; durations and fades are judged by eye on one
  desktop; the CPU compositor measured about 13 ms a frame for the example's
  interface at 1280×720, and is a reference and fallback rather than a way to
  run a busy interface at 4K. `cargo run --release -p liminalkit --example
  planet --features game`.
- `liminalkit_raster::mesh`: `Painter::draw_textured_mesh` draws a
  premultiplied buffer through a mesh of screen-space triangles, each vertex
  naming its texel, opacity and light: the one primitive that is not
  axis-aligned. Positions snap to 1/256 px and coverage is by integer edge
  functions under the top-left rule, so a translucent mesh has no doubly
  blended seams; a one-to-one mesh reproduces its texture byte for byte; the
  texture's edge is anti-aliased analytically; optional smaller levels are
  sampled where texels land smaller than pixels.
  `sample_bilinear_premultiplied` is its sampler.
- `Painter::draw_buffer` composites a region of a premultiplied
  `WindowBuffer` at an opacity: an offscreen layer onto a frame.
  `blend::over_span_faded` is its row, and `blend::scale_premultiplied` is now
  public.
- Wheel input arrives whole. `liminalkit_shell::scroll::WheelSample` is one
  wheel or touchpad movement in the unit the platform reported (120ths of a
  notch on Windows, lines, or pixels), with its phase, the modifiers held
  and when the host received it; `PixelApp::on_wheel` delivers it and
  `Input::wheel` collects it. `Input` keeps each sample of a frame rather
  than their sum: `Input::consume_wheel` hands them over one by one,
  `Input::take_scroll_pixels` converts them with the caller's own line and
  page size and rounds nothing, and `Input::take_scroll` is unchanged for
  callers that want the simple form. `ScrollSettings` holds the system's
  lines-per-notch (default three). `ScrollState::scroll_by_precise` moves
  both axes by fractional distances and carries what does not yet make a
  pixel, so a touchpad reporting a third of a pixel a frame scrolls slowly
  instead of not at all; `ScrollState::offset_x` is the horizontal offset.
  `scrollbar_thumb` is the one thumb calculation, in 64-bit arithmetic,
  with a minimum length that follows the display scale. Experimental: not
  yet calibrated against real devices.
- `liminalkit_shell::scroll::WheelTrace` writes wheel samples as text and
  reads them back exactly, and the `scroll_capture` example records what a
  device sends to a file. `WheelTrace::frames` cuts a recording into the
  frames a window redrawing at a given interval would have seen.
- `liminalkit_shell::scroll::WheelInterpreter` says how far a wheel
  movement goes, and nothing else: it holds no position, so an application
  with its own scroll model can use it alone. It classes each sample from
  its value (`InputClass::Notched` for whole notches, `Fine` for fractions,
  `Precise` for pixel or phased input), converts it with the caller's own
  line, page and pixel (`Units`, in any unit: pixels or pages), and
  accelerates whole notches by how quickly they follow one another
  (`Cadence`), never by how large one is. Continuous input is never
  accelerated. `WheelProfile::{precision, natural, accelerated,
  rapid_navigation}` are the presets; Ctrl+wheel comes back as
  `Interpreted::Zoom` unconverted, Shift turns the wheel sideways, and Alt
  multiplies. The classifier and the presets were written against three
  recordings kept in `crates/liminalkit-shell/tests/scroll_traces` (a
  coarse wheel, a finely graded wheel and a precision touchpad on Windows)
  and are otherwise uncalibrated.
- `liminalkit_shell::scroll::MotionController`: one axis of position in any
  unit, moved over time. A wheel movement is added to a target when it
  arrives and the position closes on it by a formula in elapsed time
  (`Smoothing::Exponential`, or `Smoothing::None`), so smoothing changes how
  a scroll looks and never how far it goes, and a recording replays to the
  same positions at any frame rate. Also `nudge` (direct), `jump_to`,
  `animate_to`, `halt`, a speed limit, and a `Ledger` of what was asked for,
  accepted, refused at an end and dropped on reversing.
- The `scroll_tuning` example: a long document driven by the two, with the
  profile, smoothing, lead, device mode and speed limit on the keys, a
  readout of what each wheel movement was taken for, and record and replay.
- Scroll regions. `ScrollState::update` is one frame of anything that
  scrolls: it takes the wheel that belongs to the region, moves, and
  returns the whole-pixel offset to draw and hit-test at. A press on the
  content stops a scroll where it was drawn. `ScrollState::scrollbar` makes
  a bar interactive: the thumb drags one to one, including off the bar and
  out of the window, and a press beside it pages towards the pointer and
  repeats while held; the table's and the list's bars now do both.
  `Ui::scroll_area` scrolls arbitrary content on both axes with bars where
  needed. `ScrollTuning` (kept in the state) sets the wheel profile,
  smoothing, lead, page time, `MiddleButton` (`Pan` by default; `Off`,
  `Autoscroll`, `Both`) and `ScrollBoundary`. Also `animate_to`, `jump_to`,
  `reveal_animated`, `target`, `is_animating`, `halt`.
- Nested regions. `Input::route_wheel` gives the wheel to the region inside
  another even though the outer one draws and asks first, by remembering
  for one frame which regions were drawn. `ScrollBoundary::Auto` (default)
  lets a roll that starts at an inner region's end move the outer one, but
  keeps a roll that runs out part-way where it is; `Contain` never passes
  it on; `Chain` hands the unused distance outward at once.
- `Input`: `is_middle_held`, `take_middle_click`, `middle_press_position`,
  `released` (the button that came up this frame and where),
  `request_cursor`/`take_cursor_request`; `liminalkit_shell::scroll::
  host_time` is the clock samples and frames share.
- `MotionController::fling`: let go at a velocity and coast to a stop the
  controller knows from the moment of launch (`target`), stopping dead at
  an end. `ScrollTuning::fling` (off by default) makes content dragged with
  the pointer carry on when it is let go while moving, and
  `ScrollTuning::drag_pan` makes the left button drag the content like a
  hand tool, with the open and closed hand cursors. There is no overscroll
  in the new controller; `scroll_physics::ScrollAxis` keeps its elastic
  ends for a touch surface that wants them.
- With `liminalkit-native` on Windows the system's wheel settings are read
  (lines per notch, a page per notch, or the wheel off; characters
  sideways) at start-up and whenever they change, and delivered through the
  new `PixelApp::on_scroll_settings`, which `App` passes to `Input`. The
  plain host still assumes three lines. Experimental: a wheel sample is
  marked `SourceHint::Touchpad` when the system says a touchpad sent the
  message. On the one machine tried it said so for none of 89 touchpad
  messages, which matches the documented need for the thread to register
  as touchpad-aware; no registration is done, and the classifier does not
  depend on the hint. Build `scroll_capture` with `--features native` to
  see what a machine reports.
- A clock for anything that moves. `PixelApp::on_frame` is called before
  each `render` with `FrameInfo { now, interval }`, where `interval` is one
  frame of the window's display (from its refresh rate, a sixtieth of a
  second when it does not say). `Input::begin_frame` and `Input::now` carry
  the time to the controls, and `Context::now` and `Context::frame_interval`
  to a `Screen`. `Input::request_frame`, or `Context::request_frame`, asks
  for one more frame; `App` turns that into an `animation_interval` of one
  display frame for as long as something keeps asking and goes back to
  sleeping when nothing does. The KMS host calls `on_frame` too, with the
  panel's frame as the interval (`Pacer::frame`).
- A wheel sample delivered straight after a slow tick or frame is marked
  `after_busy`: it waited in the queue, so its distance in time from its
  neighbours is not how fast the wheel was turning.
- `FrameTimer::record_wheel` and `FrameSnapshot::wheel_to_present`: the time
  from a wheel movement to the frame that showed it, printed with the frame
  log. A `FrameSnapshot` built by hand needs the new field.

- `liminalkit-native`: operating-system controls inside a liminalkit window.
  On Windows, declared buttons, toggles, check boxes, combos, text fields
  and labels are Win32 child controls (through `winsafe`) placed each frame
  at device-pixel rectangles on top of the drawn frame, in the ordinary
  winit window; elsewhere `NativeControls::is_native()` is `false` and the
  application draws them. `liminalkit` feature `native` adds
  `liminalkit::native::{button, toggle, label}`, which place the native
  control or draw liminalkit's. The workspace's `unsafe_code = "forbid"`
  now has one exception: this crate denies unsafe and allows it in its
  Windows-only backend module.

- `CursorShape::Grab` and `CursorShape::Grabbing`: an open and a closed hand
  for something that can be picked up and dragged. Windows has no hand cursor
  (winit maps `Grab` to the four-way move arrows), so the host draws the hands
  itself (`cursors::hand_cursor`), sized to the display scale and redrawn
  when it changes; macOS and Linux use the cursor theme's hands.
- A table cell can show a colour rather than name it:
  `Cell::swatch(text, fill)` (`CellAppearance::Swatch`) draws a small square
  of `fill` at the cell's leading edge and the text after it, and
  `Ui::swatch` draws the same outside a table. A `match` over
  `CellAppearance` needs the new arm.
- The system clipboard works on Windows: `Clipboard` text read, write and
  clear go through the Win32 clipboard (CF_UNICODETEXT) via arboard's
  clipboard-win backend, so Ctrl+V in a `TextFieldState` with an attached
  `ClipboardWorker` pastes there as it does on Linux and macOS. Image copy
  on Windows returns an error that says it is not implemented.
- Text rendering options, `liminalkit_text::TextRendering`, held by the
  `TextCache` (`with_rendering`, `set_rendering`, `rendering`): coverage
  corrected for the sRGB blend against the background each string is drawn
  on (`gamma`, `contrast`); opt-in vertical grid fitting that puts the
  baseline, x-height, cap height and ascender of Latin faces on whole pixels
  and folds small overshoots onto them (`hinting: Hinting::Vertical`); and
  opt-in horizontal stem darkening (`stem_darkening`). `text::render_with`
  and `text::render_shaped_with` rasterize with them. Underneath,
  `liminalkit_raster::gamma` builds the coverage curves, and
  `Painter::blend_coverage_row_mapped`, `Painter::backdrop` and
  `blend::blend_coverage_span_mapped` (with its scalar specification) apply
  them.
- `liminalkit`, the facade crate: re-exports the component crates as
  `liminalkit::{raster, text, shell, ui, windowing, kms}`, a prelude, and
  `App`/`Screen`/`Context`, the short road from a type that draws a screen to
  a running window or a bare display.
- Stateless layout solvers in `liminalkit-ui`: `Length`, `vstack`, `hstack`,
  `Flow`, `Insets`, `Anchor`, `place` and `centre`, beside the existing grids.
- Typed keyboard focus: `Focus<K>` accepts a caller-supplied key so
  conditional and reordered controls keep their focus.
- Modal dialogs and popovers (`Overlay`), context menus (`Menu`), drag and
  drop (`Drag<T>`), and a multi-line text area (`TextAreaState`).
- Unicode bidirectional reordering and bracket mirroring for right-to-left
  text, behind the `bidi` feature of `liminalkit-text`.
- A tutorial (`docs/TUTORIAL.md`, `examples/tutorial.rs`), an architecture
  guide, a contributing guide with the versioning method, a README per
  crate, crates.io metadata on every crate, a headless frame benchmark
  (`examples/frame_bench.rs`), the gallery screenshot in the README, and a
  CI workflow.

## 0.4.0 — 2026-10-28

Renamed from pixelkit to liminalkit (`cccde64`): every crate, path and
import changed. Added `liminalkit-kms`, the Linux DRM/KMS and evdev host, so
the same `PixelApp` runs on a bare display. The README gained the account of
how a screen is composed.

## 0.3.0 — 2026-09-29

The window host left the shell for its own crate, `pixelkit-windowing`
(`593e10d`): a dependant adds the crate and moves its imports. With it came
several OS windows on one event loop, draggable panes and floating in-app
windows. The 2026-10-08 additions (`614c9c6`: more windowing, the clipboard
on every platform, and the controls an operational screen needs) were
additive and belong to this generation.

## 0.2.0 — 2026-09-24

The shell was reworked for a real application (the `legeos-toolkit-additions`
branch, merged in `7492c49`): full text input, IME and scroll phases, and
presenter injection changed `Input` and `PixelApp`; the clipboard and
platform modules arrived; AccessKit accessibility behind a feature;
`Modifiers::logo`/`accel`, `OperationalPalette` and `Theme`; full-screen and
pointer-shape requests; selection on `TextFieldState`; and measured SIMD
coverage merging in the rasterizer.

## 0.1.0 — 2026-08-17

Extracted from `restaurant-pos`/`thai-text` as pixelkit (`d02493c`): raster,
text, shell and ui. The same week brought single-pass cached wrapping, span
blends and shadows; on 2026-08-21 raster images, keyboard focus, form
widgets and grid layout for the LegeOS shell, all additive.
