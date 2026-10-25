# Validation receipt

## Preview 4 desktop transition — 2026-10-25

Native demo acceptance found that leaving a maximized Studio window could make the transparent companion invisible while its radial menu still rendered. Desktop mode now explicitly leaves the native maximized state, reapplies companion dimensions after the OS restores the old window size, and restores maximization when returning to Studio. The corrected maximized-to-Desktop transition was checked in the actual Windows window. Release application suites passed again: 226 Windows and 236 Linux/WSL tests. This is a separate correction discovered after preview 3 was packaged.

## Preview 3 settings, recall and motion — 2026-10-25

Settings now share the same row/card layout. Scene controls have one owner for appearance, camera, motion, face/gaze, behavior, world and desktop placement. Base shading, independent outlines and image effects can combine; PBR with outlines was checked in the native Windows preview. Duplicate model IDs, silence timing, recall switches, task proposals and group-chat membership controls were removed. Progress display/narration lives under Assistant tools. Native Windows checks visited all top-level menus and all seven Scene sections; the corrected profile gallery and restored idle playlist were also checked in the running app.

Default semantic recall uses Vercel `openai/text-embedding-3-small` over explicit notes and accepted evidence. Automatic send waits for the current query, with a bounded fallback; pending suggestions never enter recall. Evidence embeds its meaningful text while retaining provenance in the recalled context. An isolated live check returned both synthetic sources through the production Vercel worker in 904 ms. The configured decision model chose a greeting from 159 eligible stationary motions in 588 ms. These timings describe one run, not latency guarantees.

Bundled playlist and emotion-binding paths now resolve across installation roots and Windows/Linux separators. The full stationary catalog is available to the decision model; automatic locomotion remains excluded. Appraisal can start a gesture while the reply streams, followed by talking during speech and idle afterward. The local authored-material/spring baseline was restored, but collision-free hair or clothing across every retargeted animation is not established.

Release-mode tests passed 969 library and 226 application tests on Windows, and 1,106 library and 236 application tests on Linux/WSL. Subsequent menu-only checks passed the application suites again, and both preview-3 executables built. The conversational tool loop remains off by default. Live browser MCP, physical microphone/global-hotkey acceptance, native Linux compositor behavior and all-motion collision acceptance remain open.

## Preview 2 integration — 2026-10-25

The native conversational tool loop is implemented separately from the reviewed coding-task loop and remains **off by default**, including the existing local Hikari profile. Disabled chat sends one normal streaming request with no tool definitions. Enabled requests freeze the selected tool grants, expose native function schemas, execute bounded sequential calls, return assistant/tool-role results, and request a final answer when the call budget ends. Unknown tools and malformed arguments produce results without process execution. Configured MCP grants bind the loaded server address; explicit tool selection and cancellation are covered by a loopback server test. Real browser MCP operation remains unverified.

Live Windows acceptance through the production chat worker, using the configured DeepSeek V4.1 Flash/Vercel route, passed all three scenarios: Tavily search plus page extraction returned a sourced endpoint answer in 7.62 seconds (one search, two extraction calls); Hikari delegated a workspace marker read to Claude in 5.11 seconds and to Codex in 25.12 seconds, then relayed the exact marker. These were explicit isolated tool-enabled tests; the normal profile was not enabled by them. CLI adapters use existing local authentication and frozen read-only workspaces/model overrides. Full T3 sessions, automatic file editing and unsolicited background work are not claimed.

The original **167** VRM-renamed Kimodo Waifumations BVHs are bundled with generation prompts and source hashes. Both platforms parse every file through the production importer and discover idle/talking playlists. Separate 90-frame offscreen renderer runs on Windows (Hikari wardrobe) and Linux/WSL (AliciaSolid) traced idle → talking → joy gesture → talking → idle, with speech/mouth state and camera checks. Frame 30 from each run was visually inspected; this does not establish collision-free motion across all clips or avatars. Rejected clearance experiments and Kimodo runtime/weights are excluded.

Bundled Silero 6.2.1 and ONNX Runtime 1.24.4 passed real CPU detection on both platforms using the same generated offline speech WAV: 328 frames, 245 speech frames and silence probability 0.00167. Pinned download hashes, upstream licenses and platform-specific runtime packaging are checked. Pause/manual-stop settings, PDF/DOCX/text extraction, native file-drop intake, persistent agent settings and per-binding Windows global-hotkey conflicts have automated coverage. Physical microphone/background-keyboard acceptance and GUI attachment-send acceptance remain open. Computer Use was stopped by the user; no subsequent desktop-input acceptance is claimed.

Release all-target checks passed **1,228 tests across 50 targets on Windows** and **1,416 across 50 targets on Linux**. After the Windows socket fixture was made explicitly blocking, all four native tool-provider regressions also passed on Linux. Seven Python package-integrity tests passed; formatting and whitespace checks passed. Both release executables built. An isolated 1440×1000 Windows offscreen render of Assistant tools was visually inspected: loop off, Tavily switch, independent Codex/Claude switches, call budget and MCP server controls are visible without overlap. This is a rendered UI check, not desktop-input acceptance.

A fresh Linux profile also passed live Tavily search plus page extraction through the current release chat worker in **9.80 seconds**, with one search and two extraction calls. The first selected page could not be extracted; Hikari read another official documentation page and stated that limitation in its final answer. Existing environment credentials were passed privately to the isolated check; no credential file or personal conversation was included in its profile. Linux Codex/Claude login and real browser-server tests were not run.

The previous hosted Windows/Linux run completed native build/test/package steps but failed its temporary artifact upload because the account's Actions artifact quota was full. Temporary Actions uploads are now an explicit manual-workflow option with one-day retention; push/PR checks still build and test both platforms. Release archives are published separately as GitHub Release assets.

## Windows and Linux port — 2026-10-24

Native MSVC Windows x64 and Ubuntu 24.04 in the isolated local `WaifuStudioTest` WSL distribution compile with pinned Rust 1.95.0. Release all-target tests passed **1,207 on Windows** and **1,395 on Linux**, using `--test-threads=1` because animation tests share one process-wide importer. A subsequent Linux run passed **18 credential tests**, including the additional supported-provider startup check. Both native release executables built. Five packaging regression tests passed. The new hosted Windows/Ubuntu workflow has not been dispatched.

Windows acceptance loaded Hikari's bundled VRM in the real Studio window, switched to a transparent borderless desktop companion, opened the radial menu and returned to Studio. A title-query deadlock found during that check was removed: the Windows bridge now receives the actual primary HWND from winit and reads native move state without synchronous title messages. Geometry uses DWM bounds, taskbars and one consistent DPI coordinate space. Mixed-DPI/multi-monitor arrangements, taskbar seating and global-shortcut behavior still need their own interactive acceptance.

The installed offline Discord replay passed through the actual Windows process supervisor into the application's persisted history, with attributed input, opt-in defaults, no model work, unchanged memory and verified cleanup. Windows Python launchers now wait for their native child; `os.execv` would otherwise exit the supervised Windows parent and cause its Job to terminate the child. Dedicated Windows fixtures passed Unicode/space paths, literal input, descendant cleanup and full-stdin timeout behavior. Memory persistence was repaired to use Windows file replacement rather than Unix directory fsync; the existing notes/facts/semantic-memory tests pass.

Linux offscreen acceptance rendered AliciaSolid with llvmpipe Vulkan. The default Hikari wardrobe model exceeds this software driver's 128 MiB storage-buffer-binding limit; it rendered on the Windows hardware GPU. Native Wayland/KWin/Hyprland desktop behavior was not re-tested in this WSL environment. The pre-existing Vulkan/X11 fallback to Studio remains in place. Linux voice-worker tests used Node 22.23.4 and the locked dependencies; its native Opus module was built in a path without spaces because upstream node-gyp lost an include path in the mounted checkout.

The local default Windows profile was populated with the user's existing Vercel, Fish, OpenAI and OpenRouter credentials. Values are not recorded in this receipt or copied into runtime packages. The startup allowlist now supports all configured built-in chat providers on both platforms. No live chat/speech/provider requests were used as acceptance evidence, and physical audio remains a separate check.

Commands and local logs: `work/cross-platform/test4.{out,err}.log`, `linux-test4.{out,err}.log`, `linux-capture2.err.log`; source launchers are `run.cmd` / `run.ps1` and `run.sh`. See `docs/CROSS-PLATFORM.md` for dependencies, build, packaging and remaining acceptance boundaries.

## Explicit click-through and direct window dragging — 2026-10-23

User testing rejected the previous input behavior: automatic margin click-through still disabled native input even though the manual "Pass clicks" setting was false. Native hit testing now stays enabled throughout the companion window unless the user explicitly selects full passthrough and its recovery surface is ready. Dragging and the right-click menu no longer depend on a CPU raycast against an animated mesh. A left-button press delivered to the companion window starts its drag; presses delivered to auxiliary windows cannot start it. Global desktop pointer sampling remains for gaze and placement, not for turning native input off.

## Companion-only startup correction — 2026-10-23

The prior launch used Studio mode, so its side panel was visible and desktop dragging was disabled. Normal application startup now selects the draggable Desktop companion. Full Studio is explicit (`--studio`, a requested settings/screen/history/room/capture workflow, or an action from the right-click menu). `--desktop` remains an explicit override. Offscreen UI capture keeps its Studio behavior.

Entering or returning to Desktop no longer creates an auxiliary controls window. That recovery surface is requested only by the explicit right-click "Pass clicks" action; its request is session-only and never saved. Settings remain accessible from the right-click menu. Escape dismisses the menu without silently opening Studio. Regression coverage checks default mode selection, absent controls at startup, restored input after closing explicit controls, no automatic reopening after returning from settings, and Escape. **208 application tests**, strict all-target Clippy, package formatting and whitespace checks passed. Real pointer movement remains a manual acceptance check.

The corrected optimized build was launched by command line. Its current service invocation remains active, reports exactly one native window and a loaded VRM, and has no logged auxiliary controls window, startup panic, shader-processing failure or error line. This verifies companion-only startup; physical dragging is left for the user's check.

## Desktop dragging and window occlusion — 2026-10-23

Native drag status now distinguishes unavailable information from an observed end. Hyprland uses the existing compositor bridge with the ordinary pointer grab, preserving button-release delivery; KWin retains its observable native drag. New and legacy-default desktop settings enable detection on either supported compositor, preserving explicitly disabled current profiles.

Window occlusion is enabled by default and has one switch in the existing Desktop controls. A depth-only pass runs before avatar rendering: the supporting window sits just behind the projected pelvis, and windows above that support in the desktop stacking order hide intersecting geometry. Front-facing limbs remain visible. Work-area seats mask only the panel below the usable floor. The bounded mask uses up to 16 intersecting rectangles; stale/missing geometry, standing, dragging, mode changes and disabling occlusion clear it. Input follows the explicit click-through policy described above, independent of the visual mask.

The production renderer passed headless NVIDIA/Vulkan pixel checks with MSAA off and 4× MSAA: red geometry behind a simulated window disappears into transparent pixels, green geometry in front remains visible, and the unmasked baseline stays visible. Reproduce with `cargo run --offline --locked --example verify_desktop_occlusion -- 1 /tmp/desktop-occlusion-msaa1.png` and repeat with `4`. No desktop window, live provider, microphone, or saved character profile is used by this fixture.

The optimized application build, strict all-target Clippy, package formatting and whitespace checks passed. The final offline all-target suite passed **1,364 tests** with `--test-threads=1` (1,081 library; 206 application/UI; 77 integration/example tests). An earlier parallel run hit the pre-existing shared-stream cancellation fixture's "Previous connections are still closing" timeout; that exact test passed immediately alone, then the entire serial suite passed. Initial sandbox-only fixture failures were local socket/special-file permission errors; the final suite ran with those fixture permissions. No provider credentials or live-service calls were used by these automated checks. Build SHA256: `a0c2d4e90d80023e1f4c1dc47a182decc82a8fe9a5914b666fd0e8d9555adba4`.

The release was launched by command line through the existing desktop user session (`waifu-studio-desktop-20261023-occlusion.service`). Process/window creation and active service state were confirmed, with no startup panic or shader-processing failure. The saved profile still logs a missing background image under an older `work/waifu-studio-ui/` path; that is separate from the transparent desktop depth pass. No visible compositor/model acceptance is claimed from this launch check.

This establishes the shader/depth path, not visual acceptance of a real VRM on the live compositor. Window masks are rectangular and do not reproduce rounded corners or translucent window contents. Pelvis-relative depth still needs the user's model/contact check; this is separate from avatar self-collision. Manual acceptance: hold a drag longer than one second; release onto a window/taskbar; move/resize the supporting window; overlap it with another window; grab the character again and check that the mask releases.

## Earlier validation

2026-10-19 Waifumations: the first clearance video was rejected because upper arms still crossed the torso. The mesh-derived replacement also remains rejected: clear sampled axes can coexist with unacceptable arm jumps. Correction stays experimental/off. Three explicit arms-at-side prompt trials were generated separately; the talking trial has no arm-axis/selected-torso crossings in 180 source frames, but selected surface intersections remain in 70 of 91 native samples. Idle and shrug fail the axis screen too. All remain unapproved. See [the review receipt](reviews/waifumations-arm-clearance.md). 605 offline tests, strict all-target Clippy and package formatting passed; these do not establish visual acceptance or complete collision protection.

Date: 2026-10-18; earlier GPU observations are identified below. Linux KDE/Wayland, NVIDIA GeForce RTX 5060 Ti, NVIDIA 615.71.09, Vulkan. Rust 1.98.1. These observations apply to this source/build, not all devices or imported avatars.

## Compiled and automated checks

New-setup speech defaults (2026-10-18): fresh libraries and added Vercel presets select `deepseek/deepseek-v4.1-flash`, explicit reasoning None and `routing.only = ["baseten"]`; fresh hosted speech selects Drama 3, HTTP WAV and Low latency. S2.1 Pro remains selectable. Existing explicit saved profiles and legacy field fallbacks remain unchanged. **578 tests passed** using `cargo test --all-targets --quiet -- --test-threads=1` (472 library, 95 application/UI, 6 inspection, 5 example tests); strict all-target Clippy, package formatting, whitespace check and executable build passed. The new regression checks startup choices and saved-choice round trips, while the Studio test checks startup HTTP and explicit WebSocket/SSE selection. A preceding parallel suite hit the shared chat-worker cap in a scene-chat fixture; the complete serial run passed. No new live benchmarks, live-provider requests, current-profile changes or GUI restart were performed; historical latency results remain the evidence for selecting these defaults.

Latest reply timing integration (2026-10-18): **577 tests passed** (`cargo test --all-targets`: 471 library, 95 application/UI, 6 inspection, 5 example tests), strict all-target Clippy with warnings denied, package formatting, whitespace check and the normal binary build passed. An existing loopback streaming/cancellation test now verifies worker → request → headers → first raw text ordering, and the Studio send fixture checks preparation/return stamps and history-save duration. Two new deterministic tests verify first-write-wins stamps, cancellation and per-turn isolation. The composer **+ → Latest reply timing** shows one latest in-memory receipt; it stores no text or credentials and writes nothing to disk. First-text layout is stamped only when a corresponding history label intersects its clip region; this is not GPU presentation or physical audio onset. The timing menu itself has not been interactively accepted. The synchronous history save is an identified possible UI stall; its contribution to the user's >1-second live delay remains unmeasured. No live provider call or live-app restart was performed.

Settings layout integration `c4fc60e` (including `78d81c6`, merged with `894bf20`): **575 tests passed** via `cargo test --all-targets` (469 library, 95 application/UI, 6 inspection, 5 example tests). The switch-label interaction regression is fixed without weakening its existing click-through test. Strict all-target Clippy with warnings denied and package formatting also passed. The normal development binary built successfully. Isolated 1280×800 offscreen captures of expanded [Voice](work/ui-shots/advanced-voice.png), [Memory](work/ui-shots/advanced-memory.png), and [Scene](work/ui-shots/advanced-scene.png) were visually inspected. The Scene capture precedes the final motion-card wrapper; other pictured settings match this integration. Captures used a separate synthetic profile with empty keys and loopback endpoints. The user's running app and profile were not restarted or modified. This verifies sampled layout and offline behavior, not live-provider latency or physical audio acceptance.

Render snapshot `6002c64`: the optional linear RGB correction compiled and rendered Neuro with the cel look on the Vulkan RTX 5060 Ti; [capture](previews/neuro-rgb-correction-preview.png). Seven visual tests passed, including old-profile defaults, bounded channels, selected-camera isolation and transparent-mode removal; strict all-target Clippy passed. This verifies the native shader path, not pixel-for-pixel three.js parity. Snapshot `9cdfdf6`: seven gaze tests passed, including viewport-relative pointer normalization, direction/limits, smoothing, and existing avatar/rest-pose isolation tests. Physical pointer/desktop acceptance remains open. Concurrent uncommitted speech/memory work is not covered by these receipts.

Integration snapshot `3d003c9`: **452 tests passed** (364 library, 82 controller/runtime/UI, 6 inspection), strict all-target Clippy and package formatting passed. The microphone cache fixture verifies same-configuration reuse, model/runtime replacement, eviction on replacement failure, and independent state ownership. Silero resets recurrent/context buffers before each recording; no real model inference, microphone hardware timing or end-to-end speech latency improvement was measured in this change. Silence presets preserve the existing value until explicitly selected.

Integration snapshot `920a374`: `cargo test --all-targets` passed **451 tests** (363 library, 82 controller/runtime/UI, 6 inspection), package formatting passed, and strict all-target Clippy passed. New coverage includes Vercel reasoning capability parsing/validation and Hyprland geometry fixtures plus fake-socket movement. The composer selection test opens the actual nested reasoning dropdown, chooses High, and confirms model/settings persistence and draft preservation. Desktop tracking now selects KWin or Hyprland through the same bridge interface; no live Hyprland or physical desktop input acceptance is claimed. Current-session inspection found KDE/KWin Wayland, correcting the audit's earlier Hyprland assumption.

Integration snapshot `cd6fcc5`: `cargo test --all-targets` passed **443 tests** (355 library, 82 controller/runtime/UI, 6 inspection). This includes deterministic reply-emotion motion bindings and stale automatic-pick cancellation. Snapshot `04cb63f` additionally passed the mocked HTTP request test for history → recalled memory → latest user ordering, package formatting, and strict all-target Clippy. Concurrent follow-up edits require their own receipt; these results do not validate later worktree changes.

- `cargo build --bin waifu-studio`: passed, development profile.
- `cargo fmt --package waifu-studio -- --check`: passed.
- `cargo clippy --all-targets -- -D warnings`: passed.
- Earlier baseline `cargo test --all-targets`: **377 passed** (315 library, 56 controller/runtime/UI, 6 inspection; example targets have zero tests).

Tests cover VRM generation/metadata/container checks, malformed inputs, failed replacement and unload cleanup, material conversion/restoration and weak-cache release, profile isolation/persistence/duplication/removal, connection index remapping, secret exclusion, bounded history/serialization/worker counts, SSE Unicode/truncation/limits/cancellation, mocked HTTP chat/model data, Fish hosted/local/OpenAI speech request bodies and error handling, BVH channel order/interpolation/mapping/rest orientation/avatar isolation, control-window recovery logic, image size/format/metadata preparation and multimodal message shape, energy VAD/WAV/multipart transcription, microphone disable/stale-result cancellation, per-character note isolation/persistence/deletion/retrieval bounds, MCP current/legacy fallback, persistent request IDs, cancellation headers, error classification and immutable tool-result provenance.

Additional tests cover CLI argv review and output/cancel/timeout bounds, SSH argument quoting (no remote connection), strict task proposals, bounded-loop state transitions and job ownership, preservation of edited manual drafts, bounded head/tail worker receipts, incremental fragmented WAV decoding, stereo alignment during concurrent publication/underruns, playback before HTTP EOF, Voice Design request/response/cancellation fixtures, and scene switching/suppression/asset release.

Targeted vendored tests separately passed: **two MToon outline tests** (shared mesh cleanup and morph synchronization) and **two spring-step tests** (fixed-step counts and stall bound). The tests used copied production source in a scratch fixture with dependency dev-test configuration removed. A full upstream suite was not established: two upstream model tests require Alicia assets absent from the vendored package. No full-upstream-pass claim is made.

The composer regression drives egui pointer events through the actual model menu, selects a fetched model and High reasoning, and verifies that both persist while the multiline draft remains unchanged. Provider tests verify old-profile migration and omission/serialization of the optional reasoning request field. This is simulated UI input, separate from physical compositor input.

The native desktop recovery test drives Bevy ECS state: passthrough remains disabled without the control context, and closing controls restores hit testing and removes the controls/camera. It does not simulate physical compositor mouse delivery.

## Latest integrated additions

The current build also compiles/tests:

- Avatar part visibility isolation, original-state restoration, persisted stable keys, duplicate-name handling and bounded discovery. Part controls exclude outline meshes and other scene objects.

- VRM0/1 morph-expression bindings, blink curves, bounded delayed-scene discovery and cleanup; buffered/progressive audio RMS measurement from decoded samples; generation-safe stop, EOF/drop and stale-thread behavior. Character profiles retain independent facial settings and speech transport.
- VRMA linear/step/cubic rotation interpolation, source-bind correction and malformed data; ASCII/compressed binary FBX, axis correction and local reference clips when present. Playback weight remains effective while paused.
- Task-board overlap, batch review isolation, cancellation, worker teardown, bounded retention, UTF-8-safe head/tail receipts and preservation of pending output when the composer is full.
- Semantic query freshness, note revisions, character isolation, vector caching, remote opt-in, endpoint failure backoff and malformed embeddings via local fake HTTP servers.
- Fish timestamp SSE framing, unknown-length WAV split across events, playback before EOF, finite-WAV trailing alignment, cumulative timing replacement, size/error bounds and cancellation via a fake loopback HTTP server. This original fixture pass did not make live synthesis calls; subsequent authenticated synthesis benchmarks are documented below and in benchmarks/RESULTS.md.
- Hosted Fish WebSocket MessagePack framing, fragmented progressive WAV, timestamp replacement, bounded text/audio, explicit proxy handling and cancellation using local fixtures. Streamed reply filtering tests cover every split of the task-proposal opening marker, ordinary Markdown, Unicode and bounded text. Studio tests cover speech queue replacement, character-switch cancellation, streamed task filtering and prevention of speech restarts after Stop.
- Native calibrated MFCC analysis: 17 core tests, including 30 numerical vectors from pinned upstream C kernels adapted to native float math, five rates, three scoring methods and standardization on/off. Profile parsing supports uLipSync JSON and wLipSync WLIP with malformed-input bounds. This is not unmodified WASM/Unity bit parity or speech-recognition accuracy evidence; see LIPSYNC-NOTICES.txt for pins, licenses and deliberate differences.
- Playback-to-vowel bridge: nine tests cover consumed-sample routing, incompatible-rate RMS fallback, profile/character generation changes, EOF/cancel cleanup, stale output, tuning and bounded-ring overflow recovery. Synthetic calibration fixtures do not establish real-voice vowel accuracy. No voice calibration is bundled.
- Local behavior controller and lifecycle integration: explicit opt-in, Idle/Thinking/Speaking transitions, manual emotion smoothing/decay at 15/30/60/120 FPS, bounded outputs, stale/wrong-character events, reset/cancel/disable cleanup, protected facial morphs and manual-expression priority. Strict opt-in reply emotion/VAD metadata is now connected for the focused avatar; raw-chat sentiment inference and a general Jev embodiment appraiser remain absent.

Five additional streaming-budget regressions use short budgets: healthy playback exceeds the network budget without failing; a slow network still expires; an undrained consumer fails and clears audio; continued consumer progress cannot bypass the absolute cap; cancellation interrupts a full queue promptly. The HTTP WAV, SSE and WebSocket paths share this budget. This tests the timing mechanism without a 90-second real-service recording. The existing HTTP/SSE per-read timeout can still delay worker exit for up to 90 seconds after cancellation or budget exhaustion.

Additional checks cover Fish hosted chunk-control defaults/validation, WebSocket error redaction and concurrency-limit reporting, fake-server voice-library query/filter/pagination/selection/cancellation, gateway pin/model normalization, and editable bundled defaults. All 22 bundled models pass the VRM metadata inspector. Bundle tests cover exact stored text, stable IDs, edits/deletions across reload, path relocation and path traversal rejection. The three shipped persona strings were independently compared byte-for-byte against the recovered personality inventory. This does not establish visual/rig compatibility for every bundled model.

Latest checks also cover gateway-specific provider order/allowlists/cost/latency/throughput request shapes, reasoning options, preservation of exact model suffixes, and filtering non-chat catalog entries. Direct Fish multipart, Vercel transcription JSON/headers and compatible OpenRouter/local transcription use separate tested adapters. No authenticated Vercel or OpenRouter request was made; this process has no Vercel gateway credential available. This is not a statement about account entitlement.

Multi-character tests cover independent loading/removal/failure isolation, speaker-targeted morphs, preservation of focused settings, FIFO speech/cancellation and focused Voice Design ownership. A real loopback HTTP group conversation drives submit → SSE → poll for two recipients and verifies independent personas, notes, evidence ledgers, histories and speech order. Jev recipient choice uses mocked evaluation responses and validates offered UUIDs and stale suggestions. Membership/placement persistence tests cover bounded versioned files, stale IDs, startup edits, malformed-file preservation and shutdown draining. New multi-character rendering has not received GPU acceptance.

Evidence-ledger tests cover opt-in, source/evidence retention, candidate acceptance, duplicate rejection, correction chains, expiry, physical content removal from the active file, read-only scoped recall and malformed/oversized input. Forgetting a correction cannot revive its obsolete ancestors. Focused-request HTTP coverage checks that retrieved notes/evidence occur once and stay out of stored human turns. Reply-affect parser tests cover stream splits and malformed/oversized blocks; a real ECS integration sends provider events through studio polling and behavior, verifying hidden metadata never reaches pending speech/display/history while a supported expression receives a drive. An audio-gap regression preserves the same behavior epoch until speech actually finishes. This does not establish perceived expression or lip-sync quality.

Optional Silero ran real CPU ONNX inference using `examples/check_silero.rs`: two fresh sessions, each with 32 silence windows, both maximum probability **0.008911**. The official model SHA256 was `1a153a22f4509e292a94e67d6f9b85e8deb25b4988682b7e174c65279d8788e3`; native ONNX Runtime was **1.31.0**. This checks real inference, tensor contracts and repeated session initialization, not microphone input or speech-detection accuracy. The model and runtime are not bundled.

These additions have **automated/offline evidence**, not fresh GPU, microphone, paid-provider or perceived lip-sync acceptance. The previous GPU captures below predate this integration. Current development binary build and strict Clippy pass; no release benchmark was run.

## Earlier GPU execution

The earlier baseline application rendered and saved captures, with successful exit and no shader/error/panic entries:

- Local user's Riko VRM 0.0 with MToon, PBR and unlit modes.
- AliciaSolid VRM 1.0 from the inspected reference checkout.
- Generated minimal GLB fixture through ordinary glTF scene loading.
- Transparent desktop mode with the separate controls window created.
- A real local WebWaifu4 `surprise.bvh` clip retargeted onto `rikov343.vrm`.
- Chat, Voice, Memory, Tools and Tasks panels, plus procedural-scene controls.
- Three real procedural environments: Cozy room, Moonlit garden and Neon rooftop.
- All three generated illustrated backgrounds through the actual camera backdrop material.
- Desktop mode with a selected environment and image: both are hidden and the alpha background is preserved.
- Connections panel and simple image-background fixture were captured in the earlier base pass.

`desktop.png` is RGBA with its corner pixel `(0,0,0,0)`: renderer output transparency is verified. `studio.png` has an opaque background. `screenshots/` contains actual renderer captures, not design mockups. The earlier capture runs referenced original model files. This later local package now includes the requested 22-model character bundle and three attributed Homunculus desktop VRMAs; the separate WebWaifu4 motion library remains external.

The first GPU pass uncovered incompatible Bevy 0.20 WESL shader imports. The final vendored MToon/outline shaders use Bevy 0.19 WGSL imports and interfaces; the successful captures above were produced after this correction. Unused WESL compilation is disabled in the final app manifest.

## Practical limits

