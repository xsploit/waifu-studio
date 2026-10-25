# Authoring local process and hook integrations

**Provisional API 1: an implementation checkpoint, not a stable published release.** This guide describes the current source and bundled examples. Check it against the source when updating an integration; older envelopes in the [design draft](EXTENSION-DESIGN.md) are not the wire contract. Execution is currently supported on Linux.

Any language can implement the protocol using a bundled executable or executable script with an installed interpreter. There is no language SDK requirement, network listener, child handshake or `ready` message. The host owns avatars, playback and conversations; your process submits requests over stdin/stdout.

Executable installations run local code with the user's OS authority. **There is no OS sandbox.** Host capability grants restrict host actions and private event payloads, not the child's independent filesystem or network access. Only install code you trust.

## Check a package without installing it

Run the built executable with this option alone:

```sh
waifu-studio --check-extension /path/to/package
```

Exit code0 means the existing content/integration manifest validators accepted the schema and declared paths; errors return a nonzero exit code. The short summary reports category and counts, without displaying credential values or arbitrary manifest text. This command runs before app-data discovery, startup credentials, provider discovery, GPU/window initialization or any extension process. It writes nothing and never installs or grants permissions.

This is a manifest check, not runtime approval. The installer still validates the full copied folder (including its own file/symlink/size policy), and a passing check does not prove that assets decode/render or that a handler behaves correctly. Files can change after checking. The same command handles content packs, persistent processes and hooks; executable checks use the platform's existing executable-path validation.

## Make a package

For a relocated app, ship bundled examples in `assets/extensions/` beside the executable. The Add menu uses that installation's assets, including when optional examples are omitted; it does not borrow omitted examples from the original build checkout. Missing examples remain disabled with an explanation. Debug binaries without an adjacent `assets/` directory use the source checkout's examples. Release binaries require adjacent assets or an explicit `WAIFU_STUDIO_ASSET_ROOT`; the source launcher sets that root for development. Installing a bundled integration still leaves it disabled and ungranted until the user enables it.

Put `extension.json`, your executable and any assets in one folder. A minimal persistent gesture package looks like this; provide the referenced files yourself or start from the bundled [reply-gesture](../assets/extensions/reply-gesture/) package:

```json
{
  "api_version": 1,
  "id": "org.example.reaction",
  "version": "1.0.0",
  "name": "Example reaction",
  "description": "Request one gesture after a completed reply.",
  "provenance": {"author": "Example author", "license": "MIT"},
  "process": {"program": "worker.py", "args": [], "working_directory": "."},
  "capabilities": ["lifecycle.observe", "motion.play"],
  "settings": [
    {"id": "motion", "label": "Gesture", "type": "motion", "default": "org.example.reaction/nod"}
  ],
  "content": {
    "motions": [
      {"id": "nod", "name": "Gentle nod", "asset": "motions/nod.bvh", "role": "gesture", "loopable": false}
    ]
  }
}
```

Use a stable lowercase dotted package ID. Local content IDs resolve as `package.id/local-id`; they must be unique across content categories. Use a three-part numeric version such as `1.0.0`. See [content packs](content-packs.md) for other content categories.

`program` must name a regular file inside the package, not `/usr/bin/python3`, a PATH command or a URL. On Linux it must have an executable bit; a Python script can use a `#!/usr/bin/env python3` shebang. `args` are literal arguments, never shell commands. `working_directory` defaults to the package root. Absolute paths, traversal and escaping references are rejected; installation rejects symlinks throughout the copied folder. Ensure the user has any required interpreter or external dependencies.

For a one-shot hook, **replace** `process` with `hooks`:

```json
{
  "hooks": [
    {"event": "reply.finished", "handler": {"program": "hook.py"}}
  ]
}
```

That fragment belongs in the complete manifest. Supply exactly one persistent process **or** one to four hooks with distinct triggers. Supported triggers are `motion.playback.started`, `motion.playback.ended`, `extension.stopping`, `extension.stopped`, `extension.started`, `app.ready`, `character.selected`, `reply.finished`, `speech.started`, `speech.stopped` and `emotion.changed`. There are no manifest subscription filters or arbitrary event-name hooks. Hook launch requires `lifecycle.observe`. Each hook gets a fresh session and the captured triggering event. Reply, speech and emotion hooks also receive the action target pinned when the invocation was captured; the observation-only hooks below do not. That target need not be the event's source character. It does not become an ongoing lifecycle subscriber; it can receive its own motion terminal receipt. Exit after finishing the reaction.

