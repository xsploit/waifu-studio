# Extension contract draft

2026-10-20. **Integration design draft, not a stable public executable-plugin API.** Content-only installation and the Extensions page now exist; see [content packs](content-packs.md). It establishes the boundaries requested by the implementation plan; API v1 will be published after content-pack, hook and process-runtime checks. Existing Bevy plugins are compiled into the app. The MCP client remains a separate explicit tool interface.

## Current implementation and author entry point

Start with [Authoring local process and hook integrations](extension-authoring.md) for source-verified manifest, wire, permissions and lifecycle examples.

This document retains the original proposals and dated implementation receipts. Earlier example envelopes and event tables are design history, **not the current wire specification**. Use `src/extension_protocol.rs`, `src/extension_manifest.rs` and the bundled handlers as authoritative; do not copy the draft envelopes below into a plugin.

Current executable support includes supervised processes, one-shot hooks for `reply.finished`, `speech.started`, `speech.stopped` and `emotion.changed`, typed settings, private credentials, explicit saved-reaction tests and attributed external messages. Content packs and the Extensions page are implemented. The Twitch receiver uses EventSub; explicit browser device authorization is implemented; host-owned token refresh is implemented for device-authorized credentials; live-account acceptance remains outstanding. Execution support is currently Linux. This API remains an integration checkpoint rather than a stable published compatibility promise.

## Ownership and architecture

The host owns character selection, conversations, playback, facial channels, assets and saved settings. An extension receives versioned events and submits requests to host action services; it does not receive mutable Bevy world access. A persistent integration runs in a supervised external process. A small hook runs a handler for matching events. A content pack needs no executable process.

Start with local stdin/stdout JSON Lines to support Python, JavaScript, Rust and other languages without a network listener. Keep logs on stderr. Each extension instance has its own installation ID, package directory, writable data directory, configuration and lifecycle generation. A blocked, exited or disabled process cannot hold the renderer open. Reconnecting external tools can be added through another transport later using the same envelopes.

Motion orchestration now lives in host-owned `MotionServices`, advanced from ordinary Update. UI controls bind the same shared controller handles. UI controls and extensions must use the same request/ownership path. Source starting points are `src/ui.rs` (`manual_motion`, `reply_motion_frame`, `companion_motion_frame`), `src/companion_motion.rs`, `src/motion_decision.rs`, and `src/animation.rs`.

### Body and facial priorities

- Explicit manual motion controls hold until Resume automatic animator. Replies and extensions cannot silently release that hold.
- Desktop drag/seating owns the body while active. Replies arriving during that state are consumed rather than replayed on release.
- A permitted reaction is one-shot. When it finishes, choose talking if the target is still speaking, otherwise an idle. A short reply can finish before its reaction does.
- Talking begins with actual audio playback for that character, can bridge bounded chunk gaps while speech remains pending, and ends after playback and pending speech end. Generation alone is not playback.
- Ambient idle/talking selection runs only while higher-priority owners have relinquished control. Character/avatar changes invalidate stale requests and speech state.
- Expressions use temporary named contributions. Manual expression channels, blink, gaze and lip sync keep their ownership; an extension cannot persistently replace them by asking for a reaction.

These rules apply equally to core reply behavior and plugin requests. Events report what happened; they are not instructions granting permission to bypass ownership.

## Package draft

`extension.json` is the proposed root manifest. Required metadata: `api_version`, stable namespaced `id`, `name`, package `version`, description and content provenance/license where applicable. Optional sections: content, a process entry point, hooks, requested host capabilities, event subscriptions and setting descriptions.

Content entries have stable IDs scoped by package ID. Categories: character assets/personas; motions and idle/talking/emotion playlists; expression mappings; room/background assets; and presets for existing settings. Asset references stay within the package and are resolved by the host. A manifest cannot create a new renderer feature or procedural room implementation by naming it. Unsupported content fails with a readable compatibility error.

Executable entry points use an explicit program and argument array, platform overrides when necessary, and a defined working directory. Configuration is data and is never interpolated into a shell command. The host supplies package/data locations and only the environment/configuration explicitly needed by that instance. Provider operations reuse configured host connections; plugin authors do not need a separate direct OpenAI credential.

