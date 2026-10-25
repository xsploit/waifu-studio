# Outgoing speech implementation checkpoint

This is an internal implementation contract, not a released extension media API. Discord voice currently receives audio; bidirectional call acceptance is still open.

## One decoded source

`SpeechMeter::buffered_routed` and `streaming_routed` produce the existing local playback asset plus one bounded PCM reader. Only the claimed playback decoder consumes the source. The same sanitized samples feed local playback, meter, visemes and the tap. Ordinary constructors retain their existing behavior and allocate no tap.

The tap returns interleaved whole mono/stereo frames in the decoder's native sample rate. It does not resample, change playback speed, encode Opus, deliver a protocol message or prove remote playback. Its queue is bounded to32768 samples; drains allocate at most4096 frames outside the audio callback. Overflow fails the tap without blocking or truncating local playback. Dropping the tap only detaches that consumer.

## Completion versus cancellation

A natural source EOF retains its queued tail when the next sentence begins. An explicit `SpeechMeter::stop` invalidates every outstanding tail immediately, including naturally completed ones. Replacement during active playback invalidates the interrupted source; later decoder Drop cannot turn that interruption into successful completion. A current decoder dropped before EOF permits its queued samples to drain only until replacement or explicit Stop.

A routed asset dropped before decoder creation terminates its tap through a lifetime guard owned only by playback assets/clones and the claimed decoder. Retained playback receipts cannot keep that guard alive. If an asset remains retained but playback never starts, the eventual route must still enforce a startup deadline using consumption evidence, and clear routes on target/session changes. It must not treat queueing or asset creation as playback.

Asset construction and explicit Stop share a control-thread lock so their generation/epoch changes cannot interleave. The audio callback never takes that lock. EOF and ordinary replacement resolve on one tap-state atomic; cancelled states cannot be overwritten by late format or overflow errors.

## Call-format conversion

`CallPcm` converts bounded native PCM chunks into1920-sample PCM16 arrays:960 frames of48kHz stereo, or20ms. Mono duplicates into both channels; stereo preserves independent channel history. It reuses the existing causal polyphase FIR implementation, whose default microphone alias remains16kHz. Chunk boundaries preserve fractional phase and filter history. Input format is fixed per converter and an invalid chunk is rejected before state changes.

Normal finish flushes the FIR tail and silence-pads one final partial packet; repeated finish returns no audio. Cancellation must discard the converter rather than flush interrupted speech. This converter does not forward packets, alter playback speed, claim another decoder or drive a call clock.

## Private transport authority

The provisional `audio.playback` grant now controls private `HostMessage::AudioOutput` publication. It is independent of text/lifecycle observation. `AudioPlaybackRoute` captures the installation, session, process incarnation, origin and character target; each publication checks that identity, the current authoritative target and live grant. Hooks and manual test children are excluded. Revocation stops the connection to cancel queued media; regrant does not revive the old process and requires explicit restart. The payload/target/session boundary is implemented, but no production speech source calls it yet.

## Remaining routing requirements

- Bind every route to the exact character generation, reply/utterance and authorized extension incarnation. Recheck grants and target validity before forwarding; a previously drained chunk is not an authorization token.
- Preserve sentence order and drain completed tails before starting the following remote sentence. Keep explicit Stop/reply cancellation distinct from ordinary local sentence completion.
- Wire the bounded call-format converter into authorized routes, preserve resampler history across appropriate sentence boundaries, and map local playback-speed changes accurately into outgoing timing.
- Connect production speech sources to the implemented media grant, framing, acknowledgements, deadlines, receipts and cancellation paths. Queued, delivered and played remain different observations.
- Route decoded audio through a shared clock for call-only operation when local output is muted or unavailable. The current tap progresses only when the local decoder is consumed; it does not satisfy this mode on its own.
- Reconcile outgoing playback with receiver suppression, independent mute controls, stop/disconnect, echo handling and retry policy. Do not synthesize provider speech twice to avoid ownership work.

Verify each stage with owned fake hosts and synthetic audio before requesting live service acceptance. The temporary bundle verifier covers installation/supervision/failure handling; it does not prove outgoing call audio.

## Private playback feedback boundary

The provisional `audio.playback.receipt` child action uses the independent `audio.playback` grant and the usual session/process/target/replay checks. Its strict `receipt` payload has one of three phases: `ready` with a playback UUID, `consumed` with a positive cumulative `frame_count`, or `finished` with a cumulative count and `completed`, `cancelled` or `failed` outcome. Counts cannot exceed65536. Consumption acknowledges adapter pulls, not playback; completion will mean SDK Idle after drain, not remote audibility.

