# Discord voice development dependencies

This isolated development folder pins the supported bot voice stack. It is not an installed extension, does not sign in, and does not join a Discord channel. No app setting or audio consumer changes when these tools run.

`package-lock.json` pins dependency integrity. Installation requires public registry access and the native Opus install step; dependencies are intentionally excluded from Git. Use Node22.12 or newer. From this folder:

```sh
npm ci
node check.mjs
npm test --offline
```

The check and tests perform local codec/DAVE operations only. Do not supply credentials. `node check.mjs --dependencies` also displays the voice library's dependency report.

`VoiceCodec.encode` accepts exactly20ms of48kHz stereo PCM16 and produces Opus. `decode` accepts already-decrypted Opus and uses libopus's16kHz mono decoder, preserving complete320-sample host frames. Short packets accumulate less than one frame; packets up to60ms can emit up to three frames. Encoding/decoding and initialization errors use fixed messages. Close each instance when its participant/session ends; native storage is reclaimed by Node's native-object lifecycle. Codec instances are not session ownership, decryption, endpointing or transport implementations.

The first local trial used `opusscript`, which failed when several codec objects coexisted. The pinned native `@discordjs/opus` backend passes that case; `opusscript` was removed. This is a local observed result, not a universal upstream compatibility claim.

Remaining work includes bot/Gateway quota coordination, DAVE voice connection lifecycle, receive routing and backpressure, outgoing host media permission/protocol, shared decoding of existing TTS streams, stop/mute/echo handling, supervised package integration and real call acceptance. In particular, `StreamingVoice` currently allows one claimed decoder; a second independent call decoder would steal or fail to consume speech. Do not duplicate provider synthesis to bypass that ownership requirement.

## Receive bridge

`receive-bridge.mjs` supplies the receive-side host protocol state machine without sockets or credentials. Construct it from a real host handshake, an `emit(record)` callback and the known bot user ID. It requires audio-transcription, external-message and lifecycle-observation grants, then waits for a usable target event. `begin`, `packet`, `end`, `cancel`, `setMuted`, `host` and `tick` accept monotonic milliseconds supplied by the transport. `packet` accepts only already-decrypted Opus from the owning participant. A transport must stop that subscription when `receiving` becomes false; `active` also includes waiting for ASR completion. Forward every bounded host record to `host` in order and drive `tick` while idle.

The first nonbot speaker owns a turn; overlapping speakers are refused, not mixed or queued. There is a four-request credit window and a32-frame total queued/inflight limit. Start must be acknowledged before frames; Finish waits for every frame acknowledgement. The final short packet is padded with silence to preserve its samples. Invalid audio, overflow or mute cancels the whole turn. Target replacement discards old audio and late responses without retargeting. Writes/results with uncertain outcomes never replay; missing acknowledgements stop the bridge after10 seconds. A refused Finish is cancelled rather than retried. Recording/transfer has a35-second deadline and submitted completion an80-second absolute turn deadline. The codec/host's30-second sample limit still applies.

Private completion metadata is correlated with turn and target; it exposes only the latest unread outcome through `takeOutcome`. Cancel after an accepted Finish waits for the host's correlated result. Dropping target authority can prevent a completion receipt. No result is proof of playback or generated reply. `emit` must serialize/write the supplied record once and must not recursively call back into the bridge. A thrown write is uncertain and stops it; backpressure must remain bounded by the credit window.

This state machine is now attached to the supported receiver API through receiver.mjs, but is not yet a supervised runnable package. Actual DAVE connection lifecycle, live endpointing acceptance, outbound-speech echo suppression, I/O framing, Identify coordination, outgoing audio and live service acceptance remain open. Its local tests use real Opus packets and a synthetic host, not a Discord call.

## Receiver binding

`receiver.mjs` now attaches that bridge to the supported voice receiver API. It requires an already Ready connection and an idle, unstopped bridge. The connection owner must establish the intended bot/channel/DAVE connection and retain connection-level error handling. Ready is not an independent DAVE negotiation proof. This binding does not log in, join, rejoin, fetch participants or destroy the shared connection.

