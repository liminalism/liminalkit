# liminalkit-raster

The drawing layer of [liminalkit](../../README.md): an XRGB `WindowBuffer`, a clipped `Painter`, and one analytic anti-aliasing kernel shared by vector paths and glyphs.

**What it is for.** Filling and stroking rectangles, rounded rectangles, circles, polylines and closed paths under the non-zero or even-odd rule; blitting bitmaps and images prepared once for the size they are drawn at; decoding and encoding PNG; rasterising SVG path icons once and tinting them. Coverage can go through a `gamma` curve on its way to the blend (`Painter::blend_coverage_row_mapped`), which is how text is corrected for blending in sRGB. Everything lands in a `Vec<u32>` the caller owns.

**What it is not.** Not a scene graph, not a GPU abstraction, not a compositor. There is no retained display list: a frame is whatever the caller drew into the buffer. Fast paths are checked byte for byte against a scalar specification and can be switched off with `LIMINALKIT_SIMD=scalar`.

**Who uses it.** Every other liminalkit crate, and anything that wants a dependable CPU rasteriser without a GUI: a thumbnailer, a report renderer, a test harness that compares pixels.

No dependencies beyond `wide`. No `unsafe`.
