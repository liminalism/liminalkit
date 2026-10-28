# Provenance

liminalkit is extracted from GUI code written for other in-house projects, all
by the same author, and generalised. Where a module is a near-verbatim copy,
its history lives in the source repository:

| liminalkit module | Source |
|---|---|
| `liminalkit-raster/src/painter.rs` | `Kitchen-Concept/restaurant-pos/crates/pos-client-ui/src/paint.rs` |
| `liminalkit-raster/src/blend.rs` | `Kitchen-Concept/restaurant-pos/crates/pos-simd/src/scalar.rs` (the SIMD levels were left behind: liminalkit forbids `unsafe`) |
| `liminalkit-raster/src/kernel.rs` | `Kitchen-Concept/restaurant-pos/crates/thai-text/src/raster.rs`, itself a trim of `Lege-ecosystem/lege-pdf` `pdf-render-cpu::raster` |
| `liminalkit-raster/src/{path,shapes,color,png}.rs` | new; `hline/vline/blend_rect` follow `Kitchen-Simulation/crates/kitchen-ui/src/paint.rs` |
| `liminalkit-text/src/{sfnt,layout}.rs` | `thai-text` verbatim |
| `liminalkit-text/src/{font,text}.rs` | `thai-text`, generalised from two embedded faces to `Face::from_bytes` + `FontSet`, with letter-spacing added |
| `liminalkit-text/src/cache.rs` | `pos-client-ui/src/text.rs`, keyed by `TextStyle` |
| `liminalkit-text/test-fonts/` | Noto Sans / Noto Sans Thai (OFL 1.1), test fixtures and examples only |
| `liminalkit-shell/src/input.rs` | `pos-client-ui/src/input.rs` |
| `liminalkit-windowing/src/host.rs` | `pos-client-ui/src/shell.rs` via `liminalkit-shell/src/shell.rs`, with HiDPI scale tracking and the `Presenter` seam added; the seam follows `Lege-ecosystem/lege-viewer/src/present/mod.rs` |
| `liminalkit-windowing/src/{panes,windows}.rs` | new |
| `liminalkit-ui/src/widget.rs` | `pos-client-ui/src/widget.rs` minus the recipe card |
| `liminalkit-ui/src/{chrome,list,tooltip}.rs` | new |