`character.selected` hooks are metadata-only. They receive only lifecycle observation permission and a null action target, even if the installation has additional grants. The event contains nullable `previous` and `new` character UUIDs and a positive `selection_revision` that increases within this application run, independently of avatar generations. Neither UUID is an avatar action target. Pending selection hooks are superseded by newer selections; persistent integrations can observe ordered source events if they need every switch. A no-op selection produces no selection event. See the [selection observer](../assets/extensions/selection-observer/) for a hook that records metadata without acting on the avatar.

`app.ready` means local host initialization and the initial installed-extension reconciliation have completed for this application run. It contains a nonnil `app_run_id`, and does not establish avatar/provider/account readiness or successful startup of every installed extension. The notification is source-only and is not replayed for a later install, enable or restart. Ready hooks are observation-only and always receive a null action target; ordinary character/target changes do not give them avatar authority. See the [startup observer](../assets/extensions/startup-observer/). Queue capacity, expiry, process limits and shutdown can still prevent delivery; it is not a durable startup job system.

`extension.started` describes an actual persistent integration process incarnation: `installation_id`, source `session_id` and source `process_generation`. It is not an enable toggle, launch request or external service-ready notification. A fast-exiting child can genuinely have started; a failed spawn cannot. Source identities inside the event belong to the original integration, while the outer host envelope belongs to this observer session. Startup hooks get observation-only grants and a null action target. Hook invocations and manual test children do not emit this event, preventing recursive handler creation. Late registration, reset and observation regrant do not replay earlier launches, even when an OS receipt reaches the host after the subscription boundary. Queues, expiry and source-gap handling still apply; observation is not a durable audit log. See the [integration startup observer](../assets/extensions/integration-start-observer/).

`extension.stopping` records the first host stop request for an actual persistent-process incarnation, with `installation_id`, source `session_id`, source `process_generation` and a typed `reason`: `disabled`, `removed`, `restarted`, `reconfigured` or `host_shutdown`. It does not confirm exit. `extension.stopped` uses the same source identities with nullable `exit_code` and nullable `requested_reason`, and comes from retained supervisor termination evidence after a real spawn. Null exit code does not identify a particular signal or error. A requested reason is host intent captured before completed supervisor termination was published, not proof of exit causation. OS exit may already have occurred while supervision is still cleaning up. A spontaneous exit can have no preceding request. Neither event proves external cleanup or avatar-effect cancellation. No arbitrary process/provider error text is exposed.

Stop/exit hooks are observation-only and targetless. Hook invocations and manual-test children are excluded at source. App shutdown still launches no new hooks; its closing notification to existing observers is the separate `app.stopping` event. The [integration exit observer](../assets/extensions/integration-stop-observer/) records the distinction without controlling the avatar.

`motion.playback.started` reports a clip becoming active in the animator, after a successful import or direct clip selection. It carries `playback_id`, nullable `source_target`/`motion_id`, and the same tagged `source` as the terminal event below. It uses the admission identity; it does not reconstruct an avatar from the current selection. A new explicit Play request and restoration of parked playing state each create a fresh host playback identity and activation. Pause/Stop can terminate that identity; loop wraps do not create activations. An import cancelled or failed before activation has an end without a start. Start and end can be collected together, in timestamp order, each at most once. This is logical animator activation, including a crossfade; it does not prove a rendered frame, visible pose, nonzero displacement or GPU presentation.

Both motion observation hooks require `lifecycle.observe`, receive a null action target, and retain only observation permission even when their installation has broader grants. Source target/provenance is metadata, not authority to control that avatar. New subscriptions do not replay earlier activations, and revoking observation drops queued activation metadata. The owned [motion activation observer](../assets/extensions/motion-start-observer/) writes `latest-motion-start.json` in its private data directory and requests no actions. It is available under **Extensions → Add → Examples → Motion activation observer** and needs local Python 3; it makes no account or network connection.

`motion.playback.ended` observes general admitted host and extension playback requests. It carries `playback_id`, nullable `source_target`, nullable registered `motion_id`, a tagged `source`, and `outcome` (`completed`, `cancelled`, `failed`). Source is `{ "type": "host" }` or `{ "type": "extension", "integration_id": "…", "session_id": "…", "process_generation": 1, "action_id": "…" }`. Attribution is captured at admission; unknown source incarnation remains null, rather than being replaced by the current selection at termination. Asset paths and arbitrary failure text are excluded. An admitted import can fail before activation; rejected admission creates no playback, and loop wraps do not produce completion. Cancellation does not prove that every retained/crossfade pose disappeared.

These general motion-end hooks are observation-only and permanently targetless. No controller, message-delivery or event-emission grants survive into the invocation, preventing indirect terminal-hook feedback chains. This does not constrain full-capability persistent integrations intentionally chaining their private receipts. The original `motion.finished` stays a requester-specific receipt with its existing target/action correlation; it is not broadcast as this general source event. See the [motion end observer](../assets/extensions/motion-end-observer/).

