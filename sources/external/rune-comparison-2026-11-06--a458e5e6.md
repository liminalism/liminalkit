# Outside comparison of liminalkit with Rune on performance and memory, received 2026-11-06

Pasted by the project owner from an outside agent, which had compared liminalkit with Rune, a retained-mode GPU-rendered Rust GUI framework in active development, with the instruction to work in the repository based on the advice and to keep asking whether the better parts of it can be made simpler. The owner's prompt is the two paragraphs this note replaces; the rest is the agent's text as pasted, including its chart data, which the paste flattened.

## Rune vs. LiminalKit: performance and memory

My initial conclusion is that LiminalKit has the stronger architecture for minimizing baseline memory consumption and keeping a simple application lightweight, while Rune has the stronger architecture for sustained rendering performance, complex animations, and large displays.

However, Rune is substantially more performance-conscious than a typical retained-mode GUI framework. It uses keyed state, optimized layout, GPU instancing, caching, and specialized fast paths. It would be a mistake to assume that LiminalKit is necessarily faster simply because it has fewer abstractions.

The more interesting distinction is where each framework spends its resources:

- LiminalKit spends CPU time generating pixels directly, while retaining very little framework-managed UI state.
- Rune spends more memory maintaining a scene, text resources, and GPU infrastructure, but can avoid substantial CPU work when rendering or animating that scene repeatedly.

Both are capable of essentially zero rendering work while idle. Their differences become important when the user interacts with the application.

This comparison is based on the repositories' current architecture, implementation documentation, and the performance measurements published in Rune's repository. It is not a same-machine benchmark of the two frameworks.

## 1. Expected runtime performance

| Characteristic                         | LiminalKit                               | Rune                                           | Likely advantage        |
| -------------------------------------- | ---------------------------------------- | ---------------------------------------------- | ----------------------- |
| Idle CPU consumption                   | Sleeps until events or scheduled redraws | Sleeps when settled                            | Tie                     |
| Small, static UI                       | Direct drawing, minimal bookkeeping      | Scene and layout machinery                     | LiminalKit, potentially |
| Repeated redraws                       | Re-executes drawing commands             | Reuses retained resources and GPU state        | Rune                    |
| Smooth animation                       | CPU rasterization every frame            | GPU instances and spring animations            | Rune                    |
| Scrolling large datasets               | Visible-row drawing, app-controlled      | Virtual rows, retained geometry, GPU rendering | Workload-dependent      |
| 1080p interactive UI                   | Efficient if screen is relatively simple | Efficient GPU compositing                      | Depends on complexity   |
| 4K, frequent updates                   | CPU pixel workload becomes significant   | GPU scales more favorably                      | Rune                    |
| Cold startup                           | No GPU initialization required           | GPU device and pipeline initialization         | LiminalKit likely       |
| Predictable memory footprint           | Explicit state and bounded caches        | More retained and backend-managed resources    | LiminalKit              |
| Rasterization without graphics drivers | Supported                                | Requires a usable GPU backend                  | LiminalKit              |

### LiminalKit's significant performance limitation

I found an important detail in LiminalKit's software presenter (→ github.com).

Its default presentation path effectively does:

That means LiminalKit maintains its own framebuffer and copies the entire image into softbuffer's presentation surface.

Even if you optimize the actual drawing to touch only a small portion of the screen, the default presenter still copies all the pixels.

This matters more than the number of widgets.

At 1920×1080, one full copy transfers approximately 7.9 MiB. At 60 FPS, that is approximately 475 MiB/s in framebuffer copying alone, before accounting for software rasterization and whatever the compositor subsequently does.

At 4K, it becomes about 1.9 GiB/s.

These rates are not necessarily problematic on a modern desktop, but they are avoidable memory traffic. They are particularly consequential on an embedded machine sharing memory bandwidth among the CPU, GPU, and other services.

Rune's GPU-native approach avoids this particular full-frame CPU-to-presentation-buffer copy.

This is the first architectural optimization I would investigate for LiminalKit. A presenter that lets the rasterizer draw directly into an appropriately accessible presentation buffer could eliminate one full-frame copy. Whether that is possible depends on the backend's mapping and buffer-lifetime rules.

## 2. Memory usage

This is where the difference becomes more interesting.

