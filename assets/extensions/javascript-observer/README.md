# JavaScript lifecycle observer

A dependency-free Node.js persistent integration using the same JSON Lines protocol as the Python examples. Install this folder as a local integration, grant lifecycle observation, then enable it. Node.js must be available on PATH; no npm install is required.

The process writes started.json with its host session identity. It records app.ready metadata when observed, and on app.stopping atomically writes latest-shutdown.json then exits. It never requests reply text, avatar actions or network access. A late installation need not receive app.ready.

Shutdown notification is best-effort. Missing or malformed notification, EOF or a host crash may prevent a receipt; no external cleanup or reliable delivery is implied. Files are replaced in the host-provided extension data directory, not appended indefinitely.

The sample deliberately rejects sequence/generation numbers outside JavaScript's safe integer range instead of silently rounding a protocol u64. Longer-lived implementations can use a lossless JSON parser. stdout stays empty; protocol requests belong there if an author extends this example. Host capabilities do not sandbox local code.

MIT licensed. Provisional API 1, currently Linux supervision only.
