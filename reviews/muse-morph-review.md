## Verdict

Cannot be done safely with a small app-only patch in Bevy 0.19.1. A safe version requires coordinated changes in the app plus vendored `bevy_shader_mtoon`/`bevy_vrm`, and one explicit decision on device-recreation. Clearing morphs without that coordination either breaks morph rendering on the next re-extraction or panics on extracted meshes.

Baseline note: use the corrected no-avatar figures (387.5 MiB RSS / 354.6 MiB PSS) for attribution; the 88.08 MB is heaptrack *live heap*, so the RSS saving will be smaller and allocator-dependent.

## Why the obvious moves fail

- No per-field extraction exists. In [bevy_mesh-0.19.1 `mesh.rs`](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_mesh-0.19.1/src/mesh.rs:2289), `Mesh::take_gpu_data()` extracts attributes, indices, `morph_targets`, and `morph_target_names` atomically; each field lives in a private `MeshExtractableData<T>` (line 238). There is no "extract morphs only."
- The current retention is by configuration, not a leak: [loading.rs](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/loading.rs:156) sets only `load_materials = RENDER_WORLD`; `load_meshes` stays at the default (both worlds), so [render_asset.rs](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_render-0.19.1/src/render_asset.rs:336) *clones* each mesh into the render world and the main-world copy — including the 88.08 MB `Vec<MorphAttributes>` built by the glTF loader's `collect()` — is retained forever.
- Clearing morphs directly (`set_morph_targets(vec![])`, or untracked mutation) is a tracked `Modified` asset event → re-extraction → the GPU pipeline key loses `MORPH_TARGETS`, the morph slab allocation is freed, and expressions keep writing weights to nothing. Untracked clearing defers the same corruption to device recreation, when `collect_render_assets_to_reextract` ([render_asset.rs:280](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_render-0.19.1/src/render_asset.rs:280)) re-extracts from a morphless main-world mesh. Do not ship this, as instructed.
- Setting `load_meshes = RENDER_WORLD` alone is also unsafe. It frees *all* CPU geometry, not just morphs, and post-extraction accessors panic (`ExtractedToRenderWorld`), so the three lazy CPU consumers break:
  - Outline regeneration: [mtoon `smooth_outline_mesh`](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/vendor/crates/bevy_shader_mtoon/src/lib.rs:298) reads position/normal from the source mesh and clones it (morphs included, line 338). The cache is weak (`Weak<OutlineMeshHandle>`, line 141) and the child is despawned when `outline_mode == None` (line 220), so regeneration after unlit/outline-off genuinely re-reads the source.
  - First person: [first_person.rs `strip_head_vertices`](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/vendor/crates/bevy_vrm/src/first_person.rs:160) clones the whole source mesh (line 188) on the `SetupFirstPerson` event; a lazy trigger after extraction panics.
  - Picking: `MeshRayCast` reads positions/indices from `Assets<Mesh>` in the main world. However, Bevy provides `SimplifiedMesh`, which the ray caster *prefers* over `Mesh3d` ([ray_cast/mod.rs:115, 202](/home/subsect/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_picking-0.19.1/src/mesh_picking/ray_cast/mod.rs:115)) — this is the sanctioned escape hatch. Nothing in `src/` inserts one today.

## The feature-preserving design (app + vendored patches, no engine fork)

Do all CPU-dependent derivation *eagerly* at load finalization, then flip the source meshes to `RENDER_WORLD` so extraction itself moves the morphs out — no untracked clearing anywhere.

