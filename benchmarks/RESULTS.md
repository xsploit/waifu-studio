# Native pipeline benchmark — 2026-10-18

Target: **500–700 ms from Send to first usable speech audio**. This target has **not been reached** on the measured Vercel → Fish path. These are real authenticated calls through the configured proxy, using the production Rust adapters. No OpenRouter calls were made.

## What the measurements mean

- The clock starts before the LLM request and WebSocket connection. WebSocket sessions open immediately and receive LLM deltas; HTTP, SSE and buffered modes wait for the completed LLM response, matching the native implementation.
- First token means first non-empty text delta. First audio means a production decoder was available, and nonzero samples were verified separately. No speakers or audio device were opened; device latency, audible onset, dropouts and perceived quality are unmeasured.
- PCM is drained as quickly as published, without synthesized underflow silence. Network/decode completion is not real-time playback completion. `earliest_playback_end_ms` is only first-ready + sample duration, a lower-bound estimate.
- The prompts are a controlled short greeting with no history, memory, images, reply metadata or tools. Generated wording and output duration vary. This is not a benchmark of every persona or a loaded interactive session.
- Sequential network requests; candidates interleaved between repetitions. Initial requests can include cold connections. Local development/build/render work continued during parts of the run, so this is exploratory tuning, not an isolated performance certification. Three samples do not establish p95 latency or a permanent winner.
- Production startup buffering is retained (180 ms). No latency claim is based on reducing the buffering that protects against crackling.
- `transport_first_wire_audio_ms` comes from each transport and has different origins (SSE starts at synthesis; WS at session open). Use the benchmark’s common-clock first audio field for cross-mode/end-to-end comparison.

## Whole-pipeline comparison

Low reasoning, output budget 1024, balanced Fish latency, chunk160/min20/conditioning on. Medians of successful calls; failures remain in the raw results.

| LLM | Fish model | Mode | Success | First token ms | First audio ms | Token → audio ms | Network done ms | Text chars |
|---|---|---|---:|---:|---:|---:|---:|---:|
| gemini-3.5-flash | s1 | ws | 3/3 | 3154 | 3700 | 504 | 4407 | 118 |
| gemini-3.5-flash | s1 | http | 3/3 | 2448 | 3147 | 699 | 3897 | 121 |
| gemini-3.5-flash | s1 | sse | 2/3 | 3374 | 4200 | 826 | 4902 | 126 |
| gemini-3.5-flash | drama-3 | ws | 3/3 | 2949 | 3447 | 549 | 4251 | 125 |
| gemini-3.5-flash | drama-3 | http | 3/3 | 3472 | 4230 | 707 | 4901 | 117 |
| gemini-3.5-flash | drama-3 | sse | 3/3 | 3026 | 4283 | 954 | 5049 | 117 |
| gemini-3.5-flash-lite | s1 | ws | 3/3 | 652 | 1163 | 511 | 1947 | 112 |
| gemini-3.5-flash-lite | s1 | http | 3/3 | 589 | 1389 | 775 | 2324 | 113 |
| gemini-3.5-flash-lite | s1 | sse | 3/3 | 885 | 1796 | 911 | 2462 | 97 |
| gemini-3.5-flash-lite | drama-3 | ws | 3/3 | 636 | 1140 | 493 | 2154 | 116 |
| gemini-3.5-flash-lite | drama-3 | http | 3/3 | 1330 | 2039 | 746 | 2824 | 119 |
| gemini-3.5-flash-lite | drama-3 | sse | 3/3 | 1281 | 2114 | 833 | 2869 | 110 |

One Flash3.5/s1/SSE request failed before audio with a connection/timeout error. It was not retried or hidden. The other 35 calls decoded speech. The output-budget-256 screening was intentionally replaced by 1024 for these whole-pipeline comparisons because reasoning could exhaust the smaller budget.

## Additional Gemini candidates: reasoning disabled

The catalog explicitly permits `none` for these candidates. Gemini3.5/3.8 advertise Low/High only; no unsupported `none` was forced onto them. Same Drama3 WebSocket voice and baseline Fish tuning.

| LLM | Success | First token ms | First audio ms |
|---|---:|---:|---:|
| gemini-2.5-flash-lite | 3/3 | 509 | 1059 |
| gemini-2.5-flash | 3/3 | 502 | 1041 |
| gemini-omni-flash-preview | 0/3 | — | — |

Gemini Omni Flash Preview rejected the normal persona system instruction on all three calls. It is not a compatible drop-in for this chat request shape. No model fallback was attempted.

## Speech-only screening

