# Opus: remaining-feature audit and first bounded patch

Date: 2026-10-18. Read-only review. No Cargo runs, no app-source edits, no network or paid calls, no desktop input.
Evidence labels:
- **[src]**: inspected source.
- **[test]**: an in-repo unit or fixture test (fake server, no live provider).
- **[bench]**: root's recorded live benchmark JSON.
- **[catalog]**: `/home/subsect/Documents/Codex/2026-10-17/ma/work/vercel-benchmark-catalog.json`.
- **[none]**: no evidence.

Nothing below is desktop, hardware or audible acceptance.

## Revision 2 (after root review, same day)

Corrections to sections 0 and P1–P2 below. Where they conflict, these take precedence.

- **New benches. [bench]** All runs are Fish WS + drama-3, n=3, medians. "First audio" means decoder readiness.

  | File | Model, settings | TTFT | First audio |
  | --- | --- | --- | --- |
  | `pipeline-no-reasoning.json` | `gemini-2.5-flash` @ none | 502 ms | 1041 ms |
  | `pipeline-no-reasoning.json` | `gemini-2.5-flash-lite` @ none | 509 ms | 1059 ms |
  | `pipeline-latency-routing.json` | `gemini-3.5-flash` @ low, `sort=ttft`, with drama-3 | 2830 ms | 3327 ms |
  | `pipeline-latency-routing.json` | `gemini-3.5-flash` @ low, `sort=ttft`, with s2-pro | 3475 ms | 3975 ms |
  | `pipeline-small-chunks.json` | `3.5-flash-lite` @ low, chunk 100, min 0, no conditioning | 822 ms | 1298 ms |

  - `gemini-omni-flash-preview` failed 3/3 because it rejects system instructions.
  - Routing: latency routing showed no win, so "routing unmeasured" is now stale.
  - Small chunks: no clear win; the run is within noise of the earlier lite @ low (~0.64 s TTFT, ~1.14 s first audio).
- **Best measured is still ~1.04–1.15 s first decoded audio.**
  - The leading configurations: 2.5-flash and 2.5-flash-lite @ none, and 3.5-flash-lite @ low.
  - Fish WS remains ~0.5 s after the first token.
  - The 500–700 ms target is not reached by any measured text-LLM + Fish path. Only a realtime or direct-speech transport, or a Fish first-audio cut, can close the remaining ~0.4–0.5 s.
- **Default delivery is WebSocket.** The app's default is `VoiceDelivery::LiveWebSocket` (`src/voice.rs:70`, upgrade test at `src/data.rs:545-552`). The enum's `#[default] Buffered` (`src/voice.rs:23-26`) is not what users get. So P1 is **not** on the default path. It only helps users who chose Buffered, HTTP or Vercel speech. Demote it below P3 and the capability validation.
- **P1 is not shippable as written.** `advance_speech_queue` starts the next item only when `!voice_busy && !playing` (`src/studio.rs:342-360`). Chunk N+1 would be synthesized only after chunk N finishes playing, which adds a full TTS round trip of silence between sentences.
  - A correct P1 needs a prefetch stage: start synthesis of chunk N+1 while N plays, holding at most one decoded buffer, with cancellation on stop.
  - It also needs a gap measurement (end of N to start of N+1 audio) in the bench.
  - Do not implement it blindly. A design note is required first.
- **P2 is withdrawn.** Silently mapping None or Minimal to Low would re-enable reasoning against an explicit user choice. Replacement **P2′**:
  - Parse `reasoning_options` from the Vercel catalog in `model_catalog.rs`, which currently keeps IDs only (`:307-357`).
  - Restrict the effort dropdown to the listed values.
  - Reject unsupported explicit efforts at send time with a clear error, rather than sending or rewriting them.
  - Auto (no field) stays as is.
  - Tests:
    - Catalog fixture parsing for 2.5 (none…high, toggle) and 3.5 (low/high).
    - A body/validation error for `3.5-flash-lite` + None.
- **Revised first-patch order:**
  1. P3: the first-flush WS threshold. This is on the default path. Measure with root's bench.
  2. P2′: capability validation.
  3. A prewarm of the Fish WS session on composer focus, which is on the default path.
  4. A P1 design plus prefetch, only if non-WS speech matters.

## 0. Critical path to 500–700 ms Send→first audio (highest priority)

