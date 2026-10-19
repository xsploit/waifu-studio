# Opus follow-up CPU/presentation review

Read-only CLI review; no builds or runtime measurements by reviewer. Statements marked hypotheses remain unverified. Parent per-thread sampling already exists and was included in the prompt.

The best-supported next step is to attribute CPU per thread before touching any code. I haven't found a source-backed hotspot clear enough to optimise blind. On presentation, one existing profile setting can test the FIFO-at-20 FPS question without changing any defaults.

## Verified from source

- **The frame cap is a sleep on the main thread.** `pace_frames` runs in `First` and sleeps for whatever is left of `1/rate`, timed from the end of the previous pace call (`src/runtime.rs:223-240`). It runs in both reactive and continuous mode, so it applies to every FIFO and AutoNoVsync sample.
- **Spring bones cost scales with frame rate.** They run at a fixed 1/60 step, up to 15 steps per frame (`spring_bones.rs:12-37`). At 20 FPS that's about 3 steps per frame; at 60 FPS it's about 1. Total simulation work per second is roughly the same, but anything that runs once per frame triples.
- **Two per-frame costs in spring bones:**
  - Transform propagation runs a second time on every frame that has spring joints: `mark_dirty_trees`, then `propagate_parent_transforms`, then `sync_simple_transforms` (`spring_bones.rs:255-262`). Bevy's propagation runs in parallel on the compute pool.
  - For every joint and every collider, on every step, `compose_global` walks the full ancestry back to the root and rebuilds the global transform from local ones (`:358-385`, called at `:403` and `:428`). Joints in the same chain repeat the same walks.
- **Shaders and preview are already guarded.** Material tuning, expression writes and outline sync all read before they write (`shaders.rs:417, 488, 511, 534`). The preview only does work while its tab is open (`avatar_preview.rs:129-133`). I found nothing new there beyond what's already been rejected.

## Hypotheses (not measured)

1. **The 28.6% compute-pool share may not be main-world work at all.** It could be render-world work, animation or visibility evaluation, or the second propagation pass. I didn't check which pool Bevy's render thread draws from, so spring and propagation code shouldn't be assumed to be the hotspot yet.
2. **If spring/propagation does dominate, two fixes would keep results identical:**
   - Cache each joint's parent global transform within a step (parents come before children), instead of re-walking to the root.
   - Re-propagate only the spring subtrees.

   Either fix needs a test that compares output to the current code within floating-point tolerance.
3. **The CPU baseline is too noisy for small gains.** Three identical runs spread from 30.6% to 41.8%, about 11 points. With three runs per side, a saving smaller than roughly 10 points can't be told apart from noise.

## The one next action: per-thread CPU attribution

Use the existing release binary, AutoNoVsync with the reactive loop, the same isolated profile and the 28 s settle. Then:

1. Snapshot `utime+stime` from `/proc/<pid>/task/*/stat` at the start and end of a 12 s window, grouped by thread name (main, compute pool, render, async, IO). This needs no build.
2. If symbols are available, add `perf record -g` on the compute-pool thread IDs to get function-level attribution.
3. Repeat across 5 or more launches.

This tells you whether spring/propagation, animation or render work deserves the first source change, and it gives a lower-variance per-thread baseline to judge that change against.

## Presentation diagnosis, safest order

1. **Test whether the cap and FIFO interact.** With FIFO and the reactive loop, set the existing `focused_fps` slider to 144 in the isolated profile (the code already allows 15–144). FIFO still caps delivery at 60.
   - If FIFO then delivers about 60, the 20 FPS comes from the pacing sleep's phase interacting with FIFO's blocking acquire (three vblanks per frame). That's a hypothesis to test, not a fix.
   - If it stays at 20, the throttling is in the compositor or the surface path.

   Either way it's a profile value, not a default change.
2. **Log the present mode that was actually chosen, not just the requested one.** `log_runtime_metrics` records `Window.present_mode`, which is only what the app asked for. AutoNoVsync can resolve to Immediate, which tears, or to Mailbox, which is tear-free, depending on the driver. Until the resolved mode is logged, the 59.8 FPS AutoNoVsync result says nothing about tearing.
3. **Optionally add an explicit `PresentMode::Mailbox` probe**, gated by `runtime_probe` like the existing no-VSync switch. Mailbox is tear-free by specification. If it matches AutoNoVsync's cadence and CPU, that gives a tear-free 60 FPS candidate to evaluate before any default is reconsidered.

The X11 opaque-alpha failure is out of scope, as you asked.