Optionally add `"test_process": {"program": "test.py"}` for the explicit **Test reaction** control. It supplements, rather than replaces, the required process/hooks. It receives saved settings, current grants and a fresh session with a pinned target, but no ordinary reply/speech/emotion trigger stream. The test handler must initiate its own deterministic reaction.

An integration-start observation in a separate observer session:

```jsonl
{"api_version":1,"type":"host","session_id":"00000000-0000-4000-8000-000000000001","sequence":6,"process_generation":1,"message":{"type":"event","event":{"type":"extension.started","installation_id":"00000000-0000-4000-8000-000000000005","session_id":"00000000-0000-4000-8000-000000000006","process_generation":2}}}
```

A stop request and a separately confirmed termination for that source incarnation:

```jsonl
{"api_version":1,"type":"host","session_id":"00000000-0000-4000-8000-000000000001","sequence":7,"process_generation":1,"message":{"type":"event","event":{"type":"extension.stopping","installation_id":"00000000-0000-4000-8000-000000000005","session_id":"00000000-0000-4000-8000-000000000006","process_generation":2,"reason":"restarted"}}}
{"api_version":1,"type":"host","session_id":"00000000-0000-4000-8000-000000000001","sequence":8,"process_generation":1,"message":{"type":"event","event":{"type":"extension.stopped","installation_id":"00000000-0000-4000-8000-000000000005","session_id":"00000000-0000-4000-8000-000000000006","process_generation":2,"exit_code":null,"requested_reason":"restarted"}}}
```

## Host shutdown

`app.stopping` is a best-effort notification for already-running persistent integrations with lifecycle observation permission. It identifies the application run previously initialized by `app.ready`; a late-started observer may see stopping without having seen ready. It is not a supported manifest hook trigger, and shutdown never starts a new handler just to deliver it.

The host quiesces action admission before offering this event. No new avatar actions, message delivery or restarts are permitted during teardown. Use the short notification window for local cleanup and exit promptly; the host still force-stops survivors within its existing shared shutdown deadline. A blocked stdin pipe, partial earlier record, revoked observation grant, protocol error or process crash can prevent notification. No application-level cleanup acknowledgment is required or implied. A queued host record alone does not prove child receipt.

An illustrative stopping event, in an already established session:

```jsonl
{"api_version":1,"type":"host","session_id":"00000000-0000-4000-8000-000000000001","sequence":9,"process_generation":1,"message":{"type":"event","event":{"type":"app.stopping","app_run_id":"00000000-0000-4000-8000-000000000004"}}}
```

The [JavaScript observer](../assets/extensions/javascript-observer/) demonstrates this without Python or npm packages. It writes an explicit receipt and exits when stopping is observed. Its example test uses Node.js if installed; a development machine without Node skips that optional runtime check. Package validation itself does not require Node.

## Speak JSON Lines

Read host messages from stdin. Write one compact UTF-8 JSON object followed by LF to stdout and flush it. Put diagnostics on stderr; stdout is exclusively protocol data. Unknown fields, enum values and API versions are rejected. Keep each record comfortably below 64 KiB.

These are complete wire records. UUIDs, generations and sequences are illustrative: copy identities from the actual host. First the host sends a handshake, then a target event when authorized:

```jsonl
{"api_version":1,"type":"host","session_id":"00000000-0000-4000-8000-000000000001","sequence":1,"process_generation":1,"message":{"type":"handshake","origin":{"integration_id":"org.example.reaction","display_name":"Example reaction"},"settings":{"motion":"org.example.reaction/nod"},"capabilities":["lifecycle.observe","motion.play"]}}
{"api_version":1,"type":"host","session_id":"00000000-0000-4000-8000-000000000001","sequence":2,"process_generation":1,"message":{"type":"event","event":{"type":"target.changed","target":{"uuid":"00000000-0000-4000-8000-000000000002","generation":3}}}}
```

Use only capabilities actually listed in the handshake. A target can be `null`; wait for a usable target before requesting an action. All actions, including `catalog.get`, require the exact target UUID and generation. A reply's `character` identifies the source and is not an action target.

A selection notification carries source UUIDs and a separate revision. It may arrive before the new avatar loads; a persistent integration must still wait for a usable `target.changed` before requesting avatar actions. Selection hooks always receive a null target and observation-only grants.

```jsonl
{"api_version":1,"type":"host","session_id":"00000000-0000-4000-8000-000000000001","sequence":3,"process_generation":1,"message":{"type":"event","event":{"type":"character.selected","previous":null,"new":"00000000-0000-4000-8000-000000000002","selection_revision":1}}}
```

