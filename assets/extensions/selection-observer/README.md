# Character selection observer

This Python 3 example is a metadata-only one-shot hook for `character.selected`. Install its folder from Extensions → Add → Local integration from folder, then enable its lifecycle observation permission and the integration. No credentials or third-party packages are needed.

Each invocation atomically replaces `latest-selection.json` in its host-provided extension data directory. It records only the triggering selection event. It never requests reply text, sends messages, plays motions, changes expressions or connects to a service.

A selected character may still be loading or may fail to load. Selection does not authorize avatar actions. This hook accepts no usable action target; `target.changed`, if provided, must contain null. An integration that controls a loaded avatar must independently use the host's current target UUID and incarnation generation.

The example exits after recording one event. No-op selections do not produce a new selection event. Host queues are bounded and rapid replacement can discard stale pending hooks; this latest-event receipt is not a complete audit log or durable event subscription. A persistent integration can observe the source stream when it needs ordered selection metadata.

The reader bounds records, packet count and its wait time. Invalid authority, missing observation permission, unexpected event type or early EOF causes a nonzero exit without a receipt. Runtime supervision and host permissions still apply; the process is not OS-sandboxed.

License: MIT. This source example accompanies provisional API 1; it is not a stable published extension release.
