# Embodiment brain options: affect state machine vs fly connectome

Research date: 2026-10-17. Read-only: Flyhard source and reports fetched through the GitHub API at commit `328906f4a0e62c8f9fc18805cf6edae6989b82a5`; no clone of its ~190 MB repo, no connectome download, no provider calls, no edits to Waifu Studio source. Companion to `DECISION-OPTIONS.md` (decision adapters) and `CUSTOMIZATION.md` (character engine).

## Recommendation

Implementation status update (2026-10-18): native `behavior.rs` now supplies off-default Idle/Thinking/Speaking lifecycle expression control with smoothing, decay, manual-channel protection and stale-event rejection. `reply_affect.rs` accepts explicit emotion/VAD reply metadata with decaying per-character mood; local emotion playlists or optional Jev motion choices can use completed replies. Pointer/ambient gaze and BVH/VRMA/FBX playback exist separately. These are bounded authored controls, not a trained connectome brain. Optional Jev incoming-message appraisal now offers a mutually exclusive alternative to reply metadata, with discrete VAD categories, strict schema validation, bounded asynchronous workers and stale-result rejection. The unified controller trait, broader decision policy, slow drives and connectome sidecar below remain proposals.

Ship a **conventional bounded affect state machine** as the default embodiment controller, driven by future adapters based on the inspected typed Jev/OpenAI decision APIs at turn rate. Define one `EmbodimentController` trait so a connectome controller can be dropped in later as an out-of-process, opt-in experiment. Do not put PyTorch, CUDA or a 25 M-edge graph in the Bevy process.

Reasons, from the evidence below: the initially inspected connectome controller (Flyhard) is a Python/PyTorch GPU model, its measured forward pass is for a training benchmark on an RTX A6000, its outputs are trained for one task (wheel angle via foreleg joints), and it resets neural state every decision. Nothing about it produces gaze or expression; any such mapping would be an engineered decoder we write and must evaluate ourselves.

## What was verified (Flyhard)

Repository: <https://github.com/MarkUnthank/flyhard> (MIT, pushed 2026-09-15).

- `src/flyhard/connectome.py`: `SparseConnectome` is a leaky rate model over an immutable CSR adjacency. Per step: `state = (1-leak)*state + leak*tanh(W·state + drive)`, where each measured edge weight is `count/row_total × (0.05 + 0.90·sigmoid(gain))` and leak is per-neuron in the same bounded range. Unsigned transmission; the docstring says transmitter biology is not asserted.
- `src/flyhard/motor_policy.py`: `WheelPolicy` wraps the core with a **frozen random signed** sensory mapping (9 observation features → VNC sensory neurons) and a **frozen random** 7-row decoder from motor neurons to joint targets. Each 50 ms decision starts from a zero state and runs 4 graph steps. Only edge gains and leaks train.
- `reports/2026-09-09/graph-manifest.json`: MaleCNS v1.0, filter `status == 'Traced'` → 165,122 neurons, 25,563,197 edges. Source weights file is 1.05 GB (`connectome-weights-male-cns-v1.0-minconf-0.5.feather`, CC-BY-4.0). This is why the earlier "165,122 / 25,563,197" screenshot numbers are not unique identifiers.
- Claimed scope (README, `e03-wheel-pilot/metrics.json`): requested-angle steering, 100/100 held-out targets after 600 updates, one seed, status `preliminary_pass`. Visual driving and multi-seed replication are explicitly untested.

### Measured performance (from Flyhard's own reports, not re-run here)

| Measurement | Value | Source |
|---|---|---|
| Full-graph 4-step forward, batch 4, median | 11.9 ms | `e01-core-v2/metrics.json`, RTX A6000, torch 2.8 cu128, autograd enabled |
| Backward (training only) | 250 ms | same |
| Peak GPU allocation, training | ~3.0 GB | e01 and e03 |
| Graph setup/load | 5.2 s | e01 |
| Connected CARLA capture | 24 sim-s in 82.9 wall-s, incl. loading and recording | README |
| FlyGym body physics | ~11,064 steps/s | `e00-a6000/metrics.json` |

**Unknown:** CPU-only inference latency, consumer-GPU latency, latency without autograd, Rust/`no-torch` port performance, RAM on this machine, and any behavior beyond wheel steering. Back-of-envelope (derived, not measured): CSR with int64 columns plus f32 base/gain is ~0.5 GB resident before PyTorch overhead.

