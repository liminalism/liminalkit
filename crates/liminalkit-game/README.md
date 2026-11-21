# liminalkit-game

**EXPERIMENTAL.** An interface over a rendered scene, for games: layers of ordinary liminalkit controls that move, fade, shimmer and flash without being repainted; an animated notification stack; popovers that follow points of the world; and windows that lie on a planet and peel off flat when dragged past its edge.

One description of a frame is drawn by a CPU compositor or, from the same data, by the host's GPU. The crate has no GPU dependency.

```sh
cargo run --release -p liminalkit --example planet --features game
```

## What is in it

| Module | |
|---|---|
| `anim` | `Tween`, `Spring`, `Ease`, `Envelope`, keyframes. Time arrives as `Option<Duration>`; with no clock everything settles at once. |
| `effect` | Shimmer, flash, glow, scanlines, pop. Resolved each frame to a block of plain floats; `shade` is the scalar specification. |
| `notify` | `NoticeQueue`: priorities, lifetimes that pause on hover, coalescing, and enter / leave / make-room animation. `notify::toast` draws cards with the toolkit's toast. |
| `anchor` | Stateless placement of a popover beside a screen point: flips at edges, avoids reserved rectangles, holds its side with hysteresis. |
| `world`, `view`, `math` | A sphere, a cylinder and a plane to lay sheets on; the camera as a trait, with a ready pinhole. |
| `surface` | A layer's premultiplied buffer and its own `Input`, in its own coordinates. Revisions and damage for uploads. |
| `sheet` | Where a layer goes: flat, lying on a surface, or peeling between the two. |
| `layer`, `frame` | A layer; a frame as an ordered list of them. |
| `composite` | The CPU compositor. |
| `input` | `Router`: the pointer to the topmost layer it is really on, through the warp. |
| `gpu` | The reference WGSL, its vertex and uniform layouts, a Rust mirror of the fragment stage, and a parity table. |

With `default-features = false` the crate needs only `std` and keeps `anim`, `effect`, `notify`, `anchor`, `gpu`, `world`, `view` and `math`.

## A frame

1. As pointer events arrive, give them to `Router::pointer`. It reports whether the interface took each one; the rest belong to the world.
2. Advance what the application animates: tweens, a `NoticeQueue`, `anchor::place`.
3. Paint each layer whose `Surface::needs_paint`, with ordinary `Ui` code.
4. `Layer::prepare` each layer with the host's time and, for layers lying on something, a `Scene`.
5. Push the layers into a `GameFrame`, `finish` it, and draw it: `composite` on the CPU, or `GameFrame::gpu` on a GPU.

Moving, fading, popping, shimmering and peeling repaint nothing. They change a layer's mesh and a block of uniforms.

## Windows on a planet

A `Sheet` on a `Sphere` keeps a frame on the surface, in the planet's own coordinates, so it turns with the planet. It is carried to the screen through the host's camera, as a mesh fine enough that straight edges stay within a fifth of a pixel of the true curve. Dragging its handle slides it over the surface with the held point under the cursor. Past the horizon it peels: the part in the hand flattens first and the rest follows. Dropped on the planet, a flat sheet lays back down.

At the scale of a planet the bend across a window is a pixel or three. What reads as lying on the surface is the foreshortening changing across the window, and the fade and wrap at the horizon; this crate draws those from the real geometry and does not exaggerate the curve.

The host describes its camera and its planet each frame. For a camera with an eye, an orientation quaternion `q` and a vertical field of view:

```rust
let view = Pinhole {
    eye: V3::from_array(eye),
    right: V3::from_array(q * X),
    up: V3::from_array(q * Y),
    back: V3::from_array(q * Z),
    tan_half_fov_y: (0.5 * fov_y).tan(),
    viewport,
};
let planet = Sphere {
    radius_m,
    pose: Pose { origin: V3::ZERO, rotation: M3::from_quat_xyzw(spin.to_array()) },
};
let scene = Scene { surface: &planet, view: &view, light: Some(toward_the_sun) };
```

The same machinery lays a sheet on a `Cylinder` (a curved wall of instruments, with no planet) or a posed `Plane` (a panel tilted in perspective); `Pinhole::css` is a camera for hosts with no 3D scene.

## Drawing with a GPU

Walk `GameFrame::gpu()`. For each `GpuDraw::Textured`:

1. Find or create the texture for `texture.key` (`Bgra8Unorm`; the buffer is premultiplied, sRGB-encoded).
2. Upload what `texture.damage_since(last uploaded revision)` says, if anything.
3. Write `mesh.vertex_floats()` (32 bytes a vertex), `mesh.indices()` (`u32`) and `uniforms.to_bytes()` (144 bytes).
4. Draw with `gpu::layer_wgsl()`: `vs_layer`, and `fs_layer_encoded` for a UNORM target or `fs_layer_linear` for an sRGB one; premultiplied blending; a linear clamp-to-edge sampler; no depth, no culling.

A layer that only moved, faded or changed effect uploads no pixels: its mesh and uniforms are all that changed.

`gpu::PARITY` is a table of inputs to the effects and hand-derived outputs. `effect::shade` is tested against it here; a host should run the same rows through the shader in its own GPU tests.

## What is not known

- The shader has been validated with naga and its arithmetic is mirrored and tested in Rust, but it has not drawn a frame on a GPU from this workspace.
- The CPU compositor shades before sampling where the shader samples before shading. The difference is below a level of a channel for every effect except at the outer edge of a glow under magnification.
- On an sRGB target the shader corrects coverage per layer, where a CPU overlay composited afterwards is corrected once as a whole. They agree where layers do not overlap.
- Peel and lay durations, lift, the horizon fade and the ripple are judged by eye on one desktop display.
- The CPU compositor costs per pixel every frame for anything that moves. Measured with the example at 1280×720 on one desktop (two windows on the planet, a shimmering panel, a glowing window with scanlines, a notification): about 13 ms a frame. It is a reference and a fallback, not a way to run a busy interface at 4K.
