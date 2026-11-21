# Hikari wardrobe arm collision investigation

These are diagnostic experiments, not approved animations or a shipped fix. The
experimental arm guard remains off by default. Do not enable it based on these
results: the tested solver visibly distorts gestures and still has unverified
transitions, collision coverage, and performance.

## Target and source

The current wardrobe avatar is `assets/characters/models/81453f3567d8-hikari-wardrobe.vrm`
(SHA-256 `81453f3567d8c57e2153a2aa232931a2e09b537c301ba82b8b8abfe3055ba21a`).
The recovered Linux continuous-physics manifest identifies a different Hikari
avatar, SHA-256 `9bcc6aedeb4b19843faa3c753c7d7cc2fc7061bb07ddfea01260fcc501481b98`.
Clearance on one outfit does not establish clearance on the other.

The Linux `pose-keys-interior-v1` experiment used full-body and hand constraints.
Its seed 108 BVH (SHA-256 `3cb7844ca5f52c55d0767d642b8c2abaa34af9ef05e06d6acd3a14d0b688d8f7`)
was captured on the current avatar with the guard off, at 30 FPS for all 180
frames, from four views with native spring simulation. It holds the arms out
in a shrug-like pose instead of the desired relaxed arms-at-sides idle. It is
not a replacement or an approved clip.

A separate skinned-triangle diagnostic using final render matrices and morph
weights found intersections in the shoulder/armpit region on all 180 frames;
it found none below model-space y=1.44. Some shoulder intersections can be
garment seams or connected surfaces. The diagnostic tests selected triangles,
not every surface, contained geometry, swept motion, or transitions, so zero
reported crossings would not prove the clip collision-free. The previous
Linux experiment used different surfaces and static helper locals; its counts
are not directly comparable.

## Guard experiments

The previous native sampler omitted skirt spring bones under upper-leg bones
and restricted its torso sample height near the hips. A prototype extends the
sample region and traces those bones, uses skinned hand points and limits changes
in its correction. It was compared with the guard disabled for breathing,
hands-behind-back, crossed-arms and thinking motions on this avatar. The first
two seconds were captured at 30 FPS from four views; five frames per clip were
screened against selected skinned triangles.

The conservative prototype reduced some reported crossings but visibly raised
and distorted the arms. Its worst measured hand step was about 19 cm at 30 FPS;
the correction limiter reduced this to about 4–5 cm in the next sampled run,
but does not guarantee clearance during interpolation. The guarded thinking
gesture remained unnatural. Reducing the collision penalty improved its pose
but failed an existing upper-arm crossing regression, so that variant was
rejected. Runtime guard time was roughly 6–7 ms per frame after setup in those
headless captures, excluding first-frame setup. These are not live desktop FPS
measurements.

The source and captured frames have not been rebaked or substituted into the
167-motion library. The work-in-progress guard must not be described as
collision-safe. Further work needs avatar-specific motion constraints, surface
and swept-path verification, believable poses and transitions, and measurement
in the actual desktop runtime before enabling automatic use.