### What the evidence says
- **LLM first token dominates. [bench]** `benchmarks/pipeline-finalists.json` medians, all with `reasoning_effort=Low` (`examples/bench_pipeline.rs:328`):
  - `google/gemini-3.5-flash`: TTFT 2.3–3.5 s. Generation after the first token (`llm_done − first_token`) is only ~0.1–0.2 s, so the cost is pre-token reasoning or queueing, not output length.
  - `google/gemini-3.5-flash-lite` + Fish WS (s1 or drama-3): TTFT ~0.64 s, first decoded audio ~1.14–1.16 s. This is the best measured path, and it is ~2× the target.
  - The Fish WS share is ~0.5 s (first decoded audio minus TTFT). Note that "first decoded audio" is decoder readiness, not speaker onset (the bench `method` string says so).
- **Gemini 3.x on Vercel cannot turn reasoning off. [catalog]**
  - Every 3.x language model (`3-flash`, `3.1-flash-lite`, `3.5-flash`, `3.5-flash-lite`, `3.6/3.7/3.8-flash`) and `3.8-live` lists only `reasoning_options: effort ∈ {low, high}`.
  - Only the 2.5 family (`2.5-flash`, `2.5-flash-lite`, `2.5-pro`) and `omni-flash-preview` list `toggle` plus `effort ∈ {none, low, medium, high}` plus `budget_tokens`.
  - So "Gemini 3.5 Flash with no reasoning" is not an offered Vercel option. Low is the floor. **The expectation that Gemini 3.5 Flash is fastest is contradicted by the bench**: Flash-Lite at Low is ~4× faster to first token.
- **The app sends unsupported efforts. [src]**
  - `ReasoningEffort` offers None/Minimal/…/Max for every model (`src/provider.rs:19-31`).
  - `chat_body` sends whatever is chosen (`src/provider.rs:337-347`).
  - The automatic effort is Minimal, and only for `openai/gpt-5*` (`src/provider.rs:337-344`).
  - Nothing reads the catalog's `reasoning_options`: grep finds no use in `src/`, and `model_catalog.rs:307-357` keeps IDs only.
  - Effect: choosing None or Minimal for `gemini-3.5-*` sends a value the catalog does not list. Whether Vercel rejects it, clamps it, or ignores it is **unknown [none]**.
  - With "auto" (no effort field), the Gemini default is also **unmeasured**. Every bench row was forced to Low.
- **Routing sort exists but is unmeasured. [src]**
  - Vercel `sort` is mapped to `ttft` / `tps` / `cost` under `providerOptions.gateway` (`src/provider.rs:182-193`, `350-357`).
  - The bench has a `--routing latency` flag (`bench_pipeline.rs:329-330`), but every recorded file says `"routing": "default"`.
- **There is no per-model upstream provider list. [catalog]** The catalog's keys have no `providers` or `endpoints` field. The order/only lists are free text (`src/provider_controls.rs:93-111`). Exact provider slugs for Gemini (for example a Vertex vs AI Studio slug) cannot be stated from local evidence. They need a public, unauthenticated docs or endpoint read, which I did not make.
- **`google/gemini-3.8-live` is a realtime model on Vercel. [catalog]** It is `type:"realtime"` with `tags:["websocket-realtime"]`, audio and text in and out, effort low/high, and `zdr:"none"`.
  - It would remove the separate TTS hop, but there is no client for it in `src/`, and its voice would not be the character's Fish voice.
  - The catalog also has speech models `google/gemini-3.8-flash-tts` and `-lite-tts` (effort low/high).
  - Feasibility verdict: a separate experimental transport, not the first patch.

### Pipeline gaps in our own code (fixable without new providers)
1. **Only Fish WebSocket overlaps the LLM with TTS. [src]**
   - `feed_reply_speech` / `finish_reply_speech` act only when `ws_reply_job == id` (`src/studio.rs:481-497`).
   - Otherwise `Finished` calls `speak(full content)` (`src/studio.rs:956-966`).
   - So Buffered (the default, `src/voice.rs:23-30`), HTTP-WAV, SSE and Vercel speech all wait for the whole reply before the first TTS byte is requested.
