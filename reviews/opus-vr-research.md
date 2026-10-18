# VR / OpenXR integration research — Waifu Studio (Bevy 0.19)

Author: Opus 5.5 research agent, 2026-10-18. Read-only research: no app source changed, no Cargo run, no VR code written, no runtime installed, no headset or device accessed.

## Evidence classes used in this report

- **[SRC-APP]**: inspected app source in this tree (paths are relative to the app root).
- **[SRC-UP]**: inspected published upstream crate source. `bevy_mod_openxr`, `bevy_mod_xr` and `bevy_xr_utils` 0.6.0 were downloaded from crates.io into `~/.cache/vr-research/dl/` (not this tree). `openxr` 0.21.1 and `bevy_egui` 0.42.0 were already in `~/.cargo/registry`.
- **[PRIMARY]**: crates.io API, the GitHub API for bevy_oxr/WiVRn/OpenXR-SDK, Monado GitLab raw source and the Arch package file list. All fetched unauthenticated on 2026-10-18, with the proxy environment left intact.
- **[HOST]**: read-only probes of this machine (`ldconfig`, `pacman -Q`, `lspci`, XDG paths).
- **[INFER]**: my engineering inference, not yet verified by any build or run.

Nothing in this report is backed by a fake test, a headless render, desktop acceptance or hardware acceptance; no such evidence exists for VR. The app's own `FEATURE-STATUS.md:43` correctly lists VR/OpenXR as "Not implemented".

---

## 1. Bottom line

1. **A desktop-PC VR (PCVR) prototype on Linux is feasible on the current stack without changing any version.** `bevy_mod_openxr`/`bevy_mod_xr`/`bevy_xr_utils` **0.6.0** (published 2026-06-20) require `bevy_* ^0.19`, `wgpu ^29`, `wgpu-hal ^29`, `openxr ^0.21.1` and `ash ^0.38`. That matches the lock exactly: Bevy subcrates 0.19.1, wgpu/wgpu-hal 29.0.4, ash 0.38.0 (`Cargo.lock:238,6554,6629`) [PRIMARY + SRC-APP]. `openxr` 0.22.0 (2026-09-17) exists, but bevy_mod_openxr 0.6.0 pins `^0.21.1`, so 0.21.x is the version to use.
2. **Do not upgrade Bevy if VR is wanted soon.** Bevy 0.20.0 shipped 2026-10-08 and bevy_egui 0.43 targets it. bevy_oxr's "Update to bevy 0.20 + openxrs 0.22" is still **open PR #212** (opened 2026-10-09), with no 0.7 release on crates.io [PRIMARY].
3. **This host currently has no OpenXR stack at all** [HOST]:
   - no `libopenxr_loader` in `ldconfig -p`;
   - no `/usr/share/openxr`, `/etc/xdg/openxr` or `~/.config/openxr` (so no `active_runtime.json`);
   - no Monado, WiVRn, SteamVR or ALVR packages, Flatpaks or Steam library entries.

   The GPU is an NVIDIA RTX 5060 Ti (`nvidia-open-dkms 615.71.09`) plus an Intel UHD 770 iGPU (`lspci`), with `vulkan-icd-loader 1.4.357` (`pacman -Q`). Compositor/desktop: **not established by this research.** The only signals were environment hints in the agent shell: `XDG_SESSION_TYPE=wayland`, and `hyprctl version` reported `HYPRLAND_INSTANCE_SIGNATURE not set`. That is not process proof of any compositor, and it does not rule out the KWin Wayland session seen in prior host evidence. Root's exact `pgrep` check is authoritative. VR conclusions here do not depend on which compositor is running: OpenXR runtimes present to their own compositor, and only the desktop mirror window goes through the session compositor. Today only the **no-runtime fallback path** can be exercised here.
4. **The avatar is a companion character, not the user's body.** So head tracking and controllers are needed, but full-body IK is **not** required for a first VR milestone. Gaze-at-HMD and placing the avatar in stage space can reuse existing code (§6).
5. **Standalone Quest is not realistic near-term.** bevy_mod_openxr does support Android, but the app depends on `rfd`, `zbus`, `cpal`, `ort` load-dynamic, blocking `reqwest` and desktop windows (`Cargo.toml:21-45`) [SRC-APP]. For a Quest, use PCVR streaming via WiVRn instead (§4).