LiminalKit: one 32-bit framebuffer at each resolution

One buffer

Two buffers

0 MiB20 MiB40 MiB60 MiB80 MiB1280×8001920×10802560×14403840×2160

Calculated framebuffer storage only, not measured total process memory

The two-buffer figures are useful because LiminalKit's default desktop path has the application framebuffer plus the softbuffer presentation surface. Additional operating-system or compositor allocations are excluded.

### LiminalKit's text cache is not as small as I expected

Reading liminalkit-text/src/cache.rs (→ github.com), I found:

So your default rasterized-text cache has a 48 MiB budget, plus entries and other supporting caches.

That isn't 48 MiB allocated at startup. The cache grows as strings are encountered. But it is a substantial allowance for software-rendered text, especially on a constrained machine.

The implementation also performs wholesale eviction when its limits are exceeded, rather than conventional LRU eviction.

I understand why: it keeps normal cache hits inexpensive. But if an application repeatedly cycles through more text than the cache can retain, this can cause repeated expensive rasterization.

For a small kiosk, I'd consider configuring 8–16 MiB initially, then measuring hit and eviction behavior. Your API already supports custom limits.

### Rune's retained memory overhead

Rune has several additional resource consumers:

- A retained node representation, layout and motion state.
- Parley text layouts and associated shaping resources.
- A 2048×2048, 8-bit glyph atlas design, with approximately 4 MiB per full page.
- GPU textures, staging resources, presentation surfaces and pipelines.
- Optional cached subtree textures, with a default 64 MiB cache budget.

Its renderer documentation (→ github.com) confirms the two-page glyph-atlas pool and subtree texture budget.

However, Rune also has unusually informative real measurements.

In research/31-layout-text-cost.md (→ github.com), the developers measured the following settled live heap:

| Workload                                       | Rune live heap |
| ---------------------------------------------- | -------------- |
| Virtualized 10,000-row list, 278 mounted nodes | 11.4 MB        |
| 1,268-node Markdown document                   | 20.8 MB        |
| 5,283-node settings-like scene                 | 57.9 MB        |
| One 50 KB text-editing field                   | 19.2 MB        |

These are measured application/runtime heap figures, not complete process RSS or GPU memory. They also come from an earlier implementation; subsequent optimizations changed Rune's memory structures.

Even so, they reveal an important distinction: Rune's retained scene and text representations can consume significant amounts of memory independently of screen resolution.

LiminalKit doesn't have an equivalent retained-tree cost. If you create application data structures, those still consume memory, of course, but the framework doesn't independently duplicate that data as a persistent widget scene.

I would therefore expect LiminalKit to have a meaningful memory advantage for a modest fixed-screen application, though I cannot defensibly assign a percentage without running both.

## 3. Actual performance evidence from Rune

Rune is not merely relying on GPU acceleration to compensate for a complicated framework. Its developers have been aggressively benchmarking and reducing CPU overhead.

Their research/37-what-to-own-next.md (→ github.com) reports these CPU timings on an M2 Max, using headless workload replays:

| Workload                       | Median CPU/frame | 95th percentile |
| ------------------------------ | ---------------- | --------------- |
| Team chat scrolling            | 0.46 ms          | 0.75 ms         |
| Notes scrolling                | 0.63 ms          | 0.84 ms         |
| Agent conversation scrolling   | 0.56 ms          | 1.36 ms         |
| Agent conversation typing      | 0.88 ms          | 1.15 ms         |
| Agent conversation live resize | 1.23 ms          | 1.72 ms         |
| Task board scrolling           | 0.25 ms          | 0.48 ms         |

Those are impressive numbers for a retained-mode Rust GUI framework.

They are not end-to-end frame times, and they exclude GPU and presentation overhead. But they show that Rune's retained architecture does not inherently impose a large CPU cost per interaction.

The more recent research/38-view-rerun-and-commit-cost.md (→ github.com) reports further reductions to scene rebuilding and allocations.

Rune also demonstrates an effective mechanism for camera transformations. Its live_only() path avoids rerunning the view and layout during certain interactions. In a published 6,000-element pinch scenario, CPU time fell from 13.7 ms to 2.0 ms per frame.

That is exactly the sort of optimization that matters for fluid GUIs.

