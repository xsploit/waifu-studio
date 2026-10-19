# Live memory mapping measurement

**Correction:** the "fresh empty profile" below automatically seeded and loaded the bundled Hikari avatar. A later Heaptrack startup log exposed this. That sample is NOT an empty stage, and the engine-floor conclusion below is withdrawn. The mapping breakdowns remain measurements of loaded processes. A corrected no-avatar run explicitly disables the bundle and uses a different fresh directory. Do not use the old 581.6 MiB sample to estimate engine-only cost.

The corrected, **unprofiled** no-avatar run measured **387.5 MiB mapped RSS / 354.6 MiB PSS**. It explicitly set `WAIFU_STUDIO_BUNDLE` to a nonexistent work-only path and used a new profile, and reported 12 images/4 meshes with no avatar-load event. Its heap/anonymous pages were 127.1 MiB, versus about 310 MiB in the earlier loaded snapshot. These remain different-run/profile samples, not exact attribution. A separate Heaptrack run of no-avatar startup held 123.31 MB of tracked allocations at termination, versus 243.20 MB with bundled Hikari. Those are decimal MB of requested allocations, not RSS; forced termination means Heaptrack's "leaks" label denotes outstanding live allocations, not proven leaks. The loaded capture attributes 88.08 MB to retained morph-target vectors. That is the next substantial feature-preserving optimization candidate.

2026-10-19. Existing release binary only; no compilation, tests or GPU benchmark. Read `/proc/PID/smaps`, grouped resident and proportional pages by pathname. The normal app was temporarily stopped, an empty temporary data directory was launched for 18 seconds, then the normal app was restored. Saved user settings were not edited. Only one app ran at a time.

| Sample | Mapped RSS MiB | Mapped PSS MiB |
|---|---:|---:|
| Earlier loaded session | 589.6 | 556.6 |
| Fresh profile, auto-loaded bundled Hikari (originally mislabeled empty) | 581.6 | 544.0 |
| Restored normal profile | 571.4 | 538.4 |
| Corrected no-avatar profile, bundle explicitly disabled | 387.5 | 354.6 |

The restored normal profile reported 34 images, 20 meshes and 11.2 MiB CPU image data, confirming loaded assets. Its mapped RSS comprises 309.9 MiB heap/anonymous pages, 161.4 MiB identifiable graphics libraries and NVIDIA device mappings, 86.3 MiB executable/other special mappings, and 13.9 MiB other libraries. Anonymous pages can include driver allocations; these groups do not identify allocation owners.

The auto-seeded profile's heap/anonymous total was 300.1 MiB; the genuinely no-avatar profile used 127.1 MiB. Profiles, warm-up history, shader compilation, allocator state and focus differ, so these remain indicative experiments rather than a controlled subtraction. Summed smaps residency is not necessarily identical to earlier `/proc/status` RSS samples. These are regular RAM mappings, not GPU VRAM totals.

Conclusion: at least about 160 MiB of the loaded mapping total is visibly graphics-related, and about 86 MiB is executable/file mapping. Profiling now identifies retained morph vectors as a substantial avatar-side CPU allocation, separate from already-released decoded texture pixels. Do not equate Windows Task Manager's private working set with these Linux counters.

Next work: determine how to release retained morph CPU data without breaking late outlines, first-person mesh generation, exact picking or render reuploads. Separately measure CPU render cost with preview enabled/disabled. Profiling used a locally unpacked Heaptrack, with no system package installation. Its peak RSS includes profiler overhead and must not be reported as ordinary app RAM.

Work scripts: `work/memory-mappings.py`, `work/empty-stage-memory.py`. Raw mapping summaries: `work/memory-{loaded,empty,restored}-mappings.json`. No chat text, keys or environment values were recorded in these summaries.

Corrected run: `work/measure-no-avatar.py`, `work/memory-no-avatar-unprofiled.json`. Profiling: `work/capture-empty-heap.py`, `work/heaptrack-{empty,no-avatar}.zst`, `work/heaptrack-loaded-live.txt`, and `work/heaptrack-no-avatar-summary.txt`. The historical `heaptrack-empty` filename actually denotes bundled Hikari; `heaptrack-no-avatar` is the verified empty stage.
