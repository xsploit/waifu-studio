# Companion Starter attribution

Copyright (c) 2026 VRM Loader contributors.

The three motion assets in this package are original Waifu Studio procedural BVH motions, authored for this application. They contain no third-party motion data. Each asset is explicitly licensed under the MIT License, reproduced in `LICENSE`; retain that copyright and permission notice when redistributing copies or substantial portions.

| Pack asset | Original application asset | License |
|---|---|---|
| `motions/idle-look-down.bvh` | `assets/animations/companion/idle-look-down.bvh` | MIT |
| `motions/talking-nod.bvh` | `assets/animations/companion/talking-nod.bvh` | MIT |
| `motions/talking-tilt.bvh` | `assets/animations/companion/talking-tilt.bvh` | MIT |

The BVHs are byte-for-byte copies of the shipped files. They use upper-body channels without root translation or locomotion; arm channels are absent. They were not generated or altered for this pack.

The manifest and documentation in this package are also MIT licensed. `cozy-room` is a reference to the host application's existing Cozy Room implementation; this package contains no room image, room geometry, or other external asset. The preset contains only the host's typed `idle_motion` setting.
