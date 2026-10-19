# Fable 5.1 expression gaze review

2026-10-18. Direct Claude CLI, coordinated through Squad. Read-only source review; no builds or live acceptance by reviewer.

## Parent disposition

D1 accepted: authored maps initially received procedural eye micro-rotation, producing near-inert eyes. Parent replaced this with an explicitly virtual angular focus from the same smoothed pointer/ambient target (+/-60 degrees horizontal, +/-40 vertical), minus only this controller’s actual head/neck overlays. Strength now scales mapped output once, rather than distorting the target or authored curve. World-space and animated-head-aware targeting remain unsupported. Added target/head-offset magnitude and post-map strength regressions. Follow-up review and final validation are recorded separately below when available.

Case-variant VRM0 types remain unsupported rather than silently defaulting unknown types to bone; authored missing-field fallback diagnostics are intentional. No physical/visual acceptance is claimed.

## Original review messages

```text
  [2026-10-19T01:17:46Z] fable-gaze -> manager: fable-gaze review of current src/gaze.rs (988 lines) and src/expressions.rs (read fully, incl. tests through line ~3125), cross-checked against local UniVRM (CurveMapper.cs, VRMLookAtBlendShapeApplyer.cs, VRMLookAtBoneApplyer.cs, glTF_VRM_FirstPerson.cs) and three-vrm-core.module.js (VRMLookAtRangeMap, Bone/Expression appliers, _v0ImportDegreeMap). Scope note: I inspected the post-fix source; the two test mismatches I found earlier (curve 0.625 vs 0.5; expression_weights(NaN,INF) expecting [0,0,1,0] while map() returns 0 for non-finite) are already corrected in current lines 2826-2831 and 2881, so I do not count them.
  | 
  | VERIFIED (from source constants, no build):
  | D1 Range-map input magnitude makes authored eyes near-inert. gaze.rs:316-327 produce the procedural eye counter-rotation residual, clamped to +-0.22 rad yaw / +-0.15 rad pitch then scaled by eyes=intensity*eye_motion (default 0.45*0.35=0.16). gaze.rs:529 converts that to degrees (<=12.6 deg * eyes, ~2 deg at defaults) and feeds it as the VRM look-at angle. The VRM maps are authored for the head-relative target angle (inputMaxValue default 90 in both references). Consequences: VRM1/VRM0 bone type with defaults (90->10): output = input*10/90, i.e. authored-bone eye rotation is ~9x smaller than the pre-existing procedural eye-bone overlay (max 1.4 deg * eyes vs 12.6 deg * eyes). Expression type: weight = deg/90 -> <=0.14*eyes, ~0.02 at defaults, visually nothing. Trigger: gaze enabled on any UniVRM/VRoid export (all carry look-at metadata), either mode. Suggest feeding the eye's intended gaze angle relative to the head (pointer/ambient target angle minus the applied head yaw/pitch) into the maps, or at least not multiplying by the 0.22/0.15 micro-clamp when authored maps exist. Math/signs are otherwise right; it is the magnitude/semantics of the input.
  | 
  | CHECKED OK: VRM1 map = outputScale*saturate(deg/inputMax) (expressions.rs:234-246) matches three-vrm; defaults 90 and 1/10 by type match. VRM0 uses xRange/yRange directly with Unity Hermite over t=clamp01(deg/xRange) (250-271), matching UniVRM CurveMapper.Map; default curve [0,0,0,1,1,1,1,0] is linear in Unity, so defaulting is harmless. Expression horizontal uses outer map for both sides, left>0 -> lookLeft, up>0 -> lookUp; bone uses outer for the eye on the looked-at side and inner for the other, pitch via up/down maps (289-345): matches both UniVRM applyers and three-vrm. Anatomical +X=left basis from leg/arm/eye positions and the -pitch flip (gaze.rs:244-252, 420-425) keep signs consistent for VRM0 and VRM1. Manual weight ownership (active slider or selected expression) and overrideLookAt block/blend apply to the four look presets (expressions.rs:954-999). No double drive: frame.eyes is None for Expression/Waiting/Unsupported/zero-defined, apply_gaze skips eye bones, and PreUpdate clear_overlay removes any prior bone overlay. link.input is cleared at the top of sample_gaze every frame and only accepted for the exact active root (1327), so disable, avatar switch and root removal drop look weights the same frame; runtime reset on root change restores baselines. Scheduling Sample -> ExpressionSystems -> AnimationSystems -> Apply -> Propagate is consistent; eye-drive decision uses last frame's published link, giving one frame of Waiting on switch, not a residual.
  | 
  | HYPOTHESES / LOW:
  | H1 expressions.rs:420 VRM0 lookAtTypeName is matched case-sensitively and unknown strings become Unsupported (eyes still). UniVRM parses it ignoreCase and defaults unknown to Bone; three-vrm treats anything but exact BlendShape as Bone. A file with 'bone'/'blendShape' casing gets still eyes here. Decide intentionally.
  | H2 expressions.rs:428/469 VRM0 degree map missing yRange defaults to 1 for BlendShape (three-vrm behaviour); UniVRM's glTF_VRM_DegreeMap default is 10 regardless of type (saturates 10x earlier). Only matters when yRange is absent; UniVRM exports always write it.
  | H3 expressions.rs:380 VRM1 inputMaxValue > 180 is rejected to 90; three-vrm accepts any value >= 0.01. Edge only.
  | H4 gaze.rs:608-628 status: if the anatomical basis is not found, cache.bones is empty, so an AuthoredBone model reports 'rig has no eye bones' instead of the waiting-for-rest-bones cause. Cosmetic.
  | H5 Per-map 'is missing' notes (373, 449) are appended to the gaze status every frame; a VRM1 bone model that omits all four maps shows four notes. Cosmetic.
  | Not inspected: behavior.rs reserved_expression coverage of look names (compose_behavior could add to look morphs if not reserved) - out of my file scope.

```

## Follow-up review availability

A focused direct Fable 5.1 re-review of the D1 correction was attempted with read-only tools. It returned `You've hit your session limit · resets 10:50pm (America/Los_Angeles)` and exited1. No second review pass is claimed. Parent reviewed the corrected local-target semantics and added numerical target/head-offset plus output-strength regressions.
