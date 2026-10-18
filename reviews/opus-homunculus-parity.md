# Desktop Homunculus parity review (Opus 5.5, 2026-10-18)

Scope: read-only comparison of the current Waifu Studio desktop companion against
upstream `not-elm/desktop-homunculus` at the local reference checkout
(`work/desktop-homunculus-reference`, HEAD `30d11a5a`, 2026-05-04) and
`work/bevy_vrm1`. Reference `CLAUDE.md` / `engine/AGENTS.md` were read first.
No Cargo, app edits, desktop control, compositor operations or network calls were
made. Line numbers are from the files as read during this pass; root is editing
concurrently, so re-check before patching.

Evidence tiers used below:

- **S** – source reading only (this pass).
- **F** – fake/offline tests exist (Rust unit/ECS fixtures, `tests/kwin_surfaces.cjs`).
- **H** – headless render exists (`previews/desktop-*.png`, windowless Vulkan).
- **D** – actual desktop/compositor/hardware acceptance. **No item has D evidence.**
  `VALIDATION.md:73,132,149` and `FEATURE-STATUS.md:13,46` state this explicitly.

## 1. Architecture difference that frames everything

Upstream renders into one **fullscreen, transparent, always-on-top window per
monitor** (`engine/crates/homunculus_windows/src/lib.rs:115-160,243-262`) and moves
the VRM *inside* it. Drag, sitting and hit-testing are all done in that overlay's
world space. External windows come from `CGWindowList` / `EnumWindows`
(`homunculus_screen/src/global_windows/{macos,windows}.rs`). On Linux,
`GlobalWindows::find_all()` returns `None` (`homunculus_screen/src/global_windows.rs:85-90`):
**upstream has no Linux window sitting at all.**

Waifu Studio uses a **small 420×640 companion window** (`src/runtime.rs:283-286`)
that the **compositor** moves (`window.start_drag_move()`,
`src/desktop_behavior.rs:278`). Global geometry comes only from an opt-in temporary
KWin script (`assets/desktop/kwin-surfaces.js`, `src/desktop_platform.rs`). That is
the right shape for Wayland, where clients cannot place themselves or read global
positions. It means several upstream mechanisms are not portable: per-frame hips
compensation during a drag, and global cursor polling. These are platform-limited,
not missing work.

## 2. Parity matrix