Related-project coverage was expanded in the dated addendum below; Chreatures remains a lead rather than an inspected implementation. Flyhard's README lists further references not used by its controller: [FlyGym/NeuroMechFly](https://github.com/NeLy-EPFL/flygym), [FlyVis](https://github.com/TuragaLab/flyvis), [FlyBody](https://github.com/TuragaLab/flybody), [FlyGM preprint](https://arxiv.org/abs/2602.17997). Dataset: <https://male-cns.janelia.org/download/>.

## Comparison

| | Bounded affect state machine + typed decisions | Connectome graph controller |
|---|---|---|
| Runtime | Proposed pure Rust, in-process; microseconds per tick is an unmeasured target | Python/PyTorch sidecar; 11.9 ms/decision on A6000; CPU unknown |
| Memory | KB per character is an unmeasured design estimate | ~0.5 GB graph (est.), ~3 GB GPU in training |
| Output meaning | Designed: named states, gaze targets, clip IDs | None intrinsic; requires our decoder from motor neurons → avatar channels |
| Determinism | Exact given seed and inputs | Deterministic forward given weights; GPU sparse ops may differ in last bits across devices |
| Temporal state | Explicit decay/hysteresis we tune | Flyhard resets each decision, so no carried state unless we change that |
| Tuning | Sliders per character | Train decoder/gains against labelled avatar behavior we don't have yet |
| Failure | Clamp and fall back to idle | Sidecar crash/timeout → must fall back to the state machine anyway |
| Value | Coherent, inspectable behavior | Novelty and organic micro-variation; unproven for avatars |

## Division of labor

- **Language writer (LLM):** what to say. Returns text plus optional speech intent.
- **Decision adapters (Jev / OpenAI Decisions, per `DECISION-OPTIONS.md`):** discrete, slow choices at turn rate: appraisal (V/A/D nudge), gesture ID from an allowed list, attention target category. Never in the frame loop.
- **Audio lip-sync:** viseme/mouth weights from the TTS audio envelope or phonemes, every frame. No brain or LLM involvement; the controller may only scale an overall `mouth_gain` within bounds.
- **Embodiment controller (this doc):** continuous, fast, local channels:
  - gaze target and saccade timing (user/camera, cursor, screen region, idle wander)
  - blink rate, head micro-motion, breathing amplitude
  - expression **intensity** for the face preset chosen by affect (not which emotion: that comes from appraisal)
  - motion selection among allowed idle/fidget/gesture clips, with cooldowns
  - posture lean (approach/withdraw) from arousal and dominance

A connectome controller could only plausibly own the micro-variation channels (gaze jitter, fidget timing, intensity modulation). Semantic choices stay with appraisal.

## Proposed contract (Rust)

Lives in a new `src/embodiment.rs`; no new dependencies for the default controller.

```rust
/// Inputs are already-validated, bounded values; controllers never see raw text or audio.
pub enum EmbodimentEvent {
    Tick { dt: f32 },
    Affect { valence: f32, arousal: f32, dominance: f32 },     // [-1,1], [0,1], [-1,1]
    Appraised { epoch: u64, gesture: Option<GestureId> },      // from decision adapter
    SpeechStarted { epoch: u64 }, SpeechEnded { epoch: u64 },
    UserActivity { kind: ActivityKind },                       // typing, pointer near, idle
    AttentionHint { target: GazeTarget, ttl: f32 },
    Reset,
}

pub enum GazeTarget { User, Cursor, ScreenPoint(Vec2), Wander, Down }

/// Every field is clamped by the host after the controller returns.
pub struct EmbodimentAction {
    pub gaze: GazeTarget,
    pub saccade_now: bool,
    pub blink_now: bool,
    pub expression_intensity: f32,   // 0..=1, scales the affect-selected preset
    pub mouth_gain: f32,             // 0.5..=1.5, multiplies audio-driven visemes
    pub posture_lean: f32,           // -1..=1
    pub play_clip: Option<ClipId>,   // must be in the character's allowed set
}

pub trait EmbodimentController: Send + Sync {
    fn id(&self) -> &'static str;
    fn handle(&mut self, event: &EmbodimentEvent);
    /// Must not block; return the latest action available now.
    fn action(&mut self) -> EmbodimentAction;
    fn reset(&mut self, seed: u64);
}
```

Host rules (enforced outside any controller):

- Clamp every numeric field; drop `play_clip` not in the allowed set or still on cooldown; drop `Appraised` whose epoch is older than the current speech/turn epoch.
- Smooth outputs (critically damped spring per channel) so a controller swap or fallback never snaps the face.
- Per-entity controller instance keyed by avatar/entity ID.

