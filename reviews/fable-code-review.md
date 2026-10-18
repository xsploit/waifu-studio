# Fable 5.1 read-only code review — waifu-studio

Date: 2026-10-18. HEAD `26e52f5`. Reviewed commits `609299a`, `d5d8477`, `26e52f5`
plus the uncommitted worktree. Worktree snapshot at review time (files modified):
`expression_controls.rs`, `lipsync_controls.rs`, `ui.rs` (parent),
`fish_ws.rs` (cancellation agent, in progress), `facts_memory.rs` (persistence
agent, in progress, test still being debugged), `motion_library.rs` (categories).
Concurrent edits continued during the review; findings on in-progress files are
tagged with the state seen.

Verification run (offline, `CARGO_TARGET_DIR=../../work/studio-target`):
`cargo test --lib -- provider::tests memory::tests facts_memory::tests fish_ws::tests motion_library::tests`
→ 68 passed, 1 failed (see F1). Library tests compile with all worktree edits.

No source files were edited. No paid requests, credentials or GUI used.

## Findings

### F1 — VERIFIED (worktree, in progress): facts pending-candidate test fails
`src/facts_memory.rs:1701` `mutations_during_saves_and_stale_events_lose_nothing`:
```
assertion `left == right` failed
  left: ["I like pears"]
 right: ["I like pears", "I like plums"]
```
A candidate observed while an acceptance save is in flight, followed by a
character switch away and back, is lost from the restored candidate list.
The test body contains a debugging `eprintln!` (line ~1689), so the persistence
agent is actively on this. Not a committed defect. Flagging so the parent does
not treat the worktree as green. Suggested check: order of `Event::Ledger`
(stale Save → `accepting.take()`) vs the outbox snapshot taken in
`queue_pending`, and whether the Load for the re-selected character races the
second `WritePending` on the serialized worker.

### F2 — VERIFIED, low: lexical recall trace is silently empty in semantic mode
`src/studio.rs:777` calls `memory.context("")` when semantic notes exist, so
`RecallTrace.query_terms == 0` and only pins appear. The inspector in
`memory_controls.rs:88` then reads "0 query terms", which looks like a
tokenization failure rather than "lexical recall bypassed, semantic retrieval
used". Behaviour is correct; only the explanation is misleading.
Bounded fix: add a `mode` field (`Lexical | PinsOnlySemantic`) to
`RecallTrace`, set by a `context_pins_only()` entry point or by `context("")`
when `query.is_empty()`, and show it in the inspector. Test: fixture with pins
and matching notes, call `context("")`, assert mode + pins-only hits.

### F3 — VERIFIED, low: scene chat evidence recall leaves no trace
`src/scene_chat.rs:450` uses `facts_memory::context_from_disk`, which discards
the trace (`facts_memory.rs` `recall_traced(..).and_then(|(c, _)| c)`), while
`scene_chat.rs:459` calls `memory.context(&text)` which *does* replace the
notes trace. After a scene reply, the Notes inspector shows the scene query
but the Evidence inspector still shows the previous studio-chat query, with no
hint that the two traces come from different sends. Bounded fix: either both
inspectors record a `source` (`Studio | Scene`) and the scene path stores the
facts trace too, or the scene path does not touch the notes trace. Not a
correctness bug for prompt bytes (oracle tests confirm byte equality).

### F4 — VERIFIED, info: byte-for-byte prompt equivalence holds
`d5d8477` and `26e52f5` keep legacy-oracle tests (`legacy_context`,
`legacy_recall`) and they pass against the current worktree. The over-budget
line still consumes one of the six slots exactly as before; `over_limit` is
computed from `ranked.len()` before the budget loop, which matches that
semantics. `context()` guards (`enabled && loaded && !busy`) precede trace
replacement, so a bailed call never clobbers the previous trace.

### F5 — VERIFIED, info: first-phrase coalescer is consistent with Fish flush
`src/provider.rs` `DeltaCoalescer`: the push that completes a first-phrase
boundary is itself emitted eagerly (`eager` is evaluated before the boundary
loop), so the token that triggers Fish's `TextQueue` first flush
(`fish_ws.rs:90-93`, same thresholds) is never held in the 32 ms window. The
coalescer counts from 0 while `TextQueue.since_flush` already includes the
`[direction] ` prefix, so Fish's boundary is reached no later than the
coalescer's. Edge: the 24-event cap can end the eager phase one token before
Fish's `>=24 && whitespace` rule for single-character tokens; worst case one
32 ms window. Not actionable.
Hypothesis (not verified, model-dependent): the coalescer counts raw delta
characters, including any `<waifu-affect>` block that `ReplyAffectFilter`
strips (`reply_affect.rs:116`). If a model emitted the block first, the eager
phase could end before Fish sees prose. The prompt places it at the end, so
this is theoretical.

### F6 — VERIFIED, info: cancel paths after `609299a`
Cancellation still only raises the `stopped` flag; a worker blocked in a read
lives until the 180 s client timeout, and `CHAT_WORKERS` is capped at 2
(`provider.rs:560`). Two rapid cancels against a stalled upstream make the
third send fail immediately with "Previous connections are still closing".
Pre-existing and documented in code; `609299a` did not change it. The bench
`Chat::close` App path correctly tolerates this (records
`chat_worker_closed=false`) but the next iteration can then hit the permit cap
and record a Failed row rather than a measurement; the row's `error` field
will say so. Stale `Delta` events after cancel are dropped in `studio.rs:915`
by the job-id guard; no Finished/Failed arrives after cancel, and
`cancel_chat` clears `job` so nothing waits on one.

### F7 — VERIFIED, info: fish_ws worker slot change (worktree, in progress)
`fish_ws.rs` `MAX_WORKERS = 2`: the single `SocketControl` slot is written only
after `check(active, generation)` under the lock (`fish_ws.rs:834-836`), and
`Permit::drop` takes it only when the generation matches, so a stale worker
cannot overwrite or shut down the live socket. `publish` double-checks the
generation under the mailbox lock. `is_busy()` now stays true while a stale
worker unwinds in DNS/connect; no caller in `src/` consults
`FishWsState::is_busy`, so no UI gating regresses. Per-address
`ADDRESS_TIMEOUT` is applied only to non-final addresses, which is the right
shape. Nothing actionable at this snapshot.

### F8 — VERIFIED, info: parent UI edits
`expression_controls.rs`, `lipsync_controls.rs`, `ui.rs` compile with the
library tests and the updated `filter_motions` test passes. The "Restore mouth
defaults" and "Reset analysis tuning" buttons copy individual fields from
`Default`, so unrelated user settings are preserved as the help text claims.
No issues found.

## Not reviewed in depth
`facts_memory.rs` persistence diff beyond trace interactions and F1, and the
rest of `fish_ws.rs`, because both were changing during the review.

## Parent verification after review

F3's claimed cross-inspector mismatch is withdrawn: `scene_chat::run` creates its own `MemoryState` at `src/scene_chat.rs:431`. Its note trace is worker-local and cannot overwrite the UI's note trace. Neither scene-worker trace appears in the inspectors, as FEATURE-STATUS already states.

F2 is addressed by an explicit `context_pins_only()` entry point and a trace mode label. Ordinary empty lexical queries remain labeled lexical. Prompt content remains unchanged. F1 remains an integration gate for the in-progress persistence implementation, not a committed defect.

F1 resolution: Opus changed queue ordering so pending writes are flushed before a character reload is queued. The FIFO worker now reads the latest submitted candidate state. Parent reran the complete suite after that fix; the regression and all other targets passed. Opus additionally reports 30 consecutive targeted stress runs.
