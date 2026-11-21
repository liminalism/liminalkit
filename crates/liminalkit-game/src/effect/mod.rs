//! Effects on a layer: shimmer, flash, glow, scanlines, pop.
//! **EXPERIMENTAL.**
//!
//! An effect is a few numbers on a layer, not pixels. Each frame
//! [`Effects::resolve`] turns those numbers and the host's time into a
//! [`LayerUniforms`] block of plain floats — where the shimmer's band is right
//! now, how bright the flash is — and that block is all either renderer sees.
//! The GPU reads it as a uniform buffer; the CPU hands it to [`shade`]. Time
//! is settled once, here, in `Duration`: a shader never sees a clock, so it
//! cannot drift from the CPU and cannot lose precision in a long session.
//!
//! Nothing here needs a second blend mode. Shimmer and flash *brighten* what
//! is already in the layer, by an amount scaled by the layer's own alpha and
//! capped at it, so the result is still a valid premultiplied pixel and is
//! composited source-over like everything else. That is also what masks them
//! to the layer's shape: where the layer is transparent there is nothing to
//! brighten.
//!
//! With no clock an effect that plays out over time is simply over: a flash
//! is dark, a shimmer is absent, a pop is at rest.

mod shade;

use std::time::Duration;

use crate::anim::{Ease, Envelope};
use crate::geom::{Argb, RectF};

pub use shade::shade;
#[cfg(feature = "surfaces")]
pub(crate) use shade::{scan_keep, shade_row};

/// A band of light sweeping across the layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shimmer {
    pub color: Argb,
    /// Brightness at the band's centre, `0..=1`.
    pub intensity: f32,
    /// Direction the band travels, radians clockwise from rightwards.
    pub angle: f32,
    /// Width of the band, in texels.
    pub width: f32,
    /// How long one crossing takes.
    pub sweep: Duration,
    /// Rest between crossings.
    pub pause: Duration,
    /// Cross again and again, or once.
    pub repeat: bool,
    /// Host time the first crossing starts.
    pub started: Duration,
}

impl Shimmer {
    /// A soft white band crossing left to right every few seconds.
    pub fn sheen(started: Duration) -> Shimmer {
        Shimmer {
            color: Argb::WHITE,
            intensity: 0.35,
            angle: 0.35,
            width: 90.0,
            sweep: Duration::from_millis(900),
            pause: Duration::from_millis(2_400),
            repeat: true,
            started,
        }
    }
}

/// The whole layer brightening toward a colour and back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flash {
    pub color: Argb,
    /// Brightness at the envelope's peak, `0..=1`.
    pub peak: f32,
    pub envelope: Envelope,
    /// Host time it was set off.
    pub fired: Duration,
}

/// A halo outside the layer's content rectangle, and optionally a line on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glow {
    pub color: Argb,
    pub intensity: f32,
    /// How far the halo reaches, in texels.
    pub radius: f32,
    /// Width of a line drawn on the content's edge, in texels; 0 for none.
    pub outline: f32,
    /// Breathe: a period and how deep the dip is, `0..=1`.
    pub pulse: Option<(Duration, f32)>,
}

/// Horizontal lines drifting over the layer: a hologram.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scanlines {
    /// Texels from one line to the next.
    pub period: f32,
    /// How dark a line is, `0..=1`.
    pub depth: f32,
    /// Texels per second the lines drift down.
    pub speed: f32,
    /// How much the whole layer's brightness jitters, `0..=1`.
    pub flicker: f32,
    pub started: Duration,
}

/// The layer springing to size from a little smaller or larger.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pop {
    pub fired: Duration,
    pub duration: Duration,
    /// The scale it starts from; it ends at 1.
    pub from: f32,
    pub ease: Ease,
}

/// A layer's effects. `Default` is none.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Effects {
    pub shimmer: Option<Shimmer>,
    pub flash: Option<Flash>,
    pub glow: Option<Glow>,
    pub scanlines: Option<Scanlines>,
    pub pop: Option<Pop>,
}

/// What effects need to know about the layer they are on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    /// The layer's texture, in texels.
    pub size: [f32; 2],
    /// The part of it the content fills; a glow is drawn around this.
    pub content: RectF,
    /// The content's corner radius, in texels.
    pub corner: f32,
}