Same fixed 63-character utterance and saved reference voice. One sample each, so these are candidates, not ranked winners. All 25 calls succeeded.

| Fish model | Buffered ms | HTTP ms | SSE ms | WebSocket ms | Timestamp WebSocket ms |
|---|---:|---:|---:|---:|---:|
| s1 | 1157 | 627 | 729 | 1122 | 1071 |
| s2-pro | 1980 | 628 | 757 | 1050 | 1067 |
| s2.1-pro | 1906 | 711 | 848 | 1163 | 1146 |
| s2.1-pro-free | 4371 | 1035 | 1043 | 1724 | 1832 |
| drama-3 | 1114 | 906 | 700 | 1096 | 1042 |

The shared sentinel fix was exercised by live timestamp SSE, including s2.1-pro-free, without the previous multi-gigabyte declared-length truncation error.

## Tuning experiments

| Experiment | Samples | First token ms | First audio ms |
|---|---:|---:|---:|
| Flash3.5Lite/Drama3 WS: chunk100/min0/conditioning off | 3/3 | 822 | 1298 |
| Flash3.5 WS: Vercel latency sort (s2-pro + Drama3) | 6/6 | 3223 | 3731 |
| Drama3 WS speech only: normal latency | 3/3 | — | 1509 |

Smaller chunks/disabled conditioning and latency sort did not establish a reliable whole-pipeline win. The normal Fish latency sample was slower than the balanced screening, with sample count/order caveats. Baseline balanced160/min20/conditioning-on is retained.

## Applied settings and remaining work

The selected profile keeps the requested **Gemini3.5Flash**, now with **Low** reasoning, and the existing **Drama3 reference voice**, now using **live WebSocket** delivery so speech can overlap generation. Keys, persona, voice identity and speaking speed are retained. The FlashLite and no-reasoning results are available alternatives; they were not silently substituted for Flash3.5. Settings were saved for the next application launch.

The current best repeatable medians are around **1.0–1.15 seconds** to decoded speech with lightweight Gemini choices. A 500–700 ms whole-pipeline target needs further reduction in both first-token time and Fish startup, or a separate realtime speech-to-speech path. No unsupported latency guarantee is made.

The WebSocket chunker now flushes its first phrase at a whitespace boundary after24 characters, short sentences at10, clauses at44 and long text at68 UTF-8 characters. The original finalist/no-reasoning runs preceded the first24-character phrase change; the later direct Drama/S2.1 and warm-connection experiments include it. It preserves every text byte, queue bounds and explicit manual-flush mode. HTTP/SSE do not yet overlap incremental sentence synthesis. No Jev/animation/network decision call has been added to the first-audio path.


## Repeated Drama 3 versus S2.1 Pro

Same saved reference voice, balanced latency, chunk160/min20/conditioning on, three interleaved samples per candidate. Both comparisons include the new first24-character WebSocket phrase policy. The pipeline uses Gemini3.5FlashLite, Low reasoning, 1024-token budget, no prewarming. No model or voice fallback.

### Send to first decoded audio, with streaming LLM

| Fish model | Mode | Success | First token ms | LLM finished ms | First audio ms |
|---|---|---:|---:|---:|---:|
| drama-3 | ws | 3/3 | 689 | 806 | 1121 |
| drama-3 | http | 3/3 | 593 | 760 | 1664 |
| drama-3 | sse | 3/3 | 701 | 846 | 1576 |
| s2.1-pro | ws | 3/3 | 667 | 807 | 1363 |
| s2.1-pro | http | 3/3 | 736 | 978 | 1665 |
| s2.1-pro | sse | 3/3 | 507 | 685 | 1429 |

### Fixed text already available; speech request to first decoded audio

| Fish model | Mode | Success | First token ms | LLM finished ms | First audio ms |
|---|---|---:|---:|---:|---:|
| drama-3 | ws | 3/3 | — | — | 1034 |
| drama-3 | http | 3/3 | — | — | 630 |
| drama-3 | sse | 3/3 | — | — | 709 |
| s2.1-pro | ws | 3/3 | — | — | 1132 |
| s2.1-pro | http | 3/3 | — | — | 689 |
| s2.1-pro | sse | 3/3 | — | — | 887 |

HTTP is the fastest measured fixed-text transport for both models, while WebSocket wins the implemented whole chat pipeline by accepting LLM deltas. HTTP/SSE currently wait for the complete reply: this is a comparison of the shipped paths, not proof that a sentence-streamed HTTP implementation would lose. That needs a prefetch stage and measured playback gaps. Responses vary in text length and LLM time; three samples do not isolate a permanent backend advantage. All timings exclude the audio device.

