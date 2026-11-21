# Hikari breathing idle: isolated spacing trial

This is a research trial, not an approved animation or a general collision fix.
The running app, animation library, automatic playlists and saved settings were
not changed. The native arm guard was disabled for all renders.

## Method

The current Hikari wardrobe VRM was rendered using the native retargeter and
spring simulation. `idle_breathing_01.bvh` supplies the body motion. Its arm
channels were replaced with the first frame of `talking_calm_01.bvh` (identical
SOMA hierarchy), then the upper-arm Z channels were adjusted outward by ten
degrees. That keeps a static arm pose while the body moves; it is not a newly
generated Kimodo clip, and it sacrifices arm movement. The reproducible script
and candidates are in `work/test-readiness/guard-repair`.

The zero-degree arm-pose transplant visibly improves on the original behind-the-
back placement but shows crossings around the hands and skirt. On the ten-degree
candidate, the selected-surface transverse intersection diagnostic reported no
crossings below model-space y=1.30 at sampled times 0, 2, 4 and 6 seconds. It
still reported hundreds of crossings in upper-arm/torso regions at those times;
some may be garment seams or overlapping attached surfaces. This diagnostic does
not test every surface, contained meshes, swept movement, or transitions. Zero
crossings in its lower region is not proof of collision-free hands or clothing.
At ten degrees the arms look somewhat held away from the body; at twenty degrees
they are visibly more raised. Six- and eight-degree variants also had no
detected crossings below y=1.30 at frame zero, but their remaining times and
transitions have not been checked. The candidate has not been visually reviewed
frame by frame or tested in the live app or across different avatars.

`relaxed-breathing-spread10/index.html` under the trial directory allows manual
review of all 240 quad-view frames captured continuously at 30 FPS with springs.
The exact loop seam and transitions from other clips need separate review. Do
not put this candidate into an automatic playlist without those checks and a
judgment of pose quality.

## Animated variation

A later isolated variation adds a smooth two-degree periodic sway to the arm
spacing, centered at eight degrees, while retaining the breathing body track.
This is a small procedural edit of the existing motion, not a new Kimodo
generation or a talking gesture. Native capture produced 240 continuous
four-view frames with springs. Four independently posed surface checks at 0, 2,
4 and 6 seconds found no crossings below y=1.30 with the selected-surface
diagnostic; it reported 625, 557, 644 and 660 total crossings respectively,
primarily in upper-arm/torso areas. Some may be attached garment surfaces; the
numbers do not classify or exclude all legitimate intersections. The full
sequence, loop seam, transitions and other surfaces remain unapproved.
`relaxed-breathing-sway/index.html` allows stepping through the captured frames.
The arm sway starts and ends at the same added angle, but the inherited body
channels are not identical at the endpoints: the largest direct named-bone
Euler difference is about 3.04 degrees in neck X. This is not a full joint-space
or visual seam test and is another reason not to promote the candidate yet.

## Research and next direction

NVIDIA Kimodo supports full-body keyframes and hand/foot constraints, including
inbetweening (https://research.nvidia.com/labs/sil/projects/kimodo/). Its CLI
accepts constraint files and seeds (https://github.com/nv-tlabs/kimodo). These
are skeleton-space constraints, not a guarantee of collision clearance on
Hikari's clothing. The prior constrained trial was a shrug, not the requested
relaxed idle. A stronger production pipeline would author a target-avatar pose,
translate it to source-space constraints, generate several seeded candidates,
retarget each to the actual outfit, and review the complete clip plus loop and
transition paths. Avatar-specific surface tests must complement visual review.

The VRM animation specification describes converting rotations through the
source and destination rest poses (https://github.com/vrm-c/vrm-specification/blob/master/specification/VRMC_vrm_animation-1.0/how_to_transform_human_pose.md).
The native retargeter already follows that normalization pattern; it does not
account for differences in garment geometry. Renaming SOMA joints alone does
not fix those geometry differences. The specification has separate LookAt gaze
handling (https://github.com/vrm-c/vrm-specification/blob/master/specification/VRMC_vrm_animation-1.0/README.md),
and the current body animation code excludes the eye bones from its pose cache.
