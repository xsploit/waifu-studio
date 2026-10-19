# Muse Wonder 1.0: shared CPU morph architecture review

Read-only source review before implementation. No build, runtime, paid calls or source edits. Items labeled hypotheses require parent verification.

## Verdict

Design is sound in principle. No fundamental blocker in GPU extraction or the two local derived-mesh consumers. Main risks are the deduplication system's event behaviour, `Reflect` support for `Arc<...>`, and extraction-state handling — not the `Arc` storage change itself.

Savings arithmetic checks out: `MorphAttributes` is `#[repr(C)]` with 3×(`Vec3` 12 B + `f32` pad) = **48 B** ([morph.rs:132-152](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_mesh-0.19.1/src/morph.rs:132)). 262 144 × 48 = 12 582 912 B per vector; ×7 = 88 080 384 B (~88 MB decimal / 84 MiB). One shared backing frees 6 × 12.58 MB ≈ **75.5 MB**, matching the claim — provided `len` fills `capacity`, otherwise the retained `Arc<Vec>` keeps the donor's full capacity, so picking the smallest-capacity equal donor (or accepting donor capacity) matters.

## Verified issues (read from source)

**Storage change itself — low risk**
- Current field is `morph_targets: MeshExtractableData<Vec<MorphAttributes>>` ([mesh.rs:238](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_mesh-0.19.1/src/mesh.rs:238)); `MeshExtractableData<T>` derives `Debug, Clone, PartialEq, Reflect, Default` ([mesh.rs:45](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_mesh-0.19.1/src/mesh.rs:45)). `Arc<Vec<_>>` satisfies `Clone`/`PartialEq`/`Debug` with value (not pointer) equality, so `Mesh: PartialEq` semantics are preserved.
- Existing public surface to preserve: `morph_targets() -> Option<&Vec<MorphAttributes>>` ([mesh.rs:2407](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_mesh-0.19.1/src/mesh.rs:2407)), `try_morph_targets() -> Result<&Vec<...>>`, `set_morph_targets(Vec<...>)`, `try_set_morph_targets`, `with_morph_targets`. All are implementable over `Arc<Vec>` via deref. There is **no** mutable morph getter anywhere — mutation is only whole-vector replacement through `MeshExtractableData::replace`, so mutation isolation holds as long as the new shared setter takes `Arc<Vec<...>>` by value and no `Arc::get_mut`/`&mut` path is added.
- `get_morph_targets() -> Option<&[MorphAttributes]>` ([mesh.rs:864](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_mesh-0.19.1/src/mesh.rs:864)) is the only slice accessor the renderer uses; it is unaffected.

**GPU extraction — no change, no GPU saving (as claimed)**
- `take_gpu_data()` ([mesh.rs:2289](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_mesh-0.19.1/src/mesh.rs:2289)) just moves the `MeshExtractableData` out; an `Arc` clone there is a refcount bump. The extracted mesh is consumed by `RenderMesh::prepare_asset`, which reads `mesh.morph_targets()` and uploads per-`AssetId` ([bevy_render mesh/mod.rs](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_render-0.19.1/src/mesh/mod.rs:195)), and the slab allocator copies `bytemuck::cast_slice(morph_targets)` per mesh id ([allocator.rs](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_render-0.19.1/src/mesh/allocator.rs:484)). Each outline/first-person asset id gets its own GPU slab regardless of shared CPU backing. Correct to claim zero GPU saving.

**Local consumers — both benefit, neither breaks**
- Outline: `smooth_outline_mesh` does `mesh.clone()` then replaces only normals ([bevy_shader_mtoon lib.rs:338](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/vendor/crates/bevy_shader_mtoon/src/lib.rs:338)). The outline genuinely needs identical per-vertex morph displacements (its child copies `MeshMorphWeights`), so sharing morph CPU backing is correct; position/normal/index data still deep-clones.
- First-person: `strip_head_vertices` clones the mesh and mutates only indices ([first_person.rs:188](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/vendor/crates/bevy_vrm/src/first_person.rs:188)). Vertices are never renumbered, so morph targets (indexed by vertex) stay valid on the shared backing.

**Serialization — no regression**
- `Mesh` itself is not `Serialize`; `SerializedMesh` preserves only topology/attributes/indices and already drops morph data entirely ([mesh.rs:2579-2606](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_mesh-0.19.1/src/mesh.rs:2579)). Nothing to change there.

