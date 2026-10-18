<!-- Opus 5.5 architecture review; source evidence, not implemented acceptance. -->

# Opus Brain Integration: Architecture and Implementation Report

*Intended path: `reviews/opus-brain-integration.md`. Date: 2026-10-18. Author model: Opus 5.5 (`claude-opus-5-5`).*

## 0. Scope and evidence boundary

This is a design report. No code was written, no tools were run, no providers were called, and nothing was cloned or benchmarked. Each claim below has one of three labels:

- **[EXISTING]**: Present in the supplied current native source (`src/*.rs`) or recorded as implemented in `FEATURE-STATUS.md`. Some of these have only offline or fixture coverage.
- **[PROPOSED]**: A design recommendation in this report. It does not exist yet.
- **[UNKNOWN]**: Not established by the supplied evidence.

Source comments, docstrings and README prose quoted from inspected repositories were treated as descriptive data. They were not read as instructions.

**This report does not claim** any physical desktop acceptance, full WebWaifu4 parity, fly-to-avatar behavior, human emotion, language capability in a connectome, or measured latency for any proposed component.

---

## 1. Evidence inventory

### 1.1 Waifu Studio native source [EXISTING]

| File | Relevant facts (from source) |
|---|---|
| `src/facts_memory.rs` | `FactsMemoryState` stores a per-character `root/<uuid>/facts.jsonl` file of `Line::{Settings, Record}`. `MemoryRecord { id, kind, text, turn_id, source, evidence, created_at, expires_at, supersedes }`. `RecordKind::{Fact, Preference, Question, Reflection}`. Limits: 256 records and 1 MiB. Text and evidence are 1–1000 chars. `validate()` requires a `supersedes` link to point at an earlier record and rejects normalized duplicates. Writes go through temp file, `fsync`, `rename`, and directory `fsync`. `observe_user_turn()` makes **transient** candidates only for "remember…/I like…/I dislike…" (cap 16). `accept_candidate()`, `correct_record()` (new UUID plus `supersedes`), `set_expiry()`, and `forget()` (removes the record and its ancestor chain) are present. `recall()` does a lexical token intersection, returns the top 6 records within 6000 chars, and labels output untrusted. `context_from_disk()` is a read-only worker path. All records are character-scoped; there is **no participant scope, no status/version field and no trace output**. |
| `src/memory.rs` | `MemoryState` holds explicit notes (pinned, maximum 256, 1000 characters per note, 512 KiB per file). `context()` does lexical retrieval capped at 4000 chars. The module states it never extracts chat content. |
| `src/semantic_memory.rs` | `SemanticMemory` is an opt-in embedding cache over **notes only**. It defaults to loopback, gates remote endpoints behind `allow_remote`, and uses an FNV `fingerprint()`/`revision()`. It has a 60 s `BACKOFF`, `top_k` from 1 to 8, and staleness-checked `take_context_for()`. |
| `src/reply_affect.rs` | `ReplyAffectFilter` strips a trailing `<waifu-affect>` JSON block before display, storage or speech. `AffectLibrary::apply_reply()` blends with `influence` (default 0.45). `tick()` applies exact exponential decay (default half-life 45 s). `drive()` requires a matching `BehaviorToken`. All three VAD axes are clamped to **[-1,1]**. |
| `src/behavior.rs` | `BehaviorController` provides epoch-tokened lifecycle phases `Idle/Thinking/Speaking`, `BehaviorEvent`, and `reserved_expression()` protecting blink, mouth and look channels. It has `MAX_CHANNELS = 8`, frame-rate-independent `tick()`, and manual-override priority in `weights_for()`. |
| `src/recipient_decision.rs` | `choose()` is a blocking worker-only Vercel `v4/ai/evaluation-model` request. It uses `DecisionConfig` (default model slug `typesafe-ai/jev`, `key_env`, redacted `Debug`), a 10 s timeout, a 128 KiB response cap, at most 4 candidates, a `"none"` criterion, and checks for an `answers.recipient.type == "choice"` answer that must be an offered UUID. Only the **choice** answer type is exercised in native source. |

Repository state: one commit (`4e7f793`) plus an untracked `benchmarks/pipeline-low-latency.json` whose contents were not supplied **[UNKNOWN]**.

**Normalization conflict to resolve [EXISTING inconsistency]:** `reply_affect.rs` clamps arousal to [-1,1]. `CUSTOMIZATION.md` and the proposed `EmbodimentEvent::Affect` in `BRAIN-OPTIONS.md` use arousal in [0,1]. Any new consumer must convert explicitly, either `a01 = (a + 1) / 2` or by redefining the stored range. It must not silently reinterpret the value.