The following reply/action exchange is a separate illustrative sequence.

A completed reply might arrive as:

```jsonl
{"api_version":1,"type":"host","session_id":"00000000-0000-4000-8000-000000000001","sequence":3,"process_generation":1,"message":{"type":"event","event":{"type":"reply.finished","reply_id":"00000000-0000-4000-8000-000000000003","character":"00000000-0000-4000-8000-000000000002","outcome":"completed"}}}
```

Send the motion action on stdout. Child request sequences are independent of host sequences:

```jsonl
{"api_version":1,"type":"action","id":"nod-1","sequence":1,"session_id":"00000000-0000-4000-8000-000000000001","process_generation":1,"target":{"uuid":"00000000-0000-4000-8000-000000000002","generation":3},"action":{"type":"motion.play","motion_id":"org.example.reaction/nod","mode":"one_shot"}}
```

The host acknowledges admission and separately reports the source playback's terminal outcome:

```jsonl
{"api_version":1,"type":"host","session_id":"00000000-0000-4000-8000-000000000001","sequence":4,"process_generation":1,"message":{"type":"result","id":"nod-1","target":{"uuid":"00000000-0000-4000-8000-000000000002","generation":3},"result":{"status":"ok","value":{"type":"accepted"}}}}
{"api_version":1,"type":"host","session_id":"00000000-0000-4000-8000-000000000001","sequence":5,"process_generation":1,"message":{"type":"event","event":{"type":"motion.finished","target":{"uuid":"00000000-0000-4000-8000-000000000002","generation":3},"motion_id":"org.example.reaction/nod","action_id":"nod-1","outcome":"completed"}}}
```

An `accepted` result is not proof that animation finished. With `lifecycle.observe`, match `motion.finished` by session, process generation, target, action ID and motion ID. Outcomes are `completed`, `cancelled` or `failed`: manual replacement cancels ownership; an import failure can fail after admission. Do not infer completion from elapsed time or an inactive player. A stopped/disconnected connection cannot be relied upon to receive a final event. There is no expression terminal event.

A rejection instead has this shape:

```jsonl
{"api_version":1,"type":"host","session_id":"00000000-0000-4000-8000-000000000001","sequence":4,"process_generation":1,"message":{"type":"result","id":"nod-1","target":{"uuid":"00000000-0000-4000-8000-000000000002","generation":3},"result":{"status":"error","code":"unknown_motion","message":"Motion is not registered"}}}
```

Treat error codes as machine-readable and messages as diagnostics. Codes are `invalid_request`, `unsupported_version`, `permission_denied`, `stale_session`, `stale_process`, `stale_target`, `unknown_motion`, `unknown_expression`, `busy`, `unavailable` and `internal_error`. Manual playback hold, other host owners and the process-wide bounded animation importer can return `busy`; import contention creates no new playback request. Retry only under your own bounded policy, never override the user.

Use positive increasing request `sequence` values within a session/process incarnation. IDs are correlation labels, not replay keys; use distinct IDs while tracking outstanding work. Identical retransmissions do not execute twice: pending duplicates are suppressed and up to 128 completed responses are cached. Older evicted requests are rejected, not executed again. Authenticated rejected requests also consume their sequence. Prefer waiting for a result before the next request. Never reset sequences within a session or automatically replay work after reconnect; a new handshake starts a fresh session.

## Actions and permissions

Declare only the capabilities needed, then have the user grant them. Each row shows the exact inner `action` object; wrap it in the action envelope above.