LiminalKit has a different advantage: it doesn't need to reconstruct or reconcile a widget tree in the first place. But it still needs to draw the screen after a change.

I wouldn't declare LiminalKit faster in CPU terms without actually measuring its rendering cost. Rune has credible evidence of very low update overhead, whereas LiminalKit currently provides a benchmark program rather than published comparable results.

## 4. Startup and idle behavior

At rest, both architectures are sensible. LiminalKit's host does not continuously redraw; Rune likewise stops animation ticks and unnecessary rendering when settled.

Startup favors LiminalKit architecturally because it does not have to initialize a GPU device or compile GPU pipelines.

Rune's own startup documentation (→ github.com) says a small macOS app can get its first window on screen in roughly 300 ms on Apple silicon, using GPU prewarming and precompiled shaders. That is not bad, but it represents extra startup machinery.

Rune has also spent considerable effort reducing GPU-driver CPU overhead, as described in research/30-overlay-gpu-cost.md (→ github.com).

For CPU utilization and battery consumption, the distinction is therefore largely about what happens while the screen is active. Neither framework should waste significant CPU time simply because a static window is open.

## 5. Where I would expect each to win

LiminalKit

Small desktop utilities and embedded operational screens

A relatively static 1280×800 or 1080p interface, infrequent updates, straightforward layout, and limited animations. LiminalKit's minimal state machinery, predictable allocations, and direct DRM/KMS support are real advantages.

Rune

Dynamic, animated, high-resolution interfaces

Rich transitions, rapidly scrolling text, large composited surfaces, and continuous interactions. Rune can keep work on the GPU while reusing layout, text, and scene resources.

An important nuance is that a high-frequency updating dashboard isn't automatically a victory for Rune. If only a few numerical values change on a small panel, LiminalKit could be extremely competitive, particularly with partial redraw and direct presentation. Conversely, an animation-heavy UI on a 4K monitor can make software rasterization expensive regardless of how little widget state the toolkit retains.

## 6. What I'd change in LiminalKit

From a performance-engineering perspective, I'd prioritize three things rather than adopting Rune's architecture.

First, investigate eliminating the framebuffer copy. The current default presenter makes a whole-frame copy unavoidable. A direct-write presentation path could materially reduce bandwidth consumption without changing your application or widget APIs.

Second, build an optional dirty-region rendering contract. Right now the normal pattern is to clear and paint the whole screen. Retained widget state isn't necessary for invalidation: the application already owns enough state to identify affected rectangles. The key is ensuring those rectangles remain valid through drawing and presentation.

Third, develop an instrumented cross-framework benchmark. Your existing frame_bench.rs (→ github.com) is a useful foundation. It already measures warm/cold software frame times and process resident memory across multiple resolutions. I would add CPU allocation counts, text-cache occupancy and evictions, paint-versus-present timing, and an actual interactive host workload.

Rune's benchmarking culture is worth emulating. Its research documents include unfavorable results, explanations of bottlenecks, and experiments that were abandoned. That makes its performance claims more credible.

## Final assessment

If I were choosing a renderer for a memory-constrained Raspberry Pi kiosk, I would still favor LiminalKit. It offers a more predictable resource model, works without a GPU rendering stack, and can drive a bare Linux display directly. Its current design is particularly well suited to an interface that mostly waits for human input.

If I were choosing one for a large, graphically rich application on a modern workstation, I would favor Rune's architecture. Its GPU pipeline and retained rendering resources offer more headroom, and its demonstrated CPU performance is already strong.

The biggest practical risk for LiminalKit is memory bandwidth and sustained CPU rasterization, not framework bookkeeping. The biggest practical risk for Rune is higher baseline memory and GPU infrastructure overhead, not that its retained tree is necessarily slow.

I wouldn't replace LiminalKit's explicit-state design with Rune's retained-tree model. I'd instead focus on the presentation path and incremental drawing. Those two improvements could preserve the main reason LiminalKit exists while improving the part of its performance profile where Rune is most likely to outperform it.

The decisive next experiment is the same operational screen at 1080p in both frameworks, measuring full-process RSS, warm frame CPU time, actual presentation latency, and sustained CPU/GPU utilization—not widget counts or synthetic layout-only timings.
