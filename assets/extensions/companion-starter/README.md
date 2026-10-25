# Companion Starter

An API-v1 content-only pack of three original, MIT-licensed companion motions. Select this folder, containing `extension.json`, in the host's folder-based pack installer. See the application's `docs/content-packs.md` for support and lifecycle details.

Package ID: `org.waifu-studio.companion-starter`

Version: `1.0.0`

| Local ID | Content | Intended use |
|---|---|---|
| `idle-look-down` | BVH, role `idle` | Gentle head glance during idle. |
| `talking-nod` | BVH, role `talking` | Gentle chest/head nod during speech. |
| `talking-tilt` | BVH, role `talking` | Gentle chest/head tilt during speech. |
| `cozy-room` | Built-in room reference | Optional selection of the existing Cozy Room. |
| `enable-idle-motion` | Typed preset | Sets `idle_motion` to `true`; other settings are unspecified. |

Resolved IDs use the package namespace, for example `org.waifu-studio.companion-starter/talking-nod`. Roles describe intended playlist use; they do not themselves force playback. Installing a pack does not automatically apply its preset or select its background.

These motions have no arm channels, locomotion, or root translation. Their sampled endpoint poses match, but the manifest leaves `loopable` unspecified: it makes no guarantee about smooth endpoint velocities, transitions, avatar-specific behavior, collisions, or rendered quality. No gesture/emotion or expression mappings are supplied.

No character model, generated clip, external asset, process, hook, or capability request is included. The room requires the host's existing implementation. Read `ATTRIBUTION.md` and retain `LICENSE` when redistributing.