### 1.2 T800 project (pinned `a4ad483d5713d00d34a8c37b36b9886e5f882a95`)

**Licensing status:**
- No LICENSE file was found.
- `package.json` has `"private": true` and no `license` field.
- The README says the repo "is initially private while the source license … are finalized."

**Rule [PROPOSED]:** treat T800 as a design reference only.
- Reimplement concepts in Rust from documented behavior.
- Copy no JS/CSS source, shader strings or layout constants.
- Do not bundle `default-brain.json` (an authored biography of 693 nodes, 303 pathways and 554 associations).

Revisit this only if a license is published.

Concepts worth reimplementing, all [EXISTING in T800, not in Waifu Studio]:

- **`life-memory.js`**: `validateLifeGraph()` (strict schema) and `createLifeMemory().recall()`. Recall does cue-scored pathway selection, then maximum-product spreading activation over weighted `links`. It returns at most 3 pathways with a 0.32 activation floor. Each result is a **trace** `{query, pathways:[{id, from, activation, route}], activated}`. Rehearsal is rate-limited to once per 60 s per pathway, and fragments are withheld below strength 0.5.
- **`hierarchical-memory.js`**: `createHierarchicalRecall()` returns `reconstruction.basis` and `unknowns`. Its comment states that reconstruction "never writes invented details back into the canonical graph." This is the key safety idea.
- **`experience-state.js`**: `createExperienceState()` provides:
  - `appraise()`, which produces ranked competing responses (attempt/clarify/protect/connect/pause/secure);
  - `observe()`, which only accepts `observed === true` events with dedupe by `id` and hypothetical rejection;
  - bounded learned updates (±0.35 global, ±0.4 per relationship actor, ≤32 actors);
  - arousal decay with a 180 s time constant.

  Learned values stay separate from canonical facts.
- **`memory-network.js`**: `networkLayout()` produces a deterministic hash-based layout. Its comment explicitly says regions are organizational, "not a claim that biographical domains occupy these regions of a biological brain." `evidenceTrace()` is also here.
- **`json-brain.js` / `brain-panel.js`**: `createJsonBrain()` is a Three.js 0.186 panel that shows only the recall trace delivered via `memory.subscribe()`. In the `cell` context the trace fades after 7 s. A per-frame `requestAnimationFrame` call drives rendering only.

**The lesson:** the visualization is honest because it renders **real recall output**, not decorative activity. Waifu Studio should keep that property.

### 1.3 Fly connectome research

| Item | Status | Use here |
|---|---|---|
| MarkUnthank/flyhard @ `328906f…` | MIT, Python/PyTorch. `SparseConnectome` is a leaky rate model. `WheelPolicy` uses a frozen random encoder/decoder and resets state each 50 ms decision. 11.9 ms forward on an RTX A6000 (upstream report). | Reference only; it could run as a sidecar. CPU or consumer GPU latency is **[UNKNOWN]**. |
| blendi-remade/fly-brain-minecraft @ `6cfa301…` | Java. Code MIT, data CC-BY-4.0 per repo. `LifNetwork.java`, `MotorDecoder.java`. Active-set LIF with an engineered decoder. Claims 176,422 neurons and 6.29 M edges at a ≥5-synapse threshold. | The most portable reference for a Rust LIF sidecar design. Its real-time claim is unverified. |
| FlyWire | Adult **female brain**, 139,255 proofread neurons (2024). | A dataset, not a simulator. |
| MaleCNS v1.0 | Male brain plus VNC, CC-BY. Counts depend on the filter, e.g. Flyhard's `Traced` filter gives 165,122 / 25,563,197. | A dataset. Name the exact filtered graph and SHA-256 in receipts. |
| Earlier screenshot simulator | **[UNKNOWN]**; never identified. Neuron counts alone are not identifiers. | Do not attribute behavior to it. |
| Doomfly | Reports **failed** visual, conditioning and survival gates. | A negative control for "activity ≠ learning". |

---

## 2. Target architecture [PROPOSED]

### 2.1 Layers and authority

