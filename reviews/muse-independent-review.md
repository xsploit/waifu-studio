# Waifu Studio — independent review at 88cfbfc

Date: 2026-10-18. Reviewer role: read-only source review per `REVIEW-HANDOFF.md`.
Requested baseline: `88cfbfc` ("Keep viewport gaze supplied during desktop click passthrough").
No source files were edited. No paid API requests, no real data directory, no desktop app launch/control, no credential/proxy/voice-ID output.

## Bottom line

**No verified code defects were found at 88cfbfc in the priority matrix areas.**
One verified low-severity documentation drift (stale test count in README) and two hypotheses are listed below. Everything else checked either passed the offline suite or remains explicitly untested (see "Not tested"). Passing offline tests are not converted into interactive/audio/compositor acceptance anywhere in this report.

### Scope caveat: the tree moved during the review (other agents' work, preserved)

- At review start: `HEAD = 88cfbfc`, working tree clean (verified by `git status --porcelain`).
- During verification runs, another agent committed `7f5ef9f` ("Verify compositor gaze through the application schedule": schedule-level gaze test, `src/gaze.rs`, `src/gaze_controls.rs`, `VALIDATION.md`) and left uncommitted work in `src/shaders.rs` + `FEATURE-STATUS.md` (avatar-private material variants, keyed `(Entity, UntypedAssetId)`).
- Those edits were **not** reverted, edited, or relied on as part of the 88cfbfc baseline. The final `cargo test`/`clippy` runs below therefore executed against `88cfbfc + 7f5ef9f + uncommitted shaders WIP`, not a pristine 88cfbfc tree. Counts are reported as observed, with the baseline receipt (499 at 88cfbfc-era code, per VALIDATION.md) kept separate.
- The uncommitted shaders WIP was skimmed only to confirm it did not break the gates (it passed clippy/fmt/tests in the final run, including its two new privacy tests). It is not reviewed as a finding for or against 88cfbfc.

## Commands run and results (one Cargo process at a time, as specified)

All runs from the repository root, `CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=../../work/studio-target nice -n 10`. No `cargo fmt --all`. No target artifacts deleted.

| # | Command | Result |
|---|---|---|
| 1 | `cargo test --all-targets` | **PASS — 503 passed, 0 failed** (406 library, 90 application/controller/UI, 6 integration, 1 benchmark/example; remaining example targets 0 tests). Observed on the moved tree (see caveat). The 88cfbfc-era receipt in VALIDATION.md is 499; `7f5ef9f` documents 500/501 intermediate totals. |
| 2 | `cargo clippy --all-targets -- -D warnings` | **PASS** — no warnings (`Finished dev profile` only). |
| 3 | `cargo fmt --package waifu-studio -- --check` | **PASS** — exit 0, no diff. |
| 4 | `git diff --check` | **PASS** — exit 0 (checked the concurrent WIP diff as found; no whitespace errors introduced by this review, which made no source edits). |

Supporting targeted evidence already in the tree and re-confirmed by the full-suite pass: gaze freshness/invalid/clear tests, transcript auto-send guard tests, armature hips-conversion regressions, scene-recall prefix tests, speech split-marker tests, HTTP prefetch ordering/cancel tests, Fish worker slot tests.

## Verified defects

### V1 — Low (documentation): README test receipt is stale by ~126 tests

- **File:line:** `README.md:241` (Development section).
- **Trigger:** Any reader trusting the README "Latest receipt" line.
- **Reproduction:** Read `README.md:241` ("377 tests pass (315 library, 56 binary, 6 inspection)"), then run command #1 above or read the newest `VALIDATION.md` sections (499 at the auto-send slice; 500/501 after the gaze follow-up).
- **Expected vs actual:** Expected the README receipt to match the current suite size; actual is 377 vs ≥499 verified. `VALIDATION.md:18` correctly labels 377 as an "Earlier baseline", so only the README line is wrong.
- **Evidence tier:** Verified by inspection + verified by run (full-suite count above).
- **Smallest correct fix:** Update `README.md:241` to the current receipt (or delete the numbers and point only at `VALIDATION.md`). No code change.

No other verified defects. In particular, the following historical review leads were checked against current source and are **fixed / not reproducible at 88cfbfc** (do not re-file):

