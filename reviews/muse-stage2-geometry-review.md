# Meta Muse Wonder stage 2 geometry review

Read-only CLI review, 2026-10-20. Configured model `muse-wonder-1.0`; served response metadata: `muse-wonder-1.0-contributor`. Source inspection only; no reviewer tests, shell, GPU or media inspection. Original findings below; parent disposition is in stage2 progress.

## Top finding

No literal `PASS` was found. The false-clear risk is narrower but real: a snapshot can return `coverage_complete=True` and CLI success while none of the intended arm-vs-torso/lower-body comparisons contains geometry.

I inspected source only. I did not run tests, shell commands, builds, GPU work, or inspect media. The production claims about four 180-frame captures, 720 samples, 16 primitives, and 644 Rust tests are supplied evidence in [stage2-progress.md](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/reports/stage2-progress.md:11), not independently verified here. The referenced [full-skin-audit report](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/reports/full-skin-audit.md) was absent when read was attempted, so it was not used.

## P1 — Confirmed false-clear paths

### 1. Empty comparisons are reported as complete coverage

The only comparisons are arms/distal against torso/lower-body in [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:694). Detection and coverage are then computed at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:703): `coverage_complete` only checks unsupported primitives, runtime errors, and extraneous palettes. It does not require either side of any comparison to be non-empty.

Reproducible failure: use the existing hips-only synthetic primitive pattern in [test_full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/test_full_skin_audit.py:168). It has no arm or torso surface, yet the test asserts `coverage_complete` at line 172. The CLI test also expects success from `main()` at [test_full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/test_full_skin_audit.py:337), and `main()` returns success when coverage is marked complete at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:802).

Minimum fix: make `coverage_complete` require every declared comparison to have non-empty source and target surfaces, or add an explicit per-comparison `surface_nonempty` condition folded into coverage. Change the hips-only expectation to incomplete, and add a genuinely populated arm/torso fixture for the positive case.

### 2. Palette joint identity permits same-name joint swaps

Palette validation binds joints only through sequential index, display name, inverse-bind matrix, skin-matrix consistency, and FK agreement in [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:157). Skinning then trusts the palette position directly at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:252).

Reproducible failure: give two source joints the same name or no name and identity inverse binds. Swap their model matrices in the exported palette while also swapping the corresponding skin and FK matrices. Name, bind, `model @ bind == skin`, and FK checks all remain internally consistent, but vertices weighted to joint 0 receive joint 1’s transform.

The existing stale/foreign controls only corrupt a name, index, bind, or skin value in [test_full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/test_full_skin_audit.py:120); they do not test a coherent duplicate-name swap.

Minimum fix: export and require the source node index for every palette joint, equal to `p.joint_nodes[i]`. Until that exists, reject a palette whose source joint names are duplicated or missing. Add the coherent-swap negative control.

### 3. Morph-target identity is count-only

Source targets are retained as an ordered list at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:124). `deform()` checks only morph-weight count and finiteness before applying them in source order at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:241).

Reproducible failure: use two targets with orthogonal deltas, for example +X and +Z. Supply weights corresponding to the opposite exporter order. The count check passes while the auditor skins the wrong morphed geometry. This is a format-evidence gap; it does not establish that the current Rust exporter actually permutes weights.

The only morph control uses one target at [test_full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/test_full_skin_audit.py:85), so permutation is untestable in the current snapshot format.

Minimum fix: export per-target identity, such as primitive-local target ordinal plus POSITION accessor index or hash, and verify it in the auditor. Add a two-target permutation control. If exporter ordering is contractual, state and test that contract explicitly.

### 4. Coordinate-convention evidence is optional

The skin-matrix convention is checked only when `skin_matrix_order` is present, and avatar SHA is checked only when present, at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:680). Neither absence affects `coverage_complete`; the distinction is only reported descriptively at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:705). Meanwhile `deform()` directly applies the exported palette without validating mesh-node scope at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:251).

Reproducible acceptance gap: remove `skin_matrix_order` and `avatar_sha256` from an otherwise matched snapshot. The auditor still marks coverage complete. If the exporter’s palette convention changes, or a relocated avatar differs, the auditor has no mandatory evidence tying the palette to the inspected source convention.

Minimum fix: require the exact recognized `skin_matrix_order` value for complete coverage. Require `avatar_sha256` for complete coverage whenever `--avatar` relocation is used. Add missing-convention and changed-avatar controls.

## P2 — Coverage can silently omit relevant boundary geometry

Selection uses only the four groups defined at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:28) and the any-corner rule at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:137). Joint labels are inherited from the decoder’s influence grouping in [vrm_collision_profile.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/vrm_collision_profile.py:219), including helper exclusion based partly on names. A triangle dominated by an excluded-helper joint can fall out of every compared group. The resulting unselected count is recorded at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:144), but it does not affect coverage.

Also, there are no arm-vs-arm or within-group self-crossing comparisons. Those omissions may be intentional scope, but they are not represented in `coverage_complete`.

Minimum fix: report unselected triangles by dominant joint label, especially `excluded_helper`, and define whether that category is outside the production acceptance scope. Either add arm-vs-arm/self comparisons or encode them as explicitly unexamined rather than allowing global coverage language.

## P2 — Missing meaningful negative controls

- Same-tree BVH traversal is used for the closed-surface self-contact proof at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:631), but the brute-force BVH test covers only two distinct trees at [test_full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/test_full_skin_audit.py:251). Add a same-tree brute-force pair comparison and a self-intersecting mesh larger than one BVH leaf.

- Coplanar overlap is correctly excluded from strict crossings by [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:403), and the limitation is disclosed in the generated report text at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:755). The existing test only checks triangle-contact primitives at [test_full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/test_full_skin_audit.py:307). Add an `audit_pose` control proving coplanar overlap remains explicitly unknown/non-detected rather than being interpreted downstream as clearance.

- Sampled proximity uses only unique vertices and centroids at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:456), and correctly labels the result non-exact at [full_skin_audit.py](/home/subsect/Documents/Codex/2026-10-18/its-useful-memory-idea-is-separating/work/clearance/full_skin_audit.py:480). Add a long edge-to-edge near-contact control where the true closest points are not vertices or centroids, pinning that the sampled minimum may overestimate.

The strict-crossing, closed-manifold, weld, and containment code is generally fail-closed in the inspected source: open targets become unknown, unsupported primitives poison coverage, and sampled proximity is not labelled a full-surface minimum. The highest-priority correction is therefore not the geometry predicates; it is preventing empty or weakly identified evidence from being summarised as complete coverage.