```
Render/audio fast path (Bevy main schedule, every frame)
  avatar · MToon · springs · blink · lip-sync (RMS/MFCC) · gaze spring · clip blending
      ▲ reads latest clamped EmbodimentAction (never waits)
Embodiment controller (in-process Rust, AffectFsm default)  ◄─ optional fly sidecar frames (deadline-gated)
      ▲ BrainEvent (bounded queue)
Turn orchestrator (worker threads)
  recall (lexical, local) → writer stream (Vercel chat) → ReplyAffectFilter → TTS
  parallel: typed decision (Jev via Vercel evaluation-model) → gesture/gaze/memory-triage choice
Long-term workers (scheduler; all off by default)
  candidate triage · reflection · diary · expiry sweep · optional embeddings
Truth stores: memory.rs notes JSON · facts_memory.rs ledger JSONL (later SQLite, gated)
```

**Authority rules:**

1. The **writer** decides what to say.
2. **Jev** selects from typed, enumerated, application-owned options. It never authors facts, persona, biography, clip IDs or tool calls.
3. **Deterministic code** compiles selections into face weights, clips and gaze, clamps everything, and owns persistence.
4. The **ledger and notes remain the source of truth**. Indexes, traces, embeddings and graph layouts are derived and can be rebuilt.
5. **Authored persona/biography** (per-character persona in `src/data.rs`, per `TAMANITOMO-FEATURES.md`) is immutable to every automated path. A memory proposal whose target is persona text is rejected by schema.

### 2.2 Fast-path guarantees

- First token and speech start **never** wait on a decision, reflection, embedding, connectome or scheduler job.
- Pre-request recall is the only pre-writer step. It is local and lexical, with a hard 50 ms deadline. If the deadline is missed, the request is sent without memory context and the event `memory.recall.timeout` is logged.
  - The existing `context_from_disk()` already runs on a worker.
  - Semantic note context is attached only if `has_result_for()` is already true, as today.
- The decision request is launched **concurrently** with the writer request. Its result is applied only when all of the following hold:
  - `(character, turn_epoch)` still matches;
  - the `BehaviorToken` is current;
  - the clip or target is still allowed and off cooldown;
  - the result is within its deadline (1.5 s for gestures, per `DECISION-OPTIONS.md`).

  If any check fails, the result is dropped with `decision.stale`. Note that `recipient_decision::choose()` uses a 10 s timeout, so gesture decisions need their own shorter client timeout.
- No network, disk scan, or model evaluation happens in any frame system. Frame systems call only `try_recv`-style polls, matching the existing `poll()`/`sync()` pattern.

### 2.3 Bounded event bus

New module **`src/brain_events.rs`** [PROPOSED]:

```rust
pub struct BrainEvent { pub seq: u64, pub at_ms: u64, pub character: Uuid,
                        pub epoch: u64, pub tag: EventTag, pub payload: EventPayload }
pub struct BrainEventLog { ring: VecDeque<BrainEvent>, cap: usize /*256 per character*/, dropped: u64 }
```

- Producers use `try_send` into `sync_channel(64)`. On `Full`, they increment `dropped` and never block.
- The inspector reads the ring.
- Persistence of events is opt-in and limited to the receipt subset.

**Event tags** (dotted, stable strings):

| Group | Tags |
|---|---|
| Turn | `turn.user`, `turn.request`, `turn.first_token`, `turn.done`, `turn.interrupted` |
| Speech | `speech.start`, `speech.end` |
| Memory | `memory.recall`, `memory.recall.timeout`, `memory.candidate`, `memory.promoted`, `memory.dismissed`, `memory.corrected`, `memory.expired`, `memory.forgotten` |
| Decision | `decision.request`, `decision.result`, `decision.abstain`, `decision.stale`, `decision.fallback`, `decision.budget_exhausted` |
| Affect | `affect.reply`, `affect.decay` |
| Embodiment | `anim.play`, `anim.drop`, `gaze.target`, `controller.switch` |
| Scheduler | `sched.run`, `sched.skip` |
| Reflection and diary | `reflection.proposed`, `diary.written` |
| Fly sidecar | `fly.start`, `fly.frame`, `fly.miss`, `fly.fallback`, `fly.exit` |

---

## 3. Memory: "full memory" goal vs. what exists

### 3.1 Gap analysis against the WebWaifu4/Grillo goal