- Native UI physical input testing was attempted, but desktop inspection repeatedly reported that the user's desktop changed during inspection. The requested automatic launch was also refused because the KDE physical-input activity guard is not installed. A later inspection still reported desktop changes. No guard was bypassed; the refreshed build has not been opened on the user’s desktop by this integration pass. Topmost/click-through behavior under the actual compositor and full mouse/keyboard acceptance remain unverified; renderer transparency, native-window creation and ECS recovery are separate verified observations.
- Chat/TTS protocol tests use local fake servers. Separate bounded live checks verified Vercel streaming and Fish read-only authentication below. No paid Fish synthesis, Voice Design, microphone recording, live STT/embeddings/MCP, real coding-agent model task, SSH host or perceived audio quality is claimed.
- The earlier BVH screenshot establishes baseline visual playback. New offscreen captures below additionally establish rendered bundled VRMA poses on Neuro. These observations do not establish universal artist-rig compatibility, locomotion, constraints or physics interaction. FBX supports the first stack and one baked layer; cubic easing is approximated linearly.
- Progressive HTTP WAV, hosted timestamp SSE and hosted WebSocket text-in/audio-out (with optional timestamps) are implemented. Word timings remain metadata rather than mouth shapes or synchronized caption highlighting. Optional calibrated MFCC vowel analysis is a separate playback-sample path with RMS fallback; live calibration accuracy and perceived lip-sync are unverified. Voice Design candidates are previews, not saved reusable voices. The bounded agent loop is sequential; the separate reviewed task board runs concurrent jobs, without persistent multi-agent session/steering parity with T3.
- 60/30/15 FPS are configurable caps. No sustained CPU/GPU benchmark, long-duration memory-leak soak, release-build performance, thermal/power measurement, or arbitrary-asset memory guarantee was run. Capture timings include model load, shader preparation and an intentional five-second wait; they are not frame-time benchmarks.
- Outline meshes are shared through weak ownership and generated with render-world-only usage. Normal smoothing still runs synchronously on first creation, so very large unique meshes can cause load hitches. Spring work uses bounded fixed substeps. The current main-thread FPS limiter can still delay input at very low caps; sustained latency needs profiling.
- GPU/driver allocations can remain reserved after strong handles are released. Unit tests check ownership/entity cleanup, not driver process RSS reclamation.
- Windows/macOS/Quest hardware, VR/OpenXR, general Luna/OpenAI Decisions adapters and fly-connectome integration are unverified or deferred as documented. The scene-recipient Jev adapter has offline protocol evidence only. Optional semantic notes and the evidence ledger are implemented; live embeddings, model-driven extraction, automatic reflection and Grillo promotion remain unverified or absent. Reply moods are session state; configuration persists. Research documents describe options, not shipped integrations.

## Desktop, room and visual integration (2026-10-18)

Private credentials load before worker/renderer initialization. Tests cover the allowlist, strict parsing, size limits, symlink/ownership/0600 checks and process-environment precedence without exposing real keys. The private file stays outside the source package.

Actual authenticated provider observations: the native Vercel adapter streamed exact `OK` with `openai/gpt-4o-mini` (16 output-token cap) and with the app's default `openai/gpt-5-nano` (128 output-token cap). Read-only authenticated Fish `/model?self=true&page_size=1&page_number=1` returned HTTP 200. No OpenRouter requests were made. These observations verify these paths with this account at that time, not all models/routes or Fish synthesis.

New portable seated-pose tests verify arbitrary rest axes, anatomical forward under the loader's rotated root, exact restore, and isolation. Monitor-placement tests cover scale conversion, negative origins, bounded sizing and explicit one-shot positioning. These are offline/ECS results, not compositor acceptance. Homunculus's macOS/Windows external-window tracking was inspected; no equivalent Linux backend was found or ported.

Visual controls preserve old-scene defaults, sanitize settings and restore camera baselines; GPU-free tests check camera isolation and transparent-mode suppression. Real headless Vulkan rendering uses the actual room meshes, native VisualPlugin, MToon avatar loader and optional seated-pose plugin. The preview example creates no desktop window. Render inspection caught and corrected shadow crushing in the Neon/Cinematic contrast presets.

The wall-art PNG was generated with the built-in image generator; the prompt/provenance is in `assets/rooms/ART-NOTES.md`. Architecture and furniture are actual procedural 3D geometry. Room previews are renderer outputs, not generated room images.

Opus room pass was integrated and reviewed through actual GPU captures in `previews/rose-bedroom.png`, `previews/violet-studio.png` and `previews/cyan-lounge.png` (1280×960). It adds a bed canopy/drapes, wall/shelf detail, gaming accents, a larger hex arrangement, lounge furniture and stronger fill lights. The worker used written reference descriptions and earlier previews; root separately opened the original reference images. This is a procedural furnishing pass, not reference-quality art acceptance. The final preview camera uses 45° FOV; warmup is 180 frames and an empty clear frame is retried to avoid accepting an uncompiled GPU pipeline as a successful room render.

Earlier room/desktop integration: 282 tests passed; strict Clippy, native binary/example builds and package formatting checks passed. These checks do not change the compositor/physical-input limitations above.


## Menus, model catalogs and Vercel speech update

The current binary builds, strict Clippy and package formatting pass, and all 308 tests pass. The added UI regression searches and selects model 499 in a 500-model catalog; the existing composer interaction regression still verifies selection, reasoning and draft persistence. New offline tests cover catalog debounce/credential changes/modality filtering, non-repeated failures, and Vercel v4 speech wire format, bounded decoding, cancellation and sanitized errors.

Settings now fill the available window width and height, with labeled navigation, full scrolling pages and section jump controls. Speech/transcription catalogs are expanded searchable lists. Fish includes all five documented hosted model IDs. LLM catalog requests run once per selected connection/credential change and retry explicitly; stale request generations are discarded. Keyboard-driven connection edits debounce for 400 ms. Speech discovery has bounded workers and the same debounce. Discovery never generates text/audio.

Live observations from `cargo run --example check_model_catalogs -- --speak-vercel`: Vercel advertised **320 chat, 13 speech and 10 transcription models**. One five-character `Test.` request using the native v4 speech adapter and `openai/tts-1` returned **21,044 bytes WAV**, decoded by the native audio library into **10,500 samples**. It was not played aloud. Fish's documented five-model picker was checked separately without synthesis. No OpenRouter call was made. Counts are provider snapshots, not fixed catalog sizes or proof of every voice/model combination.

These UI changes have source/build/egui-interaction evidence; the revised full desktop layout has not been visually accepted in the running app. Restart the app to load this binary. Logs: `work/menu-model-tests.log`, `work/menu-model-build.log`, `work/menu-model-clippy.log`, `work/menu-model-live.log` in the parent workspace.

## Streaming defaults and caption removal

All 300 tests pass after the streaming-default upgrade. The new regressions cover provider-supported delivery defaults, preservation/idempotency of the live-pipeline action, and a saved version-1 library upgrading once while preserving model, reference, speed and enabled state. Subsequent explicit buffered choices survive reopening a version-2 library. The desktop recovery regression now verifies native transparency/explicit alpha, decoration/shadow suppression, and restoration. Vercel speech fixtures use the buffered Vercel preset explicitly. The floating response-caption render path has been removed; Chat history remains accessible. Native build, strict all-target Clippy and package formatting pass. No additional authenticated synthesis or GUI compositor acceptance is claimed for this change.

## Streaming audio crackle correction

The old decoder emitted silent frames whenever its per-frame mutex try-lock failed, including when PCM was buffered. The streaming queue now publishes complete PCM frames through a bounded atomic single-producer/single-consumer ring. Producer publication cannot cause artificial silence or move stereo channels out of alignment. Cancellation/failure invalidates the source immediately without resetting the consumer index from the producer thread. A concurrent 50,000-frame regression verifies exact stereo order through repeated ring wraparound, and a completion/publication regression verifies final-frame delivery; all 264 library tests and the native binary build pass. This corrects a confirmed source of clicks; real network underruns, provider audio quality and physical audio-device playback remain separate checks.

## Opus menu, personality and motion integration

All 308 tests pass (264 library, 38 binary/runtime/UI, 6 inspection). The native binary builds, strict all-target Clippy and package formatting pass. The new UI tests verify settings growth beyond the old 1080-pixel cap, original-personality matching and custom-text replacement behavior, and motion row selection invoking the loader. Catalog tests verify the three exact prompts against the original bundle, exact character/model matching, and grouped motion discovery excluding symlinks. The local motion browser reads the existing WebWaifu4 library in place; no motion assets or private user settings are added to the source package. These are code and egui interaction checks; the new full-page layout still requires user desktop acceptance.


## Homunculus retargeting and desktop interactions

The preceding integration passed **337 tests** (289 library, 42 binary/runtime/UI, 6 inspection). Native build, strict all-target Clippy, and package formatting checks pass. `node tests/kwin_surfaces.cjs` additionally passes six fake KWin scenarios. No live compositor script was executed during verification.

Retargeting was compared against desktop-homunculus’s pinned bevy_vrm1 0.7.1 implementation. Tests cover arbitrary bind axes, source/target model-rest hips-height scaling, translated/rotated/scaled application roots, VRM0/1 facing, VRMA LINEAR/STEP/CUBICSPLINE translations, FBX hips translation, BVH frame-zero deltas, and the actual bundled sitting clip. An equal-revision animation-resource swap regression protects desktop entry/exit. Clip selection crossfades for 0.5 seconds; rapid switches continue from the last visible mixed pose. The desktop host restores the complete previous user animation state on exit or disabling automatic motions.

The three unmodified CC BY 4.0 Homunculus VRMA assets are attributed in `assets/animations/desktop/ATTRIBUTION.md`. `examples/render_desktop_motion.rs` uses the production VRM loader and retargeter, fixed sample time, and a genuine windowless Vulkan render target. Captures in `previews/desktop-idle.png`, `desktop-grabbed.png`, and `desktop-sitting.png` show the bundled Neuro at 0.5 seconds with transparent corners. Still poses do not prove a full motion loop or mouse-driven transitions.

Desktop mode now requests a 420 × 640 primary window and restores the prior Studio resolution/position. Its radial menu provides settings, character selection/import, voice settings, sit/stand, passthrough, Studio and framing actions. Native compositor dragging starts only after an avatar mesh hit, and menu clicks suppress starting a drag. Portable sitting fixtures cover drop bands, stacking/occlusion, stable IDs, resizing, hips anchoring and disappearing windows.

KDE Wayland external-window sitting is **experimental, with Automatic detection now the default on compatible sessions**. The legacy disabled default migrates once; explicit Disabled choices in the current settings schema survive reloads. Its temporary KWin bridge provides bounded, authenticated session-D-Bus geometry snapshots, per-output taskbar work areas, native-drag state and PID/title-scoped movement. Startup/unload run outside the frame loop. Offline Rust/JS fixtures check parsing, visibility, ordering, process scoping, bounded placement and crash cleanup. Native Wayland/KWin acceptance, physical input/passthrough and dynamic taskbar following remain unverified. Other compositors have no new external-window backend.

## Fish live WAV compatibility

A real short hosted Fish WebSocket request exposed RIFF/data placeholder sizes `0xffffff24` / `0xffffff00`. The joiner now recognizes only that exact standard 44-byte header pair as a stream. Other finite lengths, incomplete PCM frames, and socket closure without normal finish remain errors; offline regressions cover all cases. A single post-fix request through the configured proxy completed: 86,060 audio bytes, three wire audio frames, first audio 1,038 ms, and 35,522 nonzero decoded samples. This verifies reception/decoding with the saved hosted model/voice. It does not verify physical playback, crackling, calibrated lip sync, or sustained latency.

The HTML/Chromium settings migration remains research only. The current settings implementation is still egui and has not gained user acceptance from these tests.


## Full-page selectors and imported-rest correction

The settings page is an opaque CentralPanel covering the entire remaining viewport beside navigation. Actual offscreen Vulkan captures of the production application at 2214 × 1513 are in `previews/settings-characters-fullpage.png`, `settings-connections-fullpage.png` and `settings-voice-fullpage.png`. They use an isolated no-key fixture, disabled speech and an unavailable loopback chat endpoint; the Connections error is intentionally offline fixture state. These are rendered application pages, not HTML mockups or physical mouse acceptance.

Companions, connection models, speech/transcription models, Fish voice results and motions use dropdown selectors. Companion/model/motion dropdowns include search; models and motions retain virtualized results. A single section dropdown replaces the jump-button row. Automated pointer-event tests exercise opening, selection, closing and preservation of custom personality text. Full-page painted coverage and scrolling are checked at 2214 × 1513, 1280 × 900 and 800 × 600.

The pinned Homunculus normalization was rechecked against its Bevy VRM implementation. Production idle arm relaxation had changed the target transforms before the retargeter captured rest, introducing approximately 69 degrees of extra arm rotation. Imported humanoid transforms now have immutable snapshots before procedural changes, including inactive avatars. A separate idle pose supports Stop, omitted bones and avatar switching. Regressions use the real bundled VRMA and the locally available WW4 Wave.fbx; a runtime regression prevents either arm relaxing before both imported-rest snapshots exist. This is source/ECS evidence, not acceptance of every avatar or moving animation.

The KWin bridge now permits authenticated discovery when environment hints are absent, and reports visible bottom dock geometry separately from monitor work areas. Window drops retain Homunculus's cursor band; floor/taskbar drops additionally consider projected hips/feet. Fresh native geometry controls release origin, including short drags with consumed release events. Six fake JavaScript scenarios plus Rust regression fixtures cover these cases. No live KWin script or desktop input was executed in this verification pass; physical taskbar sitting remains unverified.

## Latest integration: lip sync, previews, desktop and latency

The current all-target suite has377 passing tests. Native development build, all-target Clippy with warnings denied, and cargo formatting checks passed. These checks do not establish physical desktop or speaker acceptance.

A confirmed lip-sync regression is fixed: simply opening the manual expression sliders inserted zero weights for all channels; the runtime treated their presence as a manual override, silencing automatic vowels/RMS mouths. Zero weights no longer reserve a channel, and drawing a slider no longer inserts an untouched value. Positive manual weights and explicit selected-expression ownership remain effective. Regressions cover legacy-zero RMS/vowels/blink and consumed WAV samples → speech meter → audio envelope → actual morph weights, including mouth closure on Stop. Existing saved zero entries are tolerated. Calibration remains optional for RMS lip sync; no real-voice calibration or physical speaker/mouth timing was established.

Character/Scene settings keep a live render-to-texture avatar preview with framing, rotation and zoom. Windowless captures use the actual Vulkan renderer. Optional head/eye gaze and the refined blink cadence have bounded, isolated ECS tests. The new spring collision implementation separately passed16 vendored tests (15 unit plus one mapping test), including referenced VRM0 spheres/VRM1 capsules, attached transforms and the supplied Neuro collider fixture. Hair/skirt motion has no visual acceptance yet; this is joint/collider physics, not cloth/self collision.

Desktop seating now requires a detected surface and uses Homunculus's10% hip-height clearance. Sitting state is transient; stale saved flags cannot restore a floating pose. KWin movement failures clear the seat. Snap queues authenticated bridge movement rather than relying on Wayland window positioning. Fresh KWin cursor tracking supports mesh-margin passthrough; grabs and menus retain input. Offline routing, serialization, freshness, recovery and one-shot snap tests pass. Six fake JavaScript bridge scenarios pass. Physical input, dynamic taskbar following and passthrough on the user's compositor remain unverified.

Live latency evidence now includes repeated Drama3 versus S2.1 Pro across WebSocket/HTTP/SSE:18/18 whole-pipeline and18/18 fixed-text calls succeeded, plus6/6 explicitly pre-opened WebSocket experiments. HTTP won the fixed-text comparison, while WebSocket won the currently implemented streamed-LLM pipeline. Only WebSocket overlaps LLM text and speech; HTTP/SSE await the full reply. No500–700ms end-to-end guarantee, device-onset measurement or crackle acceptance is claimed. See benchmarks/RESULTS.md for per-mode medians, LLM alternatives and prewarming costs.

## Provider routing, prerecorded ASR and product footage

Subsequent authenticated tests use the production adapters with the configured proxy and no speaker device. DeepSeek V4.1Flash and its fast variant were tested with reasoning none, Vercel default/TTFT/throughput routing, provider-only restrictions and provider order. The instrumented five-call baseten/Drama3 WS Low batch measured359ms median first text and1070ms decoded audio. Public endpoint slugs were verified, and generation lookup attributed34 successful instrumented/Gemini/order/ASR requests after timing. Strict pins returned429 capacity errors in other batches; no model fallback hid those errors.

Recorded16kHz mono PCM was transcribed through Fish base, Fish Pro, Vercel Fish and Vercel Whisper, then passed as the actual LLM input. All12 ASR requests succeeded; one subsequent strict baseten chat request returned429, leaving the Vercel Fish full sequence2/3. Median complete-WAV-upload→decoded-reply was1623ms for Fish Pro and2887ms for Vercel Whisper. This excludes capture, endpointing, transcript review and physical playback. The reused ASR client preserves redirects-off, timeout/size limits and per-request authorization.

The product video contains1139 actual offscreen frames at15fps,1280×832 with title bands, and75.933s of app footage/Fish narration. The same production metered decoder consumes narration samples to drive mouth movement; no audio device opens. Full H.264/AAC decoding passed, all six chapter views were visually inspected, and no desktop-input mechanism was used. Physical desktop interaction and perceived lip-sync/audio quality remain unaccepted.

New automated coverage checks final SSE routing metadata without duplicate text, invalid narration/duration rejection before frame output, and preservation of existing recording files. The source-only Opus architecture and latency reports remain reviews and proposals, not proof of implemented brain features.


## 2026-10-18 integrated speech, memory and review fixes

All-target offline tests pass: 382 library, 83 app and 6 integration tests (471 total). All-target clippy with warnings denied passes. This supersedes the earlier statement that HTTP waits for a full reply: buffered and HTTP WAV reply speech now chunk during LLM streaming, with bounded two-request HTTP prefetch and ordered delivery. Scene replies emit bounded filtered sentence groups before model completion. SSE is unchanged. These paths have fake-server ordering, cancellation and metadata tests; no new live latency or physical audio acceptance is claimed.

Facts memory conservatively deduplicates filler-only variants, retaining added qualifiers, locations, ingredients, numbers and negation. Saving prunes fully expired correction trees while preserving live corrections and their evidence; Expire now explains the later permanent prune. Legacy near-duplicate files still load. All 32 facts-memory tests pass.

Fable review fixes preserve recent Hyprland geometry and queued moves over transient failures, sort never-focused windows below focused peers, stop RGB correction when its extracted component is removed, order correction before bloom, and restrict pointer gaze to primary-window cameras. Fake-socket and fixture checks pass; no real Hyprland or new GPU/interactive acceptance is claimed. Chromatic-aberration relative ordering remains unpinned because its system is private upstream.

## Shared reply scheduler and current-mode benchmark

Studio, scene speech and `bench_pipeline` now share one sentence splitter; Studio and the app-cadence benchmark share the bounded HTTP reply queue itself. Offline checks cover stale events, queue accounting, unfinished replies, ordered multi-clip consumption with different sample rates, and first-audio timestamps surviving later clips. The app path opens WebSocket on first text rather than granting it the historical eager-open head start.

A new 24-row authenticated Low-latency run compared Drama3 and S2.1 Pro across WS/HTTP/SSE/buffered, with three repeats, real chat-worker events, 16 ms polling and the configured proxy. 24/24 succeeded and all chat workers closed. S2.1 Pro HTTP had a 1336 ms median first decoded audio in this sample; the target remains unmet. See benchmarks/RESULTS.md for the complete table, mode-dependent draining scopes and short-reply limitations. These are decoder observations, not physical speaker or interactive desktop acceptance.

Final shared-scheduler checks: 384 library + 83 app + 6 integration + 1 benchmark test = 474 passing tests; all-target clippy with warnings denied and package formatting pass. Builds used one job to limit local resource use.


## Microphone anti-aliasing and endpoint hysteresis

Nearest-neighbor conversion was replaced with a causal polyphase windowed-sinc filter. The filter preserves state and partial stereo frames across callbacks, bypasses conversion at native 16 kHz, and allocates its bounded coefficient/history buffers before recording. Supported microphone rates are 4–384 kHz. Synthetic tests retain 1 kHz and 6 kHz signals, reject 8.5 kHz and 12 kHz input energy at 44.1/48/96/192/384 kHz, and check 8 kHz upsampling image rejection. Split callback output is byte-identical to whole-buffer conversion.

Energy endpointing requires 30 ms of sustained onset, then holds speech at 70% of the onset level. Silero requires two consecutive 512-sample frames (64 ms) and uses a lower hold probability that preserves the UI's full threshold range. A 10 ms click no longer arms endpointing; tests retain quiet speech and still stop on configured trailing silence. All audio is retained during onset confirmation. Silence presets and the 30-second cap are unchanged.

These are offline signal/state-machine checks, not microphone-device acceptance or measured ASR accuracy gains. The causal FIR adds approximately 1.3–16 ms of group delay depending on input rate, with native 16 kHz bypassed; energy endpoint decisions also use 10 ms windows. Physical capture, noise tuning and full conversational round-trip acceptance remain open. No recording, upload or paid call was made for this slice.

Final checks: 390 library + 83 app + 6 integration + 1 benchmark = 480 passing tests; all-target warnings-denied clippy, package formatting and diff whitespace checks pass. Builds used one job. A requested Fable review could not start because the orchestrator rejected dispatch at its automated causal-hop limit; no new Fable review is claimed.


## Emotion-bound motion playlists

Emotion-mapped motion now accepts multiple selected clips per emotion through one multi-select dropdown per emotion. Playback rotates deterministically in selection order, with independent positions for each emotion; duplicate, missing and locomotion paths are excluded by the current scan. Older single-binding profiles are unchanged, and multiple bindings round-trip without a schema migration. Rotation is session-only and resets when the selected character or avatar changes.

Cooldown-skipped replies do not advance the playlist. Replies without same-job affect metadata or a loaded avatar play nothing, and loading an avatar later never replays an older reply. Existing manual override and import-busy guards still apply; attempts skipped by an active import are not queued. No decision request is added.

Offline tests cover rotation, duplicate/missing/locomotion rejection, cooldown skips, separate emotion positions, avatar/character resets and missing-avatar behavior. An egui click test selects two checkboxes without closing the dropdown, clears the playlist, removes an unavailable binding and verifies that merely editing bindings never enables automatic mode. No live GUI/animation acceptance is claimed.

Reply affect is intentionally trailing metadata. Emotion-mapped motion therefore starts after completion, not necessarily at speech onset; waiting for that metadata before speech would conflict with the first-audio latency goal. No prose-based emotion guess or extra classifier was added.

Final checks: 391 library + 87 app + 6 integration + 1 benchmark = 485 passing tests; warnings-denied all-target clippy, package formatting and diff whitespace checks pass. The strengthened avatar-reset regression also passes. Builds used one job. No new external-model review or live GUI acceptance is claimed for this slice.


## Lexical recall quality

Notes and facts now share a lexical term policy: distinct query terms, common English function-word filtering, punctuation/possessive/contraction normalization, short identifiers and numbers retained, and explicit negation preserved. Latin-script note matching uses whole tokens, so tea no longer selects steak. Common Chinese/Japanese/Korean-script queries retain substring matching for unspaced writing; this is not a multilingual segmenter or semantic inference.

Query scanning is bounded to the first 16 KiB at UTF-8/word boundaries, then at most 64 distinct content terms. Oversized partial tokens are dropped, never turned into prefix matches. Pinned-note priority, facts recency ties, correction/expiry exclusion, six-record limits, existing prompt byte budgets and quoted untrusted-data formatting remain unchanged. The trace shows the terms actually used. This deliberately changes which evidence is selected for previously noisy/short-term queries; historical trace-format oracle tests remain unchanged for unaffected fixtures.

Offline regressions cover tea/steak separation, common-word-only queries, VR/AI/NY and numeric terms, duplicate-query terms, contraction negation, preserved pinned notes, unspaced Japanese text, bounded long UTF-8 inputs and exact trace scores. No inference requests or storage migration are added. Dataset-level recall accuracy and interactive acceptance remain unmeasured.

Final checks: 398 library + 87 app + 6 integration + 1 benchmark = 492 passing tests. After the final irregular-contraction cleanup, all 50 targeted memory/term tests pass; warnings-denied all-target Clippy, package formatting and diff whitespace checks pass. Builds used one job. Fable review dispatch was rejected by the orchestration causal-hop limit; no external-model review is claimed.

## Hips translation across armature frames

Hips translation deltas now pass from source-parent coordinates into the source model frame, through facing and model-height conversion, and into the target-parent coordinates. The transforms retain nonuniform scale and rotation, while application placement remains excluded. Singular/nonfinite target parent transforms leave the hips at their rest translation rather than applying an invalid inverse; rotation retargeting remains independent. This uses rest armature frames, not a general animated-parent constraint solver.

Offline coverage includes a rotated/scaled source and target together, partial/zero weights and reversed facing, plus full app updates with rotated/nonuniformly scaled target armatures and independent desktop placement. Existing bundled sitting, restoration, omitted-bone and crossfade regressions remain covered. Arbitrary FBX rigs and visible foot placement still require visual acceptance.

Final checks: 399 library + 87 app + 6 integration + 1 benchmark = 493 passing tests; warnings-denied all-target Clippy, package formatting and diff whitespace checks pass. Temporarily restoring the old local-space displacement makes the new armature-conversion regression fail; the corrected implementation was restored before the full suite. All builds used one job. No new external-model review or live visual acceptance is claimed.

## Fear and disgust reply emotions

The opt-in reply metadata schema now accepts fear and disgust alongside the existing eighteen labels. They preserve their own VAD mood, persistence and local body-motion bindings. The facial bridge matches custom fear/fearful/afraid and disgust/disgusted expression names case-insensitively; an avatar with no matching expression receives no unrelated facial preset fallback. The existing manual-expression priority and mouth/blink/look exclusions remain in force. No extra classifier call is added, no prose-based emotion inference occurs, and speech still streams before trailing metadata completes.

Offline regressions cover parse/identity, every marker split for both emotions, saved-mood round-trips, custom-expression matches, missing-expression behavior and manual priority. A motion-state test checks both new labels use the user's scanned binding with no decision request. Live facial appearance remains unaccepted. Existing saved labels remain unchanged and the feature stays opt-in.

Final checks: 400 library + 88 app + 6 integration + 1 benchmark = 495 passing tests; warnings-denied all-target Clippy, package formatting and diff whitespace checks pass. Builds used one job. No new external-model review or live GUI acceptance is claimed.

## Scene memory recall inspection and request ordering

Group chat now transports bounded note/evidence recall traces from the scene worker to a collapsed, per-recipient inspector. It retains at most four recipients from the latest successfully started submit, resets on the next submit, and keeps focused-chat inspectors independent. Diagnostics retain existing bounded IDs, scores, matched terms and selection/exclusion counts, never raw queries or record text; nothing is saved or sent to providers. The panel labels these as prepared context, not proof of request delivery or successful replies, since later validation/network errors can still occur. Cancel drops the event receiver, preventing late workers from publishing new traces; already observed traces remain inspectable.

Scene prompts now keep persona and up to 38 previous history messages ahead of query-dependent notes/evidence, then append the latest user message. Recalled data remains in user-role messages and never enters saved history. This preserves the existing conversation prefix when recall changes, as focused chat already does; no provider cache-hit or latency improvement is claimed without live measurements.

Offline coverage checks stable prefix/order/history bounds, user-role recall isolation, worker-side traced recall matching the existing prompt and inspector trace, disabled/missing ledgers creating no files, and the real scene worker's two isolated recipient traces against local fake streaming requests. GUI appearance remains unaccepted.

Final checks: 401 library + 88 app + 6 integration + 1 benchmark = 496 passing tests; warnings-denied all-target Clippy, package formatting and diff whitespace checks pass. Builds used one job. External-model review remains unavailable through the delegation causal-hop limit; no live provider cache measurement or GUI acceptance is claimed.

## Current WebWaifu4 FBX GPU captures

Snapshot `cc26ca1`: the current production VRM loader and retargeter rendered the recovered WebWaifu4 `Wave.fbx` on bundled Neuro using Vulkan / NVIDIA GeForce RTX 5060 Ti. The importer mapped 52 humanoid bones. Windowless captures at [0.5 seconds](previews/ww4-wave-retarget.png) and [1.5 seconds](previews/ww4-wave-retarget-1.5s.png) are 1280×960 RGBA, with 72,096 and 73,435 visible pixels respectively. Both were visually inspected: different torso/arm poses, whole avatar inside the frame. This is current GPU evidence for these two sample poses, not temporal smoothness, source-motion fidelity, arbitrary FBX rigs or physical desktop interaction. The earlier source/target-armature algebra regressions remain separate synthetic evidence.

Source fixture SHA-256: `24b579c01571b1641646a45ac0def6b6d48a7964c7e02bbb7d24b742b50823fd`. The original FBX is not redistributed by this capture update. Command: `CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=../../work/studio-target nice -n 10 cargo run --example render_desktop_motion -- LOCAL_WW4_WAVE_FBX previews/ww4-wave-retarget.png --time 0.5 --frames 60`; the second run used the built executable and `--time 1.5`. No window, audio device, provider, key or network request is involved.

Desktop inspection again returned “desktop changed during inspection”; no focus/input actions, guard bypass or live companion launch followed. Native drag/drop, sitting and live audio acceptance remain open. Code is unchanged from the 496-test receipt above; only these captures and documentation were added.

## Optional one-turn microphone pipeline

Microphone settings now offer Send transcript automatically, default false for new and old profiles. Manual Start captures the selected character and whether automatic send was enabled with an empty draft. Enabling the option mid-recording cannot arm a review-only recording. Current auto-send must still be enabled when the result arrives; character changes cancel pending transcription, and an explicitly mismatched completion is also discarded by the host. Empty results do nothing. Existing drafts, active focused chat and memory/model readiness failures leave text available for review. The ordinary send path handles lexical/semantic/evidence recall, provider validation and streamed reply speech. No transcription is silently combined with an existing draft and automatically sent. The microphone never restarts itself; this is optional single-turn automation, not wake-word or full-duplex conversation.

Offline checks inject generation-tagged transcription completions to verify captured intent, one-shot consumption, stale suppression, cancellation and old-profile defaults. Host tests cover disabled/unarmed mode, occupied drafts, active chat, changed characters, empty results and loading memory. A local fake HTTP provider verifies an eligible transcript enters production spawn_chat and saved user history. These tests do not open a microphone, synthesize speech, contact live providers or establish real end-to-end latency. UI and physical microphone acceptance remain open.

