> Follow-up verification by the integrating agent, after this source-only Opus review: provider slugs were fetched directly from Vercel's official `/v1/models/{creator}/{model}/endpoints`, not inferred from another aggregator. Vercel exposes `deepseek/deepseek-v4.1-flash-fast` on baseten; both the ordinary and fast model were benchmarked. The ASR client is now reused, event observation and WS connection/text/flush diagnostics are added, and generation lookup is used where available. The rest of this report is the original review: suspected defects and proposed fixes are not all confirmed or implemented. See `benchmarks/RESULTS.md` for current measured results.

# Waifu Studio Performance Audit: Send → First Decoded Audio

**Scope:** This audit covers only the supplied sources. I ran nothing. Every latency figure below is either one of your measurements (≈1041 ms and ≈1168 ms, n=3 each) or third-party endpoint metadata. None of it is a Vercel-measured result for your route. The 500–700 ms target is currently unmet, and nothing here shows it is met.

---

## 1. Benchmark fairness: strict pins vs default/ttft/tps

### What the harness gets right
- `route_cases` sends `pin:slug` as `only: [slug]`. Per the Vercel doc, `only` restricts both routing and fallbacks. A pin is therefore strict, and an unavailable slug fails visibly instead of silently rerouting.
- Routes are the innermost loop, so repeats interleave candidates rather than warming one candidate first.
- In `pipeline` phase, `speech.open()` runs right after the LLM thread spawns. The Fish WebSocket connect therefore overlaps LLM time-to-first-token (TTFT).

### Defects, ranked

| # | Location | Defect | Fix |
|---|---|---|---|
| B1 | `provider::parse_sse`, `bench_pipeline::main` | The served provider is never recorded. You cannot prove that `default`, `ttft` or `tps` picked the provider you assume, or that a pin was honored. | Record the response headers and the chunk `id`. If routing metadata (`finalProvider`, `sort.executionOrder`) shows up in the final chunk, save it. If not, resolve it afterwards with the Generation Lookup API using the generation id. Rows without attribution should be labelled unattributed. |
| B2 | `bench_pipeline` vs `provider::spawn_chat` | The benchmark calls `stream_chat` directly with a 1 ms poll. The app goes through `spawn_chat`, which coalesces every delta after the first for up to **32 ms**, and then through Bevy's frame loop (about one frame). The benchmark is therefore optimistic by roughly 30–50 ms at the moment the first phrase flushes. | Add a `--app-path` mode that uses `spawn_chat` plus a 16 ms poll, or fix the app (see S5). |
| B3 | `bench_pipeline::main`, `fish_ws::WsReader::process` | The timing fields use different start points. `transport_first_wire_audio_ms` is measured from the Fish worker's `budget.started()`. Every other field is measured from `start`. With `--prewarm-ms`, the worker clock starts during prewarm, so this field is not comparable. | Pass an `Instant` origin into the stats, or report `ws_connected_ms` and `first_flush_sent_ms` on the shared clock (see §2). |
| B4 | `main` loop order | Each iteration uses the same route order. The first request of the run also pays the TLS handshake to `ai-gateway.vercel.sh`, because the shared `client()` pool is still cold. That penalizes whichever case comes first. | Discard one warm-up request, and rotate the route start offset by `iteration`. |
| B5 | `--reasoning`, `ProviderConfig::chat_body` | DeepSeek V4.1 Flash advertises the efforts `none`, `low`, `high` and `max`. `minimal` is not listed. Its listed parameter is `reasoning`, while the code sends `reasoning_effort`. Whether Vercel translates that field is unverified. `parse_sse` also drops reasoning deltas, so hidden reasoning shows up only as inflated `first_token_ms`. | Benchmark with `--reasoning none`. Also record `first_any_delta_ms`, counting `delta.reasoning*` as well as content, plus reasoning token usage if it is returned. |
| B6 | Prompt and cache | The 2-turn prompt understates the app's real TTFT, because the real app has persona plus history. Identical repeats also benefit from implicit caching, which every listed DeepSeek endpoint supports, from run 2 onward. | Add a `--context-file` with a realistic persona and history size. Report run 1 separately from runs 2–n. |
| B7 | Statistics | n=3 against metadata p95s of 0.9–15 s cannot separate providers. | Use `--repeat 5` across at least 2 sessions at different hours. Report the median and the worst case, with errors counted rather than dropped. |
| B8 | `main` teardown | `worker.join()` after cancel can block until the 180 s client timeout if a stream stalls mid-line, because cancel is checked only between SSE lines. | Join with a deadline, or detach after the row is saved. |