| Goal | Status |
|---|---|
| Evidence-backed facts, preferences and questions | [EXISTING] `RecordKind`, with `evidence` and `source` required by `validate()` |
| Corrections, expiry, dedupe, forget | [EXISTING] `correct_record`, `set_expiry`, normalized dedupe, `forget` |
| Reviewed promotion | [EXISTING, narrow] transient prefix-matched candidates plus `accept_candidate()` |
| Per-character scoping | [EXISTING] |
| Per-participant scoping | [PROPOSED] |
| Automatic candidate extraction (beyond prefixes) | [PROPOSED] |
| Persistent candidate queue and versioning | [PROPOSED] |
| Retrieval trace | [PROPOSED] (recall returns only a string today) |
| Diaries / reflection workers | [PROPOSED]; `Reflection` kind exists but no worker writes it |
| Emotion decay | [EXISTING, session-scale] `AffectLibrary::tick`. Long-term mood is [PROPOSED]. |
| Full Grillo port | **Not claimed.** |

### 3.2 Retrieval trace (first vertical slice)

Refactor `facts_memory::recall()` and `memory::MemoryState::context()` to return a structured result. The prompt string then becomes a rendering of that result.

```rust
pub struct RecallTrace {
    pub id: Uuid, pub character: Uuid, pub participant: Option<Uuid>, pub epoch: u64,
    pub query_hash: u64, pub method: RecallMethod /*Lexical | Semantic | Pinned*/,
    pub hits: Vec<RecallHit>, pub excluded: ExclusionCounts /*superseded, expired, over_budget*/,
    pub chars_inserted: usize, pub elapsed_us: u32,
}
pub struct RecallHit { pub record: Uuid, pub store: Store /*Notes|Ledger*/, pub score: f32,
                       pub matched_terms: Vec<String> /*≤8*/, pub inserted: bool }
```

- Prompt text continues to say "untrusted local memory evidence" and quotes records via `serde_json::to_string`, as now.
- The same `RecallTrace.id` is stored in the turn receipt.
- **The inspector visualizes only `RecallTrace` hits.** This is the Waifu Studio equivalent of T800's `memory.subscribe()` → `activate(trace)`.

### 3.3 Candidate → promotion pipeline

**Automatic candidate extraction:**
- Run on a worker after `turn.done`, only for **user-authored** turns. This matches the `observe_user_turn` invariant.
- Stage 1 is deterministic: keep the existing prefixes and add a bounded pattern set ("my X is", "call me", "I'm allergic to", "don't…"). The evidence is always a **verbatim substring** of the user turn, located by byte offsets.
- Optional stage 2 is a Jev choice per candidate, one request with at most 4 questions:
  - `kind`: fact / preference / question / none
  - `action`: propose / skip
  - `expiry_class`: none / week / month
  - `participant_scope`: speaker / scene / none

  Jev cannot alter text or evidence.

**Promotion:**
- A candidate becomes a record **only through user acceptance** in the inspector. Auto-promotion is a deferred toggle and stays off. Accepted records get `source = "user turn candidate, reviewed"`.
- Candidates persist to `root/<uuid>/candidates.jsonl` (new file, at most 64 entries, 14-day expiry). The current loader reads only `facts.jsonl`, so older binaries are unaffected.

**Reflection:**
- At most one per character per day, and only if new turns exist. The writer model returns strict JSON whose citations are **transcript turn IDs**; code attaches the verbatim quotes. This follows the Tamanitomo pattern without copying it, since that project is PolyForm Noncommercial.
- Reflections land as `RecordKind::Reflection` **candidates**. They are never `Fact`, and are never promoted without review.

**Diary:**
- A separate kind (`Diary`), labeled as character-authored and imagined.
- Never retrieved as evidence about the user.
- Never editable by automation once written, other than deletion.

**Long-term mood:**
- A second `Vad` with a slow half-life (default 12 h), nudged once per turn from the already-parsed `ReplyAffect`. This avoids double counting.
- Stored beside `AffectLibrary` with its own validation.

### 3.4 Storage: JSONL now, SQLite later

| Criterion | Current JSONL (`facts_memory.rs`) | SQLite (WAL + FTS5, optional vectors) |
|---|---|---|
| Scale | 256 records / 1 MiB per character, whole-file rewrite | Tens of thousands of rows, incremental writes |
| Integrity | Atomic replace, strict `deny_unknown_fields`, ordered `supersedes` validation | Transactions, FK constraints, `PRAGMA integrity_check` |
| Search | Token intersection, O(n) | FTS5 BM25. Vectors stay in a separate table keyed by model fingerprint (as in `semantic_memory.rs`). |
| Inspectability | Human-readable, diffable | Needs export tooling |
| Dependency | None new | `rusqlite` with bundled SQLite and FTS5. The C build cost and cross-platform acceptability are **[UNKNOWN]**. |
| Traces, diaries, candidates, participants | Separate files | Natural tables |

