# Feature-preserving efficiency follow-up


## Final verified result

The release now shares complete immutable CPU morph backing and normally caps only Bevy's compute pool maximum at four. I/O/async pools, CPU affinity, frame-cap settings, rendering quality, spring simulation, speech and animation features remain unchanged. Smaller machines still allocate fewer workers. Metrics-only overrides remain available for reproducible comparisons; ordinary launches ignore stray override values.

- Matched pre/post morph runs: **83.92 MiB lower RSS and PSS**, at matching actual 20 FPS. This is the measured morph saving, not a 60 FPS comparison.
- In the shipped FIFO mode at matching actual 20 FPS, compute-only median CPU fell **10.91% → 8.75%** (19.8% relative), with no presentation-policy change.
- At actual **59.8 FPS**, compute-only A/B median CPU fell **30.66% → 23.41%** for one avatar and **40.74% → 26.41%** for four instances (one core = 100%).
- Four **distinct** VRMs, room and body animation at requested 144: both policies delivered **142.8–142.9 FPS**, median CPU **137.60% → 96.62%**, without a material frame-tail regression in these samples.
- Final source: **597 all-target offline tests**, strict Clippy, touched-source formatting and whitespace checks passed; release rebuild succeeded.
- Two post-change switch captures: **12 selections / 345 frames each**, nonzero recorded audio through the meter on every shot; matching per-model mesh/image/morph counts across three cycles and both worker policies. Late-cycle Riko and Alicia frames were visually inspected.
- A non-metrics launch with a stray `WAIFU_RUNTIME_COMPUTE_THREADS=12` still created **four** compute workers. The normal app was restored, verified alive, with four workers.

Opus 5.5 and Meta Muse Wonder 1.0 independently reviewed source, implementation and the proposed worker policy. Their bounded headroom/load checks were completed; see the individual review files. Physical pointer-hover interaction, live providers, speakers/perceived lip sync, Windows measurements, power consumption and a long-duration RSS soak were not accepted by these checks. No Windows/Mate 200 MB parity or GPU-memory saving is claimed.

The existing native FIFO presentation issue remains separate: the isolated fixture delivered 20 FPS with FIFO, including a cap-144 probe. Actual-60/144 comparisons use the metrics-gated AutoNoVsync diagnostic; no presentation default was changed and tearing was not evaluated. Sections below retain chronological checkpoints; old 'pending' statements are superseded by this final result and later receipts.

2026-10-19. Opus 5.5 and Meta Muse Wonder 1.0 reviewed the current source independently. Muse ran through the installed Meta CLI (`muse-wonder-1.0`); Opus ran through the Claude agent integration. Neither reviewer built or ran the app.

## Applied source changes

- Camera effects now avoid cloning/sanitizing settings and building render plans on unchanged frames. Late cameras still initialize, suppression and disabling still restore original state, and sanitized comparison prevents non-finite raw settings from forcing identical component writes. The regression covers steady frames, late cameras and NaN settings.
- The emotion/behavior bridge copies affect configuration only when it changes or the character has no entry. It still handles pending affect and ticks behavior every frame.
- No feature, shader quality, frame cap, animation cadence or speech path was reduced. The later measured worker-policy change bounds only the compute maximum, as described above.

Opus found no blocking defect in the camera optimization, identified the NaN robustness issue (addressed), and cautioned against an empty-material-frame shortcut: frames normally contain the active avatar even at zero weights, and restoration requires a final update. That shortcut was not implemented. The preview render-texture reupload suspicion was independently rejected by both parent and Muse: Bevy AssetMut only notifies on mutable dereference, not immutable size checks.

## Allocator experiment — not a shipped default

Existing release binary, normal Bevy pools, same saved profile. Alternating fresh launches, 18 s settling followed by 8 s sample. Current source edits were not in that binary. No compilation ran during samples.