Provide a synchronous cached `lookupParticipant(id)` returning `{id, name, bot}` with an explicit boolean bot flag, plus a monotonic millisecond clock. A speaking-start event creates a subscription only after the bridge admits that known participant. Existing foreign subscriptions are left alone. Streams use the library's AfterSilence behavior (600ms default,250–2000ms bounds) and flow immediately; the eight-packet stream high-water mark is not a hard UDP memory limit. Host transfer remains bounded by the bridge's frame/credit budget; receiver streams are destroyed rather than paused when ownership ends.

The I/O driver calls `binding.host(record)` and `binding.tick()` while idle. Target changes, stream errors, malformed audio, mute and non-Ready connection states detach the owned stream and cancel work. A normal silence end calls Finish. Old stream callbacks cannot feed a replacement. `close()` removes owned listeners/subscriptions without taking over foreign ones; keep the host I/O pump alive long enough for cancellation acknowledgements/deadlines. Reconnect is explicit and must wait for old turn settlement or use a new supervised process incarnation.

Pass retained `muted` and `outputSpeaking` state when reattaching. `setMuted` and `setOutputSpeaking` combine independently, so playback suppression clearing cannot clear a user mute. The latter still needs wiring to actual outgoing-call playback receipts; it is not evidence that acoustic echo suppression is complete. Silence timing was tested with the actual library's AudioReceiveStream, while connections, participant cache and host receipts were local fixtures. Live encryption, call receive, reconnect, stdio supervision and outbound speech remain unverified.

## Host pipe framing

`host-io.mjs` accepts binary Node input/output streams and synchronous record, tick and stop callbacks. Its fixed64KiB input buffer handles fragmented UTF-8 and several JSONL records per chunk. Invalid UTF-8/JSON, oversized lines, omitted/duplicate handshakes, partial EOF and callback failures stop once with fixed reasons. The first handshake has an eight-second deadline; an unfinished line has an absolute five-second deadline that drip input does not refresh. Full session/target/grant validation still belongs to ReceiveBridge; a future startup driver must validate before authentication.

Output has32-record/64KiB outstanding limits including in-flight writes. Backpressure queues records without rewriting them; write failure or ten seconds without progress stops the pipe. A20ms unreferenced timer drives idle deadlines and synchronous onTick work, or tests can explicitly tick. Stop removes owned listeners, pauses input and discards unsent records, while already attempted writes remain uncertain. Caller-owned streams are not destroyed; the process driver must close connection resources and terminate/reap as needed. Rejected callback promises are observed, but asynchronous record/tick callbacks are refused rather than accumulating unbounded startup work.

Local tests exercise framing, deadlines, output budgets and errors. A real credential-free child fixture uses HostPipe and ReceiveBridge to send Start/one decoded frame/Finish over OS pipes, waits for correlated completion and exits on EOF. Early EOF has a distinct fixture status to distinguish it from import/spawn failures. These child tests require subprocess I/O permission in a sandbox; a generic failed child is not a passing early-EOF receipt. This does not yet assemble a production sign-in/join driver or establish Rust-supervisor-to-Node adapter acceptance.

## Driver lifecycle

`driver.mjs` assembles HostPipe, ReceiveBridge and ReceiverBinding around an injected asynchronous `connect({settings, signal})` operation. The connector must validate destination/settings and perform shared Identify admission before any real login, honor the AbortSignal, and return `{connection, botUserId, lookupParticipant, dispose}`. It owns setup resources until returning them. Dispose must be idempotent and cover both voice connection and bot client resources. The connector and credential reader are supplied below; lifecycle tests still use local fake connections.

The driver validates the host handshake before deferring one startup attempt. It buffers at most16 records/64KiB during that attempt, verifies host identity/order and immediately handles app stopping. It applies queued target changes before attaching the receiver. Startup has a15-second deadline checked before the effect, by idle ticks and before accepting a result. EOF, stop, overflow, reconfiguration via stop/restart and timeout abort startup; a late returned session is disposed without becoming a receiver. There are no automatic retries. User/output mute state can be set before readiness and is passed into the binding.

Stop is sticky, detaches receiver/host I/O and invokes owned session disposal once. Async cleanup rejection is observed in `cleanupFailed`; this does not prove cleanup completed or interrupt an uncooperative connector. The eventual executable and Rust supervisor still need a bounded shutdown/kill/reap boundary. A pending connector can remain pending after stop, but cannot cause another attempt or capture audio; if it eventually resolves, the late-result cleanup runs. Connection loss stops this incarnation rather than silently rejoining with old authority.