/// One frame of a layer's effects, settled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved {
    pub uniforms: LayerUniforms,
    /// Scale about the layer's centre, from a pop.
    pub scale: f32,
    /// Brightness jitter from scanline flicker, to multiply into opacity.
    pub flicker: f32,
    /// No effect changes any pixel this frame: [`shade`] would return its
    /// input.
    pub identity: bool,
    /// Something will be different next frame.
    pub animating: bool,
}

impl Effects {
    /// Whether any effect is set at all.
    pub fn is_empty(&self) -> bool {
        *self == Effects::default()
    }

    /// Texels of transparent border a layer needs round its content for
    /// these effects to have somewhere to draw.
    pub fn margin(&self) -> u32 {
        self.glow.map_or(0, |glow| {
            let reach = glow.radius.max(glow.outline / 2.0).max(0.0);
            if reach > 0.0 {
                reach.ceil() as u32 + 1
            } else {
                0
            }
        })
    }

    /// Settle every effect at host time `now` (`None`: no clock).
    pub fn resolve(&self, now: Option<Duration>, shape: Shape) -> Resolved {
        let mut uniforms = LayerUniforms {
            size: [0.0, 0.0, shape.size[0], shape.size[1]],
            ..LayerUniforms::IDENTITY
        };
        let since = |start: Duration| now.and_then(|now| now.checked_sub(start));
        let mut animating = false;

        if let (Some(shimmer), Some(elapsed)) =
            (self.shimmer, self.shimmer.and_then(|s| since(s.started)))
        {
            let sweep = shimmer.sweep.as_secs_f64();
            let cycle = sweep + shimmer.pause.as_secs_f64();
            let elapsed = elapsed.as_secs_f64();
            let into = if shimmer.repeat && cycle > 0.0 {
                elapsed % cycle
            } else {
                elapsed
            };
            animating |= shimmer.repeat || elapsed < sweep;
            if sweep > 0.0 && into < sweep && shimmer.intensity > 0.0 && shimmer.width > 0.0 {
                let (sin, cos) = shimmer.angle.sin_cos();
                let half = shimmer.width / 2.0;
                // The band starts wholly before the layer and ends wholly
                // after it, measured along its direction of travel.
                let corners = [
                    0.0,
                    cos * shape.size[0],
                    sin * shape.size[1],
                    cos * shape.size[0] + sin * shape.size[1],
                ];
                let first = corners.iter().copied().fold(f32::INFINITY, f32::min) - half;
                let last = corners.iter().copied().fold(f32::NEG_INFINITY, f32::max) + half;
                let offset = first + (last - first) * (into / sweep) as f32;
                let [r, g, b, _] = shimmer.color.to_f32();
                uniforms.shimmer_line = [cos, sin, offset, half];
                uniforms.shimmer_color = [r, g, b, shimmer.intensity.clamp(0.0, 1.0)];
            }
        } else if self.shimmer.is_some() && now.is_some() {
            // Not started yet.
            animating = true;
        }

        if let Some(flash) = self.flash {
            match since(flash.fired) {
                Some(elapsed) if !flash.envelope.finished(elapsed) => {
                    animating = true;
                    let [r, g, b, _] = flash.color.to_f32();
                    let level = flash.peak.clamp(0.0, 1.0) * flash.envelope.at(elapsed);
                    uniforms.flash = [r, g, b, level.clamp(0.0, 1.0)];
                }
                Some(_) => {}
                None => animating |= now.is_some(),
            }
        }

        if let Some(glow) = self.glow {
            let mut intensity = glow.intensity.clamp(0.0, 1.0);
            if let (Some((period, depth)), Some(now)) = (glow.pulse, now) {
                if !period.is_zero() {
                    animating = true;
                    let turn = (now.as_secs_f64() / period.as_secs_f64()).fract() as f32;
                    let dip = 0.5 - 0.5 * (std::f32::consts::TAU * turn).cos();
                    intensity *= 1.0 - depth.clamp(0.0, 1.0) * dip;
                }
            }
            if intensity > 0.0 && (glow.radius > 0.0 || glow.outline > 0.0) {
                let [r, g, b, a] = glow.color.to_f32();
                uniforms.glow_color = [r, g, b, intensity * a];
                uniforms.glow_rect = [
                    shape.content.x,
                    shape.content.y,
                    shape.content.right(),
                    shape.content.bottom(),
                ];
                uniforms.glow_shape = [
                    shape.corner.max(0.0),
                    glow.radius.max(0.0),
                    glow.outline.max(0.0),
                    0.0,
                ];
            }
        }

        let mut flicker = 1.0;
        if let Some(scan) = self.scanlines {
            if scan.period > 0.0 && scan.depth > 0.0 {
                let elapsed = since(scan.started).map_or(0.0, |d| d.as_secs_f64());
                let phase = (elapsed * f64::from(scan.speed) / f64::from(scan.period)).fract();
                uniforms.scan = [
                    scan.period,
                    scan.depth.clamp(0.0, 1.0),
                    -(phase as f32) * std::f32::consts::TAU,
                    0.0,
                ];
                animating |= now.is_some() && scan.speed != 0.0;
            }
            if let (true, Some(now)) = (scan.flicker > 0.0, now) {
                animating = true;
                // One value per thirtieth of a second, the same on any
                // machine: a hash of the tick, not a random number.
                let tick = (now.as_secs_f64() * 30.0) as u64;
                let hash = tick
                    .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                    .rotate_left(23)
                    .wrapping_mul(0x2545_f491_4f6c_dd1d);
                let noise = (hash >> 40) as f32 / (1u64 << 24) as f32;
                flicker = 1.0 - scan.flicker.clamp(0.0, 1.0) * noise;
            }
        }

        let mut scale = 1.0;
        if let Some(pop) = self.pop {
            match since(pop.fired) {
                Some(elapsed) if elapsed < pop.duration => {
                    animating = true;
                    let t = elapsed.as_secs_f32() / pop.duration.as_secs_f32();
                    scale = pop.from + (1.0 - pop.from) * pop.ease.apply(t);
                }
                Some(_) => {}
                None => animating |= now.is_some(),
            }
        }

        Resolved {
            uniforms,
            scale,
            flicker,
            identity: uniforms.is_identity(),
            animating,
        }
    }
}