| Capability | Action or access |
| --- | --- |
| `motion.play` | `{"type":"motion.play","motion_id":"org.example.reaction/nod","mode":"one_shot"}`. Registered catalog ID, never a file path; at most 193 UTF-8 bytes, no whitespace/control characters. |
| `expression.apply` | `{"type":"expression.apply","name":"happy","strength":0.7,"duration_ms":1200}`. Existing unreserved expression, finite strength 0–1, duration 1–60000 ms. Names at most 128 bytes; internal spaces allowed. Manual facial channels, blink, gaze and lip sync retain ownership. |
| `catalog.get` | `{"type":"catalog.get"}`. Returns `{"status":"ok","value":{"type":"catalog","motions":[{"motion_id":"org.example.reaction/nod","name":"Gentle nod"}],"expressions":[{"name":"happy"}]}}` inside the result. Bounded to 128 total entries and the record budget; do not assume exhaustive enumeration. |
| `message.deliver` | `{"type":"message.deliver","text":"An external notification"}`. At most 4096 bytes. The host supplies integration attribution; the child cannot choose a user/assistant role. Success requires persisted sink acceptance; hosts without a sink return `unavailable`. Delivery itself is not a request for provider generation. The host user can separately enable **Reply to incoming messages** for the selected character; an accepted delivery can then start an ordinary reply. Acceptance still means delivery only, even if generation cannot start. |
| `message.deliver` | `{"type":"chat.offer","message":{"source_id":"chat-1","author_id":"viewer-1","author_name":"Viewer","text":"Hello"}}`. Candidate admission for host chunking, not history delivery or a reply guarantee. Exact IDs/authors/text stay data. |
| `event.emit` | `{"type":"event.emit","name":"integration.org.example.reaction.notice","data":{"count":1}}`. Names must belong to `integration.<package-id>.…`; data is bounded to 8 KiB, depth 16 and 1024 nodes. `event.emitted` is returned to the originating connection, not broadcast as a cross-plugin event bus. |
| `lifecycle.observe` | Metadata events, target changes and own motion terminal receipts. Does not authorize reading reply text or performing actions. |
| `reply.read` | Private `reply.text` chunks, independently authorized. Hook launch still requires `lifecycle.observe`. |
| `audio.playback` | Experimental private host-to-extension synthesized speech audio. Separate from text/lifecycle grants; no production outgoing route/player is enabled yet. Revoking this grant stops the integration; an explicit restart is required after regrant. |
| `audio.transcribe` | Experimental `audio.input` start/frame/finish/cancel records. Also requires `message.deliver`. The application admits persistent authorized sources through bounded configured-ASR intake; Discord call transport remains unfinished. See [audio boundary](discord-adapter.md#inbound-audio-boundary-not-connected-yet). |

The host reply opt-in defaults off, is saved per character and is never supplied by the child. Automatic replies reuse the latest attributed turn and preserve composer drafts/attachments; they skip local-user evidence intake, triage and incoming-message appraisal. They use existing configured chat/voice routes and read-only lexical recall. No semantic result is consumed for this trigger. Only one focused reply can be active; incoming replies have a global ten-second cooldown in the running app. Busy, loading, configuration and cooldown failures do not retry or roll back accepted context. There is no additional host backlog. The integration must retain its own bounded intake/backpressure policy. This policy applies to all permitted external messages, including alert text, not only Twitch chat.

`message.delivered` carries `target`, `text` and host-authored `origin`; `event.emitted` carries `target`, `name`, `data` and `origin`. Their accepted results correlate by request ID; the event payloads do not contain an action ID. For message persistence, readback after a rename/sync ambiguity confirms visibility, not crash durability: acceptance does not guarantee survival across a crash or power loss. Do not assume these events indicate any downstream provider response or animation.

The lifecycle payloads below live under `message: {"type":"event","event": …}`:

| Event | Payload fields after `type` |
| --- | --- |
| `reply.finished` | `reply_id`, nullable `character`, `outcome`: `completed`, `failed` or `cancelled`. No private reply text or failure detail. |
| `reply.text` | `reply_id`, nullable `character`, `offset`, `text`. Completed reply text in chunks up to 4096 bytes; offsets are UTF-8 byte offsets, not character indexes. Authorized chunks precede the corresponding `reply.finished`. |
| `speech.started` | `playback_id`, nullable `character`, `voice_generation`, nullable `reply_id`. Actual audio playback, not synthesis admission. |
| `speech.stopped` | Same identities plus `reason`: `observed_terminal` or `cancelled`. |
| `emotion.changed` | `character`, `owner`: `studio` or `scene_chat`, `previous`, `emotion`, nullable `reply_id`. Emotion values are the current `AffectEmotion` enum in source. |
| `motion.playback.started` | `playback_id`, nullable `source_target`/`motion_id`, tagged host/extension `source`; animator activation only, not a rendered-frame or avatar-authority receipt. |
| `motion.playback.ended` | `playback_id`, nullable `source_target`/`motion_id`, tagged host/extension `source`, `outcome`; original logical playback context, not current action authority. |
| `extension.stopping` | Source `installation_id`, `session_id`, `process_generation`, typed stop-request `reason`; not exit evidence. |
| `extension.stopped` | Same source IDs, nullable `exit_code`, nullable `requested_reason`; confirmed supervisor termination. |
| `extension.started` | `installation_id`, source `session_id`, source `process_generation`; actual persistent-process launch only. |
| `app.stopping` | `app_run_id`: best-effort host-exit metadata to an already-running persistent observer; not a one-shot hook trigger. |
| `app.ready` | `app_run_id`: UUID for this locally initialized application run. No avatar or provider readiness claim. |
| `character.selected` | Nullable `previous`, nullable `new`, positive `selection_revision`; source selection only, independent of avatar loading. |
| `target.changed` | `target`: `{uuid,generation}` or `null`. |
| `motion.finished` | `target`, `motion_id`, `action_id`, `outcome`, as above. |

There are no `app.ready`, `speech.finished`, `reply.failed` or `motion.started` events in the current protocol. Persistent processes observe new source events, not guaranteed historical replay. Keep source reply/playback identities separate from the loaded target and process identities.

## Structured chat chunks

Persistent integrations can offer individual chat candidates with `chat.offer` using their existing message-delivery grant. IDs are opaque, nonempty strings up to128 UTF-8 bytes without whitespace/control characters; author names are nonempty control-free text up to128 bytes. Text is nonblank and at most2048 bytes; newline, CR and tab are preserved as data, while NUL and other controls are rejected. The final encoded single-message chunk must also fit4096 bytes; oversized candidates are rejected without truncation.

An accepted offer produces no `message.delivered` event. It enters a volatile16-candidate host buffer for up to15 seconds, deduplicated by integration/source ID with a bounded256-ID,60-second cache. Live dedup saturation refuses further admission rather than evicting recent IDs. After four seconds, or when full, the host takes whole FIFO messages from one source that fit a4096-byte JSON chunk. A busy scene/history writer or focused generation defers delivery within the original expiry. Only explicit pre-commit Busy delivery can restore the same chunk; uncertain saves are discarded without retry.

The source must remain the same enabled, permitted persistent process incarnation, installation/settings/grants/credential revision and loaded avatar. Reconfiguration, restart/exit, revoke/disable/removal, target changes or cancelled focused reply clear stale candidates. One-shot hooks retain immediate `message.deliver`; delayed chat offers are for persistent sources. Dedup is in memory and does not promise exactly-once across app restarts.

Delivered chunk text is `{"kind":"chat_chunk","messages":[...]}` with exact source ID, author ID/name and text, inside the existing attributed external turn. The existing per-character incoming-reply opt-in controls generation and memory-intake boundaries. The host can optionally pick up to two original messages using the shared decision-model connection when incoming replies and Pick chat highlights are enabled. The selection stays in the host; no provider credentials are given to children. Unknown/duplicate choices and failures discard the chunk, and authority/deadlines are checked again before saving. See the Twitch setup guide for configuration.

## Saved settings and private credentials

Settings are typed scalar values in `handshake.message.settings`, not arbitrary JSON configuration or shell interpolation. Available definition shapes include:

```json
[
  {"id":"active","label":"Active","type":"toggle","default":true},
  {"id":"cooldown","label":"Cooldown","type":"integer","default":5,"min":1,"max":60},
  {"id":"label","label":"Label","type":"text","default":"","max_length":128},
  {"id":"trigger","label":"Trigger","type":"choice","default":"Reply","options":["Reply","Speech"]},
  {"id":"motion","label":"Gesture","type":"motion","default":"org.example.reaction/nod"},
  {"id":"expression","label":"Expression","type":"expression","default":""},
  {"id":"character","label":"Character","type":"character","default":""}
]
```

Use at most 32 definitions and 16 KiB of complete saved settings. Setting IDs are lowercase slugs of at most 64 bytes. Text is bounded by its declared byte limit, at most 1024; choices have 1–32 options. Motion and expression settings are strings: `""` means None, which your handler should respect by skipping the action. Motion strings follow the 193-byte identifier rules above. Expression strings allow internal spaces, reject controls/whitespace-only nonempty values and reserved blink/gaze/mouth channels, and are at most 128 bytes.

Character settings are plain UUID strings: `""` selects **Loaded character**, and a nonempty value must be a canonical lowercase, hyphenated, nonnil UUID. The searchable dropdown lists library characters and preserves unavailable selections. This setting is a handler filter, not an action target or permission: a handler must compare it with the source character and current host-issued target. It cannot load or retarget an avatar.

Unavailable catalog choices can remain saved across avatar or pack changes; saving a selector does not authorize its use. Handle runtime `unknown_motion`/`unknown_expression`. Updates retain compatible saved values, drop removed fields and use defaults for new/incompatible fields. Saving changed settings replaces an enabled process after it is reaped, with a fresh session; unsaved UI drafts are not sent. Settings never add grants.

Optional `credentials` definitions use objects such as `{"id":"access_token","label":"Access token"}`. Values belong in the separate private Credentials UI, never settings, manifests, protocol logs or source control. Credential values are not included in handshakes or the installation index. Saving/clearing changed credentials replaces the enabled process. There is currently no portable credential-value request over this JSONL protocol; the native bundled Twitch receiver uses the internal Rust credential module. Do not depend on its private storage format as a third-party wire contract.

## Lifetime and resource limits

The host clears the inherited environment, selectively retaining PATH, locale/temp, proxy and certificate variables. It supplies `WAIFU_EXTENSION_DATA` for stable per-install writable data and `WAIFU_STUDIO_EXECUTABLE` for the host executable. It does not automatically forward provider credentials. Write mutable state into the data directory; install updates copy a new package revision rather than modifying your running source folder.

| Boundary | Current limit/behavior |
| --- | --- |
| Manifest/content | 256 KiB manifest; 512 content entries. |
| Process arguments | 64 arguments, 4096 bytes each, 16 KiB total including terminators. |
| Running children | Four shared slots for persistent processes, hooks and manual tests; stopping children occupy slots until reaped. |
| Hook backlog | 32 invocations / 1 MiB globally; queued work expires after five seconds. Loss is reported in runtime status; delivery is not guaranteed. |
| Hook/manual-test lifetime | At most ten seconds; one active invocation/test of each kind per installation. |
| Transport | Bounded queues; 64 KiB records. Drain stdin promptly and avoid stdout bursts. |

Target changes, grant revocation, disable, removal and configuration replacement invalidate owned work. Do not retarget an old action to a newly selected avatar. Hooks stop after their bounded invocation; persistent process failures require explicit Restart rather than automatic retry. Reset for hooks discards current/queued work and waits for new events. Lifecycle gaps/overflow can stop a persistent connection, so tolerate EOF and avoid treating silence as success.

## Install and check a local package

1. Make the entry points executable, validate JSON and test your child with a local pipe-based host harness. Feed the envelopes above with fresh identities; verify LF framing, flushing, rejection handling and ACK/terminal correlation. This needs no provider or network.
2. In **Extensions → Add… → Local integration from folder…**, select the folder containing `extension.json`. The content-only installer intentionally rejects executable packages. Installation copies and validates the folder; editing the source folder later requires reinstalling it.
3. New executable installations start disabled with no grants. In **Permissions**, grant only the requested capabilities you intend to use. Load an avatar, then **Enable & start** or **Enable hooks**. Check runtime status and stderr diagnostics.
4. Save settings before testing. If `test_process` exists, click **Test reaction**. It uses saved grants/settings and the currently loaded target; it does not bypass permissions. A successful process exit/status alone proves neither an accepted action nor visual completion: inspect the correlated action result and motion terminal outcome.
5. Exercise missing grants, no target, unknown assets, manual pause/replacement, target changes and disable during a pending action. Verify no old request moves or messages a newly selected character. For hooks, test the real trigger as well as any separate manual handler.
6. Reinstall the same package ID to test an update and restart the app to check persistence. Existing grants are intersected with the new requests; newly requested grants are never automatic. Disable/remove releases owned work; removal retains package/data files. Do not use retained files as evidence that a removed integration is active.

Useful source-backed examples:

- [lifecycle-observer](../assets/extensions/lifecycle-observer/): persistent metadata observation.
- [javascript-observer](../assets/extensions/javascript-observer/): dependency-free Node.js startup/shutdown observation.
- [startup-observer](../assets/extensions/startup-observer/): observation-only app-ready hook.
- [integration-start-observer](../assets/extensions/integration-start-observer/): observation-only persistent-process start hook.
- [integration-stop-observer](../assets/extensions/integration-stop-observer/): separate stop-request and confirmed-exit hooks.
- [motion-end-observer](../assets/extensions/motion-end-observer/): general observation-only playback outcome hook.
- [selection-observer](../assets/extensions/selection-observer/): observation-only character-selection hook.
- [reply-gesture](../assets/extensions/reply-gesture/) and [hook-gesture](../assets/extensions/hook-gesture/): settings, target checks and waiting for actual motion terminals.
- [emotion-hook](../assets/extensions/emotion-hook/): one-shot `emotion.changed` observation.
- [twitch-replay](../assets/extensions/twitch-replay/): offline attributed-message acceptance followed by a gesture; explicit Restart replays it.
- [twitch-live](../assets/extensions/twitch-live/): native receiver wrapper and optional local test handler. Account authorization/network behavior are separate from offline protocol acceptance.

Authoritative implementation references: [protocol](../src/extension_protocol.rs), [manifest validation](../src/extension_manifest.rs), [settings](../src/extension_settings.rs), [connection/replay handling](../src/extension_connection.rs), [host actions](../src/extension_actions.rs), [process supervisor](../src/extension_process.rs), [runtime](../src/extension_runtime.rs) and [installed-package binding](../src/extension_installed.rs). This guide is source-verified; its examples and suggested checks are not a claim of live third-party or network acceptance.

## Offline restart-pressure check

On Linux with Python3, the repository includes a bounded real-process lifecycle
regression:

```sh
cargo test --offline --lib repeated_restart_disable_and_remove_reclaim -- --nocapture
```

It re-executes that exact test in an isolated test process so unrelated parallel
tests cannot distort its FD/thread measurements. Twenty cycles create forty
actual child incarnations, alternating target changes and transient permission
revocation, discarding queued synthetic reply text at stop, checking fresh-session
receipts, and removing/reaping each owned child. File descriptors and threads
must settle within the small stated allowance after every cycle; focused runs
also report observed baseline/peak/final counts. It makes no network calls and
uses no saved user profile or credentials.

This checks bounded restart pressure and cleanup. It is not long-session native
speech/GPU, live Twitch, compositor recording or cross-platform acceptance.

### Experimental private speech-output boundary

`audio.output` is a host message, not a broadcast event or child action. Its `output` has phase `start`, `frame`, `finish` or `cancel`, with a nonnil UUID `playback_id` and exact target. Frames carry zero-based `frame_sequence` and exactly1920 interleaved PCM16 samples (20ms48kHz stereo); finish carries `frame_count`. The maximum frame count is65536. Unknown fields, non-PCM16 values, nil IDs and malformed lengths are rejected by the normal protocol encoder/parser.

The runtime captures an authorized persistent installation's session, process generation, origin and target. It rechecks those plus current host target, active state and grant on each publication. Hook/manual-test processes cannot receive this route. Transport admission is not a delivery/playback receipt. The current implementation connects bounded ordered delivery, acknowledgements/timeouts, worker playback, and production speech decoding. Calls-only speech uses a device-independent decoder clock. These paths have offline tests; live bot/DAVE calls, remote audibility, echo/disconnect and native user acceptance remain unverified. This private experimental boundary is not a stable public media API.

### Provisional private playback feedback

`audio.playback.receipt` is reserved for the call worker and requires `audio.playback`, independently of `audio.transcribe` or message delivery. Its `receipt` is `{ "phase": "ready", "playback_id": "UUID" }`, `{ "phase": "consumed", "playback_id": "UUID", "frame_count": 1 }`, or `{ "phase": "finished", "playback_id": "UUID", "frame_count": 1, "outcome": "completed" }`. Finished outcomes also include `cancelled` and `failed`. Unknown fields/outcomes, nil UUIDs, zero consumed counts and counts above65536 are rejected. The outer action supplies the normal host-assigned session/process/target identity.

The app accepts feedback only for an exact active host-owned SpeechOutputs route; unknown sources are rejected. The worker constructs playback and sends receipts only under the separate playback grant. Development bundles support receive, playback and duplex grant sets; receive remains the builder default. Host correlation, bounded flow control and production speech source/clock routing are connected. Routing settlement distinguishes completed, partial, stopped, source failure and provider failure. Those are host routing outcomes, not additional wire receipt phases. Never interpret consumption or a completed SDK player as proof that another person heard the audio.


A playback-capable persistent service announces `{ "type": "audio.playback.available" }` once its media output is ready for the current outer target. Wait for the accepted result before consuming a Start. Readiness is scoped to the host-authored process/session/target and is invalidated on target/incarnation/authority changes. The playback grant alone permits the private `target.changed` control for eligible services; it does not authorize general lifecycle or reply observation. Hooks and manual-test invocations are not eligible playback services. The default Discord development bundle requests receive-only capabilities; `--mode playback` requests only playback, and `--mode duplex` requests both direction sets. A playback-only worker does not construct participant capture or ASR.


### Configure and check call output

Build a development Discord voice bundle using the [worker guide](../tools/discord-voice/README.md), then install it from **Extensions → Add → Local integration from folder…**. Review its requested permissions, fill in its bot/server/voice-channel setup, and explicitly enable it. Installing a bundle does not connect an account. Only enter credentials into the app's existing private credential field.

In **Voice & mic**, choose **Speech output**:

- **Local + calls** (default): new speech uses local speakers and ready call outputs.
- **Local only**: new speech uses local speakers.
- **Calls only**: new speech requires a ready call output and does not fall back to local speakers.

Previews remain local in all three modes. Changing output affects new utterances; it does not reroute speech already in progress. The selector uses the character's existing voice service and configured credentials; it adds no direct OpenAI account requirement.

The Voice page explains selected-character readiness and offers an **Extensions…** shortcut when call setup is missing. The installed integration card has separate process and **Call output** status: a running process alone is insufficient. **Ready** requires current playback permission, a current active process and an acknowledged media route for the selected avatar instance. Character replacement, process restart or permission loss invalidates that route. **Not ready** guidance identifies the next permission, enablement, setup or character step. Missing runtime resources are shown as unavailable.

These controls and offline receipts do not establish that a remote participant heard speech. Keep live sign-in, native device/interaction, DAVE compatibility, audibility, echo and reconnect checks separate from package and protocol verification.
