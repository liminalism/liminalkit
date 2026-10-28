# Embedded fonts

Both faces are Noto, © 2010, 2012–2020 Google Inc. and 2015–2020 Google LLC,
licensed under the **SIL Open Font License, Version 1.1**. They are embedded in
the binary (`include_bytes!`) rather than loaded from the system, because a
receipt is a tax document and its glyph shapes must not depend on what happens
to be installed on the store's machine.

| File | Used for |
|---|---|
| `NotoSansThai-Regular.ttf` | Thai. 37 KB, `glyf` outlines, 140 glyphs. Carries GSUB `ccmp` and GPOS `kern`/`mark`/`mkmk` — the lookups the shaper implements. |
| `NotoSans-Regular.ttf` | Arabic numerals, Latin, currency. |

Two faces are necessary, not a stylistic choice: **Noto Sans Thai contains no
Latin letters and no Arabic digits.** Its 101 mapped codepoints are 87 Thai
characters plus a handful of marks. Prices, PLU codes and quantities would
render as blanks from it alone.

Bold is synthesized by outline emboldening rather than by embedding a second
weight, which keeps the binary small and the two faces' weights consistent.

Both are unmodified upstream releases from `fonts-noto-core`. Replacing them
means re-running the golden tests: `UPDATE_GOLDEN=1 cargo test -p thai-text`.