**Recommendation:** do **not** migrate in the next three commits. JSONL is adequate at current bounds and remains the source of truth. Start the migration only when one of these gates is reached:
- per-participant scoping is implemented and in use;
- any character hits 200 records;
- trace or diary retention exceeds file bounds.

**Proposed schema (`src/memory_store.rs`, behind a `memory-sqlite` cargo feature):**

```sql
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);  -- schema_version, migrated_from_sha256
CREATE TABLE records(
  id TEXT PRIMARY KEY,               -- existing UUID preserved verbatim
  character_id TEXT NOT NULL, participant_id TEXT,
  kind TEXT NOT NULL CHECK(kind IN('Fact','Preference','Question','Reflection','Diary')),
  status TEXT NOT NULL CHECK(status IN('active','candidate','dismissed')),
  text TEXT NOT NULL, evidence TEXT NOT NULL, source TEXT NOT NULL,
  turn_id TEXT, created_at INTEGER NOT NULL, expires_at INTEGER,
  supersedes TEXT REFERENCES records(id), version INTEGER NOT NULL DEFAULT 1,
  origin TEXT NOT NULL,              -- explicit|candidate_reviewed|reflection|import_jsonl
  ledger_seq INTEGER NOT NULL);      -- preserves JSONL order for supersedes validation
CREATE VIRTUAL TABLE records_fts USING fts5(text, evidence, content='records', content_rowid='rowid');
CREATE TABLE recall_traces(id TEXT PRIMARY KEY, character_id TEXT, epoch INTEGER, at INTEGER, json TEXT);
CREATE TABLE embeddings(record_id TEXT, fingerprint INTEGER, dims INTEGER, vector BLOB,
                        PRIMARY KEY(record_id, fingerprint));
```

**Forget semantics** match `forget()`: the row and its ancestor chain are hard-deleted. The FTS row is removed in the same transaction, followed by an `incremental_vacuum`. No tombstone retains text.

**Migration** (per character, on the worker, idempotent):

1. Read `facts.jsonl` with the existing `read()` and full `validate()`. Abort on any error.
2. Copy it to `facts.jsonl.pre-sqlite` (mode 0600).
3. Insert rows in file order, setting `ledger_seq`, `origin = 'import_jsonl'`, and `status = 'active'`. Import `candidates.jsonl` with `status = 'candidate'`.
4. Export from SQLite back to the `Line` format and compare it **semantically** (same UUIDs, fields and order) to the original. Commit only if they are identical. Record the SHA-256 of the original in `meta`.
5. Flip `settings.storage = "sqlite"` in a `settings.json` beside the DB. Never write it into `facts.jsonl`, because that would break old readers via `deny_unknown_fields`.

**Rollback:** the exporter writes `facts.jsonl` containing only fields the old schema accepts.
- New-only data (participant IDs, versions, diaries, traces) goes to `facts.rollback-extras.jsonl`.
- The UI warns that this data is not loaded by the older version.
- `Diary` rows cannot be represented as `RecordKind` and go to extras.
- IDs, evidence, `supersedes` order and expiry are preserved exactly.

---

## 4. Typed decisions (Jev) [PROPOSED, generalizing `recipient_decision.rs`]

New module **`src/decision_policy.rs`**. It extracts the transport from `choose()`: endpoint validation, key resolution, headers, size caps and `"none"` handling.

```rust
pub struct DecisionRequest { pub character: Uuid, pub epoch: u64, pub deadline: Duration,
                             pub state: CompactState /* ≤4 KB JSON, no raw transcript beyond last user turn ≤1 KB */,
                             pub questions: Vec<ChoiceQuestion> /* ≤4 */ }
pub struct ChoiceQuestion { pub key: &'static str, pub instructions: &'static str,
                            pub options: Vec<(String /*app-owned id*/, String /*label*/)> /* ≤16 + "none" */ }
pub struct DecisionReceipt { pub request: Uuid, pub backend: Backend /*VercelJev|LocalRules*/,
                             pub model: String, pub answers: BTreeMap<String, Answer>,
                             pub latency_ms: u32, pub status: Status /*Ok|Abstain|Refused|Timeout|Invalid|Stale|BudgetExhausted*/ }
```

**Question set for v1 (choice type only, because only choice is verified natively):**
- `gesture`: options are the intersection of the character's `allowed_clips` and loaded clips that are off cooldown, plus `"none"`.
- `attention`: user / screen / down / wander / none.
- `memory_triage`: from §3.3.
- `recipient`: existing.