| Arena setting | RSS MiB | PSS MiB | CPU, one core = 100% | Last logged cadence |
|---|---:|---:|---:|---|
| Default, run 1 | 661.0 | 531.7 | 40.35% | 59.8 FPS, focused |
| 2, run 1 | 620.1 | 490.8 | 41.35% | 59.8 FPS, focused |
| Default, run 2 | 656.3 | 527.0 | 12.12% | 15.0 FPS, unfocused |
| 2, run 2 | 612.9 | 485.1 | 37.35% | 59.8 FPS, focused |

Both arena-limited samples used approximately 40 MiB less RSS/PSS than the preceding default launch. This supports further testing, not a universal savings or CPU claim. The second default run lost focus and cannot support CPU comparison. One matched pair differs by +1 percentage point CPU; another arena-limited sample is lower. Samples lack frame-time percentiles and active-workload contention measurements. No default allocator setting was changed. The running experimental app retains the per-process arena cap until restart; saved settings are unaffected.

## Remaining work

### CPU morph backing investigation

Heaptrack's loaded-avatar capture attributes 88,080,384 live requested bytes to morph vectors. Opus independently parsed Hikari: seven face material primitives reference the same 57 target accessors over 4,296 vertices. Each expanded vector has capacity 262,144 elements of 48 bytes, exactly accounting for that allocation. Body and hair have no morph targets. This is retained data, not proof of a leak.

The implementation under review keeps complete immutable morph data and shares its CPU backing between clones and identical meshes through a local Bevy mesh patch. Existing CPU positions, indices, morph getters, rendering, late outline creation and first-person derivation must remain available. No GPU savings are claimed. An alternative that strips CPU morph data after upload was rejected because renderer device re-extraction can bypass ordinary mesh asset events, and future CPU readers would observe missing data. Estimated savings of roughly 75 MB remain a prediction until the patch builds and equivalent runtime measurements confirm them.

Implementation checkpoint: the patched Bevy mesh and downstream app compile, and four offline morph regressions pass. They cover shared clone backing and replacement isolation; exact-byte deduplication and weak-cache release; extraction errors and whole-Mesh reflected cloning; real asset-event deduplication without Modified events and later mesh replacement. Full-suite/Clippy and release-runtime checks remain pending at this checkpoint. The private morph field's reflection representation changes from Vec to opaque Arc; the patch notes record this explicitly. Public Vec/slice getters and setters retain their signatures; the app's SerializedMesh path already excludes morphs upstream.

Correction discovered during Heaptrack capture: the earlier fresh-profile sample automatically loaded bundled Hikari, so the supposed empty-stage baseline is invalid. Its engine-floor inference is withdrawn. The allocator alternating runs used the normal profile throughout and are unaffected. A new no-avatar capture explicitly overrides the bundle path to a nonexistent work-only file and uses a new profile directory.

Attribute the empty-stage anonymous/heap baseline with allocation profiling, measure frame-time distributions at fixed focus/cadence, repeat arena trials and check load/speech contention before selecting defaults. Test repeated avatar/motion switches for stable resource counts. The current measurements do not prove feature-wide runtime acceptance, a 200 MiB target, or parity with Mate's Windows working-set counter.

The raw Muse review is retained separately; its stage-settings clone item was already guarded in source, and its affect clone item has now been fixed. Its material empty-frame suggestion must be read with Opus's restoration caveat above.

## Fixed-cadence ten-run allocator follow-up

Five alternating launches per setting, 28 s settle + 12 s sample, isolated profile fixed to 60 FPS, same avatar and release binary. All runs logged 59.8 FPS, mixed_focus=false; one arena-2 run was unfocused (still fixed 60 FPS). No compilation or concurrent profiling ran. Normal app restored after completion.

| Arenas | Median RSS MiB | Median PSS MiB | Median CPU, one core = 100% | Median last-window p99 ms |
|---|---:|---:|---:|---:|
| arenas-default | 665.5 | 536.5 | 38.98% | 17.73 |
| arenas-2 | 626.5 | 497.5 | 38.57% | 18.77 |

