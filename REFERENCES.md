# References and provenance

## Included dependencies

- [unavi-xyz/bevy_vrm](https://github.com/unavi-xyz/bevy_vrm), revision `3c46eab833f8a63d934835606dc63141d11aa24c`, vendored in `vendor/`. Includes `bevy_vrm`, `bevy_shader_mtoon`, and `serde_vrm`; their licenses are retained. Backport: Bevy workspace version set to 0.19 and the newer `MeshCompressionArgs`/`default_mesh_compression` plugin initialization removed. The Bevy 0.20 MToon/outline shaders are ported to native Bevy 0.19 WGSL imports and vertex/morph interfaces in mtoon.wgsl and outline.wgsl; original WESL files are retained for provenance.
- [Bevy](https://bevyengine.org/): app pin 0.19.0, resolved Bevy internal packages 0.19.1 in Cargo.lock.
- [bevy_egui](https://github.com/vladbat00/bevy_egui): 0.42.0, native immediate-mode controls.

## Behavior and architecture references

- [not-elm/desktop-homunculus](https://github.com/not-elm/desktop-homunculus): Bevy desktop mascot, VRMA animation and desktop interactions. Rust MIT OR Apache-2.0; creative assets/docs CC-BY-4.0. Linux is listed as planned in its inspected README.
- [not-elm/bevy_vrm1](https://github.com/not-elm/bevy_vrm1): VRM1 and VRMA animation reference; the desktop-homunculus revision inspected here pins 0.7.1, whose exact rest-space retargeting equations were checked. Its VRM1-only support is why it does not replace this app's dual-generation loader.
- [xsploit/WebWaifu4](https://github.com/xsploit/WebWaifu4): user's local existing avatar UI, BVH/FBX retargeting, sequencing and affect design. Local package declares UNLICENSED; no implementation source is copied into the MIT application. Three original persona texts were imported as user-provided character data and are not relicensed by the application source licence.
- Hikari/Jev: recovered local speech subset with provenance, expression/appraisal research, and prior memory notes. Details and evidence boundaries in CUSTOMIZATION.md.
- [MateEngine](https://github.com/shinyflvre/Mate-Engine): desktop transparency, topmost and input-passthrough behavior reference. Its inspected MateEngine Pro License v2.1 is restrictive; no code copied.

## Service documentation

- [Vercel AI Gateway OpenAI compatibility](https://vercel.com/docs/ai-gateway/openai-compat)
- [OpenRouter API](https://openrouter.ai/docs/api-reference/overview)
- [xAI API](https://docs.x.ai/docs/api-reference): OpenAI-compatible Grok chat preset; users supply their own API key and model selection.
- [Fish TTS endpoint](https://docs.fish.audio/api-reference/endpoint/openapi-v1/text-to-speech.md)
- [Fish models](https://docs.fish.audio/developer-guide/models-pricing/models-overview) and [pricing/rate limits](https://docs.fish.audio/developer-guide/models-pricing/pricing-and-rate-limits): hosted `s2.1-pro-free` is the selected free-tier default; account restrictions apply.
- [Drama 3](https://docs.fish.audio/features/text-to-speech/drama-3): hosted performance-direction support.
- [Fish streaming guidance](https://docs.fish.audio/developer-guide/best-practices/real-time-streaming): the implemented client supports progressive HTTP WAV, timestamp SSE and WebSocket text/audio streaming; hosted Fish defaults to live WebSocket delivery.
- [Voice Design API](https://docs.fish.audio/api-reference/endpoint/openapi-v1/voice-design): separately paid preview candidates, not persistent TTS voices.
- [Fish Speech local schema](https://github.com/fishaudio/fish-speech/blob/main/fish_speech/utils/schema.py)

Mocked protocol tests do not establish live service availability, account access or voice quality.

## Local execution and assets

Codex CLI, Claude Code, Pi and SSH are external programs supplied and authenticated by each user; they are not bundled. T3-AGENTS.md records the inspected CLI references. The Unix runner supports explicitly reviewed tasks and an opt-in bounded loop; SSH requires a remote POSIX shell, GNU timeout and a previously trusted host key. These adapters do not establish compatibility with every CLI version or remote setup.

The three files in `assets/backgrounds/`—`cozy-room.png`, `moonlit-garden.png` and `neon-rooftop.png`—are AI-generated illustrations created for this app. They are separate from the locally constructed procedural 3D environments in `src/environments.rs`. The separately documented character bundle contains 22 VRMs. Three unchanged Homunculus desktop motion assets are bundled under CC BY 4.0; see assets/animations/desktop/ATTRIBUTION.md. Generated scenery is not evidence of animation, lip-sync or VR support; see FEATURE-STATUS.md for implementation boundaries.

## Desktop sitting implementation inspected on 2026-10-18

Inspected desktop-homunculus commit `30d11a5ad72f97f405117086839acd0ea87e1c49`: drag release detects an 80-pixel strip above another window, stores its identity and relative offset, follows its frame, smooths the hips anchor, and plays a separate sitting VRMA. Its external-window enumeration is implemented for Windows/macOS; other platforms return no windows. The app now implements a portable drag/seat state machine, actual mesh picking, native compositor dragging, and the attributed upstream idle/grabbed/sitting VRMA assets. Its retargeter follows the pinned bevy_vrm1 0.7.1 rest normalization and hips-height scaling, excluding application placement; clip changes crossfade over 0.5 seconds. The custom radial menu follows Mate’s release-to-select UX; no Mate implementation code is copied. KDE Wayland external-window and per-output work-area snapshots are provided by a separate experimental opt-in KWin bridge. Offline fixtures pass; live compositor operation remains unverified.

Relevant upstream files: `engine/crates/homunculus_drag/src/lib.rs`, `homunculus_sitting/src/lib.rs`, `homunculus_screen/src/global_windows.rs` and its Windows/macOS backends.

## Streaming defaults and transparent-window initialization

Read-only comparison with the local WebWaifu4 source and saved settings confirmed Vercel HTTP text streaming plus the Fish live bridge, balanced latency, chunk length 160 and minimum chunk length 20. Native LLM requests already use `stream: true`. The native Fish default now uses the same transport/tuning, preserving the selected current model instead of copying WebWaifu4's obsolete `drama-3-preview` identifier. Version-1 libraries upgrade their global/per-character transports once.

Homunculus explicitly selects compositor alpha and disables window shadows. The native app follows current Bevy 0.19's transparent-window example for platform alpha selection (Linux premultiplied, macOS postmultiplied), and restores decorations/shadows when returning from Desktop. ECS checks verify settings and restoration; compositor appearance requires desktop verification.

## T800 brain reference inspected on 2026-10-18

Read-only review of [Cyberdyne-T800-Agent-Embodiment-Project](https://github.com/newbmechanix-ship-it/Cyberdyne-T800-Agent-Embodiment-Project) at commit `a4ad483d5713d00d34a8c37b36b9886e5f882a95`. No source, authored biography or assets were copied. The default graph uses `hierarchical-memory.js` for calendar/topic/follow-up recall with bounded nodes and prompt size; `life-memory.js` also implements cue-based weighted association traversal. Neither requires a vector database. `experience-state.js` keeps bounded outcome-driven tendencies separate from authored facts, rejects hypothetical events and deduplicates updates. Its brain visualization exposes actual retrieval traces. See [JSON brain guide](https://github.com/newbmechanix-ship-it/Cyberdyne-T800-Agent-Embodiment-Project/blob/a4ad483d5713d00d34a8c37b36b9886e5f882a95/docs/JSON-BRAIN.md).

Its body/appraisal state influences model prompts, while explicit action/gesture names drive procedural robot joints. This is different from Waifu Studio's existing reply-affect metadata and VRM expression ownership. Useful future additions are an optional authored association graph with an inspectable recall trace and a separate bounded relationship/outcome store. Existing evidence-backed facts remain authoritative. Prefer typed action-completion events to T800's English-result-text success classification. These mechanisms were researched, not integrated into this build.