Installation must validate the complete package before activation. Updates retain the installation ID and user settings; disable/removal releases owned contributions and uses existing asset fallbacks. Shared source assets and user-authored settings are not deleted. Content migrations and executable updates are separate from silently overwriting a running package.

External processes execute local code and are **not OS sandboxes**. Declared capabilities authorize host API calls and event payload access; they do not restrict all filesystem/network operations a process could perform. The installation UI must describe executable packages truthfully.

## Event envelope draft

```json
{
  "api_version": 1,
  "type": "event",
  "id": "evt-1042",
  "name": "speech.started",
  "character_id": "c7be74ab-0e9f-432e-a350-bb76189bec48",
  "generation": 17,
  "sequence": 1042,
  "origin": "host",
  "data": { "speech_id": "speech-31", "reply_id": "reply-9" }
}
```

Names and fields are provisional. Sequence numbers order events within a host session; IDs correlate actions and results. A session identity must accompany the runtime handshake so reconnects do not mistake a new session for old sequence numbers. `generation` belongs only to the selected character/avatar instance and changes when that instance is replaced; starting or ending speech, replies or motions does not change it. Playback identities use `speech_id`, `reply_id` and motion request IDs. Extension instance generation separately invalidates output from a replaced process. A short-lived handler receives the triggering event through stdin, not through command interpolation.

| Event | Host meaning and emission point |
| --- | --- |
| `app.ready`, `app.stopping` | Startup completed / bounded shutdown notification. Shutdown does not wait indefinitely for handlers. |
| `extension.started`, `extension.stopping` | Configure a specific instance and revoke its requests/contributions on stop. |
| `character.selected` | Selected identity actually changed; include avatar-loading status rather than implying the new VRM is already visible. |
| `avatar.ready`, `avatar.failed` | Production loader outcome for the requested character/generation, distinct from previous visible avatar. |
| `reply.finished`, `reply.failed`, `reply.cancelled` | Source reply completion/cancellation, once per reply ID; success/failure are distinct. Text is included only for subscriptions with chat-read access. |
| `speech.started`, `speech.finished` | Playback start / final consumed playback end, with character and speech IDs. A synthesis job or queued decoder is not sufficient. Stop/error/cancel supply a reason. |
| `emotion.changed` | Validated host affect for a particular character and source reply; include the source rather than presenting an inference as a user fact. |
| `motion.started`, `motion.finished`, `motion.cancelled` | Accepted host motion lifecycle with request ID and owner; natural completion differs from replacement. |
| `integration.<package-id>.<event>` | Validated package-scoped external events such as a Twitch subscription. Never impersonate `host` origin. |

Keep hook matching simple: event names plus optional exact payload/character matches. No arbitrary synchronous model evaluations or script callbacks in per-frame rendering. Lifecycle events must be emitted at their source; sampling one mutable “last reply” field can lose two completions between frames.

Current source anchors: `Studio::select_character` and `poll_studio` in `src/studio.rs`; loader requests/outcomes in `src/loading.rs`; `poll_voice`, `play_voice_event`, `play_clip`, `SpokenAudio`, `SpeechPlayback` and `SpeechMeter` for actual playback identity; `AffectLibrary::apply_reply` in `src/reply_affect.rs`; clip selection/completion in `src/animation.rs`.

The stage 1 implementation now routes every production audio spawn through `spawn_speech`, attaching an immutable character/voice-generation snapshot and a metered-source playback receipt. The receipt starts with decoder sample consumption (including silence), ends at EOF/drop, and is invalidated immediately by meter cancellation or replacement. Idle/talking orchestration matches both source ownership and the current utterance; it bridges pending chunk gaps for at most 750 ms. These are internal source primitives, not implemented extension events. Public lifecycle emission, reply/speech IDs, cancellation reasons, and correlation with avatar/target generations still need implementation and verification before API v1.

## Action envelope draft