/// Everything a layer's draw needs besides its texture and mesh: nine
/// four-float vectors, 144 bytes, laid out for a uniform buffer as they are.
///
/// The first two vectors belong to the draw (target size, opacity); the rest
/// are the effects, already settled for this frame. A host fills `size[0..2]`
/// and `params` through [`LayerUniforms::for_draw`]; everything else comes
/// from [`Effects::resolve`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerUniforms {
    /// Target width and height in pixels, texture width and height in texels.
    pub size: [f32; 4],
    /// Opacity, rim-glint width in pixels, last mip level, unused.
    pub params: [f32; 4],
    /// The shimmer band: unit direction, offset along it, half-width.
    pub shimmer_line: [f32; 4],
    /// Shimmer colour (straight) and intensity.
    pub shimmer_color: [f32; 4],
    /// Flash colour (straight) and level.
    pub flash: [f32; 4],
    /// Glow colour (straight) and intensity.
    pub glow_color: [f32; 4],
    /// The rectangle the glow surrounds: left, top, right, bottom, texels.
    pub glow_rect: [f32; 4],
    /// Corner radius, halo radius, outline width, unused.
    pub glow_shape: [f32; 4],
    /// Scanline period, depth, phase in radians, unused.
    pub scan: [f32; 4],
}

impl LayerUniforms {
    /// No effect, full opacity.
    pub const IDENTITY: LayerUniforms = LayerUniforms {
        size: [0.0; 4],
        params: [1.0, 0.0, 0.0, 0.0],
        shimmer_line: [1.0, 0.0, 0.0, 1.0],
        shimmer_color: [0.0; 4],
        flash: [0.0; 4],
        glow_color: [0.0; 4],
        glow_rect: [0.0; 4],
        glow_shape: [0.0; 4],
        scan: [0.0; 4],
    };

    /// Bytes in the block.
    pub const SIZE: usize = 144;

    /// The block's fields in order, with their shader types: the one list the
    /// byte packing and the shader's struct are both written from.
    pub const LAYOUT: [(&'static str, &'static str); 9] = [
        ("size", "vec4f"),
        ("params", "vec4f"),
        ("shimmer_line", "vec4f"),
        ("shimmer_color", "vec4f"),
        ("flash", "vec4f"),
        ("glow_color", "vec4f"),
        ("glow_rect", "vec4f"),
        ("glow_shape", "vec4f"),
        ("scan", "vec4f"),
    ];

    fn fields(&self) -> [[f32; 4]; 9] {
        [
            self.size,
            self.params,
            self.shimmer_line,
            self.shimmer_color,
            self.flash,
            self.glow_color,
            self.glow_rect,
            self.glow_shape,
            self.scan,
        ]
    }