## Other fast LLM candidates

Drama3 WebSocket, no reasoning, latency routing; three samples each. These precede the first24-character phrase change.

| LLM | Success | First token ms | First audio ms |
|---|---:|---:|---:|
| nvidia/nemotron-3.5-lightning | 3/3 | 756 | 1219 |
| spacexai/grok-4.1-fast-non-reasoning | 0/3 | — | — |
| anthropic/claude-haiku-5.5 | 3/3 | 990 | 1599 |

The catalog-listed Grok ID returned404 on all three calls. It was not replaced. A faster individual first-token sample did not establish a faster median whole pipeline.

## Already-open Fish WebSocket experiment

The benchmark opens Fish1000ms BEFORE the Send clock, then streams the same FlashLite Low reply. This is explicit preparation outside the reported Send interval; add1000ms for preparation-to-audio. Runtime prewarming is not implemented. Three samples per model. Compare with the cold Drama/S2.1 table above; these are not simultaneous paired trials.

| Fish model | Success | First token ms | First audio after Send ms |
|---|---:|---:|---:|
| drama-3 | 3/3 | 906 | 1332 |
| s2.1-pro | 3/3 | 644 | 1187 |


## Fish Low latency with Gemini FlashLite

Gemini3.5FlashLite Low, three samples per cell; same voice and chunk settings. This run differs from the balanced experiment in both timing and backend conditions, so it is not an isolated causal estimate.

| LLM | Route | Fish | Mode | Success | First token ms | LLM done ms | First audio ms | Worst audio ms |
|---|---|---|---|---:|---:|---:|---:|---:|
| google/gemini-3.5-flash-lite | default | drama-3 | ws | 3/3 | 1020 | 1147 | 1520 | 2040 |
| google/gemini-3.5-flash-lite | default | drama-3 | http | 3/3 | 572 | 695 | 1308 | 2200 |
| google/gemini-3.5-flash-lite | default | s2.1-pro | ws | 3/3 | 613 | 779 | 1168 | 2611 |
| google/gemini-3.5-flash-lite | default | s2.1-pro | http | 3/3 | 708 | 879 | 1548 | 2287 |

Low mode is not a universal winner: Drama3 HTTP beat WS in this batch, whereas S2.1Pro WS beat HTTP. Latency is a quality trade-off; perceived voice quality is not measured.

## DeepSeek Flash: Vercel provider slugs and sorting

Exact model deepseek/deepseek-v4.1-flash, reasoning none. Public Vercel model-endpoints API verified the provider slugs. pin:slug sends providerOptions.gateway.only:[slug], preventing other-provider fallback; default/latency/throughput use default/ttft/tps. These early runs have no served-provider response metadata, so automatic routes are unattributed.

| LLM | Route | Fish | Mode | Success | First token ms | LLM done ms | First audio ms | Worst audio ms |
|---|---|---|---|---:|---:|---:|---:|---:|
| deepseek/deepseek-v4.1-flash | default | — | text | 3/3 | 739 | 870 | — | — |
| deepseek/deepseek-v4.1-flash | latency | — | text | 3/3 | 471 | 855 | — | — |
| deepseek/deepseek-v4.1-flash | throughput | — | text | 3/3 | 518 | 659 | — | — |
| deepseek/deepseek-v4.1-flash | pin:baseten | — | text | 2/3 | 260 | 352 | — | — |
| deepseek/deepseek-v4.1-flash | pin:relace | — | text | 2/3 | 462 | 1047 | — | — |
| deepseek/deepseek-v4.1-flash | pin:inference-net | — | text | 3/3 | 374 | 692 | — | — |
| deepseek/deepseek-v4.1-flash | pin:togetherai | — | text | 3/3 | 418 | 519 | — | — |

The strict baseten and relace pins each returned one HTTP429 capacity error in three attempts. Those errors were retained, with no fallback or retry. baseten had a median TTFT of260ms on its two successful calls; compare reliability as well as the successful-only median.

## DeepSeek Flash: complete pipeline with provider pins

Reasoning none, Fish Low latency, three samples per combination, no prewarming. All 36 calls in this batch succeeded, including both strict provider pins. Same saved reference voice; no speaker device.