Local fixtures cover ready startup, EOF/deadline late results, same-batch app stopping, missing grants, queued-record/byte overflow, connector failure, disconnect, cleanup rejection and delayed-timer races. Live authentication, quota admission, DAVE join, credential handling and runnable installed-package acceptance remain open.

## Shared Identify admission

The native app now supports `--discord-identify-admission` alone. This local, token-free helper reserves through the same private per-user boot-clock/file-lock coordinator as the Rust Discord worker. It prints version1 JSON with Reserved or Wait plus wait_ms; it does not authenticate, open a profile, connect to Discord or return a token. Extra/misplaced arguments are rejected before admission. The helper may create/update the existing default application coordination directory, so never invoke it just to inspect an account. Tests override XDG_DATA_HOME only in owned child fixtures.

This currently requires Linux, matching the existing native coordinator. It does not coordinate other applications, OS users or hosts, or replace Discord's authoritative service session-start limits.

`identify.mjs` provides `reserveWithHost` with fixed arguments, a two-second helper timeout,1KiB output cap, cancellation and strict response parsing. It passes only the app's data-directory/runtime environment, not provider or bot credentials. `SharedIdentifyThrottler` implements the library's `waitForIdentify` interface for shard0, obtaining a fresh shared reservation for each invocation. Actual connectors must install it through `ws.buildIdentifyThrottler`, preserving admission on internal reconnect attempts; a one-time pre-login check is insufficient.

Admission waits have a15-second deadline, cancellable bounded delays and checks after a reservation result. A late/uncertain consumed reservation is never refunded or retried. Failures invoke one fatal teardown callback. The hook stays pending until the library's shard-close AbortSignal is observed before rejection; throwing alone is not used as proof that downstream Identify logic cannot continue. If teardown cannot close the shard, the supervisor still needs to kill/reap the process. This is fail-closed resource ownership, not a forced async cleanup guarantee.

The real Node-to-native test is enabled by `WAIFU_IDENTIFY_TEST_EXECUTABLE=/absolute/path/to/waifu-studio npm test --offline`. It uses isolated temporary state and checks shared reservations. Without that explicit path this optional integration case is reported skipped. The connector factory below now wires this adapter; its runnable installed entry remains open. Remote session-start allowances, live quota behavior and bot/DAVE acceptance remain open.

## Bot connector and credentials

`connector.mjs` now supplies `createBotConnector` for the driver. Merely importing it performs no sign-in or credential read. It pins configured guild/voice-channel IDs before async work, reads only the existing private bot_token credential, constructs the supported bot Client with Guilds/GuildVoiceStates intents and shard0, and installs SharedIdentifyThrottler through the actual library factory hook. Before each Identify invocation it refreshes Gateway metadata and refuses exhausted allowance or a multi-shard recommendation before requesting a shared local reservation. This does not prove live quota compatibility or eliminate remote quota races with other applications.

After login it requires a ready bot identity and a cached ordinary guild voice channel belonging to the configured guild. Text/stage channels are refused. Join requests DAVE encryption, selfDeaf=false and an independent connection group. The connector waits for voice readiness and the library's encrypted-session privacy-code indication under cancellation and a ten-second voice-stage deadline; the driver's overall startup deadline still applies. It performs no REST member fetches: participant identity comes from members currently cached in the configured voice channel. Gateway loss/errors abort and dispose bot/voice resources rather than silently rejoining. Shared TTS audio is still not routed into this connection.

`credentials.mjs` reads the host credential schema from the explicit installed data directory only. It currently requires Linux, an owned private real directory and an owned0600 regular single-link credentials.json opened with no-follow/nonblocking flags. Reads are bounded to32KiB, revision/value limits are validated, OAuth metadata is refused for this bot-token reader, and token values must be graphic ASCII up to4096 bytes. It never falls back to token environment variables or modifies the file. Errors are fixed and omit path/content. Tests use owned synthetic credential files and injected fake bot clients; no saved user token or real Discord login has been used.

