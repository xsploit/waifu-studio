## Verdict

No confirmed per-frame texture-recreation bug in the preview. The largest defensible wins are avoiding repeated main-world recomputation/clones and attributing the ~310 MiB anonymous heap before touching assets. Do not chase the 582 vs 571 MiB delta or compare Linux RSS to Mate Engine's Windows number.

## 1. `avatar_preview.rs` hypothesis — rejected for Bevy 0.19.1

In [avatar_preview.rs](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/avatar_preview.rs:168), `images.get_mut(&target.0)` returns `AssetMut`. In vendored Bevy 0.19.1 `bevy_asset/src/assets.rs:440,627-687`:

- `Deref` (used by `image.width()`/`height()`, `&self` methods) does **not** set `changed`.
- Only `DerefMut` sets `guard.changed = true`; `Drop` then queues `AssetEvent::Modified`.
- `resize()` at [avatar_preview.rs:171](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/avatar_preview.rs:171) is only reached when size differs, so only then does `Modified` fire and the render world re-extract/recreate the GPU texture.

Safe micro-fix (optional, not a leak fix): read with `images.get()` first, call `get_mut` only on mismatch. Real preview cost is the second render pass itself — already gated by `camera.is_active` for `Studio + Characters|Scene` at [avatar_preview.rs:129](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/avatar_preview.rs:129). During window/dock drags, every distinct size queues a realloc; quantising `preview.size` to 8/16 px steps or coalescing resizes retains visual quality and cuts realloc churn. Validate by counting `AssetEvent::Modified` for that handle over 60 s idle (expect 0) vs. during a drag.

## 2. Memory conclusions challenged

- **Linux mapped RSS ≠ Windows private working set.** [memory-mapping-measurement.md](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/reviews/memory-mapping-measurement.md:11) itself notes 161.4 MiB is shared graphics libraries/NVIDIA mappings and 86.3 MiB executable/file mappings. PSS (538–557 MiB) is the closer Linux comparator; neither equals Task Manager Private Working Set, which also responds to `EmptyWorkingSet` trimming in Mate Engine ([mate-engine-efficiency.md:9](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/reviews/mate-engine-efficiency.md:9), off by default) without proving lower committed memory.
- **Empty 581.6 vs loaded 571.4 MiB is not a subtraction.** Different profiles, warm-up, shader compilation, allocator arenas and focus; the loaded run being *lower* proves variance dominates. No avatar-cost figure (neither ~0 nor ~8 MiB) is derivable. Requires same-profile A/B/A unload→reload, ≥5 repeats, median PSS + anonymous + `smaps_rollup`, plus VRAM (`nvidia-smi`/Bevy render diagnostics) reported separately.

## 3. Concrete unnecessary per-frame work (retain all features/FPS)

| Priority | Location | Waste | Safe fix |
|---|---|---|---|
| P1 | [shaders.rs `apply_material_expressions`](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/shaders.rs:454) every `PostUpdate` | `HashSet::new()` + `material.clone().sanitized()` + full `Variants` scan + `ToonValues::target/overlay` per variant, even with empty expression frame. Writes are already value-guarded, so no GPU upload — pure CPU/alloc. | Early-out when `frame.0` empty and tuning unchanged (mirror `apply_material_tuning`'s `Local` guard at [shaders.rs:413](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/shaders.rs:413)); reuse a cleared `Local<HashSet>`. Identical output. |
| P1 | [visuals.rs `sync`/`plan`](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/visuals.rs:303) every `PostUpdate` | `settings.clone().sanitized()` in `sync:349`, again in `plan:304`, again per-camera at `:370`; `plan` recomputed even when `changed == false`. | Sanitize once, pass by reference; skip `plan` when unchanged and baseline present. |
| P2 | [behavior_runtime.rs `poll_behavior`](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/behavior_runtime.rs:178) every `Update` | `scene.affect.clone()` unconditionally to assign into entry. | Only clone/assign when config differs or entry vacant. |
| P2 | [runtime.rs `stage_settings`](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/runtime.rs:629) in chained `Update` | Clones appearance/facial/visual configs into state each call. | Guard with equality/`is_changed` before cloning. |
| P2 | [avatar_preview.rs `update`](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/avatar_preview.rs:141) | Bounds scan uses `parents.iter_ancestors` per mesh — bounded (only while `bounds.is_none()`), so low priority. | Leave; or cache on root change. |
| OK | [shaders.rs `update_outlines`](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/shaders.rs:522), vendored `sync_outline` | Already value-check before `get_mut`; regression test at [shaders.rs:567](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/shaders.rs:567) proves no dirtying. | No change. |
| OK | [runtime.rs `pace_frames`/`frame_cadence`](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/runtime.rs:169) | 60 fps focused / 15 unfocused reactive sleep, no busy-spin. | Keep; do not lower FPS. |

Not found as problems: environment sync is change-gated, outline/morph sync compare-before-write, CPU texture release and `malloc_trim` reclaim already shipped per [runtime-efficiency.md](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/reviews/runtime-efficiency.md:19).

## 4. Measurement plan (before any RAM claim)

1. Same-profile A/B/A: load avatar → unload → reload ×5; report median PSS, anonymous heap, and VRAM separately; discard first (shader-warm) run.
2. Attribute the ~310 MiB anonymous bucket with heaptrack/DHAT or jemalloc profiling on the empty stage — this, not textures (11.2 MiB CPU-resident), decides the next fix.
3. CPU: Tracy/`perf` with `WAIFU_RUNTIME_METRICS=1` at fixed 60 fps, focused, preview on/off, to price `apply_material_expressions`, `visuals::sync`, and the second render pass.
4. Event counters: assert zero `Modified` for preview/material assets during 60 s idle after fixes; screenshot-compare materials/gaze/expressions before/after to prove no visual regression.

Read-only review; no files changed, no builds/tests run.