| LLM | Route | Fish | Mode | Success | First token ms | LLM done ms | First audio ms | Worst audio ms |
|---|---|---|---|---:|---:|---:|---:|---:|
| deepseek/deepseek-v4.1-flash | pin:baseten | drama-3 | ws | 3/3 | 289 | 438 | 1047 | 1056 |
| deepseek/deepseek-v4.1-flash | pin:togetherai | drama-3 | ws | 3/3 | 332 | 372 | 1066 | 1075 |
| deepseek/deepseek-v4.1-flash | latency | drama-3 | ws | 3/3 | 360 | 600 | 1072 | 1079 |
| deepseek/deepseek-v4.1-flash | pin:baseten | drama-3 | http | 3/3 | 400 | 611 | 1224 | 1408 |
| deepseek/deepseek-v4.1-flash | pin:togetherai | drama-3 | http | 3/3 | 396 | 465 | 1103 | 1943 |
| deepseek/deepseek-v4.1-flash | latency | drama-3 | http | 3/3 | 511 | 830 | 1461 | 2921 |
| deepseek/deepseek-v4.1-flash | pin:baseten | s2.1-pro | ws | 3/3 | 348 | 1091 | 1114 | 3131 |
| deepseek/deepseek-v4.1-flash | pin:togetherai | s2.1-pro | ws | 3/3 | 395 | 414 | 1098 | 1187 |
| deepseek/deepseek-v4.1-flash | latency | s2.1-pro | ws | 3/3 | 445 | 796 | 1138 | 1325 |
| deepseek/deepseek-v4.1-flash | pin:baseten | s2.1-pro | http | 3/3 | 281 | 459 | 1139 | 1240 |
| deepseek/deepseek-v4.1-flash | pin:togetherai | s2.1-pro | http | 3/3 | 371 | 416 | 1137 | 1369 |
| deepseek/deepseek-v4.1-flash | latency | s2.1-pro | http | 3/3 | 404 | 760 | 1513 | 1598 |

## DeepSeek Flash Fast alternative

Exact Vercel model deepseek/deepseek-v4.1-flash-fast, strict only:[baseten], reasoning none and Fish Low. The public endpoint listing exposes baseten as its only provider. All 12 calls succeeded. The fast suffix did not establish an end-to-end advantage over the ordinary model.

| LLM | Route | Fish | Mode | Success | First token ms | LLM done ms | First audio ms | Worst audio ms |
|---|---|---|---|---:|---:|---:|---:|---:|
| deepseek/deepseek-v4.1-flash-fast | pin:baseten | drama-3 | ws | 3/3 | 537 | 616 | 1057 | 1132 |
| deepseek/deepseek-v4.1-flash-fast | pin:baseten | drama-3 | http | 3/3 | 453 | 526 | 1171 | 1192 |
| deepseek/deepseek-v4.1-flash-fast | pin:baseten | s2.1-pro | ws | 3/3 | 452 | 514 | 1213 | 1245 |
| deepseek/deepseek-v4.1-flash-fast | pin:baseten | s2.1-pro | http | 3/3 | 451 | 532 | 1452 | 1458 |

## DeepSeek with an already-open Fish WebSocket

The Fish connection is opened1000ms BEFORE the Send clock; add1000ms for preparation-to-audio. Five calls, strict baseten pin, Drama3 WS Low, no reasoning. Prewarming does not establish a consistent overall win. WS diagnostic clocks include that1000ms preparation and must not be subtracted directly from Send-clock fields.

| LLM | Route | Fish | Mode | Success | First token ms | LLM done ms | First audio ms | Worst audio ms |
|---|---|---|---|---:|---:|---:|---:|---:|
| deepseek/deepseek-v4.1-flash | pin:baseten | drama-3 | ws | 5/5 | 723 | 915 | 1176 | 1569 |

## Instrumented DeepSeek finalist

Five calls with the same strict baseten pin and Drama3 WS Low. WS diagnostic fields use the session worker clock, approximately aligned with Send when prewarm is zero; they measure writer acceptance, not server acknowledgement. Routing attribution is recorded only when returned.

| LLM | Route | Fish | Mode | Success | First token ms | LLM done ms | First audio ms | Worst audio ms |
|---|---|---|---|---:|---:|---:|---:|---:|
| deepseek/deepseek-v4.1-flash | pin:baseten | drama-3 | ws | 5/5 | 359 | 726 | 1070 | 1329 |

| WS worker diagnostic | Median ms |
|---|---:|
| ws_connection_ready_ms | 624 |
| ws_first_text_written_ms | 627 |
| ws_first_flush_written_ms | 627 |
| transport_first_wire_audio_ms | 1065 |

Served-provider response attribution: baseten.

## DeepSeek provider ordering with fallback

order:[baseten,togetherai] plus sort:ttft, no only restriction; other available providers may still be considered. Three successful calls, generation lookup confirmed baseten in all three; no fallback was observed.

