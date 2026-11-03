# The test fonts

Two faces, used only by liminalkit's tests and examples through the
unpublished `liminalkit-test-fonts` crate. They are not part of any published
crate and no application build carries them.

| File | Family and version | Glyphs | Size | Copyright |
|---|---|---|---|---|
| `NotoSans-Regular.ttf` | Noto Sans Regular, Version 2.004 | 3317 | 512,672 bytes | Copyright 2015 Google LLC |
| `NotoSansThai-Regular.ttf` | Noto Sans Thai Regular, Version 2.000 | 140 | 37,744 bytes | Copyright 2016 Google Inc. |

Both are unmodified upstream builds from the `fonts-noto-core` distribution,
hinted with ttfautohint 1.8.3 as their version strings record, and licensed
under the **SIL Open Font License, Version 1.1** (<https://openfontlicense.org>).
The reserved font name is "Noto".

Why these two: Noto Sans Thai maps 101 codepoints, the 87 Thai characters
plus marks, and no Latin letters or digits; Noto Sans has no Thai. Together
they exercise per-character fallback, mixed-face runs, and the OpenType
lookups the shaper implements (`ccmp` substitution, `kern`, `mark` and
`mkmk` positioning), which Noto Sans Thai carries in 37 KB.

Replacing either file means re-running `cargo test --workspace`: several
tests in `liminalkit-text` and `liminalkit-ui` assert exact glyph ids,
advances and ink columns of these builds.