---

## 2. High-severity integration hazards (concrete, source-cited)

These already exist in the app and would break or degrade any naive `add_xr_plugins(...)` integration.

### H1. Frame pacing would throttle the XR loop to roughly 15 Hz — HIGH
- `src/runtime.rs:62` adds `pace_frames` in `First`. `src/runtime.rs:87-108` then `std::thread::sleep`s to `focused_fps` / `overlay_fps` / `background_fps`.
- `src/runtime.rs:110-131` (`frame_cadence`) rewrites `WinitSettings` to `UpdateMode::reactive_low_power` every frame. `src/runtime.rs:54-57` sets the same at build.
- The defaults are focused 60, background **15**, overlay 30 (`src/desktop_settings.rs:40-42`). Someone wearing a headset leaves the desktop window unfocused, so the main schedule — and with it OpenXR `wait_frame`/`locate_views`/`end_frame` (`bevy_mod_openxr src/openxr/render.rs:168-171,306-379`) — would run near background_fps.
- bevy_mod_openxr only forces `WinitSettings { Continuous, Continuous }` under its optional `window_support` feature (`init.rs:156-162`) [SRC-UP]. Even then, the app's later `StudioRuntimePlugin` insert and its per-frame `frame_cadence` would overwrite it.
- Required: while `XrState::Running`, skip `pace_frames` and `frame_cadence` and keep `UpdateMode::Continuous`, letting `xrWaitFrame` pace the loop. [SRC-APP + SRC-UP; effect is INFER until measured]

### H2. OpenXR owns device creation at `Plugin::build`, so VR must be chosen at launch — HIGH
- `add_xr_plugins` disables `RenderPlugin` and inserts `OxrInitPlugin` before it (`bevy_mod_openxr src/openxr/mod.rs:55-61`).
- `OxrInitPlugin::build` loads the loader and creates the instance and the Vulkan/wgpu device from the runtime's requirements. It then adds `RenderPlugin { render_creation: RenderCreation::manual(...) }` (`init.rs:102-125,254-317`).
- If that fails it logs `Failed to initialize openxr` and adds `RenderPlugin::default()` with `XrState::Unavailable` (`init.rs:186-212`) [SRC-UP].
- Consequences:
  - VR cannot be a mid-session UI toggle. The runtime (monado-service / WiVRn server) must already be active when the app starts.
  - Do **not** wrap the plugin group in non-VR launches. Even with fallback, that path changes renderer construction for every user.
  - The app sets no custom `RenderPlugin`/`WgpuSettings` today (`src/main.rs:233-252`), so the fallback `RenderPlugin::default()` loses nothing *now*. Any future custom render settings would be silently dropped on the fallback path.
  - The XR device is created with all adapter features and adapter limits (`graphics/vulkan.rs:385-447`), so Bevy's PBR features stay available.
- On Linux the loader is opened at runtime: `openxr` default feature `loaded` → `Entry::load()` dlopens `libopenxr_loader.so` (`openxr-0.21.1/src/entry.rs:59-68`). A VR-feature binary therefore still starts on machines without a loader, via the fallback. Arch's `extra/openxr 1.1.60` ships the unversioned `usr/lib/libopenxr_loader.so` [PRIMARY]. Distros that put the `.so` symlink only in a `-dev` package would need `load_from("libopenxr_loader.so.1")`.

### H3. `window_support` would clobber the app's window config — MEDIUM
With `window_support` enabled, `add_xr_plugins` `.set(WindowPlugin { primary_window: transparent + AutoNoVsync, ..default() })` (`mod.rs:73-81`). That replaces the app's title, resolution, premultiplied alpha and `ExitCondition` (`src/main.rs:240-252`, `src/runtime.rs:38-49`) and resets `exit_condition`.
- Recommendation: leave `window_support` off (it is not in defaults: `default = ["vulkan"]`) and keep the app's own WindowPlugin.
- Still use `PresentMode::AutoNoVsync` (or Mailbox) on the desktop mirror window while XR runs. A FIFO-vsynced 60 Hz mirror can stall the shared render loop below the HMD rate [INFER — the upstream default suggests the same].

