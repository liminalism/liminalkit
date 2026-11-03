# Contributing

## Build and test

```sh
cargo test --workspace --features liminalkit/test-fonts
cargo clippy --workspace --all-targets --features liminalkit/test-fonts
cargo fmt --all --check
cargo run -p liminalkit-ui --example gallery --features test-fonts   # target/gallery@1x.png
```

Stable Rust 1.85 or newer. The workspace forbids `unsafe`. `test-fonts` embeds two Noto faces for examples and tests; application builds leave it off.

## What a change should keep true

- **The application owns its state.** A control takes a rectangle and `&mut` to its state and returns what happened. Do not add a registry, a hashed identity, or per-widget storage inside the toolkit. Keyboard focus accepts a caller-supplied key for the same reason.
- **Layout stays stateless.** Layout helpers are functions from a rectangle to rectangles. There is no retained layout hierarchy, and adding one needs an overwhelming reason.
- **Units are explicit.** Controls take device pixels. Logical sizes are converted once at the call site, and `Theme`'s sizes are logical. A new control scales theme tokens through `Ui::px`.
- **Ownership, allocations, and hidden state are visible in the signature.** A convenient API is welcome; a mechanism that keeps state between frames without the caller seeing it needs a strong reason.
- **Fast paths match the scalar specification byte for byte.** `LIMINALKIT_SIMD=scalar` turns them off; a change to a raster kernel keeps both paths in agreement.
- **Anything hardware-dependent says so.** Touch gestures, scroll physics, and the KMS mode set are labelled untested on hardware until they have been run on it, and a change does not remove that label without evidence.

## Tests

Test behaviour at the state level: a scroll offset clamped at both ends, a selection that survives a resort, a click landing on the right row given the offset, a caret that never lands inside a Thai cluster. Pixel-exact assertions belong in the raster kernel, where a scalar specification exists to compare against. Whether a screen looks right is what the gallery render is for.

## Pull requests

One change per pull request, with the reasoning in the description. A behaviour change comes with a test that would have failed before it. A change to how units or focus work updates the README's "Units" or "Keyboard focus" section in the same change.

The repository also keeps a typed knowledge ledger in `.akr/` with generated views in `docs/generated/`; those are maintained by the project's own tooling and a contribution does not need to touch them.
