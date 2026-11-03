# liminalkit-ui

The controls of [liminalkit](../../README.md), in immediate mode over `liminalkit-raster`, `-text` and `-shell`.

**What it is for.** A `Ui` over a painter, a text cache, this frame's input, a theme and a scale. Controls are methods on it: labels, buttons, tabs, checkboxes, toggles, radios, text fields (single- and multi-line), dropdowns, context menus, modal dialogs and popovers, tables and lists that draw only the visible rows, toasts, tooltips, and drag and drop. Each takes a device-pixel rectangle and `&mut` to state the screen owns, draws, and returns what happened. `Focus<K>` gives Tab traversal, positional by default or by a caller-supplied key. `layout` has stateless stacks, a flow cursor, insets, alignment and grids.

**What it is not.** No widget tree, no identity map, no per-widget storage, no retained layout hierarchy, no constraint solver. Theme sizes are logical and the controls scale them; rectangles are device pixels.

**Features.** `accessibility` records every drawn control into a `Semantics` collector for AccessKit. `test-fonts` forwards to `liminalkit-text`.

**Who uses it.** Applications, through the `liminalkit` facade or directly; `liminalkit-windowing` does not depend on it.
