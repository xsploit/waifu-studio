# Waifumations: clipping-free Kimodo subset

11 user-generated Kimodo motions, kept from the original 167-clip Linux animation library because they showed no arm/body clipping (worst under 1 cm) on Hikari, Neuro-sama and Jim. The other 156 clipped through the body on most avatars and were removed at the project owner's request on 2026-11-11; they remain in git history. These are the original VRM-renamed BVH files, with their generation prompts and SHA-256 hashes in `manifest.json`. `tools/kimodo_to_vrm_bvh.py` documents the name conversion.

The kept clips are three idles, one talking clip and seven reactions/greetings. Automatic companion playback uses the idle/talking categories.

These are experimental motions, not collision-corrected or visually accepted for every avatar. Hikari can show arm/body/clothing intersections. Rejected clearance-solver and prompt-trial outputs are not included; see `reviews/waifumations-arm-clearance.md` for the historical findings. No generated weights or Kimodo runtime are bundled.