## Per-character behavior budget

Store as versioned preset data alongside the character:

```json
{ "controller": "affect_fsm", "seed": 1,
  "gaze": { "saccade_hz": [0.2, 1.5], "user_focus": 0.6 },
  "blink_hz": [0.1, 0.5],
  "expression_intensity_max": 0.85,
  "posture_lean_max": 0.4,
  "clip_cooldown_s": 8, "max_clips_per_min": 4,
  "allowed_clips": ["idle_a", "nod", "wave", "stretch"],
  "experimental": { "enabled": false, "tick_hz": 20, "deadline_ms": 30, "max_misses": 3 } }
```

Defaults sit inside these bounds; UI exposes them under Embodiment → Advanced.

## Deterministic fallback

`AffectFsm` is always constructed. States: `Idle`, `Attentive`, `Listening`, `Speaking`, `Reacting(gesture)`, `Withdrawn`. Transitions from events plus V/A/D thresholds with hysteresis; timers from a seeded `SmallRng`-style PRNG (or a hand-rolled xorshift to avoid a dependency). Same seed + same event log ⇒ same action log, so it is testable with a golden-trace unit test.

If an experimental controller is selected, the host runs it **and** `AffectFsm`; it uses the experimental action only when it arrived within `deadline_ms`, and switches back to the FSM after `max_misses` consecutive misses, sidecar exit, or any non-finite value. The UI shows which controller is live.

## Dependency isolation for a connectome experiment

