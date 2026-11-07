# Architecture cleanup, 2026-11-06

Two refactors were retained. Two subsequent attempts failed their validation gate and were restored from file checkpoints. No push or merge was performed.

The final unmodified-source verification passed **1,275 tests across 52 Windows targets**, the release executable build, formatting and whitespace checks. Two checks of the exact retained file writer passed under Linux. An earlier retained-state suite also hit the Fish fixture timeout before this successful final run; that failed result is preserved in `validation.json`. The intermittent failure's cause remains unresolved.

Branch: `refactor/architecture-knots-2026-11-06`, starting at `4381d91`.

- [Starting wiring diagram](wiring-before.html): four red numbered knots, original file/line references and proposed fixes.
- [Retained wiring diagram](wiring-after.html): green means retained; red means rolled back.
- [Comparison image](comparison.png), with before on the left and after on the right; [HTML version](comparison.html).

The diagrams fit a 1366 by 768 screen. Feature isolation and reset were checked in headless Chrome, and the rendered images were visually inspected. The source map covers `src/**/*.rs`, grouping files by feature and tracing direct source module references. It is an inferred source map, not a runtime call graph. Vendor code, tools, examples and assets are outside this map. Line references describe the starting working tree, including existing local edits.

## Retained changes

1. **Decision evaluation transport**, commit `93b07a4`. Four copies of credential selection, HTTP transport, cancellation checks and response handling now use `src/evaluation.rs`. Each feature retains its question schema and answer validation. Explicit policies preserve appraisal's 1.5-second deadline and the other decisions' 10-second deadline; request deadlines also cover the body. Memory triage now shares the gateway OIDC fallback. The old `recipient_decision::DecisionConfig` import remains compatible through a re-export. Validation: 1,273 tests across all targets and the release executable build passed.
2. **Durable profile and memory writes**, commit `3076f09`. Eight callers share bounded JSON serialization, private sibling temporary files, content flushing, platform-specific replacement and failure cleanup in `src/durable_file.rs`. Callers retain payload validation, limits and directory ownership. Extension index/credential transactions remain separate because their multi-step recovery differs. Validation: 1,232 library, executable and integration tests plus the release executable build passed. Two tests against the exact retained writer source also passed under Linux, including Unix permissions, overwrite, failure retention and temporary-file cleanup.

The source commits change 16 files, adding 435 lines and removing 555, including five new regression tests. All validation used the same working tree with the user's existing changes preserved; this was not a clean isolated checkout.

## Rolled-back attempts

3. **Emotion vocabulary**: the proposed shared `AffectEmotion` catalog was restored in full. Its validation failed at `src/fish_ws.rs:2553`, in `cancel_during_stalled_proxy_connect_closes_tunnel_without_direct_bypass`, waiting for the fixture's CONNECT request. The release build passed. Three isolated reruns of that fixture passed, which does not establish a full-suite pass.
4. **Advanced settings forwarding**: direct calls to `settings_kit::advanced` were restored to the original wrapper. Accompanying cleanup and a Unix relative-filename directory-sync correction were also restored. The same unchanged Fish fixture timed out during this full-suite validation; the release build passed. All 19 Fish WebSocket tests subsequently passed together in isolation, but the failure was also reproduced in the complete compiled library suite. No root cause was established, and no test was skipped or weakened to accept a refactor.

The relative-filename Unix issue remains open: `Path::parent()` can return an empty path for a bare filename, so directory sync may report an error after replacement. The attempted correction passed a separate three-test Linux writer harness before rollback; only the two-test retained-source Linux run is acceptance evidence for this branch.

## Reproduction

Use the repository's pinned Rust 1.95 toolchain. On this machine, prepend `%USERPROFILE%\.cargo\bin` to PATH; the other installed Cargo selects an older compiler. Set `CARGO_TARGET_DIR` to `D:\Waifu Studio\target\windows`.

```text
cargo test --offline --release --locked --all-targets -- --test-threads=1
cargo build --offline --release --locked --bin waifu-studio
cargo fmt --package waifu-studio -- --check
git diff --check
```

Per-attempt logs and rollback checkpoints are in the ignored `work/architecture-review` directory. `validation.json` records the final retained-state results. The original 13-file tracked patch was compared byte-for-byte after the cleanup, and the original untracked files were left in place. The `main` branch remains at the starting commit.
