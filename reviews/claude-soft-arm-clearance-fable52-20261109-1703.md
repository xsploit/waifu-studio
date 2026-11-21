Read-only review, by source and saved evidence only: I had no shell, so no tests, builds or captures were run. Of Codex's three round-3 findings, one is confirmed as a real medium defect, one as real but low, and one is correct logic with an unproven trigger.

### F1 - VERIFIED, medium: Interrupted crossfade applies the soft correction twice (confirms Codex round 3 #1)
- where: `src/animation.rs:1740` (snapshot), `src/animation.rs:1751-1757`, `src/animation.rs:1905`, `src/animation_soft_clearance.rs:351`
- trigger / reproduction: Soft mode on, a pose with a visible correction showing. Select clip B, then clip C before B's 0.5 s blend ends.
- expected vs actual: Expected the arm to stay continuous across the switch. Actual: because a transition is already active, `outgoing` is `None` and the blend starts from `transition.from`, a snapshot of live transforms that already contains the upper-arm correction. `soft_arms.applied` is not cleared, so `correct_arms_soft` rotates that snapshot again. The first frame shows roughly double the correction (up to about 14° extra in one frame), bypassing the rate limit. A single uninterrupted switch is fine, because it blends from the re-sampled outgoing clip.
- evidence tier: read code
- smallest correct fix: When the snapshot is taken from visible transforms (`previous.is_some()`), call `cache.soft_arms.reset()`. The shown pose equals the snapshot, so zeroing the state keeps continuity. Add an interrupted-transition lifecycle test.

### F2 - VERIFIED, medium: Acceptance evidence does not cover the code under review, and the gate is looser than the contract
- where: `.taskgraph/STATE.md:9,44,63,68`; `work/soft-clearance-eval/metrics.py:63`; `work/soft-clearance-eval/sweep/ranking.json`
- trigger / reproduction: Compare the contract's "Done when" with the recorded evidence.
- expected vs actual:
  - **Stale binary:** the 167-clip sweep is recorded as "binary before round-2 fixes". The walk capture precedes fix-review-1 and fix-review-2, which changed the strict point set and the proxy end bands. Codex only re-ran the script on old captures.
  - **Threshold drift:** the contract says per-frame change ≤ 1.5 cm; `metrics.py` passes at ≤ 2.0 cm. The measured 1.13 cm passes either, but the gate no longer encodes the contract.
  - **Displacement checked on one clip:** the ≤ 12 cm hand displacement bar was measured on the walk clip only (5.9 cm). In the sweep, 64 of 167 clips reach the 13.8° cap, where a 0.5 m shoulder-to-wrist reach moves the hand about 12 cm.
  - **Self-graded intrusion:** every intrusion number is the solver's own proxy depth (`report.before/after`), not an independent measurement.
- evidence tier: read code (plus the saved logs and ranking)
- smallest correct fix: Re-run the walk capture and sweep on the current binary, restore the 1.5 cm threshold or amend the contract, and compute hand displacement per clip in the sweep.

### F3 - HYPOTHESIS, medium: Partial first mesh capture latches as ready (Codex round 3 #2: logic confirmed, trigger unproven)
- where: `src/animation_body.rs:97-106`
- trigger / reproduction: A first scan sees only some of the avatar's skinned meshes and yields at least one strict point; the rest arrive later.
- expected vs actual: Expected a rescan when more meshes load. Actual: `ready` returns true forever once `strict` is non-empty. With fewer than 32 points `BodyProxy::fit` returns `None` and soft mode is silently dead; with more, the proxy is fitted to a partial body.
- evidence tier: read code
- smallest correct fix: Compare the loaded-mesh count on every call, not only when the set is empty, and rescan when it grows. I could not show that this app loads an avatar's meshes in stages, so this stays a hypothesis.

### F4 - HYPOTHESIS, medium: Proxy likely over-reports intrusion, so contact gestures are pushed away and the metric flatters the fix
- where: `src/animation_soft_clearance.rs:159` (depth includes the full sleeve radius), `:113-119` (extents), `:37-47` (one rigid frame); `sweep/ranking.json` summary
- trigger / reproduction: Poses where the arm legitimately touches or rests near the body: hand on hip, self-hug, arm across chest, hands in lap, bows.
- expected vs actual: Expected no correction for visually clean contact. Actual signals:
  - The proxy reports ≥ 2 cm intrusion in 155 of 167 clips before correction.
  - `talking_thinking_01` intrudes on 73 of 73 frames at a mean of 10 cm, which is implausible as real penetration.
  - The clearance margin is the 95th-percentile sleeve radius added to a 96th-percentile ellipse.
  - Normals are horizontal only, so a hand resting on the thighs is pushed sideways.
  - A single hips-to-neck frame misfits bent or twisted torsos.