The cap reduced median RSS/PSS by about 39 MiB. These short steady-state samples do not establish contention during avatar imports, chat or speech; background desktop workloads were not isolated. No allocator default is changed. Raw results: `work/allocator-controlled-results.json` in the project working directory.

## Shared-morph release comparison (completed)

Six alternating fresh launches, three per binary, same isolated profile and selected `b70818a59cb3-model.vrm`, 28 s settle + 12 s sample. No compilation or other profiling ran. Settings requested 60 FPS, but **both builds actually delivered 20 FPS**; these results must not be represented as a 60 FPS test. Each run reported 34 images, 20 meshes, 11.2 MiB CPU image data and no mixed focus. Normal app was restored to the new release.

| Build | Median RSS MiB | Median PSS MiB | Median CPU, one core = 100% | Median last-window p99 ms |
|---|---:|---:|---:|---:|
| baseline | 655.08 | 527.17 | 12.33% | 51.58 |
| shared-morph | 571.16 | 443.26 | 11.58% | 52.54 |

Measured reduction: **83.92 MiB RSS / 83.92 MiB PSS** by difference of medians (about 12.8% RSS, 15.9% PSS). CPU was slightly lower, but this small short-sample difference is not a general CPU-improvement claim. Individual windows had slow-frame outliers in both builds; no broad p99 improvement is established.

Every new-build run measured **96 MiB logical CPU morph capacity / 12 MiB unique backing**. Direct parsing confirms eight primitives sharing the same 57 target accessors in this selected model. The earlier 7-primitive/88.08 MB Heaptrack analysis was a different Hikari file (`9bcc6aedeb4b-hikkyc2.vrm`), not an arithmetic discrepancy. No VRAM reduction is claimed.

Release SHA-256 baseline: `df807118d0bc7794359babda75256ccb4edef5bbae1f45c9e84afe8469c007d9`; shared-morph release: `0687bc9ad772c2bba3e8ff3742b23791eda12e5a83719c1987f13883e03f2ee8`. Raw results: project `work/morph-controlled-results.json`.

Render smoke: isolated windowless showcase exited successfully with 60 captured frames, Toon outlines, the bundled idle-maid body motion and existing recorded audio decoded through the meter/mouth pipeline without speakers. A captured frame was inspected and the avatar/face/clothing/room render. A missing `rooms/wave-art.png` lookup was logged, so this is not a fully clean room-asset check or personal motion/lip-sync acceptance. No live provider calls were made.

Next: investigate the reactive timer / actual FPS mismatch and measure at actual 60 FPS, as requested by the user. An opt-in diagnostic continuous event-loop switch, gated by runtime metrics, retains the existing per-frame sleep cap and leaves ordinary defaults unchanged. Its new runtime tests/check/rebuild are pending.

## Actual-FPS diagnostic follow-up

Four alternating launches of the same shared-morph release, all profile caps set to 60: normal reactive mode versus an opt-in continuous event-loop probe retaining `pace_frames`. Both modes still delivered about 20 FPS. This does **not** support the reactive-timer hypothesis. No timer default was changed. KScreen independently reports the active display at 3840×2160, 60 Hz.

| Event loop | Median RSS MiB | Median PSS MiB | Median CPU, one core = 100% |
|---|---:|---:|---:|
| reactive-default | 570.76 | 442.90 | 10.91% |
| continuous-probe | 572.27 | 444.36 | 11.54% |

Raw results: project `work/cadence-controlled-results.json`. Next hypothesis is presentation pacing; an opt-in no-VSync diagnostic and explicit configured-loop/presentation/cap logging are being checked. These are test-only process overrides, not shipped default changes.

Render-fixture correction: the missing room-art lookup came from placing the nonexistent bundle override in `work/`; environment artwork is resolved relative to that manifest location. A nonexistent manifest under the real `assets/characters/` directory disables seeding while retaining the correct room-art root. The previous warning is not evidence of a production asset regression. Clean rerender pending.