The worker entry below is supplied; the offline Linux development bundler below produces an installable dependency directory. Connecting this factory to a manually launched tool would perform real authentication; do not invoke it as an offline check. Real sign-in/DAVE negotiation/receive, server permissions, service failures, outgoing audio and user-confirmed call acceptance remain open.

## Worker entry and shutdown

`worker.mjs` now assembles the real connector and driver for supervised stdio use. It accepts no arguments and requires absolute WAIFU_EXTENSION_DATA and WAIFU_STUDIO_EXECUTABLE values from the host. Missing environment, extra arguments and malformed host input fail with fixed text before connection setup. There is no credential-bearing CLI argument or fake/offline mode in the production entry. Importing the module is inert; only direct execution or explicit runVoiceWorker invocation starts its I/O driver. A valid granted handshake can cause real authentication, so this entry is not a preflight command.

On EOF, host stop or SIGINT/SIGTERM, the entry freezes intake and checks startup settlement, owned async disposal and pending protocol writes. It allows two seconds for shutdown, then exits with failure if asynchronous work remains unresolved. Short fixed stderr output has a further50ms flush cap, while unexpected-exception redaction remains active until exit. A clean settled shutdown exits successfully; cleanup failure and unexpected errors produce fixed failure text. The Node timer cannot interrupt a synchronously blocked event loop/native call: the Rust supervisor's process-group kill/reap boundary remains required. Offline Rust tests now verify direct-worker reaping with this event loop blocked and termination of an active same-group descendant. These use the actual worker module with a fake connector, not an installed release bundle or live Discord connection.

Real child fixtures use fake connectors with unresolved startup/disposal promises, late ready results, SIGTERM and a synthetic private exception. They verify that startup/disposal was reached before EOF, the worker itself exits under its budget rather than receiving the parent's emergency kill, late resources are disposed, and raw exceptions are not printed. Production-entry negative cases use an empty temporary data directory and no valid handshake; they perform no credential read/login. These checks do not prove live cancellation of Discord SDK operations or descendant-process reaping under the Rust supervisor.

The development entry is ready; dependency bundling and managed installation remain open; the development worker has focused Rust-supervisor coverage. Do not add this experimental receive path to the ordinary Add menu or claim bidirectional calls until outgoing audio and the declared service gates pass.

The Linux supervision fixtures require Node and these pinned local dependencies. The descendant test confirms its process-group membership while alive, then confirms exit and cessation of heartbeat writes after host shutdown. A zombie is accepted as evidence of descendant exit only; the system reaper owns orphan reaping. These fixtures do not prove containment of code that deliberately escapes the process group.

## Offline Linux development bundle

After installing the pinned dependencies on the target Linux machine, run:

```sh
python3 build_bundle.py /absolute/path/to/new-discord-voice-bundle
waifu-studio --check-extension /absolute/path/to/new-discord-voice-bundle
```

The output path must not already exist. `--node /absolute/path/to/node` selects the runtime; its distribution must include the adjacent root LICENSE. The bundle includes that runtime, its license, production modules, dependency metadata/licenses and native Opus/DAVE bindings. It removes npm command shims, type declarations, source maps and temporary native-build products to fit the existing installer limits. It neither downloads nor invokes npm, signs in, installs or enables an integration. Its codec-only preflight runs before publishing the output directory. Native dependencies are specific to the build machine's Linux architecture/libc; this is not a portable cross-platform release. `bundle-platform.json` records Node/architecture/ABI; the runtime is about160MiB including dependencies on the current machine.

The generated API1 manifest declares server/voice-channel settings, a private bot-token field, and the direction-specific capabilities selected by `--mode` (receive by default). The managed `run.sh` uses its own bundled Node, with no PATH Node or development-checkout dependency. A real valid host handshake can authenticate; do not use that launch as an offline preflight. Installed relocation, codec preflight, immediate EOF and malformed-handshake rejection have been checked in an isolated temporary store; fresh installed Rust-runtime lifecycle checks now cover all three bundle modes; live call acceptance remains open. Keep these experimental bundles outside the ordinary Add menu until their declared gates pass.

To reproduce installed-launcher supervision checks without accounts, from the repository root:

