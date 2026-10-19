# Memory decision experiment

This is a synthetic smoke evaluation, not an enabled companion feature or a claim that Jev outperforms a generative model. It asks whether typed decisions can help identify memory candidates while Rust preserves exact user evidence and a person reviews changes. One live Jev run has been collected; see "Jev pilot result" below.

`examples/evaluate_memory_decisions.rs` offers six decisions: new fact, new preference, explicit correction, duplicate, skip and abstain. Eighteen fixed cases include paraphrases, negation, changed preferences, unrelated existing records, transient remarks, fiction, third-party quotations, schema injection, ambiguity and a Spanish preference. Each case offers zero or one existing accepted record. No user conversations, application settings or saved credentials are loaded. No memory writes occur.

## Offline checks

Announce Cargo work before running it; use one process and one low-priority job:

```bash
CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=../../work/studio-target nice -n 10 cargo test --example evaluate_memory_decisions
CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=../../work/studio-target nice -n 10 cargo clippy --example evaluate_memory_decisions -- -D warnings
CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=../../work/studio-target nice -n 10 cargo run --example evaluate_memory_decisions -- --help
```

All three offline tests passed, strict example Clippy passed and help-only startup completed on 2026-10-18. The loopback fake server validates the evaluation protocol and strict offered-choice parsing; it does not measure model quality.

## Explicit live experiment

A live run sends synthetic prompts and may incur charges. Set the key in the named environment variable separately; never put it in command arguments or this document. A configured proxy route is mandatory for remote endpoints and is installed explicitly in the client; redirects are disabled. Environment variables are not changed. Without `--run`, the program only prints help and does not load credentials or contact an endpoint.

```bash
CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=../../work/studio-target nice -n 10 cargo run --example evaluate_memory_decisions -- \
  --run --model typesafe-ai/jev --rounds 3 \
  --output benchmarks/memory-decisions-jev.json
```

The output file must not already exist. Requests are sequential; rounds are limited to 1–5. Each request has a 10-second deadline, response bodies are capped at 16 KiB, and remote bodies/errors/credentials are not echoed into the result. Each row records the expected and actual choice or a fixed error category. The report includes accuracy over all attempts and p50/p95 latency for valid responses. Errors count as nonmatches, rather than disappearing from the denominator. Latency includes the complete HTTP request and response, not just inference; first-request overhead is included. Round order rotates deterministically. The harness does not measure provider billing or token use.

Compare other models only through an endpoint that supports this same typed evaluation protocol. Use the same fixtures, rounds and routing conditions. An ordinary chat endpoint cannot be substituted directly. Eighteen hand-authored cases are too small to establish broad accuracy; inspect false retention of fiction, missed negation and incorrect correction decisions separately before enlarging the held-out set. A high aggregate score does not justify automatic writes.

## Jev pilot result — 2026-10-18

`typesafe-ai/jev`, 3 rounds × 18 cases, via the Vercel gateway using the key already saved in Waifu Studio's private credentials file (loaded only into the benchmark process; never printed or written). Raw rows: `benchmarks/memory-decisions-jev.json`.

- 51/54 correct (94.4%), 0 transport or parse errors.
- Latency for the full HTTP round trip: p50 217 ms, p95 292 ms. The first request took 3.6 s (connection setup/cold start).
- Every fiction, third-party quote, schema-injection, temporary-state and question case was skipped in all three rounds. Ambiguous and uncertain claims abstained every time. Corrections and duplicates were all identified.
- The only miss was consistent: "I do not eat peanuts." was labeled `preference` instead of `fact`, in all three rounds. The label is debatable (allergy versus dietary choice); either way, the candidate would still be offered with its exact negated quote, because Rust stores the quote rather than the model's paraphrase. The fixture was left unchanged so this result is not graded after the fact.

This is encouraging for bounded triage: fast, consistent and conservative about fiction. It is still 18 hand-written cases. Token use and price were not measured.

## Decision model comparison — 2026-10-18

Same 18 cases × 3 rounds, same harness, Vercel's shared evaluation endpoint, app-saved key injected only into the process. Price is Vercel's listed input price per million tokens (output is free for all). Raw rows: `benchmarks/memory-decisions-<model>.json`.

| Model | Correct | p50 / p95 ms | $/M input | Misses (each 3/3 rounds) |
|---|---|---|---|---|
| microsoft/microsoft-decision-1 | 54/54 | 353 / 551 | 0.042 | none |
| typesafe-ai/jev | 51/54 | 217 / 292 | 0.042 | peanut negation → preference |
| liquid/d1 | 51/54 | 340 / 1252 | 0.02 | peanut negation → preference |
| openai/gpt-6-luna-decisions | 51/54 | 277 / 756 | 0.10 | peanut negation → preference |
| liquid/d1-ultra | 51/54 | 426 / 1825 | 0.042 | ambiguous referent → skip |
| perplexity/pplx-decider-v1.2 | 48/54 | 320 / 1420 | 0.02 | peanut negation; ambiguous referent → skip |
| perplexity/pplx-decider-flash-v1.2 | 45/54 | 252 / 433 | 0.01 | peanut negation; ambiguous and uncertain → skip |
| convaiinnovations/laya | 18/54 | 298 / 690 | 0 | 12 cases, including fiction → abstain and a third-party quote → fact |

Every model answered identically in all three rounds, so this is effectively 18 data points per model, not 54. The peanut label is debatable. Missing an ambiguity is the more serious error: skipping it silently drops something the user may have meant, while abstaining leaves it for review. Laya is unsuitable. Decision-1 and Jev are the leading candidates: same price, Decision-1 had no misses, and Jev was fastest and most consistent in latency. A larger held-out set is needed before choosing between them.

## Runtime integration — 2026-10-18

`src/memory_triage.rs` now runs this exact prompt (the example imports `body`/`parse` from it) as an off-default memory suggestion source, defaulting to Decision-1. It only offers pending candidates; acceptance, correction checks and recall rules are unchanged. The constraints below were the integration criteria and still apply.

## What would justify runtime integration

Keep exact source quotes and source-turn IDs in Rust. A model decision must not invent evidence, establish truth, overwrite a record or accept its own suggestion. If pilot results are useful, the next runtime slice is an opt-in, bounded background candidate classifier that rechecks character/turn/source revisions before offering a draft for review. Correction recommendations must point to a still-current offered record and retain explicit acceptance. Network failures or abstention should leave memory unchanged.

This experiment does not test source-record selection from a large ledger, semantic recall quality, multi-claim extraction, reflection generation, relationship memory or episodic memory. Those require separate evidence. Jev's typed choices could reduce generative work for triage; summaries and newly written reflection prose still require a text-generating model or a deterministic template.