```json
{
  "api_version": 1,
  "type": "action",
  "id": "celebration-12",
  "character_id": "c7be74ab-0e9f-432e-a350-bb76189bec48",
  "generation": 17,
  "action": {
    "name": "motion.play",
    "motion_id": "waifumations.starter/celebrate",
    "mode": "one_shot"
  }
}
```

| Action | Required behavior |
| --- | --- |
| `motion.play` | Resolve a registered asset ID; validate target/generation and playback owner. Return a busy/manual-hold error or accept a one-shot with normal crossfades. Do not force an arbitrary file path or locomotion into standing ambient playback. |
| `expression.apply` | Resolve an available expression/mapping, finite bounded strength and duration. Contribute temporarily; restore on expiry, cancellation or disable. |
| `message.deliver` | Bind to the explicit `character_id`/`generation` in the action and reject a stale target; never redirect queued messages to a newly selected character. Deliver an attributed external message through the existing conversation pipeline. Keep source metadata separate from user-authored facts. Respect provider state, bounded queues and user-configured cooldowns. |
| `event.emit` | Publish only namespaced external events permitted for that instance; retain origin and causal ID for deduplication and feedback-loop protection. |
| `catalog.get` | Return readable characters, registered motions, expressions and supported capability IDs for dropdowns and authoring. Do not reveal provider credentials. |

An immediate result reports acceptance/rejection with the same action ID. Later lifecycle events report actual start/completion; accepted is not synonymous with played. Errors include unsupported version/action, unknown asset, stale target, manual hold, desktop ownership, disabled capability, queue full and timeout. Define idempotency per instance/session so reconnect/retry cannot replay a celebration or duplicate a chat message.

Chat-read, external-message delivery, motion requests, expression requests and namespaced event emission are separate host capabilities. Requesting avatar control does not grant private chat history or durable-memory writes. Core memory acceptance/corrections remain in their existing reviewed path.

## Process lifecycle and limits to prove

Handshake/version negotiation → instance configuration → catalog/subscriptions → events/actions → results → stop/restart. Before freezing v1, select and document limits for line size, queue depth, outstanding actions, hook duration, process shutdown and restart backoff, then test each failure boundary. Partial lines and stdout floods must not create unbounded buffers or block the main thread. Repeated failing handlers have visible status and a bounded recovery policy.

Disabling or replacing a package invalidates its generation, drops late output, cancels pending work and removes owned temporary contributions. The host continues its own idle/talking behavior. Keep package settings and credential references independent of executable lifetime.

## User experience and first acceptance fixture

One Extensions page supports install, configure, status, enable/disable, restart and removal. Manifest-described settings use the existing compact settings kit. Event-to-action mappings use character/motion/expression dropdowns and a Test button; raw messages and developer logs stay under Advanced.

The first real integration is Twitch: a package receives a real subscription/channel event, emits a namespaced event, and requests a reviewed celebration motion/expression. It must also support deterministic local replay. Chat intake and subscription delivery have different Twitch requirements; choose the current transport/auth flow during implementation. Core animation code contains no Twitch branch.

Acceptance also includes an owned content-only pack, a small hook, persistent process crash/restart, blocked/flooded output, malformed/incompatible messages, stale targets, manual hold, stop mid-gesture, character switch, disable/uninstall and resource stability. Fixture acceptance and live Twitch service acceptance are separate. Discord/media and other transports extend proven requirements later.

