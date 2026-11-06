# liminalkit-text

The text layer of [liminalkit](../../README.md): TrueType parsing, the OpenType substitution and positioning lookups the bundled Latin and Thai faces use, exact-area glyph rasterisation through `liminalkit-raster`'s kernel, wrapping, and a cache keyed by `(text, style)` that reports its occupancy and traffic (`TextCache::stats`) so it can be sized to the screen and the machine.

**What it is for.** An application embeds its faces, registers them in a `FontSet` with fallback chains, and draws strings with a `TextCache`: measure, wrap, draw, draw fitted with an ellipsis, with ligatures, kerning and mark attachment where the face provides them. Per-character fallback walks the chain so a Thai string in a Latin style finds the Thai face.

**How it looks.** A `TextRendering` on the cache sets how glyphs become pixels. By default coverage is corrected for the sRGB blend (gamma 1.8, contrast 0.5, against the background the string is drawn on), which keeps dark text on a light panel at its weight with tighter edges and gives light text on a dark panel the weight a naive blend takes from it. Vertical grid fitting (`Hinting::Vertical`, which puts the baseline, x-height, cap height and ascender of Latin faces on whole pixels) and horizontal stem darkening are opt-in. `TextRendering::UNCORRECTED` draws exactly what earlier versions drew.

**What it is not.** Not a full Unicode shaper: lookups the bundled faces do not use are skipped. Bidirectional text is reordered only with the `bidi` feature, which resolves each line with the Unicode Bidirectional Algorithm (`unicode-bidi`), shapes right-to-left runs in reverse with mirrored brackets, and draws runs in display order; left-to-right output is identical with it off. Not a font discovery service: faces come from bytes the application supplies. The two Noto faces the tests and examples use live in the unpublished `liminalkit-test-fonts` crate, never in this one.

**Who uses it.** `liminalkit-ui` for every label, and anything that needs shaped text in a pixel buffer without a GUI.

No external dependencies unless `bidi` is enabled (`unicode-bidi` and `unicode-bidi-mirroring`, which depend on nothing). No `unsafe`.
