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