- Separate process (e.g. Python with Flyhard's `SparseConnectome`, or a later Rust sparse port), launched only when the user enables it. Waifu Studio gains no Python/torch/CUDA dependency.
- Local IPC only (Unix socket or stdin/stdout), newline-delimited JSON or fixed-size f32 frames: host sends an encoded feature vector at `tick_hz`; sidecar returns raw decoder outputs. All decoding to `EmbodimentAction` happens host-side through the same clamps.
- Graph data acquired by the user via Flyhard's hash-checked `scripts/acquire_connectome.py`, stored outside the app directory; never bundled or auto-downloaded.
- Record sidecar version, graph SHA-256 (Flyhard's is `eff4093b…771d`), decoder version and seed in the turn receipt.

## Before calling the connectome version better

Measure on this machine: p50/p95 decision latency (CPU and GPU, `torch.no_grad`), RSS/VRAM, and frame-time impact. Then run blind A/B clips of the FSM vs the connectome controller on the same event logs and rate coherence, responsiveness and repetitiveness. Until then it is a novelty option, not an upgrade. It does not add learning, memory, personality or awareness; persistent state only exists if we stop Flyhard's per-decision reset, and that alone is just recurrence.

## Evidence boundary

Confirmed from source: Flyhard model equations, interfaces, graph filter, and the numbers in its committed report JSON. Not confirmed: re-running anything, CPU/consumer-GPU performance on this host, any avatar mapping, or behavior of a stateful Flyhard variant. The additional source inspections below do not establish local runtime acceptance.


## Expanded primary-source check — 2026-10-17

No datasets downloaded, simulators installed, training performed or games run. Repository descriptions and committed results are upstream evidence; performance and behavior have not been reproduced here.

### Wiring datasets are not executable minds

| Dataset | Verified scope | Practical distinction |
|---|---|---|
| [FlyWire](https://flywire.ai/) | Official site describes the adult **female brain**, with 139,255 proofread neurons in its October 2024 flagship publication. [Release-783 annotations](https://github.com/flyconnectome/flywire_annotations/tree/a83b2776d60d5764cef36b927f5f9679c16c47a2) distinguish publication-matched versions from subsequent cross-dataset updates. | Brain reconstruction; do not present it as the complete male brain-plus-nerve-cord graph or treat current annotations as identical to the 2024 release. |
| [MaleCNS](https://male-cns.janelia.org/) | Official news dates v1.0 to **2026-06-08** and publication to **2026-09-03**. [Janelia](https://www.janelia.org/project-team/flyem/male-cns-connectome) describes an integrated male brain and ventral nerve cord with intact neck connective; data CC-BY. | Different animal, sex and anatomical coverage. Filtered neuron/edge counts vary by retained status, confidence, threshold and boundary handling; name the exact graph rather than treating counts as universal. |

Both provide measured structural connectivity and annotations. A runtime still chooses neuron dynamics, synaptic signs/strength interpretation, delays, sensory encoding, initial state, learning rules and motor decoding. Those choices determine simulated behavior; neither dataset inherently provides human emotions, language, an autobiographical memory, or a ready-made avatar policy.

### Runnable software: what each actually supplies

- **[FlyVis](https://github.com/TuragaLab/flyvis/tree/0a69fa2fc8e1bfe1d837d722c252ab5f5d6fcbfa)** is the official PyTorch implementation associated with [Lappalainen et al., Nature 2024](https://www.nature.com/articles/s41586-024-07939-3). It supplies connectome-constrained visual-system models, pretrained-model tutorials, optic-flow training and neural-response analyses. Inspected `flyvis/network/network.py` exposes explicit state/integration time steps and PyTorch forward simulation. This is visual computation research, not a whole-fly personality or humanoid animation system. Useful later for experimentally deriving motion-sensitive attention signals; unnecessary for ordinary cursor gaze.
- **[Flybody](https://github.com/TuragaLab/flybody/tree/d015e9bfe441bd90ae431bac24c55cb74bdbce26)** supplies a MuJoCo fruit-fly body and walking/flight/vision-guided task environments. Its README separates minimal body installation from optional TensorFlow/Acme policy execution and Ray training. Anatomical physics and learned locomotion policies do not establish a whole-connectome controller. It is a body/testbed, not a drop-in brain.
- **[FlyGym / NeuroMechFly](https://github.com/NeLy-EPFL/flygym/tree/38c8ec61034cd59bc5ba0de20688d4a3c0000d60)** supplies embodied sensorimotor simulation, vision/olfaction interfaces, contact/joint observations and hierarchical controller interfaces. The current README dates its incompatible 2.x rewrite to March 2026 and cautions that not all 1.x features are available. Reported speedups are upstream claims, not measurements here. Useful for studying fly locomotion, with no direct benefit to a humanoid VRM unless we build a separate mapping.
- **[Fly Brain Minecraft](https://github.com/blendi-remade/fly-brain-minecraft/tree/6cfa30175003ef25da68a237d5eda958f8047b82)** is an actual Java/Fabric 1.21.1 mod with `LifNetwork.java`, `MotorDecoder.java` and corresponding test sources. Source inspection confirms active-set leaky integrate-and-fire dynamics and an explicit decoder with thresholds/dwell times/refractory handling. Its README describes a thresholded MaleCNS graph, sensory encoders, named neuron populations and engineered motor output; claims include 176,422 neurons and 6.29M connections at five-or-more-synapse filtering. It is a more directly portable reference for an event-driven native simulation than PyTorch Flyhard, but behavior/calibration and “real time on eight cores” remain upstream claims until tested. Java code can inform a Rust design; it does not become Rust-ready by virtue of using the same data. Code MIT, data CC-BY-4.0 per repository.
- **[Doomfly](https://github.com/nftechie/doomfly/tree/71ecf53d78eaffaf1a57ed7b0ccf5d458abc9f33)** explicitly reports that its current v6 candidate **failed visual, conditioning and survival gates**. Its documented loop uses frame-driven sensory proxies, MaleCNS dynamics, fixed engineered neuron-to-button assignments and experimental plasticity on a small selected connection set. It requires a Python/C++ simulator with several GB RAM and downloaded graph inputs; the web viewer does not host the brain. Its negative results are a useful counterexample to equating neural activity or weight changes with learned useful behavior.

### Refined choice for this Rust companion

Keep the conventional embodiment controller as the shipping default. If the user enables a fly experiment, the clean first research comparison is **a small bounded sparse/LIF sidecar inspired by the Minecraft implementation versus the proposed deterministic state machine**, using identical scripted input events and explicit output decoders. Record whether it uses a real dataset subset or merely a synthetic recurrent network; a synthetic approximation must not be labelled the full fly connectome.

Start with gaze/fidget timing and expression-intensity modulation. Leave semantic dialogue, durable user memory, gesture selection, permissions and audio lip-sync in their established systems. Do not route an unmeasured full CNS graph through every render frame or grant neural activity authority to send messages or invoke tools. Use the same opt-in 20 Hz/30 ms experimental budget and fallback described above, then measure actual suitability. These budgets are acceptance targets, not demonstrated full-graph performance.

FlyVis is the strongest primary research reference here for a validated **visual-modeling** objective; Flybody/FlyGym are stronger **embodiment testbeds**; Minecraft is a practical **native-style simulation reference**; Doomfly is particularly useful for its explicit **negative learning evidence**. None currently supplies a demonstrated human companion brain. No current Rust avatar integration was verified.
