# Character customization and behavior roadmap

This is a researched design, not a claim that these behavior features are already implemented in Waifu Studio. Base desktop app comes first; XR/VR follows a usable avatar, chat, voice, and settings workflow.

## Recovered implementations

- WebWaifu4 survives at `/home/subsect/Documents/Codex/2026-10-11/files-pasted-by-the-user-review-2/work/WebWaifu4`. Its `src/lib/chat/affect-bridge.ts` implements persistent V/A/D blending with 45-minute decay; `reply-metadata.ts` normalizes emotion and affect; `prompt.ts` carries affect into the writer; `src/components/VrmStage.tsx` maps affect into facial weights. `src/App.tsx:4745` updates affect from reply metadata. This surviving checkout is not proof that Hikari's separate appraisal route survived.
- Audrey survives at `/home/subsect/jev-discord-demo`. `src/backend.ts` calls the Jev evaluation model through the Vercel gateway, asking typed questions for expression, mood, behavior, response pace, memory relevance, and participation. `src/expression.ts` deterministically compiles selected feelings into prompt blocks; `src/state.ts` handles mood decay/nudges; `src/drives.ts` handles slow drives; `src/dashboard.ts` exposes decisions and internal state.
- Jev selfbot survives at `/home/subsect/Work/jev-selfbot`. `src/brain.mjs` contains a typed Jev gateway request. `src/hikari/provenance.json` pins the copied Hikari speech subset to original revision `9a393201248cc521ee1a80259e764e51fa939e4e`. Copied sources include Fish WebSocket speech, VAD and transcription, under `src/hikari/source/`. The full original Hikari checkout was not found at its recorded path.
- Prior Hikari work recorded a separate `/companion/appraise`, mood update before writing, deterministic `voice-direction.ts`, and Drama 3 directions. These are historical records, not recovered full source. Do not silently treat the copied speech subset as a full Hikari reconstruction.

## Character engine contract

Use separate adapters for writer, appraiser, memory retrieval, voice synthesis, transcription, and embodiment. One turn should produce a validated receipt: provider, status, timing, previous affect, appraisal, chosen behavior, compiled directions, and resulting affect. Distinguish disabled, local fallback, provider success, and failure in the UI.

Keep the rule “Jev selects; code compiles.” Appraisal returns bounded values and known behavior IDs. Deterministic code handles smoothing, voice syntax, face weights, animation blending, and interruption. Speech audio drives lip-sync independently of affect.

Normalize scales deliberately: Audrey uses all three V/A/D axes in `[0,1]`; WebWaifu4 and recorded Hikari use valence/dominance `[-1,1]` and arousal `[0,1]`. Convert Audrey valence and dominance as `2*x-1`; do not convert arousal. Update persistent mood once per turn. Reply emotion may drive the face without becoming a second appraisal.

A provider adapter can later support Jev or an OpenAI Decisions endpoint. The later DECISION-OPTIONS.md verifies current official OpenAI Decisions documentation; no live provider call or runtime integration is claimed. An evaluation adapter is not automatically interchangeable with Decisions. Validate request schemas, answer types, probability semantics, and timeout handling before enabling either. No paid calls were made.

“Javala” was not identified in the searched memory registry or recovered Jev source. Do not invent a provider from the name. The earlier fly research references Janelia MaleCNS, but that is not evidence that Javala means Janelia.

## Customization priorities

| Stage | Controls |
| --- | --- |
| Base desktop | Per-character name, avatar and source license, persona, writer connection/model, scene and lighting, camera, scale and placement, expression bindings, saved presets, import/export, chat history controls |
| Behavior | Baseline V/A/D, reactivity, recovery/decay, emotional expressiveness, curiosity/social energy, reply pace, initiative and quiet hours, allowed behaviors, appraiser selection, confidence thresholds, readable decision receipts |
| Voice | Provider and voice ID, speaking rate, volume, interruption, microphone/transcription source, consent for remote audio, lip-sync gain/smoothing/vowel weights; compile only provider-supported emotion directions |
| Memory | Per-character and per-user scope, inspect/edit/delete, explicit versus proposed memories, retention and retrieval controls, provenance and correction handling |
| Embodiment | Gaze, blink, breathing, head/posture/fidget intensity, expression blend limits, spring-bone tuning, grounded animation transitions, optional controller selection |
| Later XR/VR | Session/device setup, tracking origins, scale and calibration, interaction mapping, spatial audio and comfort controls, permission-scoped sensing |

Prefer a good default plus an Advanced section. Presets should be versioned data; secrets belong in a separate credential store and must not be exported with a character.

## Optional fly connectome controller

Prior research inspected `https://github.com/MarkUnthank/flyhard`, specifically `src/flyhard/connectome.py` and `motor_policy.py`. The record describes a sparse recurrent model with measured adjacency, trainable bounded gains/leak, and four graph updates after resetting state on each control decision. Its inputs concern wheels/joints, not camera understanding. A local working fly-to-avatar integration was not located.

A future experiment could map bounded conversation/audio events into a neural controller and decode outputs into gaze, posture, fidgets, or expression intensity. Explicitly engineer or train those mappings; a fly connectome does not inherently encode smiles, human emotions, autobiographical memory, or language. Keep the writer and speech lip-sync independent. Run the controller behind the same bounded embodiment interface as a conventional animation state machine, with reset, reproducible seed, performance budget, and an immediate fallback. Compare coherence and responsiveness before claiming an improvement.

The exact earlier screenshot simulator was never identified; neuron counts alone did not establish its identity. Related historical leads were Fly Brain Minecraft, Chreatures, and Doomfly. Their presence in prior research is not implementation evidence for this app.

## Current base implementation

README.md and VALIDATION.md are the implementation and test receipts. The current base includes VRM0/1 rendering, three shader modes, BVH/VRMA/FBX rotation imports, profile/scene/provider/facial controls, Fish-compatible speech with amplitude lip-sync and timestamp SSE, explicit microphone/VAD/STT, image attachments, manual MCP calls, reviewed concurrent task cards, and per-character explicit local notes with optional semantic lookup. Scene-recipient Jev suggestions, focused-reply emotion/VAD metadata and an evidence ledger with reviewed candidates are now connected. General decision-based embodiment, automatic Grillo reflection/promotion, material expressions, journal/routines, connectome and XR remain roadmap items. TAMANITOMO-FEATURES.md records the separately reviewed routine/reflection features.
