# Opus 5.5 implementation review

Read-only CLI review, no builds or runtime claims. Registry/render-source access was denied to this reviewer; Muse and parent separately inspected them.

I found no correctness defects in the morph-sharing implementation. The concrete problems are gaps in the tests, plus one outline staleness issue that predates this change. Limitation: I couldn't open the registry copy of `bevy_mesh` 0.19.1 (file access outside the repo was denied), so I didn't diff the vendored crate against upstream. I relied on `WAIFU-PATCH.md` and checked the vendored code directly. I also couldn't read the render-side morph upload code, which lives in Bevy's other crates.

## Verified

**Patch wiring**
- `Cargo.toml:45-46` patches `bevy_mesh` to the vendored path, and `Cargo.lock:1208` resolves it to 0.19.1 from that path.

**Immutable sharing and mutation isolation (holds)**
- Morph data is only ever read or swapped whole. The setters (`mesh.rs:2402`, `mesh.rs:2442`) replace the shared buffer; none of the getters gives mutable access to it. A grep of `.morph_targets` in `vendor/crates/bevy_mesh/src` turns up no way to edit one in place, so changing one mesh can never alter another's morph data.
- Calling the setter on a mesh that has already been moved to the render world keeps it marked as moved and returns an error (`mesh.rs:116-123`).

**Event schedule and order (holds)**
- `share_morphs` runs in `Last`. That is after asset events are published and before the render world copies meshes, so the render world receives the already-shared buffer.
- The ownership swap uses `get_mut_untracked`. It emits no `Modified` event, so it can't loop or trigger a re-upload.
- Removal events only prune the cache.
- The `.expect` at `morph_sharing.rs:104` cannot fire: `get` just succeeded on the same mesh in the same system.

**Clone paths (no extra RAM, verified)**
- Outline: `smooth_outline_mesh` (`bevy_shader_mtoon/src/lib.rs:338`) clones the mesh, so it shares the source's morph buffer. Its asset usage is `RENDER_WORLD` (render-world only), so the main-world copy is moved out after upload and drops out of the metric.
- First person: `first_person.rs:188` clones the mesh, so it shares the buffer too.
- In both cases the system's pointer-equality check returns the existing buffer immediately.
- Avatar meshes keep both main-world and render-world copies (`loading.rs:158-163`), so CPU values stay available for re-extraction, picking and late clones.

**Signed zero / NaN / padding (holds)**
- `MorphAttributes` is `repr(C)` and `Pod`, with explicit `pad_a/b/c` fields and no hidden padding.
- Comparison is byte-for-byte, so `0.0` and `-0.0` are never merged, and NaNs merge only when their bits match. This is stricter than `PartialEq`, so the data sent to the GPU never changes.

**Weak cache lifetime (holds)**
- Cache entries are weak references, and every match is upgraded and byte-checked. A dead or stale buffer can never be returned.
- A dead entry costs only a small allocation header until the next prune, which runs on any mesh event frame. The morph data itself is freed when its last strong reference drops.

**Stale reload (holds for morphs)**
- A `Modified` event gets the new buffer re-shared or registered.
- The app enables no `file_watcher` feature, so file hot reload isn't active anyway.

**Reflection change (no current app impact, verified)**
- No app or vendored code registers `Mesh` reflection, uses `ReflectAsset` or remote inspection (BRP), or reads the private field. The only `reflect_clone` is in the test.
- `Mesh` is an asset, so world serialization doesn't cover it.

**Metric is opt-in**
- `cpu_bytes` only runs when `WAIFU_RUNTIME_METRICS` is set (`runtime.rs:62`).

**Outline staleness (verified, predates this change)**
- `OutlineMeshCache` is keyed by the source mesh ID, and `sync_outline` skips any entity that already has an outline child.
- If a source mesh is replaced in place, its outline keeps the old geometry and morphs.
- This is not caused by morph sharing. It only matters if you add hot reload or in-place mesh edits.

## Test coverage gaps
1. **Signed zero / NaN:** no test. Worth adding one asserting `0.0` vs `-0.0` don't share, identical NaN bits do share, and different NaN bits don't.
2. **Hash collisions:** the hash samples only the first, middle and last entries. No test has two buffers that collide on that hash but differ at an unsampled index (e.g. length 5, differing at index 1).
3. **Redundant writes in test 4:** `Assets::add` and `Assets::insert` already publish `Added`/`Modified` through the real `AssetPlugin`, so the manual `write_message` calls duplicate them. The test still passes, but it isn't proving that the system reacts to real asset events. Dropping the manual writes would make it do so.
4. **Test 2's byte check proves nothing:** `shared` is the same buffer as `canonical`, which was byte-checked against `b`, so the assert can't fail.
5. **Not covered:**
   - re-sharing after a mesh is replaced with values identical to another mesh's
   - a render-world-only mesh's first frame (the outline path)
   - an extracted mesh being skipped without panicking
   - the canonical owner being replaced while the other meshes keep the old values

## Hypotheses (not verified)
- **Bucket cost:** morph data is sparse, so the three sampled entries may often be zero. Distinct buffers of the same length would then share a bucket and be byte-compared against each other on each event frame. This is only a CPU cost, and probably small. Hashing a few more sampled indices would cut it.
- **Where the savings come from:** most of the saving is probably the cheap clone itself (outline, first person, and the render-world copy no longer deep-copies morph data). The cross-asset merging mainly pays off during avatar switches, while the old and new models of the same file overlap. The `morph_logical_mib` / `morph_unique_mib` metric would confirm this.


Parent disposition: signed-zero/NaN/candidate-collision regression added. Removed redundant manually injected events from the AssetPlugin test and added re-sharing after replacing the other alias. Verification of these revised tests remains pending. The clone-vs-cross-asset savings attribution is a reviewer hypothesis: Hikari’s seven independently expanded buffers were already verified by accessor analysis and Heaptrack; runtime counters will measure current sharing.

Parent correction: Bevy's MeshPlugin does call `register_asset_reflect::<Mesh>()` (`vendor/crates/bevy_mesh/src/lib.rs:66`). The review's statement that no code registers Mesh reflection is too broad. The patched registration compiles, and whole-Mesh reflected cloning passes its regression; the documented private-field reflection-type change still applies. This review alone does not prove compatibility for generic private-field inspection.
