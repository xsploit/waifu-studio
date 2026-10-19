# Local immutable morph backing patch

Upstream: crates.io `bevy_mesh` 0.19.1 (MIT OR Apache-2.0). The normalized
published manifest is retained with a standalone workspace boundary; the
original upstream manifest is included for reference.

`Mesh` stores morph values in `bevy_platform::sync::Arc<Vec<MorphAttributes>>`.
Existing public Vec/slice getters and Vec replacement setters retain their
signatures and extraction errors. Clones share immutable values; replacement
never mutates another clone. Two fallible shared-backing APIs support app-side
deduplication. Geometry, morph names, GPU allocation and uploads are unchanged.

The app canonicalizes byte-identical backing on mesh asset events, with weak
cache entries. Ownership-only substitutions use untracked access because GPU
bytes do not change. CPU values remain available for device re-extraction and
late outline/first-person cloning. It does not clear morph data after upload.

Reflection caveat: the private `morph_targets` field now contains an opaque Arc
instead of a reflectable Vec. Whole-Mesh reflect cloning remains supported;
generic reflection code inspecting that private field's inner vector would
observe a different type. `SerializedMesh` already excludes morph values in
upstream and is unchanged. No app code depends on that private field layout.

When upgrading Bevy, compare this file's description with the source diff and
run the app's morph_sharing regressions, all-target checks and runtime morph,
outline and avatar-switch checks before accepting the upgrade.