## Native presentation probe (2026-10-19)

Same release, isolated profile, continuous loop, requested cap 60, 28 s settling and 12 s sampling. No compilation ran during either measurement. Normal app restored after the experiment.

| Presentation | Actual FPS | RSS MiB | PSS MiB | CPU, one core = 100% | Logged p99 ms |
|---|---:|---:|---:|---:|---:|
| Native FIFO | 20.0 | 595.44 | 467.58 | 10.83% | 51.28 / 98.78 |
| Native AutoNoVsync | 59.8 | 576.37 | 447.96 | 30.41% | 17.91 / 17.33 |

This demonstrates actual 60 FPS with shared morph backing, unchanged visual settings and 96 MiB logical / 12 MiB unique morph backing. It is one sample per presentation mode, not a matched baseline-versus-patch benchmark at 60 FPS, nor evidence of a general CPU improvement. Disabling VSync remains an opt-in diagnostic, not a default change: tearing and platform behavior require evaluation.

The X11 probe failed before measurement: its surface advertised only Opaque alpha while the app requested PreMultiplied. No X11 performance result exists. This is separate from the successful native Wayland sample. Raw results: project `work/presentation-controlled-results.json`; diagnostic logs retain presentation mode, cap and actual cadence.


Final refresh checks: all-target strict Clippy, touched-source rustfmt and `git diff --check` passed after the 596-test run. Corrected offscreen showcase completed 60 frames at its configured capture cadence of 30 FPS; frame 30 was visually inspected. Avatar, clothing, face, outlines and furnished room render, with bundled body motion and recorded audio routed through mouth/meter processing. No missing room-art, rendering error or panic was logged. One Bevy startup warning reported missing render-world time; no live-provider, speaker or perceived lip-sync acceptance is implied. The normal app was restored. Capture cadence is separate from the native 59.8 FPS resource sample.


## Repeated actual-60 FPS measurements

Three fresh launches with the normal reactive event loop and opt-in AutoNoVsync, same isolated avatar/profile and unchanged quality/features. Each settled 28 s then sampled 12 s. All six logged cadence windows reported 59.8 FPS, focused, with 34 images / 20 meshes / 11.2 MiB CPU images and 96 MiB logical / 12 MiB unique morph backing. This is repeated short-run evidence, not a long-duration soak or a baseline-versus-patch comparison at 60 FPS.

| Run | RSS MiB | PSS MiB | CPU, one core = 100% | Last logged p99 ms |
|---|---:|---:|---:|---:|
| 1 | 581.91 | 453.49 | 39.74% | 17.86 |
| 2 | 580.96 | 453.10 | 41.82% | 17.45 |
| 3 | 570.18 | 441.86 | 30.57% | 17.30 |

CPU varies substantially (30.57–41.82%); the earlier single 30.41% sample must not be treated as a stable CPU cost. Read-only per-thread counter sampling ran alongside the process counters: most CPU time is in the Compute Task Pool; async and I/O pools are effectively idle. Names alone do not identify which systems consume compute time. Do not lower task-pool size from this evidence. No builds or intrusive profiling ran. Normal app restored. Raw data: project `work/sixty-controlled-results.json`, `work/sixty-threads-{0,1,2}.json`.


## Reviewer-directed FIFO cap probe

Opus 5.5 and Muse Wonder 1.0 completed new read-only CPU/presentation reviews (`reviews/opus-efficiency-next.md`, `reviews/muse-efficiency-next.md`). Both recommend attribution before spring/transform changes. Their cache/subtree suggestions are hypotheses requiring correctness checks; dynamic simulated ancestors make naive caching unsafe.

