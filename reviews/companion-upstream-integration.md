# Homunculus + Mate-Engine integration audit

Checked 2026-10-18 against both exact repositories requested by the user. Local checkouts match GitHub HEAD (verified with `git ls-remote`, without changing either checkout):

- [Desktop Homunculus](https://github.com/not-elm/desktop-homunculus/tree/30d11a5ad72f97f405117086839acd0ea87e1c49), commit `30d11a5`.
- [Mate-Engine, shinyflvre fork](https://github.com/shinyflvre/Mate-Engine/tree/2c5ea6b8f4cf5e1773a0816b46d9267cda5174d4), commit `2c5ea6b`.

This is a feature and integration audit, not a claim of complete parity. Earlier `opus-homunculus-parity.md` findings describe an older tree; several are already fixed. The app is Rust/Bevy on Linux/Wayland, not Unity or a webview host.

## Changes made in this audit

1. Standing desktop companions now use the same configurable idle/talking playlists as Studio. Actual speech selects talking motions. Dragging and verified sitting suspend playlists; releasing the pose reselects the current speech/idle role. Manual pause is preserved.
2. Reply body motions are suppressed while desktop dragging/seating owns the body, and the reply ID is consumed so it does not replay after standing up. These suppressed replies do not launch motion-decision requests. Faces and lip sync retain their own channels.
3. Automatic transparent-margin click-through can recover from a fresh global cursor even after the controls window is dismissed. Hitting the avatar, dragging or opening its radial menu restores interaction. Stale/missing tracking falls back to an interactive window. Forced whole-window passthrough still requires separate recovery controls.

These follow the useful behavior of both projects without importing Mate source or assets. The previous desktop fix (`f20dfb7`) already separated closing controls from leaving desktop mode and removed distant-seat searching.

## Reuse boundaries

Homunculus Rust is MIT OR Apache-2.0; TypeScript is MIT; creative assets/documentation are CC BY 4.0. Our three official VRMA motions retain their attribution in `assets/animations/desktop/ATTRIBUTION.md`, and the adapted interaction module retains its MIT notice.

Mate's `LICENSE.md` starts with MateEngine Pro v2.1: private study/modification, source-publication requirements for derivatives, noncommercial/distribution restrictions. It separately reserves rights in bundled assets and contains third-party terms. Its README's mixed-license summary is not a general MIT grant. This audit copies no Mate implementation or assets. Behavior equivalents are written in our own Rust implementation. Unity shaders, Animator assets and Win32 P/Invoke cannot simply be plugged into Bevy/Wayland.

## Feature inventory

Source paths below are relative to each upstream repository. Local paths are relative to Waifu Studio.

| Behavior | Upstream source | Current local implementation / remaining work |
|---|---|---|
| Drag by avatar mesh; release near a window edge | Homunculus `engine/crates/homunculus_drag/src/lib.rs`, `homunculus_screen/src/global_windows.rs` | `src/desktop_behavior.rs`, `src/desktop_interaction.rs`: native compositor drag, 80px drop band, exposed-edge filtering. Real drag acceptance outstanding. |
| Window/taskbar sitting and tracking | Homunculus `homunculus_sitting/src/lib.rs`; Mate `Assets/MATE ENGINE - Scripts/AvatarHandlers/AvatarTaskbarController.cs`, `AvatarWindowHandler.cs` | Trusted window/work-area snapshots, seat smoothing, moving/resizing support, vanished-seat cleanup, standing with feet on edge. KWin + Hyprland adapters, offline tested; real taskbar/mixed-DPI acceptance outstanding. |
| Transparent desktop and automatic input regions | Homunculus `homunculus_hit_test/src/lib.rs`, `homunculus_windows/src/lib.rs` | Mesh hover/native hit-test toggling; now also works after dismissing controls when global tracking is fresh. Alpha-texture-aware ray hits remain missing (transparent hair cards may count as geometry). |
| Multi-monitor handling | Homunculus `homunculus_windows/src/lib.rs`; Mate `Settings/MonitorHelper.cs` | One companion window can cross outputs; snapshots include work areas and reachable placement. Unlike Homunculus, no fullscreen overlay per monitor. Mixed-scale live checks needed. |
| Idle / grabbed / seated state clips | Homunculus `mods/persona/shared/behavior-config.ts`, `mods/assets/vrma` | Official attributed VRMA clips and 0.5s crossfades. Drag/sit take priority over local playlists. |
| Timed idle variation and motion ownership | Mate `AvatarHandlers/AvatarAnimatorController.cs`, `AvatarSleepController.cs` | Idle/talking round-robin playlists, original nod/tilt/look-down clips, bounded cache, no repeated file import per frame. Works standing in Desktop as well as Studio after this audit. Sleep/wake state still absent. |
| Talking and emotion actions | Homunculus persona/voice mods; Mate animator/AI handlers | Actual playback selects talking; reply metadata maps to scanned one-shot playlists; incoming appraisal and facial lifecycle are separate. Body gestures are skipped during drag/sit. No Unity Animator graph dependency. |
| Pointer gaze and authored VRM eyes | Mate `AvatarHandlers/AvatarMouseTracking.cs` | `src/gaze.rs`, VRM range maps, head/neck/eye smoothing, morph/material expression look-at, bounded desktop cursor targeting. No spine tracking or authored per-animation gaze permissions. |
| Touch/head-pat reactions | Mate `AvatarHandlers/AvatarBigScreenTouchHandler.cs`, `PetVoiceReactionHandler.cs` | Missing dedicated touch events, hit regions, cooldowns and reactions. A drag must not count as a pat. High-value next implementation, original clips/expressions required. |
| Drag sway / movement expressions | Mate `AvatarHandlers/AvatarSwayController.cs` | Grabbed VRMA exists; velocity-driven additive sway and motion-expression input absent. Must compose with animation and restore bones instead of adding another uncontrolled writer. |
| Sleep/wake on inactivity | Mate `AvatarHandlers/AvatarSleepController.cs` | No animation sleep controller. `src/routines.rs` supplies fictional time context only, not sleep behavior. Add optional user-configured clips and wake on speech/grab. |
| Music-aware dancing | Mate `AvatarHandlers/AvatarAnimatorController.cs`, `AvatarDancePlayer.cs`, `AvatarDanceSync.cs` | Dance clips can be imported/played. Automatic music detection, beat analysis, group dance sync and an MMD/VMD player are absent. Use Linux media-session support rather than copying Windows audio-session code. |
| Gravity / autonomous walking | Mate `AvatarHandlers/AvatarGravityController.cs`, `AvatarLocomotionController.cs` | Missing. This is separate from drop seating and would require optional movement permissions, collision/edge policy, platform integration and accepted clips. Do not enable movement/snap by surprise. |
| Screen-edge hiding / peek | Mate `AvatarHandlers/AvatarHideHandler.cs` | Missing. User-directed edge seating remains the primary behavior; no automatic relocation added. |
| Clothing visibility and outfits | Mate `Assets/MATE ENGINE - Mod SDK/MEClothes.cs`, `AvatarClothesHandler.cs`, `AvatarHandlers/AccessoiresHandler.cs` | `src/appearance.rs` saves per-character mesh-part visibility. Grouped named/exclusive outfits and attaching new accessory geometry are missing. Hiding existing geometry is not garment simulation. |
| Chibi proportions | Mate `AvatarHandlers/ChibiToggle.cs` | Avatar scale/placement exists; altering head/body proportions is absent. Requires reversible rest-space changes compatible with springs/retargeting. |
| Toon/anime rendering | Mate `Assets/MATE ENGINE - Shaders` | Native MToon/Lit/Unlit, cel presets, map strengths, outlines, expression colors/UVs and postprocessing. Unity Poiyomi shaders are not portable assets. No AO implementation or claim of visual parity. |
| FPS and loading efficiency | Homunculus `homunculus_power_saver/src/lib.rs`; Mate settings/optimization helpers | Focused/background/desktop FPS controls, asynchronous imports, cached bounded clips. No memory/CPU savings claim without measurement. |
| Tray and explicit recovery | Homunculus `homunculus_tray`; Mate `Assets/MATE ENGINE - System Tray` | Auxiliary native controls + radial recovery; closing controls now stays in Desktop. A Linux status-notifier/tray icon is still absent and valuable. |
| Multiple independent desktop characters | Homunculus per-VRM observers; Mate dance-sync/instance helpers | Scene roster exists; desktop drag/body animation currently belong to the selected avatar. Independent draggable desktop windows/controllers remain missing. |
| External agent control | Homunculus `homunculus_mcp`, `homunculus_http_server`, `homunculus_api`, CLI | `src/mcp.rs` is a bounded user-driven MCP CLIENT. It is not equivalent to an avatar-control MCP SERVER. A local explicit control API remains absent. |
| Mods and UI extensions | Homunculus `homunculus_mod`, TypeScript SDK/webviews; Mate mod SDK/Workshop | Local avatar/motion imports, appearance settings and explicit tools exist. Neither upstream mod format/CEF host/Unity bundles/Steam Workshop is supported. Native extension design is separate work. |
| Speech, mic and AI chat | Homunculus speech/audio/mic and voicevox/stt mods; Mate AI and LLMUnity handlers | Streaming providers, local-compatible endpoints, buffered/HTTP/WS/SSE speech, mic/VAD/STT and lips are already implemented. VOICEVOX-specific adapter and embedded LLMUnity runtime are not integrated. Offline-green does not establish physical lip sync. |
| Extras: food, particles, rich presence, games | Mate `AvatarFoodController.cs`, `AvatarParticleHandler.cs`, DiscordRPC, `Game APIs` | Missing optional extras; lower priority than accepted desktop behavior. Do not copy its sound/particle/art bundles. |

## Priorities after these fixes

1. Real desktop acceptance: drag/drop onto a window and taskbar, dismiss controls, transparent-margin click-through, move/resize/close the supporting window, cross-monitor drag. This is the gate before adding physics or autonomous relocation.
2. Touch reactions, then velocity-driven drag sway, using the existing facial/body priority controller.
3. Optional sleep/wake playlists and a tray/reopen action.
4. Named outfit visibility groups; optional media-session dancing with explicit settings.
5. Independent desktop companions and an explicit local avatar-control server if needed. Walking/gravity, mod runtimes and games remain larger projects.

No source-only inventory or offline unit test is presented as live compositor, audio or avatar acceptance. No builds or upstream services were run in either reference repository.

## Verification

The full offline suite passed: 583 tests (473 library, 99 binary, 6 integration, 2 + 3 example tests). After the final desktop motion-decision guard, all 99 binary tests passed again. Strict all-target Clippy, rustfmt on touched Rust files and `git diff --check` passed. Tests cover playlist suspension/resumption with manual pause retained, fresh/stale global-hover recovery without controls, and existing seat/drag/restoration paths. All Cargo work used one low-priority job. No live model request, new benchmark, external agent connection or physical desktop/audio acceptance was performed.