### Suggested matrix (within the 150-row limit)

```
--phase llm --models deepseek/deepseek-v4.1-flash --reasoning none --repeat 5 \
--routing-matrix default,ttft,tps,pin:togetherai,pin:baseten,pin:inference-net,pin:relace,pin:wafer
```

That is 40 rows.

**How the pins were chosen.** The pins come from the supplied metadata's p50 latency and throughput, which are not Vercel TTFT figures:

| Provider | Latency p50 | Throughput | Notes |
|---|---|---|---|
| togetherai | 369 ms | 280 tps | fp8 |
| baseten | 395 ms | 262 tps | fp8 |
| inference-net | 336 ms | — | p95 852 ms |
| relace | 250 ms | — | p95 1666 ms, a tail risk |
| wafer | 482 ms | — | p95 954 ms |

Exclude alibaba (p50 3857 ms).

**Before treating a pin error as a speed result:**
- Confirm each slug exists on Vercel for this model, using the model-endpoints API. The metadata looks like an aggregator's endpoint list.
- Check that fp8 quantization is acceptable for persona quality.

**"Fast variants".** The supplied evidence shows no faster variant IDs. Discover them from `/v1/models` before benchmarking. `pin_model` preserves suffixes, so a variant ID passes through unchanged.

**Doc ambiguity.** The Vercel doc says REST options go "under `providerOptions`", but its curl examples nest them as `providerOptions.gateway`. The code uses the nested form. B1 attribution is how you confirm the gateway honors it.

After the LLM phase, run `--phase pipeline --modes ws --speech-models s2.1-pro --latency low` on the top two pins plus `default`.

---

## 2. Is LLM streaming actually overlapping Fish?

**Partially. The overlap exists, but the two critical-path boundaries are not instrumented.**

What is overlapped:
- **Fish connection setup.** DNS, proxy TCP, HTTP CONNECT, TLS, WebSocket upgrade and the `start` event all run on the worker while the LLM is still in TTFT, in pipeline mode.
- **Text.** `TextQueue::push` flushes after the first ≥24 characters at a whitespace boundary, at sentence punctuation after ≥10 characters, at a clause after ≥44, or hard at 68. `WsReader::pump` drains the queue every ~2 ms, because `read` loops through `pump` and a WouldBlock read with a 2 ms sleep. The first phrase reaches Fish within a few ms of the LLM producing about 6 tokens.

What is **not** proven:
- **Whether the WebSocket is ready before the first flush.** If `connect()` takes longer than TTFT plus the first phrase, the connection is on the critical path. This plausibly happens through the proxy for fast pins.
- **Fish's own share of first audio.** Fish latency is first nonzero audio minus first flush sent. That number is not recorded.

**Add to `LiveStats`, all on the shared clock:**
- `ws_ready_ms`, recorded after the `start` event is sent in `connect`.
- `first_flush_sent_ms`, recorded in `pump` when it writes a `Command::Flush`.

Then compute per row:
- `llm_share = first_flush_sent − send`
- `connect_stall = max(0, ws_ready − first_flush_sent)`
- `fish_share = first_nonzero − max(ws_ready, first_flush_sent)`

These three figures tell you which fix below matters. Use `first_nonzero_audio_ms` as the headline metric rather than `first_decoded_audio_ms`, because decoder readiness can come before any audible sample.

