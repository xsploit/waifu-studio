# Opus 5.5 Windows performance review

Read-only Claude CLI review using `claude-opus-5-5`; completed 2026-10-28. The following is the reviewer's response, not a measured acceptance report. The running application was not changed by the review.

Parent verification: the 2560x1600 window size is observed, but maximization and bloom/HDR were not established. The app used Vulkan on the NVIDIA GPU. Metrics instrumentation adds work, so a metrics-enabled run is not an overhead-free normal-launch measurement. Bevy extracts all present occlusion components each frame; merely avoiding a same-value main-world write does not avoid extraction.

## Follow-up measurements and change

The running Windows Studio instance was configured for 60 FPS while unfocused and had diagnostic metrics enabled. Two process samples measured approximately 42% and 37% of one CPU core. They do not identify a hot system. The user's existing 60 FPS preference was retained; the application was not restarted or modified in place. Windows Performance Recorder's system-wide CPU profile could not be started from the current non-administrator session.

Inactive desktop-occlusion masks are now removed from the camera, avoiding their render-world extraction and uniform upload. Nonempty masks are still inserted and updated as needed. Windows and Linux tests cover activation, updates and removal, and a Windows GPU regression verifies active, removed and restored masks with 1x and 4x MSAA. Existing desktop-occlusion geometry tests pass on both platforms.

An exploratory, windowless comparison used Hikari and the same talking animation at 2560x1600. Three 2.5-second CPU samples per build had medians of 41.86% of one core before and 37.49% after. Their ranges overlapped, the runs were short, startup and other system activity can affect them, FPS was not measured, and the builds also contained earlier disabled animation-guard changes. This does not establish the size or cause of any CPU improvement or represent a desktop benchmark. Longer controlled tests and actual CPU attribution remain necessary.

---
# Waifu Studio Windows CPU review (read-only)

I couldn't find the cause of the ~42% figure from source alone. Nothing below was measured by me: I built nothing, ran nothing and didn't touch the running app. Every cost claim here is a hypothesis unless it cites existing measurement files. The Mate Engine comparison is limited because I couldn't search outside the repo.

## What the existing measurements show (`reviews/efficiency-followup.md`)
- **All earlier numbers are Linux, not Windows.** They came from KScreen, Wayland and `/proc`. With 4 compute workers at 59.8 FPS they show 23–26% for one avatar (lines 165–185). Window size and Studio-vs-Desktop mode weren't recorded, so the Windows 42% can't be compared with them directly.
- **The Linux CPU profile was spread out, not one hotspot** (lines 140–146). Scheduler-related entries were about 6%, transform propagation 2.17%, springs (`compose_global`) 2.3% and skin extraction about 1%.
- **Enabling metrics doesn't change how the app runs.** With metrics on but no override variables, the presentation mode is still Mailbox (`runtime.rs:86-93`). Logging happens once every 10 seconds. So metrics-on samples represent normal behaviour.

## Prioritized findings

**1. The 42% sample is missing context.** The 2560×1600 window must be the maximized Studio window. Desktop companion mode caps the window at `420×640×zoom` and at the monitor size minus one pixel (`runtime.rs:625-641,729-742`). That means the full egui UI is rebuilt and tessellated every frame (`ui.rs:124`), and 3D post-effects (HDR, bloom) are likely active (`visuals.rs:319-322`). No MSAA override exists, so Bevy's default (4×, if I recall Bevy's default correctly) applies at 2560×1600. If the Characters or Scene tab is open, a second camera also renders the avatar every frame (`avatar_preview.rs:129-131`). Each future sample should log the mode, tab, backend and adapter.

**2. Main-thread egui work in Studio mode (hypothesis, possibly the largest item).** `studio_ui` (`ui.rs:124`) builds the whole UI every frame and has no early return except in Desktop mode (`ui.rs:153`). Chat history layout and tessellation grow with conversation length. This doesn't exist in the Linux numbers if those were taken in companion mode.

**3. Springs run a full second transform-propagation pass every frame** (`vendor/crates/bevy_vrm/src/spring_bones.rs:255-262`). On top of that:
   - `idle_motion` writes the avatar root transform every frame (`runtime.rs:1163-1184`), so the whole skeleton is marked dirty in the first pass.
   - `compose_global` walks every ancestor up to the world root for every joint, and again for every collider in every chain, on each step (`spring_bones.rs:358-385,400-408`).
   - It builds a full `to_matrix()` per ancestor just to check that values are finite (`:371`).

   The Linux profile measured springs at only about 2.3%. I can't estimate the second pass's share without per-system timing.

**4. Desktop mode only: window enumeration on the main thread.** `snapshot()` calls `EnumWindows` (with DWM queries per window) plus `EnumDisplayMonitors` whenever its cache is older than 33 ms (`desktop_platform/windows.rs:182-192`). That's up to about 30 times per second, from `sync_desktop_platform` and `track_desktop_pointer` in Update (`desktop_behavior.rs:174-177,311-313`). The same code also replaces `surface_status` with a new String every frame (`desktop_behavior.rs:314`), so `DesktopSettings` counts as changed every frame. The cost depends on how many top-level windows are open.

