# Motion end observer

A Python 3 observation-only hook for motion.playback.ended. Install and enable it with lifecycle observation before playing a motion. It replaces latest-motion-end.json in its own extension data directory, then exits. No credentials or third-party packages are required.

The general event describes an admitted logical playback request: completed, cancelled or failed. An accepted import can fail before playback activates. A rejected admission creates no playback. Loop iterations are not completion; a loop ends when stopped or superseded. Cancellation does not prove every crossfade contribution or retained pose has disappeared.

Each admitted request has a unique playback_id. Source attribution is captured at admission, not guessed from the selected avatar at the end. source_target may be null when the original avatar incarnation is unknown. A nonnull source target is metadata, never action authority. motion_id is an optional registered opaque reference; raw asset paths and arbitrary errors are excluded. Extension source session/generation/action IDs identify the original requester, distinct from this observer invocation.

The existing requester-specific motion.finished receipt remains separate. These new hooks receive only lifecycle observation permission and a permanently null action target. They cannot request another gesture, send a message or emit an event to create an indirect completion loop. Persistent integrations with action grants can still intentionally chain their own receipts; no universal loop-prevention claim is made.

Observation is bounded and not a durable event log. Late subscription, reset or observation regrant does not replay older outcomes. Application shutdown starts no new handlers. This sample confirms host metadata receipt, not rendered motion, clipping quality or speaker playback.

MIT licensed. Provisional API 1; Linux supervision and Python 3 required.