- RGB-correction disable leaving the pass running: `src/color_correction.rs:103` now gates `ViewQuery` with `With<ColorCorrection>`, and `:48` pins `.before(bloom)` before tonemapping.
- Pointer gaze reading the preview camera: `src/gaze.rs:425-431` restricts to active primary-window render targets (Primary or Entity == primary).
- Hyprland transient poll error un-seating/dropping moves and never-focused windows sorting on top: `src/desktop_platform/hyprland.rs:439-445` preserves snapshot/pending move (records only `error`), `snapshot()` honors `SNAPSHOT_TTL` (`:453-468`), and negative `focusHistoryID` maps to `i64::MAX` (`:225-232`).
- Failed pending-candidate write forgotten on reselect: covered by passing tests `failed_pending_write_survives_reselect_until_explicit_retry` and `failed_write_for_unselected_character_surfaces_on_return` in the full run.

## Area-by-area inspection notes (88cfbfc baseline)

- **Listening / studio send (`listen.rs`, `listen_controls.rs`, `studio.rs`):** Auto-send intent is captured at Start (`start_for`, `src/listen.rs:239-243`: requires setting on + empty draft + character), consumed once (`poll_transcript:295-304`), cleared on cancel (`:277-282`), re-checked against the current setting at arrival (`studio.rs:1382-1386`), and discarded on character mismatch (`studio.rs:1376-1378`); character switches cancel in-flight work (`studio.rs:1343-1354`). Enabling mid-recording cannot arm; disabling mid-recording disarms. Busy-chat, occupied-draft, empty-transcript, and memory/model-readiness paths all fall back to review text via the normal `send_chat_with_semantic` path. No defect found. The whitespace-only draft edge (send check uses `trim().is_empty()`, insertion preserves the draft with a newline) preserves user input; not a defect.
- **Lip sync (`lipsync.rs`, `viseme_audio.rs`, `expressions.rs`, `metered_audio.rs`):** RMS default and calibrated MFCC paths, zero-weight manual sliders not reserving channels, stop/silence via meter generations, and speaker-targeted mouths all have passing ECS/fixture tests in the suite. No code defect identified statically. Perceived sync is untested (below).
- **Speech pipeline (`provider.rs`, `fish_ws.rs`, `voice.rs`, `reply_speech.rs`, `scene_chat.rs`, `studio.rs`):** Delta coalescer eager first-phrase phase, `CHAT_WORKERS=2` permit with synchronous failure event, Fish `MAX_WORKERS=2` generation-checked socket slot, HTTP `HTTP_SLOTS=2` prefetch with `TurnOrder` sequencing and generation-filtered `try_recv`, split-marker filtering in `ReplySpeechText`/`ReplyAffectFilter`, stale-job guards in `poll_studio`, and scene `SpeechStream` early-chunk budget were all read and are consistent with their tests. Slot accounting in `VoiceWorker::start_pending` is safe: a claimed `Slot` with no pending request is dropped (and decrements) on return; the `Arc<Slot>` in `spawn` releases only after the download thread exits, as documented. No verified defect.
- **Desktop companion picking/drag (`desktop_behavior.rs`, `runtime.rs`, `desktop_platform*.rs`):** Mesh raycast excludes outline meshes, click-through recovery requires the controls context, drag uses the compositor native grab with bridge/geometry fallbacks, and the new gaze-pointer supply (`track_desktop_pointer:157-180`) clears outside Desktop mode and timestamps from `snapshot.received_at`. No defect found statically. The transparent-texel hit gap is a known limitation, not re-filed.
- **Motion/import retargeting (`animation.rs`, `animation_import.rs`):** Hips deltas are transformed source-parent → model (facing/height) → target-parent via `rest_parents` and `parent_inverse` (`animation.rs:865-885, 942-967`); singular/non-finite target parents (determinant < 1e-12) and >128-deep chains leave hips at rest. Rotated/non-uniformly-scaled synthetic regressions pass. No defect found.
- **Appearance/gaze (`color_correction.rs`, `gaze.rs`, `visuals.rs`, uncommitted `shaders.rs` WIP excluded):** See fixed historical items above. `GazePointer` freshness (500 ms, future/NaN rejected) is tested. No new defect.
- **Memory/scene recall (`recall_terms.rs`, `memory.rs`, `facts_memory.rs`, `scene_chat.rs`):** Shared bounded terms (16 KiB scan, ≤64 distinct, function-word filter, whole-token Latin / CJK substring via `matches`), pinned priority, history-prefix ordering (persona, ≤38 history, recall, latest user) in both focused and scene paths, per-recipient traces capped at 4 with cancel preserving already-observed traces. No defect found.