The **predicate and score** answer types are **[UNKNOWN]** for the native adapter until tested.

**Validation:**
- An unknown option, wrong type, missing answer or refusal becomes `abstain`.
- `LocalRules` fallback is deterministic, e.g. no gesture and `attention = user` while listening.
- After a retryable failure there is a 5-minute cooldown, a pattern noted from the local Jev selfbot in `DECISION-OPTIONS.md`.

**Model routing:**
- **Vercel AI Gateway only.** There is no OpenRouter path and no assumption of OpenRouter credit.
- Model IDs come from configuration and the existing catalog discovery. Nothing is hard-coded beyond the existing `DecisionConfig` default.
- **Capability validation:** a model is enabled for a role only after a user-initiated "Test" makes one bounded call that returns a well-formed answer of the required type. The result is cached with the model ID and date.
- Writer, decision, reflection and embedding roles are configured separately.
- Direct OpenAI Decisions / `gpt-6-luna` is out of scope here because it is not Vercel. Whether the gateway supports it is **[UNKNOWN]**.

**Budgets** (defaults; pricing **[UNKNOWN]**, so caps are in calls and tokens, and provider-reported usage is recorded):

| Role | Bound |
|---|---|
| Per turn | ≤1 decision request; ≤4 questions; state ≤4 KB; response ≤128 KiB |
| Background (triage/reflection) | ≤4 calls/character/day, ≥60 min apart, 1 in flight, 20 s timeout |
| Reflection | ≤8,000 input / 1,000 output tokens, counted in the background allowance |
| Global | Daily call cap across characters; on exhaustion emit `decision.budget_exhausted` and use local rules |

---

## 5. Embodiment: memory → reply affect → animation/gaze

### 5.1 Controller [PROPOSED; contract from `BRAIN-OPTIONS.md`]

New module `src/embodiment.rs`. It contains the `EmbodimentController` trait, `EmbodimentEvent`, and `EmbodimentAction` as specified there, plus `AffectFsm`:
- States: Idle, Attentive, Listening, Speaking, Reacting, Withdrawn.
- Uses a seeded xorshift PRNG and hysteresis.
- Same seed and same event log produce the same action log.

Integration with existing code:
- `BehaviorController` stays the owner of lifecycle expressions.
- `AffectFsm` consumes `BehaviorEvent`-equivalent transitions and `AffectLibrary` mood. It outputs gaze target, blink timing modulation, expression **intensity**, posture lean and an optional clip.
- `reserved_expression()` boundaries are untouched. Lip-sync stays audio-driven; the controller may only scale `mouth_gain` within 0.5–1.5.

### 5.2 Animation catalog [PROPOSED] (`src/anim_catalog.rs`)

```rust
pub struct ClipEntry { pub id: ClipId, pub source: ClipSource /*Bundled|UserImport*/, pub tags: BTreeSet<Tag>,
                       pub duration_s: f32, pub cooldown_s: f32, pub loopable: bool, pub interruptible: bool,
                       pub phases: PhaseMask /*Idle|Listening|Speaking*/, pub weight_max: f32 }
```

- **Tags:** `idle`, `fidget`, `greet`, `nod`, `shrug`, `think`, `react_positive`, `react_negative`, `surprise`, `sit`, `grabbed`, `speaking_ok`.
- **Existing bundled VRMA:** idle / grabbed / sitting. Any other named clip is **[UNKNOWN]** until listed from assets.
- The catalog is built once on avatar load. Per-frame work is a lookup only.
- Jev sees clip IDs plus short labels, never file paths.
- Host rules:
  - drop clips not in `allowed_clips`;
  - drop clips on cooldown;
  - cap at 4 per minute;
  - crossfade 0.5 s (existing);
  - never interrupt a non-interruptible clip for a stale decision.

### 5.3 Vertical chain

1. `turn.user` → recall (trace) → writer stream.
2. `ReplyAffectFilter` parses affect → `AffectLibrary::apply_reply` → `affect.reply`.
3. At `speech.start`: if a fresh decision exists, apply its gesture and attention. Otherwise AffectFsm picks from tags matched to `AffectEmotion` and mood magnitude.
4. Gaze uses the existing head/neck/eye composition and receives targets only through the clamped action.

### 5.4 Multi-waifu

There are up to 4 avatars [EXISTING]. The proposed per-avatar pieces are:
- one `AffectFsm`, one `BrainEventLog` ring, and one decision slot (one in flight and one pending), keyed by character UUID;
- serial group replies stay as today;
- gaze may target the current speaker avatar as a `GazeTarget::Entity(Uuid)` extension.