Final checks: 402 library + 90 app + 6 integration + 1 benchmark = 499 passing tests; both new host transcript tests pass ten consecutive repeats. Fixture teardown now waits for notes, evidence and semantic settings loads before removing temporary directories. Warnings-denied all-target Clippy, package formatting and diff whitespace checks pass. Builds used one job. External-model review remains unavailable through the delegation causal-hop limit; no physical microphone/speaker or live latency acceptance is claimed.

## Current production microphone settings capture

The production app at `84ddab6` plus the speech-capability copy correction starts, loads Neuro and renders the entire Voice & microphone page without a desktop window. [Current native UI capture](previews/microphone-settings-current.png), 1600×4096, was visually inspected on Vulkan / NVIDIA GeForce RTX 5060 Ti. The optional auto-send checkbox is visible and unchecked, alongside the disabled microphone and manual Start controls. New poll_input memory/semantic resource wiring runs in the real production schedule without startup/system failures. This proves the rendered layout and schedule startup, not physical input, microphone capture or automatic-send interaction.

The render uncovered and corrected misleading timestamp-SSE help text that claimed WebSocket text streaming was unimplemented. The panel is now named Fish timestamp SSE, and describes the separate Delivery option that accepts incremental chat text. Transport behavior is unchanged; all six existing Fish SSE protocol tests pass.

The separate no-history fixture uses loopback-only chat, speech and transcription routes, nonexistent fixture key-environment names and no credential file. Speech/microphone stay disabled; voice-library hosted fetching is avoided by selecting FishLocal. Catalog connection-failure text is intentional fixture state. Audio and window plugins are disabled by the existing offscreen path. No paid calls or desktop-control actions were made. The initial hand-written fixture used an incorrect adapter spelling and was rejected before rendering; correcting it to the existing FishLocal serde variant required no app-code workaround.

Final checks for this copy/capture-only update: six Fish SSE tests, warnings-denied all-target Clippy, package formatting and diff whitespace checks pass. The full 499-test baseline is recorded above; no transport/runtime logic changed in this update. Both windowless production renders exited successfully. Builds used one job at reduced scheduling priority; no external-model review is claimed.

## 2026-10-18 — compositor cursor for viewport gaze

Desktop pointer tracking now supplies the library gaze overlay with window-relative compositor coordinates. This keeps the existing viewport pointer target available during native click passthrough. Samples retain the bridge receipt timestamp, expire after 500 ms, reject nonfinite/future samples, and are cleared outside Desktop mode or when tracking cannot supply a cursor. Native cursor and ambient behavior remain fallbacks. This does not add off-viewport following or morph-only gaze.

Evidence: `cargo test --lib gaze::` passed 8/8 including the new freshness/invalid/clear regression; single-job `cargo clippy --all-targets -- -D warnings`, package formatting, and `git diff --check` passed. No interactive compositor acceptance or new model review: requested Opus delegation was rejected by the orchestration causal-hop limit. No paid requests or GUI launches.

### Integrated gaze verification follow-up

After `88cfbfc`, the single-job all-target suite passed **500 tests** (403 library, 90 application/controller/UI, 6 integration, 1 benchmark). A further schedule-level gaze regression then passed with all 9 gaze tests: a primary-window camera and avatar receive no native cursor events, opposite bridge coordinates turn the head in opposite directions, and disabling gaze restores the rest rotation. The library now has 404 tests; the complete 501-test total has not been rerun after adding that test. Updated the gaze panel's stale claim that compositor pointer input was unavailable. These remain offline checks, not live compositor acceptance.

## 2026-10-18 — avatar-private shader materials

Shader preview variants are now keyed by avatar root plus imported source asset. Primitives within one avatar still share conversions, but separate instances receive distinct mutable Toon/Lit/Unlit materials. Both MToon and StandardMaterial imports are cloned before tuning. Imported source assets stay unchanged, preventing a later instance from capturing already-scaled authored map strengths. Weak cache cleanup remains tied to the last mesh.

Evidence: all 8 shader tests passed, including new regressions for three sequential instances under nonidentity tuning, material isolation, unchanged imported source values, PBR restoration, existing mode/outline switching and weak-cache cleanup. Strict all-target Clippy passed. The production windowless capture `previews/material-isolation-current.png` completed on Vulkan and was inspected: Neuro is visible in the Scene & avatar preview. No interactive desktop window or audio device was used. This is a material-ownership prerequisite; material-only VRM expressions remain unimplemented. Fable review dispatch was attempted but rejected with “Maximum automated causal hop budget reached”; no fresh external-model review is claimed.

The final source change after those checks only wraps a documentation comment. No new build or render was started after the user requested advance load notifications.

## 2026-10-18 — early sentence speech for focused timestamp SSE

Focused timestamp-SSE replies now start on filtered 40–300-character sentence groups while the LLM is streaming. The shared `SseReplySpeech` scheduler allows one request at a time and waits for both network completion and playback drain before advancing, because a new Fish SSE request invalidates the prior audio generation. Stop, errors and character changes clear pending sentence state; the reply stays marked handled so its completed text is never re-synthesized after Stop. WebSocket remains the default. No SSE prefetch, gap-free playback or new live latency improvement is claimed. The SSE panel clarifies that word timings are local to the current sentence request.

The `--app-path` pipeline benchmark now uses the same SSE scheduler and simulated real-time drain, labeled `shared-studio-sse-reply`. Historical measurements are unchanged; they used the old full-reply SSE path.

Checks, sequential single-job builds announced in chat: `cargo test --lib reply_speech::` 4/4, `cargo test --bin waifu-studio studio::tests::` 19/19, `cargo test --example bench_pipeline` 2/2, and strict all-target Clippy clean. Loopback tests verify an SSE sentence is requested before reply completion, decoded PCM arrives, network completion alone cannot advance while playback is active, Stop drops the remaining tail, split task metadata is hidden, stale events cannot finish a current request, and the benchmark consumes both chunks fully at a simulated real-time rate. No paid calls, speakers or GPU render. The complete all-target suite has not been rerun for this slice.

Independent Muse review saved verbatim to `reviews/muse-independent-review.md`: 503 all-target tests passed on its explicitly documented moving tree before this SSE patch. The stale README count is corrected; the two low hypotheses are dispositioned in `reviews/README.md` without claiming runtime reproductions or making speculative code changes.


## 2026-10-18 — live current-pipeline comparison

After advance notification in chat, ran the production benchmark at `44fe22d` with `--phase pipeline --app-path --frame-ms 16 --models deepseek/deepseek-v4.1-flash --speech-models drama-3,s2.1-pro --modes ws,http,sse --repeat 3 --provider-only baseten --reasoning none --latency low --chunk-length 160 --min-chunk 20 --condition true --max-tokens 1024`. Single-job compilation completed, followed by 18 sequential bounded provider requests through the configured proxy. All rows succeeded and workers closed. Results are saved in `benchmarks/pipeline-sentence-sse-low.json`; medians and limitations are in `benchmarks/RESULTS.md`. No microphone/speaker/GPU render or saved-setting changes.

Opus 5.5 implementation and Fable 5.1 review dispatch became available again, but both workers failed before doing work with 429 account rate-limit/cooling-down errors. Neither produced edits or a fresh review. Material-only expressions remain pending; no model work is attributed to these rejected tasks.

## 2026-10-18 — explicit hosted HTTP/Low trial preset

Added `VoiceConfig::use_http_low_latency` and a hosted-Fish-only “Try HTTP low latency” action in Voice & mic. It selects progressive HTTP WAV, Low latency, chunk length 160, minimum 20 and previous-chunk conditioning. Model, reference, voice, endpoint, key-variable name, speed, performance direction and speech-enabled choice are preserved. Other adapters are a no-op. Clicking stops active speech, saves the selected character's settings and does not synthesize speech; no saved profile or default changes without that action. The existing live-pipeline action remains available for switching back.

Checks: single-job `cargo test --lib voice::tests::` passed 16/16, including identity/opt-in preservation, adapter isolation and idempotence for the preset. Strict all-target Clippy and package formatting were checked. No live requests or GPU render were made for this slice. Button wiring was source-checked and compiled, not interactively clicked.


## 2026-10-18 — bounded SSE cancellation admission

Timestamp SSE now permits at most two blocking HTTP workers. A cancelled request still holding a connection no longer prevents one replacement request from starting. Further requests remain pending at the two-worker limit; generation checks continue to discard cancelled results and audio. This does not interrupt blocking DNS or HTTP reads: behind two stalled cancelled workers, another request can still wait for the existing 90-second request timeout.

After advance build notification, single-job `cargo test --lib fish_live::` passed 7/7, including a loopback regression holding two cancelled HTTP requests, admitting the first replacement before release, bounding workers, and rejecting stale events. `git diff --check` passed. Strict all-target Clippy was attempted but encountered the concurrent, unfinished material-expression worker's dead-code errors; it is not claimed clean for this slice. No live provider, GUI, speaker or GPU check.

## 2026-10-18 — material expressions and local fictional routines

Material-only expression definitions now survive import and feed the shared manual/binary/override/blink/mouth/behavior weight path for the focused avatar and scene extras. VRM1 color, emission, shade, matcap, rim and outline binds and VRM0 `_Color`, `_EmissionColor`, `_ShadeColor`, `_RimColor`, `_OutlineColor` use additive authored deltas on avatar-private variants, freshly recomposed to avoid accumulation and restore when weights/root membership disappear. Existing tuning and emission/matcap strengths remain effective; outlines synchronize after expression application. Unrotated shared UV0 transforms are supported by Lit/Unlit. Unsupported Toon UV transforms, rotated/UV1 transforms, VRM0 `_MainTex_ST` and unknown properties appear in status. Toon-specific colors have no Lit/Unlit counterpart. Ten new fixture/schedule/arithmetic tests cover material-only imports, malformed bindings, colors/emission strength, overrides, isolation, extras, restoration, tuning and UV arithmetic. No bundled avatar was claimed to reproduce material-only expressions, and no new GPU capture or visual acceptance was performed.

The Scene panel now offers an off-default fictional weekly routine stored with character scene settings. Up to 32 anchors with 120-character activity labels select current/next imagined activity in the Unix system-local timezone; duplicate starts and invalid settings disable selection with a visible error. Each anchor lasts until the next, including weekly rollover. Clock jumps/suspend/DST re-evaluate current wall time without catch-up actions. No model call, background scheduler, prompt insertion, user-presence inference or outgoing message. Non-Unix platforms explicitly report local clock unavailable. Three deterministic tests cover boundaries, validation and old profile defaults/round trips; the existing character-switch test now verifies routine isolation too. No GUI interaction acceptance.

Parent announced each compilation in chat and used one low-priority Cargo job at a time. `cargo test --all-targets`: 423 library + 91 application/controller/UI + 6 integration + 2 benchmark = **522 passed**, zero failures. `cargo clippy --all-targets -- -D warnings`, package-only `cargo fmt -- --check`, and `git diff --check` are clean. The material worker ran no compilation or live requests. Claude Opus/Fable retries failed before work (429 account cooldown); later retries were rejected by the orchestration causal-hop limit before contacting Claude. This slice has parent review and offline tests, not a fresh Fable review.

## 2026-10-18 — shared mesh UV transforms in MToon

Extended the local MToon material, Rust GPU uniform and WGSL uniform with the same affine UV transform. The existing mesh-UV helper applies it to base color/alpha, normal, shade, shade shift, emission and rim-multiply texture sampling. Matcap remains view-space. Toon/Lit/Unlit conversion preserves the authored transform, and material expressions now blend and restore shared unrotated UV0 scale/offset in all three appearances. Rotated/UV1 expression transforms and VRM0 `_MainTex_ST` remain explicitly unsupported; this does not provide per-texture transforms or general Unity shader support.

Expanded the material schedule/variant tests to assert Toon UV blending and restoration alongside Lit/Unlit and source immutability. The initial library run passed 422/423: one updated test incorrectly expected an empty diagnostics list despite the still-correct Toon-only color warning. Corrected the assertion to retain that warning; no production behavior was weakened. The subsequent full `cargo test --all-targets` passed 423 library + 91 application + 6 integration + 2 benchmark = **522**, zero failures. Strict all-target Clippy, package formatting, explicit formatting of the modified vendored Rust file and diff whitespace are clean.

After advance chat notification, built and ran `examples/render_mtoon_uv.rs` with one low-priority Cargo job. This windowless/no-audio fixture renders procedural emissive stripes through the actual MToon Vulkan pipeline and accepts a capture only after corresponding identity/transformed UV samples are red/blue. It passed on the NVIDIA RTX 5060 Ti; `previews/mtoon-uv-transform.png` was also inspected visually. This verifies the live GPU uniform/sampling path on that fixture, not interactive avatar appearance or every texture/prepass combination. No provider request, microphone or desktop GUI action occurred. Each build/render was announced before starting and its completion reported.


## 2026-10-18 — explicit local note associations

Manual character notes now support explicit undirected links, edited under each note’s Related notes section and persisted in that character’s existing bounded JSON document. Links require two distinct existing local note IDs, canonical ordering, uniqueness and at most 512 edges. Deleting a note removes its incident links. Old documents default to no links and linked recall off. Switching characters clears links and the recall opt-in before loading the selected file; completed writes invalidate the old recall trace. No new model call, extraction, inferred fact, graph server or storage migration.

An off-default checkbox enables one-hop lexical expansion after direct/pinned ranking: at most two linked candidates are considered, only from directly matched notes actually inserted in the prepared context. Linked candidates do not become seeds, and the existing six-note and 4,000-byte limits still apply. Pins-only semantic retrieval remains unchanged. Focused and scene inspectors show the directly matched seed ID for linked hits; no graph trace is injected into the prompt. Default/off context remains byte-identical under the existing frozen regression tests. This is authored association recall, not autonomous learning or a trained connectome.

After advance notifications and with one low-priority Cargo build job: targeted memory filter passed 49 tests; full all-target suite passed 427 library + 91 application + 6 integration + 2 benchmark = **526**, zero failures. Then strengthened the existing loopback scene-worker test with per-character persisted links: the additional targeted run passed, proving each linked note enters only its character’s request and its scene trace records provenance. Strict all-target Clippy, package formatting and diff whitespace are clean. No live provider, real GUI or hardware test. Fable retry after the user’s Claude re-login was rejected by the orchestration causal-hop limit before reaching Claude; login health and a fresh Fable review remain unverified.


### Group reply affect — 2026-10-18

Opted-in scene recipients request the existing trailing affect schema without an extra model call. Incremental filtering removes affect metadata from progress, history, transcript and speech. Scene moods are isolated by character; captured settings must still match on completion. Cancellation, roster removal and configuration changes discard stale moods. Supported expressions use the shared facial/material composition with existing manual and reserved-channel priorities. Local behavior must also be enabled for automatic expression playback.

Offline checks: `cargo test --all-targets` passed 528 tests (429 library, 91 binary, 6 integration, 2 benchmark). After tightening configuration invalidation and the final speech-filter path, all 12 scene-chat tests passed again. The mock streaming test checks opted-in Alice versus opted-out Bob, speech before completion and filtered persistence. New tests cover receipt isolation, stale settings, cancellation and bounded local facial weights/restoration. Strict all-target Clippy, package formatting and diff whitespace checks passed on the final source.

No live provider, GUI, speaker/lip-sync or multi-avatar visual acceptance was run. Metadata arrives after prose; this does not synchronize emotion to speech onset and does not add group body-motion selection. A fresh Fable review was attempted but blocked before model invocation by the orchestration causal-hop limit.


### Optional fictional routine chat context — 2026-10-18

A separate persisted `share_with_chat` flag defaults to false for old and new profiles. Both routine display and sharing must be enabled, and anchors/local time must be valid. Current/next activity labels are JSON-serialized inside explicitly fictional user-role context immediately before the latest user turn. Focused and serial group requests use their own character settings; context is not added to saved conversation history. No extra provider call, scheduler, presence inference or outreach was added. Platforms without a local-clock implementation omit the context.

Checks: all 430 library tests and 91 app tests passed. The focused production-request mock test was extended afterward and passed again, checking user-role placement and exclusion from history. The group streaming mock checks Alice-only context, absence from Bob's request and absence from persisted history. A pure routine regression checks both opt-ins, escaping, current/next selection, invalid schedules and old-profile migration. Strict all-target Clippy, package formatting and whitespace checks passed. No live model or GUI acceptance was run.


### VRM0 texture-transform expressions — 2026-10-18

