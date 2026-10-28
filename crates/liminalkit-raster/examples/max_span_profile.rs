//! Small reproducible comparison of scalar and SIMD coverage merging.
//!
//! Run with `cargo run -p liminalkit-raster --release --example max_span_profile`.

use std::hint::black_box;
use std::time::{Duration, Instant};

use liminalkit_raster::blend::max_span;

fn scalar(destination: &mut [u8], source: &[u8]) {
    for (slot, &cover) in destination.iter_mut().zip(source) {
        *slot = (*slot).max(cover);
    }
}

fn measure(length: usize, iterations: usize, merge: fn(&mut [u8], &[u8])) -> Duration {
    let mut destination = (0..length)
        .map(|index| (index * 37 + 11) as u8)
        .collect::<Vec<_>>();
    let source = (0..length)
        .map(|index| (index * 53 + 197) as u8)
        .collect::<Vec<_>>();
    let start = Instant::now();
    for _ in 0..iterations {
        merge(black_box(&mut destination), black_box(&source));
    }
    black_box(destination);
    start.elapsed()
}

fn main() {
    println!("length  scalar ns/row  SIMD ns/row  SIMD/scalar");
    for (length, iterations) in [
        (8, 30_000_000),
        (32, 20_000_000),
        (256, 10_000_000),
        (4096, 1_000_000),
    ] {
        let mut scalar_samples = Vec::with_capacity(7);
        let mut simd_samples = Vec::with_capacity(7);
        for round in 0..7 {
            if round % 2 == 0 {
                scalar_samples.push(measure(length, iterations, scalar));
                simd_samples.push(measure(length, iterations, max_span));
            } else {
                simd_samples.push(measure(length, iterations, max_span));
                scalar_samples.push(measure(length, iterations, scalar));
            }
        }
        scalar_samples.sort_unstable();
        simd_samples.sort_unstable();
        let scalar_ns = scalar_samples[3].as_nanos() as f64 / iterations as f64;
        let simd_ns = simd_samples[3].as_nanos() as f64 / iterations as f64;
        println!(
            "{length:>6}  {scalar_ns:>13.2}  {simd_ns:>11.2}  {:>11.2}",
            simd_ns / scalar_ns,
        );
    }
}