| LLM | Route | Fish | Mode | Success | First token ms | LLM done ms | First audio ms | Worst audio ms |
|---|---|---|---|---:|---:|---:|---:|---:|
| deepseek/deepseek-v4.1-flash | latency | — | text | 3/3 | 522 | 809 | — | — |

## Gemini provider filtering comparison

Gemini3.5FlashLite Low, same short prompt, automatic/default vs ttft/tps and strict google/vertex pins. Public Vercel endpoint listing confirms both slugs. Three repetitions; no cross-model reasoning substitution.

| LLM | Route | Fish | Mode | Success | First token ms | LLM done ms | First audio ms | Worst audio ms |
|---|---|---|---|---:|---:|---:|---:|---:|
| google/gemini-3.5-flash-lite | default | — | text | 3/3 | 600 | 785 | — | — |
| google/gemini-3.5-flash-lite | latency | — | text | 3/3 | 579 | 754 | — | — |
| google/gemini-3.5-flash-lite | throughput | — | text | 3/3 | 754 | 854 | — | — |
| google/gemini-3.5-flash-lite | pin:google | — | text | 3/3 | 694 | 792 | — | — |
| google/gemini-3.5-flash-lite | pin:vertex | — | text | 3/3 | 597 | 749 | — | — |

Generation lookup verified all34 successful observed calls in the instrumented, Gemini routing, provider-order and recorded-ASR batches. The five instrumented DeepSeek calls were served by baseten. Gemini default, ttft and tps all selected vertex in these runs; strict google selected google. Provider attribution was obtained after timing and adds nothing to the latency clock. Failed calls without a generation ID remain unattributed.

The direct-stream harness bypasses the app’s32ms later-delta coalescing and frame polling. It measures the production adapter chain, not physical UI-button-to-speaker latency. The real app can add scheduling time. The baseline prompt is short and is not a full persona/history benchmark.

## Prerecorded ASR → LLM → speech

Same 16kHz mono PCM recording of a synthetic spoken test prompt. Timing begins when the complete WAV is submitted. This excludes speaking duration, microphone capture, Silero startup, trailing-silence endpointing, transcript review and audio hardware. The app still requires transcript review before Send. This is an adapter sequence experiment, not a shipped automatic conversational mode. Each transcript is used as the actual LLM user input; neither audio nor transcript is committed. Connection reuse is enabled for ASR; first requests may pay connection setup. DeepSeek V4.1Flash, reasoning none, baseten pin, Drama3 WS Low.

| ASR route | ASR success | Complete sequence | ASR done ms | First token from upload ms | First audio from upload ms |
|---|---:|---:|---:|---:|---:|
| Fish direct transcribe-1 | 3/3 | 3/3 | 690 | 1291 | 1764 |
| Fish direct transcribe-1-pro | 3/3 | 3/3 | 604 | 1002 | 1623 |
| Vercel fish-audio/transcribe-1 | 3/3 | 2/3 | 749 | 1251 | 1895 |
| Vercel openai/whisper-1 | 3/3 | 3/3 | 1822 | 2130 | 2887 |

Failure retained (ASR succeeded; the subsequent strict LLM pin failed): Provider returned HTTP 429 Too Many Requests: The request limited providers for this model and they are currently at capacity. Add more providers or remove restrictions for automatic fallback. Providers considered: baseten..
## Reproduce

From the app directory, build `cargo build --example bench_pipeline`, then explicitly run, for example:

```sh
../../work/studio-target/debug/examples/bench_pipeline \
  --run --phase pipeline --models google/gemini-3.5-flash-lite \
  --speech-models drama-3 --modes ws,http,sse --repeat 3 \
  --output /absolute/results.json
```

Use `--phase speech`, `--reasoning none|low|minimal`, `--routing default|latency|throughput|cost`, `--provider-only SLUG`, `--provider-order SLUG,SLUG`, `--routing-matrix default,latency,pin:baseten`, `--latency normal|balanced|low`, `--chunk-length`, `--min-chunk` and `--condition` to compare supported combinations. Preserve the configured proxy; the executable refuses to benchmark when none is configured. The per-sample40-second loop limit cannot forcibly interrupt an already-blocked provider read.

Raw JSON files here contain timings and errors, never API keys, chat transcripts or voice IDs. `llm-baseline.json` is an exploratory100-token screening with an obsolete error-label bug; `llm-corrected.json` used256 tokens and could still exhaust reasoning budget. Neither is used to rank the final1024-token whole-pipeline results.