Supersedes the earlier `_MainTex_ST` limitation above. VRM0 material-expression targets now map Unity `[scale_x, scale_y, offset_x, offset_y]` to glTF coordinates using `offset_y = 1 - offset_y - scale_y`. This matches the inspected [three-vrm VRMExpressionLoaderPlugin conversion](https://github.com/pixiv/three-vrm/blob/dev/packages/three-vrm-core/src/expressions/VRMExpressionLoaderPlugin.ts), retrieved on 2026-10-18. The formula follows conjugating texture coordinates by `v -> 1-v`; no Unity shader source was ported.

Both VRM0 and VRM1 now share authored base-texture validation. Only unrotated UV0 is accepted, converted values must remain finite, material names must resolve uniquely, and binds retain the existing total cap. Targets feed existing avatar-private Toon/Lit/Unlit additive composition and zero/removal restoration. This does not add per-texture coordinate channels, UV1/rotation support or general Unity shader compatibility.

All 431 library tests passed, including a new parsed VRM0 regression checking nonidentity authored UVs, three sample coordinates at zero/half/full weight, agreement of Toon/Standard material transforms, restoration and rotated/UV1 rejection. The existing material-only import test now expects the UV binding rather than an unsupported-property warning. Strict all-target Clippy, package formatting and whitespace checks passed. No new GPU render or material-expression avatar acceptance was run.


### CLI framing and SSE terminal failures — 2026-10-18

Fixed the two low-severity findings supplied in the user’s loop-review report. CLI environment selection now widens only the three furnished rooms, preserves saved framing for `none`, and retains the explicit room-view override. SSE reply speech now clears in-flight state and queued text on either synchronous submission rejection or owned asynchronous failure; late text cannot restart the failed scheduler. Regression tests cover environment kinds and both failure paths.

The integrated working tree, including the incoming-message appraisal below, passed all 538 tests (437 library, 93 application, 6 integration, 2 benchmark). Strict all-target Clippy passed afterward; its intervening changes were style-only. This is integrated offline evidence, not a pristine pre-appraisal checkout or live speech acceptance.

### Optional incoming-message Jev appraisal — 2026-10-18

Automatic affect now offers mutually exclusive reply metadata (default) or incoming-message Jev evaluation (extra request), still disabled unless opted in. Jev uses the existing Scene decision connection and strict offered emotion plus discrete valence/arousal/dominance choices; those categories map to authored -0.75/0/+0.75 values, not calibrated measurements. Bounded fictional-character/persona/message context excludes paths. Failures are fixed redacted messages and responses are capped.

The background adapter admits at most two workers globally, including cancelled blocking requests, with no queued retry. Results expire after 1.5 seconds and require matching turn, character, affect settings and decision settings. Cancellation cannot interrupt all blocking network/DNS work. Source changes also invalidate captured reply-metadata settings while hidden metadata remains filtered. Focused late results can update mood but do not replay body-motion decisions. Group speech is handed off before waiting for an outstanding appraisal; the serial scene job can remain busy briefly until that result expires.

Offline checks: the integrated all-target suite passed 538 tests, strict Clippy passed, and local HTTP fixtures cover wire schema, offered choices, cancellation/admission, stale snapshots, source exclusivity and group/focused application. No live Jev/provider, GUI, GPU render, microphone, speaker onset or lip-sync acceptance. No fresh Opus/Fable review: delegated invocation hit the orchestration limit; a direct Claude CLI smoke request reached the service but reported the session limit.


### Direct Claude Squad review and mailbox hypothesis — 2026-10-18

Direct Opus 5.5 responded to a smoke request; Opus implementation investigation and Fable 5.1 source review then ran through the local Squad mailbox. See reviews/fable-appraisal-review.md for original findings and parent dispositions. Fable ran no tests. The late-appraisal body-motion omission follows the documented no-replay policy; failed provider results are safely discarded. Replaced appraisal thread spawn with Builder so thread creation failure returns a fixed error instead of panicking and drops the captured slot permit. OS exhaustion was not forced.

Opus confirmed the historical Fish mailbox overflow hypothesis is unreachable with current producers: one Playing, coalesced Timing and one terminal require at most three entries. Parent removed the proposed runtime admission policy and synthetic duplicate-Playing fixtures, retaining a realistic regression across 500 timing updates, both terminal outcomes, cleanup and stale-generation rejection. No speech-stall fix is claimed.

Parent announced sequential one-job, low-priority checks: appraisal tests 3/3 and Fish tests 19/19 passed. No full-suite rerun in this slice; the previous 538-test receipt remains historical. No live provider, GUI, microphone, GPU or physical audio check.

Strict all-target Clippy, package formatting and diff whitespace also passed on the final source.


### Appraisal total response deadline — 2026-10-18

Confirmed Fable slow-body lead against installed reqwest 0.12.28: a blocking client timeout restarts on each body Read, while a request timeout also guards the async body total lifetime. Added the existing 1.5-second budget at request level. A loopback server drips ten bytes at 300 ms intervals. New regression passed with the fix; removing only the request timeout made it fail after the full 3.00-second body; restored fix and reran all four appraisal tests successfully. Strict all-target Clippy also passed. No full-suite rerun, GUI, live request or DNS interruption claim. Opus expression-gaze work remained separate and unfinished during these checks.


### Recipient and motion total deadlines — 2026-10-18

Applied the same confirmed reqwest fix to recipient and motion evaluation requests: their existing ten-second timeout now also covers the complete response body rather than only successive blocking reads. No retry, routing, model selection or cancellation semantics changed. The appraisal slow-drip regression above establishes the dependency behavior; these two adapters were not each subjected to a ten-second drip fixture.

Announced one-job low-priority `cargo test --all-targets decision`: 3 library + 19 application tests passed. This was a moving-tree check including intermediate Opus expression-gaze source (an unused-helper warning appeared); it does not validate the unfinished gaze feature and is not a fresh strict-Clippy receipt. No live requests, rendering or GUI interaction.


### Authored bone and expression gaze — 2026-10-18

Opus 5.5 implemented selected-avatar VRM1 bone/expression and VRM0 Bone/BlendShape look-at, including range maps, flattened Unity Hermite curves, shared morph/material expression composition, manual/overrideLookAt priorities and root-tagged same-frame release. Legacy models without metadata keep procedural eye bones. Expression mode never rotates eye bones and works without anatomical anchors. Malformed/unknown metadata reports diagnostics. World-space targets, offsetFromHeadBone and companion gaze are not implemented.

Fable 5.1 reviewed source against local UniVRM/three-vrm references and confirmed that the initial authored-map input was too small: it reused eye micro-rotation. Parent corrected it to a virtual angular focus (+/-60 horizontal, +/-40 vertical) from the smoothed pointer/ambient target, subtracting only controller head/neck overlays actually applied. Eye strength scales mapped output once. This is not a camera-calibrated or animated-head-aware world ray. Added numerical target/head-offset and post-map zero/nonfinite-strength tests. The original Fable review and parent dispositions are in reviews/fable-expression-gaze-review.md. A focused Fable retry hit Claude’s session limit; no second review pass is claimed.

Initial gaze tests14/14 passed; first full library run448/449 exposed a new fixture’s incorrect Hermite expectation. Parent corrected the analytic default midpoint0.5 and retained a separate nonlinear midpoint0.625 regression; production interpolation was unchanged. The pre-target-correction full suite passed550. After correcting target semantics, an old micro-offset ceiling assertion failed and was replaced by the new bounded-angle/strength contract. A strict-Clippy fixed-size-chunk style issue was corrected without interpolation changes.

Final announced sequential one-job low-priority checks: full `cargo test --all-targets` passed451 library +93 application +6 integration +2 benchmark = **552**, zero failures; strict all-target Clippy, package formatting and diff whitespace passed. No GPU render, GUI, live provider, physical audio or representative-avatar visual acceptance. All compilation completion was announced.


### Optional microphone clip trimming — 2026-10-18

Added serialized off-default `ListenConfig.trim_silence` and a checkbox. Energy and neural VAD record the confirmed onset and latest hold frame; neural positions use the analyzed frame cursor rather than the capture buffer’s later length. Before WAV creation the opt-in trims only outside that speech span, retaining 200 ms before/after and all internal pauses. Invalid/missing bounds, inference failure or a backlog of one full neural frame preserve the complete clip. No thresholds, existing saved silence delay, microphone permissions, routing or automatic-send rules changed. This reduces uploaded clip size; no endpointing or measured ASR latency win is claimed.

Announced one-job low-priority `cargo test --lib listen::`:18 passed, including default-off migration, exact sample/context preservation, internal pauses, analyzed-vs-captured positions, backlog protection and invalid-bound fallbacks. Strict all-target Clippy, package formatting and diff whitespace passed. No full-suite rerun in this slice; the prior552-test receipt is historical. No physical microphone, ASR service or GUI test. Claude’s previously returned session limit prevented a fresh Opus/Fable slice; implementation/review here was parent-only.


### Read-only live mouth diagnostics — 2026-10-18

Added an expandable Face-panel diagnostic with the focused avatar’s consumed-speech envelope before sensitivity and its largest composed vowel/configured-mouth expression weight after manual/override rules. Both are ephemeral FacialState fields, not profile settings. Companion-targeted speech does not populate the focused meters; removing the avatar clears them. Manual mouth weights may remain without audio; the UI now explicitly distinguishes disabling audio-driven motion from manual expressions. A composed expression weight does not prove mesh motion, physical playback or perceptual synchronization. No mouth-driving behavior changed.

Extended existing scheduled consumed-audio test: software signal remains present with lip sync off while the composed mouth drops to zero; stop and avatar removal clear diagnostics. Existing scene-speaker isolation test now checks the focused diagnostics remain zero for companion speech. Announced one-job low-priority `cargo test --lib expressions::` passed36 tests, including material-expression matches; strict all-target Clippy, package formatting and diff whitespace passed. No full-suite rerun, GUI, GPU, provider or physical audio check. Parent-only slice while Claude session limit remains in effect.


### Isolated current-binary render — 2026-10-18

At source HEAD 88cafc8, announced single-job low-priority `cargo build --bin waifu-studio` succeeded. Live desktop input was refused by the KDE tool because its physical activity guard is unavailable; no bypass or focus-grabbing fallback was used. Instead, announced offscreen GPU capture ran the current binary with separate fixture data, a localhost port-1 model endpoint, and empty gateway/Fish credential variables. Process exited successfully after VRM_READY and STUDIO_CAPTURE. Parent inspected `work/ui-acceptance-88cafc8.png`: Scene & avatar page and bundled Neuro full-body preview render at 1280x800. Face diagnostics are below the captured viewport and are not visually verified by this image. No microphone, provider, physical audio, perceived lip-sync or live compositor acceptance. Build and render completion were announced.


### Explicit local audio preview — 2026-10-18

Voice controls now offer Preview local WAV/MP3 through a background file picker and bounded 32 MiB regular-file read. Selected bytes use the existing decoder validation, focused-character metered playback and viseme path shared with Voice Design previews; no synthesis request or library voice-setting rewrite. Voice generation and selected-character checks reject late file results/errors after Stop, a newer speech request or character change. Stop clears staged bytes. Studio controller tests passed21/21, including the new stale-result/settings-preservation regression. Strict all-target Clippy and source formatting/whitespace checked. No full-suite rerun or physical playback/GUI acceptance. Parent-only implementation while the last reported Claude session limit remains in effect; no fresh Fable review.

Local-preview follow-up: extracted bounded reader opens with O_NONBLOCK on Unix before checking regular-file metadata, preventing a selected FIFO from hanging the sole picker worker. Two targeted local-preview tests passed, covering normal bytes, sparse oversized file, directory, missing path, FIFO, stale generations/characters and unchanged voice settings. Strict all-target Clippy passed. No physical playback or full-suite receipt.


### Integrated baseline 4b8f1e4 — 2026-10-18

Announced sequential one-job low-priority `cargo test --all-targets` passed at clean source HEAD4b8f1e4:453 library +95 application +6 integration +2 benchmark =556 tests, zero failures. Includes optional VAD trimming, live mouth diagnostics and bounded local audio preview. Strict Clippy passed on this source in the prior slice. No new GPU, physical playback, microphone, provider or live desktop acceptance. Updated REVIEW-HANDOFF.md to this baseline and identified the newest unreviewed slices; Claude availability still follows its last reported session limit. Compilation/test completion announced.


### Optional automatic reflection drafts — 2026-10-18

Per-character SceneSettings now save an off-default trigger and separate reviewed reflection connection (session keys skipped). The app-open selected-character worker takes a session baseline and waits for 1–12 new accepted source IDs (default3); attempts are separated by10–1440 minutes (default30). Only current accepted non-reflection records are offered, bounded by the existing12-source selector. Pending reflections, worker activity, failed pending persistence or disabled evidence memory pause drafting. Attempts consume the snapshot even on error/abstention; unchanged evidence never retries. At most128 character checkpoints per process; further characters fail closed. Checkpoints are not persisted: restart takes a fresh baseline, with no catch-up. Disabling/changing config or character cancels automatic results before polling; results retain existing citation/current-source validation and always enter the review queue. Pending survival still requires the separate retention setting. Manual connection/settings remain independent.

Initial strict-Clippy application check exposed an egui borrow conflict; copying the manual connection before borrowing the automatic config fixed it. Final strict all-target Clippy passed. Facts-memory36/36 passed, including2 pure scheduling tests and a mock HTTP automatic-draft/disable-before-poll test. Full announced one-job low-priority `cargo test --all-targets` passed559 tests, zero failures. No live provider calls, GUI interaction, physical audio or fresh Opus/Fable review; last reported Claude session limit has not reset. Documentation describes optional drafting, not a Grillo port or automatic fact promotion.


### Synthetic memory behavior evaluation — 2026-10-18

Added `examples/evaluate_memory.rs`, using production asynchronous ledger mutations and recall over random temporary synthetic data. Announced one-job low-priority `cargo run --example evaluate_memory` exited0 and reported8/9 expectation matches. Paraphrase recall failed as documented in MEMORY-OPTIONS.md and the committed benchmark JSON; it was not rewritten as a passing test. No live models, embeddings, user conversations, hardware or GUI were accessed. This is a diagnostic evaluation, not a new full-suite receipt or general memory-quality claim.


### Reflection cancellation before send — 2026-10-18

Reflection draft() now checks cancellation before endpoint/key work and again before send, and sets a60-second per-request deadline in addition to the client timeout. The request-wide deadline follows the reqwest blocking-body behavior already verified in the appraisal slice; no separate60-second drip test was run here, and DNS interruption is not promised. Cancellation after send still only discards results. New loopback regression proves a pre-cancelled call never connects. Existing cancellation test now waits for a connection before cancelling so it continues to test in-flight cleanup; character-switch test waits for the response before changing selection to prove queued results are dropped. Initial test-edit compile failure misplaced a mock-server join; restored proper joins before rerun. Final8 reflection tests passed. Strict all-target Clippy passed; no fresh full suite or live request.


### Generated reflections follow evidence corrections — 2026-10-18

Confirmed acceptance/recall gap: the previous code checked citations only when creating a draft; later source corrections/expiry/forgetting could leave a generated interpretation eligible. Added structured versioned citation JSON within the existing evidence string (UUID, full-text revision, bounded quote), revalidation at acceptance and focused/disk recall, stale-reflection trace counts/inspector warning, removal of stale pending drafts after successful ledger updates, and deletion of linked generated ledger reflections/quotes on source forgetting. Legacy generated provenance is unverifiable: pending drafts are discarded at successful update/load; accepted records remain inspectable but excluded from recall, and unstructured legacy quotations require explicit removal. Manual inspector-authored reflections retain their semantics. No file-schema migration or secure-erasure claim.

Two new regression tests cover acceptance after source mutation, pending invalidation after correction, accepted-reflection expiry suppression, derived deletion on forgetting, bounded escaped citation encoding and legacy/manual distinction. First targeted run38/39 exposed over-broad deletion of newer ordinary fact corrections when forgetting an ancestor; restricted additional cascading to derived reflection chains, preserving existing behavior. Rerun39/39 passed. Announced one-job low-priority full suite passed459 library +95 application +6 integration +2 benchmark =562 tests, zero failures. No live provider, GUI or fresh independent Fable review.

Final strict all-target Clippy, source rustfmt checks and diff whitespace passed for the reflection-provenance slice; compilation completion announced.


## Accepted-evidence semantic lookup — 2026-10-18

Added a saved source selector: explicit notes remain the default; accepted evidence is optional. The evidence projection includes current record text, kind, source and evidence; pending candidates, superseded/expired records and stale generated reflections are excluded. Indexing still requires semantic opt-in plus the selected memory source enabled; non-local embedding endpoints retain their separate consent check. Lookup consumption checks the source scope, loaded character, current revision, saved settings and matching draft, then consumes the result once. Focused chat retains lexical notes in evidence mode and avoids duplicate lexical facts when evidence results attach; retrieved context remains a transient untrusted data message rather than saved history.

The initial targeted compile failed on the generic Bevy ResMut mutability bound; corrected before tests ran. Existing five semantic tests passed, then a new local fake-vector regression passed for source selection, once-only consumption, correction, expiry and forgetting. Added explicit loaded-character validation, pending-candidate exclusion and old-settings default coverage. Full announced one-job low-priority `cargo test --all-targets` passed: 461 library + 95 application + 6 integration + 2 benchmark = 564 tests. Formatting and diff whitespace checks passed.

Fake vectors test lifecycle and isolation only; they do not demonstrate real paraphrase retrieval quality. The committed lexical behavior evaluation remains 8/9, with its paraphrase miss unchanged. No live embedding endpoint, GUI, GPU render or physical audio check was run. Fable review delegation was attempted but rejected by the orchestrator automated causal-hop limit; no fresh independent review is claimed.

Final strict all-target Clippy completed cleanly after the source-character check and additional tests. No compilation remains active from this slice.

## Synthetic memory decision probe — 2026-10-18

Added an explicit-run-only evaluation example and operating notes in `benchmarks/MEMORY-DECISIONS.md`. Eighteen synthetic cases exercise typed memory-triage choices; the tool reads no app data, loads no saved credentials, never writes a ledger, and uses an explicitly configured proxy for remote runs. Three offline tests passed, including a loopback evaluation-wire/strict-choice test. Strict example Clippy passed and a help-only invocation completed without a live request. These Cargo invocations were announced and sequential with one low-priority job. No model quality, cost or speed result is claimed; production classification remains unimplemented pending evidence of benefit. The prior full 564-test application gate remains the last full-suite receipt; this slice only adds an example and documentation.

## Live Jev memory decision pilot — 2026-10-18

Ran `evaluate_memory_decisions --run --model typesafe-ai/jev --rounds 3` with the existing app-saved gateway key injected only into that process environment. 51/54 correct, 0 errors, p50 217 ms / p95 292 ms (first request 3.6 s). The single, consistent miss was the peanut negation fixture (fact expected, preference returned). The output file was checked for credential leakage. No app data, memory ledger or settings were read or written. Pricing was not measured.

## Decision model comparison — 2026-10-18

Ran the same 3×18 memory decision set on seven more Vercel evaluation models. Results: Microsoft Decision-1 54/54; Jev, Liquid d1 and GPT-6 Luna Decisions 51/54; Liquid d1-ultra 51/54; Decider v1.2 48/54; Decider Flash 45/54; Laya 18/54. Zero transport errors. All models were deterministic across rounds. Each output file was checked for credential leakage. No app data or ledger was read or written.

## Decision-model memory triage — 2026-10-18

Added off-default runtime triage (`src/memory_triage.rs`) sharing the benchmark prompt. Six new tests cover prompt/parse/default model, verbatim-quote candidates kept out of recall, correction linking and acceptance refusal after the target changes, retained correction candidates across restart, late results discarded on character change, and disabled/rule-matched turns sending nothing. Full suite 568 passed (467 lib + 95 bin + 6 integration), example tests 3 passed, strict all-targets Clippy and formatting passed. Cargo ran with one low-priority job. No live in-app request or GUI acceptance yet.

## Settings redesign: Memory page and decision-model dropdowns — 2026-10-18

New `settings_kit` (centered column, cards, label/hint rows, toggle switch) and a simplified Memory page: Use memories in chat, Learn from chat (rule candidates + decision-model triage + retention), Suggestion model dropdown, Reflections, a review queue with Remember/Dismiss, and a "What <name> knows" list with Forget and quick-add. The previous inspectors moved unchanged under a closed Advanced fold. Decision models are now picked from a dropdown filtered to Vercel `type: evaluation` (Group chat, Memory, incoming-message emotion, motion suggestion). Global controls are narrower (440 px column, smaller padding/headings). New characters default to memory on (notes and evidence in chat, candidates, triage, automatic reflection drafts); saved per-character choices are kept, and deletion resets to these defaults. Seven tests updated for the new defaults. 568 tests, strict Clippy and formatting passed. Offscreen 1280x800 captures of the empty and seeded fixture Memory page were inspected; no live provider or interactive GUI acceptance.

## Settings redesign: remaining pages and on-by-default features — 2026-10-18

Voice & mic, Scene & avatar, Connections and Group chat now open on short settings cards (`settings_pages.rs`); every original control is kept under a closed Advanced fold. All settings pages share a centered column; the "Jump to section" bar was removed. Connections shows chat/decision models and a Keys card that reports whether each needed key variable is present (values never shown). The decision-model connection moved from Group chat to Connections → Advanced.

New defaults (saved choices are kept): replies read aloud (bundled Hikari voice too), microphone on with automatic send, transcription through the Vercel gateway (`AI_GATEWAY_API_KEY`) instead of direct OpenAI, expressions/local behavior, emotions from chat, head/eye gaze, routine with chat sharing, and automatic body motion after replies. Old saved microphone configs without a backend still mean OpenAI-compatible. Hosted speech with no key is skipped with one notice instead of failing every reply. Tests that pinned the old off defaults were updated or given explicit opt-outs. 568 tests, strict Clippy and formatting passed; fresh-profile 1280x800 captures inspected. No live provider or interactive GUI acceptance.


## Triggered motions play once — 2026-10-18

After the user clarified that the existing settings cover their needs, limited this slice to triggered motion playback; no idle-rotation, settings UI or camera changes. Automatic reply motions (including emotion-mapped playlists) and reviewed suggestion Play now use explicit one-shot imports rather than inheriting the generic looping preference. Natural completion crossfades over 0.5 seconds into the already bundled CC-BY Homunculus standing idle, instead of repeating the gesture or holding its final pose. Manual imports keep their playback path; Pause, Stop and manual replacement remain available. Idle handoff waits for a newer pending import so it cannot cancel that import. No WebWaifu4 assets were copied: the local package declares UNLICENSED and no redistribution grant was found.

Sequential one-job low-priority shared-target checks: focused animation tests initially passed 19/19; final full `cargo test` passed 469 library + 95 application + 6 integration = **570 tests**, zero failures. Includes one-shot completion despite an inherited loop preference, crossfade into the bundled idle, preservation of an in-flight replacement import, and Stop/manual replacement cancellation. `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` and diff whitespace checks passed. No GPU capture, interactive visual acceptance or provider calls; the running user app and real profile were untouched. Camera root-cause investigation and varied idle work were left outside the final scope following the user clarification.


## Companion playlists and compact navigation — 2026-10-18

Added an icons-only navigation rail with existing hover tooltips, local configurable idle/talking playlists, three original stationary BVH clips, bounded eight-clip caching and crossfaded imports. Emotion one-shots take priority; actual speech playback selects talking motions, then quiet returns to idle. Manual motion controls pause the animator until Resume. Desktop mode retains its separate sitting/interaction controller. All 582 tests passed serially (473 lib, 98 bin, 6 integration, 2 + 3 example tests); strict all-target Clippy, package formatting and whitespace checks passed. Normal binary built. An isolated 1280×800 Scene capture was inspected for layout; real speech/motion visual acceptance remains open. The running user profile was switched from saved Gemini to DeepSeek V4.1 Flash, Baseten-only, reasoning None, preserving unrelated settings.


## Desktop controls and contact seating — 2026-10-18

Compared the local Homunculus drag/screen/sitting source: seating is chosen at drag end within an 80-pixel edge band. Closing auxiliary desktop controls now dismisses them without returning to Studio or respawning them; the avatar stays interactive, and explicit Return to studio still restores its window. Sit requests now require nearby surface contact rather than finding a distant surface below the avatar. Drag-drop seating and support tracking remain. All 45 desktop tests passed serially, including controls-close recovery, explicit Studio restoration, distant-seat rejection and contact seating. Strict all-target Clippy and binary build passed with one low-priority Cargo job. Real compositor drag/drop acceptance remains open.


## Homunculus + Mate source integration audit — 2026-10-18

Verified both upstream HEADs and saved the feature/licensing inventory in `reviews/companion-upstream-integration.md`. Standing desktop avatars now use local idle/talking playlists; drag/sit suspend them and suppress reply-body decisions without later replay. Automatic transparent-margin passthrough can recover via fresh global cursor tracking even after dismissing controls; stale geometry restores interaction. Forced passthrough still requires separate recovery. 583 offline tests passed; after the final decision guard, 99 binary tests passed again. Strict all-target Clippy, touched-file rustfmt, whitespace checks and binary build passed. The updated app was relaunched; live compositor/audio acceptance remains open. No Mate source/assets copied, no new benchmarks or live provider calls.


## Fish speech startup pipeline — 2026-10-19

Compared recovered WebWaifu4 and Fish documentation; see `reviews/fish-startup-fix.md`. Shared credential-free HTTP clients now reuse connections, reply WebSockets connect while text is generated, first HTTP/SSE phrases can leave earlier, and WAV playback readiness uses 40 ms of PCM rather than 180 ms. Composer timing now separates speech connection, first text queued and first audio queued (not physical speaker onset). Saved provider, transport, voice, reference and speed settings are preserved.

Serial full all-target suite: **587 passed** (476 lib + 100 bin + 6 integration + 2 + 3 example tests). Strict all-target Clippy, touched-file rustfmt, diff whitespace and normal binary build passed, using one low-priority Cargo job. New offline tests cover HTTP connection reuse with per-request authorization, early WAV readiness, first-phrase grouping and WebSocket connection before reply text. No new paid benchmarks or live synthesis. Actual audible latency and underrun resilience remain unverified.


## Runtime efficiency and hidden assets — 2026-10-19

Normal launcher now selects release mode and one default build job. Avatar textures release CPU pixels after upload while retaining CPU meshes; unchanged outlines no longer dirty material assets; plain Toon/Unlit views skip unused directional shadows. Desktop-suppressed rooms and covered image backgrounds release their asset handles and restore the saved selection on return. GNU/Linux returns unused allocator pages once after a settled avatar change, on a bounded worker and outside active chat/speech. Optional content-free FPS/focus/image-memory diagnostics support comparison.

Final all-target serial suite: **588 passed** (477 lib + 100 bin + 6 integration + 2 + 3 example tests). Strict all-target Clippy, touched-file rustfmt, launcher syntax and whitespace checks passed. Release built successfully; all Cargo work ran as one low-priority job. The updated release app is running.

Short real-process sample after load reclamation: **652 MiB RSS / 524 MiB PSS, 41.9% of one CPU core**, with **59.8 update FPS** reported in adjacent focused windows, normal affinity/allocator settings, 41 threads and no swap. Original debug startup sample was 1014 MiB RSS and 285.5% of one core, without FPS instrumentation. Image CPU storage in the final app was 11.2 MiB. A later NVIDIA process snapshot still reports 843 MiB VRAM; no VRAM reduction is claimed. CPU affinity/arena-limit experiments are not shipped defaults.

No provider/model benchmark, synthesis, microphone or real motion acceptance was run. The screenshot tool returned an occluding browser rather than reliable avatar evidence, so it supplies no visual acceptance. No 200 MiB target, matched competing-engine result, long-run leak freedom or cross-platform efficiency claim is made. Method, experiments and remaining work: `reviews/runtime-efficiency.md`.


## Feature-preserving efficiency follow-up — 2026-10-19

Camera-effect synchronization skips unchanged frames while initializing late cameras and retaining sanitized comparison for non-finite input. Affect configuration is copied only when changed. All 589 offline all-target tests passed before adding opt-in frame-time instrumentation; afterward 12 runtime/behavior tests passed, strict all-target Clippy passed, and the release binary rebuilt successfully with one low-priority job (2m25s). The instrumentation adds p50/p99 wall-frame intervals and mixed-focus marking only when WAIFU_RUNTIME_METRICS is enabled. No FPS or visual-quality default was lowered.

The earlier purported empty-stage memory baseline was invalid: startup automatically imported bundled Hikari. Explicitly disabling the bundle in a new test profile gave 387.5 MiB mapped RSS / 354.6 MiB PSS without profiling. Heaptrack identified 88.08 decimal MB of morph vectors live at capture termination; this is retained CPU data, not a proven leak. See reviews/memory-mapping-measurement.md and reviews/efficiency-followup.md. Arena-limit trials remain experimental; no allocator default has been changed. No new live speech/microphone or visual acceptance is claimed.

## Shared CPU morph backing — 2026-10-19 verification checkpoint

Vendored Bevy mesh 0.19.1 now retains immutable shared CPU morph backing, preserving existing Vec/slice APIs and data for later mesh derivation and device re-extraction. Event-driven byte-exact canonicalization merges duplicate buffers without Modified events or GPU re-upload. Weak cache entries do not retain morph allocations. The private reflection representation change is documented in `vendor/crates/bevy_mesh/WAIFU-PATCH.md`; whole-Mesh reflected cloning is tested. No shader, speech, animation-rate or quality default is reduced.

Final sequential one-job checks: **595 tests passed** (483 lib + 101 bin + 6 integration + 2 + 3 examples), strict all-target Clippy clean, touched-source rustfmt and diff whitespace clean. Five morph regressions cover sharing and replacement isolation, collision-safe byte equality including signed zero/NaN payloads, weak release, extraction errors, reflection cloning, real asset events without spurious modifications, and re-sharing a later replacement. Release build succeeded in 12m17s. Opus 5.5 and Muse Wonder 1.0 read-only implementation reviews found no correctness defect, with review limitations and dispositions recorded in `reviews/`. Matched release runtime measurements and rendering checks remain pending at this checkpoint; predicted savings are not a measured result.


## Efficiency verification refresh (2026-10-19)

Current worktree: **596 all-target offline tests passed** (483 library + 102 binary + 6 integration + 2 + 3 example tests), serially with one low-priority Cargo job. This supersedes the 595-test source checkpoint above.

Six alternating release launches measured an 83.92 MiB RSS/PSS reduction from immutable shared CPU morph backing, with both builds actually at 20 FPS. A separate native AutoNoVsync diagnostic reached 59.8 FPS with unchanged visual settings: RSS 576.37 MiB, PSS 447.96 MiB, CPU 30.41% of one core, logged frame p99 17.91/17.33 ms. The ordinary native FIFO trial remained at 20 FPS. These presentation probes are not a 60 FPS baseline-versus-patch comparison and do not change defaults. X11 failed surface configuration (opaque-only surface versus requested premultiplied alpha), so it has no performance result. See `reviews/efficiency-followup.md` for scope, raw-result paths and caveats.


Final refresh checks: all-target strict Clippy, touched-source rustfmt and `git diff --check` passed after the 596-test run. Corrected offscreen showcase completed 60 frames at its configured capture cadence of 30 FPS; frame 30 was visually inspected. Avatar, clothing, face, outlines and furnished room render, with bundled body motion and recorded audio routed through mouth/meter processing. No missing room-art, rendering error or panic was logged. One Bevy startup warning reported missing render-world time; no live-provider, speaker or perceived lip-sync acceptance is implied. The normal app was restored. Capture cadence is separate from the native 59.8 FPS resource sample.


## Final feature-preserving efficiency pass (2026-10-19)

Current source passes **597 all-target offline tests** (483 lib + 103 bin + 6 integration + 2 + 3 examples), strict all-target Clippy, touched-source formatting and whitespace checks. Release rebuild succeeded. Normal compute maximum is four with unchanged I/O/async policies; a non-metrics runtime check confirms stray diagnostic overrides are ignored and four workers are created. Normal app restored and verified alive.

Measured immutable morph saving: 83.92 MiB RSS/PSS at matched actual 20 FPS. Separate compute-policy comparisons at actual 59.8 FPS lower median CPU by 23.6% (single avatar) and 35.2% (four instances); four distinct VRMs plus room/motion sustain approximately 143 FPS with 29.8% lower median CPU. No visual settings, simulation cadence or FPS caps were lowered. Presentation defaults remain unchanged; native FIFO timing remains a separate known limitation of the test fixture.

Post-change GPU checks switched through four distinct models three times per worker policy, rendering 690 frames total with body motion and nonzero recorded-audio meter output on every shot. Per-model mesh/image/morph counts match across repeated cycles and both policies; late-cycle Riko/Alicia frames inspected. No warnings or errors in these final captures. These are offline renderer/meter/ownership checks, not live provider, physical audio, pointer-hover, Windows, energy or long-duration RSS acceptance. Detailed results, reviewer dispositions and raw-result paths: `reviews/efficiency-followup.md`.


Final shipped-presentation cross-check: six alternating 12/four-worker launches, native FIFO/reactive, both actual 20.0 FPS. Median CPU 10.91% → 8.75% of one core (19.8% relative decrease), lower in every pair. Asset/morph counts match; no additional worker-related RAM saving or frame-tail improvement claimed from this small native-mode set. Raw data: project `work/compute-fifo-controlled-results.json`. Normal app restored; no build or profiler ran during the comparison. Changes are uncommitted; receipts cover the current worktree.

## Emotion-first motion and settings camera fix — 2026-10-19

Reply emotions with no explicit binding now play scanned clips named after them (whole-word match, CC0 prefix stripped; e.g. joy*/happy* for Happy). All 19 non-neutral emotions find 1-8 clips in the local WebWaifu4 library. New scenes default to "Match emotion" body motion and incoming-message appraisal, so the emotion is known while the reply streams; the mapped motion now starts on appraisal and the same chat job is ignored at completion. Existing priority then plays the talking playlist while speech continues and idle afterwards. Saved affect configs without a source keep reply metadata.

Camera: settings pages draw on egui's background layer, which egui does not report as pointer-captured, so scrolling a settings page zoomed the hidden scene camera out (and drags rotated it). Camera mouse input is now limited to the conversation view and desktop mode.

593 tests (one new), strict Clippy and formatting passed. No live provider, audio or visual playback acceptance.

### Final skin-pose diagnostics — 2026-10-20

The existing `render_desktop_motion` example now exports final render-global skin-joint matrices (including spring helpers), root-relative local FK, rest matrices, inverse binds and final skin palettes. Existing humanoid/source JSON remains available. This diagnostic change does not alter runtime animation or physics defaults.

`cargo build --offline --locked --example render_desktop_motion` and strict Clippy for that example passed using the existing studio target directory; touched-source rustfmt and `git diff --check` passed. Native Hikari fixed-side capture produced 91 sample positions for repaired seed108 plus one known-bad text-only seed109 pose. The selected-surface final-skin audit measured 0/91 crossing frames for the candidate and 209 crossings in the bad pose; maximum candidate chest displacement versus static helpers was 1.62993 mm and global/local-FK matrix disagreement 2.98e-7. Missing palettes/joints, wrong inverse binds and stale globals were deliberately rejected by the audit.

These are diagnostic receipts, not candidate acceptance or a self-collision fix. Selected geometry, no morph/containment/swept checks, paused sampling and unchanged example spring settings cannot establish realtime maximum-jiggle clearance. The user's visible-clipping rejection remains in effect. Artifacts and audit script live in the neighboring task workspace and spare-drive `waifu-motion-library/clearance-trial/final-skin-v1/`. Full app tests were not rerun for this example-only change; no commit/push.

### Core motion ownership and camera checks — 2026-10-20

Current worktree: **622 all-target offline tests pass** (497 library + 114 binary + 6 integration + 2 and 3 example tests). Strict all-target offline Clippy, touched-source rustfmt checks and `git diff --check` pass. The debug application build also passes. Evidence logs are copied to the task workspace's `reports/stage1-evidence/`; no commit or push was made.

Manual controls now hold body-motion ownership until Resume, cancel late automatic imports, and consume intervening reply IDs without replaying them on release. Resume is available in the ordinary Scene Behavior card. A real animation-system regression covers one-shot reaction completion into talking while speech remains active, or idle when it has already ended. Separate regressions cover character/avatar ownership, a new utterance, and bounded (750 ms) speech chunk gaps.

Every production speech spawn now carries immutable character/voice-generation identity and a decoder-consumption receipt. Queued assets and decoder creation alone do not start talking; consumed silent samples do. EOF, decoder drop, replacement and Stop invalidate playback. Receipt and source-ownership regressions run without providers or an audio device; they do not establish physical speaker timing.

Camera coverage exercises settings scroll/drag input, the F shortcut, explicit recenter/framing, resize, preview orbit/zoom/focus, and preview texture limits. F no longer reframes the hidden main camera from settings; preview target resizing preserves aspect ratio. The original pure settings-toggle zoom complaint has not been reproduced, so these corrections do not close that user-visible acceptance item.

Native isolated-profile startup loaded Hikari (`VRM_READY`); an offscreen 1280x900 Scene settings capture was rendered and inspected. Interactive desktop verification could not run because the KDE input activity guard is missing; it was not bypassed. The isolated native instance was stopped. Capture/logs: `/run/media/subsect/toshiba/kimodo/waifu-motion-library/validation/stage1-camera-20261020/`. Live spoken-conversation transitions, physical audio, interactive camera acceptance, strong-jiggle clearance, and extension runtime/API acceptance remain outstanding.

### Continuous motion probe and chest spring controls — 2026-10-20

Current source passes **642 offline all-target tests** (511 library + 120 binary + 6 integration + 2 and 3 example tests), strict all-target Clippy, changed/new Rust formatting, whitespace checks and debug application build. One low-priority Cargo job was used, sequentially. Current receipts are in the task workspace's `reports/stage1-evidence/followup-*.log`; earlier 622-test logs are preserved. No commit or push.

The opt-in offscreen showcase now validates a diagnostic timeline and records final motion/speech/camera/spring state. Recorded audio uses the production preview/meter/viseme paths, without providers or speakers. This exposed and fixed stale automatic gestures on character/avatar switches and an idle fallback before talking. Added regressions also cover paused single-idle Resume, transient importer retry without blacklisting, preview replacement, and applying camera framing during windowless capture without accepting physical desktop input. Take2 completed 525 frames at configured 15 fps: reaction → talking → idle, explicit speech stop, clean character switch, and one unchanged camera transform throughout the scripted settings round trip. Full-body Hikari and Neuro settings frames were inspected. Native interactive camera and live/provider audio acceptance remain open.

Per-character chest spring presets now retain authored model settings, offer Expressive and Maximum jiggle, and expose manual joint selection under Physics details. Tuning is reversible, uses real selected-model spring joints, and leaves gravity, colliders, hair/clothing and the vendor simulation intact. Defaults retain authored tuning (strength 0); no maximum-jiggle default is claimed. Take2 exposed zero tuned joints because the controller checked the wrapper instead of the vendor's nested scene root. The corrected lookup and unchanged-frame cache pass 14 spring regressions, including nested roots and restoration. Continuous post-fix renderer evidence remains pending at this source checkpoint. Neither these controls nor existing selected-surface diagnostics guarantee mesh self-collision safety or approve the motion library.

Post-fix take3 completed 525 frames / 35 seconds at configured capture cadence 15 fps. It discovers 104 actual spring entries on Hikari and 131 on Neuro, tuning four real bust joints at strength 1 on each throughout their shots; selected spring transforms are finite. Nine trace checks pass, covering frame completeness, reaction/talking/idle, no idle fallback, Stop, avatar switch, scripted settings camera preservation, tuning and mouth weights. Hikari enters talking at frame152 and idle at199 when recorded audio ends; Stop ends consumption at42 in shot1. Neuro begins idle despite the previous unfinished reaction; settings open105/close135 without camera-transform changes. Talking and settings images were inspected, including the correct four-joint UI status. Artifacts: spare-drive `waifu-motion-library/validation/stage1-motion-20261020/frames-take3/trace.json`, `take3-receipt.json` and `render-take3.log`. This supersedes the pending post-fix renderer status above; native interaction, live speech, strong-deformation visual acceptance and mesh clearance remain separate open checks.

### Continuous final-surface diagnostics and independent reviews — 2026-10-20

Four fixed front/side captures of repaired108 and known-bad109 completed 180 frames each at 30 app updates/sec, with four real chest joints tuned at strength1,16 source primitives, actual morph arrays and final spring palettes. GPU readback does not pause or seek the animator. Each app update corresponds to two expected vendor60Hz spring substeps; substeps were not individually instrumented. Original metadata named app cadence `simulation_*`; separate clarification receipts preserve that history. New exports use explicit app/spring fields. Continuous capture now rejects nonaligned or clamped cadences and gross clock drift. A fresh four-frame GPU smoke verified the new fields, paired frames and active tuning.

The CPU baseline auditor processed all360 front-view poses in263.84 seconds. Every source primitive matched with zero source/palette errors; source snapshot and per-frame witnesses are retained. Known-bad109 has distal-arm/torso strict edge/triangle incidences in119/180 frames and distal-arm/lowerbody incidences in115/180. Repaired108 has zero in both comparisons but upper-arm/torso incidences in180/180 frames, including mixed-weight shoulder/chest skin. These counts are not collision depths or visual acceptance. Containment remains unknown on open/nonmanifold target components; no clip or library is approved and no swept guarantee exists.

Read-only reviews completed with served `claude-opus-5-5` and Meta `muse-wonder-1.0-contributor` (configured `muse-wonder-1.0`): `reviews/opus-stage2-export-review.md` and `reviews/muse-stage2-geometry-review.md`. Opus's app/spring labeling issue and extension target-generation ambiguity were corrected. Muse's empty-comparison, joint-identity and convention findings are tracked in the task auditor report; the360-frame baseline run predates those evidence-check changes. Reviewer source inspection is distinct from parent test/GPU evidence. Warmup and visible pixels do not independently prove every shader pipeline drew every primitive.

Current source passes **646 offline all-target tests**, strict all-target Clippy, debug app/example builds, touched-source formatting and whitespace checks. Task `reports/stage2-progress.md` and `stage2-evidence/review-*` contain the detailed receipts and source hashes. Native input, providers, physical audio, maximum-deformation visual acceptance, starter-pack acceptance and extension runtime remain open. Nothing was committed or pushed.

Final auditor follow-up: **45 CPU Python tests passed** (34 auditor plus11 existing decoder tests). Auditor schema1.1.0 rejects empty comparisons, ambiguous unnamed/duplicate-name palettes without explicit source joint indices, unknown matrix convention and unhashed avatar relocation. Two historical and two fresh-format real frames pass source/comparison coverage checks with all16 primitives; the fresh export retains explicit app/spring cadence without claiming measured spring counts. These bounded checks do not rerun or reclassify the360-frame baseline.

### Content packs and extension foundations — 2026-10-20

717 offline all-target Rust tests and strict all-target Clippy passed. All 30 touched/new Rust files pass formatting and whitespace checks; the debug app builds. This extends the earlier stage-2 receipt rather than retroactively changing it.

Content-only manifests, durable revision installation, an exclusive store writer, background management and the compact Extensions page are implemented. Enabled motion roles/emotion metadata join the host catalog; explicit use applies existing import/playback/room/preset paths. Disabling pack motions revokes pending/playing poses while retaining manual ownership and unrelated imports. Enabled expression mappings honor available expressions and manual/reserved ownership; disable/update removes their owned contribution immediately. The original three-clip MIT Companion Starter is included; no generated full-body clip is approved.

MotionServices advances ownership/catalogs in ordinary Update, independently of egui. The protocol/session gate tests distinguish session/process/target identities and positive request sequences, enforce capability/event namespace checks and prevent replay after bounded-cache eviction. Linux process-transport tests use actual children for exchange, malformed/oversized/flooded output, closed stdin, bounded stop/timeouts, final-output draining and escaped-descendant drain bounds. Other-platform supervision and executable-plugin integration remain unsupported checkpoints; external programs are not sandboxed.

Opus 5.5 and Meta Muse Wonder supplied alternate source reviews. Their findings led to kill-before-reap group handling, bounded exit drain, durable terminal receipts, explicit bounded shutdown, exclusive store ownership, visible busy rejection and staging cleanup. Render-frame drops do not join arbitrary copy/dialog work. A preexisting copy can finish under its lock after state drop; replacement stores retry safely.

Extensions layout captures at 1280×800 and 800×720 were inspected. Their isolated profile was seeded with a managed pack, so those images establish layout, not native input or folder-dialog acceptance. Real worker/store lifecycle behavior is tested separately. Final source hashes, commands, logs, review dispositions and remaining boundaries are recorded in `/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/reports/stage4-progress.md` and `stage4-evidence`.

Stage 2 additionally completed 728 fresh rest/correction control frames with strong physics. Two rest and two initial corrected candidate frames have zero audited arm/body incidences, but the existing correction visibly over-spreads the arms. It stays experimental/off. Those bounded results do not establish full-sequence, between-frame or per-VRM clearance. Native pacing, live audio/provider checks, hooks/process host actions, Twitch, Discord and platform acceptance remain open. Work is uncommitted and unpushed.

## Host-action execution checkpoint — 2026-10-20

Added `extension_actions.rs` and `extension_connection.rs`. A bounded coordinator now joins the actual Linux process transport, typed protocol and replay gate. It dispatches opaque execution tickets, returns cached/rejected results without re-execution, rechecks current permission/target/process identity, and stops on malformed/flooded transport. `send_host` adapts the encoder's LF framing to the transport deliberately.

The host executor resolves namespaced registered motion IDs, preserves manual/desktop priority, applies separately owned temporary expressions, supplies integration event attribution and cancels owned effects when the connection stops. Tokens prevent stale cancellation from stopping newer same-file/manual motion. Expressions expire/cancel without overwriting host affect/lifecycle; manual and protected facial channels retain priority. External message delivery explicitly returns Unavailable until its attributed persistence/UI path exists; it never routes external text through user-fact extraction.

Review fixes include cancellation before desktop playback is parked, active-owner reclamation, predictable unknown-asset errors and serialized catalog-size limits including escaping. Speech receipts independently latch consumption history and terminal state so short playback is observable after EOF; this is a source prerequisite, not public speech event delivery.

**745 offline all-target Rust tests passed; strict all-target Clippy, touched-file rustfmt and whitespace checks passed.** One actual supervised Python fixture applies an expression through coordinator → admission gate → host executor → shared BehaviorController, receives acknowledgement, and loses its contribution on disable. Additional real child tests cover duplicate replay, malformed output and lifecycle invalidation. This is execution/control evidence, not visual/native UI or live service acceptance. Logs and exact source hashes: `stage4-evidence/actions-*`.

Executable manifests remain disabled. The normal app has not yet wired a persistent runtime manager to this bridge. Next: retain/drain source lifecycle receipts with explicit overflow outcomes and avatar identity; add attributed external notifications; then wire runtime lifecycle/configuration, one-shot hooks and persistent fixtures into the compact Extensions page before enabling executable installation. No commit/push or credential/proxy changes.

## Source lifecycle and app-runtime checkpoint — 2026-10-20

Focused and scene replies now record completion/failure/cancellation at their source with captured character and reply IDs. The scene transcript's persisted shape remains compatible. Scene speech queues carry the same reply IDs into actual audio registration. Every production audio spawn registers an independently retained consumption receipt; starts and terminal observations survive short playback and entity destruction. Explicit cancellation waits for actual receipt invalidation/termination, preserving consumption between cancellation intent and deferred audio stop. Already-terminal receipts retain their observed terminal reason.

The source journal is bounded to 256 records/2 MiB, with monotonic sequences and explicit coalesced loss records. Speech tracking is bounded to 64 receipts. A per-connection relay holds at most 32 records/1 MiB and sends at most eight wire events per frame. Full reply text streams in UTF-8-safe chunks with byte offsets, followed by terminal metadata. Lifecycle metadata and private reply text use separate `lifecycle.observe` and `reply.read` grants. Revocation permanently purges queued private text; source/queue overflow explicitly stops the affected subscription rather than silently dropping an apparently complete stream.

`ExtensionRuntime` is now advanced by the actual app's ordinary Update chain, using shared host animation/behavior services independently of egui. It supports four explicitly configured supervised processes, no-avatar metadata delivery, fresh-session explicit restart after authoritative termination, status and disable/removal cleanup. A transient controller revoke/regrant is latched until owned effects are cancelled. Disabled processes immediately discard queued private text. Avatar target generation follows actual character/entity incarnation, independently of speech/reply changes. Empty runtime frames avoid catalog construction. A retained runtime clone performs bounded shutdown after the app runner returns.

The existing Extensions page can show compact local-runtime status, enable/disable and Restart rows when explicitly configured runtime entries exist. **Executable installation/configuration is still rejected by content-only manifests**; the default app runtime is empty. No arbitrary program discovery or automatic crash retry was enabled. `message.deliver` still returns Unavailable until typed attribution survives storage, UI and memory intake. One-shot event hooks, motion/emotion source events and live services remain separate unfinished work.

**783 offline all-target tests passed**, strict all-target Clippy passed, all 13 touched/new files passed rustfmt, whitespace checks passed, and the debug app builds. Actual Linux Python-child tests cover large UTF-8 reply reconstruction, text revocation, relay overflow, no-avatar metadata, expression control after avatar availability, restart and disable, including transient permission changes and private backlog destruction. Focused-source tests retain multiple replies past the latest-result slot and retain speech events after entity removal. An initial parallel integration run hit an existing scene-worker admission timeout; the retained log records it. Subsequent complete suites passed; no permit leak or production-limit change was asserted.

The current debug app rendered the isolated content-pack Extensions screen with VRM_READY and no schedule/resource panic. Picture and receipts: `stage4-evidence/lifecycle-*`, `waifu-lifecycle-extensions.png`. That image has no configured executable entry and does not prove runtime-row pointer interaction, native input or physical audio. No commit/push or credential/proxy changes.

Next: explicit executable-package validation/install/configuration and startup binding to the runtime; attributed external notifications; owned event-hook fixture and persistent sample; Twitch replay before live account acceptance. The API remains an integration draft and the overall roadmap is incomplete.


## Installed local integrations — 2026-10-20

Explicit local-code installation now binds validated copied revisions to the app runtime. New installs are disabled without grants; accepted enabled state and still-requested grants survive explicit updates and app restarts. Content/executable trust-type changes are rejected. Separate stable data directories, compact saved permission controls, status/restart and an owned Python lifecycle-observer example are implemented. Capacity deferral avoids latching a startup failure while an old process terminates. General manifest-defined settings and credential references are still unfinished.

Final serial offline all-target suite: **801 passed** (652 library, 134 app, 6 integration, 2 benchmark, 7 example tests), zero failures. Strict all-target Clippy, scoped rustfmt and whitespace checks passed. The final UI wording clarification was compiled and linted after the suite. The parallel run hit the previously observed scene connection-admission timeout; its failure log is retained, with no production-limit change or claimed repair. Debug app built.

Actual offscreen Extensions rendering in an isolated profile showed the installed example process Running. The child wrote a target.changed receipt for the loaded avatar. This proves app startup binding, process/event delivery and rendered layout, not native pointer/dialog interaction, physical audio, live services or cross-platform execution. Linux-only supervision remains explicit. No provider calls, real-profile credential changes, commits or pushes.

Next: manifest-defined typed configuration/credential references, owned one-shot event hooks, attributed external-message persistence and intake, then Twitch replay before live service acceptance. Current API remains a draft.


## Typed integration settings checkpoint — 2026-10-20

Implemented strict manifest-defined toggle/integer/text/choice settings with bounded schemas/defaults and plain value storage separate from grants. Content-only installs reject these executable settings. The store persists complete effective values with its existing atomic/rollback semantics and migrates compatible values against copied revisions; unknown, wrong-type, oversized and invalid-range values are rejected. New fields default compatibly for earlier indexes. Credential references remain unfinished; these values are not password storage.

The Extensions page folds controls under Settings, keeps a draft until explicit Save, supports Reset defaults, and displays inline validation. Startup handshakes carry validated settings. Changed saved settings wait for old-process termination and launch a fresh session; equivalent saves do not restart code. Manual restarts retain configuration. Fixed the A → B → A while-stopping cache defect found during review. The lifecycle-observer example uses all four control types and records its actual startup settings.

Final one-job low-priority offline all-target suite with one test thread: **819 tests passed** (670 library, 134 app, 6 integration, 2 benchmark, 7 example tests). Strict all-target Clippy, scoped rustfmt and diff whitespace passed; debug app built. Real-child regression covers typed settings received after disk reopen, changed and equivalent saves, fresh sessions, manual restart, disabled reopen, and reversal before termination. Earlier intermediate failure was an obsolete expected error-string assertion, corrected to match explicit integration installation; logs retained.

Hooks remain rejected, not implemented. The reviewed design records bounded invocation queues, shared process admission, permanent payload reduction on revocation, lifecycle trigger grants, captured source/target identity, explicit expiry/failure and no automatic replay. Motion-completion source receipts are a prerequisite for useful animation hooks that await their own completion. Next: implement those receipts and the bounded one-shot hook path; credentials and attributed external message intake remain later work. No provider calls, real-profile changes, commits or pushes.

Actual 1280×900 offscreen Settings capture shows all four typed controls with the installed integration Running. Its child startup receipt matches the configured toggle, choice, text and integer values. Native pointer/typing remains unverified. Receipts and source hashes: stage4-evidence/settings-*. No build or capture remains running.


## Correlated motion results and Reply gesture example — 2026-10-20

Owned one-shot requests now expose independent retained pending/completed/cancelled/failed receipts. The animation source records natural completion at its terminal frame, cancellation on replacement/manual control/stop/parking/drop, and failure on asynchronous import errors. Idle handoff cannot overwrite a terminal outcome. Only current/import receipt slots live in AnimationState; independently retained host records are capped at32 and released on disconnect.

The draft motion.finished event requires captured target, registered motion ID, original action ID and terminal outcome. The host emits it once to the original active session/process under LifecycleObserve, preserving cancellations after target or control-grant loss and discarding unobserved/disconnected receipts. Newer requests cannot overwrite earlier terminal attribution. No-avatar runtime frames now reconcile and publish source cancellation rather than simply deleting the receipt.

The owned Reply gesture Python integration includes one unchanged original upper-body nod with MIT attribution. Configured reply-finished or speech-started events request that registered motion only for the matching loaded source character. It waits for correlated terminal results, respects cooldown and host busy/manual/desktop ownership, and retains one latest-result file. It requests no reply text or provider key. The Extensions example menu offers both observer and gesture examples. This is a persistent event/action integration; manifest-launched one-shot process hooks remain rejected.

Final serial offline all-target suite: **831 passed** (682 library,134 app,6 integration,2 benchmark,7 example tests). Strict all-target Clippy, scoped rustfmt, whitespace and debug build passed. An installed real Python process received a source reply event, requested the bundled motion through the supervised protocol, used actual asynchronous BVH import and the production animation plugin with virtual time, and received a completed terminal receipt. Headless source skeleton evidence is not native or visual playback acceptance. Other real-child/source tests cover all terminal outcomes, grant/target loss, same-file replacement, queue bounds and disconnect cleanup. No live provider/Twitch calls, real-profile changes, commits or pushes.

Next: bounded manifest-launched hook invocations; emotion source events and attributed external-message persistence/intake; Twitch deterministic replay followed by live account acceptance. Motion source completion is now available for hook implementations to await; the overall roadmap remains incomplete.

Actual isolated offscreen Reply gesture Settings capture shows its process Running, trigger dropdown and cooldown control with VRM_READY and no schedule/resource panic. No event was injected through this capture; event-to-motion execution is established separately by the real-child headless test. Native pointer/typing and visual gesture acceptance remain unverified. Receipts and source hashes: stage4-evidence/motion-events-* and reply-gesture.png. No build or capture remains running.

Render review replaced raw trigger protocol names with Reply completed / Speech started in the sample settings. The actual installed-child event→motion→terminal test passed again against this final sample; refreshed render captured. Rust code is unchanged from the831-test and strict-Clippy snapshot.


### One-shot hooks checkpoint — 2026-10-20

848 offline all-target tests passed. Strict all-target Clippy passed after two test-only literal-unwrap fixes; scoped rustfmt and diff whitespace checks passed. Debug build and 1280×900 offscreen Extensions capture passed: enabled hooks show Waiting for events, folded permissions and compact settings. The capture is an isolated manual installation fixture, not native-input acceptance. Real installed Python handler / production motion playback tests separately prove nod terminal receipt and process-slot release.

A subsequent runtime review identified that launching a queued invocation could overwrite an overflow status. A retained loss indicator is being added and requires renewed tests. Emotion transition events are the next slice. Twitch live integration, attributed external message delivery and native visual playback remain unfinished. No commit or push.


### Emotion transitions and retained hook losses — final checkpoint

860 offline all-target tests passed; strict all-target Clippy, scoped rustfmt, diff whitespace and final debug build passed. This supersedes the earlier pending-loss-fix checkpoint. The new public-frame overflow regression verifies losses remain visible after launch and Reset. Emotion wire/relay tests use actual Linux Python transport without an avatar and verify observation permission and permanent revoke/regrant purge. Source tests cover focused and scene ownership, same-label suppression, stale/failed result suppression, and scene configuration/cancellation neutral resets.

Evidence/source hashes are archived as `stage4-evidence/emotion-*` in the task reports folder. The earlier hook capture remains layout evidence for the unchanged page, not a capture of native playback or this later source state. One-shot trigger choices remain reply/speech only; emotion metadata is available to persistent observers. Attributed external message delivery and Twitch replay/live remain unfinished. Nothing committed or pushed; no build remains running.


### Attributed messages and offline Twitch replay checkpoint

881 offline all-target tests passed, followed by one additional replay-control test (882 distinct passing tests). Strict all-target Clippy, scoped rustfmt, whitespace checks and debug build passed. The real installed Python replay uses the production runtime/motion plugin and an actual Store sink: one attributed external message persists, then the registered nod reports Completed. The binary sink separately proves no memory candidates/triage/generation, stale/busy rejection, and unconfirmed-write quarantine. Provider tests preserve local-only bytes and image placement with external envelopes.

Review fixes cover stale UI overwrite after ambiguous commit, cancelled scene writers retaining exclusivity through actual termination, duplicate result suppression and target pinning. An isolated offscreen app run delivered the message through the actual binary adapter and accepted the nod. That still capture ended before a terminal nod receipt, so completion evidence is the separate runtime test. A second run with the replay disabled reopened the stored message and rendered External · Twitch replay at 1280×900. No native desktop-input, visual-motion quality, live Twitch, or render-frame write-latency claim. Receipts/source hashes and inspected image are archived as stage4-evidence/external-* in the task report folder. No commit or push.


### Twitch receiver checkpoint — 2026-10-20

Implemented private per-install credential definitions/storage/status/UI and the bundled receive-only native Twitch worker. Token identity/scopes, hourly validation, selected chat/subscription/cheer subscriptions, proxy-preserving observation transport, actual motion terminal correlation, reconnect draining and latched input failures are implemented. Saved secrets stay outside settings/index/handshakes; masked UI drafts do not read saved values back. Device sign-in/token refresh and real-account acceptance remain pending.

Verification: full offline all-target suite passed 919 tests before the final dedup change. Updated reducer then passed all 15 dedicated tests (three additional tests, 922 distinct tested cases across these runs). Final strict all-target Clippy and scoped rustfmt/whitespace checks passed. Build/UI capture pending at this checkpoint. Local HTTP/WebSocket/proxy fixtures use fake credentials; the actual bundled launcher test verifies missing credentials fail before network startup. These are not live Twitch evidence.

Review fixed duplicate/control reconnect drain handling, duplicate-based liveness, retained stdin overflow/read errors, and eviction replay. Dedup retains 256 IDs for330 seconds, including control envelopes; at capacity the worker stops explicitly rather than evicting or reconnecting. Busy-channel scalability remains limited. Process Restart resets this in-memory cache. No commit or push.

Final checkpoint: debug binary built successfully; final targeted Twitch worker/reducer tests passed (see archived twitch-final-targeted.log). Rendered isolated 1280x900 Extensions overview and expanded credentials captures succeeded; expanded capture visually inspected. This is offscreen rendering, not native interaction or live Twitch acceptance. No build remains running.


### Emotion one-shot hooks — 2026-10-20

Manifest hooks now accept `emotion.changed` alongside reply/speech triggers, including all four unique handlers in one installation. Captures preserve the original character, focused/scene owner, labels and genuine optional reply correlation. Observation is required; no reply text or action target is added. Existing bounded process/queue/expiry/permission rules remain in force. Revoking observation permanently discards queued captures.

The Extensions example menu includes an owned metadata-only Emotion hook. The actual package installs through ExtensionStore, binds through InstalledRuntime, starts a real copied Python handler, records original metadata and exits. Disablement prevents receipt changes and re-enablement waits for a fresh event. This is headless process evidence, not native input/visual-motion or live Twitch acceptance.

925 offline all-target tests passed, followed by the added real installed-example test (926 distinct). Strict all-target Clippy passed before that new test; its targeted strict Clippy also passed. Scoped formatting/whitespace and normal debug build passed. UI capture/review pending. No commit or push. The overall roadmap remains active; live Twitch sign-in/account acceptance, broader mapping UI and streaming/distribution acceptance remain open.

The isolated 1280x900 Emotion hook Extensions capture rendered and was visually inspected. A separate read-only source review found no actionable defects. Normal capture is a manually seeded disabled installation; actual installation/launch is proven separately by the real-child test. Native pointer/keyboard interaction remains unverified.

800x900 narrow Extensions capture also rendered and was visually inspected: visible controls fit without horizontal clipping. No build/capture remains running.


### Registered motion selectors — 2026-10-20

Integration settings now support a generic registered-motion dropdown with human/pack labels, search, virtual rows, bounded/truncated controls, None and retained unavailable selections. The 193-byte ID validation matches MotionPlay; migration and unrelated saves preserve valid references without requiring current pack availability. Selecting never grants control or enables a pack. Twitch Alert gesture uses the selected ID, defaults to the original nod, and explicit None suppresses gesture requests without consuming cooldown or blocking messages.

937 offline all-target tests passed, including four selector tests, schema/migration, Twitch selected/default/None/invalid/terminal behavior and persistence across disable/reopen. Strict all-target Clippy, scoped formatting/whitespace and debug build passed. Isolated normal1280x900 and narrow800x900 offscreen captures rendered and were visually inspected; controls fit. Fixture data is manually seeded without secrets; real store persistence is covered separately. No native-input/live Twitch acceptance, commit or push. Reply/hook-gesture sample selector follow-up is still in progress; this checkpoint's checks do not establish that slice.


Final motion-selector checkpoint: both Reply gesture and Reply gesture hook packages now expose the same Gesture dropdown, retain original nod fallback when absent, and honor explicit None without action requests. Three actual subprocess test drivers cover default/custom/193-byte IDs, disabled/None behavior, invalid settings and terminal waiting. Final source/package snapshot passed **940 offline all-target tests**, final strict all-target Clippy, scoped rustfmt and whitespace checks. Original animation assets remain unchanged. Normal debug build and inspected normal/narrow captures are from the unchanged Rust production snapshot. Independent read-only reviews found no actionable defects. Evidence: stage4-evidence/motion-selector-*. No build/capture remains running; nothing committed or pushed. The full goal remains active: generic character/expression/event mappings and test actions, live account setup/acceptance, native streaming and platform acceptance remain outstanding.


### Avatar expression selectors and Twitch pairing — 2026-10-20

Integration schemas support exact avatar-expression selectors with None, retained unavailable values,128-byte/control/nonblank bounds and reserved blink/mouth/gaze exclusion. Expressions retain case/internal spaces. Extensions uses the loaded FacialState catalog with bounded searchable dropdowns. Choosing a value does not grant ownership.

Twitch optionally applies a temporary expression at75% for2000ms before requesting the selected gesture. Message, expression and motion use distinct correlated ACK stages; motion waits for its actual terminal. One shared cooldown covers the reaction. Default expression is None, absent expression permission preserves the existing motion path, and target changes/rejection/timeouts clear the chain. Existing installations retain old grants without automatically gaining the new requested expression capability.

954 offline all-target tests passed, covering schema/migration, dropdown interaction/search/bounds, saved names without an avatar, upgrade grant preservation, expression-only/both/neither, duplicate ACKs, target/cancellation/cooldown and noncelebrating messages. Strict all-target Clippy and scoped formatting passed. Normal build/UI capture pending at this checkpoint. No real Twitch calls or native visual reaction acceptance; no commit or push. Overall goal remains active.


Final expression-selector checkpoint: normal debug build passed. Isolated1280x900 and800x900 offscreen captures loaded Hikari (VRM_READY), displayed the actual available happy expression beside the gesture dropdown, and were visually inspected with no horizontal clipping. Final help text was shortened to explain overlapping expression/gesture behavior; no Rust/behavior changes followed the954-test/Clippy snapshot. Captures use a manually seeded disabled Twitch installation with no credentials; they do not demonstrate live events, native input or visible expression/motion reaction. Source reviews found no further actionable defects. Evidence: stage4-evidence/expression-selector-*. No build or capture remains running. No commit/push. Live account setup, mapping test actions/character selection and broader streaming/platform acceptance remain open; the overall goal stays active.


### Explicit saved-reaction test — 2026-10-20

The Extensions card now offers Test reaction for packages declaring test_process. It runs saved settings in a fresh supervised child pinned to the loaded character, under existing permissions and the shared four-process budget. Stop, disable, reconfigure, permission loss and target change cancel it. The parent retains process status; process success does not establish visible playback. The owned Twitch tester uses no network, credentials or external messages.

966 offline all-target tests passed, including actual bundled Python subprocess cases and installed-store/host expression ownership, completion and disable cleanup. Strict all-target Clippy, scoped rustfmt, whitespace checks and normal debug build passed. Whole-workspace cargo fmt is blocked by existing missing vendor example camera modules; scoped changed-source formatting passed. Independent read-only runtime review found no actionable defects. Evidence: stage4-evidence/reaction-test-{tests,clippy,build}.log. Native visual reaction, updated UI captures and live Twitch acceptance remain unverified. No commit or push.

User clarified that “irtc” meant IRC chat, not a channel name. No channel or Twitch application ID is confirmed. Current Twitch implementation uses EventSub; an IRC chat option requires separate evaluation and must preserve proxy routing and third-party memory attribution. Overall goal remains active.


Reaction-test capture follow-up: isolated 1280x900 and 800x900 Extensions captures rendered successfully and were visually inspected; visible controls fit without horizontal clipping. A focused 1280x900 Twitch-card fixture shows the Saved reaction row and correctly disabled Test reaction button for a disabled installation. Advanced capture shows saved expression/gesture selectors and empty credential status. These manually seeded offscreen fixtures do not establish enabled-button interaction or visible playback. Evidence: stage4-evidence/reaction-test-{ui,narrow,details,card}.{png,log}. Twitch official chat documentation (https://dev.twitch.tv/docs/chat) currently recommends EventSub/API and describes IRC as limited; retain EventSub while evaluating compatibility separately. No live account call or credential change.


Enabled-test UI follow-up: isolated 800x900 fixture shows enabled Twitch, failed credential-free service, available Restart and enabled Test reaction, with no visible horizontal clipping. Worker has no credentials; this verifies independent test availability rendering, not a button click, playback or live service. Added docs/twitch-setup.md with source-checked account/scope binding and current limitations; marked design-history wire examples as nonauthoritative. Browser device-auth core implementation is now underway separately; no sign-in feature completion claimed.


### Explicit Twitch device authorization and author guide — 2026-10-20

Credentials now offers Connect with Twitch, explicit Open browser, transient authorization code/status and Cancel for the owned Twitch package. A registered public-client application ID is entered for each attempt; no client secret is used. Manual credential fallback remains. Saved event selection shares scope mapping with the receiver. Bounded HTTP workers retain existing environment proxy routing, prohibit redirects, cap responses at64KiB and use5s connect/8s request deadlines. Polling respects interval/slowdown/expiry; returned tokens are validated for application, user identity and required scopes before private persistence.

Only one auth worker is admitted at a time, including cancellation/reaping. Pending results are pinned to the entire installed snapshot and credential revision. Store-owner writes use an atomic admission gate; cancellation before commit prevents saving, while a started write cannot be undone. Installation/configuration/credential changes discard pending work. Saving does not enable a package or grant host capabilities. Access/refresh tokens are stored together; automatic refresh is NOT implemented. Save-error status acknowledges uncertain persistence and refreshed credential status rather than falsely claiming nothing was saved. A late rejection for a cancelled write preserves the cancelled status.

976 offline all-target tests passed. Subsequent command boxing/style and cancellation-status refinement passed33 final auth-focused tests. Final strict all-target Clippy, scoped rustfmt/whitespace and normal debug build passed. The six auth-core tests use local fake HTTP, covering pending/success/slowdown, denial/expiry, cancellation/late-token discard, URL/input/body bounds and validation identity/scope failures. Actual extension-store worker regression covers private once-only saving, unchanged enable/grants and cancellation. Source review found no remaining actionable auth issue. New docs/extension-authoring.md includes source-backed exact protocol examples; an actual Rust parser/validator regression checks every copyable JSONL record. Local links resolve.

Isolated1280x900 and800x900 offscreen captures loaded Hikari and were visually inspected: Connect and manual credential controls fit without horizontal clipping. Fixtures contain no credentials; Connect is correctly disabled until an application ID is entered. Captures predate only the final cancellation-status refinement, which does not alter this idle layout. Neither native pointer interaction, visible challenge/consent interaction nor live Twitch authorization/event delivery is established. User explicitly requested continued offline work; do not start live authorization/account calls. Automatic refresh, recorded native event-to-reaction/capture acceptance and remaining roadmap gates stay open. No build/capture remains running. No commit or push; overall goal active.

Evidence: stage4-evidence/twitch-sign-in-* and extension-authoring-tests.log; source hashes captured separately.


### Host-owned Twitch refresh — 2026-10-20

Device authorization now stores validated OAuth account/application/scopes and expiry metadata atomically with its access/refresh pair in the private credential file. Existing files load without metadata; manual value changes clear it and no-op saves retain it. Public status, handshakes and settings contain no metadata or tokens. OAuth metadata and values are bounded and validated together.

For enabled Twitch installations with this metadata, the host schedules refresh within five minutes of expiry using the single global authorization slot. Preparation and commit both check the full installation snapshot and private credential revision. HTTP refresh is a single bounded grant with no retry after uncertain outcomes; returned account, application and required scopes must match. The rotating pair and new metadata are committed together. Cancelled workers retain admission until actual completion, and disable/removal/configuration/credential changes invalidate work. Automatic attempts are limited to once per credential revision per host session and at least60 seconds apart globally. The child remains receive-only and never refreshes/writes credentials.

Successful saves restart the enabled receiver via ordinary credential reconciliation. This can create an event gap and resets the child’s in-memory duplicate cache; no uninterrupted/exactly-once cross-restart guarantee is made. Unknown grant or save outcomes can require fresh browser consent. Legacy/manual credentials without validated metadata are not automatically refreshed.

989 offline all-target tests passed. A subsequent preparation-status label refinement passed43 final auth-focused tests and9 refresh-focused tests (overlapping filters; not additive). Final strict all-target Clippy, scoped rustfmt/whitespace and normal debug build passed. Core tests use local fake HTTP for rotation, expected-account/client/scope rejection, conservative expiry, cancellation/late-token discard and uncertain response/no-retry cases. Credential tests cover legacy compatibility, private metadata validation, atomic revision changes, no-op retention and manual invalidation. Controller tests use a bounded fake command channel to check enabled/due/revision/cooldown admission and stale snapshot cancellation; store persistence uses the actual worker separately. These are compositional offline receipts, not a full live refresh/reconnect recording.

Read-only review found no blocking defect; its minor preparation-versus-saving label issue was corrected with a regression assertion. The suggested cancel-after-job-creation/before-prepare-receipt race deserves a dedicated controller harness follow-up. Existing normal/narrow sign-in captures cover unchanged idle UI markup; no new active refresh visual acceptance is claimed. User explicitly requested offline work. No live Twitch/account calls, commits or pushes. No build/capture remains running. Overall goal remains active.

Evidence: stage4-evidence/twitch-refresh-*.log plus source hashes. Remaining service acceptance: live authorization/refresh/events, recorded visible integration reaction and native capture; broader motion/platform/streaming roadmap gates remain separate.


### Refresh cancellation receipt race — 2026-10-20

Added a controller regression with an actually live held auth worker: cancel before the prepared-job receipt arrives, retain global admission until worker completion, reject a second attempt while draining, and emit no credential-save command. The focused test and all807 library tests passed offline. Test helper has a hard two-second safety deadline and no network or credentials. No live authorization, commit or push.


### Recorded offline extension reaction — 2026-10-20

Added optional showcase extension_test plan entries that invoke the real installed-test runtime through saved grants/settings, with no direct motion/expression bypass. Plan validation rejects invalid identity/timeline and mixed manual/probe controls. Traces retain runtime status and motion/speech/camera observations; failures retain explicitly incomplete diagnostic traces. Seven showcase tests passed. Diagnostic capture supports5–30 FPS; production process deadlines are unchanged.

Recorded85 frames at5 FPS with Hikari and a credential-free isolated profile: talking motion -> real child-triggered one-shot nod -> talking motion -> idle after reused narration ends. The child receipt is completed, the trace is successful, and the camera transform is constant across all85 observations. This is offscreen recording, not live Twitch, native input, speakers or streaming acceptance. Initial15 FPS captures at1280x900 and800x600 exceeded the wall-clock test deadline because screenshot throughput was slower than simulated animation time; logs retained. Representative frames were inspected; the final idle moves right toward the crop boundary, so framing and broad collision/animation acceptance remain open. Video: /run/media/subsect/toshiba/kimodo/waifu-motion-library/validation/extension-reaction-20261020/offline-reaction-5fps.mp4 (17s, reused narration).

Strict all-target Clippy, scoped rustfmt and whitespace checks passed. Debug build passed. Read-only review found no actionable issue in runtime admission or cancellation regression. Final all-target tests are recorded separately below. No commit/push; no live authorization/account/provider calls. Overall goal remains active. Evidence: reports/stage4-evidence/extension-reaction-* and waifu-extension-recording-* logs.

Final checkpoint: 994 offline all-target tests passed; strict all-target Clippy, scoped rustfmt, whitespace and debug build passed. No build or capture remains running. No commit/push.


### Configurable lifecycle reaction hook — 2026-10-20

Added the owned Configurable reaction hook example to Install example. One card now selects a lifecycle event (completed reply, speech started/stopped, emotion changed), character, registered gesture and optional temporary expression. The Character setting is a bounded plain string: empty follows the captured loaded target, otherwise it requires a canonical lowercase hyphenated nonnil UUID. Search lists library names/UUIDs; temporarily missing identities remain saved. This is a handler filter, not an authorization grant, avatar switch or alternate action target. Settings preserve draft/save semantics and existing permissions.

The Python handler uses the normal provisional API1, bounded local JSONL, host-issued session/incarnation/target identities and exact action correlation. Unselected events, nonmatching source characters, failed/cancelled replies and disabled mappings request no action. Expressions overlap one-shot gestures after expression acceptance; motion acceptance alone does not complete the invocation. A saved-reaction test uses the same handler with --test, saved settings and current grants while retaining the character filter. No private reply access, provider calls, credentials or network are used. All four declared triggers launch bounded children, including unselected triggers that exit without actions; this is not dynamic host subscription filtering.

1009 offline all-target tests passed. New unit/store tests cover UUID schema/type bounds, compatibility migration, invalid-save atomicity and restart retention. Subprocess tests cover filters, malformed settings, None choices, host identities and correlated motion completion. Actual installed-runtime tests demonstrate an unselected reply event producing no expression, a selected speech-start event applying and releasing the saved expression, and manual preview honoring a changed character filter after reconciliation. Tests do not establish live speech/provider delivery.

Offscreen1280x900 and800x900 screenshots loaded Hikari and were visually inspected. The first render exposed a redundant control-side note squeezing Character into a one-letter column; it was removed, and both repeated captures show readable compact rows without horizontal clipping. Before-fix artifacts are retained. Fixtures explicitly seed a credential-free profile; actual package installation is verified separately by Rust tests. No native pointer interaction or live account acceptance is claimed.

Normal debug build, scoped rustfmt and whitespace checks passed. Read-only review found no actionable protocol/selector issue. Clippy receipt follows separately. Evidence: reports/stage4-evidence/configurable-hook-* and waifu-configurable-hook-* / waifu-character-setting-*; captures: /run/media/subsect/toshiba/kimodo/waifu-motion-library/validation/configurable-hook-20261020/. No commits/pushes; overall goal remains active.

Final configurable-hook checkpoint: strict offline all-target Clippy passed with warnings denied; scoped rustfmt and whitespace checks passed.1009 all-target tests and debug build passed. No build or GPU capture remains running. No commit/push. Native interaction, real service acceptance, capture/streaming, motion approval, pacing, Discord and platform verification remain separate open roadmap gates.


### Opt-in replies to attributed incoming messages — 2026-10-21

The earlier delivery path only saved attributed context. Added a host-owned per-character Reply to incoming messages switch on the Extensions page. It defaults off in legacy/default scenes, does not inherit to a legacy character without saved settings, and resets off for newly imported characters. Other inherited scene preferences are preserved; stored opt-in restores only with its character. Packs/presets cannot supply this host policy. The control is disabled without a selected character.

After successful delivery and exact target/scene-writer checks, opted-in host delivery can start an ordinary reply from the latest attributed external turn. It appends no authored-user turn, does not consume composer/attachments or semantic retrieval, skips user evidence intake/triage and raw-message appraisal, and uses existing read-only lexical recall plus configured provider/speech/affect/completion paths. Provider projection still quotes external metadata/text as third-party data. A global ten-second running-app cooldown prevents character switching from bypassing the incoming-reply rate boundary. One focused generation remains active; no new backlog or automatic retry is introduced. Twitch's existing bounded intake/backpressure still applies, and all delivered external context (including alerts) follows the same opt-in.

Delivery and generation have separate outcomes: after context persistence succeeds, a busy/loading/provider/cooldown reply failure leaves the delivery accepted and saved, with a fixed notice. Disabled/grantless integrations still cannot deliver. No new provider credentials, direct-OpenAI path or integration-specific generation shortcut was added. Permissions help now correctly describes available attributed delivery.

1017 all-target tests passed with Cargo offline and test threads serialized. Eight focused external-boundary tests passed. New loopback production-provider tests retain external ID/origin/text, omit drafts and attachments, preserve facts/triage, exercise ordinary reply completion/lifecycle, reject stale/nonlatest/quarantined/invalid-provider starts, and cover cooldown after cancel and cross-character attempts. Data tests cover default/migration, character switching, persistence and fresh imports. An initial glTF fixture omitted required scene nodes; it was corrected before the passing full run. Strict all-target Clippy, scoped rustfmt/whitespace and debug build passed. Native live-model/voice/Twitch acceptance remains untested.

Evidence correction: the first failed negative provider fixture inherited a pinned gateway route, so changing base_url did not force synchronous invalid-URL rejection; a gateway request using synthetic fixture text may have been attempted. That run is not offline transport evidence. The corrected fixture replaces the complete route/configuration; passing focused tests use explicit loopback or invalid local/custom routes. Earlier offscreen captures also did not exclude incidental chat-model catalog discovery, so their blanket no-provider-call statements were too broad. Main now disables chat-model discovery before offscreen UI execution; both lookup entry points and a regression enforce this. Independent lookups on other screens and arbitrary extension networking remain separate. No credentials were displayed or modified. See stage4-evidence/external-reply-evidence-caveat.md.

Current final UI captures use a credential-free isolated profile with explicit fixture opt-in, local-only provider configurations, owned nonnetworked hooks and chat-model discovery disabled. Screenshots prove layout only, not native pointer or actual delivery acceptance. Final visual receipt follows. No commit/push; overall goal remains active. Remaining native capture, real service/account, motion quality/pacing, Discord and platform gates remain open.

Final incoming-reply receipt:1280x900 and800x900 captures were visually inspected; the incoming-message row is readable with no horizontal clipping.1017 tests, strict all-target Clippy, scoped rustfmt/whitespace and debug build passed. No build/capture remains running. No commits/pushes. Evidence corrections above remain applicable; live service/model/voice and native interaction are not established.


### Bounded Twitch notification backlog — 2026-10-22

Source inspection distinguished the existing32-record host-input transport queue from the missing pending-notification backlog: the native receiver previously discarded notifications whenever an action was pending. The receiver now retains FIFO notifications pinned to the captured loaded target, up to16 waiting/in-flight texts of4096 bytes each. Original notification expiry is15 seconds, including in-flight delivery; queue overflow, stale targets and expiry are reported without logging message contents. Target changes discard the queue and in-flight delivery instead of retargeting.

Only a correlated explicit Busy refusal of message.deliver can retry, with at least1 second backoff, at most3 total attempts and the original expiry retained. Busy is a pre-commit refusal in the host delivery path. Acceptance, uncertain/missing results, other errors, timeouts, EOF/disconnect and expression/motion refusals are never replayed. Earlier/duplicate ACKs cannot advance a retry incarnation. Host-result handling checks expiration before consuming an ACK, so late results cannot revive expired work or start a gesture. The real worker loop dispatches waiting work after polling host input, independently of the UI.

This is a short receiver backlog, not an AI-reply queue or a delivery guarantee. Accepted context can still produce no reply because of host opt-in, busy/loading/configuration/cooldown admission. Existing process restart event gaps and in-memory dedup limits remain. Earlier references to bounded intake did not prove pending notification retention; this change supplies that specific boundary.

1021 all-target tests passed with Cargo offline and one test thread, including21 Twitch-worker cases. Four new regressions cover FIFO/capacity/text bounds/expiry, Busy backoff and retry exhaustion, late ACK/no uncertain replay, target-generation discard, expression Busy exclusion and result-consumption deadline ordering. Existing fake HTTP and WebSocket fixtures remain local; no live Twitch/account acceptance is established. Initial read-only queue review found no actionable issue; parent follow-up added the late-result deadline guard with its own regression. Strict all-target Clippy with warnings denied, scoped rustfmt/whitespace and debug build passed. UI/rendering code did not change in this checkpoint; no new UI/native acceptance is claimed. No build/capture remains running. No commit/push. Overall goal remains active; broader service, motion, pacing, capture, Discord and platform gates remain open.

Evidence: stage4-evidence/twitch-queue-source-sha256.json and waifu-twitch-queue-*.log.


### Read-only extension author check — 2026-10-22

Added waifu-studio --check-extension FOLDER, used alone as the first option. It runs before ProjectDirs/app-data discovery, startup credentials, provider discovery, GPU/window creation and child execution. It selects the existing strict content/integration manifest validators through a shared read-only check_pack entry point, preserving bounded JSON reads, supported versions/capabilities, settings and declared content/process/test/hook path checks. Output uses fixed labels and counts, not credential values or arbitrary manifest names/descriptions. No pack is installed, enabled or granted permissions. Mixed modes (including the internal Twitch worker mode) are rejected before startup.

This is deliberately a manifest/declared-path check, not full installation or asset/runtime approval. The installer still checks the whole copied folder and its own symlink/size policy and revalidates the destination. The check does not decode/render every asset or execute/test a handler. Files can change afterward. Author and content-pack guides describe these boundaries.

Three real-binary regressions passed: owned content/persistent/hook examples validate without app-data creation; a declared executable sentinel is never run and the manifest remains unchanged; malformed capabilities, missing folders/assets and combined options fail without entering app or worker startup. A standalone command receipt validates the actual Configurable reaction hook manifest (API1, one content entry, four hooks, five settings, zero credential fields).

1024 all-target tests passed, plus strict all-target Clippy with warnings denied, scoped rustfmt/whitespace and debug build. The first full-suite attempt under the newly restricted sandbox had74 failures including denied loopback/Unix sockets and FIFO creation. No application fix was inferred from those denials; scoped execution access was granted for existing local fixtures, and the full serialized rerun passed. The new CLI tests passed within the restricted sandbox as well. No live service/account or new native visual acceptance was performed. No commits/pushes; no build/capture remains running. Overall goal remains active; broader motion/pacing/capture, live Twitch, Discord and platform checkpoints remain open.

Evidence: stage4-evidence/extension-check-source-sha256.json and waifu-extension-check-*.log.


### Structured incoming chat chunks — 2026-10-22

Twitch Chat intake now offers Chunks (default) or Direct. Chunks uses structured chat.offer admission without a delivered event; alerts retain immediate delivery/reaction. Exact source message IDs, author IDs/names and raw text remain attributed external data. The bounded buffer groups whole same-source messages after four seconds or at capacity, up to16 pending and4096 encoded JSON bytes per chunk. Original15-second expiry and256-ID/60-second dedup remain bounded. Only explicit pre-commit Busy can restore originals; uncertain saves never replay. Target, source generation, settings/grants and credential revisions invalidate outstanding chunks, including transient grant revocation. Invalid chat is isolated without terminating the Twitch worker.

This checkpoint implements FIFO grouping only. The configured decision-model picker (zero to two offered IDs, no generated quotes) remains pending; no Jev ranking or selection-quality claim is made.

1040 all-target tests passed in the preceding serialized offline run. A subsequent derived-Default cleanup passed all nine focused incoming-chat tests, strict all-target Clippy and debug build. Scoped formatting and whitespace passed. Normal and800x900 captures were inspected and show the Chunks dropdown without horizontal clipping. These are layout-only captures from a disabled, uncredentialed local fixture whose copied manifest omits credential controls to expose the settings section; they do not prove full Twitch UI interaction or live service behavior. No live model/Twitch calls were used for this checkpoint. No commit/push; capture and build processes have finished.

Evidence: stage4-evidence/chat-chunks-source-sha256.json, waifu-chat-chunks-*.log, waifu-chat-chunk-final-focused-tests.log and chat-chunks-{normal,narrow}.png. Earlier historical network caveats remain recorded separately.


### Installed extension card simplification — 2026-10-22

Unified content/integration enablement onto the shared switch, preserving busy/error/runtime admission and stop/enable behavior. Main view retains status, errors, descriptions, content and everyday settings; settings open by default. Process restart/hook reset, saved-reaction tests, permission grants, provenance, package/version, diagnostics and removal remain available under Advanced. No permissions or credentials are automatically granted. All14 existing extension-page tests passed, debug build and whitespace checks passed.1100x900 and800x900 captures were inspected: right-aligned controls remain inside the content column and long content scrolls vertically. Layout-only isolated fixture, with disabled/uncredentialed Twitch and credentials omitted from its copied manifest; no live/native service acceptance. No commits/pushes. Full extension roadmap and decision picker remain open. Evidence: stage4-evidence/waifu-compact-extensions-v2* and waifu-compact-extension-controls-tests.log.


### Optional decision-model chat highlights — 2026-10-22

The host now connects chunked incoming chat to an optional decision picker using library.decision and the existing credential-loading path. Extensions → Incoming messages exposes Pick chat highlights (default off), conditional on incoming-reply opt-in, plus the existing searchable decision-model dropdown. Direct delivery remains immediate and bypasses this picker. No provider keys are sent to child integrations.

Each request contains bounded exact attributed originals and two choice questions. Choices are mapped from offered opaque slots to original source IDs; unknown, duplicate, malformed and inconsistent selections fail closed. Zero choices produces no turn; one/two choices retain exact text, author/source metadata and original arrival order. No generated summaries or quotes are persisted. Selection errors discard the chunk with a generic notice, never silently reverting to answer-all. Explicit pre-commit Busy retains a ready selection without re-querying; uncertain saves never replay.

Selection uses a bounded cancellable worker (at most two including closing requests), ten-second transport/job deadlines and the existing fifteen-second original TTL. Before dispatch the host rechecks target/source authority, permissions/incarnation and credential revision through cancellation tickets, reply/picker opt-in, decision configuration, and original expiry. Disabled sources and cancelled replies invalidate outstanding work before the empty-runtime early return. Shared bounded evaluation transport was extracted from recipient selection; existing recipient wire tests still pass.

1046 all-target tests passed offline, including four new decision tests, one chunk selection/expiry test, one host invalidation test and expanded exact-selected-delivery/no-replay coverage. After a final cancellation guard/error-label cleanup, six focused shared/chat decision tests passed; the local-wire test was then strengthened to exercise the actual worker and all four chat-decision tests passed again. These focused counts overlap the full suite. Strict all-target Clippy, debug build, scoped formatting and whitespace passed. All model-wire checks use local fake HTTP and synthetic messages. No live provider/Twitch requests or account authentication were used for this checkpoint; real selection quality is not measured. No commits/pushes. The larger extension/motion/streaming/platform goal remains open.

Evidence: stage4-evidence/chat-picker-source-sha256.json and waifu-chat-picker-*.log. Layout capture receipts follow; fixture warnings are expected because credentials/catalog access are deliberately unavailable.


### Reviewed chat-selection expiry and extension readiness — 2026-10-22

Read-only reviewer Rawls identified partial-expiry loss: pruning the offered set before validating a worker result could reject an expired selected ID and discard another still-live selected original. Reconciliation now observes liveness without mutating the offered set. Selection validation precedes expiry filtering, and ready selections are re-filtered before saving after Busy waits. A staggered-arrival regression and expanded exact-save/no-replay assertions cover both boundaries. Reviewer follow-up confirms the finding is addressed, with no concrete regression in this specific change; source-only review is not live/runtime acceptance.

1047 all-target offline tests passed, followed by strict all-target Clippy, scoped formatting/whitespace and debug build. No new visual capture was needed for this host-only fix; earlier captures remain layout evidence only. No live provider/Twitch calls, commits or pushes. No build remains running.

Updated current content/Twitch author guides to the Add menu, enable switches and Advanced permissions/restart/tests. Saved extension-readiness.md with a source/acceptance matrix: three architectural layers implemented, Linux-only execution, correlated motion receipts rather than general motion hooks, and explicit outstanding live/native/platform/streaming/Discord and quality gates. The full goal remains active.

Evidence: stage4-evidence/chat-picker-reviewed-source-sha256.json, chat-picker-source-review.md and waifu-chat-picker-reviewed-*.log (plus focused expiry log).


### Native Vulkan/X11 startup guard and recording boundary — 2026-10-22

A local window-only recording attempt exposed a real startup crash: NVIDIA Vulkan/X11 reported supported alpha modes [Opaque], while primary_window requested PreMultiplied unconditionally. The app panicked in surface configuration before model load. The final fix uses actual RawHandleWrapper Xlib/Xcb identity plus actual RenderAdapterInfo Vulkan identity on Linux, in main-world Last before extraction/configuration. It selects Auto there, returns Desktop to Studio and displays the build limitation. Native Wayland and other backends remain unchanged. This explicitly treats Vulkan/X11 transparent Desktop as unsupported in this build; it does not infer transparency support on untested hardware. The direct wgpu29 dependency reuses the already locked renderer version; no vendored engine code changed.

An initial generic temporary-surface probe avoided the X11 panic but a Wayland capture stalled after saving. That implementation was removed entirely. It is not the final fix or clean-exit evidence. A read-only reviewer confirmed ordering/specificity in the replacement, with no concrete finding. No unsafe handle access, extra GPU surfaces, RenderApp probe systems, caches or atomics remain.

Final isolated native tests: Wayland saved Hikari's transparent735x1120 frame and exited0 in7.22 seconds;823200 alpha samples span0–255 with90.1444% fully transparent. X11 saved an inspected Studio fallback frame with the explanation visible and exited0 in7.83 seconds. Both used a temporary local-only profile, disabled Twitch and no credentials/replies. Ordinary system/user settings were not changed. All owned fixture processes are stopped.

The FFmpeg X11 window-only attempt produced zero video frames with XGetImage BadMatch/permission errors, despite exit0. Its receipt is explicitly corrected to capture_success=false; no recording acceptance is claimed. No root/monitor capture or other app recording was attempted. Practical native Wayland recording still needs user-approved window selection in the screen-sharing portal, followed by actual audio/lip-sync and recovery review. docs/streaming-capture.md documents the current support and checklist; README links the author/user guides. Remaining current menu references now match Add/Advanced and enable switches.

1048 all-target offline tests passed, plus strict all-target Clippy, debug build, scoped formatting and whitespace. No live provider/Twitch calls, commits or pushes. No build/capture remains running. The broader goal stays active, including compositor/OBS video/audio, live Twitch/model quality, motion, pacing, Discord and platform acceptance.

Evidence: stage4-evidence/native-alpha-backends/ contains original crash logs, failed recording receipt, removed-probe limitation, source review, final native frames/clean-exit receipts/alpha histogram, source hashes and test/lint/build logs.


### Connections and Voice clarity; local presets/session isolation — 2026-10-22

Connections now has one ordinary provider/model/creativity selector location. Advanced retains exact IDs, refresh, endpoints and routing rather than repeating ordinary selectors/sliders. Voice service/read-aloud/model/speed controls are not duplicated in speech tuning; the everyday voice/reference-ID field is on the ordinary Voice card. Keys reports each feature independently with saved/session/OIDC sources, using Loaded/Not loaded rather than claiming a verified connection. Local services can work without keys. Manage requests opening/scrolling to the existing Advanced section without exposing any key value.

Two functional issues were found during the cleanup. Simple microphone model discovery passed an empty session key; it now passes ListeningState.session_key. New Custom/local selections previously installed direct hosted OpenAI speech/transcription URLs despite their labels. Those compatible-wire presets now use localhost8000 endpoints with no hosted key environment variable; Vercel/OpenRouter choices retain their routes and explicit saved endpoint/key fields remain unchanged. Both transcription service selectors now use change_service: cancel/discard the previous operation, clear its provider-scoped session key, then configure the new route while retaining microphone/VAD preferences. This prevents forwarding an old service's session credential to the newly selected service.

1050 all-target offline tests passed, including two new preset/credential-isolation/explicit-profile preservation regressions. A subsequent unused-import/documentation cleanup passed strict all-target Clippy and debug build; scoped formatting and whitespace passed. Read-only reviewer found no concrete regression in the scoped change. Normal1100x900 and narrow800x900 Connections/Voice captures completed and were inspected without horizontal overflow. These are layout-only, uncredentialed local-only fixtures; catalog warnings and Not loaded statuses are expected. No model/voice/microphone requests, live provider/account acceptance or native Manage-click acceptance is claimed. No commits/pushes; no build/capture remains running.

Evidence: stage4-evidence/simple-connections/ contains source hashes, review disposition, full-suite/lint/build logs and four layout captures. docs/connection-settings.md explains everyday/Advanced controls, saved/session status and local routing. Broader whole-app UI, service, motion, capture, Discord and platform acceptance remain open.


### Everyday voice browser and focused microphone tuning — 2026-10-22

Moved Browse Fish voices from the long Advanced section to a folded Voice library card immediately after the ordinary Voice card and before Microphone. Searches now happen only on explicit Search/Enter/pagination actions; opening the browser no longer triggers the prior automatic fetch. Public/My voices and title remain primary; tag/language/sort/page-size controls are under More filters. Selected labels are truncated within the existing control width. Removed duplicate microphone enable/autosend/service/model dropdowns from Advanced; endpoint/exact-ID, key, language, VAD/silence and manual diagnostics remain there. Ordinary controls keep their existing behavior.

Source review also exposed retained voice results across credential changes. VoiceLibraryState now records an in-memory connection fingerprint (library/speech endpoint, key source, adapter and effective credential), clears cached/unapplied choices and cancels pending work on change. UI reconciles before poll/selection and fetch reconciles before admission. Old workers retain their slot until they close; queued stale responses cannot repopulate the chooser. Only the fingerprint is retained in state; no new credential persistence or environment mutation.

1051 all-target offline tests passed, including a new channel-based account/endpoint stale-result regression. After the final layout-only move beside Voice, all173 binary/UI tests passed again (overlapping, not an additional full-suite count). Strict all-target Clippy, debug build, scoped formatting and whitespace passed. Read-only reviewer found no concrete regression. Normal1100x900 and narrow800x900 captures of the browser entry were inspected without horizontal overflow. Open/populated browser and native pointer/search acceptance remain unverified; fixtures are uncredentialed and local-only, and no library search/model/voice/microphone action was triggered during rendering. No live account/provider calls, commits or pushes. No build/capture remains running.

README/connection guide now point to the normal browser location and reflect the pre-existing new-profile auto-send default accurately; this checkpoint did not change that default. The broader extension/service/motion/capture/platform goal remains active.

Evidence: stage4-evidence/voice-browser/ contains full/final-focused tests, lint/build receipts, source review/hashes and two normal-window layout captures.


### One session-key manager — 2026-10-22

Connections → Keys → Manage now reaches four masked, service-labelled memory-only overrides together at the beginning of Advanced: selected chat connection, decision model, voice and microphone. Removed their duplicate editors from deeper connection/speech/microphone controls; expert endpoint/key-environment settings remain. Voice and transcription service rows point users to Connections for keys. Existing credential fields, saved/environment fallback and serialization exclusions remain unchanged; no runtime environment mutation, secret copying between features or new persistence path was introduced.

A new headless Egui user-input regression clicks and types distinct synthetic keys into all four visible fields, verifies correct feature routing, checks that no raw key is painted and saves the library to verify no session value appears on disk or in microphone configuration JSON.1052 all-target offline tests passed. After a semantic-neutral Clippy control-flow cleanup the input regression passed again; strict all-target Clippy, debug build, scoped formatting and whitespace passed. Read-only reviewer found no concrete regression.1100x900 and800x900 expanded-panel captures were inspected; all four fields fit without horizontal overflow. Empty-field, uncredentialed local-only fixture; no native Manage-click or live-provider acceptance claimed. No live calls, commits or pushes; no build/capture remains running.

Evidence: stage4-evidence/session-manager/ contains source hashes/review, full/final-focused tests, lint/build and two expanded-panel captures. Updated docs/connection-settings.md describes the single session override location. Broader UI/extension/native/live/platform gates remain open; the goal remains active.


### Isolated extension restart pressure — 2026-10-22

Added a test-only lifecycle regression covering 20 restart/disable/remove cycles and 40 real local Python process incarnations. It exercises host expression requests, avatar-generation changes, temporary grant revocation, private queued reply disposal and fresh-session isolation. PID plus process-start identity confirms old incarnations terminate; controller contributions and queues are reclaimed. The focused isolated run returned file descriptors 4 → 7 → 4 and threads 2 → 3 → 2. These are bounded test observations, not native speech/GPU long-session or performance acceptance.

A source reviewer identified parallel-test interference in process-wide resource counts. The final test re-executes its exact test path in an isolated single-threaded harness, requires a scenario metric marker and successful exit, and enforces a 75-second parent timeout. Re-review found the issue addressed with no further concrete finding. The isolated scenario also passed alongside ten other concurrent extension-runtime tests. Final all-target offline suite: 1,053 tests passed; strict all-target Clippy passed. Production runtime code was unchanged in this checkpoint; no additional native capture or production build was needed.

Evidence: reports/stage4-evidence/lifecycle-stress/ contains final full-suite, lint, isolated and parallel logs and source hashes. No live calls, commits or pushes. Native recording/audio, live Twitch/model quality, whole-app UI, motion, pacing and platform gates remain open. The goal remains active.


### Independent character-selection lifecycle — 2026-10-22

Implemented character.selected source metadata with previous/new nullable UUIDs and a positive monotonic application-run selection revision. Actual host selection, removal and final import paths record changes after old-source cancellation; no-op selection emits no event while existing same-selection reload/cancellation behavior remains. Selection is independent of avatar loading, and target.changed retains loaded-avatar action authority. Speech decoder termination may follow selection; no synthetic stopped receipt is invented.

Selection hooks retain only lifecycle.observe and a permanently null action target even when installations have broader grants. New revisions supersede queued/running older selection handlers; ordinary avatar availability changes do not retarget the metadata handler. Persistent observation retains ordered source events. Added the metadata-only Python selection-observer example, installable from Add → Examples, plus readable trigger labels and updated author documentation. Its latest-event receipt is not a durable complete event log.

Review found and fixed first content-pack character import suppressing None→id notification. New regressions cover source/cancellation order, rapid switches/no-op/import/removal, positive revision/wire validation, queue expiry/overflow/revocation, real-child authority restrictions and cancellation, and installed observer latest-only/null-target/restart/reenable behavior. Final all-target offline suite passed 1,063 tests, including the real installed observer and twelve subprocess cases in one script regression. Strict all-target Clippy, debug build, scoped rustfmt, whitespace and final --check-extension for the bundled package passed. Two semantic-neutral test fixture lint cleanups followed the full suite. Initial sandbox loopback denials and malformed glTF fixtures were corrected before the final suite; they are not accepted failures.

Evidence: stage4-evidence/character-selection/ contains final test/lint/build/package receipts, source hashes and review disposition. No live provider/account calls, commits or pushes. No build/capture is running. Native menu/selection interaction, app/extension lifecycle events, general motion hooks and the broader motion/pacing/capture/service/platform gates remain open; goal stays active.


### Local app-ready lifecycle — 2026-10-22

Added app.ready with a nonnil app_run_id, emitted once after initial local extension loading has settled and installed bindings are reconciled, before source-journal drain. It represents local host initialization, not avatar, provider, account or every-extension success. A sticky initial-load completion signal prevents later OAuth/install busy work from delaying readiness. Initial error, worker-start failure and receiver failure settle the same boundary.

Ready hooks are observation-only, permanently targetless and independent of selection/avatar availability. Existing queue/expiry/process bounds remain; no historical ready event is replayed after late installation, enablement or restart. Added Startup observer under Add → Examples with a metadata-only Python handler and author documentation. It records only app_run_id metadata; it is not a durable startup task. The four-hook-per-package bound is unchanged.

Final offline all-target suite passed 1,074 tests, including source/initial-load-vs-OAuth-busy failures, wire/relay/permission/queue checks, installed real-child startup delivery, late-install non-replay and fifteen subprocess cases in one script regression. Strict all-target Clippy, debug build, scoped rustfmt, whitespace and final bundled-package --check-extension passed. Formatting and semantic-neutral cloned-test-slice lint cleanups followed the full suite; production behavior was unchanged by those cleanups. Read-only review found the busy-state issue, confirmed its fix and identified no further concrete defect.

Shutdown remains a separate implementation gap. The current immediate-kill supervisor cannot prove delivery merely by queuing a stopping event. Saved and source-reviewed extension-shutdown-contract.md: quiesce authority, preserve partial JSONL framing without flushing revoked private bytes, enforce one shared grace/cleanup deadline and signal every survivor before receipt waits. No graceful stopping or extension-started/stopping implementation is claimed.

Evidence: stage4-evidence/app-ready/ contains final test/lint/build/package logs, source hashes, source review and shutdown-design disposition. No live provider/account calls, commits or pushes. No build/capture is running. Native menu/startup acceptance, app stopping, extension lifecycle, general motion hooks and broader motion/pacing/capture/service/platform gates remain open. Goal remains active.


### Bounded graceful app shutdown and JavaScript plugin example — 2026-10-22

Implemented a distinct sticky quiescence transition: begin_shutdown cancels extension-owned controller contributions while the world is live, rejects already admitted actions and cannot be reversed by runtime clones, restart, grants, hooks or later frames. Main wires a CPU Last exit system and keeps transport waiting in its existing post-run retained-runtime path. Calling shutdown alone is transport teardown, not proof of cancellation of controllers held by a live embedding caller.

app.stopping carries the retained source app_run_id only to eligible existing persistent observers with lifecycle observation and a fully written handshake. It is not a manifest hook and starts no replacement handler. One grace interval of at most100ms stays inside the supplied total deadline; main retains its two-second total. Untouched staged/queued input is discarded, while partial prior JSONL forces termination rather than completing revoked private data or splicing records. At cutoff all survivors are signalled before any receipt wait. Protocol/overflow/explicit-stop immediate termination remains. No synthetic app-ready identity is invented. Notification admission is not delivery proof; terminal supervisor receipts remain authoritative for process exit.

Added a dependency-free Node.js lifecycle observer under Add → Examples. The installed real runtime test explicitly executed Node v26.8.1, observed startup, received the stopping receipt and exited with no avatar actions or provider requests. Eleven additional raw Node smoke cases covered malformed/session/authority/private-text handling, fragmented records, oversized input and invalid UTF-8. Node runtime checks are optional on development machines without the interpreter; this host executed the check and did not skip it.

1,085 all-target offline tests passed, including real owned-expression cancellation, stale ticket rejection, shared four-child/zero-deadline cleanup, queued private input purge, partial framing, blocked stdin, floods/held descendant pipes and CPU exit-system scheduling. Strict all-target Clippy, debug build, scoped rustfmt, whitespace and final JavaScript package --check-extension passed. After the full suite, the non-short-circuit reap fold was rewritten as an explicit all-child loop for Clippy; all four shutdown regressions passed again on that final source. Read-only transport/runtime review found no concrete defect; review also established the required live-controller cancellation boundary.

Evidence: stage4-evidence/graceful-shutdown/ contains full/focused/transport/actual-JavaScript logs, lint/build/package receipts, source hashes and review dispositions. No live provider/account calls, commits or pushes; no build/capture remains running. Native exit/menu acceptance, extension-started/stopping and disable/remove/restart notifications, general motion hooks and the broader service/motion/pacing/capture/platform gates remain open. Goal remains active.


### Actual integration-process start lifecycle — 2026-10-22

Implemented extension.started with source installation_id, session_id and process_generation, backed by an OS-spawn receipt retained independently of bounded stdout/event delivery. Failed OS spawn produces no event; a genuinely launched child that exits quickly or floods output retains its evidence. Collection occurs before incarnation replacement/removal as well as ordinary polling. A fixed32-record observation buffer transfers original identities into the host-owned LifecycleJournal, preserving its sequence ownership and reporting overflow explicitly as source loss.

Only persistent integration incarnations produce this event; one-shot handlers and manual test children are excluded at source to prevent recursive hook spawning. Startup hooks retain observation-only grants and a null target. Subscription timestamps/floors reject starts predating registration, reset or observation regrant, including delayed receipt delivery. Restarting a persistent integration creates a new session/generation; no host action target is inferred from either source UUID. Added Integration startup observer under Add → Examples and documented the copyable wire event. Its latest-event file is not a durable audit log or a service readiness claim.

Read-only review found two issues and confirmed their fixes: stale subscription boundaries after restart/regrant, and constructor handshake failure discarding a real fast spawn/exit receipt. A stopped coordinator now retains evidence without falsely reporting Running. Source, wire, metadata grants, retained fast/flood/failure receipts, bounded gap accounting, two drains before delivery, persistent regrant, failed spawn, hook/test exclusions, quiescence and real installed observer restart/fast-exit behavior are covered.

1,095 all-target offline tests passed. Strict all-target Clippy, scoped rustfmt, whitespace, debug build and final example --check-extension passed. After a semantic-neutral test-only conditional cleanup, the installed observer regression passed again. Seventeen real Python subprocess cases also cover the standalone example's input/authority/identity validation. No native app or live account/provider acceptance is implied.

Evidence: stage4-evidence/extension-started/ contains final full/focused/lint/build/package logs, example subprocess log, source hashes and review disposition. No commits/pushes, live calls, or running builds/captures. Extension-stopping/terminal and disable/remove/restart reason notifications, general motion hooks and the broader native/service/motion/pacing/capture/platform gates remain open; goal remains active.


### Stop-request versus confirmed integration exit — 2026-10-22

Added extension.stopping for the first captured host request and extension.stopped for retained completed supervisor termination after an actual persistent-process spawn. Both preserve source installation_id/session_id/process_generation. Stopping carries a typed reason; stopped carries nullable exit_code and requested_reason. Reasons distinguish disabled, removed, restarted, reconfigured and host_shutdown. Null code does not identify a signal/failure, and requested intent is not proof of exit causation or external cleanup. Failed OS spawn produces no invented incarnation exit. A missing retained receipt is reported once as source loss rather than fabricated completion.

Request capture and terminal timestamp publication share the terminal mutex. The ordering boundary is completed supervision/reaping, not earlier OS exit while pipes/descendants may still be cleaning up. First reason wins; late host requests do not rewrite completed evidence. Installed reconciliation now preserves distinct disable/uninstall/reconfiguration reasons instead of labelling all removals alike. Old-incarnation observations are collected before replacement/removal and forwarded through the existing bounded journal/gap/subscription boundaries. Hook/manual-test sources are excluded, callbacks remain observation-only/targetless, and app quiescence launches no handlers. Disable/remove/restart remain nonblocking immediate-stop operations; no new per-child grace wait was introduced.

Added Integration exit observer under Add → Examples with separate latest-stop-request.json and latest-exit.json receipts, plus documented schemas/examples. 1,104 all-target offline tests passed, including actual normal/nonzero/fast exits, first request correlation, already-completed/late request behavior, lost-receipt gap, reason routing, bounds, exclusions, subscription reset and real installed observer delivery without recursion. The standalone observer passed28 real subprocess cases. Strict all-target Clippy, scoped formatting, whitespace, debug build and final example --check-extension passed. A test-only unused closure parameter was removed after the full suite; production behavior was unchanged. Final read-only source review found no concrete defect.

Evidence: stage4-evidence/extension-stopped/ contains final test/lint/build/package logs, subprocess log, source hashes and source review. No live provider/account calls, commits or pushes; no build/capture is running. General motion lifecycle hooks, native menu/service/exit acceptance and the broader motion/pacing/capture/Discord/platform gates remain open. Goal remains active.


### General retained motion playback outcomes — 2026-10-22

Added motion.playback.ended for admitted ordinary host and extension playback requests, separate from unchanged requester-only motion.finished. Each request has a fresh playback UUID, immutable source snapshot, nullable original target/registered motion ID, tagged host or extension source and completed/cancelled/failed outcome. Unknown host incarnation stays null; current selection, file paths and arbitrary error text are never substituted at drain time. Retained generic observations survive requester receipt cleanup/disconnect.

All three import activation paths transfer the admitted identity; rejected loads create no record and admitted failures can terminate before activation. Non-loop timeline completion is recorded before pause, loop wraps do not complete, and same-file selections remain distinct. Source cancellation describes logical request/ownership termination rather than disappearance of every outgoing pose. Bounded64-receipt storage lives independently of replaceable animation state and reports evidence loss explicitly. Desktop park/restore preserves legacy visible clip/time/manual flags while cancelling parked logical requests and minting restored identities. Pure scrub no longer calls synthetic play/pause admissions.

General motion hooks are observation-only and permanently targetless; controller/message/event grants cannot be restored into the callback. This prevents terminal-hook feedback chains, while full-capability persistent integrations may still intentionally chain their own correlated receipts. Motion and integration observations merge by original timestamps into the host journal; per-event subscription eligibility avoids mixed-source floor suppression and historical replay after reset/regrant. Added Motion end observer under Add → Examples.

1,119 all-target offline tests passed, including all three imports, rejection/failure/late completion, ordinary nonloop and one-shot completion, loop wrap/no-avatar behavior, same-file supersession, scrub, revocation preserving unrelated import, desktop parking/restoration, drop/replacement, immutable attribution, requester disconnect, bounded gaps, mixed-source subscriptions and real installed metadata-only observer delivery. The standalone Python observer passed31 real subprocess cases in one aggregate test. Strict all-target Clippy, scoped formatting, whitespace, debug build and final example --check-extension passed without subsequent production edits. Source reviews found no remaining concrete defect; original requester receipt compatibility remains covered.

Evidence: stage4-evidence/motion-ended/ contains full-suite/lint/build/package logs, subprocess log, source hashes and source review; motion-lifecycle-contract.md records scope and evidence boundaries. No native rendered-motion, collision, live voice/provider or service acceptance is implied. No commits/pushes or live calls; no build/capture is running. Broader native menu/workflow, motion-quality pack, pacing, streaming, Twitch/model quality, Discord and platform gates remain open; goal stays active.


### Labeled settings navigation at ordinary window sizes — 2026-10-22

Settings retain their 184px labeled navigation down to 800px window width, rather than hiding labels below1100px. Conversation and smaller windows retain the existing compact rail. The existing full-content/scrolling layout regression now exercises the expanded rail at800x600 and passed. Offline debug build, rustfmt check and git diff whitespace check passed. A fresh800x900 GPU render of Extensions with the disabled synthetic Twitch fixture shows readable navigation labels, wrapped hints, bounded controls and no horizontal overflow. This is one page/layout check, not acceptance of every settings page or native workflow. Evidence: stage4-evidence/navigation-labels/. No provider/account calls, commits or pushes; no build/capture remains running. Overall goal remains active.


### Simpler everyday Twitch account setup — 2026-10-22

The Twitch credential section is now named Twitch account. Normal setup shows browser sign-in and the application-ID replacement field; access and refresh-token fields remain available inside collapsed Manual tokens. Other integrations retain their manifest credential controls. Masking, undo clearing, saved-value reuse and explicit save/clear paths are preserved. The setup guide matches the new labels.

All16 extension-page tests passed offline, including saved-ID connect/invalid override and draft-save coverage. Strict binary Clippy, debug build, scoped rustfmt and diff whitespace checks passed. A fresh800x900 disabled-Twitch synthetic-ID GPU render confirms collapsed manual tokens, labeled navigation and no horizontal overflow. Manual expansion interaction and live sign-in remain unverified; no live account/provider calls occurred. Evidence: stage4-evidence/twitch-account-ui/. No commits/pushes or running build/capture. Goal remains active, including motion, pacing, capture, service, Discord and distribution acceptance gates.


### Discoverable app access and setup interaction coverage — 2026-10-22

Permissions now lives directly on executable extension cards instead of under Advanced, retaining the compact disclosure/menu and explicit grant/revoke behavior. This exposes a required setup step alongside account/settings without changing granted capabilities or enabling integrations. Twitch and content-pack setup guides use the new location. Advanced retains process controls and diagnostics.

Added an actual egui pointer-event regression using an installed, disabled Twitch package: open Twitch account, verify sign-in/application-ID controls, verify token fields absent, expand/collapse Manual tokens and verify both token fields appear/disappear. It asserts no credentials saved, no authorization started and no pending store command. No runtime reconciliation or service worker is invoked by the test. All17 extension-page tests passed offline; strict binary-and-tests Clippy, debug build, scoped formatting and diff whitespace checks passed. The new800x900 GPU capture shows Permissions outside Advanced without overflow.

Evidence: stage4-evidence/extension-setup-interactions/. No live calls, commits/pushes or running build/capture. Full native interaction, live integration, motion/physics, pacing, capture, Discord and distribution gates remain open; goal remains active.


### Discord message decoding boundary — 2026-10-22

Added a pure bounded Discord Gateway MESSAGE_CREATE decoder returning the existing ChatMessage type. Exact server/channel filtering excludes DMs and other channels/threads; bot, webhook, system and content-less messages are ignored. Ordinary messages/replies preserve original text and author identity. Source IDs qualify original message IDs with server/channel IDs. Nickname/global-name/username fallback is explicit. Invalid IDs/names, malformed payloads, oversized packets and host-invalid text fail with fixed diagnostics; no truncation or network fetching.

Seven targeted offline tests passed; strict library/tests Clippy, scoped rustfmt and diff whitespace checks passed. No full-suite claim is made for this checkpoint. This is a transport prerequisite, not a usable Discord integration: no Gateway connection/authentication, heartbeat/resume, installed-process delivery, outgoing messages or voice/media implementation exists yet. docs/discord-adapter.md records integration boundaries and next work. Evidence: stage6-evidence/discord-message-decoder/. No live calls, credentials, commits/pushes or running builds. Overall goal remains active.


### Discord Gateway connection policy — 2026-10-22

Added a deterministic token-free Gateway state machine for bounded Hello/auth waits, jittered heartbeat scheduling/ACK deadlines, server-requested heartbeats, disconnect/invalid-session recovery, fatal close-code stops and resumable sequence checkpoints. Bot READY data validates the bot identity and Discord WSS resume endpoint. Authenticated resume can decode replay before RESUMED; duplicate/older dispatches are ignored. A duplicate ACK arriving at the next heartbeat boundary cannot acknowledge the newly emitted heartbeat.

All18 Discord decoder/policy tests passed offline, including resume replay, recoverable/fatal close codes, rate-limit delay, missing ACK/Hello/auth timeouts, malformed inputs, untrusted resume endpoints and duplicate-ACK timing. Strict library/tests Clippy, scoped formatting and diff whitespace checks passed. Evidence: stage6-evidence/discord-gateway-policy/. No full-suite or live compatibility claim.

The policy emits effects only: socket/TLS, bot credential loading, actual Identify/Resume writes, reconnect scheduling/session-start quotas, host action delivery and media are still required. Decoder sequence admission is not host delivery evidence. The extension remains unavailable in Add until a functioning worker exists. No network/account calls, credentials, commits/pushes or running builds. Full goal remains active.


### Bounded Discord-to-host chat bridge — 2026-10-22

Added explicit saved server/channel configuration validation and a pure host bridge emitting normal chat.offer envelopes. A valid handshake, message grant and actual loaded target are required; offers capture the target rather than acquiring a later avatar. Bounds cover16 queued/in-flight messages, one pending action,15-second original TTL,10-second result timeout and256 source IDs retained60 seconds. Source dedup survives avatar changes and fails closed at capacity.

Only a correlated explicit Busy refusal permits up to3 attempts with1-second minimum spacing and the original expiry. Accepted/other-error/uncertain-timeout outcomes are not replayed. Matching includes action ID and target; host envelopes must match session/generation and advance host sequence. Avatar changes clear owned pending/queue state. App shutdown and malformed/stale-incarnation failures permanently quiesce this bridge; transport must also stop it on EOF/cancellation/output failure.

All27 Discord tests passed on final source; strict library/tests Clippy, scoped rustfmt and diff whitespace checks passed. Tests cover permission/target admission, encoded protocol-valid offers, retries, uncertainty, queue and dedup bounds, expiry, avatar replacement, shutdown, stale frames/results and explicit configuration. Evidence: stage6-evidence/discord-host-bridge/. No full-suite, real-child delivery or service acceptance claim. Socket worker, credential loading, actual supervised delivery and voice/media remain unfinished. No live calls, commits/pushes or running builds; full goal remains active.


### Experimental native Discord message worker — 2026-10-22

Wired --discord-extension to private bot credentials, explicit saved server/channel settings, the Gateway policy and standard host chat.offer bridge. Added an experimental manually installable discord-live package; it remains outside Add pending transport acceptance. Uses the existing proxy-aware TLS WebSocket helper with64KiB frame/message bounds, bounded HTTPS Gateway discovery, single-shard validation, session-start allowance checks, per-worker5-second Identify floor, heartbeat/resume, bounded reconnect attempts and redacted fixed errors. Outgoing Discord writes are authentication/heartbeat/control only; no Discord chat-send or voice transport exists.

Host input is bounded32 with out-of-band EOF/error/overflow; it is checked before service effects/output, after bounded connection/discovery and during reconnect waits. Failed host writes permanently stop uncertain chat delivery. Network control write/discovery failures stop for explicit restart. Eight reconnect failures stop, resetting only after60 seconds healthy. Invalid or oversized individual chat is rejected with a fixed diagnostic while preserving session/checkpoint, rather than killing authentication. No bot token is copied to host actions, settings or logs.

Final all-target offline suite passed1,170 tests with --test-threads=1, including35 Discord decoder/Gateway/bridge/worker tests and two actual CLI/package tests. CLI checks never supply a token and terminate before networking. Synthetic Gateway message→encoded host offer is covered in-process; broken outputs, private control frames, malformed discovery/quotas, input overflow and EOF are covered. Strict all-target Clippy, scoped rustfmt, git diff whitespace and debug build passed. Evidence: stage6-evidence/discord-native-worker/.

Verification limitations: the first sandboxed full run failed76 local fixture/socket/special-file operations with permission errors; an escalated parallel run then passed928 library tests but failed the existing extension_installed store-reopen test at lock acquisition. Serial full suites passed before and after the final chat-rejection fix. This does not establish the cause or repair of parallel store-lock contention; retain that finding.

No live account/provider/Discord calls, credential changes, commits/pushes or running builds. Actual local socket-loop fault tests, installed-host successful delivery, shared-token identify coordination across installations, live service compatibility, outgoing reply design and all call/media work remain open. The helper tests and compilation do not establish those gates. Broader motion/physics, pacing, streaming and distribution acceptance also remain open; overall goal remains active.


### Real localhost Discord session-loop tests — 2026-10-22

Extracted the production connected-socket loop into a private testable session driver, preserving the existing TLS/proxy connector, discovery, reconnect delays and per-worker Identify floor. Session errors now quiesce the host bridge as well as propagating the failure. Real TCP/WebSocket fixtures call that same driver using synthetic controls and credentials; they do not override production service URLs or contact Discord.

Five local socket tests cover actual fragmented Hello frames, Identify/READY, exact chat-to-host JSONL output with no token, app shutdown/EOF during idle read, missed heartbeat ACK, fatal4004 authentication close, binary-frame rejection and a second connection resuming/replaying with duplicate suppression. The shutdown case found a real input-loop error: after accepted app.stopping, the poller could read another input-channel state and report unexpected disconnect. It now immediately returns stopped.

All1,175 all-target offline tests passed serially on final source. Strict all-target Clippy, scoped formatting, diff whitespace and debug build passed. Evidence: stage6-evidence/discord-local-sockets/. No remote account/provider/service calls, credential changes, commits/pushes or running builds/captures. Fixtures use plain localhost sockets; this does not establish Discord TLS/discovery compatibility, cancellation during DNS/connect/HTTPS, actual installed-host successful delivery, all quota/failure cases or voice/media. Shared-token identify coordination remains open. The prior parallel store-lock contention is still unresolved; serial success does not repair it. Discord remains experimental/outside Add; full goal stays active.


### Store owner lock lifetime across fork/exec — 2026-10-22

Investigated the earlier parallel store-reopen failure. Reproduced the same lock-retention mechanism deterministically: a duplicated descriptor and a controlled Linux child paused before exec both keep the shared OS open-file description alive after the parent drops its store. Both new tests failed against close-only cleanup. CLOEXEC does not cover that pre-exec window. This demonstrates a matching failure mechanism; the original transient did not retain a child/descriptor trace.

Added a private StoreLock guard that explicitly unlocks before closing the owner descriptor. The guard is installed immediately after successful acquisition, covering subsequent open-error cleanup as well as normal store drop. Lock-file inode retention and active-owner exclusion are preserved. The duplicate-descriptor regression also verifies closing an old descriptor does not release a newly acquired store owner's lock. The fork fixture executes only pause in the child, then is killed/reaped by a parent guard; no child Rust allocator/destructor work occurs.

Both reproductions passed after the fix. All1,177 all-target offline tests passed using normal/default parallel test execution, including the formerly failing installed-store reopen case. Strict all-target Clippy, scoped rustfmt, diff whitespace and debug build passed. Evidence: stage4-evidence/store-lock-lifetime/ contains red/green targeted logs, full parallel suite, lint/build and final source hash. This supersedes the unresolved lock-retention item from recent stage6 checkpoints; it is not a universal concurrency or crash-recovery claim.

No live service/account/provider calls, credentials, commits/pushes or running builds. Discord installed-host/quotas/live/media, motion/physics, pacing, native capture and distribution acceptance remain open; overall goal remains active.


### Installed native Discord replay and import Busy semantics — 2026-10-22

Added Add→Examples→Discord replay (offline), an owned versioned package launching --discord-replay-extension. The native path uses embedded synthetic Gateway packets, the decoder and the normal chat.offer bridge; it contains no discovery/socket/credential calls. It waits for loaded-target authority, filters duplicate/wrong-channel/bot packets and replays once per process start. Restart mints one fresh synthetic message ID; duplicates within that start retain their identity. Host incoming-reply settings can still invoke configured models in ordinary app use; the fixture itself never does.

Two actual installed/reconciled/supervised-process tests passed: disabled/no-grant defaults, wait without target, unchanged package launcher→native CLI→host-authored attributed offer sink, exact original author/text, one accepted candidate, no replay on avatar change, fresh explicit restart, disable/reaping and cancellation of Busy retries after target change. A test-only launcher shim supplies the real application executable because runtime current_exe otherwise refers to the Rust test harness. The recording sink is the validated runtime offer boundary, not real conversation history, model replies, speech or rendered reaction. Both Discord packages also pass the read-only package CLI check.

A parallel full run exposed existing global animation-import contention being returned as Unavailable. Added typed MotionLoadError admission and a typed owned-load entry point, preserving existing string-returning UI APIs. Contention now returns Busy with no new playback request; failures retain Unavailable at the extension boundary. Updated the existing bounded retry test helper to the corrected code. A focused regression covers Busy versus invalid-path admission without taking the shared import permit. No import capacity, rendering, animation quality or playback behavior was relaxed.

All1,180 all-target offline tests passed serially on final source. Strict all-target Clippy, scoped rustfmt, whitespace and debug build passed. Normal parallel runs still expose motion-completion fixtures receiving a legitimate Busy refusal from the process-wide single importer; final recorded failure is the reply-gesture example, rather than the resolved store-lock problem. Parallel motion-fixture isolation remains open; serial success does not resolve it. Evidence: stage6-evidence/discord-installed-replay/ contains targeted, failed-parallel, final serial/lint/build logs and source hashes.

No live accounts/services/providers, credentials, commits/pushes or running builds/captures. Live Discord remains outside Add pending shared-token quota coordination, discovery/TLS/interruption and actual network-to-installed-host acceptance; actual conversation/history, voice/media and broader motion/pacing/capture/distribution gates remain open. Overall goal remains active.


### Independent motion fixture processes — 2026-10-22

Resolved the parallel motion-fixture interference recorded in the Discord replay checkpoint. The reply-gesture, one-shot hook and Twitch replay roundtrips require actual successful imports and terminal receipts; unrelated library-test imports were legitimately occupying their shared process-wide one-worker importer. Each of these three fixture tests now launches only its exact named case in a fresh harness, following the repository's existing isolated OS-resource test pattern.

Production import capacity, Busy behavior, asset loading, playback, completion, target correlation and saved-attribution assertions remain unchanged. The original inner8-second deadlines remain; a20-second outer bound covers child startup and kills/reaps a timed-out harness. The parent checks successful exit, exact selected test name and a one-test-passed receipt, so an empty exact-test selection cannot produce a false pass. No tests were ignored or accepted Busy as successful playback.

All three targeted cases passed. All1,180 all-target offline tests then passed using normal/default parallel execution on final source. Strict all-target Clippy, scoped rustfmt, whitespace and debug build passed. Evidence: stage4-evidence/motion-fixture-isolation/. The earlier serial-only motion-fixture limitation is superseded by this isolation checkpoint; production still rejects conflicting import admissions as Busy. No universal nondeterminism-free claim is made.

No live services/accounts/providers, credential changes, commits/pushes or running builds/captures. This verifies CPU/logical/headless playback and source attribution, not native rendered animation quality. Discord quotas/live/media, motion/physics visual review, pacing, capture, distribution and the full goal remain open.


### Shared Linux Discord Identify admission — 2026-10-22

Added an internal per-OS-user coordinator used before Identify writes by normal supervised native workers, across installation/profile data directories. A private fixed-inode file under the default Waifu Studio data directory stores only schema version, kernel boot UUID and next eligible boot-clock millisecond. It contains no token/account/message IDs. Admission reserves and syncs five seconds under a nonblocking OS lock; held locks return a short wait rather than blocking the session loop. Reservations survive worker restart and remain consumed after cancellation/uncertain writes. Resume bypasses Identify admission.

Uses Linux CLOCK_BOOTTIME plus kernel boot identity, independent of wall-clock changes and including suspend. New boot uptime must reach5 seconds even without saved state. Corrupt/oversized/inconsistent state fails closed. Root/file ownership, private permissions, leaf symlink rejection and one-link file checks are enforced. Unlock guards release on early errors. Coordination conservatively spans all this app's bot tokens for one OS user; it controls client admission, not server processing times, other applications/users/hosts or the full Discord daily budget. Service discovery still checks remaining session starts. Non-Linux backend remains unsupported with explicit error, matching current integration-runtime support. Offline replay does not open the coordination directory.

Eight coordinator regressions cover live kernel clock, two handles/restarts,8 simultaneous independent handles with one reservation, a separate exact child process observing prior reservation, contention/error unlock, reboot/backward state, initial-boot grace, symlink/permissions and size limits. Real localhost socket tests verify a shared wait prevents Identify, Resume does not request a slot, and EOF after reservation prevents control output. Core tests use synthetic boot times and temporary roots; no real default coordination state or credentials were touched.

All1,190 all-target offline tests passed in default parallel execution. Strict all-target Clippy, scoped rustfmt, whitespace and debug build passed. Evidence: stage6-evidence/discord-shared-identify/. This completes the same-OS-user cross-installation admission implementation gate, not live quota/service acceptance. Live Discord stays outside Add; discovery/TLS blocking-stage interruption, actual live network-to-installed history/reply, other platforms, voice/media and broader motion/pacing/capture/distribution gates remain open. No live calls, credential changes, commits/pushes or running builds/captures; full goal remains active.


### Discord discovery deadline and startup cancellation boundaries — 2026-10-22

Added private testable discovery client/request helpers while keeping the production endpoint fixed. Eight local HTTP/startup tests cover Bot/User-Agent headers, valid metadata, no redirect follow or credential forwarding to a redirect target, redacted HTTP errors,64KiB body bounds, slow-drip total deadline, host EOF around success/error operations, pre-EOF skip, late-value disposal and terminal input failure. Fixtures use localhost, synthetic credentials, no-proxy clients and temporary in-memory host channels. No runtime endpoint override is exposed.

The first targeted run passed7 tests but failed the slow-drip deadline: blocking Client::timeout restarted on each Read and the body lasted roughly2 seconds despite a150ms client timeout. Added request-level timeout to reach the underlying async body, using the existing appraisal-client pattern. Production now has an absolute8-second request/body deadline; the short400ms request fixture passes within its1.5-second scheduler ceiling. The client-only control would still exceed that ceiling. Red/green logs are archived.

A generic private startup-boundary wrapper now checks host state before and after both discovery and the existing proxy/TLS connector. EOF takes precedence over late operation success/error, and late returned values/sockets are dropped before continuation/authentication. Input failure remains terminal. This is boundary cancellation, not interruption inside an OS syscall; tests prove real HTTP boundaries plus generic returned-resource disposal, not a DNS/TLS handshake abort. The supervisor remains the termination boundary during blocked calls.

All1,198 all-target offline tests passed in normal/default parallel execution. Strict all-target Clippy, scoped rustfmt, whitespace and debug build passed. Evidence: stage6-evidence/discord-startup-boundaries/. No live service/account/provider calls, real credentials/default profiles, commits/pushes or running builds/captures. Live TLS/service compatibility, DNS/TLS blocking-stage interruption, actual live network-to-installed history/reply, voice/media, platform and broader motion/pacing/capture/distribution gates remain open. Goal remains active.


### Supervisor termination during blocked Discord startup — 2026-10-22

Added two Linux exact-child tests that execute production HTTP discovery and the existing proxy-aware WebSocket connector under the real ProcessHandle supervisor. One fixture withholds HTTP response headers after receiving a synthetic Bot-auth request; another withholds TLS ServerHello after an actual ClientHello. A separate child installs a permanently pending reqwest resolver hook; no DNS query or address is produced, and a trap listener receives no TCP connection. These are exact test harnesses calling the production helpers, not a live Discord worker or new runtime endpoint override.

Parents wait for physical entry evidence (HTTP headers, TLS record, resolver marker), confirm the process is still active, then stop it. Assertions require shutdown/reaping via the2-second shutdown API with2.5-second scheduling ceiling, retained Stopped termination, no after-operation continuation marker, absent child PID, rejected late input and closed HTTP/TLS sockets. The targeted two-test run, covering all three phases, completed in0.04 seconds. Child-only launch shims clear all proxy variables and route harness text away from protocol stdout. No parent proxy/system configuration changes occur.

All1,200 all-target offline tests passed in normal/default parallel execution. Strict all-target Clippy, scoped rustfmt, whitespace and debug build passed. Evidence: stage6-evidence/discord-blocked-supervision/. No live services/accounts/providers, real credentials/default profiles, commits/pushes or running builds/captures.

This verifies supervised termination/reaping and socket closure during representative stalled startup operations. It does not establish successful trusted TLS/certificate acceptance, a stall inside native OS getaddrinfo, graceful in-call cleanup, live service/quota compatibility or full installed live-worker network-to-history/reply acceptance. Those, voice/media, other platforms and the broader motion/pacing/capture/distribution gates remain open; the full goal remains active.


### Owned localhost HTTPS/WSS certificate acceptance — 2026-10-22

Added four real TLS tests with owned public fixture material: test-local CA acceptance for HTTPS discovery and WSS upgrade/Gateway Hello, plus rejection of unknown roots and wrong IP peer identity for each transport. The negative certificate shares the fixture issuer/key and differs in its SAN so name validation is exercised independently of trust. HTTPS verifies a synthetic Bot header only after trusted TLS; WSS verifies no provider authorization/model header is sent. Failed TLS yields no decrypted application request or WebSocket upgrade and fixed redacted errors.

Factored the observation connector without changing production roots/routes/limits: production callers pass the default connector; a cfg(test) wrapper supplies fixture connectors and accepts only WSS127.0.0.1 with empty environment lookup. Root stores are local to test clients; no OS or global trust changes occur. Only a cached dev-dependency rustls0.23.46 was added; existing Cargo edits were preserved and no package downloads occurred. Owned CA/peer DER and public test-only server key are checked in with provenance and an optional OpenSSL regeneration script. Tests read DER directly; fixtures are valid2020–2120.

All1,204 all-target offline tests passed in default parallel execution, including prior proxy/failure/cancellation checks. Strict all-target Clippy, scoped rustfmt, whitespace and debug build passed. Evidence: stage6-evidence/discord-local-tls/ includes targeted/full/lint/build logs and hashes for code, metadata and all fixture files.

No live Discord/provider/account calls, real keys/default profiles, commits/pushes or running builds/captures. The success uses fixture trust, not Discord's actual public certificate chain. Actual DNS/public trust/service/quota acceptance, full installed live network-to-history/reply, native OS resolver fault injection, voice/media, other platforms and broader motion/pacing/capture/distribution remain open. The live adapter remains experimental/outside Add and the full goal remains active.


### Actual application replay-to-history check — 2026-10-22

Added developer CLI --verify-discord-replay, an explicit execution check that owns a fresh private temporary profile and accepts no profile arguments. It installs/enables the bundled replay, launches the unchanged Python/native package entry point through InstalledRuntime (no test launcher shim), and uses the real StudioExternalMessages sink, IncomingChat queue and Store history. A synthetic headless target supplies host authority; no renderer/VRM is started. It waits for the actual four-second chunk window rather than advancing test time.

Chat/voice/model discovery/replies are disabled, provider and decision routes are invalidated, and memory intake is left enabled for boundary verification. Success requires one external turn, exact original source/author/text, host origin, an observed queued-before-delivery stage, disk/UI history agreement, preserved composer draft, unchanged memory candidates/records/notes/generation/triage status with no busy work, no response/picker work, no credentials, installation opt-in defaults, and successful worker/profile cleanup. A shared read-only Studio response-work predicate preserves the prior delivery Busy check. The new queue status query is read-only.

Two real application-binary integration tests passed: the full normal launcher→app intake→disk path and rejection of caller profile arguments before any profile access, preserving a supplied sentinel directory. A final manual execution emitted status ok, history_rows1, role external, queued_before_delivery true, disk_matches true, memory_unchanged true, model_work_started false, draft_preserved true, credential_free true and cleanup_verified true, with empty stderr. Final JSON is archived.

All1,206 all-target offline tests passed with default parallel execution. Strict all-target Clippy, scoped formatting, whitespace and final debug build passed. Evidence: stage6-evidence/discord-application-history/. No live accounts/services/providers, real credentials/default profiles, commits/pushes or running builds/captures. This closes the offline installed replay→actual app history gate, not live network-to-history/reply, rendered speech/motion, voice/media, native OS resolver faults or platform/distribution acceptance. Broader motion/physics, pacing and capture gates remain open; overall goal remains active.


## Native pacing: safe Mailbox default — 2026-10-22

The normal native window now requests Mailbox. A documented local `bevy_render` 0.19.1 patch resolves unsupported Mailbox directly to FIFO, retaining VSync; the old intermediate Immediate fallback is removed only for Mailbox requests. Actual surface creation/reconfiguration logs requested and resolved mode. Explicit FIFO and diagnostic auto/no-VSync policies remain unchanged; overrides still require metrics. Frame caps, deadlines, rendering quality, shaders and assets are unchanged.

Four exact-source capability tests cover supported/unsupported Mailbox, including Immediate-capable surfaces without Mailbox. The patched debug native Wayland FIFO → default → FIFO reversal measured 20 → 59.2–59.6 → 20 app updates/sec with the same focused 2240×1575 workload and asset counts. Actual surface modes were FIFO → Mailbox → FIFO. A metrics-disabled launch ignored a stray diagnostic FIFO override, resolved Mailbox and reached VRM readiness without metrics. All four owned native fixture processes ended, with no errors; existing app instances were untouched. No real credentials/services/speaker output were used.

All 1,337 serial offline all-target tests passed across 46 suites, zero failures/ignored. Strict all-target Clippy, package formatting, whitespace and debug build passed. Evidence and patch provenance: `/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/reports/stage3-evidence/safe-mailbox-default/`. Unsupported Mailbox fallback is capability-test evidence, not a native unsupported-adapter run. App-update timing and renderer brackets do not prove physical display smoothness/tearing, GPU timing, release timing, long sessions or platform acceptance. Broader roadmap gates remain open; changes are uncommitted/unpushed, and no owned build/test/capture remains running.


## Relocated examples and optimized pacing — 2026-10-22

Extensions → Add now resolves shipped examples from executable-adjacent assets instead of requiring the build checkout. Development binaries without adjacent assets retain source discovery. Omitted packaged examples do not borrow a development copy; unavailable starter/examples are disabled with a concise hint. Existing Twitch availability, package validation and disabled/ungranted installation behavior are preserved. Real owned example relocation to a directory containing spaces, installation with a nonexistent development-root fixture, packaged omission/development fallback and egui disabled/enabled pointer behavior are covered by regressions. This is not complete-package or native-menu acceptance.

The corrected locked/offline optimized build passed. Native release FIFO → default → FIFO, 48s each with matching focused 2240×1575 workload, resolved FIFO → Mailbox → FIFO and measured 20 → 59.8 → 20 settled app updates/sec. Default settled frame-interval p99 was17.17–17.24ms. A metrics-disabled startup ignored a stray FIFO probe and resolved Mailbox. All owned fixture children stopped, with no recorded errors/fallbacks; existing apps were untouched. No credentials, live services or speaker output were used.

All1,341 serial offline all-target tests passed across46 suites, zero failed/ignored. Strict all-target Clippy, package formatting and whitespace passed. The initial release warning from an import moved to test-only use was corrected before final checks. Evidence: `/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/reports/stage7-evidence/portable-examples-release/`. Physical presentation/tearing, a complete relocated package, long sessions and other-platform acceptance remain open, as do broader motion/live-service gates. Nothing committed/pushed; no owned build/test/native fixture remains running.


## Private package with source hidden — 2026-10-22

Shared asset discovery now covers characters, motion lists, illustrated backgrounds, room art, examples and headless replay. Release builds require executable-adjacent assets or an explicit WAIFU_STUDIO_ASSET_ROOT; source run.sh sets that root. Debug fallback and explicit external motion libraries remain available; the old local WebWaifu4 checkout is development-only. The first Clippy run caught a redundant and_then(Some) after simplifying room paths; corrected before final checks.

Created the private Linux candidate on the spare drive, with 136 hashed runtime files, no profiles/credentials/source/cache and no unapproved external generated motion pack. Actual archive extraction/relocation passed17 manifest checks, actual installed offline replay and3 native captures under bubblewrap: home/source hidden, IP network disabled, minimal HOME/XDG/profile/TMP environment, read-only package, unrelated working directory. First load seeded22 models; a second move with spaces/non-ASCII repaired22 model paths and preserved fixture selection/persona/background RGB. Captures and logs showed room art, shipped illustration, VRM readiness and actual Mailbox; no native errors, and package inventory stayed unchanged. The fixture manually repointed its absolute illustrated-image path, so generic reference migration is not verified. An initial manually loaded head-only BVH showed arms out; normal automatic startup showed lowered arms. These stills are not full motion/conversation/clipping or settings interaction acceptance.

All1,344 serial offline all-target Rust tests passed across46 suites, zero failed/ignored. Five Python packaging regressions, strict all-target Clippy, optimized locked/offline build, formatting and whitespace passed. Opus5.5 completed two generic packaging-plan reviews without source/tools; Muse1.0 reached Meta but returned subscription-quota HTTP429 and no review. No private-source export was performed. Logs/dispositions/source/harness/receipts/captures: `/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/reports/stage7-evidence/private-package-v1/` and sibling portable-plan-reviews/. Archive and separate SHA256 sidecar: `/run/media/subsect/toshiba/waifu-studio-private-packages/waifu-studio-linux-x86_64-private-20261022-v1.tar.gz`. This is a private same-host candidate, not redistribution or cross-distribution/platform approval. Native interactive/streaming/live/long-session and motion quality gates remain open. All owned runs terminal; no commit/push/publication.


## 2026-10-22: reviewed motion and bundle fixes

Fixed queued emotion admission, revocation of cached/mixed/parked motions, incoming-only cancellation, and recorded bundled-model relocation. Opus 5.5 reviewed three successive candidate snapshots; its final pass reported no verified findings. The final unchanged source passed 1,356 serial offline tests across 46 suites, strict all-target Clippy and formatting. Six behavioral probes fail against a separate old-source snapshot. Source hashes, raw results and review dispositions are in `/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/reports/waifu-fix-loop-20261022/`. DeepSeek completed through the official OpenCode client after a supported per-run permission/reasoning configuration change; its returned model, stop completion and zero cost were verified by the native session export. Its claims were dispositioned against current source. Earlier HTTP403 and reasoning-only length failures were preserved; no alternate provider was used. The optimized locked/offline executable rebuild passed. These checks are not new rendered, collision, live-service or long-session acceptance. The earlier private package was not updated.

The same-frame park/restore follow-up preserves queued pose-cache invalidations. The latest hash-bound checkpoint is candidate-round4; earlier review/replay and closed live-run attempts retain their original scopes.


## Latest motion fixes and Squad / Harness² verification

Candidate-round7 passed 1,359 serial offline tests across 46 suites, strict
all-target Clippy, formatting and an optimized locked/offline build. All six
scoped source/context hashes match. All 41 animation tests pass. New behavioral
contrasts proved stale cache-path attribution after revocation and after stop
before fixes. The final animator clears cached clip/path identity on revocation
and whenever it restores idle, preserving unrelated later crossfades.

The real Harness² round on candidate-round6 completed eight native Opus 5.5 /
OpenCode DeepSeek reviews, four rollouts and three refinement stages through
actual Squad task receive/acknowledge/delivery. Full OpenCode stdin and native
session/model identity were verified; export capture uses a bounded regular file
after a 128 KiB pipe-truncation failure was reproduced. The final round7 patch
received a valid no-findings Opus review. DeepSeek's additional targeted answer
was rejected for incomplete finding evidence, not accepted as clean. Full-round
and targeted-patch scopes are distinct in review-matrix.json.

Evidence: /home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/reports/waifu-fix-loop-20261022/.
Rendered/interactive/live-service and long-session acceptance remain separate.
No app launch, credential/profile change, package publication, commit or push.
All owned build/test/review work is finished; bridge Squad sessions archived.

## Desktop sitting camera regression — 2026-10-23

Sit/stand transitions no longer invalidate the companion camera fit. Refitting
from transient animation bounds could shrink the camera distance, magnify the
avatar and discard the user's zoom/pan/orbit. Explicit recenter, avatar/roster
changes and portrait/room framing still request their existing fits.

Two new production-system regressions failed before the fix and pass after it.
They check repeated sit/stand transitions, preserved user camera/placement
adjustments and projected head size/headroom in a 420×640 companion viewport
through synthetic pose-bound changes and small head movements. All 210
application tests, strict all-target Clippy, package formatting and whitespace
checks passed. Logs: `/tmp/waifu-seating-before.log`,
`/tmp/waifu-seating-after.log`, `/tmp/waifu-seating-tests.log` and
`/tmp/waifu-seating-clippy.log`.

The optimized locked/offline build passed (`/tmp/waifu-seating-release.log`).
Relaunched through the CLI as `waifu-studio-input-fix-20261023.service`; the
current invocation loaded the VRM with Mailbox presentation and no recorded
errors/panics (`/tmp/waifu-seating-startup.log`). No mouse input was injected.

This is numerical/fixture evidence; actual model framing, native dragging and
window seating still need interactive acceptance. The earlier input changes
remain in place: companion input stays enabled unless full passthrough is
explicitly requested with ready recovery controls, and startup opens no settings.

## Desktop size, taskbar geometry and stacking — 2026-10-23

The native Wayland winit backend implements `set_window_level` as a no-op.
The KWin bridge now applies the requested keep-above flag only to the exact
companion PID/title, reports its actual stacking order relative to visible
panels on that output, and restores the original flag when its temporary script
finishes. Bridge teardown allows a bounded stop exchange before unloading.
The Hyprland adapter explicitly reports that above-panel stacking is unavailable
for its ordinary client surface; this does not claim generic Wayland support.

Placement bounds now have a distinct `SurfaceId::Monitor` identity. They remain
available for moving/snapping but cannot create taskbar seats. KWin supplies
seats from actual visible bottom docks or reserved bottom edges; Hyprland uses
reserved bottom edges. Explicit snap requests use verified usable output bounds
when the bridge is active. The old monitor-bottom fallback is removed. A taskbar
seat no longer writes depth masks for the panel or every overlapping application;
ordinary window-ledges retain their depth occlusion.

Desktop scroll/size controls grow the render surface uniformly with the avatar,
up to the current monitor's bounds, rather than moving the camera into a fixed
420×640 surface. Studio camera zoom remains available. Production-system tests
verify doubled projected head size in pixels with unchanged camera/headroom,
monitor fitting, preserved sit/stand framing and snapping above reserved panels.

The shipped KWin script executes against owned fake compositor objects in
`tools/test_kwin_surfaces.cjs`: all five cases pass. The identical cases against
the saved pre-change script produce four failures and one pass. They cover actual
stack-order reports, restoration, a bare monitor bottom, floating taskbar edges
and PID/title targeting. These are script fixtures, not native visual acceptance.
Logs: `/tmp/waifu-taskbar-kwin-before.log`, `/tmp/waifu-taskbar-kwin-after.log`.

All 1,374 serial offline Rust tests across 47 suites passed, with zero failures
or ignored tests. Strict all-target Clippy, package formatting and whitespace
checks passed. The first suite run caught one old Hyprland fixture assertion
that classified a monitor with no bottom reservation as a taskbar; it now checks
the distinct monitor bound. Logs: `/tmp/waifu-taskbar-tests-final.log` and
`/tmp/waifu-taskbar-clippy.log`.

The optimized locked/offline build passed (`/tmp/waifu-taskbar-release.log`).
The CLI-launched current native KDE/KWin Wayland invocation reported actual
above-panel stacking changing from `Some(false)` to `Some(true)` before VRM
readiness, with Mailbox presentation and no recorded errors/panics. This proves
the running window moved above a visible panel in KWin's stacking order; it is
not rendered seating/dragging acceptance. Receipt: `/tmp/waifu-taskbar-startup.log`.
The companion remains running for the user's test. No pointer injection,
compositor configuration edits, commits or pushes were performed.

## Voice shortcuts and complete menu audit — 2026-10-23

Added one voice-action router for focused keyboard input, the desktop context
menu and Voice settings. Ctrl+Shift+Space starts/stops the microphone,
Ctrl+Shift+M toggles voice replies, Ctrl+Shift+Period interrupts speech and
cancels microphone work, and Ctrl+Shift+Comma opens Voice settings at the
shortcut section. Existing capture/transcription, source-character attribution,
draft protection, speech cancellation and voice persistence paths are reused.
The desktop has a small busy/recording indicator with Stop/Cancel actions.

Linux global shortcuts use the GlobalShortcuts portal through a private worker
connection. Setup is explicit in Voice settings; the desktop's dialog selects
and approves bindings. Displayed keys come from the actual portal response.
Matching global actions suppress native-key fallback to prevent double toggles;
unfocused native input, foreign sessions, unbound IDs, held-key repeats and old
worker generations are rejected. Disable/error/exit cancels requests and closes
the owned session/connection off the render thread. The actual KDE desktop
reported GlobalShortcuts version 2 by introspection; no real registration or
physical shortcut/microphone/provider acceptance was performed.

Audited all ten main routes and all 24 advanced control modules. Shared cards,
rows, fields, switches, dropdowns, popups and spacing are compact and responsive;
technical sections are collapsed. The primary/auxiliary desktop contexts use
the same style as Studio. Replaced the radial with a compact context menu and
bounded the character picker. Existing settings/actions/defaults and extension
review gates remain. The new main-menu shortcut route opens the shortcut section
without forcing a settings window at startup.

All 1,390 serial offline Rust tests across 47 suites passed. This includes
13 shortcut/portal checks, shared narrow/wide row and widget-ID checks, and
small-companion context menu bounds. The portal tests use serialized messages,
in-memory queues and cancellation probes; they are not a private-bus or real
portal registration test. A layout-sensitive existing emotion dropdown test now
targets the foreground None item rather than an identically labelled closed
row. Strict all-target Clippy, package formatting and whitespace passed.
Logs: `/tmp/waifu-menus-tests-final.log`, `/tmp/waifu-voice-hotkeys-tests.log`,
`/tmp/waifu-menus-clippy-final.log`.

Rendered and inspected 22 real GPU captures: ten default routes, nine runs with
the Advanced flag and three 960×720 checks; the other captures use 1280×900. Every capture
used an isolated credential-free profile, disabled audio playback and an
unshared IP network. Empty fixture libraries do not prove populated extension,
history or per-record menu acceptance. Captures/receipts are in
`/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/reports/voice-hotkeys-menu-review-20261023/`.

The optimized locked/offline build passed (`/tmp/waifu-menus-release.log`).
Restarted only the conversation-owned test service through the CLI. The current
native invocation loaded the VRM, resolved Mailbox presentation and reported
actual KWin above-panel stacking false→true without recorded errors/panics
(`/tmp/waifu-menus-startup.log`). It remains running for interactive testing.
No microphone/provider calls, physical input injection, real portal shortcut
registration, commits or pushes were performed. Final review receipt and scoped
source hashes are saved beside the captures.


### Desktop radial restoration (2026-10-23)

Restored the existing eight-wedge desktop radial instead of the unrequested list-menu replacement. Reviewed the local Mate Engine CircleSelector source for pie/hover interaction; no Unity code or assets copied. Voice opens a five-wedge secondary radial for microphone recording, voice replies, stop, hotkey settings, and dismissal. Preserved primary-window input, drag suppression, explicit passthrough recovery, character picker, and recording status controls. Radius adapts to small companion windows. Three targeted regression tests cover compass/dead-zone selection, all voice wedges, sit/stand labels, and viewport containment at 170, 210, and 420 pixels wide. Native mouse selection and microphone behavior remain unverified.

Restoration validation: all 226 application binary tests passed; strict binary Clippy, package formatting, whitespace checks, and release build passed. CLI relaunched the conversation-owned companion service. No physical mouse or microphone acceptance claimed.


### Radial visual polish (2026-10-24)

Reviewed local Mate Engine `CircleSelector.cs`, `Button.cs`, selector/button prefabs, and the main scene menu overrides (icons, ring separators, hover coloring, scale animation). Implemented original vector icon artwork and an original charcoal/lavender palette in `src/desktop_menu/paint.rs`; no Unity artwork/code copied and no new runtime dependencies. The existing main and voice wheels/actions remain, with contextual center captions, short hints, a 160 ms opening animation, animated wedge feedback, a feathered rim and icon-only compact layouts. Paint animation leaves hit-test geometry fixed. Existing egui animation-time preference is respected.

All 227 application binary tests passed on Rust 1.99.1. Four focused radial checks also cover dead-zone/direction/voice wedge selection, sitting labels, bounds and actual egui mesh paint validity. Software-rendered review images derive from production egui tessellation plus its font atlas; they verify static UI appearance, not native GPU compositing, physical mouse acceptance, or temporal animation. Binary Clippy completed with inherited warnings; strict Clippy stops at an existing needless borrow in `src/expressions.rs:1124`, and Rust 1.99 additionally deprecates existing `fetch_update` calls. Those unrelated files were not changed.


### Saved profile cards and desktop switching (2026-10-24)

The existing Character records remain the profile storage format; no library migration or UUID reassignment. Studio navigation now labels this page Profiles. Portrait cards, search, explicit Save profile / Save as new, editable avatar selection and Use on desktop share the existing per-character model/personality/voice/provider/appearance settings. The radial Profiles action opens a compact thumbnail switcher with Profiles, Avatar and Personality tabs. New model/personality combinations clone source customization under a fresh UUID without copying history/facts; existing combinations restore their own identity. Existing custom prompt text is preserved. Source originals and fresh combinations persist through Store reopening in the regression.

Portrait extraction runs serially on a bounded background worker, deduplicates by model path, caps cached models at96, reads at most128 MiB per model and8 MiB per embedded image, limits decoder dimensions/allocation, resizes to192 px, and never fetches external images. VRM0 texture references and VRM1 image references are covered with synthetic GLBs; real Hikari, Neuro and Riko portraits are loaded in headless egui card layout checks. Gallery widths760/420/280 and switcher windows420x640/210x320 have layout/paint checks. Software review PNGs use actual production egui meshes/font and portrait textures, not a separate mockup. An outer switcher scroll area handles short windows.

Native Vulkan offscreen captures at1280x900 and960x720 failed before UI rendering: NVIDIA userspace/kernel driver mismatch (NVML library615.78). No desktop driver changes or app-service restart were attempted. Physical clicks, native GPU appearance and temporal animations remain unverified. Existing unrelated deprecation/Clippy warnings are retained.

Final profile validation: 231 binary application tests passed, final compact switcher layout test passed after sizing refinement, package formatting/whitespace passed, binary Clippy completed with the existing library warning, and final release build passed. Review images and source hashes are saved in the calling chat reports directory. The app was not relaunched because the native graphics driver mismatch remains unresolved.


### Hikari ARKit default (2026-10-24)

User explicitly selected the ARKit export. Copied and hash-verified the recovered Blender export (133 face morph targets,52 ARKit channels,88 expression clips), updated only the bundled Hikari model/hash and appended its original metadata/provenance. Updated the two identified saved Hikari profile model pointers using a private backup and atomic replacement; normalized JSON comparison confirms all other fields and the selected UUID unchanged. The existing bundle-reference receipt was updated for stock Hikari. No conversation, memory, credential, voice or personality file was edited. Older VRM files remain available. Runtime facial deformation and appearance are not claimed.


### Hikari versions, final selection (2026-10-24)

Following the user's revised preference, Wardrobe is the default; ARKit remains a separate profile. All five recovered Blender exports plus the original cool_HIKKY and older hikkyc2 are exposed as seven named Hikari choices. Copied VRM hashes were verified, source morph/expression counts recorded, and author metadata retained. Six stable variant UUIDs were added to the bundle (28 profiles total, three distinct original personality templates). The saved library was privately backed up before atomic updates, with comparison proving preservation of every pre-existing profile field other than the two explicitly changed Hikari avatar pointers. New variants were appended, with import receipts, without changing the selected profile identity. No chat, memory or credential file changed. Previous ARKit-only default notes above describe the intermediate selection, not the final default. Native model appearance and clipping remain unverified.

## 2026-10-25: Windows/Linux preview preparation

Version `0.1.0-preview.1` excludes Elmer, Zome and Lazuli from the bundled catalog and runtime asset trees, retaining 25 character profiles. The catalog test now expects 25. The restored personal profile remains outside release inputs.

Local optimized, locked all-target tests passed serially: 1,207 on Windows and 1,396 on Ubuntu 24.04 under WSL. Six Python package-integrity tests passed, including tagged-preview provenance checks. Package-scoped Rust formatting and GitHub workflow validation passed. Vendored upstream examples require source files outside this checkout, so formatting is scoped to `waifu-studio`, not all vendored packages.

The staged source audit found no known local provider credentials or credential-shaped secrets. All retained bundle model hashes matched. Runtime archives exclude profiles and record file hashes plus the source commit. These checks do not establish native Wayland/KDE/Hyprland, every GPU/audio device, or long-session acceptance; see `docs/PREVIEW.md`.


## Product walkthrough and CI test synchronization — 2026-10-25

Expanded the product README with setup, a conversation-flow diagram, feature-by-feature workflows, credentials/data behavior and current preview limits. GitHub Markdown rendering succeeded (47 headings, seven tables, three images); 49 local documentation/image links and section anchors resolved. Updated stale bundled-profile counts, Fish library search behavior, microphone defaults and Windows integration/routine guidance.

The first GitHub Linux job failed in `worker_retunes_in_place_without_restarting_smoothing_from_silence`. The test could send new settings after the first vowel output while initial PCM remained queued, then miss the brief invalidation before that queued audio republished output. The test now waits for the initial batch to drain before changing settings. Production worker behavior and packaged binaries are unchanged.

Validation: all 12 `viseme_audio::tests` passed in release mode on Windows and Ubuntu 24.04/WSL; the updated retuning test passed 100 consecutive executions on each platform. The complete Linux library target passed all 1,088 tests serially. Package-scoped Rust formatting and diff whitespace checks passed. The original GitHub result remains a failed test receipt; a new workflow must validate the corrected revision.

## 2026-10-25: Visible desktop menus, preview 5

Desktop radial actions, profile switcher and voice status now fit the visible monitor intersection instead of the full companion backing window. This matters when taskbar seating intentionally leaves the legs below the monitor. The desktop bridge supplies compositor geometry; native monitor/window geometry is the fallback. Coordinate conversion accounts for DPI, negative monitor origins and spanning windows.

Validation: all 229 Windows and 239 Linux application tests passed in optimized builds; both release executables built. Three new geometry regressions cover seated clipping at 1x/2x DPI, negative/spanning monitors and unavailable geometry. Native Windows capture confirmed seating and a complete nine-action radial menu in the visible window. The automation helper restores a partially off-screen window before clicking, so physical clicks while the backing window remains clipped are not claimed. The prior animation probe on both platforms demonstrated idle, speech-driven talking, a reaction gesture, return to talking during speech and return to idle after speech. Retargeted motion can still cause model-specific hair/clothing intersections.
