# liminalkit-shell

What a window host and a [liminalkit](../../README.md) application share, with no window system in it.

**What it is for.** The per-frame `Input` (pointer, clicks taken by one control each, keys, scroll, modifiers, IME composition, accessibility edits), `Scale` for logical-to-device conversion, the `Presenter` seam a host implements to put a finished buffer on screen, frame timing and the `LIMINALKIT_FRAME_LOG` report, the system clipboard, and touch contacts with pan and pinch.

**What it is not.** Not an event loop and not a window: those are in `liminalkit-windowing` and `liminalkit-kms`, which both drive the same `Input`. The gesture recogniser, scroll physics and touch replay modules are a separate layer that compiles and is unit-tested but has not been run on a touchscreen; they say so in their docs.

**Who uses it.** Both hosts, `liminalkit-ui`, and a custom host that wants to feed liminalkit from somewhere else.
