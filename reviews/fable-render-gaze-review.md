# Fable 5.1 review: linear RGB correction and pointer gaze

Read-only review, 2026-10-18. Scope: `src/color_correction.rs`, `src/color_correction.wgsl`, the VisualPlugin wiring in commit 6002c64, and the pointer-targeting gaze change (`src/gaze.rs`, `src/gaze_controls.rs`), which was committed as 9cdfdf6 while this review ran. No cargo invocation; Bevy behavior was verified against the `bevy_render-0.19.1`, `bevy_core_pipeline-0.19.1`, `bevy_post_process-0.19.1` and `bevy_camera-0.19.1` sources in the local registry.

## Findings

### 1. Turning RGB correction off leaves the pass running (bug, high)

`apply` selects views with `ViewQuery<(&ViewTarget, &DynamicUniformIndex<ColorCorrection>)>` (`src/color_correction.rs:100`). When `sync` removes `ColorCorrection` from the camera (`src/visuals.rs`, the `camera.remove::<ColorCorrection>()` branches), Bevy's `SyncComponentPlugin` hook only removes `ColorCorrection` itself from the render-world entity (`bevy_render-0.19.1/src/sync_component.rs:54-62`). `prepare_uniform_components` only inserts `DynamicUniformIndex<C>` for entities that still have `C` and never removes it (`bevy_render-0.19.1/src/uniform.rs:86-116`). The stale index therefore stays on the render view, the `ViewQuery` keeps matching, the target is still `Rgba16Float` while effects remain enabled, and `ComponentUniforms::binding()` keeps returning the previous GPU buffer (`DynamicUniformBuffer::get_writer` with zero items leaves the old buffer in place, `uniform_buffer.rs:275-311`). Result: unchecking "WebWaifu4 RGB color correction" while postprocessing stays on continues applying the last power/gain values until HDR is dropped (effects off or desktop mode) or the app restarts. The unit test in `visuals.rs` only checks main-world component presence, so it cannot catch this.

Fix: gate the pass on the extracted component, which is removed correctly. Either `ViewQuery<(&ViewTarget, &DynamicUniformIndex<ColorCorrection>), With<ColorCorrection>>` or include `&ColorCorrection` in the tuple. Both are one-line changes and need no new state.

### 2. Correction order relative to bloom is unspecified (medium)

`apply` is scheduled `in_set(Core3dSystems::PostProcess).before(tonemapping)` (`src/color_correction.rs:46`). Bevy's bloom is scheduled identically (`bevy_post_process-0.19.1/src/bloom/mod.rs:79`). There is no edge between them, so whether the power/gain curve is applied to the bloom contribution depends on schedule build order, and it can differ between the offscreen capture and the live app. Pin it explicitly; `bevy::post_process::bloom::bloom` is a `pub fn`, so `.after(bevy::post_process::bloom::bloom)` (correction on top of bloom) or `.before(...)` (bloom on corrected colour) both work. Pick whichever matches the reference and record it in the module doc.

### 3. Pointer gaze can map against the offscreen preview camera (bug, medium)

`apply_gaze` resolves the pointer with `cameras.iter().filter(is_active).find_map(|camera| pointer_target(cursor, camera.logical_viewport_rect()?))` over every `VisualCamera` (`src/gaze.rs:332`, `src/gaze.rs:397-406`). The avatar preview camera also carries `VisualCamera`, renders to a 600x900 image, and is active in Studio mode on the Characters and Scene tabs (`src/avatar_preview.rs:86-95`, `src/avatar_preview.rs:129-130`). For an image target `logical_viewport_rect()` is `(0,0)-(600,900)` in image pixels, which is unrelated to window cursor space. Query iteration order is unspecified, so while the cursor is inside the top-left 600x900 of the primary window the normalized target may come from the preview rect instead of the studio camera, and the mapping jumps when the cursor leaves that region. The "returns to ambient outside the avatar view" claim also does not hold for that camera.

Fix: only consider cameras whose render target is the primary window. Query `(&Camera, &RenderTarget)` and accept `RenderTarget::Window(WindowRef::Primary)` or `WindowRef::Entity(primary_entity)`; or add a dedicated marker to the studio camera instead of reusing `VisualCamera`. Add a test with two cameras to lock it.

### 4. "Avatar viewport" is the whole primary window (low, documentation/UX)

No `Camera::viewport` is ever set on the studio camera, so the viewport rect is the full primary window including the egui control panel. Hovering sliders drives the head toward the panel. If that is acceptable, soften the UI text in `gaze_controls.rs` ("Follow pointer in avatar view") and the module doc; otherwise exclude the egui-occupied rect (bevy_egui exposes whether the pointer is over egui) before calling `pointer_target`.

## Checked, no findings

- HDR gating: `ColorCorrection` is only inserted alongside `Hdr`, `apply` returns before `post_process_write()` when the format is not `Rgba16Float` or the pipeline is not compiled, so no texture flip is leaked. The hardcoded format matches `ViewTarget::TEXTURE_FORMAT_HDR`; using the constant would be a nit.
- Uniform extraction/camera lifecycle otherwise: `UniformComponentPlugin` prepare runs in `RenderSystems::Prepare` before `Core3d`; per-frame bind-group rebuild is correct; a late-spawned `VisualCamera` picks up the current plan through the missing-baseline path; despawn drops the component with the entity.
- Alpha and suppression: the shader writes `sampled.a` unchanged, `max(rgb, 0)` prevents NaN from negative bases, gain 0 gives black but keeps alpha, and `sync` removes the component when `suppressed` (desktop transparent mode, `src/runtime.rs:547`) before `Hdr` is removed. Sanitization bounds NaN/inf and ranges, and old settings deserialize with correction off.
- Invalid pointer handling: `pointer_target` rejects non-finite cursor/viewport, zero-size viewports and out-of-rect points; `cursor_position()` is `None` when the cursor leaves the window; `Motion::sample` re-filters non-finite values and clamps to [-1, 1]; `dt` is clamped. The direction test confirms right/up on screen map to the character's screen-right/up given the `left/up/forward` basis.
- Rest-space composition: pointer mode only changes the normalized offset; `raw_overlay` still conjugates through the full rest chain and anatomical basis, conditional removal still restores only gaze-authored rotations, and the existing multi-frame and avatar-switch tests still cover accumulation and restoration. Pointer target smoothing reuses the same lerp so switching between ambient and pointer is continuous.

## Summary

Two real bugs: disabling RGB correction does not stop the pass while HDR effects stay on, and pointer gaze can read the wrong camera's viewport when the avatar preview is active. One determinism gap (bloom vs correction order) and one wording/UX nit. No source changes were made.