One fresh native FIFO launch used a copied isolated profile with focused/background/overlay caps 144; normal reactive loop, 28 s settling, 12 s sample. Actual cadence remained 20.0 FPS in both windows (p50 49.98/50.01 ms; p99 52.52/51.40 ms). The simple cap-increase probe did not restore 60 FPS. It does not identify the underlying compositor/driver cause. User profile and presentation defaults unchanged; normal app restored. Raw data: project `work/fifo144-controlled-results.json`.


## First function-level CPU attribution

Local unpacked Arch perf and dependencies in project `work/perf-local` (no system installation or permission changes). Existing release, isolated profile, reactive AutoNoVsync; 28 s settling followed by a 15 s user-space `cpu-clock:u` sample at 199 Hz. Frame-pointer call chains were requested; optimized release stacks may be incomplete, so inclusive call-chain percentages are not relied upon. No compilation or resource benchmark ran alongside it. Normal app restored.

782 samples, zero lost. Leaf samples: 68.03% compute workers, 31.59% main; 78.13% app executable, 13.94% libc, 7.54% NVIDIA library. Named leaf entries include concurrent queue pop 2.81%, transform propagation future poll 2.17%, contended mutex 2.17%, spring `compose_global` 1.79% compute + 0.51% main (2.30% combined), skin extraction 1.02%, ECS executor tick 1.15% compute + 0.77% main. Many remaining entries are below 0.5%; this is not precise per-system attribution or proof of one dominant scheduler hotspot. Kernel CPU is excluded.

The app logged 59.8 FPS during the sample. Spring ancestry alone is not supported as a large whole-process win by this short capture; source-level caching would also need invalidation for simulated ancestors. Next bounded experiment: matched alternating all-CPU/eight-CPU affinity launches, preserving cadence, avatar and visual settings. No default thread-pool or affinity change is justified yet. Raw reports: project `work/cpu-attribution-{report,dso,comm}.txt`; profiler output `work/cpu-attribution-perf.log`.


## Six-launch affinity experiment (not a shipped setting)

Three alternating unrestricted/eight-logical-CPU pairs, same isolated avatar/profile, native reactive AutoNoVsync, 28 s settle + 12 s sample. All launches logged focused 59.8 FPS, matching image/mesh/morph counts. No compilation or intrusive profiling ran during these samples; read-only thread counters ran concurrently. Normal app restored afterward.

| Affinity | Median RSS MiB | Median PSS MiB | Median CPU, one core = 100% | CPU range | Median last-window p99 ms |
|---|---:|---:|---:|---:|---:|
| all-cpus | 576.64 | 448.29 | 30.16% | 29.99–34.49% | 18.07 |
| eight-cpus | 555.81 | 427.95 | 22.24% | 21.91–22.49% | 17.14 |

Host process affinity exposes 20 logical CPUs normally; eight-CPU trial process thread counts were 29–30 versus 41 normally. The median CPU difference is 7.92 percentage points (26.3% relative). These short samples support a further worker-count experiment, not a source-default change: affinity also changes I/O/async pool allocation, driver scheduling and CPU placement. Some earlier-window p99s exceeded 20 ms in the constrained trials, so no latency-improvement claim is made. Import, multi-avatar and speech contention remain untested. Raw data: project `work/affinity60-controlled-results.json` and `work/affinity60-threads-{0..5}.json`.

Next probe adds metrics-gated `WAIFU_RUNTIME_COMPUTE_THREADS=1..64`, limiting only Bevy's compute pool maximum and logging actual worker count. I/O/async assignment, CPU affinity and normal launch defaults stay unchanged. Latest source for this probe is undergoing fresh checks; the prior 596-test receipt predates it.


## Compute-worker-only comparison

The new metrics-gated probe limits only the compute pool; normal affinity, I/O and async pool policies remain unchanged. Three alternating default/four-worker pairs used the same release and isolated single-avatar profile, reactive AutoNoVsync at actual 59.8 FPS throughout. 28 s settling + 12 s sampling, no concurrent builds or intrusive profiling; normal app restored. Actual worker counts logged as 12 versus 4; process threads 41 versus 33.

