# Fish Audio capabilities and implementation boundary

Reviewed official live documentation and upstream source on 2026-10-17; the implementation status below reflects the integrated native app. No authenticated requests, account inspection, paid generations, local inference, or audible playback acceptance was performed. Hosted documentation and local Fish Speech are different contracts.

## Model and account choices

| Choice | Confirmed capability | Cost / limitation |
|---|---|---|
| Hosted `s2.1-pro-free` | Same model as S2.1-Pro, existing TTS endpoint | $0 per million UTF-8 input bytes, fair-use limits; intended evaluation, development, smaller businesses; no TTFA or DPA guarantees |
| Hosted `s2.1-pro`, `s2-pro` | Paid TTS | $15 per million UTF-8 input bytes |
| Hosted `drama-3` | Voice direction in natural language; sounds, pauses, scoped tags, multi-speaker turns | Accepted by current TTS schema, but the fetched pricing table does not list its rate. Do not present it as free or assert its price |
| Hosted `voice-design-1` | Describe a new voice and receive preview candidates | $0.01 per successful request, independent of candidate count |
| Local Drama 3 | Open-weight 35B-A3B (3B active parameters), cloning/direction, long audio streaming, editing/infilling capabilities in project | Local compute and setup required; Fish Audio Community License, not automatically unrestricted commercial use |
| Local S2-Pro | 4B previous flagship, faster than Drama 3 per project positioning | S2-Pro weights remain Fish Audio Research License; hardware-specific latency unmeasured |

Hosted calls need a Fish account/API key. Official pricing says no API subscription fees or monthly minimums. Public pages do not establish whether a particular zero-balance account can invoke every endpoint: do not promise free Voice Design, entitlement, credits, or account availability. Concurrency is account-wide: documented starter tier 5 requests, 15 after $100 total prepaid, 50 after $1,000. Native API overload returns 429 without Retry-After; use bounded exponential backoff. These general limits do not replace free-model fair-use limits.

**Explicitly validate the model header. Missing or unrecognized hosted TTS model falls back to paid `s2.1-pro`.** Never silently retry a free request with a paid model. S1 is deprecated and documented to retire December 31, 2026.

## Hosted one-shot contract

`POST https://api.fish.audio/v1/tts`

Headers: `Authorization: Bearer <key>`, `Content-Type: application/json`, `model: s2.1-pro-free` (or another explicitly selected supported model).

```json
{"text":"Hello there.","reference_id":"VOICE_MODEL_ID","format":"wav","normalize":true,"prosody":{"speed":1.0}}
```

`reference_id` is optional and accepts one model ID or an array for speakers. Inline `references` require MessagePack rather than JSON. Current hosted OpenAPI does **not** declare the local server's `streaming` request field. Standard response is audio bytes; transport chunks are byte fragments, not independently decodable clips.

- Formats: `wav`, `pcm`, `mp3`, `opus`; default MP3. Optional `sample_rate`; documented default 44,100 Hz for most formats and 48,000 for Opus.
- `prosody.speed`: documented 0.5–2.0, default 1.0. `prosody.volume`: dB, default 0; no numeric range published in this schema. `normalize_loudness`: default true, documented S2-family effect; S1 accepts but ignores it. Do not infer Drama-specific behavior from that statement.
- `latency`: `normal`, `balanced`, `low`. `chunk_length`: 100–300, default 300. `min_chunk_length`: 0–100, Fish schema default 50; the app explicitly defaults to 20 for WebWaifu-compatible low-latency chunking. These affect generation, not a promise that response read boundaries match utterances.
- `language`: BCP47 normalization/alignment hint; it does not select accent or voice.
- Drama 3 uses the same endpoint with `model: drama-3`: e.g. `[barely holding it together] I'm fine.` or `I'm fine. <whisper>Really.</whisper>`. Multi-speaker turns use `<|speaker:0|>` and an ordered `reference_id` array. Language tags are explicitly experimental.

## Actual streaming protocols

### Audio WebSocket

`wss://api.fish.audio/v1/tts/live`, with bearer/model headers at upgrade. All messages are **MessagePack binary frames**, not JSON text frames. Audio fields are MessagePack binary bytes, not base64. Send maps in this order:

```text
{event:"start",request:{text:"",format:"wav",reference_id:"VOICE_MODEL_ID"}}
{event:"text",text:"Hello there. "}
{event:"flush"}        # optional: generate buffered text while session remains open
{event:"text",text:"How are you?"}
{event:"stop"}         # drain buffered text and finish
```