| Feature (user priority first) | Upstream evidence | Local evidence | Status | Tier |
|---|---|---|---|---|
| Drag by grabbing the mesh (primary button, front-most hit) | `homunculus_drag/src/lib.rs:343-370` (`is_frontmost_hit`, alpha-skips webviews) | `desktop_behavior.rs:255-296` mesh ray cast filtered to the avatar, outlines excluded | Implemented. Alpha-masked texels such as hair cards still count as hits; upstream `homunculus_hit_test/src/lib.rs:219-256` skips them | S,F |
| Body follows cursor at hips during drag, with hips blend | `homunculus_drag/src/lib.rs:316-331,398-438` (`DragHipsOffset`, blend 6/s, `current - hips`) | Compositor interactive move keeps the *clicked point* under the cursor. The window cannot be repositioned mid-move | Platform-limited (Wayland). The grabbed clip pivots about the hips (see §4.4), so the body swings away from the grab point. Upstream re-anchors to the hips | S,H |
| Release: drop detection and seat | `homunculus_drag/src/lib.rs:440-478`, `find_sitting_window` 80 px band (`global_windows.rs:92-121`) | `desktop_interaction.rs:291-363` keeps the 80 px band and topmost exposure; `desktop_behavior.rs:297-314,418-456` uses KWin `own.move` and fresh-sample release | Implemented and extended (KWin only) | S,F |
| Window-top sitting, follows move/resize | `homunculus_sitting/src/lib.rs:126-197` (absolute X offset from `frame.min`) | `desktop_interaction.rs:224-275` (width-fraction anchor), `kwin-surfaces.js:94-171` 50 ms polling | Implemented (KWin Wayland only). Resize keeps the relative X, a deliberate change from upstream. Latency is about 50–100 ms (poll, then move applied on the next exchange); upstream polls every frame | S,F |
| Seat height (hips vs edge) | `SITTING_ADJUST = 0.9` (`homunculus_sitting/src/lib.rs:51`) used by `mascot_tracker.rs:43-60`: hips end **0.1×hip height above** the edge | `desktop_interaction.rs:264-268`: hips joint **on** the edge | **Bug.** See §3.1 | S |
| Hips anchor smoothing and snap | `HipsAnchor` 4/s, snap 0.3 (`homunculus_sitting/src/lib.rs:58-85`), Y only | `desktop_interaction.rs:36-37,250-263`, both axes, snap in projected px | Implemented | S,F |
| Taskbar / dock sitting | None explicit. On Windows the taskbar is just another enumerated window | MaximizeArea floor per output plus projected bottom-dock floors (`kwin-surfaces.js:118-149`). Hips/feet contact allowed for floors (`desktop_interaction.rs:327-349`) | Implemented beyond upstream (KWin). Not live-accepted | S,F |
| Multi-monitor | Fullscreen window and camera per monitor, render-layer isolation (`homunculus_windows/src/lib.rs:115-160`) | One window moved across outputs by KWin; per-output floors; `applyMove` clamps 24 px reachable per output (`kwin-surfaces.js:50-73`) | Implemented differently. Mixed-DPI moves not verified | S,F |
| Dock / window collision (falling, walking, bumping) | None (no physics; drop off-window stays in place, `homunculus_drag/src/lib.rs:475-477`) | None (`FEATURE-STATUS.md:13`) | Parity (both missing). Waifu Mate-style gravity is new work, not a port | S |
| Seat target disappears | Upstream keeps the last frame (`GlobalWindow::update` returns `None`, so tracking skips) | Resets to Idle immediately (`desktop_interaction.rs:237-244`) | Implemented (stricter than upstream) | S,F |
| Transparent overlay | `transparent`, explicit composite alpha, no decorations, AlwaysOnTop (`homunculus_windows/src/lib.rs:243-262`) | `runtime.rs:38-50,283-301` same flags. Desktop RGBA corner pixel verified | Implemented. Compositor stacking is unaccepted | S,F,H |
| Per-pixel click-through (transparent areas pass clicks) | Per-frame mesh ray cast toggles `CursorOptions.hit_test` (`homunculus_hit_test/src/lib.rs:89-152`) | Whole window always hit-testable unless the manual "Pass clicks" toggle is on (`runtime.rs:308,339-343`) | **Missing.** Implementable on KWin (§3.2), platform-limited elsewhere | S,F (ECS recovery only) |
| Right-click menu | Persona `pointer-click` **Secondary on the mascot** opens a HUD card-grid webview (`mods/menu/service.ts:55-60`, `mods/menu/ui/src/App.tsx:54-79`). **Not radial** | egui 8-sector radial (`desktop_menu.rs:11-12,62-68,121-226`) | Implemented as a Waifu Mate-style radial, not as upstream parity. Opens on right-click **anywhere** in the 420×640 rect, not only on the mesh (`desktop_menu.rs:119-124`) | S |
| Idle / grabbed / sitting VRMA state animations | `vrma:idle-maid`, `vrma:grabbed`, `vrma:idle-sitting` via persona behavior config (`mods/persona/shared/behavior-config.ts:17-29`) | Same three CC BY assets embedded (`desktop_behavior.rs:121-153`), 0.5 s crossfade (`animation.rs:692-717`) | Implemented | S,F,H |
| VRMA retarget (rotation) | bevy_vrm1 normalized-pose conjugation | `animation.rs:819-828`. I checked the algebra against three-vrm: `normalized = Pw·q·Lrest⁻¹·Pw⁻¹` and `raw = Pw⁻¹·n·Pw·Lrest`, identical | Implemented, matches | S,F,H |
| VRMA hips translation | `bone_translation.rs:52-73`: delta in *local* space × model-height ratio | `animation.rs:855-873` same formula | Parity, including the shared latent limit in §4.3 | S,F |
| VRM0 (−Z) targets | VRM1 only | `facing_from_sides` plus Y turn-around (`animation.rs:232-253,1047-1048`) | Extension | S,F |
| FBX / BVH sources | Not supported | `animation_import.rs:1143-1440`, `animation.rs:308-440` | Extension. All local FBX/BVH assets are T-pose rest; retarget matches three-vrm (§4.2) | S,F |
| Multiple simultaneous draggable mascots | Every `Vrm` gets drag observers (`homunculus_drag/src/lib.rs:333-341`) | Only `AvatarLoadState.active` (`desktop_behavior.rs:219-233`) | Partial | S |
| Snap to screen corner / perch | n/a (upstream has no perch) | `desktop_perching` writes `window.position` (`runtime.rs:362-404`) | Platform-limited bug on Wayland (§3.4) | S,F |
| Tray, mods, HTTP/MCP API, webview UIs, shadow panel, effects/stamps | `homunculus_{tray,mod,http_server,mcp,shadow_panel,effects}` | Native egui panels and `mcp.rs`. No tray, mod system, shadow panel or stamps | Out of the requested priority. Do not port blindly: the mod/webview/CEF stack is upstream's architecture, not a feature gap | S |
| Power saver / FPS caps | `homunculus_power_saver` | Focused/background/overlay FPS (`runtime.rs:54-57`, `desktop_settings.rs:10-12`) | Implemented equivalent | S |