2. **Scene replies speak only after each full reply, and recipients run serially. [src]** See `src/scene_chat.rs:443-508`, with `Event::Speech` at `505-508`.
3. **The WS first chunk waits for a clause, a sentence, or 68 chars. [src]** `src/fish_ws.rs:76-99` (`count >= 68`). At ~100 chars/s output that is an extra ~0.1–0.6 s before Fish gets text. A first-chunk-only lower threshold, for example the first clause or 24 chars, is the cheapest knob.
4. **Pre-request work on the UI thread. [src]** `send_chat_with_semantic` polls and selects facts, runs lexical recall, and builds affect and semantic context before `spawn_chat_with_images` (`src/studio.rs:734-834`). These are bounded and in-memory, so they are probably milliseconds but **unmeasured**.
5. **Connection warmth. [src]**
   - The chat client is shared with `pool_idle_timeout(30s)` (`src/provider.rs:568-582`). After 30 s idle, the first turn pays TCP+TLS again through the proxy.
   - Nothing prewarms the Fish WS socket before Send.
   - The Jev client is built per call (`src/recipient_decision.rs:109`), but Jev is off the hot path (explicit button only).
6. **Affect is already off the first-token path. [src]** It is a trailing `<waifu-affect>` block applied on `Finished` (`src/reply_affect.rs:9-13`, `src/studio.rs:920-935`). Keep it that way, and never move animation choice onto first token or audio.

### Honest critical-path proposal
These steps cost 0 paid calls in code. Root's benches decide the rest.
- **A. Model choice (root bench).**
  - Make `gemini-3.5-flash-lite` @ Low + Fish WS the default fast pair.
  - Root to A/B `gemini-2.5-flash-lite` @ `none` (the catalog allows it) and `3.1-flash-lite` @ Low.
  - Also A/B auto (no field) vs `low`, and `sort=ttft` vs default.
  - Expected floor from current data: ~0.6 s TTFT + ~0.5 s WS = ~1.1 s. Reaching 500–700 ms requires the LLM TTFT to fall below ~0.25 s. No measured Vercel Gemini row reaches that.
- **B. Overlap in code.** Sentence-chunked buffered speech for non-WS deliveries (patch P1 below). This turns "full reply + TTS" into "first clause + TTS". It does not beat WS.
- **C. Warmth in code.**
  - Raise the chat pool idle timeout to about 90 s, plus an optional cheap `GET /v1/models` keep-warm while the composer is focused. That GET is unauthenticated catalog traffic, so cost to verify.
  - Open the Fish WS session on composer focus or typing, not on Send. `fish_ws` already supports separate open and flush (`src/fish_ws.rs:112-122`).
- **D. Realtime experiment (later, separate).** `google/gemini-3.8-live` over WebSocket, or the Vercel `gemini-3.8-flash-tts` speech path. Only this class can plausibly hit <700 ms, and it sacrifices the Fish character voice and lip-sync calibration. It needs its own spike and an explicit paid-call approval.

## 1. Evidence memory vs WW4 Grillo

**Ours. [src, test]** `src/facts_memory.rs`
- Per-character `root/<uuid>/facts.jsonl`.
- `MemoryRecord{id, kind(Fact|Preference|Question|Reflection), text, turn_id, source, evidence, created_at, expires_at, supersedes}` (`:20-39`).
- Caps: 1 MiB and 256 records (`:17-18`). O_NOFOLLOW reads (`:500-505`).
- Dedup: exact `normalized()` text per kind only (`:240-246`, `:451-456`, `:471-482`).
- Corrections: a new record with `supersedes`, an earlier-ledger check, and forget that removes the ancestor chain (`:306-358`, `:485-493`).
- Expiry: `expires_at` filtered at recall (`:398`). Expired records are never pruned from disk.
- Candidates: only from the prefixes "remember …", "i like …" and "i dislike …" (`:251-290`), and they need explicit acceptance.
- Recall:
  - Unweighted token-overlap count (no stopwords, stemming or recency decay).
  - Top 6, ≤ 6000 bytes, sent as an untrusted *user* message (`:384-428`).
  - The query is only the current composer text (`src/studio.rs:789`), or the scene text (`src/scene_chat.rs:452-454`).
- `RecordKind::Reflection` exists, but **nothing writes it**.

**WW4. [reference src]** `server/src/memory/GrilloWorkerService.ts` (2429 lines)
- An interval-driven worker (`:525`) that runs typed "beats": relationship, consolidation, curiosity, tag elaboration, compaction, reflection (`:1781-1835`).
- An LLM tool loop with `core.worker_memory_search`, `candidate_write`, `memory_write` and `diary_write` (`:1720-1729`, `:1887-1888`), with `evidence_turn_ids` on candidates (`:373`, `:1409`).
- Lexical score = 1 for the full-substring hit + 0.2 per matched word (`:2381-2393`). Search limit 1–20, default 5 (`:1328-1372`).
- Recalled confidence clamp default 0.72 (`:1221`).
- Consolidation "does not delete raw records" (`:1796`).
- Gap: the WW4 beat worker, diary, confidence and slot memory are **absent** here.

