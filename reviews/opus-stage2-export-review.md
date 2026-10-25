# Opus 5.5 stage 2 exporter review

Read-only CLI review, 2026-10-20. Served model: `claude-opus-5-5`. Source inspection only; no reviewer tests, GPU or media inspection. Original reviewer findings below; parent disposition is in stage2 progress.

The exporter's geometry, readback and physics ordering hold up. I found one real labeling fault in the exported evidence and two minor robustness gaps. None of them invalidates the four supplied captures, but the "one sim step per sample" claim is wrong as worded.

## Source vs. evidence

- **What I read in source:** `examples/render_desktop_motion.rs` (capture, export and readback paths), `src/spring_physics.rs` (root selection, tuning and restore), `src/spring_physics_controls.rs`, the `src/animation.rs` sampler (`last_sample_time` and the advance step), the vendored spring scheduler and clock in `vendor/crates/bevy_vrm/src/spring_bones.rs`, and `docs/EXTENSION-DESIGN.md`.
- **What I took on trust:** the report and the stated 644 tests / Clippy / build results. I didn't open the PNGs, the pose JSON, the videos or the logs.
- **One claim from Bevy knowledge, not inspected:** a `Screenshot` spawned in `Last` captures that same frame's render.

## Findings

**1. Medium: the step length and "one sim step per sample" are mislabeled**
- **Where:** `/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/examples/render_desktop_motion.rs:833` and `:837`.
- **The fault:** the JSON writes `simulation_step_seconds = 1/fps` and `simulation_frame = state.frames`. But the vendor springs integrate at a fixed 1/60 s (`vendor/crates/bevy_vrm/src/spring_bones.rs:12`, `:26-36`, `:397`).
  - At 30 Hz, each sample is **two** spring steps.
  - The "one simulation step per sample" claim in the report actually counts app frames.
- **The physics itself is time-correct:** 2 × 1/60 = 1/30. So the captures are valid, only described wrongly.
- **Failure case:** with `--sequence 24`, `25` or `50`, the steps per sample alternate (e.g. 2,3,2,3…). Up to 1/60 s of time is left over each sample, so the springs trail the animation. The JSON would still claim one even step.
- **Smallest fix:**
  - Rename the field to app/animation step seconds and add `"spring_step_seconds": 1/60`.
  - Either reject `--continuous` rates where 60/fps isn't a whole number, or export the per-sample spring step count.
  - Correct the report sentence.

**2. Low: the playback clock drifts, and the end check depends on a fixed 1e-5 slack**
- **Where:** `/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/animation.rs:1263` and `examples/render_desktop_motion.rs:934`.
- **The fault:** the f32 `position += dt` drifts. The last exported time is 5.966661, not 5.966667. The exported `seconds` is honest (the position is advanced before sampling, so the label matches the sampled pose).
- **Failure case:** on longer clips or with `--end`, drift can exceed the 1e-5 slack. You then get either one extra sample in `(last_sample_time, duration)`, or a duplicate final pose after the clamp at `animation.rs:1268`. The increasing-time check would catch a duplicate, but not the extra sample.
- **Smallest fix:** in continuous mode, assert `|position − (start + index/fps)| < 0.5/fps` (computed in f64) before writing each sample. Fail if it's off; don't seek.

**3. Low: a PNG can be missing primitives (review media only)**
- **Where:** `examples/render_desktop_motion.rs:904`.
- **The fault:** continuous mode accepts any frame with ≥1024 visible pixels. Bevy silently skips draws whose shader pipelines haven't compiled yet. An early frame could therefore lack hair, clothing or outline passes and still pass, while its pose JSON is complete. Only the single-shot path retries, and only on a fully empty image.
- **Smallest fix:** before setting `continuous_started`, require one successful probe through the existing `save_capture` retry path, or two consecutive probes with equal pixel coverage.

**4. Info: `physics_tuned` counts intended tuning, not applied settings**
- **Where:** `examples/render_desktop_motion.rs:835`.
- **Detail:** it reads `info.tuned`, which is what the config asks for. This is currently sound, because `update_springs` writes the settings in PostUpdate before the transform pass and the vendor simulation runs after it (`src/spring_physics.rs:147-152`, `spring_bones.rs:247-264`).
- **Optional:** export each tuned joint's applied stiffness and drag so the audit can prove it rather than infer it.

## Checked and correct
- **Pose ordering:** animation advances, then samples (Update) → tuning → vendor simulation → second transform pass (PostUpdate) → export (`Last`). The exported skin matrices are the final render pose, including spring helpers.
- **Morph weights:** `Reference` and `Value` are resolved correctly, and missing or nonfinite weights fail.
- **Outlines:** shader outline passes are excluded.
- **Readback:** the exporter waits until every screenshot callback has fired, and failure is checked before success (`:957-972`).
- **GPU timing:** readback never pauses or seeks the animator.
- **Spring ownership:**
  - Only one SpringBones owner under the active root is tuned. If there are several, authored values are kept.
  - The old root is restored on avatar switch.
  - Settings never stack up across updates, thanks to the `last_applied` check.
- **Strong jiggle is kept:** at strength 1, stiffness ×0.2 and drag ×0.15 (`src/spring_physics.rs:292-298`).

## `docs/EXTENSION-DESIGN.md`
One thing to fix now. Line 52 ties a single `generation` to the "target character/playback lifetime". If that counter bumps on each speech or motion playback, then every `motion.play` or `expression.apply` aimed at generation 17 goes stale the moment speech starts. Character switches and playback churn would become indistinguishable.

- **Fix:** define `generation` as the character-selection/avatar-instance generation only. Carry playback identity in `speech_id` and the motion request ID, which the doc already has.
- **Related, line 93:** require `message.deliver` to state explicitly whether it binds to `character_id`/`generation`, or else route to the character selected at delivery time.

The rest of the lifecycle and cancellation boundaries look right for a draft: accepted ≠ played, stale-target errors, a separate instance generation, and a bounded shutdown.
