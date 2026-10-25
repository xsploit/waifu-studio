# Integration startup observer

A metadata-only Python 3 hook for extension.started. Install and enable this observer with lifecycle observation permission, then start a persistent integration such as the JavaScript observer. The hook writes latest-integration-start.json in its own extension data directory and exits. No credentials or third-party packages are required.

A start notification describes an actual process launch, not an enable toggle, launch request or service connection. installation_id identifies the installed integration; the inner session_id and process_generation identify that source process incarnation. The outer host envelope identifies this observer invocation and may have a different session. Neither identity authorizes avatar actions.

Only persistent integrations produce this notification. Hook invocations and manual test children do not, so startup handlers cannot recursively trigger themselves. Restarting an integration produces a new incarnation; an OS spawn failure produces no start notification. A fast-exiting child can still have genuinely started, including a launcher that exits because a later dependency is missing. No account or network readiness is implied.

Startup hooks are observation-only and targetless. Notifications are not replayed for late observers. Queues, expiry and process limits still apply; the replaced latest-event file is not a complete durable audit log.

MIT licensed. Provisional API 1; Python 3 on Linux is required by this example.