The packaging/hook inspiration is the [official OpenAI plugin documentation](https://developers.openai.com/plugins/build/plugins). This is a Waifu Studio protocol; no Codex plugin binary, event-schema or marketplace compatibility is implied.

## Current foundation checkpoint

Content manifests/installation are implemented. `extension_protocol.rs` and `extension_session.rs` provide typed integration-checkpoint envelopes, request sequences, bounded authorization/replay handling and separate session/process/target identities. `extension_process.rs` provides Linux-only bounded stdio supervision; other platforms return a readable unsupported error. Its authoritative terminal receipt remains available after output queue overflow. Ordinary handle drop only signals cancellation; the future runtime must call bounded shutdown outside render-frame teardown. Executable manifests remain rejected until host actions, source lifecycle emission, configuration and examples work end to end. None of those foundations alone establishes a live public API, Twitch or Discord support.

## Host-action bridge checkpoint

`extension_connection.rs` joins process supervision and admission/replay with bounded per-frame dispatch. `extension_actions.rs` executes against host-resolved registrations and shared animation/behavior controllers. `HostActions::poll_connection` rechecks identity/permissions, returns results, and revokes owned effects when the connection stops. Callers must reconcile before ownership transfers; desktop playback explicitly cancels owned requests before parking animation state. Motion and expression cancellation use opaque request tokens, not file-path equality.

Catalog responses are bounded by entry count and serialized size. Temporary expression ownership is separate from host emotion/lifecycle, and finished owners are reclaimed. Message delivery remains Unavailable until typed source attribution survives persistence, display and memory intake. Speech consumption is latched independently of terminal state; retaining/draining complete lifecycle receipts and routing subscriber events remain unfinished.

A real Linux child-process fixture drives and releases the shared expression controller. Executable manifests remain rejected and the app does not yet instantiate a persistent executable runtime. This checkpoint does not establish public API stability, native visual playback, Twitch, Discord or platform parity.

## Source events and app runtime checkpoint

The normal app advances `ExtensionRuntime` from ordinary Update after source reply/speech polling and behavior configuration, before motion orchestration. The empty runtime avoids catalog construction. The app retains a clone outside its runner for one total bounded process-shutdown deadline. Explicit restarts wait for an authoritative termination receipt and receive a fresh session/process incarnation. There is no automatic crash retry. Runtime-entry status, enable/disable and Restart use compact existing Extensions cards.

| Grant | Data authorized |
| --- | --- |
| `lifecycle.observe` | Captured reply outcome, speech consumption/terminal metadata, current avatar-target changes. No reply text or provider error content. |
| `reply.read` | Full completed reply text in bounded UTF-8 chunks with reply ID and byte offset. Does not imply lifecycle subscription or memory writes. |

Source ownership (character/reply/playback/voice generation) remains distinct from current avatar targets. Reply text precedes terminal metadata when both grants are present. Started means consumed decoder samples including silence; stopped means an observed terminal receipt or explicit cancellation after actual invalidation. Neither establishes physical speaker drain. An unconsumed source can terminate without a started event. Source records survive entity destruction; cancellation does not erase prior consumption or relabel an already-terminal observation.

Source journals and per-connection queues have finite record/byte/work budgets. Explicit loss records stop affected subscriptions; no indefinite lossless-delivery claim is made. Revocation permanently removes queued private text, including revoke/regrant between frames. Disable discards private backlog and owned effects; controller revocations are latched until applied even if restored before Update. See VALIDATION.md for exact limits and current receipts.

Executable manifests remain rejected until their explicit installation/configuration path binds validated packages to this manager. Message delivery remains Unavailable. Source events for motion completion/emotion changes, event-matched one-shot hooks and live Twitch/Discord remain unfinished. The rendered content-page capture does not contain executable runtime entries or prove native input.


## Installed local-code integration checkpoint — 2026-10-20

The explicit local-integration installer now accepts one bundled persistent program, strict known capabilities and contained program/working-directory paths. Ordinary content installation still rejects executable metadata. Hooks remain rejected. Unix copying preserves executable access while stripping special permission bits. Code is not OS sandboxed.

The durable index records trust type and granted capabilities with compatible defaults for earlier content indexes. New executable installations start disabled without grants; updates retain identity, enabled state and only still-requested grants. Changing content/executable trust type requires removal and explicit reinstallation. Writable data is stable per installation, separate from retained revisions. Worker-side validation prepares it without frame-thread metadata writes.

The ordinary app Update reconciles enabled installed revisions to the supervised runtime. Disable/removal/replacement cancels the previous owner and waits for authoritative termination before reusing its ID. Capacity-blocked starts wait for one of four process slots; failed executable attempts do not automatically retry. Permission changes reach the connection and revoke owned effects. Compact installed cards share saved enablement, process status, Restart and folded Permissions/Details.

An owned Python 3 lifecycle-observer example records one replaced metadata receipt in its data directory. It never requests reply text or avatar control. General manifest-defined configuration fields, credential references, one-shot hooks, attributed message delivery, motion/emotion source events and live services remain unfinished. This supersedes earlier checkpoint statements that executable installation is rejected.


## Typed settings and the next hook boundary — 2026-10-20

Local integration manifests now describe up to 32 plain toggle/integer/text/choice settings. Strict schema/default checks, complete-value validation and bounded migration keep values separate from grants. The installed index saves effective values atomically; updates migrate against the copied revision, retaining compatible values and defaulting incompatible or removed fields. Values are not secret storage and credential references remain unfinished.

The startup handshake carries `message.settings`. Missing settings deserialize to an empty map for earlier draft messages; typed values and the 16 KiB wire budget are validated before process launch. Saving changed settings replaces the process with a fresh session after termination. Equivalent saves do not restart it; manual restarts retain configuration. Draft controls have explicit Save/Reset actions and inline validation.

The following hook design is **proposed, not executable support**. Start with typed reply-finished and speech-started/stopped triggers, and initially allow either persistent-process or hook mode per integration. Queue at most 32 invocations/1 MiB, with five-second queue expiry and process lifetime, one launch/frame and one invocation active per installation. Persistent processes, running hooks and terminating children must share the four-process budget.

Triggering requires lifecycle observation; private reply text requires separate read permission. Capture source identity and target at enqueue, permanently restrict captured payloads after permission revocation, recheck grants before launch, and discard replaced configurations. Keep current manual and desktop ownership rules for actions. Show overflow, expiry and child failure; never automatically replay. A hook requesting actions must await its acknowledgement; child exit cancels owned temporary effects. Hooks with effects that outlive the process need an explicit separate ownership contract first. Motion-finished source events are still missing, so an animation hook cannot yet reliably wait for playback completion; implement that receipt path before claiming animation-hook acceptance.


## Correlated motion-terminal receipts

Owned animation requests have independent retained receipts with pending/completed/cancelled/failed outcomes. Natural completion is recorded at the one-shot terminal frame; manual takeover, stop or ownership transfer records cancellation; asynchronous import failure records failure. A later idle selection cannot relabel a terminal outcome. The animation state retains only current/import slots, while the host holds bounded independent receipt clones.

The draft `motion.finished` event now includes the original `action_id`, registered `motion_id`, captured `target` and terminal `outcome`. The host delivers it once to its original active session/process with lifecycle observation permission; a disappeared or replaced motion is not inferred to have completed. Disconnected processes cannot receive receipts. Rejected requests retain their ordinary action result rather than receiving a fabricated playback event.

The owned Reply gesture persistent integration maps source reply/speech events to a registered motion and waits for this correlated terminal event. It is an event/action example, not implementation of manifest-launched one-shot hook processes.


## Implemented one-shot hooks — 2026-10-20

This supersedes the earlier proposed-hook and rejected-hook checkpoints. The integration installer accepts either a persistent process or 1–3 unique reply-finished / speech-started / speech-stopped handlers, with shared contained-path validation. Hook registration consumes no child slot. Runtime admission shares four slots, including terminating children; queue bounds are 32 invocations / 1 MiB, five-second queue expiry, ten-second child lifetime, one launch/frame and one active invocation/install.

Captured events retain source identity and target. Observation grants gate triggers; read grants gate private text. Permission intersections are permanent for queued captures and active invocations cannot gain launch-time capabilities. Target/configuration changes discard work. Gaps stop subscriptions pending explicit reset; expiry, overflow and launch errors do not replay. The Reply gesture hook sample waits for the actual correlated motion terminal receipt before exit. Emotion triggers, attributed message delivery and live Twitch remain unfinished.


## Attributed external messages — next implementation boundary

`message.deliver` remains unavailable until external origin survives storage, display, provider projection and memory intake. Implement persisted external entries first, participating in the next ordinary request rather than automatically sending a reply. Host-generated message identity and host-owned integration origin must be typed metadata, never inferred from message text. Preserve legacy histories and count attribution bytes when trimming.

Deliver through a host sink independent of local-user submission. Reject while focused or scene generation owns a history snapshot; validate permission, target and bounded content immediately before accepting. Persist before success or `message.delivered`; resolve post-rename sync errors by checking the host-generated identity so an already-committed message cannot be appended again. Keep duplicate request admission once-only.

UI must display external origin even for text impersonating a user or system. Provider projection must use supported roles and an escaped third-party data envelope, with host guidance that external text is not a statement from the local user. Storage-only metadata must not leak as unsupported provider message fields. Delivery must never enter local-user fact extraction, triage or reflection evidence.

Required checks: legacy/reopen attribution, no candidate from external first-person claims, supported-role provider fixtures, once-only delivery, stale target/grant/busy rejection without mutation, write failure and post-rename ambiguity, and attribution labels independent of text. Then implement owned Twitch replay fixtures before live service acceptance.


## Emotion source transition checkpoint — 2026-10-20

`emotion.changed` is now bounded observation metadata for persistent integrations, gated by lifecycle observation without private reply-read permission. It carries source character, explicit Studio/SceneChat affect-store owner, previous/current enum labels and an optional source reply ID. Scene appraisals currently have no reply correlation and emit None rather than inferring one. No action target or text is included.

Tracked affect application reports only committed label changes. Focused appraisal and reply metadata record after existing stale/config checks; scene acceptance records after recipient/config checks. UI neutral/source resets and scene state discard/cancellation publish neutral transitions for nonneutral labels. Same-label VAD updates, failed/stale results and per-frame decay emit nothing. The event describes affect state, not guaranteed visible avatar expression. Hook trigger support remains the original three reply/speech triggers.

Hook overflow/expiry loss counts now survive subsequent launch status, child completion and Reset; removing/reconfiguring a registration clears its diagnostics. Explicit cancellation is not counted as queue loss.


## Attributed external delivery implemented — 2026-10-20

This supersedes the earlier Unavailable/next-boundary descriptions. A host message sink rechecks permission, session/process identity and current target through the ordinary action path. The binary adapter rejects focused/appraisal activity, scene activity and still-running cancelled scene history writers. New deliveries compare visible and saved history before mutation. An unconfirmed external write quarantines focused intake until successful explicit reload, including normal sends before their memory/composer/generation work.

Conversation turns retain optional validated host-generated message IDs and integration origin, under an external role. Legacy JSON omits the new field. Trimming accounts for attribution; malformed or duplicate external identities are rejected before overwriting histories. The save helper reads back the exact candidate after an error without retrying; readback confirms visibility, not crash durability when directory sync failed. Writes are currently synchronous bounded filesystem work; render-frame latency has not been benchmarked.

The provider uses explicit role/content DTOs. External records become escaped JSON data under a supported role plus host guidance; local-only request bytes stay unchanged. Attachment indices refer to the projected latest genuine local-user message. External delivery never invokes fact extraction/triage and starts no generation. UI labels external entries independently of their text.

The owned Twitch replay sample exercises one synthetic subscription/cheer/chat example, persisted external delivery and a registered one-shot nod. It pins the original target, suppresses duplicate delivery acknowledgements, waits for the actual motion terminal event and never reconnects/retries automatically. Restart is an explicit replay. No live Twitch account, network source or authentication is implemented by this sample.


### Native Twitch receiver and private integration credentials (implementation checkpoint)

Manifests may declare bounded credential IDs/labels. Per-install secrets live in a separate owner-only file; settings, indexes, handshakes, status DTOs and logs contain no saved values. UI replacement drafts are masked and cleared after submission. Changed credential revisions replace the running process; no-op updates retain it. Removed definitions are hidden immediately and pruned on explicit save.

The owned Twitch integration launches the same executable in early worker mode through a tiny Python 3 launcher. Its process environment is cleared and receives the executable/data paths and explicit transport allowlist, without provider credentials. The native receiver validates the supplied user token and selected scopes, uses official bounded HTTP/EventSub endpoints, and receives chat/subscription/cheer events. Proxy selection is shared with existing transport without direct fallback or Fish initialization headers. External messages remain third-party context and do not initiate AI replies. Celebration requests retain actual motion completion correlation.

This checkpoint implements a receiver, not completed live-service acceptance. Device authorization and token refresh remain outstanding. Offline/local-network regressions and rendered UI captures do not establish Twitch account operation, native visual playback, or cross-platform execution.


### Emotion one-shot hooks

`emotion.changed` is now supported by manifest-launched handlers as well as persistent observers. A package may declare all four unique reply/speech/emotion triggers. Capture retains original character, affect-store owner, labels and optional genuine reply ID under observation permission, without reply text or a current action target. Runtime registration uses the manifest hook bound. Existing queue/process/expiry/grant/target rules apply unchanged. The owned Emotion hook example records metadata and exits; its UI entry is on the same Extensions example menu.


### Registered-motion configuration

Plain settings add a `motion` selector whose wire value is a registered ID or empty for None. Its 193-byte, whitespace/control-free bound matches MotionPlay rather than the shorter ordinary action ID bound. Schema/default/migration validate the value shape; actual catalog membership remains an action-time host check. This lets disabled or temporarily missing pack selections survive unrelated settings saves and updates.

Extensions renders compact searchable, virtualized choices from the same enabled/resolved pack content as ActionCatalog, with pack-qualified human labels and retained unavailable selection state. Saving a choice never grants capabilities or enables packs. Twitch's `celebration_motion` consumes this value; absent preserves its original nod, explicit empty suppresses only gesture requests and does not consume cooldown or block attributed message delivery. Other reply/one-shot gesture examples use the same selector rather than script edits.


### Avatar-expression configuration

Plain settings add an `expression` selector: exact avatar name or empty for None,128 UTF-8 bytes, nonblank without controls, excluding reserved blink/mouth/gaze names. Internal spaces and casing remain intact. Availability is checked at action time, not during persistence/migration. Extensions receives FacialState's available names, renders bounded searchable choices, and retains unavailable selections across avatar changes.

Twitch optionally requests a temporary expression at75% for2000ms, then the selected motion after expression acceptance. It tracks distinct Message/Expression/Motion stages, rather than treating every acknowledgement as a delivery result. Only a correlated actual motion terminal finishes the motion stage. One cooldown admission covers the full reaction; expressions and motions retain independent grants. Expression defaults to None; newly requested expression permission remains ungranted on updates. Existing host-owned temporary tokens handle decay, permission revocation, target changes and disconnect without replacing manual facial state.


### Explicit manual test processes

Executable manifests may optionally declare `test_process` alongside a persistent process or hooks. It uses the same contained executable validation and copied package boundary. InstalledRuntime registers its origin, grants, saved settings, data directory and bounded lifetime only after accepting an enabled installation. This does not start a test automatically.

An explicit Test reaction click launches a fresh separate session pinned to the latest loaded target, sharing the four-child budget. Tests use the existing protocol, SessionGate and HostActions rather than direct UI controller writes. Per-install single-test admission includes reaping children. Parent stop/removal/reconfiguration, grant loss and target changes invalidate tests; restart is explicit and never automatically restarts a test. Terminal status is retained on the parent card. The owned Twitch test handler emits only selected expression/motion actions, without network, credentials, fake lifecycle events or external messages.


## Host-owned OAuth refresh checkpoint

Validated OAuth account/scopes/expiry metadata is private credential-file data, not a protocol field or public status. The host’s single store owner prepares a bounded refresh job for an enabled Twitch install nearing expiry. One global authorization slot covers device consent, refresh and cancelled workers until actual completion. A rotating grant is not retried automatically after an uncertain result. New token identity must match the stored user and application and include the saved event selection’s scopes.

The guarded commit atomically replaces the token pair and metadata only if the full installation snapshot and credential revision remain current. Manual value changes invalidate OAuth metadata; no-op saves retain it. Successful rotation uses normal credential revision reconciliation to restart the enabled receiver, with a possible service gap/cache reset. The child remains receive-only and never writes credentials. Offline evidence does not establish live refresh or uninterrupted delivery.