**Bounded slices (later, after P1):**
- **M1 recall quality (pure function).**
  - Add a stopword list and the WW4 1 + 0.2·n scoring with a recency tiebreak.
  - Include the last assistant turn in the query.
  - Tests: ranking fixtures in `facts_memory.rs`.
  - 0 dependencies.
- **M2 near-dup.** Jaccard ≥ 0.8 on token sets within the same kind is rejected as a duplicate in `insert`. Test: a paraphrase is rejected.
- **M3 expiry prune on save.** `validate` / `submit` drop records with `expires_at < now` and log the count. Test: an expired record disappears after the next write.
- **M4 reflection worker.**
  - Explicit "Reflect now" plus an optional idle trigger (≥ N user turns, never during speech).
  - Off-thread job: last K turns + current ledger → strict JSON `{kind:"reflection", text, evidence_turn_ids[]}` → *candidates* (still user-accepted).
  - Reuses `provider::spawn_chat` with a non-streaming JSON prompt.
  - It must never run on the Send path.

## 2. Multiple companions, recipients, animation categories

**Ours. [src, test]**
- Up to 4 avatars (`src/scene_roster.rs:277`).
- Scene recipients come from explicit picks, local name mentions (`src/scene_chat.rs:70-115`), or the opt-in Jev evaluation choice. Jev runs on an explicit button only, with freshness and UUID validation (`src/scene_chat.rs:149-198`, `src/recipient_decision.rs:43-168`; fake-server test at `:178-257`).
- Replies are serial. Each recipient sees only its own history, not other companions' lines in this round (`src/scene_chat.rs:443-500`). There is no affect in scene replies.
- **Decider/Luna and direct OpenAI Decisions: [none].**
- **Animation category decision: [none].**
  - The motion library is only a file scan (`src/motion_library.rs:21-60`).
  - Behavior phases are Idle/Thinking/Speaking for expressions only (`src/behavior.rs:80-140`).
  - No reply → motion (wave, think, idle variant) mapping exists. WW4 ships `Idle*.fbx`, `Thinking.fbx` and `Wave.fbx` in `public/assets/animations`.

**Slices:**
- **R1 typed decision result.** Extend `recipient_decision::choose` to return `{recipient, motion_category}` from one Jev request with two `choice` questions. The second question's criteria are a fixed enum: `idle|wave|think|nod|none`.
  - Apply the motion **on reply `Finished` or at speech start**, never before the first token.
  - Test: extend the existing fake-server test to assert two questions and enum validation.
- **R2 scene visibility.** Append the previous recipients' lines of this round to later recipients' context, as one bounded untrusted user turn. Test: the second recipient's request contains the first reply.
- **R3 scene streaming speech.** Reuse P1's chunker per recipient.
- **R4 local motion map.** A map from `AffectEmotion` to motion category (no network), as a fallback when Jev is off.

## 3. Bot agent loop / Codex+Claude delegation

[src] This exists in bounded form:
- `src/agent_loop.rs`: no I/O, a step limit and stop reasons (`:13-40`).
- `src/agents.rs`: Codex CLI and Claude Code commands (`:20-70`), a 32 KiB prompt cap and a 256 KiB output cap (`:15-16`).
- `src/delegation.rs`: proposal parsing, and the task board.

[test] Local process tests only. No real-agent acceptance. Spawned CLIs inherit the environment, so the cliproxy routing must already be set in the launching session. Not verified here.

No new slice is proposed until the latency work lands.

## 4. Settings UI and live VRM preview

[src] Full-page settings (`src/ui.rs:299-306`). An offscreen preview camera with the `VisualCamera` marker renders the live avatar (`src/avatar_preview.rs:70-100`).
- The preview docks only on **Characters and Scene** (`src/ui.rs:304-306`, camera active only there, `src/avatar_preview.rs:129-130`). It is absent on Voice, Connections and the other tabs. This is acceptable for appearance edits.
- [headless render] `previews/settings-*-fullpage.png` are offscreen captures, not mouse acceptance (`VALIDATION.md:143`).

Small fix candidate:
- The preview target is `Rgba8UnormSrgb` (`src/avatar_preview.rs:76-81`). Bloom and other HDR effects in the preview may not match the main view. Needs a headless capture check.
- Missing piece: a reasoning-effort dropdown filtered by the catalog's `reasoning_options` (see P2).

