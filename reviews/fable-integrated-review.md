# Fable 5.1 integrated code review — waifu-studio

Date: 2026-10-18. HEAD `018cebe`. Reviewed commits `759730b`, `c3a96a2`, `018cebe`
against base `26e52f5`. Scope: pending-candidate persistence (restart, outbox,
accept/dismiss/delete, stale generations, privacy, expiry), Fish bounded
cancellation admission and socket ownership, motion catalog / category UI,
recall mode, shared decision client. Read-only; this file is the only artifact.

Verification (offline, `CARGO_TARGET_DIR=../../work/studio-target`):
`cargo test --offline --lib -- facts_memory::tests memory::tests fish_ws::tests motion_library::tests composer_tests recipient_decision`
→ 47 passed, 0 failed. Two probes were run against a scratch copy of the crate
(outside the repo, deleted afterwards); results are cited inline. The previously
failing `mutations_during_saves_and_stale_events_lose_nothing` now passes.
Withdrawn F3 from the previous report was not re-examined: `scene_chat` still
builds its own `MemoryState`, so no new evidence.

No source edits, credentials, network, GUI or paid calls.

## Findings

### F1 — VERIFIED, low: a failed pending-file write is forgotten on reselect
`src/facts_memory.rs:234-238` (`Event::Pending` handling) and `:312-359`
(`restore_pending`). After `write_pending` fails, `pending_error` is set and
`queue_pending` stops. Switching to another character and back runs `Load`,
whose `read_pending` finds the *previous* good file, so `restore_pending` takes
the `Ok(Some(list))` branch and sets `retain_pending = true`,
`pending_error = None`. The candidate that failed to persist was cleared from
memory by `select_character`, so it is gone, and the UI shows retention healthy.
Probe (scratch copy, Unix, dir chmod 0o500 after the first write):
```
after reselect: pending_error=None retain=true candidates=["I like apples"]
```
"I like pears" (observed after the chmod) is lost silently and no error remains.
The same guard (`self.selected == Some(id)`) drops write failures for a
non-selected character entirely, so an outbox entry for the previous character
that fails never surfaces anywhere.
Fix (bounded): keep failures per character, e.g. `pending_failed: HashMap<Uuid,
String>`, set on `Event::Pending` regardless of selection, consulted by
`restore_pending` (prefer the recorded failure over the disk read, same as the
outbox precedence), cleared only by a successful write or opt-out for that id.
Test: the probe above, asserting `pending_error.is_some()` after reselect.
Candidate suggestions only; the accepted ledger is unaffected.

### F2 — HYPOTHESIS, low: last pending write can be cut off at process exit
`src/facts_memory.rs:648-670` `Drop` hands unsent outbox entries to the worker
and returns without waiting for completion. Bevy drops the `Studio` resource
when `app.run()` returns (`src/main.rs:361`) and `main` then exits, killing the
worker mid-`atomic_write`. Consequence is bounded: `candidates.jsonl` is replaced
atomically so it is either old or new, and at worst a `.candidates.jsonl-<uuid>.tmp`
file is left in the character directory forever (nothing removes stray temp
files; `delete()` only removes the two named files). The same exposure already
existed for `facts.jsonl` saves, so this is not a regression. Not reproduced.
Bounded fix: have `Drop` also wait (within the existing 2 s deadline) for the
`Event::Pending` acknowledgements of the writes it handed off, or keep the
worker's `JoinHandle` and join with the same deadline.

### F3 — VERIFIED, info: Fish admission and socket ownership are sound
`src/fish_ws.rs:304-385` `MAX_WORKERS = 2`; the generation is bumped on every
`open()` via `cancel()`, so two concurrent workers never share a generation.
The single `SocketControl` slot is written only after `check()` under the lock
(`:834-836`), `Permit::drop` takes it only on a matching generation, and
`publish` double-checks under the mailbox lock. A late-succeeding stale connect
drops its socket on the `check` error path. `is_busy()` is not consulted by any
caller outside `fish_ws.rs`. Known, documented limits (not regressions): two
rapid cancels against a stalled host leave the third request in `pending` for up
to `CONNECT_TIMEOUT` (10 s), and `to_socket_addrs()` (DNS) is unbounded and holds
a slot. `ADDRESS_TIMEOUT` is applied to non-final addresses only. New tests
pass, including the Linux saturated-listener ones.

### F4 — VERIFIED, info: persistence state machine
`restore_pending`'s "outbox beats disk" branch is effectively unreachable because
`poll()` never sends `Load` while the outbox holds that id, and the worker is
FIFO; harmless. A probe for a window where `busy == false && !loaded` (which
would bypass the send-path guard at `src/studio.rs:745`) did not reproduce:
`busy` stays true while the previous character's load is in flight, and the
outbox entry is flushed in the same `poll` that observes its predecessor's
completion. `observe_user_turn` bounds (1000 chars, fixed source) satisfy
`validate_pending`, so `write_pending` cannot fail validation on its own output.
Expiry pruning per frame is a `SystemTime::now()` plus a scan of ≤16 items.

### F5 — VERIFIED, info: UI and motion catalog
`filter_motions` recomputes `category_of` per entry per frame (string lowercase +
split + Vec alloc). Probe: 600 entries → 318 µs per pass in a debug build, so
tens of µs in release. Not actionable; cache in `MotionEntry` at scan time if
the picker ever lists thousands. Category persisted in egui temp data with
matching get/insert types. `MotionCatalog::resolve` never returns a caller
path, IDs are salted with the offered set, and stale scans are rejected.
"Restore mouth defaults" / "Reset analysis tuning" copy individual fields only.

### F6 — VERIFIED, info: recall mode and decision client
`memory::context_pins_only()` produces identical bytes to `context("")`
(test `semantic_pins_trace_preserves_context_and_distinguishes_empty_lexical_queries`),
and the inspector labels the mode. `recipient_decision::decision_client()` caches
one `reqwest::blocking::Client` in a `OnceLock`; proxy environment is now read
once at first use instead of per call. Only matters if proxy env vars change
while the process runs. A builder failure is cached permanently, which is fine
because it is deterministic.

## Not reviewed
Rendering, audio and GUI behaviour of the new controls (no GUI permitted).

## Integration disposition

Parent verified F1 is addressed in the subsequent implementation: failed pending snapshots/errors are stored per character, restored on reselect, and surfaced with explicit retry/removal controls. Offline regression tests cover selected and non-selected write failures. F2 remains an unverified shutdown limitation; no durability-at-process-exit claim is made.

The subsequent combined suite passes 420 tests (350 library, 64 binary, 6 inspection), strict all-target Clippy and package formatting. New motion suggestions and manual reflection drafts have offline/mock verification only, not live-provider or desktop acceptance.