- evidence tier: read code (plus the saved ranking)
- smallest correct fix: Ground-truth a handful of the top-ranked clips with zoomed off/soft frames before trusting the numbers. If these are false positives, shrink the margin (a fraction of the radius, or a dead-band) rather than tune the cap.

### F5 - VERIFIED, low: A paused pose never retries a failed capture (confirms Codex round 3 #3), and failures latch more widely
- where: `src/animation.rs:1829-1838`, `src/animation.rs:2011`; `src/animation_body.rs:115-118`
- trigger / reproduction: Pause a clip, enable soft mode while mesh or bindpose assets are not yet available.
- expected vs actual: Expected the correction to appear once assets load. Actual: the failed attempt records `last_soft_arm_clearance` and `soft_settling = false`, so the paused early return blocks every later attempt. Separately, `scanned_meshes` is written before the skeleton validity checks, so any non-mesh failure also suppresses retries until the mesh count changes, even while playing.
- evidence tier: read code
- smallest correct fix: Keep a "capture pending" flag that bypasses the early return while soft mode is requested and not ready. Set `scanned_meshes` only after the skeleton checks pass.

### F6 - VERIFIED, low: Soft mode is a silent no-op on humanoids without a Neck bone
- where: `src/animation_body.rs:128-135`; `src/animation_soft_clearance.rs:299`
- trigger / reproduction: Load a VRM 1.0 avatar that omits the optional `neck` bone and enable soft mode.
- expected vs actual: The goal says "any VRM". Actual: capture returns false and `correct_arms_soft` returns `None`, with no log or UI indication. The same silent failure applies when torso skin is weighted to non-humanoid helper bones, leaving fewer than 32 strict points.
- evidence tier: read code
- smallest correct fix: Fall back to Head (or UpperChest/Chest) for the top of the body axis, and log once when soft mode is requested but no proxy can be built.

### F7 - VERIFIED, low: Correction is dropped without a fade when weight reaches zero or the old guard is switched on
- where: `src/animation.rs:1950-1955`
- trigger / reproduction: Soft mode active with a non-zero correction. Lower the weight to exactly 0 with no transition running, or enable `arm_clearance`.
- expected vs actual: Expected release at the 0.45 rad/s rate, as on disable. Actual: `soft_wanted` goes false and `soft_arms.reset()` removes the correction in one frame. How visible it is depends on the remaining angle, which I did not measure.
- evidence tier: read code
- smallest correct fix: Keep the soft pass running while `soft_arms.active()` regardless of weight or old-guard state, passing `enabled = false` so it fades.

### F8 - VERIFIED, low: Batch receipt does not record soft mode
- where: `examples/render_desktop_motion.rs:1142`
- trigger / reproduction: Run a batch sweep with `--arm-clearance soft` and inspect `receipt.json`.
- expected vs actual: Expected the receipt to say which mode produced the frames. Actual: it records only `arm_clearance`, which is false for both off and soft, so sweep outputs are indistinguishable by receipt.
- evidence tier: read code
- smallest correct fix: Add `"soft_arm_clearance"` to the receipt JSON, as the pose JSON at line 986 already does.

## Is the approach sound?
As a bounded mitigation, yes. The target is recomputed from the freshly sampled clip pose each frame, so there is no feedback loop or drift; the rigid shoulder swing keeps elbow and hand pose; the cap and rate limit bound the damage. The rotation composition at `animation_soft_clearance.rs:351` is correct for unscaled parents.

It does not meet the stated goal as written:
- **Not a clipping guarantee:** 76 of 167 clips still show ≥ 2 cm proxy intrusion after correction, and the attack rate lags fast swings by several frames (visible in the walk log around 0.77–0.90 s).
- **Skirts are excluded by design:** yet the cited visual win was a hand no longer hidden in the skirt, so that result is incidental.
- **"Without distorting gestures" is unproven:** 64 clips saturate the 14° cap, and F4 suggests many corrections may be to poses that were not visibly clipping.

Fix F1 and settle F4 with real frames before any discussion of making it the default.

## Untested
- No `cargo test`, build, capture or interactive run; the unit tests' pass status is taken from `STATE.md` only.
- No rendered frames or zoom sheets inspected; F4 rests on numbers and code.
- Only the top of `ranking.json` and the first lines of the walk log were read.
- Codex round 1 and round 2 fixes were read in source, not exercised.
- Systems that run after `animate_humanoid` (springs, look-at, any IK) and may overwrite or compound the upper-arm rotation.
- Non-uniform or negative bone scale, VRM 0.x facing flips, and any avatar other than the Hikari wardrobe.
- Per-frame cost of skinning and fitting up to 6,000 strict points.
- The `ui.rs` and `local_control.rs` changes beyond the grep'd lines.

## Verdict
verified defects: F1, F2, F5, F6, F7, F8
