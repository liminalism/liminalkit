# liminalkit-kms

The Linux DRM/KMS and evdev host for a [liminalkit](../../README.md) `PixelApp`: full screen on a bare display with multi-touch and no window system.

**What it is for.** A kiosk, a control panel, a single-purpose device on a Raspberry Pi. `run_kms` takes the display, sets a mode (the panel's preferred one unless told otherwise), copies each frame into a scan-out buffer with the requested rotation, paces frames by the page flip, reads multi-touch from evdev with an optional calibration transform, and reconciles touchscreen hotplug. `probe` lists the displays the process can see without taking one. Errors are fatal by design: the supervisor restarts the process.

**What it is not.** No keyboard, IME, clipboard, cursor or multiple windows. Needs DRM master: a virtual terminal with no desktop, and membership in the `video` and `input` groups. The touch decoder, rotation and pacing are unit-tested; setting a mode on real panels, rotations and refresh rates is open work the README labels as such.

Compiles on every platform; does something only on Linux.