```sh
cargo run --example verify_discord_voice_bundle --offline -- /absolute/path/to/bundle
```

This creates and removes a temporary store, installs the owned bundle disabled/ungranted, and directly supervises its managed launcher with malformed input and a valid synthetic handshake without credentials. It checks fixed failure output, no actions/data writes and direct-process reaping. It also enables the owned installation in its temporary store through InstalledRuntime, verifies failure without credentials, checks repeated frames do not retry it, disables/re-enables it explicitly, and verifies disabled state after store reopen. It never authenticates or touches a real profile.

The owned child-pipe codec fixture can separately use a relocated installed copy:

```sh
WAIFU_VOICE_TEST_BUNDLE=/absolute/path/to/installed/revision node --test tests/host-worker.test.mjs
```

Run that command in this tools directory. It uses the bundle's Node plus installed HostPipe/ReceiveBridge/codec modules. The fake host verifies Start/frame/Finish, original participant identity,320 PCM samples, ordered sequences and correlated queued completion; early EOF must emit nothing. This proves relocated audio transport/codecs, not a live receiver or application ASR/history acceptance.

## Bounded private playback queue (not driver-wired)

`playback-queue.mjs` is an inert, transport-independent queue for private `audio.output` records. Construction requires the separate `audio.playback` grant and pins the host session/process/sequence. The caller must supply an authoritative target through `setTarget` or a strict `target.changed` record; Start does not grant target authority. Audio is fixed20ms48kHz stereo PCM16. The queue keeps at most32 complete frames, one active playback, one untaken terminal outcome and64 recent started IDs. It checks payload shapes, frame order/count, monotonic clock,5-second stall and22-minute absolute lifetime. Mute/target change/Cancel discard queued media; invalid records stop it. Output mute is separate from receiver/input mute.

`pull(now)` yields one complete PCM buffer to an adapter. `drained` means only handed off from this queue, not SDK/device/remote playback. The adapter must consume outcomes before starting the next stream and keep output-speaking suppression until actual player completion. It must stop/detach player resources on queue cancellation or failure. The driver now constructs this queue and its SDK/feedback binding only when audio.playback is present. Worker ACK/control routing and echo suppression are connected; the host owner is connected; source pacing/clock and production speech routing remain unfinished. Bundled receive-only permission requests are unchanged.

The bundle includes this module. Its focused tests can select a relocated installed copy with `WAIFU_VOICE_TEST_BUNDLE=/absolute/installed/revision node --test --experimental-test-isolation=none tests/playback-queue.test.mjs`. That tests the queue only, without speakers or Discord.

## Offline playback adapter draft

`playback.mjs` wraps the private queue with a caller-owned Ready connection, an owned SDK player/subscription, native Opus encoding and an object-mode stream with a one-packet high-water mark. Construction requires an unsubscribed connection. The caller must forward every host record, target/output-mute change and periodic monotonic `tick()`. Receiver/input mute remains independent.

Queue drain ends the stream; suppression remains active until that resource's actual player Idle. A `completed` receipt means SDK Idle after drain, not proof of remote delivery or audibility. Cancellation also stops the buffered SDK tail. Connection/subscription loss, early Idle, queue failure, encoding errors and a five-second drain-to-Idle deadline close the adapter. Receipt retention is bounded to64 outcomes. Tests use fake connections/players, plus a real native codec/SDK-resource packet check; they never join a call.

The development bundle now includes the adapter and its binding. Driver construction requires the separate playback grant, which the bundle still does not request. Host ownership and receipt intake are connected; production source/call-ready selection, source-clock routing and live acceptance remain open.

## Bounded playback feedback draft

`playback-feedback.mjs` accepts the adapter's Ready/Consumed/Finished callbacks and emits private `audio.playback.receipt` actions using the driver-owned `ActionSequence`. It keeps one action awaiting a correlated host ACK, one cumulative consumption count and one terminal marker. Counts coalesce while an ACK is pending; terminal feedback follows acknowledgement of the final consumption count. Missing/refused ACKs and uncertain writes stop the sender without replay. Pending actions expire after10seconds. Non-ending feedback turns allow the source’s22-minute lifetime plus the adapter’s5-second finalization window; once terminal feedback exists, at most three remaining actions are bounded by their individual ACK deadlines. At most64 recent playback IDs are retained. The host ledger keeps its own independent source deadline.