## 3. Reproducible bugs, ranked (source-derived; none desktop-verified)

### 3.1 Seated avatar sinks about 25 px into the window top or dock (High, one-line class)

`desktop_interaction.rs:264-268` sets `origin = anchor - smoothed_hips`, which puts
the projected **hips joint exactly on the seat line**. Upstream lifts it. With
`SITTING_ADJUST = 0.9` (`homunculus_sitting/src/lib.rs:51`),
`tracking_with_fixed_offset` (`homunculus_core/src/system_param/mascot_tracker.rs:43-60`)
puts the root at `edge − 0.9·hips_y`, so the hips sit `0.1·hips_y` above the edge.

I decoded the bundled clip (`assets/animations/desktop/idle-sitting.vrma`) with a
throwaway parser. Hips translation stays at `(0.0, 0.824, 0.02)` (= rest
`0.8241627`), with thighs flexed about 90°. The joint is therefore at standing
height, and the thigh/buttock volume lies below it. With hips on the edge, roughly
0.08 world units of body (≈25 px at the ~320 px/unit a 640 px-tall frame gives)
overlaps the window title bar or the panel.

Reproduce offline: in `desktop_interaction.rs` tests, any drop such as
`seat_tracks_window_move_resize_with_live_hips_and_missing_surface_restores_idle`
asserts `origin.y = seat_y − hips.y`. Hips land exactly on the line. No test
covers body clearance.

Patch (concrete): give `InteractionFrame` a `seat_lift: f32` in logical px. Compute it
in `grab_companion` (`desktop_behavior.rs:327-350`) as
`hips_px.y − project(hips_world + root_up · 0.1 · hips_height_world).y`, where
`hips_height_world = (hips_world − root_world)·root_up`. Then use
`origin = anchor − smoothed_hips − Vec2::Y * seat_lift` in `tick`. Smooth the lift
with the same `HIPS_BLEND_SPEED`, or reuse the rest hip height (stable at 0.1×0.824).
Add a fixture asserting hips `0.1·hip_height` above `seat_y`.

### 3.2 Transparent margins of the 420×640 window swallow desktop clicks (High, user priority)

`runtime.rs:308` starts with `cursor.hit_test = true`. `runtime.rs:339-343` only
disables it when the user turns on global "Pass clicks"; after that the avatar
cannot be grabbed until that toggle is turned off in the separate controls window.
Upstream re-evaluates every frame from a mesh ray cast
(`homunculus_hit_test/src/lib.rs:89-152`).

