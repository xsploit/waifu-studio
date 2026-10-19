# Waifu Studio: review and test handoff

Updated 2026-10-18. Latest published source baseline when updated: `4b8f1e4` on `main`. Inspect current HEAD and working-tree status before reviewing; concurrent implementation may be in progress. Private remote: https://github.com/xsploit/waifu-studio.git.

## Prompt for the reviewing model

Review this Rust/Bevy application for reproducible defects and test the implemented user flows. Start read-only: inspect the current HEAD, working-tree changes, and the documents below. Historical agent reports are leads, not proof that a defect still exists. Do not implement unrelated features or weaken tests to obtain green results. Preserve other agents' edits.

Report findings in severity order with **file:line, trigger, reproduction steps, expected and actual behavior, evidence tier, and the smallest correct fix**. Separate verified defects from hypotheses. State exactly what you did not test. Passing offline tests, a screenshot, and real interactive acceptance are different evidence levels.

## Read first

- `README.md`: setup and operating instructions.
- `FEATURE-STATUS.md`: implementation and acceptance matrix.
- `VALIDATION.md`: receipts; newest sections are at the bottom. Older test counts are historical.
- `benchmarks/RESULTS.md`: measured latency and limits.
- `reviews/`: historical Opus/Fable reviews; check findings against current source.
- `product-video/README.md`: video provenance.
- `BRAIN-OPTIONS.md`, `MEMORY-OPTIONS.md`, `DECISION-OPTIONS.md`, `CUSTOMIZATION.md`, `TAMANITOMO-FEATURES.md`: designs and proposals, not blanket implementation claims.

The ignored `work/integration-backlog.md` is a historical snapshot with an overriding note. Many entries marked absent there have since shipped.

## Environment and verification

**Announce before every compilation, Cargo build/test/Clippy run, or GPU render, and announce when it finishes.** The user needs to distinguish our load from unrelated system lag. Coordinate one Cargo process across all workers; do not assume a shared build-directory lock means no other worker is active.

Local checkout:
`/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio`

Current host is KDE Plasma/KWin Wayland with NVIDIA RTX 5060 Ti, driver 615.71.09, Vulkan, Rust 1.98.1, Bevy 0.19. Hyprland is installed but inactive. Keep dependency versions pinned.

Run from the repository root. **One Cargo process at a time, one build job**: the user reported desktop lag. Do not start duplicate builds in parallel.

```bash
CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=../../work/studio-target nice -n 10 cargo test --all-targets
CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=../../work/studio-target nice -n 10 cargo clippy --all-targets -- -D warnings
cargo fmt --package waifu-studio -- --check
git diff --check
```

Do not use `cargo fmt --all`, which also touches vendored code. Existing target artifacts are reusable; do not delete them unnecessarily.

Independent Muse review observed **503 passing tests** (406 library, 90 application/controller/UI, 6 integration, 1 benchmark) on its documented moving tree: `88cfbfc` plus `7f5ef9f` and material-isolation work subsequently committed as `cc1e0b2`. Preserve that scope caveat; see `reviews/muse-independent-review.md`.

Later SSE sentence scheduling passed 4 queue tests, 19 Studio tests, 2 benchmark tests and strict all-target Clippy. The hosted HTTP/Low trial preset passed all 16 voice tests, strict Clippy and package formatting. The next integrated run passed 522 tests (423 library, 91 application/controller/UI, 6 integration, 2 benchmark), including material expressions, local fictional routines and bounded SSE cancellation admission. These are dated receipts, not a fresh run by this document's reader.

Latest full-suite receipt at clean source HEAD **4b8f1e4**: `cargo test --all-targets` passed (453 library, 95 application, 6 integration, 2 benchmark = **556 tests**), zero failures. Strict all-target Clippy passed in the immediately preceding source slice. Includes authored gaze, optional VAD silence trimming, focused live mouth diagnostics and local WAV/MP3 preview. Historical 552-test and older receipts remain in VALIDATION.md. These are offline checks, not physical audio or interactive acceptance.

Newest review priority: local audio preview in `studio.rs`/`ui.rs`: bounded nonblocking regular-file read, stale generation/character rejection, Stop behavior, decoder failures and shared Voice Design playback/viseme path. Verify this can replace ongoing speech cleanly without changing saved voice settings. No fresh independent Claude review has run for the trim/diagnostics/local-preview slices. Live desktop control is currently unavailable because the KDE physical-input guard is missing; do not bypass it. An isolated offscreen Scene/avatar render succeeded, but did not expose the Face diagnostics below the viewport.

Review priority: `appraisal.rs` plus focused/group integration. Check opt-in source exclusivity, bounded two-worker admission including cancelled requests, 1.5 s result lifetime, strict offered choices, stale turn/character/config rejection, and error redaction. Group jobs may wait briefly for appraisal after handing off speech; late focused appraisals do not replay completed body-motion decisions. No fresh independent Claude review has run for this slice.

