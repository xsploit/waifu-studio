# Fish settings audit — 2026-10-18

Checked against the current [official OpenAPI](https://docs.fish.audio/api-reference/openapi.json), and the native HTTP, timestamp SSE and MessagePack WebSocket builders. This is a request-field audit, not a voice-quality certification.

## Already implemented

- Exact hosted model selection: `s1`, `s2-pro`, `s2.1-pro`, `s2.1-pro-free`, `drama-3`. Unknown models are rejected locally to avoid Fish's silent paid-model fallback.
- Buffered synthesis, progressive HTTP WAV, timestamp SSE, live WebSocket and timestamp WebSocket.
- `latency`: Normal, Balanced and Low; `chunk_length` 100–300; `min_chunk_length` 0–100; previous-chunk conditioning; speaking speed; saved reference voice ID.
- WAV/MP3 on compatible paths; streaming modes use WAV. Incremental WAV decoding accepts Fish's unknown-length sentinel. Production startup buffering remains 180ms.
- Drama direction text, voice-library search/selection, optional voice design previews, direct Fish ASR and Fish ASR through Vercel/OpenRouter adapters. OpenRouter was not called in this benchmark because the account has no credits.

Native defaults are Balanced, chunk160, minimum20 and conditioning on. Fish's server defaults are Normal, chunk300 and minimum50. Text normalization is sent as `true`; it is not currently a user setting. The LLM phrase-flush policy is separate from Fish's server-side chunk settings.

## Fields not exposed yet

| Setting | Current Fish contract | Why it might matter |
|---|---|---|
| Temperature / top-p | Both 0–1, default0.7 | Expressiveness/diversity, not a proven latency optimization |
| Normalize toggle | Defaulttrue | Spoken numbers, dates and symbols |
| Language | BCP-47 hint | Normalization and timestamp alignment; does **not** change voice/accent |
| Prosody volume | Decibel adjustment | Output level |
| Loudness normalization | `prosody.normalize_loudness`, defaulttrue on S2 | Consistent loudness; S1 accepts it without effect |
| Sample rate / MP3 bitrate | Rate override; MP3 64/128/192 kbps | Bandwidth and decoder compatibility; currently server defaults |
| Opus/PCM output | Advertised by server | Requires explicit container/decoder work before exposing |
| Audio-token cap | `max_new_tokens`, default1024 | Bounds per-chunk generation; reducing blindly can truncate speech |
| Repetition penalty / early stop | Defaults1.2 /1; early stop0–1 | Stability and batch termination, not demonstrated startup wins |
| Quality guard | `features:["quality-guard"]` | Availability depends on inference backend; no benchmark claim |
| Pronunciation dictionaries | Up to3 references or inline dictionaries | Names and specialized vocabulary |
| Multi-speaker references | Array of IDs on S2/Drama | Dialogue synthesis; app currently sends one ID per speaker request |
| Inline reference audio | MessagePack required | Zero-shot voice cloning; current JSON synthesis does not send it |

Voice cloning, saving a designed voice, and multi-speaker request authoring remain feature gaps. The documented source of voice-design save signatures still needs confirmation.

## What the latency tests establish

See [raw results and tables](benchmarks/RESULTS.md). Low latency was tested with Drama3 and S2.1Pro, HTTP and WebSocket. It was **not consistently faster for every combination**. HTTP wins some ready-text cases; WebSocket can overlap LLM deltas, while the current HTTP/SSE chat path waits for the full reply.

No additional Fish quality knob has been silently changed. Do not trade away startup buffering to advertise a lower number: audible crackling and physical speaker latency still need a separate listening test. A 500–700ms complete pipeline has not been established.
