# Waifumations arm paths and prompt trials

2026-10-19. **The original clearance video was rejected by the user. The replacement pose solver is also unsuitable for library use.** Correction remains experimental, session-only and off by default. No corrected library was published, batch rebaked or accepted. No default playlist or saved user configuration was changed.

## Failure reproduced

The first implementation checked elbows, wrists, fingers and three forearm samples against skeleton-estimated ellipsoids. It omitted upper-arm segments. At 2 seconds in `talking_thinking_01`, Hikari’s supposedly corrected left upper-arm axis enters the skinned torso at fraction 0.3326 and exits at 0.6338. Earlier favourable proxy metrics were insufficient and are withdrawn as evidence of a fix.

The user identified the temporal failure too: arms begin behind the body and travel through it. A clear destination pose does not establish a clear motion path. Independent checks at the failing pose did not establish a quaternion sign/order error; body proportions and clavicle geometry differ between the rigs.

## Replacement solver: still rejected

`animation_body.rs` captures actual torso skinning samples and clothed arm-radius measurements. `animation_clearance.rs` builds 26 mesh support planes and checks complete upper-arm and forearm capsules. Two-bone IK preserves lengths and wrist orientation. Avatar isolation, unsupported transform rejection and transactional rollback are covered by tests.

These remain coarse approximations. Hikari’s measured scalar radii reproduce at about 41.55 mm upper arm and 44.00 mm forearm; the forearm value comes from clothing. Thickness varies along each segment and a scalar can both overestimate and underestimate the garment envelope.

A diagnostic variant using 35% shoulder attachment allowance and wrist exit seeds removed axis crossings at 106 sampled poses, but shifted hands by up to 449 mm and produced a 462 mm wrist step between adjacent 15 FPS samples. **Rejected.** The current code restores a 20% shoulder allowance and removes attraction toward exit seeds. It has numerical coverage but no visual acceptance; do not use the rejected render as validation of this version. It has no full skin or swept collision protection. The UI warns that gestures can distort and the complete movement path is unprotected.

## Explicit arm-position prompt trial

The original manifest explicitly included hands-behind-back idles; several talking prompts left arm placement unspecified. `tools/waifumations-front-trial.json` asks for relaxed arms beside the torso, forward gesture paths, separated hands, and a return along the forward path to the sides.

Three six-second clips were generated locally, seed 107 plus clip index, 100 denoising steps and postprocessing. There were no positional/keyframe constraints. Raw and renamed BVHs are separate from the existing library. Conversion changes names only. Every trial render has experimental arm correction **off**.

Independent checks skin Hikari’s selected torso triangles and test both arm axes at every generated source frame:

| Trial | Source frames | Frames with axis crossings | Result |
| --- | ---: | ---: | --- |
| `idle_sides_trial_01` | 180 | 180 | Rejected: forearms intersect hips |
| `talking_front_trial_01` | 180 | 0 | Better path; surface clipping still fails |
| `shrug_front_trial_01` | 180 | 39 | Rejected: left forearm crosses during return |

Independent body/arm FK agrees with native renderer samples within 0.003 mm. The axis/selected-triangle screen does **not** establish clearance for complete skin, fingers, clothing ornaments, hair, spring motion or interpolated/crossfaded paths. Prompting helps, but target-avatar playback still determines usability.

All twelve capture sequences completed: three clips from front, rear and both sides, with 91 samples per view at 15 FPS. A passing axis screen alone does not promote a clip into the library.

A subsequent triangle/triangle screen of the talking trial found selected arm/hand-versus-torso surface intersections at 70 of 91 native samples, including shoulder/armpit regions. It uses strict edge/triangle crossings in both directions and a spatial broad phase; skinning uses the native humanoid pose and static helper locals, so spring deformation and unselected surfaces remain limitations. This is further evidence against calling the trial collision-free. All three trials remain outside the approved library. A constant outward arm-spread diagnostic reduced intersections at sampled frames but did not eliminate them even at 30 degrees; it was not baked into an animation.

The 12.13-second, four-angle comparison video is `/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/outputs/waifumations-front-prompt-trial.mp4`. It shows the new talking prompt with correction off and explicitly labels the remaining shoulder clipping. It is a review artifact, not a completed fix.

### Lower-body failure at the view change

The user correctly flagged possible hand/thigh clipping just as the views changed. The prior surface screen only selected torso-weighted faces, so it omitted much of the hips/shorts/legs. A follow-up checks the selected arm/hand surfaces against 4,824 hips/leg triangles, including the shorts, using the same native-pose skinning and two-way edge/triangle test. It finds intersections at 20 of 91 samples. Wrist-area geometry intersects the hip/shorts region from about 5.733 seconds through the final pose; the old montage switches views and restarts the motion at about 6.067 seconds. This confirms another clipping failure, not an accepted motion. Static helper locals and selected weight thresholds remain limitations.

`lower-body-audit.json` is alongside the talking side-view captures. Separate review videos keep each pair of cameras fixed for the entire clip, play at half speed, and hold the last frame for two seconds:

- `/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/outputs/waifumations-talking-uncut-front-side.mp4`
- `/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/outputs/waifumations-talking-uncut-rear-side.mp4`

These preserve the full source range, label every source-frame index, and contain no camera change or restart within either file. The source motion and app configuration remain unchanged.

## Evidence and reproduction

All capture data stays under `/run/media/subsect/toshiba/kimodo/waifu-motion-library/clearance-trial`:

- `final`: first failed baseline; preserve the old videos/metrics as failed evidence.
- `whole-arm`: rejected mesh/IK diagnostics.
- `front-gestures`: new raw/renamed BVHs, renders and `source-axis-audit.json`.

Independent scripts and the radius audit are in `/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance`.

Use the existing local `batch_generate.py` with `tools/waifumations-front-trial.json`, seed 107 and a separate output directory. Preserve the existing offline weight/text-encoder environment. Build `render_desktop_motion` offline using one Cargo job and the shared target directory. Render each converted trial with `--arm-clearance off --time 0 --sequence 15 --frames 60 --pose-json PATH`, selecting `front`, `side`, `other-side` and `rear`. `three-quarter` and `--rig-overlay` are also supported. The overlay shows left arm cyan, right arm magenta and torso yellow at actual joint positions.

## Software checks and remaining work

605 offline tests pass: 495 library, 104 binary, 6 integration. New checks cover continuous segment crossings missed by old samples, upper-arm crossings with clear wrists/forearms, and mesh/radius helpers. Existing IK checks cover lengths/reach, unchanged clear poses, orientation, avatar scope, placement and shear rejection. Strict all-target Clippy, package formatting and `git diff --check` pass. The current renderer was built by the test run; the native desktop executable was not rebuilt or launched in this follow-up. These are software checks, not visual acceptance.

Review complete target-avatar playback, including start, return, loop seam and idle/talking/emotion transitions. Reject passing-through motions and large corrections rather than hiding them behind a score. Where prompts cannot reliably give suitable start/end poses, sparse verified pose/hand constraints are the next trial; they have not been tested here. A reusable library needs per-avatar compatibility information before automatic selection. Full surface/swept collision and a temporally stable constrained retargeter remain unfinished.
