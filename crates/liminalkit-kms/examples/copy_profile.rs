//! What the copy to the glass costs per frame, for each mount.
//! `cargo run --release -p liminalkit-kms --example copy_profile`
//! (Into ordinary memory here; a real scan-out buffer is write-combined, so
//! measure on the device for absolute numbers.)
use std::time::{Duration, Instant};

use liminalkit_kms::Rotation;
use liminalkit_raster::WindowBuffer;

fn main() {
    for (w, h) in [(1280u32, 800u32), (1920, 1200)] {
        for rotation in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            let panel = (w, h);
            let (lw, lh) = rotation.logical_size(panel);
            let mut source = WindowBuffer::new(lw, lh);
            for (i, p) in source.pixels.iter_mut().enumerate() {
                *p = (i as u32).wrapping_mul(2_654_435_761);
            }
            let pitch = w as usize * 4;
            let mut dest = vec![0u32; pitch / 4 * h as usize];
            let best = (0..5)
                .map(|_| {
                    let mut samples: Vec<Duration> = (0..40)
                        .map(|_| {
                            let started = Instant::now();
                            let bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut dest[..]);
                            rotation.copy(&source, panel, bytes, pitch);
                            started.elapsed()
                        })
                        .collect();
                    samples.sort();
                    samples[20]
                })
                .min()
                .unwrap();
            std::hint::black_box(&dest);
            println!("{w}x{h} {rotation:?}: {:>7.1} µs", best.as_secs_f64() * 1e6);
        }
    }
}