Recent expression-gaze work: review authored VRM1/VRM0 range maps, Hermite curve parsing, anatomical direction signs, one active eye-drive mode, root-tagged same-frame input and manual/overrideLookAt priority. Bone rigs now obey authored ranges, so movement can be smaller than the previous procedural overlay. Tests do not establish visual fidelity; world-space target offsets and companion gaze are not implemented.

## Priority test matrix

| Area | Review and exercise | Evidence still needed |
|---|---|---|
| Microphone and automatic send | `listening.rs`, input controls and studio send path: opt-in defaults off; recording captures character/intent; empty draft required; busy chat, draft changes, cancel, failure and character switches preserve user input. Old `start` remains review-only. | Real microphone/VAD/ASR and send behavior. No always-listening or full-duplex claim. |
| Lip sync | Inspect configured/default RMS and calibrated fallback paths. Check stop, silence, voice changes and timing against actual playback. | Physical speaker onset and perceived mouth synchronization; decoded audio timestamps are insufficient. |
| Speech pipeline | `provider.rs`, `fish_ws.rs`, `voice.rs`, `reply_speech.rs`, `studio.rs`, `scene_chat.rs`: streaming before LLM completion, metadata suppression across split markers, chunk order, bounded prefetch, cancel, stale generations, errors, slot cleanup. | Live playback and inter-chunk gaps across all deliveries; stalls and network failures. |
| Desktop companion | KWin mesh hover click-through, radial menu only on character, drag release/recovery, surface sit/stand, seat disappearance and snap via bridge. | Real compositor interaction. Hyprland has fixture/socket tests only. Transparent texels within mesh triangles are a known hit-testing gap. |
| Motion/import | FBX/BVH/VRMA rest poses, rotated/scaled armature parents, stop/restoration/crossfade, local emotion playlists, cooldown, stale avatar/character and manual override. | Smooth runtime playback, source fidelity and arbitrary real rigs. Two rendered poses do not prove these. |
| Appearance and gaze | Cel looks, RGB correction disable, bloom ordering, preview vs primary camera selection, gaze restoration, alpha, material ownership and clothing visibility. Material-only expression colors and shared UV0 transforms restore across weight/root changes; inspect shader uniform layout and preserve matcap view-space sampling. | Interactive camera/shader behavior and representative avatars. |
| Fictional weekly routine | Off-default persisted scene config, duplicate/invalid anchor rejection, current/next activity at exact boundaries and week rollover; system-local clock and per-character switching. | Live panel editing and timezone/DST clock changes; no autonomous scheduler or user-presence inference claim. |
| Memory and scene | Character isolation, prepared recall traces, budgets, pinned notes, whole-token scoring, correction/expiry chains, conservative near-duplicates, failed writes and character switching. Explicit note links are undirected, character-local and opt-in for one-hop lexical recall; verify linked candidates come only from inserted directly matched seeds, obey existing caps and report provenance. | Real UI recovery and persistence failures. Verify traces describe prepared context rather than provider delivery. |

Use isolated fixture data rather than the user's actual conversations or settings. Do not enable paid requests through accidental GUI interaction.

## Important recent changes to inspect

- `9360ec2`: shared bounded lexical recall terms; function-word filtering, Latin token boundaries and CJK substring fallback.
- `ed84e05`: hips translations transformed through source/target armature parent rest frames, including rotated/nonuniformly scaled parents.
- `f253ad1`: fear/disgust affect and matching custom facial aliases; 20 emotion labels in motion controls.
- `cc26ca1`: session-only per-recipient scene recall inspector; context ordered persona, history, memory, latest user. No measured cache benefit claimed.
- `925d33d`: actual recovered WebWaifu4 Wave FBX retarget captures at 0.5 and 1.5 seconds.
- `84ddab6`: optional transcript auto-send, default false, guarded through the normal chat path.
- `2faa8ea`: Fish timestamp SSE wording corrected; windowless Voice/microphone settings capture.
- `88cfbfc` / `7f5ef9f`: fresh compositor cursor input for viewport gaze during passthrough; scheduled-overlay regression.
- `cc1e0b2`: avatar-private shader variants; imported materials remain unchanged and later loads do not compound tuning.
- `44fe22d`: focused SSE sentence requests during LLM streaming; next request waits for network and playback drain. The app-path benchmark shares the scheduler. Word timings reset per request. No SSE prefetch.
- `b79c149`: 18 current-pipeline measurements, all successful with chat workers closed.
- `25519b0`: explicit hosted-Fish “Try HTTP low latency” preset preserving voice/model/opt-in; no automatic profile migration.

Other implemented work includes sentence-streamed speech, HTTP prefetch, bounded Fish workers, Vercel effort validation, scene speech streaming, microphone resampling/VAD hysteresis, facts candidates/manual reflection, local emotion-to-motion mapping, motion suggestions, cel looks, pointer gaze and desktop sit/stand. Consult the matrix for each feature's acceptance level.