1. **On avatar ready, per avatar `Mesh3d` (before any flip):**
   - Build a picking proxy: new `Mesh`, `MAIN_WORLD`, same topology, only `ATTRIBUTE_POSITION` + indices copied. This is exact-geometry picking with parity to today (current picking already ignores morph displacement). Insert as `SimplifiedMesh` on the entity. Cost ≈ 12 B/vertex + indices, versus 48 B/vertex/target for morphs.
   - Pre-generate outline and first-person meshes (step 2/3) while the source is intact.
   - Only then set `mesh.asset_usage = RenderAssetUsages::RENDER_WORLD` via a tracked `get_mut` to force one re-extraction. Extraction moves morphs into the morph slab/texture exactly as today; the main-world mesh becomes an extracted shell that retains `final_aabb`, and the glTF `Aabb` component used by [avatar_preview.rs](/home/subsect/Documents/Codex/2026-10-17/ma/outputs/waifu-studio/src/avatar_preview.rs:123)/runtime framing is untouched.
2. **Vendored mtoon:** generate each outline mesh eagerly during finalization and hold a **strong** handle in a cache keyed by source `AssetId<Mesh>`, invalidated on avatar unload. `sync_outline` then never calls `smooth_outline_mesh` on a live source again; mode-`None` toggles visibility/reuses the cached child instead of despawn → expire → regenerate. The outline asset itself stays `RENDER_WORLD` (already the case, line 339), so its duplicated morphs are also extracted, not retained on CPU.
3. **Vendored vrm:** fire `SetupFirstPerson` splitting eagerly at load (or materialize the split variant during finalization), with the new variant's mesh `RENDER_WORLD`. A later lazy event then finds the split already done and only assigns `RenderLayers`.
4. **Device recreation / re-upload — pick one, explicitly:**
   - (a) *Avatar reload on recovery:* detect device loss in the main world and re-issue `LoadAvatar` from `AvatarLoadState::requested_path`. Simple, fully correct, heavyweight and only on a rare path. This is the recommended option.
   - (b) *Compressed spill:* before the flip, serialize each mesh's `MorphAttributes` (POD, flate2 is already a dependency) to a per-avatar temp file with an index of (AssetId, offset, vertex/target counts, names); a render-app `ExtractSchedule` system ordered before `AssetExtractionSystems` restores morphs from spill before any re-extraction. Zero RAM, but this is the invasive option and must also cover user-triggered modifications.
   - (c) Accept that `RENDER_WORLD` meshes cannot re-upload after device recreation. Only acceptable if stated as a known degradation — the app already accepts this semantics for materials.

## Risks and residual costs

- Ordering race: extraction runs in the render sub-app's `ExtractSchedule`; finalization must complete in the load frame before the tracked flip, and the eager outline/first-person generation must be inserted before the vendored `sync_outline` can run lazily against an extracted source. Any missed call site that touches `.attribute()`/`.morph_targets()` post-flip is a panic, not a silent miss — audit all `meshes.get(...)` sites in both vendored crates.
- Transient peak: eager outline + first-person clones each duplicate morphs once before their own extraction; peak RAM during load can briefly exceed today's. Sequence per-mesh (derive → flip → next) to bound it.
- GPU memory is unchanged: the morph slab/texture, plus the existing duplicated morph data on outline and first-person variants, remain on GPU. This fix targets CPU RAM only.
- Expected saving: ≈88 MB live heap (avatar loaded only), minus the small position+index proxies; RSS/PSS recovery will be less than 88 MB and should be re-measured with heaptrack, not assumed.
- Expected saving: ≈88 MB live heap (avatar loaded only), minus the small position+index proxies; RSS/PSS recovery will be less than 88 MB and should be re-measured with heaptrack, not assumed.
- Maintenance: steps 2–3 patch vendored crates (acceptable, already vendored); the engine-fork alternative (per-field extraction in `bevy_mesh`) was rejected as disproportionate for one Bevy pin.

## Bottom line

Freeing the 88 MB is achievable and worthwhile, but only as the coordinated change above. If the scope is strictly "small app-only patch," the correct conclusion is: **do not attempt it** — every app-only variant either corrupts morphs on re-extraction, panics on lazy outline/first-person regeneration, or silently loses morphs on device recreation.
