# Muse Wonder 1.0 implementation review

Read-only review; no builds or runtime claims. Parent verification at review completion: 594 all-target tests pass, strict Clippy clean.

## Top finding

No RAM-sharing correctness defect found. Byte-identical morph buffers are safely canonicalized, GPU bytes are provably unchanged, and clone mutation is isolated. Remaining items are coverage gaps and one pre-existing hot-reload hypothesis in the outline cache.

## Verified findings

**Vendor patch is minimal and exact.** Diffing [mesh.rs](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/vendor/crates/bevy_mesh/src/mesh.rs:242) against registry `bevy_mesh` 0.19.1 shows only: `Vec<MorphAttributes>` → `Arc<Vec<MorphAttributes>>`, `Arc::new` in `try_set_morph_targets`, `Arc::as_ref` in the two getters, the two new shared-backing APIs, plus import/format noise. [Cargo.toml](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/Cargo.toml:46) patches `bevy_mesh` to the vendor path; the vendor manifest only adds a standalone `[workspace]`.

**GPU extraction is unchanged.** `take_gpu_data` moves the `Arc` (no Vec copy) at [mesh.rs:2299](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/vendor/crates/bevy_mesh/src/mesh.rs:2299), and upstream `bevy_render` morph upload copies `bytemuck::cast_slice(morph_targets)` bytes. [share_morphs](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/morph_sharing.rs:57) only swaps backing when the full `u8` slices are byte-equal, so uploaded bytes cannot change. Untracked replacement at [morph_sharing.rs:102](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/morph_sharing.rs:102) correctly avoids a spurious `Modified` event / re-upload.

**Mutation isolation holds.** There is no `&mut Vec<MorphAttributes>` accessor anywhere; setters replace the whole `Arc`. Outline derivation clones then only replaces normals, and first-person derivation clones then only rewrites indices — neither can write through the shared morph `Arc`. Both derived meshes emit `Added` events and get canonicalized on the next `Last` pass, so the saving applies to them with one frame of transient duplication.

**Event order is correct.** `share_morphs` runs in `Last` ([morph_sharing.rs:15](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/morph_sharing.rs:15)), after `AssetEventSystems` in `PostUpdate` and before render extraction, so canonicalization lands before GPU upload. The test at [morph_sharing.rs:176](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/morph_sharing.rs:176) confirms no `Modified` events leak from the swap.

**Weak cache lifetime is sound.** Buckets hold only `Weak`s ([morph_sharing.rs:21](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/morph_sharing.rs:21)); `prune` drops dead entries on the next eventful frame. Buffers are never kept alive by the cache.

**Signed zero / NaN is conservative, not a bug.** Byte equality treats `-0.0` ≠ `0.0` and distinct NaN payloads as different. Since the GPU consumes raw bytes, this is the only correct choice; the cost is missed dedup, never wrong rendering.

**Reflection caveat has no current app impact.** Per [WAIFU-PATCH.md](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/vendor/crates/bevy_mesh/WAIFU-PATCH.md:18), only generic code reflecting into the private `morph_targets` field would notice. No app code does this (grep across `src/` finds only `morph_sharing` itself), whole-Mesh `reflect_clone` is tested at [morph_sharing.rs:164](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/morph_sharing.rs:164), and `SerializedMesh` already excluded morph data upstream.

## Hypotheses (not verified defects)

- **Stale outline on hot reload:** the outline cache in the vendored `bevy_shader_mtoon` is keyed by `AssetId<Mesh>` only, and `sync_outline` early-continues when a child exists. If a reload mutates a mesh in place under the same ID, the outline clone's normals/morph would go stale. This predates the morph patch and depends on whether glTF reload reuses mesh IDs — unconfirmed.
- **Measurement overstatement:** [cpu_bytes](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/morph_sharing.rs:31) uses `capacity()`, not `len()`, so reported MiB in [runtime.rs](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/runtime.rs:188) can exceed live morph bytes if loader Vecs carry spare capacity. Cosmetic only.

## Test coverage gaps

The four targeted tests (passing per parent; full suite still running, not relied on here) cover clone isolation, exact dedup, weak release, extraction rejection, and event-pass dedup. They do **not** cover: signed-zero/NaN non-merging, dedup through the actual outline and first-person clone paths, `Modified`-driven hot-reload replacement via tracked mutation, or `Removed`-event pruning. Adding the first two would lock in the invariants this change depends on.


Parent clarification: allocation counters intentionally use Vec capacity to count allocated bytes, rather than initialized data length. They are labeled allocation measurements. Added regression for candidate-key collisions, signed zero and NaN payloads after review; verification pending.
