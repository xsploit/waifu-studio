# Opus extension review

Supplied-source review, 2026-10-20. No reviewer builds, commands or live UI checks. Source snapshot predates resulting fixes.

I found four issues in `src/extension_process.rs`. None of them affect the physics or manual-ownership paths.

**1. Escaped descendant can hang the supervisor forever (lines 351–377)**
- **Trigger:** the leader exits, but a descendant that left the process group (`setsid`/`setpgid`) still holds the inherited stdout open. `stdout_closed` never becomes true. With `lifetime: None`, the loop runs until someone calls `stop()`, so the extension never reports an exit.
- **Smallest fix:** when `exited` first becomes `Some`, record a drain deadline (for example 500 ms). Once the deadline passes, break with the exit status and report "Descendant kept stdout open" instead of waiting for EOF.

**2. Group kill after reaping can hit an unrelated process group (line 149)**
- `try_wait` at line 354 has already reaped the leader. Line 359 then kills the group, so by the time `Running::drop` runs, the group can be empty.
- **Trigger:** if the group is empty, the PGID/PID can be reused by a new, unrelated group leader. `kill(-pid, SIGKILL)` in `Drop` would then kill that unrelated group. (`Child::kill` at line 151 is safe, because std skips it once the status is known.)
- **Smallest fix:** add a `group_killed: bool` to `Running` and set it at line 359. `Drop` should only call `libc::kill(-pgid)` when the leader has not been reaped.

**3. A normal child exit can be reported as a failure (lines 339–348)**
- **Trigger:** the host has a line queued or partly written when a hook finishes and exits. Rust ignores SIGPIPE, so the write returns `BrokenPipe`. The supervisor returns `Err("Cannot write extension stdin")`, and the child's real exit code and "Completed" are lost. This can happen with a correct extension.
- **Smallest fix:** on `ErrorKind::BrokenPipe` (and on `Ok(0)`), discard `pending`, mark stdin closed, and stop writing. Let the `exited`/`stdout_closed` path report the real status.

**4. The `Exited` event is always lost after a queue-overflow termination, and dropping the handle does not reliably reap the child (lines 75, 122–126)**
- **Event loss:** queue overflow means the queue is full, so `try_send(Exited)` fails silently. Callers that only use `poll()` never see the exit. Only `termination()` and `finished()` show it.
  - **Fix:** document `termination()` as the authoritative exit signal, or keep one queue slot in reserve for `Exited`.
- **Orphaned child:** `Drop` only sets the stop flag, and the worker thread is detached. If the host process exits soon after the drop (app shutdown), the worker may never run `Running::drop`, leaving the child group orphaned.
  - **Fix:** in `Drop`, after `stop()`, join the worker. This is bounded because the loop checks `stop` every 5 ms. `JoinHandle::join` takes ownership, so wrap the field in an `Option` and use `take()` to get it out.

**Minor (no change needed unless you want it):** a final stderr fragment without a trailing newline is discarded. Logs are also capped at 8 lines for the whole process lifetime, so long-running services go silent in the log after that.

Your note that this is supervision, not an OS sandbox, still applies: findings 1 and 2 only cover programs that stay in the process group or leave it in ordinary ways.