### H4. Bevy 0.20 upgrade blocks VR — HIGH (planning)
See §1.2. Pin `bevy = "=0.19.0"` (already pinned, `Cargo.toml:10`) until bevy_oxr releases for 0.20.

### H5. Spawn position: the user would stand inside the avatar — MEDIUM
- bevy_mod_openxr's primary reference space defaults to `STAGE` (`reference_space.rs:21-25`), with the origin at the room's floor centre.
- The app puts the avatar at the origin with scenery only at −Z (`src/environments.rs:1-5`), and the desktop camera at `(0, 1.2, 1.8)` looking −Z (`src/runtime.rs:213-222`).
- So in VR the user starts *inside* the avatar, and turning around shows empty space (scenery is only built at −Z). Fix: offset `XrTrackingRoot` (bevy_mod_xr `session.rs:106-119`) to about +1.5 m Z facing −Z, and add a ground/sky around +Z for the VR stages.
- Separately, the image backdrop is a quad parented to the desktop `StudioCamera` at −80 m (`src/runtime.rs:224-233`), so it never appears in the headset.

---

## 3. Crate compatibility matrix

| Crate / version | Requires | App lock | Verdict | Evidence |
|---|---|---|---|---|
| bevy_mod_openxr 0.6.0 (2026-06-20) | bevy_* ^0.19, wgpu ^29, wgpu-hal ^29, openxr ^0.21.1, ash ^0.38 (opt), bevy_window/winit optional | bevy 0.19.0 / subcrates 0.19.1, wgpu 29.0.4, ash 0.38.0 | **Compatible** | crates.io deps API; Cargo.lock |
| bevy_mod_xr 0.6.0 | bevy_* ^0.19 | same | Compatible | crates.io |
| bevy_xr_utils 0.6.0 | bevy 0.19 line | same | Compatible (controller actions, tracked grip/view entities) | crate source `tracking_utils.rs:30-44`, `actions.rs` |
| bevy_mod_openxr 0.5.0 | bevy ^0.18, wgpu ^27 | — | Incompatible | crates.io |
| openxr 0.22.0 | — | — | Not usable with oxr 0.6 (pins ^0.21.1) | crates.io |
| bevy_egui 0.42.0 (current) | bevy 0.19 | locked | Compatible; has world-space render-to-image + picking example | `bevy_egui-0.42.0/examples/render_egui_to_image.rs:69-160` |
| bevy_mod_inverse_kinematics 0.12.0 (2026-09-01) | bevy ^0.19 | — | Compatible candidate if IK is ever needed | crates.io deps API |
| Bevy 0.20 / bevy_egui 0.43 | — | — | **No released OpenXR crate yet** (bevy_oxr PR #212 open) | GitHub API |

Upstream state worth knowing [PRIMARY, GitHub bevy_oxr]:
- Still-open PRs: #210 opt-in multiview (`D2Array` view; *does not* convert Bevy PBR to single-pass stereo); #211 refresh-rate request; #205 Android surface fix.
- Merged: #203 "fix lag when session becomes visible again on Monado runtimes"; #180/#181 directional-light shadow fixes; #193 FBT plugin for Monado/WiVRn.
- Minor upstream quirk: the swapchain-format loop in `init.rs:443-450` has no `break`, so the *last* matching preference wins. That is harmless with the default single entry `Rgba8UnormSrgb` (`resources.rs:351-357`).

## 4. Linux runtime matrix (nothing installed or tested here)

| Runtime | Role | Linux status (primary) | Fit | Host state |
|---|---|---|---|---|
| **WiVRn** v26.9 (2026-09-12) | OpenXR runtime that streams to Quest/Pico/standalone headsets over Wi-Fi/USB | Active releases (v26.6.1, 26.6.2, 26.9) | **Best practical path for a Quest owner on Linux.** | Not installed |
| **Monado** v25.1.0 (2025-12-09) | Native OpenXR runtime (Index/Vive via libsurvive/lighthouse drivers, simulated devices) | Main branch has `SIMULATED_ENABLE` (`target_builder_simulated.c:32`), `QWERTY_ENABLE` (`target_builder_qwerty.c:33`) and `XRT_COMPOSITOR_NULL` (`target_instance.c:43`) | **Best headset-free acceptance runtime** (simulated HMD + null compositor); native for lighthouse headsets | Not installed |
| SteamVR (Linux) | OpenXR runtime for Valve/lighthouse HW | Not verified in this research | Possible for Index users; not recommended as first target without a test | `steam` pkg installed, **no SteamVR in library** |
| OpenXR loader (Khronos) | Required by all of the above | SDK release-1.1.65 (2026-10-16); Arch `extra/openxr 1.1.60` | Required at runtime only (dlopen) | **Missing** |

Head/controller tracking come from the runtime: `bevy_xr_utils::tracking_utils` exposes `XrTrackedView`, `XrTrackedLeftGrip`, `XrTrackedRightGrip`, `XrTrackedStage` and `XrTrackedLocalFloor` (`tracking_utils.rs:30-44`), plus `suggest_action_bindings` (`:233`). Hand tracking is requested by default and disabled with a warning when the runtime lacks the extension (`init.rs:86-92,274-278`).

## 5. Rendering implications (stereo, post-processing, toon, shadows)

- **Two full camera passes per frame.** 0.6.0 spawns two `XrCamera(0|1)` `Camera3d` entities, each targeting one layer of a 2-layer swapchain (`render.rs:146-166,310-330`); there is no multiview (#210 unmerged). The desktop `StudioCamera` would be a third full pass, and the avatar-preview camera a fourth when active (`src/avatar_preview.rs:84-99`). While XR runs, the desktop mirror should be lowered in resolution, rate-limited or disabled. Cost on the RTX 5060 Ti is unmeasured.
- **Post-processing will not reach the eyes unless tagged.** The app's tonemapping, Hdr, Bloom, ChromaticAberration and ColorGrading are applied only to `With<VisualCamera>` (`src/visuals.rs:257,310-370`). XR cameras spawned by the crate lack that marker, so they would render with Bevy's default tonemapper and no bloom, which looks different from the desktop.
  - An `Added<XrCamera>` system inserting `VisualCamera` would work, since `sync` processes entities lacking `Baseline` (`visuals.rs:328-330`) [SRC-APP].
  - ChromaticAberration and heavy bloom are known VR-comfort problems; disable them in VR [INFER].
  - MSAA is fine: swapchain `sample_count: 1` (`init.rs:461`), and Bevy resolves its own MSAA target before writing.
- **Swapchain colour:** `Rgba8UnormSrgb` by default, with an opaque blend mode (`resources.rs:351-357`). The app's transparent-overlay `ClearColor` handling (`src/runtime.rs:484`) should not be driven by XR; the eyes need an opaque environment. Desktop overlay (`ViewMode::Desktop`) should be forced off while XR runs.
- **MToon screen-space outlines:**
  - Screen mode offsets the clip position by a view-space normal scaled by viewport aspect (`vendor/crates/bevy_shader_mtoon/src/outline.wgsl:69-75`). It is computed per eye, so line width is in per-eye pixels; at about 2k-wide eye buffers lines look thinner, and the eyes can disagree slightly at silhouettes.
  - World mode (`outline.wgsl:69-71`) is stereo-consistent. Prefer it in VR, or scale the width by eye resolution. Outline visibility is already centrally controlled (`src/shaders.rs:384-401`).
- **MToon rim/matcap** uses per-view vectors (`mtoon.wgsl:175-176`). Per-eye differences are physically plausible and usually acceptable [INFER]. Lighting uses a single `MtoonSun` uniform (`bevy_shader_mtoon/src/lib.rs:60-100`), which is view-independent, so that is fine.
- **Shadows:** the app's directional light has shadows enabled (`src/runtime.rs:234-242`). Cascades are built per view, so cost roughly doubles. XR frustum corners were fixed upstream (PR #180/#181; `bevy_mod_xr camera.rs` `get_frustum_corners`). `XrProjection::get_clip_from_view_for_sub` panics (`camera.rs:93-95`), so never add `SubCameraView` to XR cameras.
- **Pipelined rendering:** the app keeps default `DefaultPlugins` with `multi_threaded` (`Cargo.toml:10`). The crate accounts for pipelining by predicting one display period ahead (`render.rs:191-200`). Its example disables `PipelinedRenderingPlugin` to cut input latency (`examples/3d_scene.rs:8-13`). Measure both settings.

## 6. Avatar: head tracking, IK vs. current motion stack

- **Current stack** [SRC-APP]:
  - Full-body retargeted clips: BVH/VRMA/FBX retargeting (`src/animation.rs:1`), plus bundled desktop motions.
  - Procedural gaze overlays on head/neck/eyes: random targets in avatar space (`src/gaze.rs:1-8,173`); explicitly "does not track the pointer".
  - Single-bone world-space `aim` (`src/desktop_pose.rs:71`).
  - Fixed-step Verlet spring bones in `PostUpdate` after transform propagation (`vendor/crates/bevy_vrm/src/spring_bones.rs:19-35,231-252`).
  - VRM first-person layer splitting on layers 7/8 (`vendor/crates/bevy_vrm/src/first_person.rs:13-31`), which the app never triggers (no `SetupFirstPerson` in `src/`).
  - **No IK solver anywhere** (grep for IK/two-bone/FABRIK found none).
- **Companion mode (recommended first):**
  - No IK is needed. Add a gaze target mode that looks at the `XrTrackedView` world position, using the existing rest-space conversion in `gaze.rs` plus `desktop_pose::aim`. Clamp it with the existing intensity/head_follow limits.
  - Spring bones are already frame-rate independent (fixed step), so a 72–120 Hz loop is fine.
  - Keep existing clips. Mouse-based desktop grab/raycast (`src/desktop_behavior.rs:205-262`, `MeshRayCast`) can be ported to controller rays because the `mesh_picking` feature is already enabled.
- **Optional interactions** (hand reach, head-pat response): use per-chain IK on top of clips. `bevy_mod_inverse_kinematics 0.12.0` supports Bevy ^0.19, but ordering must be explicit: clip → gaze overlay → IK → propagate → spring bones. Otherwise it fights `animation.rs` and `gaze.rs` writes [INFER].
- **Self-embodiment** (user drives a VRM): needs 3-point IK (head + two grips), first-person mesh layers, and leg/locomotion synthesis. That is a separate project; do not mix it into the companion milestone.

## 7. UI strategy

1. **Stage 1 — desktop remains the operator surface.** The full egui UI stays on the desktop window. `PrimaryEguiContext` lives on `StudioCamera` with `auto_create_primary_context = false` (`src/runtime.rs:205-215`), so XR cameras get no egui. Speech is the in-headset input: the existing listen pipeline uses the default input device (`src/listen.rs:331`). Audio output follows the system default sink; the app has no output-device selector, and WiVRn exposes its own sink, so routing is a user/system step [INFER].
2. **Stage 2 — one world-space panel.** bevy_egui 0.42 renders a context to an `Image` on a 3D quad with picking observers (`examples/render_egui_to_image.rs:69-160`). Its picking is driven by camera-pointer events. A controller ray needs either a custom `bevy_picking` pointer/backend or direct egui input injection from a `MeshRayCast` hit. This is an **unverified integration point** that needs a spike. Keep it to chat transcript, mute/listen and avatar/scene switching. Avoid text entry in the headset.

## 8. Packaging and desktop fallback

- Cargo feature `vr` (off by default) with optional deps on `bevy_mod_openxr = "0.6"` (default `vulkan` only, **no** `window_support`), `bevy_mod_xr = "0.6"` and `bevy_xr_utils = "0.6"`. The default build stays byte-for-byte unaffected.
- Runtime switch `--vr`: only then wrap `DefaultPlugins` with `add_xr_plugins` (`src/main.rs:233`).
  - On `XrState::Unavailable`, show a clear "no OpenXR runtime" status and continue on desktop. Fallback is upstream behaviour (H2).
  - Never combine with `--offscreen` until tested: offscreen disables `WinitPlugin` and uses `ScheduleRunnerPlugin` (`src/main.rs:253-262`).
- While running: force `ViewMode::Studio` (no overlay or click-through), apply H1/H3, and place the user via the tracking root (H5).
- The Linux binary does not link the loader (dlopen). Document `openxr` plus one runtime as user-installed; this app should not install runtimes.

## 9. Feasibility matrix

| Capability | Feasible on current stack? | Blocking evidence / dependency | Confidence |
|---|---|---|---|
| Build with `vr` feature on Bevy 0.19 | Yes | Exact version match (§3) | High [PRIMARY], not compiled here |
| Desktop fallback with no runtime | Yes (upstream path) | `init.rs:186-212`; this host lacks a loader, so it would hit fallback | High [SRC-UP], unexecuted |
| PCVR stereo render of VRM + MToon + environments | Yes, with H1/H3/H5 fixes | Two passes, outline mode, VisualCamera tagging | Medium [INFER] |
| Head tracking / avatar looks at user | Yes | gaze.rs target mode + `XrTrackedView` | Medium |
| Controllers: ray select/grab avatar | Yes | bevy_xr_utils actions + `MeshRayCast` | Medium |
| In-headset egui panel | Probably | bevy_egui image context; controller-pointer bridge unverified | Low–Medium |
| Hand tracking | Runtime-dependent | Ext auto-disabled if absent | Medium |
| Single-pass multiview | No (0.6.0) | PR #210 unmerged; does not cover PBR anyway | High |
| Mid-session VR toggle | No | Device created in `Plugin::build` | High [SRC-UP] |
| Full-body self-avatar IK | Not near-term | No IK in app; separate project | High |
| Standalone Quest APK | Not near-term | Desktop-only deps (§1.5) | High |
| Bevy 0.20 + VR | Not yet | bevy_oxr PR #212 open | High [PRIMARY] |

## 10. Staged prototype plan with acceptance criteria

Each stage names the evidence class required to pass. "Headless" and "simulated" are never reported as headset acceptance.

**S0 — Feature-gated skeleton, fallback only (possible on this host).**
- Accept on source evidence: default build unchanged; `vr` feature compiles.
- Accept with a headless/desktop run: `--vr` with no loader logs `Failed to initialize openxr` and renders the desktop scene identically to a normal launch (compare an offscreen capture with/without `--vr` once that combination is supported, or a manual desktop screenshot).
- Fails if any non-`--vr` launch constructs the renderer through OxrInitPlugin.

**S1 — Monado simulated runtime, no headset** (requires the user to install `openxr` + Monado; not done by me).
- Run `monado-service` with `SIMULATED_ENABLE=1`, optionally `XRT_COMPOSITOR_NULL=1`.
- Accept: `XrState` reaches `Running`; both `XrCamera`s are active; frames are submitted continuously; the main loop rate tracks the runtime display rate with the desktop window **unfocused** (proves H1 fixed); the desktop mirror stays responsive; clean session exit returns to `Available`/desktop without crashing.
- Evidence class: simulated runtime only. It proves no rendering correctness in a lens, comfort or tracking quality.

**S2 — Visual parity in stereo** (same simulated runtime; screenshots of the swapchain/mirror).
- XR cameras carry the VisualCamera tonemapping/grading; chromatic aberration is off in VR; outlines are in world mode or width-compensated; shadows are not clipped at frustum edges; no `SubCameraView` panic.
- The avatar spawns about 1.5 m in front of the stage origin, facing the user, with environment visible in all directions.

**S3 — Interaction** (simulated controllers via `SIMULATED_LEFT/RIGHT` or qwerty).
- Controller pose entities follow; a ray hit on avatar meshes reproduces the desktop grab/poke behaviour; gaze-at-HMD target keeps head/eye rotation within existing clamps; spring bones stay stable at the runtime rate.

**S4 — Real headset acceptance** (needs user hardware: WiVRn + Quest, or Monado/SteamVR + lighthouse HMD).
- Sustained runtime refresh rate without reprojection warnings over 10 minutes with speech + lipsync active.
- Correct world scale (1 unit = 1 m) and IPD.
- No stereo outline shimmer judged objectionable.
- Audio and mic routed through the headset.
- Recovery after HMD sleep/wake (cf. upstream Monado visibility fix #203).
- Clean quit both from the desktop and from the runtime.

**S5 (optional) — World-space egui panel spike.** Pass only when a controller ray can click and scroll the panel with no stray desktop-pointer interference.

## 11. Limitations of this research

- No `cargo` build, so compatibility is from published dependency ranges plus the lockfile, not from a resolved build.
- No runtime was installed and no headset was used. All rendering, performance and comfort statements beyond source are inference.
- SteamVR-on-Linux behaviour was not researched to primary-source depth.
- WiVRn/Monado capabilities were checked only at release/source-symbol level.
- The bevy_egui controller-pointer bridge and the effect of the desktop mirror's present mode on the XR loop need prototypes to confirm.
