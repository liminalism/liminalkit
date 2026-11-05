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

### Added

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