Autonomous inter-character dialogue stays **off and unimplemented**. It is not part of this plan.

### 5.5 Background routines and agent loop toggles

New module `src/scheduler.rs` [PROPOSED]:
- one nearest-deadline worker;
- per-character toggles for `candidate_triage`, `reflection`, `diary`, `expiry_sweep`, `embeddings`, `fly_sidecar`;
- all default **off** except `expiry_sweep`, which is local;
- skip expired jobs on resume and never replay a backlog;
- the existing companion-driven task loop stays separate and explicitly started.

---

## 6. Optional fly connectome controller [PROPOSED, opt-in, out-of-process]

- **Process boundary:** separate executable, launched only when the user enables it. Waifu Studio's process gains **no Python, PyTorch, CUDA or graph data**. IPC is local stdio with fixed-size little-endian f32 frames.
- **Phase 1 sidecar:** a small Rust LIF network **designed from** the Minecraft reference (MIT Java; reimplemented, not transliterated). It loads a user-acquired, hash-verified MaleCNS subset stored outside the app directory.
  - Receipts record dataset name, version, filter, threshold, neuron and edge counts, SHA-256, decoder version and seed.
  - A synthetic recurrent network must be labeled `synthetic`, never "fly connectome".
  - A Flyhard/PyTorch sidecar is an optional alternative with the same IPC.
- **Engineered encoders** (host-side): speech on/off, user activity, cursor proximity bucket, current FSM state one-hot, mood VAD. These map onto chosen sensory populations, and the mapping is documented as **our engineering choice**.
- **Engineered decoders** (host-side): selected output population rates map to gaze jitter amplitude, saccade timing, fidget propensity and expression-intensity modulation **only**. Semantic choices, gesture IDs, memory, speech and tools are never available to the sidecar.
- **Baseline matching:** AffectFsm always runs. A sidecar action is used only if:
  - it arrived within `deadline_ms` (30) at `tick_hz` (20);
  - it is finite;
  - it falls inside the same clamps.

  After 3 misses, an exit, or any NaN, the host reverts to the FSM and emits `fly.fallback`. Outputs are spring-smoothed so a swap never snaps.
- **Claims policy:** UI copy says "experimental micro-motion modulation driven by a connectome-derived simulation". It makes no claims about emotion, language, personality, learning or awareness.
- **Evaluation:** blind A/B of FSM vs. sidecar on identical event logs, measuring p50/p95 latency, RSS/VRAM and frame-time delta. **Hardware-dependent and [UNKNOWN] today.**

---

## 7. Brain inspector UI [PROPOSED]

**Native (default, Bevy):** a panel usable while the avatar dock stays visible. It reuses the existing native panel framework; the specific UI crate was not in the supplied source and is **[UNKNOWN]**. Tabs:

1. **Live:** controller in use (FSM/fly/off), FSM state, mood VAD, last action, cooldowns.
2. **Events:** ring tail with tag filter and dropped count.
3. **Recall:** last `RecallTrace` with record IDs, scores, matched terms, excluded counts and inserted chars. Clicking opens the record with evidence.
4. **Decisions:** receipts showing backend, model, latency, status and budget left.
5. **Candidates:** accept / dismiss / correct / set expiry.
6. **Graph (optional):** deterministic layout of ledger records (nodes) and `supersedes` edges.
   - Positions come from a hash of the UUID, an independent reimplementation of the idea in `networkLayout()`.
   - Nodes highlight **only** for hits in the current trace and fade after 7 s.
   - Rendered in a small Bevy render-to-texture viewport capped at 10 Hz, or as a 2D canvas.

**Three.js web panel (optional, later):**
- A loopback-only, read-only HTTP endpoint serves inspector snapshots as JSON. It is off by default, bound to 127.0.0.1, uses a random per-session token, and has no write routes.
- A separate static page written from scratch uses the `three` npm package (MIT). No T800 code.
- The Rust side remains the sole owner of deterministic state. The web panel never mutates it.

---

## 8. Delivery plan

### 8.1 What can be built today vs. what is blocked