Buffered, HTTP and SSE modes call `finish_text` only after the LLM finishes, so they get no text overlap by design. Do not compare them against WS on equal terms.

---

## 3. Code defects affecting latency, ranked

1. **`fish_ws::FishWsState::start_pending` / `cancel`: the next turn is blocked behind a cancelled worker.** `busy` is a single flag. `cancel()` can shut down the socket only after `connect()` has stored it in `control`. During DNS or `connect_tcp`, which can run up to 10 s, nothing interrupts the worker. A new `open()` then waits in `pending` until the old thread exits. After a barge-in during connect, the next speech start can stall for seconds.
   - **Fix:** gate concurrency with a counted permit (max 2, like `WorkerPermit` in `provider.rs`) instead of a boolean, so a new generation can start while the cancelled one unwinds.

2. **`fish_ws::connect_tcp`: no per-address timeout cap.** The first resolved address gets the whole 10 s deadline. A blackholed IPv6 address costs about 10 s before the IPv4 attempt.
   - **Fix:** cap each attempt, e.g. `timeout.min(Duration::from_millis(1500))`, and/or try IPv4 first. This is low risk.

3. **`provider::spawn_chat`: 32 ms coalescing on the speech path** (see B2).
   - **Fix:** feed Fish directly from the chat worker, or skip coalescing until the first TTS flush has gone out, e.g. while total text is under 128 bytes. The UI can keep the coalesced stream.

4. **`listen::transcribe` and `recipient_decision::choose`: a new `reqwest::blocking::Client` per call.** Each call pays a fresh TLS handshake through the proxy, and blocking reqwest starts a runtime each time.
   - **Fix:** use a `OnceLock` client, as `provider::client()` already does.

5. **`provider::client`: `pool_idle_timeout(30s)`.** Conversational gaps longer than 30 s pay TLS again to the gateway.
   - **Fix:** raise it to about 90 s and issue a cheap warm request (e.g. a GET on `models`, already permit-gated) when the composer gains focus or the mic starts.

6. **`fish_ws::publish`: silent drop when the queue has 8 entries.** With `Timing` coalesced this is unlikely to trigger, but a dropped `Finished` or `Failed` event would hang the turn.
   - **Fix:** never drop terminal events.

7. **`facts_memory::FactsMemoryState::context`: returns `None` while `busy`.** Any save, including a toggle, makes that turn silently lose recall, even though `self.records` is still valid until the save event returns.
   - **Fix:** suppress recall only during `Delete`.

---

## 4. Low-risk speed fixes (flagged and measurable)

| ID | Change | Expected effect | Risk |
|---|---|---|---|
| S1 | `--reasoning none` for DeepSeek, verified via B5 | Removes hidden reasoning from TTFT | Quality; A/B it |
| S2 | Pin the best two providers via `order`, not `only`, in the app | Keeps fallback while biasing toward speed | Low |
| S3 | Fish `latency=low`; test `chunk_length` 100 vs 160 and `condition_on_previous_chunks` | Unknown until measured | Prosody |
| S4 | First-phrase threshold 24 → 14–16 characters, as a tuning constant | Earlier first flush, saving a few tokens' worth | A short first chunk may sound clipped |
| S5 | Defect 3, uncoalesced TTS feed | Up to about 30–50 ms | Low |
| S6 | Prewarm the Fish WebSocket on composer input or mic start; close after N seconds idle | Removes `connect_stall` | Fish concurrency and billing for idle sessions is unverified; measure with `--prewarm-ms` first |
| S7 | Defects 1, 2, 4 and 5 | Mainly tail latency and turn-to-turn latency | Low |

---

## 5. ASR: prerecorded vs realtime endpointing, and Silero