| Compute workers | Median RSS MiB | Median PSS MiB | Median CPU, one core = 100% | CPU range | Median last-window p99 ms |
|---|---:|---:|---:|---:|---:|
| compute-default | 580.01 | 451.70 | 30.66% | 29.66–37.49% | 17.26 |
| compute-four | 568.04 | 439.69 | 23.41% | 23.08–25.57% | 17.31 |

Median CPU decreases 7.25 percentage points (23.6% relative); all three immediately paired comparisons decrease. The approximately 12 MiB RAM difference is smaller than the separately measured morph win and remains sensitive to allocator variation. Frame p99 medians are effectively unchanged at this sample size. No worker-policy default has changed. A maximum four-avatar fixture is the next acceptance check; active speech/import contention is not yet established. Raw data: project `work/compute60-controlled-results.json` and `work/compute60-threads-{0..5}.json`.

Fresh diagnostic checks: 14 targeted runtime/behavior tests passed; strict all-target Clippy clean; release build completed in 42.76 s. Full-suite 596 receipt predates the newly added probe regression; no 597-test claim is made yet.


## Four-avatar compute-worker comparison

Same six-launch design, with the selected avatar plus three persisted scene members, all using the same bundled model. Each run required exactly four VRM_READY events before sampling. This exercises four live rigs/spring sets; shared underlying assets make it different from loading four distinct large models. Every run logged 59.8 FPS and matching image/mesh/morph counts. Normal app restored afterward.

| Compute workers | Median RSS MiB | Median PSS MiB | Median CPU, one core = 100% | CPU range | Median last-window p99 ms |
|---|---:|---:|---:|---:|---:|
| compute-default | 585.75 | 457.35 | 40.74% | 39.15–44.90% | 17.50 |
| compute-four | 565.27 | 437.43 | 26.41% | 26.24–28.82% | 17.56 |

Median CPU reduction 14.33 percentage points (35.2% relative), with lower CPU in every paired comparison. Last-window p99 medians differ by 0.06 ms, below useful inference at this sample size. All-four readiness occurred 176–214 ms after the window-creation log in the runs; this is a loader-state interval, not full startup/GPU readiness or a general import benchmark. No normal-policy change has been made at this checkpoint. Opus and Muse are independently reviewing the evidence and proposed max-four policy before adoption. Raw data: project `work/compute60-multi-controlled-results.json` and associated logs/thread counters.


## Headroom and distinct-asset load comparison

Three alternating 12/four-worker pairs, same release, continuous loop and AutoNoVsync; isolated requested cap 144. Four distinct bundled VRMs (selected Hikari plus Neuro, Riko and AliciaSolid), furnished RoseBedroom with the CLI visual preset and a bundled idle-maid body animation. Each launch required all four VRM_READY events before the 28 s settle + 12 s sample. Startup first-ten-second metrics were retained separately. No concurrent build or profiler; normal app restored.

| Compute workers | Median RSS MiB | Median PSS MiB | Median CPU, one core = 100% | Last-window FPS range | Median last-window p99 ms |
|---|---:|---:|---:|---:|---:|
| compute-default | 748.62 | 609.75 | 137.60% | 142.8–142.9 | 7.67 |
| compute-four | 716.38 | 575.43 | 96.62% | 142.9–142.9 | 7.62 |

Median CPU decreases 40.98 percentage points (29.8% relative) at matching delivered cadence. First-ten-second FPS averages: default 138.6/139.6/138.8, four 138.9/139.1/138.8. Startup-window p99s: default 8.76/8.32/9.55 ms, four 8.02/8.51/7.84 ms. This is bounded load/headroom evidence, not a true uncapped maximum or physical-display tearing test. Steady counts match: 113 images, 129 meshes, 11.2 MiB CPU image data, 282.99 MiB logical / 42.99 MiB unique morph backing. No peak-RAM improvement is claimed. Raw data: project `work/compute144-controlled-results.json`.