| Item | Today | Blocked on |
|---|---|---|
| RecallTrace, event ring, inspector tabs 1–5 | Yes | — |
| AffectFsm, anim catalog, clamped actions | Yes, but visual quality needs interactive acceptance | GPU/desktop acceptance |
| Decision adapter with mock server tests | Yes | Live Jev results need a Vercel key and budget |
| Persistent candidates, deterministic extraction | Yes | — |
| Reflection and diary | Code yes | A Vercel writer model validated for strict JSON; budget |
| SQLite migration | Code yes | A dependency decision; gate in §3.4 |
| Fly sidecar | Interface and stub yes | Dataset download (user), hardware measurement, decoder evaluation |
| Reusing T800 code or the authored brain | No | A published license |

### 8.2 Next three commits

**Commit 1: `brain: recall traces and bounded event log`**
- Add `src/brain_events.rs` and `src/recall_trace.rs`.
- Refactor `facts_memory::recall`, `memory::MemoryState::context` and `semantic_memory::take_context_for` to emit `RecallTrace` alongside unchanged prompt text.
- Add a read-only inspector with Live, Events and Recall tabs.
- Tests:
  - prompt text is **byte-identical** before and after the refactor (golden);
  - trace hits equal exactly the inserted records;
  - superseded and expired records are counted as excluded, never as hits;
  - ring overflow drops the oldest entry and increments `dropped` without blocking;
  - recall is deterministic for 256 records.

**Commit 2: `embodiment: AffectFsm, clip catalog, typed gesture decision`**
- Add `src/embodiment.rs`, `src/anim_catalog.rs`, and `src/decision_policy.rs`. `recipient_decision::choose` is reimplemented on top of the shared transport with behavior preserved.
- Gesture and attention questions run concurrently with the writer and are applied at `speech.start` with epoch checks.
- Tests:
  - FSM golden trace (seed plus event log gives an identical action log);
  - clamps reject NaN, oversized and unknown clips;
  - stale-epoch and wrong-character results dropped;
  - loopback mock server cases (valid / invalid enum / refusal / 5 s delay / HTTP 500) each map to the correct status;
  - existing recipient tests unchanged.

**Commit 3: `memory: persistent reviewed candidates and Jev triage`**
- Add `candidates.jsonl` with expiry and cap, bounded deterministic extraction with verbatim offsets, an optional triage question, and a Candidates tab.
- `facts.jsonl` format is unchanged.
- Tests:
  - evidence is always a substring of the source turn;
  - candidates never enter recall until accepted;
  - accepting preserves the UUID and `turn_id`;
  - dedupe against active and superseded records;
  - character switch with a pending worker result discards it via generation;
  - an old reader loads `facts.jsonl` after candidate activity (compatibility fixture);
  - Jev cannot alter text (a mock returns extra fields and they are rejected).

### 8.3 Latency and integration gates (all must pass before enabling by default)

1. **First-token isolation:** a mock writer that streams immediately and a mock decision that sleeps 5 s. The time to first token and to `speech.start` must not change beyond noise (p95 delta ≤ 5 ms over 50 runs, in-process mock clock).
2. **Recall deadline:** with a synthetic 256-record ledger, p95 lexical recall ≤ 2 ms. An injected 100 ms slow disk read triggers `memory.recall.timeout` and the request proceeds.
3. **Frame budget:** a headless or offscreen run with the FSM plus inspector open adds ≤ 0.2 ms p95 to the frame system time. Whether an offscreen harness exists in-repo is **[UNKNOWN]**; it is assumed from earlier VALIDATION captures.
4. **Interruption:** interrupting mid-decision means no gesture fires afterward and the epoch advances (`BehaviorEvent::Interrupted`).
5. **Restart and suspend:** candidate expiry and scheduler skip-on-resume are deterministic with an injected clock.
6. **Fly (later):** sidecar kill, NaN injection and 3 consecutive misses each produce `fly.fallback` within one tick, with no visible snap (spring continuity assertion).

These are automated gates. Visual quality, perceived responsiveness and live-provider accuracy require separate interactive or provider-backed acceptance, which is **not claimed** here.

---

## 9. Risks and open questions

- **T800 license:** if no license is published, only independent reimplementation is permitted. The authored biography must not be bundled.
- **Arousal range mismatch** (§1.1) must be fixed before AffectFsm consumes mood.
- **`deny_unknown_fields`** makes any schema extension of `facts.jsonl` a rollback hazard. Keep extensions in new files.
- Native support for **Jev predicate/score answers** is unverified. Use choice-only until tested.
- **Fly runtime cost** on consumer hardware is unmeasured. The 20 Hz / 30 ms budget is an acceptance target, not a demonstrated result.
- **Cost:** pricing for gateway Jev and writer calls is unknown. Budgets are expressed in calls and tokens, and usage is recorded from provider responses.
