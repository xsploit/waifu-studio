# Waifumations: original Kimodo library

167 user-generated Kimodo motions recovered from the original Linux animation library and included at the project owner's request. These are the original VRM-renamed BVH files, with their generation prompts and SHA-256 hashes in `manifest.json`. `tools/kimodo_to_vrm_bvh.py` documents the name conversion.

The library includes idle, talking, reactions, greetings, dance and locomotion clips. Automatic companion playback uses idle/talking categories; automatic reply gestures exclude locomotion. Walking clips remain available for manual preview.

These are experimental motions, not collision-corrected or visually accepted for every avatar. Hikari can show arm/body/clothing intersections. Rejected clearance-solver and prompt-trial outputs are not included; see `reviews/waifumations-arm-clearance.md` for the historical findings. No generated weights or Kimodo runtime are bundled.