**Current path (`listen::ListeningState::start`):**
1. Capture.
2. Wait for `trailing_silence_ms`, which **defaults to 1200 ms**.
3. Encode the WAV.
4. Upload with a new client.
5. Wait for the full-utterance transcript.
6. Place the text in the composer for **manual review**.

The 1200 ms silence wait alone exceeds your whole target. A prerecorded design can never overlap recognition with speech. Realtime streaming ASR overlaps recognition with speech and endpoints on partial transcripts, so the remaining cost after the user stops is the endpoint wait plus a final-segment delay. No streaming ASR adapter exists in the supplied code, and I am not assuming a specific provider API.

**Ranked changes:**
1. **Measure first.** Record `speech_end_ms`, `endpoint_ms`, `upload_start_ms` and `transcript_ms`. None of these exist today.
2. **Shorten the silence wait with Silero.** Silero's frame-level probability supports roughly 500–700 ms safely. Energy VAD should keep a longer wait. Make the default depend on which VAD is active.
3. **Add Silero hysteresis.** In `capture`, a single 32 ms frame at or above threshold sets `speech = true` and resets silence. A click can start "speech" and trigger an upload. Add a minimum speech duration (about 200–250 ms) and a negative threshold (`threshold − 0.15`).
4. **Cache the Silero session.** `SileroVad::load` runs on every recording, *before* the mic opens, so words spoken immediately are lost and start-up is slow. Keep the session in `ListeningState` and reset its state per recording.
5. **Trim the WAV.** Before `wav()`, cut leading silence down to about 200 ms of pre-roll and trailing silence down to about 300 ms. This shrinks the upload and ASR work, and matters most for the base64 Vercel path, which adds 33%.
6. **Improve resampling.** `Recording::feed` does nearest-neighbor decimation without filtering. Averaging the skipped source frames is a cheap low-pass that helps both ASR and Silero.
7. **Opt-in auto-send** after the transcript arrives, keeping review as the default. Speculatively starting the LLM on a provisional endpoint and cancelling if speech resumes is a later, higher-risk step.

---

## 6. Grillo-inspired memory, kept off the first-audio path

The current recall is cheap: in-memory, at most 256 records, bag-of-words matching, at most 6000 bytes. The risks are in placement and contention, not compute.

1. **Prompt caching.** Every listed DeepSeek endpoint supports implicit caching. If the per-turn memory block is inserted *before* the history, the prefix changes every turn and caching, which helps TTFT, is lost. Insert it immediately before the latest user turn. I cannot see the app's prompt assembly, so verify this there.
2. **Permit starvation.** `CHAT_WORKERS` allows 2 concurrent chats, and `client()` keeps 2 idle connections per host. If background extraction, consolidation or reflection runs through `spawn_chat`, it can make the next user turn fail with "Previous connections are still closing", or force a new TLS handshake.
   - **Fix:** give memory jobs a separate `MEMORY_WORKERS` permit and their own client. Start them only after the turn's `LiveEvent::Finished`, and cancel them when the user sends a new message.
3. **Read memory from the in-memory state.** `context_from_disk` does synchronous file reads and validation. Use `FactsMemoryState::context` at Send time, and keep disk recall for workers that are off the critical path.
4. **Keep LLM-proposed memories as candidates.** Background consolidation output should go into the existing `candidates` flow, with the provenance and evidence fields already present, and stay transient until accepted. Do not write it directly. This preserves the existing opt-in and supersede guarantees in `validate`.
5. **Recipient selection fast path.** `recipient_decision::choose` adds a full Vercel round trip before the chat request whenever there are two or more candidates. Add a local fast path: if exactly one candidate name appears in the message, pick it without a network call. Fall back to Jev otherwise, using the shared client from defect 4.

---

**Next step:** run the B1/B3-instrumented LLM matrix, then the pipeline run on the best two pins. The `llm_share`, `connect_stall` and `fish_share` breakdown will show whether the remaining gap to 500–700 ms sits in routing, connection setup or Fish synthesis. No component can be credited with closing the gap until that data exists.
