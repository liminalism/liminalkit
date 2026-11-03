# liminalkit-text

The text layer of [liminalkit](../../README.md): TrueType parsing, the OpenType substitution and positioning lookups the bundled Latin and Thai faces use, exact-area glyph rasterisation through `liminalkit-raster`'s kernel, wrapping, and a cache keyed by `(text, style)`.

**What it is for.** An application embeds its faces, registers them in a `FontSet` with fallback chains, and draws strings with a `TextCache`: measure, wrap, draw, draw fitted with an ellipsis, with ligatures, kerning and mark attachment where the face provides them. Per-character fallback walks the chain so a Thai string in a Latin style finds the Thai face.

**What it is not.** Not a full Unicode shaper: lookups the bundled faces do not use are skipped, and bidirectional text is not reordered. Not a font discovery service: faces come from bytes the application supplies. The `test-fonts` feature embeds two Noto faces for examples and tests only.

**Who uses it.** `liminalkit-ui` for every label, and anything that needs shaped text in a pixel buffer without a GUI.

No external dependencies. No `unsafe`.