On KWin the missing input is already present. When the window is not hit-testable,
Bevy no longer receives cursor events, but `DesktopSnapshot.cursor` (global
logical) and `own_window_rect` (`desktop_platform.rs:28-35`) give the local cursor
at 20 Hz.

Patch: add a resource `DesktopHover(Option<bool>)` set in `grab_companion`. Compute
`local = snapshot.cursor − own_rect.origin`, ray cast with the same avatar filter
(`desktop_behavior.rs:258-267`), and store `Some(hit)`. In `runtime.rs:339-343` use
`hit_test = grab.active || menu.is_open() || (!click_through && hover.unwrap_or(true))`.
Keep the existing manual toggle as the forced-off override. Without the bridge
(GNOME/Hyprland/X11), leave the current behavior: platform-limited.

Also gate the radial opening on the same mesh hit (`desktop_menu.rs:119-124`) so
right-clicks on empty margins do not open it. This matches the upstream mascot-only
Secondary click.

Needs D acceptance: winit on Wayland implements `set_cursor_hittest` via an empty
input region. KWin behavior was not observed in this pass.

### 3.3 Seated pose without a seat after relaunch or "Sit down" (Medium)

- `desktop_settings.rs:18` serializes `sitting`.
- `desktop_behavior.rs:171-176` plays `idle-sitting` whenever `settings.sitting`.
- `DesktopInteraction` always starts `Idle`, and `grab_companion` only ticks a seat
  when `interaction.state == Sitting` (`desktop_behavior.rs:391-399`).

Three paths therefore show a seated pose floating mid-air with no surface and no
tracking: relaunching in desktop mode, the radial "Sit down" (`desktop_menu.rs:203-206`),
and the controls checkbox (`runtime.rs:441-447`).

Fix: mark `sitting` `#[serde(skip)]` (it is runtime state now that KWin seats exist).
Make "Sit down" run `finish_native_drop`-style seat selection at the current hips
x against the nearest surface below; if none, keep it a pose-only action and label
it as such.

### 3.4 "Snap to bottom right" does nothing on Wayland even with KWin (Medium)

`desktop_perching` (`runtime.rs:362-404`) writes `window.position`, which Wayland
clients cannot apply. It also uses the full monitor height, so it lands behind the
panel on X11. With a bridge connected it should call `bridge.request_move(origin)`
against the output's MaximizeArea from the snapshot.

### 3.5 Grab release can stick without the bridge (Medium, platform-limited, S only)

With no snapshot, `native_drag_released` reduces to `!left_pressed || !focused`
(`desktop_behavior.rs:427-442`). After `start_drag_move` the compositor/WM owns the
pointer; winit commonly never delivers the button release, and Bevy does not clear
`ButtonInput<MouseButton>` on pointer re-entry. The grabbed clip can then loop
until the next click.

On non-KWin Wayland the function also returns early at `desktop_behavior.rs:317`,
before any idle restore. Add a timeout or a "pointer moved while pressed=false"
heuristic, or clear `grab.active` on the next `CursorMoved` when no compositor
drag is known. Needs D verification.

## 4. Arms / rotation / animation notes

### 4.1 Grabbed pose looks prone: data-faithful, not a retarget bug

`previews/desktop-grabbed.png` shows the body pitched forward with arms hanging. The
VRMA keys explain it:

- Hips first key `(-0.994,-0.003,-0.026,0.103)`; rest `(0.750,0,0,0.661)`; parent
  `Armature.001` identity.
- The normalized hips rotation is therefore ≈ +94.6° about X, i.e. torso pitched
  forward (held-by-the-scruff pose).

The local conjugation matches three-vrm/bevy_vrm1 (§2), and the sitting render's
legs come toward the camera, so facing/turn-around is correct for the Neuro test
model.

