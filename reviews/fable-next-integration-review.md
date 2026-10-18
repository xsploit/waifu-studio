# Fable 5.1 next-integration review — waifu-studio

Date: 2026-10-18. Reviewed HEAD `74debd5` as requested, plus the root agent's
later commit `6002c64` (RGB correction) on request. Read-only; this file is the
only artifact. Scope: the five commits after the backlog snapshot `a20daed`
(`cd6fcc5` emotion-mapped motion, `04cb63f` memory placement, `0f86b06`
reasoning capabilities + nested dropdown, `920a374` Hyprland bridge, `3d003c9`
Silero cache) and `6002c64`, checked against `FEATURE-STATUS.md` and
`work/integration-backlog.md`. Other agents' uncommitted edits in
`voice`/`studio`/`facts_memory` were not relied on; findings cite committed lines.

Verification, offline, `CARGO_TARGET_DIR=../../work/studio-target`:

```
cargo test --offline --lib -- provider::tests motion_decision composer_tests listen:: silero desktop_platform hyprland data::tests studio::tests reply_affect
  → 59 passed, 0 failed
cargo test --offline --bin waifu-studio -- motion_decision composer_tests visuals color_correction desktop_behavior desktop_settings
  → 35 passed, 0 failed
```

One probe was compiled in a scratch copy of the crate under `../../work/`
(deleted afterwards); its output is quoted in F1. No source edits, credentials,
network, GUI, paid calls or proxy changes.

## Findings