## Latency: what the results actually show

`benchmarks/pipeline-sentence-sse-low.json`: 18 successful live rows with workers closed, DeepSeek V4.1 Flash, Baseten-only routing requested, reasoning none, Fish Low latency, three repetitions per combination, current app-path queues at `44fe22d`.

| Fish model | Delivery | Median TTFT (ms) | Median first decoded audio (ms) |
|---|---|---:|---:|
| drama-3 | WebSocket | 354 | 1496 |
| drama-3 | HTTP | 290 | 981 |
| drama-3 | sentence SSE | 804 | 1656 |
| s2.1-pro | WebSocket | 322 | 1479 |
| s2.1-pro | HTTP | 563 | 1270 |
| s2.1-pro | sentence SSE | 547 | 1400 |

HTTP won these small samples; different LLM TTFTs and reply text confound a general transport ranking. Three observed SSE chunk gaps were 723–788 ms; one HTTP-prefetched boundary was 16 ms. Most other rows used one chunk, so this is limited evidence. This is decoded audio with simulated playback drain for HTTP/SSE, **not physical speaker onset**. WebSocket drains quickly, so total decoded completion times cannot rank playback completion. The requested 500–700 ms target remains unmet. Earlier ASR results used prerecorded uploads, not live microphone acceptance.

Historical buffered and earlier full-reply SSE timings remain in `benchmarks/RESULTS.md`. Do not relabel old JSON rows as measurements of current sentence scheduling.

## Safe visual checks

Existing captures include `previews/ww4-wave-retarget.png`, `previews/ww4-wave-retarget-1.5s.png` and `previews/microphone-settings-current.png`. They prove specific rendered states only.

The isolated local fixture `../../work/current-runtime-ui` has no credential file/history, loopback service endpoints, speech/mic disabled, and auto-send off. Inspect it before reuse. A windowless production UI capture can run without a desktop window or audio device:

```bash
CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=../../work/studio-target nice -n 10 cargo run --bin waifu-studio -- \
  --data-dir ../../work/current-runtime-ui \
  --offscreen-capture previews/reviewer-voice.png \
  --capture-size 1600x4096 --screen voice
```

Expected catalog connection failures in that fixture do not demonstrate a production provider failure. Never substitute the user's real data directory for convenience.

For interactive desktop testing, follow the applicable computer-use skill and begin with window inspection. A recent tool refused because the desktop changed during inspection; an earlier guard was unavailable on KDE. **Do not bypass a desktop safety refusal by launching or controlling the app through shell.** Windowless rendering is separate from interactive acceptance.

## Known limitations and deferred proposals

- Alpha-aware mesh picking is not implemented: transparent hair/clothing texels can intercept hits. A proper fix must handle UV transforms, CPU image availability and visible intersections behind transparent geometry.
- Material-only VRM expressions need private material ownership and integration with shader tuning; not implemented as a general feature.
- Native compositor drag release without a bridge remains heuristic.
- Fish cancellation cannot interrupt DNS. Two worker slots bound admission; a third request behind two stalled cancelled connects can wait for a deadline.
- Hyprland behavior and VR are not hardware/runtime accepted. No OpenXR stack/headset is installed here. Do not upgrade to Bevy 0.20 just for VR: the evaluated OpenXR compatibility is Bevy 0.19.
- Original Hikari archive is unrecovered. The recovered reference is WebWaifu4; the directory called `work/waifu4-reference` is actually waifu2x.
- Fly-connectome avatar control, autonomous drives/journaling, a full Grillo port and broad SQLite migration remain proposals. No trained connectome-to-avatar decoder or measured benefit exists.
- Trailing reply affect cannot guarantee emotion at speech onset without delaying speech or adding inference.
- No current physical desktop, microphone, perceived lip-sync or headset acceptance should be inferred from green tests.

## Privacy and review boundaries

Preserve proxy routing and fail closed when a required route is missing. Never print credentials, proxy URLs, saved voice IDs or private conversations. Do not alter proxy environment variables to make a test pass. Keep live calls bounded and record cost/request scope; the user previously authorized bounded benchmarking, not unlimited spending or publication of private data. Do not kill unrelated applications or old processes based only on their names.

Requested deliverable: a concise prioritized defect report, commands/results, evidence levels, and an explicit untested list. If no verified defects remain, say so without converting untested flows into accepted ones.


Newest review queue (after the historical baseline above): optional automatic reflection (`automatic_reflection.rs` and settings/UI wiring), local WAV/MP3 preview, and structured generated reflection provenance in `facts_memory.rs`. Latest integrated full-suite receipt is562 tests; see newest VALIDATION sections. Specifically challenge accepting a draft after a source correction, stale accepted reflection recall, forgetting derived quotes, candidate-retention failures, and legacy unstructured-citation behavior. No fresh Fable review has passed for these slices.
