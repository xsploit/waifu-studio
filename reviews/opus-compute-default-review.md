# Opus compute policy review

Read-only CLI source/evidence review; no builds or runtime checks by reviewer.

**Verdict:** the evidence supports shipping `compute.max_threads = 4` as the normal default. I found no source-backed reason to keep it opt-in. There is one real gap to close first: every measurement ran at 60 FPS, but the app lets users raise `focused_fps` to 144 (`desktop_settings.rs:127,162`; `runtime.rs:269`). That is the only setting where four workers could plausibly become the bottleneck. One bounded run at 144 FPS settles it.

This is a source and evidence review only. Nothing was built, run or tested. I couldn't read Bevy's registry source (permission denied), so the Bevy-internal points below are from my knowledge of Bevy 0.19, not checked against the crate.

## Who actually uses the compute pool (source-checked)
- **App and vendored crates:** none. There are no `par_iter`, `ComputeTaskPool`, `AsyncComputeTaskPool` or `IoTaskPool` uses in `src/` or `vendor/crates/` (bevy_vrm, spring bones, mtoon, serde_vrm, the patched bevy_mesh). The only reference is the metrics log at `runtime.rs:233`.
- **Background work in the app** (voice, provider, MCP, agents, fish, memory reclaim, animation loading and so on) all uses `std::thread::spawn` or `Builder::spawn`. Capping the compute pool doesn't touch any of it.
- **Bevy internals** (not verified in the crate): the compute pool runs the parallel scheduler for both the main and render sub-apps, which share it under pipelined rendering. It also runs Bevy's own `par_iter` work: transform propagation, visibility, animation targets and render prepare/batching. glTF/VRM loading runs on the IO pool, which isn't changed. That fits your unchanged VRM_READY timings.

## Why the change is low-risk
- **`task_pool_plugin` change is small:** set `compute.max_threads = 4` by default, keep the metrics-gated env override, and leave `min_threads`, IO and async-compute alone. Make sure the override can still raise the cap above 4, so it still works as an A/B lever. Also update the doc comment at `runtime.rs:36-37`, which currently says normal launches keep Bevy's policy.
- **Smaller machines are unaffected:** with the default 25%/max-4 split for IO and async, compute only exceeds 4 when there are about 11 or more logical CPUs. On most smaller machines the cap does nothing.
- **Why it saves CPU:** the saving fits idle worker wake/park churn, which grows with worker count. Each scope also runs work on the calling thread, so real parallelism is a little more than four.
- **Frame time:** the four-avatar runs show no p99 regression at 60 FPS.

## Real risks (bounded)
1. **No headroom data.** p99 matching at a 60 FPS cap hides how much spare time each frame has. At 144 FPS the budget is 6.9 ms, and the main and render sub-apps compete for four workers. A shortfall would show up there first.
2. **Heaviest scenes weren't measured.** Untested: four avatars plus a room environment (RoseBedroom, VioletStudio or CyanLounge, which also switch on visual presets), plus mesh-picking hover and egui controls open at the same time.
3. **The four-avatar test used one asset, copied.** It's representative for thread count, but not for a varied skinning and morph load. That's acceptable; just say so.
4. **Load spikes.** Readiness interval matched, but frame times during avatar or environment swaps weren't sampled. glTF loading is mostly on the IO pool, so this risk is low.

## Smallest additional checks before adopting
1. **High frame rate:** focused cap at 144, four avatars, one room environment with its visual preset, alternating default and four, three runs each with the same settle and sample timing. Pass if achieved FPS and p99 for four are within run-to-run noise of default.
2. **Heavy interactive frame:** the same scene at 60 FPS with controls open and continuous hover over the avatars, so mesh picking runs. Compare p99.
3. **Offscreen capture smoke test:** one `offscreen` capture with four. It uses `ScheduleRunnerPlugin` and the same task-pool plugin, so confirm the output image is identical or equivalent.
4. **Rerun after the default flips:** the existing full suite and strict clippy, plus a unit test that the default is 4 without the env vars and that the metrics-gated override still parses.

## Unknowns to state when shipping
- Wall-clock CPU% isn't energy or power, and the host's background load wasn't isolated.
- Only one machine was measured. On hosts with fewer than ~11 logical CPUs the cap does nothing. On larger hosts the saving is likely but unmeasured.
- Nothing here tested startup GPU latency or tearing under AutoNoVsync (you aren't claiming either).

If check 1 shows a p99 regression at 144 FPS, the smallest fix is to apply the cap only when `focused_fps` ≤ 60, rather than going back to opt-in.
