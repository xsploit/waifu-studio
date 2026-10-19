# Memory decision experiment

This is a synthetic smoke evaluation, not an enabled companion feature or a claim that Jev outperforms a generative model. It asks whether typed decisions can help identify memory candidates while Rust preserves exact user evidence and a person reviews changes. No model results have been collected for this evaluation yet.

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

## What would justify runtime integration

Keep exact source quotes and source-turn IDs in Rust. A model decision must not invent evidence, establish truth, overwrite a record or accept its own suggestion. If pilot results are useful, the next runtime slice is an opt-in, bounded background candidate classifier that rechecks character/turn/source revisions before offering a draft for review. Correction recommendations must point to a still-current offered record and retain explicit acceptance. Network failures or abstention should leave memory unchanged.

This experiment does not test source-record selection from a large ledger, semantic recall quality, multi-claim extraction, reflection generation, relationship memory or episodic memory. Those require separate evidence. Jev's typed choices could reduce generative work for triage; summaries and newly written reflection prose still require a text-generating model or a deterministic template.