Following the conditional Opus/Muse reviews and these results, source now stages a normal compute maximum of four, preserving Bevy's minimum/percentage policies, I/O and async pools, affinity, frame caps and appearance settings. The metrics-only override can raise or lower the experimental maximum; normal launches ignore stray override values. Fresh full checks and post-change launch/switch captures are required before treating this staged policy as verified.

The prior four-worker windowless capture completed 60 frames with four VRM_READY events, body animation and recorded audio/mouth processing, no warnings or errors. A representative frame was inspected; the Characters preview shows the selected avatar and room. This does not establish that all four avatars are simultaneously visible in that preview or perceived live lip-sync quality.


## Post-change switch and normal-policy acceptance

The windowless plan now optionally selects an existing profile character through `Studio::select_character`, waits for the requested model rather than accepting the still-visible old avatar's Ready state, and logs per-shot asset ownership counts. Existing plans remain compatible. Narration starts after selection cancellation and load settling; a discovered test-harness stale-generation issue was corrected before audio acceptance. Production voice cancellation code was not changed.

Final captures used four distinct models in a single slot, three cycles, with body motion, furnished room and existing recorded narration. Both 12-worker and four-worker arms exited successfully: 12 shots / 345 JPEGs each, no warnings/errors, per-shot peak audio 0.3083 (last longer hold 0.5269). Every revisit returned to the same counts, and both policies matched:

| Model | Meshes | Images | Logical / unique CPU morph MiB |
|---|---:|---:|---:|
| Hikari | 25 | 35 | 96 / 12 |
| Neuro | 46 | 49 | 84 / 12 |
| Riko | 41 | 51 | 96 / 12 |
| AliciaSolid | 44 | 21 | 6.99 / 6.99 |

These are capture-fixture counts (including its render targets), not native-app asset budgets or proof of driver allocation release. The longer final shot gives the existing deferred reclaim time to run, but no steady RSS inference is made from this capture. Source counters demonstrate bounded ownership over the tested cycles; a months-long leak guarantee is not implied. Raw verifier receipt: project `work/switch-capture-audio-results.json`; corresponding plans, frames and logs remain in project work.

Final full suite after the harness correction: 483 library + 103 binary + 6 integration + 2 + 3 example tests = **597**, all passed serially; strict all-target Clippy passed; release rebuild completed in 42.93 s. No build ran during resource measurements. Normal-policy check launched without metrics/presentation probes and deliberately supplied the compute override 12: `/proc` thread names confirmed four compute workers. Normal app was restored and confirmed alive with four workers.


## Final shipped-presentation comparison

Six alternating 12/four-worker launches of the final release, native FIFO and normal reactive loop, same single-avatar fixture, requested cap 60. Each settled 28 s then sampled 12 s. Both policies actually delivered 20.0 FPS throughout, matching asset and morph counts. This directly tests the shipped presentation mode; it does not repair the separate FIFO timing issue.

| Compute workers | Median RSS MiB | Median PSS MiB | Median CPU, one core = 100% | CPU range | Median last-window p99 ms |
|---|---:|---:|---:|---:|---:|
| compute-twelve | 569.23 | 441.21 | 10.91% | 10.91–11.25% | 51.87 |
| compute-four | 566.19 | 438.29 | 8.75% | 8.66–10.16% | 52.01 |

Median CPU decreases 2.16 percentage points (19.8% relative), with lower CPU in all three paired comparisons. RAM differences here are small and inconsistent across pairs; no additional universal RAM saving is attributed to worker count. Frame-tail outliers occur, including a 98 ms window in the first 12-worker launch; no broad p99 improvement is claimed. No build, profiler or model reviewer ran during these samples. Normal app restored. Raw data: project `work/compute-fifo-controlled-results.json` and corresponding logs/thread counters.

The performance changes and review documents remain local, uncommitted worktree changes. The test/runtime receipts apply to that worktree rather than a new immutable Git commit.