Server sends `{event:"audio",audio:<bytes>}` and terminal `{event:"finish",reason:"stop"|"error"}`. Concatenate audio bytes in arrival order. A transport frame is not necessarily a complete WAV/MP3 file. Documentation has a connection-lifetime inconsistency: ordinary WS page says close after finish; newer timestamp WS page says another start may reuse the socket. Reconnect per utterance initially; do not require socket reuse. Treat server error/close as terminal and never silently change models.

### Timestamp SSE

`POST /v1/tts/stream/with-timestamp`: same request body and bearer/model headers, response `text/event-stream`. Parse SSE framing across arbitrary HTTP reads. Each `message` event contains JSON:

```text
{audio_base64:"...",content:"...",chunk_seq:0,chunk_audio_offset_sec:0,
 alignment:{audio_duration:1.2,segments:[{start:0,end:0.3,text:"Hello"}]}}
```

Base64-decode and append every audio payload in arrival order. `alignment` may be null. Each non-null alignment is a **cumulative replacement snapshot for that chunk_seq**, not a delta. Absolute word time is chunk_audio_offset_sec + segment.start/end. Do not use chunk_seq to assign audio: a frame may carry next-chunk audio with previous-chunk alignment. The inspected response schema does not establish a universal `[DONE]` marker; handle EOF/errors explicitly.

### Timestamp WebSocket

`wss://api.fish.audio/v1/tts/live/with-timestamp`: same binary MessagePack start/text/flush/stop protocol. Audio messages additionally carry content, alignment, chunk_seq, chunk_audio_offset_sec; audio remains binary rather than base64. Terminal finish has reason stop/error; request-level error can close the socket. Same cumulative alignment rules as SSE.

Word timestamps are **not visemes**. They can time captions and sentence highlighting, but do not provide mouth shapes.

## Voice Design is a separate workflow

`POST https://api.fish.audio/v1/voice-design` requires bearer auth, **`model: voice-design-1`**, JSON content type. Extra body fields are rejected.

```json
{"instruction":"An adult woman with a warm, calm voice and light rasp.","reference_text":"Welcome back. How was your day?","n":2,"speed":1.0}
```

Fields: instruction required 1–2000 characters; reference_text optional up to 150; language optional; n 1–4 (default 2); speed >0 and <=3 (default 1); num_step 1–128 (default 32); guidance_scale >=0 (default 2); instruct_guidance_scale >=0 (default 0); seed optional integer.

Response `{candidates:[...]}` with each candidate's id, index, audio_base64, sample_rate, duration_ms, plus optional text/instruct/language. Decode previews locally. Endpoint is stateless: **it does not create persistent voice models, samples, batches, or presigned URLs**. A candidate id is not documented as a ready-to-use TTS reference_id. Persistent save/model creation or an inline reference workflow is separate work; do not silently claim a designed voice was saved. Authentication, validation, balance, concurrency and upstream errors are not billed.

## Local Fish Speech differs

Current upstream `ServeTTSRequest` has `streaming: false`, `single_chunk` for Drama 3, chunk_length 100–1000 default 200, latency normal/balanced, a single string reference_id, and no prosody/speed/sample_rate fields. Model selection comes from loaded checkpoint configuration, not hosted model headers. Local API auth is optional and enabled only if its configured api_key exists.

`POST /v1/tts` with `streaming:true` **rejects formats other than wav** in current views.py. Streaming yields inference bytes; non-streaming can encode WAV, MP3, Opus or headerless 16-bit PCM. Sample rate comes from the decoder model. Do not send hosted prosody controls and claim local speed support: playback-rate adjustment or resampling is a separate client capability. Local reference IDs refer to local server reference storage; hosted IDs are not guaranteed to resolve locally. Local server exposes reference-add routes.

The README recommends SGLang-Omni for efficient Drama 3 serving. No universal VRAM minimum or host-specific real-time factor was verified. Active parameters do not mean only 3B parameters need storing. Installation, GPU compatibility, license review and audible latency remain unverified.

## Current Rust/Bevy implementation

The native app supports buffered WAV/MP3, progressive HTTP WAV for hosted/local Fish, hosted timestamp SSE, and hosted WebSocket text-in/audio-out with optional timestamps. OpenAI-compatible speech remains buffered. The hosted free-model default and explicit paid-model choices remain separate from the local server’s loaded-model contract.

WebSocket delivery uses binary MessagePack and one session per utterance. Automatic reply reading can send LLM text during generation; sentence-end flushing is configurable and Flush text now is explicit. The incremental speech filter stops at the literal `waifu_tasks` fence even across text-delta boundaries, preserving ordinary Markdown and Unicode. Completion sends stop, while received audio continues draining. Stop speech and character changes invalidate playback generations. HTTP CONNECT proxy routing is supported; unsupported HTTPS/SOCKS proxy routes fail explicitly.