## 5. Shader / postprocess parity

[src]
- Bevy built-ins only: tonemap, exposure, saturation/contrast/gamma, bloom, chromatic aberration, and MToon/PBR material tuning (`src/visuals.rs:13-80`).
- Suppressed in desktop-transparent mode.
- MToon tuning mirrors WW4 `mtoonTuning` fields (`src/shaders.rs:54-60`).
- No custom fullscreen pass exists: no vignette, scanline, film grain, or RGB-split beyond Bevy CA.
- `FEATURE-STATUS.md` honestly states there is no pixel parity.
- WW4 effect list: my reference survey agent stopped before finishing. A grep shows postprocess code in `WebWaifu4/src/components/menu/SettingsPanel.tsx` (chromatic/vignette-class terms), but I did not verify uniforms or defaults. **Parity remains unassessed.**
- Slice (later): one Bevy `FullscreenMaterial` / post-process node for vignette and scanline, with a headless before/after capture via `examples/render_rooms.rs`.

## First bounded patch for root approval

**P1: sentence-chunked speech for non-WebSocket deliveries (focused chat).**
- **Why first:** it is the largest code-only latency gain for Buffered, HTTP and Vercel speech. It has no provider or paid-call change and keeps animation and affect off the path.
- **Files:** `src/studio.rs` only.
- **Change:**
  - When `!config.delivery.is_websocket() && delivery != TimestampSse` and voice is enabled, create a `waifu_studio::speech_text::ReplySpeechText` (existing, `src/speech_text.rs:6-57`, already strips the ```` ```waifu_tasks ```` block and affect).
  - In `feed_reply_speech`, push deltas into it. At each sentence boundary, emit the first chunk once it is ≥ 1 clause, then subsequent chunks at sentence boundaries of ≥ 60 chars.
  - Emit chunks into the existing `speech_queue` via `queue_speech` (`src/studio.rs:292-319`). That queue has an 8-entry FIFO, a 12k cap and `advance_speech_queue` at `:342`. Add a non-scene flag so `scene_speech_active` semantics stay unchanged.
  - `finish_reply_speech` flushes the tail and returns true, so `:956-966` does not re-speak the whole reply.
  - Keep `stop_voice` clearing the queue (`:254`).
- **Tests (unit, no network):**
  1. Deltas "Hi there. How are you today? I'm fine." produce ≥ 2 queued chunks, in order, with the full text equal to the visible reply.
  2. ```` ```waifu_tasks ```` and `<waifu-affect>` content are never queued. This extends the existing test at `src/studio.rs:1682`.
  3. Stop speech mid-reply clears the queue, and `Finished` does not re-speak.
  4. A WS delivery still takes the old path (existing tests are unchanged).
- **Acceptance limits:** the unit tests prove ordering and filtering only. Audible gaps between chunks and the real Send→first-audio time need root's live pipeline bench with buffered, Vercel and HTTP modes, plus a speaker listen. Chunk boundaries may add prosody seams.

**P2 (tiny, can ride along or follow): effort guard for Vercel Gemini 3.x.**
- In `chat_body` (`src/provider.rs:337-347`), when `route == VercelGateway` and the model starts with `google/gemini-3`, map None/Minimal to `low`. Leave auto as is pending root's A/B.
- Test: body assertion for `google/gemini-3.5-flash-lite` with None → `"low"`.
- Basis: [catalog] effort ∈ {low, high}. Vercel's runtime behavior for other values is unverified.

**P3 (tiny): WebSocket first-chunk threshold.** In `fish_ws.rs:82`, use a lower first-flush threshold (first clause or 24 chars) before switching to the current rule. The fixture tests in `fish_ws.rs` assert the flush points.

Order: P1 → P2/P3 → root bench (2.5-flash-lite@none, 3.5-flash-lite@low, sort=ttft, warm vs cold) → R1 → M1–M3 → M4 → shader pass → realtime spike.

## Blockers and gaps in this audit
- I could not deliver early findings to root over mux: `agent_send` to `root` and `parent` returned "unknown or ambiguous alias", and no owner threadId was visible in `task_list`.
- The WW4 reference survey sub-agent stopped before completing. WW4 facts above come from direct greps only (Grillo lines cited). Decider/Luna, WW4 routing and the WW4 shader list are not verified.
- Vercel per-model provider slugs and runtime handling of unlisted efforts are unknown without a network read or call, which I did not make.