The adapter's optional `onFeedback` callback reports Ready after codec/resource/player setup, consumption after successful packet encoding/push, and terminal completion only after SDK Idle following drain. Setup failures can emit a zero-count Failed receipt without Ready. A coordinator must forward every host record to both components, tick both, stop playback if the sender stops and wait for the prior terminal ACK (`!active && !stopped`) before admitting the next Start. Retired callbacks are successful no-ops so a target change works in either forwarding order; duplicate Ready for an active playback is rejected. Target replacement discards old pending feedback. Output-speaking suppression remains controlled by actual adapter/player lifetime.

Both modules are now included in the development bundle and conditionally constructed by the driver under audio.playback. The manifest remains receive-only and requests no playback grant. The app receipt sink now connects the host-owned SpeechOutputs ledger; unknown/inactive source receipts are rejected. Production speech sources are not routed into it yet. The queue retains its first stop reason through cleanup.


## Conditional worker playback

`PlaybackBinding` owns output resources and forwards each host record to feedback before the adapter, allowing a Finished ACK to clear before the next Start. The driver forwards the same record to the receive bridge; known private outbound media advances its host sequence without becoming participant input. Receive and playback actions share one sequence. Automatic output suppression is ORed with retained external suppression and input mute; output mute is separate and survives startup.

Any output/feedback failure stops the worker, freezes new feedback, closes the owned SDK subscription/resources and disposes the connection once. Failure receipts emitted before a fatal stop are best effort, not guaranteed acknowledged. No automatic reconnect/replay occurs. An early next turn before the prior Finished ACK fails closed. The coordinator does not own the TTS source or host media authority. Existing installed verification remains credential-free; synthetic duplex tests inject connections/players and do not prove Discord audibility.


A source stall on an empty turn now cancels that playback and preserves the healthy worker/connection. The SDK underrun path uses the same cancellation when its timer wins the race. Invalid records, outstanding-feedback failure and unexpected Idle with buffered PCM still fail closed. Host-owned call readiness, PCM source routing and live acceptance remain separate gates.


## Playback readiness barrier

After connection/player setup and a valid current target, the optional playback binding emits `audio.playback.available` through the shared action sequence. The host acknowledges it only against the exact eligible persistent route. The binding waits for that ACK before a current-target Start, retains at most one availability ACK with a10-second deadline, clears it on retarget/disconnect and ignores obsolete ACK IDs. Duplicate target controls preserve already acknowledged readiness. `VoiceDriver.playbackReady` is separate from receive readiness.

The host SpeechOutputs resource snapshots candidates before action intake, accepts readiness without runtime-lock re-entry and rechecks current authority/readiness before Start. A process being alive is insufficient. The playback grant independently authorizes narrowly scoped target identity control; it does not grant lifecycle/reply observation. Hooks/manual tests are excluded from that private control channel. Production PCM source selection and constructors are still not routed.

## Independent receive and playback modes

The worker now derives receive/playback directions from the host's granted capabilities before connecting. Receive requires `audio.transcribe`, `message.deliver`, and `lifecycle.observe` together. Playback requires only `audio.playback`; playback-only mode constructs no receiver or participant codec and joins self-deafened. Receive-only mode joins self-muted. Duplex enables both directions. The connector requires explicit directions and pins them before asynchronous credential/login work.

`python3 build_bundle.py /absolute/new/bundle --mode receive` keeps the default receive-only permission request. Choose `--mode playback` for outgoing character speech without participant capture/ASR, or `--mode duplex` for both. Installation, explicit permissions, valid bot/channel configuration, and ready-call acknowledgement are still required; building never signs in. These are development bundles and live call acceptance remains open.

The app's Local + calls / Local only / Calls only selector controls new utterances; previews stay local. Calls only uses the bounded device-independent decoder clock, with explicit source and routing settlement. Lagging peers may be skipped at shared-buffer pressure or source EOF so healthy calls keep progressing. Normal/narrow offscreen UI layout and egui selector/persistence checks have passed. Native interaction and live call audibility remain unverified. Offline transport, SDK fixtures and codec preflight do not prove remote audibility.
