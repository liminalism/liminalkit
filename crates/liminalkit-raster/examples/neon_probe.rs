//! Not run: compiled for aarch64 by `scripts/neon-report.sh`, which reads the
//! assembly of these wrappers to show whether each hot loop became NEON.
//! Each wrapper is `#[inline(never)]` so its code stands alone, and is
//! called from `main` so it is kept.

use liminalkit_raster::blend;

#[inline(never)]
pub fn probe_blend_spec(d: &mut [u32], c: u32, cov: &[u8]) {
    blend::blend_coverage_span_scalar(d, c, cov)
}

#[inline(never)]
pub fn probe_blend_span(d: &mut [u32], c: u32, cov: &[u8]) {
    blend::blend_coverage_span(d, c, cov)
}

#[inline(never)]
pub fn probe_over_span(d: &mut [u32], s: &[u32]) {
    blend::over_span(d, s, true)
}

#[inline(never)]
pub fn probe_max_span(d: &mut [u8], s: &[u8]) {
    blend::max_span(d, s)
}

#[inline(never)]
pub fn probe_fill(d: &mut [u32], c: u32) {
    d.fill(c)
}

fn main() {
    use std::hint::black_box;
    let mut pixels = black_box(vec![0u32; 64]);
    let mut bytes = black_box(vec![0u8; 64]);
    let covers = black_box(vec![0u8; 64]);
    let source = black_box(vec![0u32; 64]);
    probe_blend_spec(&mut pixels, black_box(1), &covers);
    probe_blend_span(&mut pixels, black_box(1), &covers);
    probe_over_span(&mut pixels, &source);
    probe_max_span(&mut bytes, &covers);
    probe_fill(&mut pixels, black_box(1));
    black_box((pixels, bytes));
}
