# Contributing

## Build and test

```sh
cargo test --workspace
cargo test -p liminalkit-text --features bidi
cargo clippy --workspace --all-targets
cargo fmt --all --check
cargo run -p liminalkit-ui --example gallery   # target/gallery@1x.png
```

Those commands are what `.github/workflows/ci.yml` runs, plus the test suite with the `accessibility` feature and the gallery render. The README's screenshot is `docs/screenshots/gallery.png`, the 1x gallery render recompressed; regenerate it when a control's look changes. Stable Rust 1.85 or newer. The workspace forbids `unsafe`. Tests and examples draw with the Noto faces in `crates/liminalkit-test-fonts`, which is `publish = false` and only ever a dev-dependency, so no published crate and no application build carries them.

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

## Versioning

The workspace shares one version. Every crate is released together and each workspace path dependency pins that exact version.

Before 1.0 the number is `0.MINOR.PATCH`, and it is derived, not chosen:

- **MINOR counts API generations.** A generation closes when a change lands after which an application had to edit its own code to keep building or to keep behaving the same: a renamed crate, module or type; a changed signature; a changed meaning of an existing call, such as units, frame lifecycle or who owns input; a removed feature. Cargo already treats the 0.x minor as the compatibility boundary, so this is also what keeps `cargo update` safe for a dependant.
- **PATCH counts releases within a generation:** additions and fixes an existing application absorbs without an edit.
- A generation counts whether or not it was tagged. Nothing has been published to crates.io; applications consumed each generation from git, and the edits they had to make are the evidence. `CHANGELOG.md` lists the generations with the commits that closed them, which is how the current number was arrived at.
- **1.0** when a generation has gone through two application release cycles without a breaking change and the ledger's publishing question is resolved.

To cut a release: finish the `CHANGELOG.md` section, set `version` in the root `Cargo.toml` and the matching `version` on each workspace dependency there, run the commands above, and tag `v0.MINOR.PATCH`. A breaking change in a new generation also gets a line in the changelog's **Breaking** list saying what a dependant has to edit.
