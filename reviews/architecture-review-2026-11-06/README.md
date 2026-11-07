# Waifu Studio architecture cleanup, completed

All four original findings are fixed. Follow-up investigation also fixed the real Windows speech-cancellation defect that had caused the earlier validation failures. Work is on `refactor/architecture-knots-2026-11-06`; nothing was pushed or merged.

- [Starting wiring map](wiring-before.html), with numbered red findings and original file/line locations.
- [Final wiring map](wiring-after.html), with all five retained fixes in green.
- [Before/after comparison image](comparison.png) and [HTML version](comparison.html).
- [Validation receipt](validation.json) and [initial checkpoint history](INITIAL-CHECKPOINT.md).

The maps fit 1366 by 768 and 1920 by 1080 viewports. They group all source Rust files by feature and infer direct module references from source; they are wiring sketches, not compiler or runtime call graphs. Assets, tools, examples and vendored crates are outside the map. The starting source is reconstructed from `4381d91` plus the preserved original local edits. Finding 5 was discovered during follow-up. File/line locations refer to that starting source. Conditional test imports do not truncate the production source being traced.

## Changes retained

| Finding | Result | Validation after this change |
|---|---|---|
| 1. Decision evaluation | Four transport implementations now use `evaluation.rs`; feature schemas remain at their callers. Credentials, OIDC fallback, HTTP, deadlines and size limits have one owner. | 1,273 tests + release build passed |
| 2. Profile and memory writes | Eight writers use bounded serialization, private temporary files, flush, replacement and cleanup in `durable_file.rs`. Callers keep validation and directory ownership. Extension transactions remain separate. | 1,232 library, binary and integration tests + release build passed |
| 3. Emotion vocabulary | `AffectEmotion::ALL` and `as_str` supply appraisal, motion and settings. Picker order and serialized names are preserved. `abstain` remains an appraisal-only choice. | 1,276 tests across 52 targets + release build passed |
| 4. Settings forwarding | All four menus call `settings_kit::advanced` directly. Removed the redundant agent-save closure and stale helper wording. Fixed Unix directory sync for bare relative filenames. | 1,277 tests across 52 targets + release build passed |
| 5. Windows cancellation | Worker I/O and cancellation share `Arc<TcpStream>` through `SharedTcpStream`, avoiding a duplicated socket handle. The original cancellation test now runs 64 cycles with the same latency assertions. | 1,275 tests across 52 targets + release build passed |

Final verification: **1,277 Windows tests across 52 targets and the release executable build passed**. Formatting and whitespace checks passed. Linux acceptance covers three tests against the exact writer source, including relative filenames, permissions and failed replacement, plus 5,000 standalone shared-socket cancellation cycles. This is not a full Linux application build or a live provider acceptance test.

## What caused the earlier failure

The failure was at the peer-close wait after cancellation, not at connection setup. Instrumentation captured Winsock error `10057` (`NotConnected`) from shutdown while the worker was still blocked. The original result was reproduced outside Waifu Studio with just the standard library: timed connect, `try_clone`, then shutdown through the cloned handle. One recorded run failed on trial 15; an earlier run failed on trial 6. A blocking-connect control passed 500 cycles. Sharing the same socket handle passed 5,000 cycles on Windows and 5,000 on Linux. These results identify the failing handle-duplication path on this Windows runtime; they do not establish which upstream component is responsible.

The production fix uses shared ownership instead of socket duplication. `tunnel` accepts a read/write stream, so the same HTTP CONNECT implementation still serves native speech and observation connections. TLS verification, proxy routing, cancellation generations, connection budgets and worker limits are preserved. No timeout or cancellation assertion was relaxed.

The distinction between socket duplication and shared ownership is described in the [Rust `TcpStream` documentation](https://doc.rust-lang.org/stable/std/net/struct.TcpStream.html#method.try_clone); Microsoft's [shutdown documentation](https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-shutdown) describes the error and shutdown semantics. The evidence here comes from local reproduction, not an assumption that every Windows installation behaves identically.

Evidence: [failing reproducer](evidence/duplicated_shutdown_repro.rs), [shared-handle reproducer](evidence/shared_shutdown_repro.rs), [captured application failure](evidence/cancellation-failure.txt), [Windows original result](evidence/duplicated-windows.txt), [Windows shared result](evidence/shared-windows.txt), [Linux shared result](evidence/shared-linux.txt).

## Reproduction and scope

Use the pinned Rust 1.95 toolchain. On this machine, prepend `%USERPROFILE%\.cargo\bin` to PATH and set `CARGO_TARGET_DIR` to `D:\Waifu Studio\target\windows`.

```text
cargo test --offline --release --locked --all-targets -- --test-threads=1
cargo build --offline --release --locked --bin waifu-studio
cargo fmt --package waifu-studio -- --check
git diff --check
```

Each failed initial attempt was restored before continuing. The complete retries passed after the cancellation fix. Original logs and immutable rollback checkpoints remain in ignored `work/architecture-review`, including `followup-2026-11-06`. `validation.json` retains the initial failed outcomes alongside the successful follow-up gates.

The original 13-file tracked patch is still byte-identical, and original untracked files remain in place. Validation used that working tree, not a clean isolated checkout. The `main` branch remains at `4381d91`. All source and report commits are local.