The callback remains opt-in and defaults to Unavailable for integrations without a host adapter. The app sink now routes receipts into its SpeechOutputs resource; only an exact active playback/source match is accepted. The optional worker emits these actions after a private Start. The ledger correlates the playback ID and exact route/incarnation, rejects impossible phase/count transitions and releases bounded send credits only from valid feedback. A schema-valid receipt or successful sink admission never authorizes a media send by itself. Every subsequent send must still recheck the live grant, session, process and current character target.

## Host playback ledger

`PlaybackLedger` now captures one host-owned route and fresh playback UUID without retaining PCM. It admits Start once, waits for Ready before frames, limits unconsumed frames to four, validates cumulative consumption against successfully admitted frames, and requires Finish plus consumption of all admitted frames before accepting Completed. A completion receipt already in flight when the host cancels after Finish is accepted without blaming the worker, while preserving a Cancelled host outcome. A Ready receipt already in flight during startup cancellation is ignored without reviving the stream or extending its deadline.

The ledger advances send state only after its checked publish callback returns success. An uncertain/failed send is terminal and is never retried. A five-second owed-worker-progress deadline and22-minute absolute lifetime bound retention; invalid clocks and impossible correlated feedback are terminal. Foreign routes/playback IDs are rejected without consuming credits or poisoning the current playback.

This ledger is bounded accounting. The app's SpeechOutputs owner now checks current runtime grant/session/process/target on each send, correlates host attribution and cancels the captured worker on local failures. Source/sentence scheduling and the shared PCM/clock connection remain open. Dropping this ledger alone performs no transport cancellation.

## Worker feedback sender checkpoint

The inert Node feedback sender now serializes Ready/Consumed/Finished actions through the shared child action counter. It has one pending ACK, coalesced cumulative consumption and one retained terminal outcome, with bounded ACK/lifetime/retired-ID state. Adapter callbacks are connected in local tests, including real native Opus/SDK-resource coverage, setup failure and either ordering of a target-change event. A stale retired callback is a no-op; it must not fail the active adapter after retargeting.

The sender, adapter and binding are now included in the development bundle; the driver constructs them only when the separate playback grant is present. The manifest still requests receive-only permissions. Worker ACK/control forwarding, independent mutes and stop/disposal are connected. Remaining work includes the host stream owner's live authority/cancellation checks, production feedback intake and the production PCM/clock route. Module and fake-connection tests do not establish a working Discord call.


## Host speech output owner checkpoint

`SpeechOutputs` is initialized in the app and ticked outside `ExtensionRuntime::frame`. Receipt intake inside that frame only updates the pure ledger, avoiding runtime lock re-entry. A route captures its installation, session/process, origin, target and unique source/playback IDs. There are at most four active routes,64 held frames per route, four unconsumed sent frames,32 reserved terminal outcomes and64 recent source attempts. Outcomes retain their original attribution and are consumed explicitly.

Every frame send uses live runtime authority. Explicit Cancel clears queued PCM and is idempotent; teardown-only Cancel may address the captured previous target but cannot send frames there. Target/authority loss retires the route; local protocol/transport failures stop only the captured process incarnation, never a replacement. A host source pause with no worker progress owed does not fault the peer: the worker cancels the empty turn after its source-stall deadline while keeping the call alive. Outstanding worker progress and total lifetime remain bounded.

The owner accepts already-converted call frames; production speech constructors still do not feed it. Worker-ready gating is now implemented: the current eligible route must announce audio.playback.available and receive its ACK before Start. Next work is source/installation selection, shared PCM/converter scheduling, actual speech cancellation and playback-speed/call-only clock behavior. No extra provider synthesis or real account call was added.


## Readiness and target control

`audio.playback.available` is a strict empty action requiring the playback grant and the normal host-assigned session/process/target checks. It can mark only an existing eligible route from the pre-frame snapshot; it cannot create authority. Target, incarnation, origin or grant loss drops readiness. Begin re-captures live authority and requires an exact ready match. The worker announces only after its Ready connection and owned SDK player/subscription exist, and waits for the correlated ACK before accepting Start.

Current target identity is private media control for eligible persistent playback services, independently of lifecycle observation. The shared eligibility predicate excludes hooks, manual tests, stopped/restarting/removing/revoked entries. This control cannot publish other lifecycle/reply payloads. Playback revoke remains terminal; regrant requires a fresh process and readiness announcement.