### F1 — VERIFIED, medium: one transient Hyprland poll error un-seats the companion and drops a queued move
`src/desktop_platform/hyprland.rs:425-436` (worker loop). Any error from one
`poll` (a 250 ms socket timeout, a non-JSON reply, "Too many Hyprland
surfaces", or a refused `movewindowpixel`) does `state.snapshot = None`,
`state.own = None`, `state.pending_move = None` and sets `state.error`, so
`snapshot()` (`:452-466`) returns `Err` until the next successful poll ≥50 ms
later. The KWin bridge keeps its last snapshot for `SNAPSHOT_TTL` (1 s,
`src/desktop_platform.rs:24,355`); the Hyprland bridge never uses that grace.

Consequences on the consumer side:
- Seated pose lost. `src/desktop_behavior.rs:406` takes `bridge.snapshot().ok()`;
  with `None` the frame falls back to the monitor-floor `WorkArea(0)` surface
  (`:565-580`), the seat's `SurfaceId` is not in that list, so
  `DesktopInteraction::tick` resets (`src/desktop_interaction.rs:249-256`) and
  `settings.sitting = false` (`src/desktop_behavior.rs:602-604`). One glitch
  frame stands the companion up; nothing re-seats it.
- One-shot moves silently dropped. `request_move` returned `Ok(())` and the
  status reads "Snap requested through the desktop bridge; waiting for the
  compositor position." (`src/desktop_behavior.rs:65`), but the queued target is
  discarded in the same error branch. Explicit snaps (`move_requested`) and the
  stand-up move (`stood_up`, `:607`) are issued once and never retried. Seated
  placement is re-requested every frame (`:607-614`), so only those two paths stay
  wrong.

Reproduction (scratch test appended to `hyprland.rs` tests; fake socket from the
existing `fake_socket_round_trip…` fixture returning `"garbage"` for exactly one
`j/clients` after `request_move(40,50)`):
```
PROBE: move dispatched after transient error = false; clients polls = 21; snapshot ok now = true
```
No `dispatch movewindowpixel` was ever sent although the bridge recovered within
the same second.

Fix (bounded): in the error branch keep the previous `snapshot`/`own` when
`at.elapsed() <= SNAPSHOT_TTL` and only record `error` for status; keep
`pending_move` across errors (it is already validated again in `move_command`
on the next good poll); let `snapshot()` prefer a fresh-enough snapshot over a
transient error. Test: the probe above, asserting the dispatch arrives, plus a
seated-state frame test with one `Err` snapshot in `desktop_behavior`.

### F2 — VERIFIED, low: RGB correction and bloom have no relative order
`src/color_correction.rs:46` registers `apply.in_set(Core3dSystems::PostProcess).before(tonemapping)`.
Bevy 0.19.1 registers bloom identically
(`bevy_post_process-0.19.1/src/bloom/mod.rs:79`:
`bloom.before(tonemapping).in_set(Core3dSystems::PostProcess)`) and chromatic
aberration as `post_processing.after(depth_of_field).before(tonemapping)`
(`effect_stack/mod.rs:170`). None of the three constrain each other, so with
bloom or chromatic aberration enabled the gain/pow pass may run before or after
them depending on schedule topology. `pow(x,1.45)*1.1` is not commutative with
bloom's threshold/blend, so Neon-style presets can render differently between
builds, and the committed preview (`previews/neuro-rgb-correction-preview.png`)
does not pin which order it shows. WW4 has no bloom
(`WebWaifu4/src/lib/vrm/postprocessing.ts:30-43`: render → ColorCorrection →
OutputPass), so the faithful choice is correction first, then bloom, then
tonemapping. Fix: `.before(bevy::post_process::bloom::bloom)` (and before
`post_processing`) on the `apply` system. Not reproduced on GPU; the ambiguity is
read from the two registrations.

Other render points checked and found sound: the target-format guard
(`:103-105`) skips non-HDR cameras instead of binding a mismatched pipeline;
the pipeline is single-sample and writes the post-process destination; the
bind group is rebuilt per frame with the dynamic uniform index; alpha is passed
through unchanged; `VisualPlugin::sync` removes the component when suppressed
or disabled (`src/visuals.rs:370-378, 390`); defaults match WW4
(`pow 1.4/1.45/1.45`, `mul 1.1`) and load disabled; sanitize bounds are tested.
Linear-before-tonemap placement matches WW4's composer order above.

### F3 — VERIFIED, low: Hyprland focus-history sort puts never-focused windows on top
`src/desktop_platform/hyprland.rs:206-215` sorts by `(layer, focusHistoryID)`
ascending and documents 0 as most recent. Hyprland reports `-1` for windows not
in its focus history; those sort above the focused window (0) and above every
other window of the same layer, so a freshly mapped floating window the user
never touched is treated as topmost for seat/overlap decisions. The default
when the field is absent is `i64::MAX` (`:84-86`), i.e. the opposite intent.
Fix: map negative values to `i64::MAX` in the sort key. Offline fixture
`only_visible_workspace_windows_are_stacked…` uses only non-negative IDs, so it
does not catch this. Stacking is documented as approximate; this is a one-line
correction, not a parity claim.

### F4 — VERIFIED, info: Hyprland auto-enable is KDE-only and one helper is dead
`src/desktop_settings.rs:71,121` default and migrate `kwin_window_surfaces` from
`kwin_wayland_session()` only, so on Hyprland the "Automatic · KDE / Hyprland"
choice (`:230,239`) defaults to Disabled and must be chosen by hand.
`desktop_platform::window_surfaces_session` (`src/desktop_platform.rs:450`) was
added for exactly this and has no caller. Consistent with FEATURE-STATUS's
"explicitly enabled Hyprland socket backend", so either wire the helper into the
two defaults or delete it; do not leave both.

### F5 — VERIFIED, info: reasoning capability validation is correct; two edges to know
`src/provider.rs:62-98, 416-460, 807-811, 1043-1049`. Catalog parse, hide-in-UI,
refuse-at-send and never-rewrite all behave as FEATURE-STATUS states; the
gateway's `Minimal` auto-default for GPT-5 is also gated (`:390-394`). Edges:
- The global catalog is overwritten by *every* Vercel `/models` fetch, including
  one whose `generation` the studio later discards as stale
  (`src/studio.rs:1060`). Harmless because all Vercel connections share one
  catalog, but a late response from a previous key can repopulate it after a
  connection edit.
- Only `Some(...)` explicit efforts are checked. `None` (Automatic) with a model
  whose listed set is empty is valid by design.
- Scene chats go through `spawn_chat` → `stream_chat_observed_inner`
  (`:644, 773, 788`), so group replies get the same refusal.
No defect.

### F6 — VERIFIED, info: nested reasoning dropdown closes correctly
`src/ui.rs:2628-2643` keeps the popup open with a persisted bool and
`CloseOnClickOutside`; the inner `ComboBox` (`:2726-2748`) is a menu sub-popup,
and egui 0.36.2 only applies the outer close when no deeper submenu is open
(`egui-0.36.2/src/containers/popup.rs:608-623`). The composer test clicks
"Auto" then "High" and persists `High`
(`ui::composer_tests::composer_picker_changes_real_model_and_preserves_draft`,
passes). One cosmetic point: the hidden-effort filter leaves a saved unsupported
effort selected and visible as the button label while the dropdown no longer
offers it; the red error below explains it. Fine as documented.

### F7 — VERIFIED, info: emotion-mapped motion is sound
`src/motion_decision.rs:373-451, 511-612`; `src/studio.rs:1081-1102`;
`src/ui.rs:1450-1458`. The emotion is forwarded only for the same job and
character, cleared on every new send/Finished/Failed, only after
`apply_reply` succeeded (so a character with affect disabled plays nothing),
and only for an auto-playable path present in the current scan. Cooldown,
loading guard and avatar/character staleness are tested. Two design notes, not
defects: the motion starts on `Finished`, which with sentence-chunked speech is
seconds after audio begins; and one binding per emotion means Idle/Idle2/Idle3
style rotation from WW4's sequencer is not expressible.

### F8 — VERIFIED, info: Silero cache and memory placement
`src/listen.rs:22-49, 364-388, 495-498`; `src/silero.rs:52-56`. One session
per `(model, runtime)`, recurrent state reset per utterance, eviction on
in-recording inference failure, no fallback to a stale model after a failed
replacement; the cache mutex is held for the whole capture but `start()` refuses
while busy, so there is no contention, and it is released before `transcribe`.
`src/studio.rs:966-991` now places the three recall blocks after the stable
history prefix and immediately before the latest user turn; the fixture test
asserts the order. Both match FEATURE-STATUS.

## Backlog disposition at this HEAD

Done since `a20daed` (backlog rows now stale): P2′ reasoning validation; Defect 5
pool idle timeout (90 s, `src/provider.rs:710`); Silero session reuse;
`trailing_silence_ms` presets (default already 1200, `src/listen.rs:114`);
memory-block placement; non-KWin backend (Hyprland only); R4 emotion map
(committed); WW4 RGB correction (`6002c64`, replaces the "custom fullscreen pass"
row partially: RGB only, no vignette/scanline/grain).

Still open and worth doing: F1–F3 above; speech-start motion trigger; §3.5
grab-release timeout; §4.3 FBX hips parent rotation; M1–M3 facts recall quality
and expired-record pruning; R3 scene replies speaking while streaming.

Proposals that do not fit this app, and why:
- **Multiple draggable mascots (homunculus §2).** The desktop overlay is one
  transparent window with one `AvatarLoadState.active`; each extra mascot would
  need its own compositor window, hit-test and bridge identity. The multi-avatar
  scene already covers the group case inside one window.
- **Alpha-masked texel grab hits (§2).** Requires a CPU readback or per-triangle
  texture sampling on every pointer sample; the 500 ms mesh-margin hit test is
  adequate for a companion and the cost lands on the render thread.
- **WW4 playlist telemetry `animationReason` / purpose weights.** Telemetry
  fields exist in WW4 for its React inspector; there is no consumer here. The
  purpose weights only matter once tag-based rotation exists; until then they
  are dead data. Fear/disgust labels are cheap but need expression mappings
  first.
- **Prewarm Fish WS on composer focus (S6).** Opens an authenticated session
  per focus; Fish closes idle sessions and the bench shows the WS-ready cost is a
  minority of the ~1.1 s Send→audio. Worth measuring once, not shipping blind.
- **Streaming/realtime ASR transport.** No client exists, needs paid live calls
  to accept, and the current transcript-to-composer review step is the product
  decision; a full-duplex path is a different feature.
- **SQLite/FTS memory, T800 recall-graph inspector, connectome sidecar, VR.**
  Already deferred for the right reasons (256-record cap, no license, research
  only).

## Hikari source vs WebWaifu4 source

- **WebWaifu4 source is present locally** at
  `/home/subsect/Documents/Codex/2026-10-11/files-pasted-by-the-user-review-2/work/WebWaifu4`
  (confirmed: `src/lib/grillo/`, `src/lib/vrm/postprocessing.ts` read for F2).
  This is the reference for emotion labels, sequencer and postprocessing.
- **The Hikari application source is not available locally.** The recovery
  manifest `../../work/hikari-recovery-manifest.json` lists its origin as
  `Documents/Codex/2026-09-22`, which no longer exists, and the 3.18 GB
  `codex-hikari-application.tar.zst` only as Drive parts. `~/Work/jev-selfbot/src/hikari`
  exists but is the Discord VAD/transcriber/Fish-stream slice, not the
  animation or emotion code. `../../work/waifu4-reference` is waifu2x (Lua image
  upscaler), not WebWaifu4. Any "port the Hikari emotion/animation source" item
  is therefore blocked on extracting the archive; everything emotion-related in
  this repo is correctly attributed to WW4.

## Doc drift

- `work/integration-backlog.md` §3–§4 predate `0f86b06`/`3d003c9`/`920a374`
  (see disposition above).
- `FEATURE-STATUS.md` desktop row says "Hyprland … requires a floating
  companion for moves; its stacking order is approximate" — accurate; add that
  Hyprland tracking stays Disabled by default until chosen (F4).
