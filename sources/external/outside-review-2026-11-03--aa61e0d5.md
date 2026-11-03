# Outside review of liminalkit, received 2026-11-03

Pasted by the project owner from an outside agent, with the instruction to improve the toolkit according to it, except for the recommendation to publish to crates.io, which the owner does not consider timely.

## Positioning

LiminalKit — Explicit-state, software-rendered GUI for Rust. A lightweight Rust GUI toolkit for applications that need direct control over drawing, interaction, and resource use. Caller-owned state, immediate-mode controls, and software rendering give predictable behavior without a browser, retained widget tree, or mandatory GPU rendering stack. Designed for desktop utilities, operational interfaces, kiosks, and embedded Linux displays; favours straightforward code, low overhead, and a small number of understandable abstractions over comprehensive framework automation.

Development rule proposed: every major new subsystem should justify its ownership, allocations, and implicit state. A more convenient API is welcome; a hidden mechanism that undermines predictability should require strong justification. Abstractions should remain understandable and optional.

## Comparison

- egui is also immediate-mode but keeps interaction state and widget ids inside the framework; liminalkit pushes that into application structs. Matching egui's widget library feature for feature is not a good investment.
- Freya (own reactive core since 0.4, Skia rendering) and Dioxus (virtual DOM, usually a WebView on desktop) suit complex dynamic layouts; liminalkit suits operational applications with fixed, predictable interfaces (cashier terminal, system panel, monitoring dashboard, document viewer).

## Prioritised work

| Priority | Area | What needs to happen |
|---|---|---|
| Critical | External adoption | Simple application bootstrap, clear dependencies, consistent APIs |
| Critical | Layout | More reusable responsive layout mechanisms without a retained widget tree |
| High | Keyboard and focus | Stable focus ownership across conditional and reordered controls |
| High | Text | Font fallback, broader Unicode shaping, bidirectional text |
| High | Validation | Real hardware tests, cross-platform testing, objective performance benchmarks |
| Medium | UI capabilities | Dialogs, menus, overlays, drag-and-drop, richer text input |
| Medium | Documentation | Published examples, screenshots, tutorials, crate-level API docs |

## Specific findings

- Layout: the interactive window example manually computes rectangles such as `Rect::new(panel.x + s.px(380), panel.y + s.px(11), s.px(220), s.px(26))`. Add stateless layout solvers (vertical and horizontal stacks, flex allocation, alignment, margins, content measurement) as plain functions over rectangles. Explicitly forbid a new retained layout hierarchy unless there is an overwhelming reason. Make manual layout easy rather than eliminating it.
- Focus: `Focus` identifies controls by registration order, so reordering or conditionally omitting a control changes which control owns focus, the category of problem the state-ownership philosophy was meant to avoid. Introduce optional caller-supplied typed focus identifiers (an application enum) without a framework-wide widget map.
- Application API: `examples/window.rs` manages a painter, text cache, input, raster kernel, scale, theme, and per-event forwarding, and calls `Input::end_frame()` by hand. Add an optional high-level application adapter that initialises the usual resources, forwards input, and guarantees frame cleanup, keeping the low-level interfaces intact.
- Coordinate system: the README says coordinates given to `Ui` are logical pixels, but the example converts with `Scale::px` and the painter works on device-pixel rectangles. Make the contract explicit and consistent before external users depend on it.
- Text: keep the existing renderer as the lightweight path and consider an optional integration with a mature shaper such as rustybuzz for font fallback across scripts, bidirectional text, and complex shaping.
- Touch and KMS: gesture and scroll physics are labelled not hardware-tested; KMS needs validation on real Raspberry Pi displays, rotations, refresh rates, and touch controllers.
- Accessibility: AccessKit integration exists as an optional feature; practical accessibility needs testing across focus transitions, modal dialogs, text fields, and assistive technologies.
- Packaging (deferred by the owner): crates lack descriptions; workspace path dependencies lack version requirements; `liminalkit-ui` and `liminalkit-windowing` dev-depend on each other for examples, which should move to a separate examples package; a top-level `liminalkit` facade crate should be the entry point; publish order would be raster, text, shell, ui, windowing, kms, after `cargo publish --dry-run`.
- Performance evidence: the reported 30% memory reduction over Freya in Lege should become a reproducible comparison. Build the same screen with liminalkit, egui, and Freya and measure memory, startup latency, idle CPU, p95 render time, binary size, and responsiveness under heavy interaction. egui can also sleep while idle, so idle cost must be measured rather than claimed. Software rendering cost grows with resolution and redraw area; find where the memory savings stop paying for the CPU work (likely wins at 1080p dashboards and Pi displays, likely loses on animated 4K).
- Discovery: the README explains internals and how to run examples; it should answer "why would I use this instead of egui?" with a screenshot of a real Lege screen, a kiosk screenshot, and a short self-contained example. `AGENTS.md` is oriented around the AKR workflow; outside contributors need a path that does not depend on it: a public architecture guide, a contributing guide, and a tested quick-start example.

## Final assessment

Do not turn liminalkit into a universal framework. Make the existing architecture easy to adopt, improve layout and focus handling, complete publication metadata, and build convincing performance demonstrations before substantially expanding the widget collection.
