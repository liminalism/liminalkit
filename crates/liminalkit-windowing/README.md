# liminalkit-windowing

OS windows for a [liminalkit](../../README.md) application, and windows inside the application.

**What it is for.** `run_app` opens a winit window, has the application draw straight into softbuffer's buffer (or any `Presenter` the application supplies) and presents what it damaged, tracks the scale factor, forwards keys, IME, pointer, scroll, touch, file drops and theme changes to a `PixelApp`, and sleeps until input or a `Waker` arrives. `run_windows` shares one event loop between several windows, each with its own application. `Panes` tiles a region with draggable dividers; `Windows` manages floating in-app windows that move, resize, raise and close. Both are caller-owned state updated once a frame before drawing.

**What it is not.** Not a widget toolkit (that is `liminalkit-ui`, which it does not depend on) and not the Linux bare-display host (`liminalkit-kms`). `winit` and `softbuffer` stay in this crate so the others never import them.

**Features.** `accessibility` publishes the application's AccessKit tree through `accesskit_winit`.