**no_std — precedent exists, one trap**
- `bevy_mesh` is `extern crate alloc` ([lib.rs:3](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_mesh-0.19.1/src/lib.rs:3)) and already uses `alloc::sync::Arc` in [vertex.rs:1](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_mesh-0.19.1/src/vertex.rs:1). The patch must use `alloc::sync::Arc`, not `std::sync::Arc`.

**Dedup system — the real work, three verified hazards**
1. `Assets::get_mut` emits `AssetEvent::Modified` on drop, and `iter_mut` marks everything visited. A dedup pass that mutates via `get_mut` will re-trigger its own `Modified` handling. It must skip (without `get_mut`) any mesh whose `Arc` already pointer-equals the canonical one, and inspect via `get` first.
2. After extraction with `RENDER_WORLD`-only usage, morph accessors return `Err(ExtractedToRenderWorld)`. Dedup must use the `try_` getters and skip on error — the panicking `morph_targets()` variant would crash on already-extracted meshes depending on system ordering relative to render extraction.
3. The canonical registry must hold `Weak<Vec<MorphAttributes>>`, never a strong `Arc` — a strong cache entry pins 12.6 MB forever and defeats the saving. Weak entries need lazy pruning (e.g. retain-live on each dedup pass); `AssetEvent::Removed`/`Unused` are per-mesh-asset, not per-shared-vector, so they cannot drive cleanup of the shared entry directly.

## Hypotheses (not verified — need a compile/probe)

- **Reflect is the top compile risk.** `Mesh` derives `Reflect` and `morph_targets` has no `#[reflect(ignore)]` (unlike `attributes`), and the workspace enables `bevy_world_serialization` with `register_asset_reflect::<Mesh>()`. I did not confirm that `bevy_reflect` 0.19.1 implements `Reflect` for `Arc<Vec<MorphAttributes>>` in a way the derive accepts. If it does not, the patch needs `#[reflect(ignore, clone)]`-style treatment or a manual impl — this changes scene-serialization behaviour and must be decided explicitly, not discovered by accident.
- **"Exact comparison" is ambiguous.** Slice `==` on `MorphAttributes` uses derived `PartialEq`: it compares pad fields and uses float `==`, so identical-bit `NaN` payloads compare unequal (missed dedup — safe) and `-0.0` vs `0.0` compare equal (merged — harmless, since displacements add to base positions). A byte-wise `cast_slice` comparison would be truly exact. Either is defensible; the choice should be written down.
- **Patch mechanics.** The workspace `Cargo.toml` currently has no `[patch.crates-io]` section. A path patch must keep version `0.19.1` to satisfy `bevy_render`/`bevy_pbr`/`bevy_gltf`, and the registry copy's usable manifest is `Cargo.toml.orig`, not the generated `Cargo.toml`.
- **Peak memory.** Dedup frees duplicates only when it runs; if all seven Hikari meshes load in one frame batch, transient peak remains ~88 MB until the dedup pass. Fine, but the saving is steady-state, not peak.

## Minimal tests

1. `mesh.clone()` shares morph backing (`Arc::ptr_eq` via the new shared getter); `set_morph_targets` on the clone leaves the original's slice unchanged.
2. Round-trips: `Vec` setter → shared getter and `morph_targets()` agree; shared setter → `morph_targets()`/`get_morph_targets()` return equal slices.
3. After `take_gpu_data()`, `try_morph_targets` and the shared `try_` setter return `Err(ExtractedToRenderWorld)` (no panic).
4. Dedup: two meshes with equal-but-distinct vectors end up `Arc::ptr_eq` with `strong_count == 2`; a differing vector is untouched; `NoData` meshes are skipped.
5. Dedup idempotence: running the system twice emits no second `Modified` event and performs no `get_mut` on already-canonical meshes.
6. Cache cleanup: after dropping all meshes sharing a vector, the registry's weak entry fails to upgrade and is pruned on the next pass.
7. GPU-path regression: `bytemuck::cast_slice(get_morph_targets())` bytes are identical before and after clone + dedup.
8. Reflect gate: `register_asset_reflect::<Mesh>()` compiles and a reflect-cloned mesh preserves morph values — this is the test that settles the main hypothesis above.

Scope note: I inspected the registry `bevy_mesh`/`bevy_render`/`bevy_asset` sources and the local outline/first-person consumers only; Opus's in-progress patch was not present in the tree (no `[patch]` section yet), so nothing above evaluates their actual diff.