Progressive transports feed an incremental WAV decoder and bounded PCM playback queue. Network and decoder work stays off the render/audio callback threads. Supported streaming WAV is mono/stereo PCM16 or float32; raw hosted PCM remains outside this implementation. Cancellation, buffer limits, fragmented data and timing replacement have local fixture evidence, not authenticated provider or audible-latency acceptance.

Timestamp SSE and timestamp WebSocket display cumulative word timing metadata. Alignment may lag audio and does not stall playback. Synchronized caption highlighting is not implemented. Word timestamps do not produce mouth shapes: consumed decoded playback samples drive the RMS mouth envelope or, when explicitly enabled with a compatible imported calibration, native MFCC A/I/U/E/O analysis. uLipSync JSON and wLipSync WLIP profiles are supported; no calibration is bundled. The vowel analyzer has numerical/fixture evidence, with voice-specific accuracy and perceived synchronization still unverified. See LIPSYNC-NOTICES.txt and VALIDATION.md.

Voice Design supports paid candidate generation and preview playback only. It does not save a persistent voice or make candidate IDs reusable TTS references. Live account access, charges, audio quality, first-audio latency and underrun behavior still require authorized provider/hardware acceptance.

Actual WebWaifu4 source located at `/home/subsect/Documents/Codex/2026-10-11/files-pasted-by-the-user-review-2/work/WebWaifu4`. Its `src/lib/tts/manager.ts` schedules mono signed little-endian Int16 PCM converted to float, uses provider sample rate (fallback 24 kHz), and retains wLipSync. This validates a scheduling pattern, **not Fish's hosted PCM encoding**. The similarly named `work/waifu4-reference` is waifu2x Lua image-upscaler source and provides no speech evidence. Historical jev-discord-demo path is absent on this host.

## Primary sources

- [Models](https://docs.fish.audio/developer-guide/models-pricing/models-overview), [pricing](https://docs.fish.audio/developer-guide/models-pricing/pricing-and-rate-limits)
- [REST OpenAPI](https://docs.fish.audio/api-reference/openapi.json), [WebSocket schema](https://docs.fish.audio/api-reference/endpoint/websocket/tts-live), [streaming guidance](https://docs.fish.audio/developer-guide/best-practices/real-time-streaming)
- [Drama 3](https://docs.fish.audio/features/text-to-speech/drama-3), [Voice Design](https://docs.fish.audio/features/voice-design), [Voice Design API](https://docs.fish.audio/api-reference/endpoint/openapi-v1/voice-design)
- [Fish Speech README and license links](https://github.com/fishaudio/fish-speech), [local schema](https://github.com/fishaudio/fish-speech/blob/main/fish_speech/utils/schema.py), [local routes](https://github.com/fishaudio/fish-speech/blob/main/tools/server/views.py), [local encoding](https://github.com/fishaudio/fish-speech/blob/main/tools/server/api_utils.py)

Sources fetched into workspace `work/fish-review/`; upstream main and live docs can change. Findings are source/documentation evidence, not account or runtime acceptance.

Native additions: hosted min_chunk_length is configurable from 0 to 100 (the app uses WebWaifu’s 20-character default; the service schema default is 50), with condition_on_previous_chunks. Both fields are omitted for local Fish/OpenAI. Read-only GET /model browsing supports own/public search and reference selection. WebSocket provider errors preserve sanitized reason and max_concurrency; no model fallback is attempted. Voice cloning and designed-voice saving remain unimplemented.

## Transcription adapters (2026-10-18)

Direct Fish ASR sends multipart `audio` to `/v1/asr` with `model: transcribe-1` or `transcribe-1-pro`, `ignore_timestamps=true`, and optional lowercase ISO 639-1 language. Pro adds `tag_audio_events=false` and `diarize=false`. Speaker tags and bracketed event cues are removed from the composer transcript. Model names are validated before sending.

OpenRouter uses its OpenAI-compatible multipart `/api/v1/audio/transcriptions` route (for example `fish-audio/transcribe-1`). Vercel uses `/v4/ai/transcription-model`, JSON/base64 audio and its three protocol/model headers. These paths have fake-server request/response checks; no authenticated transcription or account-rollout acceptance is claimed. Do not port WebWaifu's older Fish JSON/base64 request: current Fish docs require multipart or MessagePack.

Primary contracts: [Fish OpenAPI](https://docs.fish.audio/api-reference/openapi.json), [OpenRouter OpenAPI](https://openrouter.ai/openapi.json), [Vercel transcription](https://vercel.com/docs/ai-gateway/capabilities/transcription).
