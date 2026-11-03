# Changelog

All notable changes to the liminalkit workspace. The crates share one version.

## Unreleased

The toolkit is not yet published to crates.io. This section collects what
0.1.0 will carry.

### Added

- `liminalkit`, a facade crate: re-exports the component crates as
  `liminalkit::{raster, text, shell, ui, windowing, kms}`, a prelude, and
  `App`/`Screen`/`Context`, the short road from a type that draws a screen to
  a running window or a bare display.
- Stateless layout solvers in `liminalkit-ui`: `Length`, `vstack`, `hstack`,
  `Flow`, `Insets`, `Anchor`, `place` and `centre`, plus the existing grids.
- Typed keyboard focus: `Focus<K>` accepts a caller-supplied key so
  conditional and reordered controls keep their focus.
- Modal dialogs and popovers (`Overlay`), context menus (`Menu`), drag and
  drop (`Drag<T>`), and a multi-line text area (`TextAreaState`).
- Unicode bidirectional reordering and bracket mirroring for right-to-left
  text, behind the `bidi` feature of `liminalkit-text`.
- A tutorial (`docs/TUTORIAL.md`, `examples/tutorial.rs`), an architecture
  guide, a contributing guide, a README per crate, a headless frame
  benchmark (`examples/frame_bench.rs`) and a CI workflow.

### Changed

- Every rectangle a control takes is in device pixels; `Theme` sizes are
  logical and scaled by the controls. `Ui::rect` and `Scale::rect` convert a
  logical layout once.
- Popups claim input in a pre-frame step (`Screen::before_frame` under
  `App`) and draw last.
- Direction marks and embeddings (U+200E, U+200F, U+202A..U+202E,
  U+2066..U+2069, U+061C) are never drawn; they used to render as `.notdef`
  boxes.
