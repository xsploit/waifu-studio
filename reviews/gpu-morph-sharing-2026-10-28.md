# Desktop-mode morph-buffer optimization

Windows measurements used Hikari, the bundled assets, an isolated profile, 60 FPS limits, and an inactive local Win32 desktop. The user's running instance was left alone. Performance instrumentation and local control were enabled. CPU is percent of one logical core. Results are three consecutive 12-second samples on a machine in use, with automatic animation potentially varying; the inactive desktop differs from the user's interactive desktop.

Hikari's meshes reused the same immutable CPU morph data, but the renderer uploaded it repeatedly. The storage-buffer renderer now shares an allocation for meshes with the same backing data, keeping independent per-entity morph weights. GPU allocation ownership is reference-counted and handles replacement of the original mesh; a weak reference preserves backing identity without retaining the CPU data. The WebGL image-based path is not covered by this optimization. The initial before-and-after measurements below used an earlier strong-reference implementation; the final weak-reference refinement also passed the rendering regression on both platforms.

| Measurement | Before | After |
| --- | --- | --- |
| GPU morph allocations | 12 | 2 |
| GPU morph buffer slab bytes | 322,486,272 | 28,311,552 |
| Windows dedicated GPU memory | 1,166.4 MiB | 858.8 MiB |
| Process private memory | 1,682–1,685 MiB | 1,380–1,382 MiB |
| Process working set | 538–540 MiB | 539–540 MiB |
| FPS | 58.89–58.98 | 58.72–58.87 |
| CPU, percent of one core | 39.97–43.49% | 40.23–41.14% |

These are process-level counters, not a guarantee of savings for other models or drivers. The unchanged working set matters: this test does not show a 300 MiB reduction in resident RAM. The CPU measurements do not establish a CPU improvement from morph sharing.

A fresh three-sample run of the final weak-reference build reproduced 2 morph allocations, 28,311,552 morph-slab bytes, and 858.8 MiB dedicated GPU memory in all samples. FPS was 58.90–58.96, working set 545.7–547.2 MiB, and CPU 38.02–48.96% of one core. Different idle animations played during the samples. The CPU ranges and changing motions make causal comparisons unreliable.

The rendering regression renders two meshes sharing morph data with different weights, replaces the owner's morph data, and removes the owner, checking the remaining mesh at each step. It passed on Windows and on WSL Linux using the llvmpipe Vulkan software renderer. Five runtime-metrics tests also passed on WSL Linux; this is not a native Linux GPU performance test. A fixed Hikari frame at 0.5 seconds in `amusement_01.bvh` was pixel-identical before and after the change. Six sequential swaps between Hikari and another avatar successfully loaded each model; morph-buffer slab capacity increased after the first swap from 28,311,552 to 63,700,992 bytes and then remained stable over three cycles, while the dedicated GPU counter stayed at 858.8 MiB. This is not a long-duration leak test.

With the same full-asset setup, one compute worker used 29.42%, 28.25%, and 36.72% of one core versus 40.23%, 41.14%, and 41.01% with four. However the last one-worker sample dropped to 57.28 FPS and had a 41.32 ms frame-time p99; the four-worker samples were 58.72–58.87 FPS with p99 of 17.66–20.83 ms. These short sequential runs do not justify changing the default without interactive and heavier-scene tests.

The source build's `work/test-readiness/desktop-performance/Launch-Optimized.cmd` sets the asset root and starts the release binary with the normal profile. Close an existing instance first to avoid two instances editing that profile. This is a development test build, not a packaged release.

This optimization does not repair animation poses, arm or clothing clipping, or eye targeting. The fixed comparison frame shows existing pose problems; pixel equality only demonstrates that the renderer change did not alter that frame. Other desktop resource-lifetime findings and the correction about earlier incomplete asset runs are in `desktop-mode-performance-2026-10-28.md`.
