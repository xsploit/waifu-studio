# Muse compute policy review

Read-only CLI source/evidence review; no builds or runtime checks by reviewer.

muse: workspace root: /home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio (explicit)
muse: Agent delegation: auto unavailable: workspace is untrusted.
## Verdict

Shipping `compute.max_threads = 4` as the normal default is supported by the supplied evidence for steady-state rendering on this host, conditional on two bounded checks below. The one check that could still falsify it — an uncapped/headroom run — has not been done, and the current p99 data cannot substitute for it. If that check is skipped, keep it opt-in.

## Source facts (inspected)

- Current probe is correctly gated: [task_pool_plugin / compute_thread_probe](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/runtime.rs:38) only overrides `compute.max_threads` when `WAIFU_RUNTIME_METRICS` is present and the value parses in `1..=64`. Normal launches are untouched Bevy policy, wired via [.set(runtime::task_pool_plugin())](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/main.rs:264).
- Bevy 0.19.1 defaults (registry `bevy_app-0.19.1/src/task_pool_plugin.rs`): IO `25% clamp [1,4]`, async-compute `25% clamp [1,4]`, compute `percent 1.0` of remaining, `max usize::MAX`. On 20 logical CPUs that yields IO 4 / async 4 / compute 12 — matching your stated baseline. Setting only `compute.max_threads = 4` leaves `min_threads = 1`, percent, and both other pools untouched, as proposed.
- By construction the cap is inert on small hosts: after IO/async deductions, remaining cores are already ≤4 at ≤8 logical CPUs. Regression risk is concentrated on ≥9-core hosts and parallel bursts. No low-core guarantee problem.
- The app itself has **zero** direct `ComputeTaskPool` / `par_iter` use (only the metrics `thread_num()` read at [runtime.rs:233](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/runtime.rs:233)). All app-side voice/LLM/audio work uses its own `std::thread` spawns, unaffected.

## Exact compute-pool consumers (Bevy 0.19.1 internals)

| Consumer | Relevance here |
|---|---|
| Multi-threaded ECS executor (`bevy_ecs/.../multi_threaded.rs`, enabled by `multi_threaded` in [Cargo.toml:10](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/Cargo.toml:10)) | Main consumer: parallel system execution in every schedule |
| Transform propagation (`bevy_transform/.../systems.rs`) | Parallel hierarchy propagation; scales with 4 avatars × bone hierarchies |
| Pipelined rendering + GPU preprocessing batching (`bevy_render`), mesh preparation (`bevy_pbr/render/mesh.rs`) | Render sub-app and per-frame extraction/prep |
| Query `par_iter` / parallel message iteration (`bevy_ecs`) | Not used by app or vendored VRM/MToon code — no exposure |

Mitigating structure: the main `Update` tuple in [main.rs:332](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/main.rs:332) and the runtime `Update` chain are `.chain()`ed (serial), so little per-frame parallelism exists to lose. Unchained plugin systems (expressions, gaze, spring bones, animation, roster) do parallelize across avatars. Note the executor also runs work on the calling main thread, so effective frame parallelism at cap is main + 4, not 4 alone.

## Evidence assessment (your supplied numbers, not re-run by me)

- CPU: all six paired comparisons decrease. Single-avatar medians 30.66% → 23.41%; four-avatar 40.74% → 26.41%. Consistent direction across alternating launches is the strongest signal.
- The p99 comparison is weak evidence, by construction: `frame_ms` in [log_runtime_metrics](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/runtime.rs:182) is wall-clock between `Last` invocations at a paced 59.8 FPS, so ~17.3 ms in both arms mostly measures the pacing interval, not CPU headroom. It proves the cap doesn't break the 60 FPS budget in these scenes; it cannot detect lost headroom.
- Four-avatar test uses four copies of one model with shared assets (and the shared-morph patch), so distinct-asset mesh prep, extraction, and memory bandwidth are under-tested. Load bursts were excluded by the 28 s settle. Background OS noise is uncontrolled, partially offset by alternation.

## Real risks

1. Saturated-frame regression is unmeasured: heavier scene (room environment, distinct models, playing BVH/VRMA animation, facial/morph load) at an uncapped rate could expose the 12→4 cut that the capped test hides.
2. Startup/avatar-switch bursts (scene spawn, transform propagation, render prep) ride the compute pool; steady-state sampling after settle doesn't cover them. Your creation-to-`VRM_READY` intervals are reassuring but are not full load latency.
3. Implementation-shape risk if the gate is simply removed: a stray `WAIFU_RUNTIME_COMPUTE_THREADS` in a normal launch environment would silently change shipped behavior.

## Smallest required checks before adopting (all offline, existing probes only)

1. **Resolver unit test** alongside the existing probe test: normal (no metrics) → 4 even with the env var set; metrics + valid → override; metrics + invalid → 4 (fall back to the cap, not Bevy default). One metrics-on launch with no compute env var confirming `compute_threads=4` in the existing log line.
2. **One headroom pair**: isolated profile, seeded `desktop-settings.json` with a raised `focused_fps` (loaded from the data dir at [main.rs:208](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/main.rs:208)), `WAIFU_RUNTIME_CONTINUOUS` + `WAIFU_RUNTIME_NO_VSYNC`, four avatars with an `--animation` playing, default vs max-4, compare FPS/p99. Parity here closes the main unknown; a p99 blow-up keeps it opt-in.
3. **One distinct-asset load pair**: 3–4 different bundled models in the roster, sample the *first* 10 s metrics window (load burst, no settle exclusion) plus one avatar switch each arm.
4. **Full suite + strict Clippy after the default change** — the 596 suite predates the probe; the 14 targeted tests cover the probe function, not the shipped default path. Plus one non-metrics smoke launch.

No affinity, presentation, FPS-default, or quality changes are implicated: FIFO default at [runtime.rs:76](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/runtime.rs:76) stays, and `AutoNoVsync` remains metrics-gated. Unknowns I cannot close from source: your CPU sampling method, GPU-side cost, and behavior on other ≥9-core machines — none of which block shipping given `max` (not fixed) semantics and the retained metrics override as an escape hatch.