    /// These effects, for one draw into a `viewport`-sized target.
    pub fn for_draw(mut self, viewport: [u32; 2], opacity: f32, glint_px: f32, mips: u8) -> Self {
        self.size[0] = viewport[0] as f32;
        self.size[1] = viewport[1] as f32;
        self.params = [
            opacity.clamp(0.0, 1.0),
            glint_px.max(0.0),
            f32::from(mips),
            0.0,
        ];
        self
    }

    /// Whether [`shade`] with these uniforms returns its input unchanged.
    pub fn is_identity(&self) -> bool {
        self.shimmer_color[3] <= 0.0
            && self.flash[3] <= 0.0
            && self.glow_color[3] <= 0.0
            && self.scan[1] <= 0.0
    }

    /// The block as little-endian bytes, in [`LayerUniforms::LAYOUT`] order.
    pub fn to_bytes(&self) -> [u8; LayerUniforms::SIZE] {
        let mut bytes = [0u8; LayerUniforms::SIZE];
        for (chunk, value) in bytes
            .chunks_exact_mut(4)
            .zip(self.fields().iter().flatten())
        {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> Shape {
        Shape {
            size: [200.0, 100.0],
            content: RectF::new(20.0, 20.0, 160.0, 60.0),
            corner: 8.0,
        }
    }

    fn at(ms: u64) -> Option<Duration> {
        Some(Duration::from_millis(ms))
    }

    #[test]
    fn no_effects_and_no_clock_are_both_the_identity() {
        let none = Effects::default().resolve(at(500), shape());
        assert!(none.identity && !none.animating);
        assert_eq!((none.scale, none.flicker), (1.0, 1.0));
        assert_eq!(none.uniforms.size, [0.0, 0.0, 200.0, 100.0]);
        // Time-driven effects with no clock: over, and nothing asks for frames.
        let timed = Effects {
            shimmer: Some(Shimmer::sheen(Duration::ZERO)),
            flash: Some(Flash {
                color: Argb::WHITE,
                peak: 1.0,
                envelope: Envelope::flash(
                    Duration::from_millis(40),
                    Duration::from_millis(40),
                    Duration::from_millis(200),
                ),
                fired: Duration::ZERO,
            }),
            pop: Some(Pop {
                fired: Duration::ZERO,
                duration: Duration::from_millis(200),
                from: 0.8,
                ease: Ease::CubicOut,
            }),
            ..Effects::default()
        };
        let still = timed.resolve(None, shape());
        assert!(still.identity && !still.animating);
        assert_eq!(still.scale, 1.0);
    }

    #[test]
    fn a_shimmer_crosses_the_whole_layer_and_rests() {
        let effects = Effects {
            shimmer: Some(Shimmer {
                angle: 0.0,
                width: 40.0,
                sweep: Duration::from_millis(1_000),
                pause: Duration::from_millis(500),
                ..Shimmer::sheen(Duration::from_millis(100))
            }),
            ..Effects::default()
        };
        let before = effects.resolve(at(50), shape());
        assert!(before.identity && before.animating, "not started, but will");
        let start = effects.resolve(at(100), shape());
        assert_eq!(start.uniforms.shimmer_line, [1.0, 0.0, -20.0, 20.0]);
        assert!(!start.identity);
        let middle = effects.resolve(at(600), shape());
        assert!(
            (middle.uniforms.shimmer_line[2] - 100.0).abs() < 1e-3,
            "half way across"
        );
        let resting = effects.resolve(at(1_300), shape());
        assert!(resting.identity && resting.animating, "between crossings");
        let again = effects.resolve(at(1_600), shape());
        assert_eq!(again.uniforms.shimmer_line[2], -20.0, "the next crossing");
        let once = Effects {
            shimmer: effects.shimmer.map(|s| Shimmer { repeat: false, ..s }),
            ..Effects::default()
        };
        let done = once.resolve(at(1_700), shape());
        assert!(done.identity && !done.animating);
    }

    #[test]
    fn a_flash_follows_its_envelope_and_ends() {
        let effects = Effects {
            flash: Some(Flash {
                color: Argb::rgb(0xff8000),
                peak: 0.8,
                envelope: Envelope::flash(
                    Duration::from_millis(100),
                    Duration::from_millis(100),
                    Duration::from_millis(100),
                ),
                fired: Duration::from_millis(1_000),
            }),
            ..Effects::default()
        };
        assert!(effects.resolve(at(900), shape()).identity);
        let held = effects.resolve(at(1_150), shape());
        assert!((held.uniforms.flash[3] - 0.8).abs() < 1e-6 && held.animating);
        assert!(
            (held.uniforms.flash[1] - 128.0 / 255.0).abs() < 1e-6,
            "its colour"
        );
        let over = effects.resolve(at(1_400), shape());
        assert!(over.identity && !over.animating);
    }

    #[test]
    fn a_glow_surrounds_the_content_and_asks_for_room() {
        let glow = Glow {
            color: Argb::rgb(0x00ffff),
            intensity: 0.5,
            radius: 12.0,
            outline: 2.0,
            pulse: None,
        };
        let effects = Effects {
            glow: Some(glow),
            ..Effects::default()
        };
        assert_eq!(effects.margin(), 13);
        assert_eq!(Effects::default().margin(), 0);
        let steady = effects.resolve(None, shape());
        assert!(
            !steady.identity && !steady.animating,
            "a steady glow needs no clock"
        );
        assert_eq!(steady.uniforms.glow_rect, [20.0, 20.0, 180.0, 80.0]);
        assert_eq!(steady.uniforms.glow_shape, [8.0, 12.0, 2.0, 0.0]);
        assert_eq!(steady.uniforms.glow_color, [0.0, 1.0, 1.0, 0.5]);
        let breathing = Effects {
            glow: Some(Glow {
                pulse: Some((Duration::from_millis(1_000), 0.5)),
                ..glow
            }),
            ..Effects::default()
        };
        let trough = breathing.resolve(at(500), shape());
        assert!((trough.uniforms.glow_color[3] - 0.25).abs() < 1e-5 && trough.animating);
        let crest = breathing.resolve(at(1_000), shape());
        assert!((crest.uniforms.glow_color[3] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn scanlines_drift_and_flicker_the_same_way_every_time() {
        let effects = Effects {
            scanlines: Some(Scanlines {
                period: 4.0,
                depth: 0.3,
                speed: 8.0,
                flicker: 0.2,
                started: Duration::ZERO,
            }),
            ..Effects::default()
        };
        let a = effects.resolve(at(250), shape());
        assert!(!a.identity && a.animating);
        assert_eq!(a.uniforms.scan[..2], [4.0, 0.3]);
        // 250 ms at 8 texels/s is 2 texels: half a period.
        assert!((a.uniforms.scan[2] + std::f32::consts::PI).abs() < 1e-4);
        assert!(a.flicker <= 1.0 && a.flicker >= 0.8);
        assert_eq!(a, effects.resolve(at(250), shape()), "no randomness");
        let frozen = effects.resolve(None, shape());
        assert!(
            !frozen.identity && !frozen.animating,
            "the lines stay, still"
        );
        assert_eq!(frozen.flicker, 1.0);
    }

    #[test]
    fn a_pop_runs_from_its_scale_to_one() {
        let effects = Effects {
            pop: Some(Pop {
                fired: Duration::from_millis(100),
                duration: Duration::from_millis(200),
                from: 0.8,
                ease: Ease::Linear,
            }),
            ..Effects::default()
        };
        assert_eq!(effects.resolve(at(100), shape()).scale, 0.8);
        assert!((effects.resolve(at(200), shape()).scale - 0.9).abs() < 1e-6);
        let done = effects.resolve(at(300), shape());
        assert_eq!((done.scale, done.animating), (1.0, false));
        assert!(done.identity, "a pop moves vertices, not pixels");
    }

    #[test]
    fn the_block_packs_in_layout_order() {
        let uniforms = LayerUniforms {
            scan: [1.0, 2.0, 3.0, 4.0],
            ..LayerUniforms::IDENTITY
        }
        .for_draw([1280, 720], 0.5, 3.0, 2);
        let bytes = uniforms.to_bytes();
        assert_eq!(bytes.len(), LayerUniforms::LAYOUT.len() * 16);
        let float = |index: usize| f32::from_le_bytes(bytes[index * 4..][..4].try_into().unwrap());
        assert_eq!((float(0), float(1)), (1280.0, 720.0));
        assert_eq!((float(4), float(5), float(6)), (0.5, 3.0, 2.0));
        assert_eq!(float(35), 4.0, "the last float of the last field");
        assert_eq!(LayerUniforms::LAYOUT[8].0, "scan");
        assert!(LayerUniforms::IDENTITY.is_identity());
    }
}
