# Decision and vision options for the native companion

Research date: 2026-10-17. Read-only source inspection and public documentation; no provider calls, credentials, runtime benchmarks, or application edits.

## Recommendation

Keep one typed decision interface with interchangeable Jev and OpenAI Decisions adapters. Use a separate conversational model for speech and explanations. Run gaze, blinking, locomotion, animation blending, and audio-driven lip sync locally in Bevy. Neither decision service belongs in the render loop.

For the first version, implement deterministic local behavior first, then OpenAI Decisions for optional image-aware choices and Jev for the existing state/question workflow. Preserve the ability to select either provider rather than declaring a speed or quality winner before measurement.

## What is actually verified

OpenAI's fetched official documentation explicitly describes a **public beta Decisions API**, `POST /v1/decisions`, with `gpt-6-luna` as its currently supported model. This was independently verified from public documentation; it is not inferred from the desktop mux model alias. The API accepts text and images and returns predicate, choice, or score answers. A score is an expectation over ordered levels, not an arbitrary continuous motor command. OpenAI advertises roughly 10× lower latency than Responses; that is its product claim, not a measurement for this companion. [Official Decisions guide](https://developers.openai.com/api/docs/guides/decisions)

The API reference supports image URLs or base64 data URLs, 1–200 questions per request, and up to 128 images. It does not accept audio, tools, file references, or item references. Individual answers can be refusals. These API ceilings are not sensible default operating budgets for a desktop pet. [Official create-decision reference](https://developers.openai.com/api/reference/typescript/resources/decisions/methods/create)

For richer extracted objects, use a separate Responses adapter with a strict JSON Schema; function calling fits requests to execute application tools. Structured output constrains format, but application-side permissions, value checks, freshness, and action limits still apply. [Official Structured Outputs guide](https://developers.openai.com/api/docs/guides/structured-outputs)

## Existing local work worth carrying forward

`/home/subsect/Work/jev-selfbot/src/brain.mjs:78–130` already treats Jev as a typed decision model. It resolves single-choice questions locally, calls the Vercel evaluation-model route with `typesafe-ai/jev`, records the backend, and falls back after retryable errors with a five-minute Jev cooldown. Its current generic fallback uses JSON-object chat output plus explicit validation for booleans and choice enums; it does not provide calibrated probabilities. The native implementation should retain the useful adapter and validation pattern, while keeping API calls asynchronous.

The inspected Jev request serializes `state` as JSON text. This path establishes text/state decisions; it does **not** establish direct image support in Jev. To use Jev for visual behavior initially, let a vision component produce timestamped observations, then pass those observations to Jev. Do not silently flatten a screenshot into a pretend supported Jev image request.

WebWaifu4 already contains an opt-in stream vision pattern: `src/App.tsx:2197` captures Twitch frames behind an enabled flag and busy guard; `src/lib/chat/defaults.ts:202` defaults vision off, low detail, 120-second capture spacing, and 180-second maximum age. Carry forward the explicit switch, non-overlap, and freshness ideas. This is source evidence of a browser/Twitch feature, not verification of native desktop capture or a working image decision backend.

## Native contracts

Use separate Rust contracts and explicit capability reporting:

| Component | Input | Output | Suitable backend |
|---|---|---|---|
| Decision policy | Allowed actions, compact state, recent events, optional fresh image | Named predicate/choice/score results with backend and timing | Jev adapter; OpenAI Decisions adapter; local rules |
| Visual observation | User-selected capture source and timestamped frame | Typed observations with source frame ID and expiry | Vision-capable Responses model; direct Decisions for bounded visual questions |
| Conversation | User turn, persona, permitted memories and selected observations | Response text and speech intent | Configurable conversational model |
| Embodiment | Speech audio, emotion values, selected intent | Blendshape weights, gaze target, animation transitions | Local Bevy systems; optional experimental controller |

Normalize provider results into named answers while preserving provenance. A missing answer, invalid enum, refusal, expired frame, timeout, unavailable backend, or unknown schema must resolve to `abstain`/`idle`; do not fabricate confidence. Keep decision epoch and avatar/entity ID so a late answer cannot override a newer user command or attach to another imported avatar.

Example bounded questions: “Which available gesture matches this utterance: none, nod, wave, shrug?”; “Is the selected application visibly waiting for input?”; “Which named region contains the user's indicated object?” The application owns executable actions and exact targets. Image evidence supplies observations, not permission to operate the computer.

## Proposed operating budgets

These are starting design limits to measure and tune, not observed performance:

- Render and animation: local every frame; no network wait on a Bevy schedule.
- Semantic decisions: event-driven; at most one request in flight and one replacement pending per avatar; debounce bursts by 250 ms and cap background decisions at one per 2 seconds.
- Vision: off until enabled for an explicit source; capture on request or material change; begin with a 10-second minimum interval, one cropped/downscaled frame, and a 15-second observation expiry. Continuous streaming is a separate opt-in mode.
- Network decision deadline: 1.5 seconds, then local fallback. No repeated cloud retries for transient gestures. Conversation and explicit detailed visual questions may use separately configured longer deadlines.
- Bound state length, question count, frame dimensions, daily request budget, and provider spend. Stop cloud work when the budget is reached. Record usage returned by the provider; exact cost estimates require current account/model pricing and actual image token usage.
- Store capture source, size, age, chosen action, backend/model, latency, and refusal/fallback status. Avoid retaining raw frames by default.

Configure `provider`, `base_url`, `decision_model`, `vision_model`, `conversation_model`, deadline, capture interval, and budgets independently. `gpt-6-luna` is a verified current Decisions model ID, but keep it editable and validate provider capabilities; a gateway slug, mux alias, and direct OpenAI model ID are distinct routing identifiers. Do not assume a Rust SDK already exposes a newly beta endpoint: a narrowly scoped HTTPS adapter is sufficient.

## Jev versus Luna evaluation

Build one held-out set of avatar situations with identical allowed actions and expected abstentions. For text decisions, feed both providers the same normalized state. For images, distinguish direct-image Luna from a vision-to-text-plus-Jev pipeline; measure and price the entire latter pipeline. Report action accuracy, unnecessary actions, abstention/refusal rate, p50/p95 end-to-end latency, stale-result rate, and cost per accepted action. Include network failures and interruption while a request is pending. A simulated policy replay is not a provider-backed comparison; no live comparison was run here.

## Optional flybrain experiment

Keep a flybrain/connectome-inspired controller behind an experimental embodiment interface, with the conventional animation controller available as the baseline. It may map bounded sensory/affect values into gaze, fidget intensity, posture, or expression intensity. Speech timing should continue to drive lip sync directly.

This is a design hypothesis informed by prior project discussion, not a newly verified simulator integration. A connectome does not inherently label outputs as “smile” or “look interested”; an engineered or trained decoder and evaluation are required. Persistent neural state alone does not establish learning, personality, or autobiographical memory. The exact earlier screenshot simulator was not identified. Do not make it a dependency for avatar import, conversation, or reliable idle behavior. Primary dataset entry point for later source verification: [Janelia MaleCNS downloads](https://male-cns.janelia.org/download/), not inspected in this pass.

## Evidence boundary

Confirmed: named local source paths inspected; official OpenAI guide/reference pages fetched. Not confirmed: account access, billing rates, gateway support for Decisions, direct Jev image capability, native capture, measured response times, native Bevy integration, flybrain runtime, or a provider-backed head-to-head.
