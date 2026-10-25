# Integration exit observer

A Python 3 metadata-only hook example for extension.stopping and extension.stopped. Install and enable it with lifecycle observation before starting another persistent integration. It needs no credentials, third-party packages or avatar actions.

A stopping event records the first host stop request for a process incarnation. It is intent, not evidence that the process exited. A stopped event comes from the retained supervisor termination receipt after an actual spawn. Its nullable exit_code is the available process exit code; null does not identify a particular signal or failure. requested_reason records a host request preceding publication of completed supervision when present; it is not a claim that the request caused the exit. The OS child may already have exited while supervision was still draining or cleaning up. Reasons distinguish disabled, removed, restarted, reconfigured and host_shutdown. No arbitrary process/provider error text is copied. A confirmed supervised-process exit does not prove that external cleanup succeeded or that every avatar contribution was cancelled.

The observer writes latest-stop-request.json or latest-exit.json in its own data directory and exits. Correlate the source installation_id, session_id and process_generation in both records. The outer envelope identifies this hook invocation, not the source process. Files replace the latest receipt and do not constitute a durable audit log.

Hook/test children do not emit these integration lifecycle events. Notifications are not replayed to late observers, and shutdown does not start replacement hooks. Application shutdown can therefore end observation before all terminal metadata is delivered; app.stopping is a separate best-effort notification for running persistent observers.

MIT licensed. Provisional API 1; Linux supervision and Python 3 are required by this example.
