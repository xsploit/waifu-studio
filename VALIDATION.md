# Validation receipt

Date: 2026-10-18; earlier GPU observations are identified below. Linux KDE/Wayland, NVIDIA GeForce RTX 5060 Ti, NVIDIA 615.71.09, Vulkan. Rust 1.98.1. These observations apply to this source/build, not all devices or imported avatars.

## Compiled and automated checks

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