**5. Small every-frame overheads:**
   - `sync_occlusion` re-inserts the `DesktopOcclusion` component every frame, even in Studio mode (`desktop_behavior.rs:255`). Combined with the ExtractComponent and uniform plugins, that means an extract plus a uniform-buffer write each frame. The draw call itself already skips empty masks (`desktop_occlusion.rs:214`).
   - `apply_material_expressions` clones and sanitizes the material settings and allocates a HashSet every frame (`shaders.rs:461-464`).

   Each is probably under 1%.

**6. Windows-specific hypotheses, all unmeasured:**
   - **Graphics backend is unpinned.** There's no `WgpuSettings`, so wgpu picks Vulkan or DX12 itself, and the driver's own threads count toward the process CPU.
   - **The window is always created transparent** (`runtime.rs:108`), even in Studio. DWM therefore has to composite it and can't use independent flip; DWM's own CPU (`dwm.exe`) isn't counted in the app's number.
   - **Task-pool wake/park overhead** may be higher on Windows. The 4-worker limit was chosen from Linux evidence only.
   - **Double frame pacing.** `pace_frames` sleeps (`runtime.rs:335-362`) on top of winit's reactive timer. That's fine at 59 FPS, but the timer behaviour hasn't been verified on Windows.

**7. Ruled out in source:**
   - Avatar loading does no per-frame work when idle (`loading.rs:196`).
   - Body animation returns early when not posing (`animation.rs:1706`).
   - `share_morphs` only runs when mesh events arrive (`morph_sharing.rs:87`).
   - Material writes are guarded by value comparisons.
   - Framing and arm-relaxation stop once they've completed.

## Safe Windows profiling (no source edits)
1. **Tracy for per-system timing.** Build with `cargo build --release --features bevy/trace_tracy --target-dir target-tracy`, then capture 15 s in Studio-Chat, Studio-Characters and Desktop. Look at `studio_ui`, both `propagate_parent_transforms` passes, `do_springbone_logic`, extract/prepare/render, and the desktop systems. Tracing adds overhead, so use it for proportions only, not CPU totals.
2. **ETW sampling with symbols.** Build with `$env:CARGO_PROFILE_RELEASE_DEBUG="line-tables-only"` and `--target-dir target-prof`, then record with `wpr -start CPU` / `wpr -stop x.etl` and open in WPA (or use `samply record`, which needs admin). Split CPU by module (app vs driver DLL vs ntdll) and by thread.
3. **PresentMon** for the present path ("Composed: Flip" vs "Independent Flip"), frame times and GPU busy time. Record `dwm.exe` CPU alongside.

## Reproducible A/B tests
Use the same profile, Hikari, focused window, 28 s settle + 12 s sample, three alternating runs per arm. Measure with `typeperf "\Process(<exe>)\% Processor Time"` (100% = one core) and per-thread `TotalProcessorTime` deltas.

| Test | Arms | What it tells you |
|---|---|---|
| Mode | Studio Chat / Studio Characters / Desktop companion | Cost of egui and the preview camera |
| Window size | 1280×900 / 2560×1600, Studio | Whether resolution, fill rate or driver cost matters |
| Compute workers | `WAIFU_RUNTIME_METRICS=1` + `WAIFU_RUNTIME_COMPUTE_THREADS=1/2/4` (supported today) | Task-pool overhead on Windows |
| Present mode | default Mailbox / `WAIFU_RUNTIME_PRESENT_MODE=fifo` | Present-path cost |
| Attribution only | In a copied profile: idle motion, outlines, bloom/HDR, desktop window tracking (`kwin_window_surfaces`) each off in turn; restore afterwards | Size of each feature's cost |
| Chat length | Short vs long chat history | Whether egui cost grows with history |

## Improvements to make after attribution (behaviour unchanged)
- **egui:** if it dominates, render chat history with `ScrollArea::show_rows` or virtualized rows, and cache layout.
- **Desktop window tracking:** move `EnumWindows` sampling to a worker thread that publishes snapshots, and only assign `surface_status` when it actually changes.
- **Occlusion:** insert `DesktopOcclusion` only when the mask changes.
- **Springs:** propagate only the subtrees under simulated bones instead of a full second pass. Cache collider transforms per step, with invalidation when a simulated ancestor changes. Use a cheaper finiteness check. Each needs collider and skinning regression tests.
- **Workers:** set the Windows worker limit from the sweep results, not from the Linux numbers.

## Mate Engine
`reviews/mate-engine-efficiency.md` records an earlier read of a local checkout at commit `2c5ea6b`. That audit cites frame caps defaulting to 60/90 with VSync off (`FPSLimiter.cs:42`) and quality tier 1 with half-resolution textures. I couldn't find the checkout's path because searching outside the repo was denied, so I didn't re-check that source and can't make a new CPU comparison.