## Hypotheses (not verified defects — do not treat as failures)

### H1 — Low: Fish WebSocket terminal events can be silently dropped when the mailbox is full

- **File:line:** `src/fish_ws.rs:451-453` (`publish`: `if queue.len() < 8 { queue.push_back(...) }`, no terminal-event priority).
- **Trigger (hypothesized):** More than 8 unpublished `Playing` events accumulate (e.g., UI poll stall during heavy render) before `Finished`/`Failed` is published; the terminal event is dropped, so `FishWsState::poll` never clears `input`/`id` and Studio keeps `voice_busy` set until the next cancel/new speech.
- **Expected vs actual:** Expected terminal events to always be delivered (or oldest `Playing` evicted); actual behavior under a full mailbox is unobserved — no test fills the mailbox to 8 with a pending terminal event.
- **Evidence tier:** Hypothesis (verified by inspection of the cap logic only; `Timing` coalescing mitigates one event class, `Playing` does not coalesce).
- **Smallest correct fix if confirmed:** In `publish`, always accept `Finished`/`Failed` (evict the oldest `Playing`/`Timing` when full), plus a regression test that fills the mailbox and asserts terminal delivery.

### H2 — Low: Desktop gaze pointer may flicker if the KWin bridge cadence exceeds 500 ms

- **File:line:** `src/gaze.rs:29-32` (500 ms freshness) vs `src/desktop_platform.rs:24` (`SNAPSHOT_TTL` = 1 s).
- **Trigger (hypothesized):** KWin script snapshots arriving slower than 500 ms would expire the gaze sample between receipts, falling back to native cursor/ambient and back.
- **Expected vs actual:** Unobserved; Hyprland polls at 50 ms (`hyprland.rs:19`) so it is unaffected. KWin script cadence was not measured (no live compositor run permitted/performed).
- **Evidence tier:** Hypothesis (constants read from source; no runtime measurement).
- **Smallest correct fix if confirmed:** Reuse `SNAPSHOT_TTL` (1 s) for gaze freshness, or timestamp gaze samples at local receipt when the bridge snapshot is otherwise fresh.

## Not tested (explicit)

- Real microphone capture, VAD/ASR endpointing on hardware, and the physical auto-send turn (only generation-tagged offline injections and a loopback fake provider were exercised, in-tree).
- Physical speaker onset, audible quality/crackle, inter-chunk gaps on live Fish deliveries, and perceived lip sync (all measurements stop at decoded audio).
- Live provider calls of any kind (chat, Fish hosted/local, Vercel, OpenRouter, ASR, embeddings, MCP, SSH, coding agents) — none were made; no paid requests.
- Real compositor interaction: KWin/Hyprland window tracking, click-through, drag release, seating/snap on the user's desktop; Hyprland/KWin evidence remains fixture/socket tests only. The desktop app was not launched or controlled.
- Interactive camera/shader behavior, representative/arbitrary VRM rigs, FBX/BVH/VRMA fidelity beyond the two captured Wave poses, and temporal smoothness of retargeted motion.
- Headsets/VR/OpenXR (not installed), macOS/Windows, long-duration leak/performance behavior.
- The uncommitted `shaders.rs` material-privacy WIP as a deliverable (only gate-checked; owned by the concurrent agent).
- Real UI recovery/persistence-failure flows (disk-full, permissions) beyond the in-tree fixture tests.

## Method note

Historical reports in `reviews/` were treated as leads only; each cited finding was re-checked against current source before being listed as fixed or open. The newest `VALIDATION.md` sections and `benchmarks/RESULTS.md` were read as receipts, not re-run; no benchmark was re-executed and no live latency claim is endorsed beyond what those files state (500–700 ms target unmet; decoded-audio timing only).
