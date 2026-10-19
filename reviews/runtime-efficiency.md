# Runtime efficiency investigation — 2026-10-19

## Observed results

Same saved avatar and plain-background profile, no generated speech or paid/model benchmarks. Linux, NVIDIA RTX 5060 Ti. CPU percentages below use **one logical core = 100%**, not the whole machine. RAM resident (RSS) includes shared executable/library/driver mappings; proportional RAM (PSS) accounts for sharing. These short samples are not long-duration, cross-platform or matched Mate-Engine/WebWaifu4 benchmarks.

| Build / experiment | CPU | RSS | PSS | Update FPS | Threads |
| --- | ---: | ---: | ---: | ---: | ---: |
| Original debug, early foreground 10 s | 285.5% | 1014 MiB | 886 MiB | Not instrumented | 41 |
| Original debug, later unfocused/activity-dependent 10 s | 112.2% | 1040 MiB | 909 MiB | Not instrumented | 41 |
| First release + outline/texture/shadow fixes, 10 s | 38.5% | 879 MiB | 747 MiB | 59.8, focused | 41 |
| Release experiment: 8-CPU affinity + arena limit 2, 15 s | 31.4% | 733 MiB | 604 MiB | 59.8, focused | 29 |
| Final release + settled-load reclamation, normal affinity/allocator, 15 s | 41.9% | 652 MiB | 524 MiB | 59.8, focused | 41 |

Experiments combine changes and are not isolated attribution of every patch. Early debug samples did not record actual FPS; current source defaults and focus-dependent cadence explain why different idle samples differ. The final sample used normal worker/allocator settings; CPU affinity and MALLOC_ARENA_MAX experiments are **not shipped defaults**. Resident decoded image data in the final app was 11.2 MiB. A later NVIDIA per-process snapshot still reported 843 MiB VRAM; no VRAM reduction is claimed. The selected avatar's embedded PNGs decode to about 109 MiB, in addition to other resources. There is no evidence that we have reached 200 MiB or beaten a matched browser/Unity build.

## Changes

- `run.sh` previously launched an unoptimized development executable with up to four build jobs by default. It now selects optimized release mode with one default job.
- Outline synchronization checks values before requesting mutable material access, avoiding unchanged asset modifications/uploads. Regression coverage checks clean subsequent frames and real outline updates.
- VRM and glTF avatar textures use render-world-only storage. Decoded pixels can be released from main-world RAM after upload; CPU meshes remain for picking, bounds and generated outlines. Imported source files and texture quality are unchanged.
- On GNU/Linux, after an avatar change settles for three seconds and chat/speech is idle, one bounded worker invokes glibc `malloc_trim(0)` to return unused allocator pages. It does not free live allocations or run every frame. Other platforms are unchanged.
- A suppressed room is despawned rather than hidden, freeing mesh/material handles, and rebuilt from the unchanged saved selection on return. The test checks asset release, selection changes while suppressed and restoration. The user's environment was already None, so this did not explain the original measurement.
- Hidden image backgrounds release their handles while a room or desktop mode covers them; their saved choice is kept. Returning reloads the image.
- Directional shadows are disabled in plain-background Toon/Unlit views, where the shader/backdrop does not consume them; active rooms and Lit appearance retain shadows.
- Opt-in `WAIFU_RUNTIME_METRICS=1` logs update FPS, focus, retained CPU image bytes and mesh/image counts every ten seconds. It contains no chat/profile content and is off in ordinary launches.

## Verification boundaries

The final full all-target suite passed 588 tests (477 lib + 100 bin + 6 integration + 2 + 3 example tests); strict Clippy, touched-file formatting, launcher syntax and whitespace checks passed. The full receipt is in VALIDATION.md. All Cargo work used one low-priority job. The first uncached release build took 30m18s; the subsequent application-only build took 2m25s.

The app starts and reports loaded mesh/image assets at the recorded cadence. A desktop screenshot request returned an occluding browser window rather than a trustworthy avatar capture; no visual acceptance is claimed from that image. No provider synthesis, microphone or live motion test was performed in this investigation. User previously confirmed the separate speech-startup fix feels much faster.

## Remaining work

RAM remains higher than the user's 200 MiB reference. Further work should profile live allocations and render/driver resources on this optimized build, then compare the same avatar, resolution, FPS, effects and process accounting against another engine. Avoid guessing from language/runtime alone. Repeat samples for desktop mode, minimized/background operation, avatar switches and long sessions before claiming broad efficiency or leak freedom.