What differs from upstream is anchoring, not pose. Upstream keeps the hips under
the cursor; the local compositor keeps the grab point. Clicking the head and
dragging therefore makes the body rotate away from the cursor. This is
platform-limited; a mitigation is to start the grabbed clip with
`transition_seconds` longer than 0.5 s.

### 4.2 A-pose FBX/BVH sources (withdrawn as a bug; theoretical only, no fixture)

Algebra: at a frame where the source is at its own rest (`sampled = source.rest_rotation`),
`normalized_pose` returns identity and `destination_pose` returns the target's rest
(`animation.rs:822-828`; also asserted by the second check in
`normalized_joint_space_preserves_arbitrary_source_and_destination_rests`). The
retarget therefore maps *source rest to target rest*. That is exactly three-vrm /
bevy_vrm1 behavior, and it is correct whenever the source rest is a T-pose, as VRM
targets are.

Measured on every real local motion asset (read-only parsers, mirroring
`parse_bvh` offsets and `parse_fbx`'s `Pre·Lcl·Post⁻¹` rest, chained to model space):

- WebWaifu4 `Idle.fbx`, `Idle2.fbx`, `Idle3.fbx`, `Thinking.fbx`, `Wave.fbx`:
  upper-arm and forearm rest directions are exactly `(±1, 0, 0)`, elevation 0.0°.
  Upper legs are `(0, −1, 0.008)`.
- All 35 `silly-bvh/*.bvh`: all 140 arm-chain offsets have zero Y and Z (pure ±X).

No A-pose source exists in the workspace, so there is no reproducible mismatch.
Do **not** add a generic A→T correction. Revisit only if a user file with a
non-T rest is supplied, and then fixture it first.

### 4.3 Hips delta applied in hips-parent local space (Low; shared with upstream)

`animation.rs:856-870` and bevy_vrm1 `bone_translation.rs:52-60` both add the delta
directly to the local translation, ignoring parent rotation and non-uniform scale.
That is fine for VRM1 normalized and VRM0 identity-rest rigs. It is wrong for FBX
armatures with a −90° X or 0.01-scale parent: forward hip motion becomes vertical.
FBX `rest_scale` is always `Vec3::ONE` (`animation_import.rs:1257`).

Fix: rotate the delta by `parent_rest_model⁻¹` before adding. This does not change
VRM1 results.

### 4.4 Procedural arms

`relax_arms` (`runtime.rs:756-829`) waits for both imported rest snapshots
(root's fix, covered by `runtime.rs` test `idle_arm_relaxation_waits_for_both_original_bind_snapshots`).
It skips the active avatar while posing, so desktop VRMA owns the arms.

The world-space target `(±0.32,−0.94,+0.06)` puts hands slightly **behind** the
body for avatars facing −Z in world. That is cosmetic; use the anatomical forward
from `gaze.rs:248-266` instead of +Z.

## 5. Other observations (lower priority)

- The "Desktop controls" window is a same-PID normal window with a different
  caption (`runtime.rs:310-317`). It is therefore a valid seat target and occluder
  in `kwin-surfaces.js:19-22,114-116`. Exclude by PID if unwanted.
- `frame_avatar` reframes on every sit toggle (`runtime.rs:671-683`) but ignores
  `orbit.sitting` in its math (`runtime.rs:833-920`). This only resets yaw and
  pitch.
- Camera/drag picking uses `MeshRayCast` with no alpha test. Grabs on transparent
  hair-card texels start a drag.

## 6. Highest-impact concrete patch

1. **§3.2 KWin per-mesh click-through** gives the largest everyday UX win: the
   companion stops blocking a 420×640 desktop rectangle. It is about 30 lines
   across `desktop_behavior.rs` and `runtime.rs` and reuses existing snapshot/ray
   cast code.
2. **§3.1 seat lift** is next: a small change, directly visible on every window and
   taskbar sit, and it matches upstream's calibrated 0.9 factor.
3. Then §3.3 (`#[serde(skip)]` plus a real "Sit down").

All three need actual KWin desktop acceptance after implementation. Offline
fixtures cannot prove compositor input-region behavior or visual clearance.
