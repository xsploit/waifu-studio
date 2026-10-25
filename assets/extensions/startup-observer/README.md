# Startup observer

A metadata-only Python 3 hook for app.ready. Install from Extensions → Add → Examples → Startup observer, or select this folder. Grant lifecycle observation and enable it before the next application startup.

Ready means local host initialization and the initial installed-extension reconciliation have completed. It does not mean that every extension started successfully, or that an avatar, voice service, chat provider or external account is ready. The notification contains an app_run_id identifying this application run. It is not replayed when an extension is installed, enabled or restarted later.

The hook writes the event to latest-startup.json inside its extension data directory, then exits. It receives observation-only permission and no action target. It never requests avatar control or reply text, and makes no network requests. The host still bounds queues, process slots and runtime; delivery is best-effort, not durable or guaranteed after a crash.

Python 3 is the only requirement. The bundled source is MIT licensed and uses provisional API 1.
