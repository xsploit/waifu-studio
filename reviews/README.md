# Source reviews, 2026-10-18

These are read-only Opus research/audit snapshots, not runtime acceptance. Later revisions in each report supersede withdrawn findings.

Since these reports: surface-backed transient seating with the upstream clearance, mesh-margin KWin hit testing and bridge snap requests were implemented and covered by offline tests. The initial A-pose finding was withdrawn after asset measurement; no generic A-to-T correction was added. The first WebSocket phrase flush was lowered. The zero-manual-weight lip-sync regression was subsequently fixed and tested.

The VR report is research only; no VR support or headset acceptance is claimed. The remaining-feature report is a gap list, not evidence those features shipped. Completed Fable reviews are recorded in [fable-code-review.md](fable-code-review.md) and [fable-integrated-review.md](fable-integrated-review.md); their findings describe the snapshots they reviewed. See VALIDATION.md and benchmarks/RESULTS.md for the current evidence and limitations.

Additional Opus5.5 reports are `opus-brain-integration.md` (Grillo/Jev/T800/connectome architecture) and `opus-latency-next-steps.md` (latency engineering review). Opus received source evidence with tools disabled; the integrating agent verified official provider endpoints and ran the benchmark calls. Follow-up notes distinguish later measurements and implemented instrumentation/client reuse from unimplemented suggestions.


The independent Muse review is preserved verbatim in [muse-independent-review.md](muse-independent-review.md). It began at `88cfbfc`, but its final 503-test run included `7f5ef9f` and the material-isolation work later committed as `cc1e0b2`; it does not review subsequent SSE sentence scheduling. Its README test-count finding was fixed by linking dated receipts instead of repeating a changing count.

Integration follow-up on its hypotheses: H1 is unverified and appears unreachable under the current production publication pattern: one WAV readiness callback (`voice_stream.rs` guards both call sites with `emitted`), coalesced timing, and one terminal event per generation yield at most three pending entries; stale generations are rejected and cancellation clears the mailbox. The generic eight-entry cap is not itself a reproduced production failure. H2 is also unverified: both the KWin script (`assets/desktop/kwin-surfaces.js`) and Hyprland backend request a 50 ms cadence. The 500 ms cursor freshness limit deliberately rejects stalled tracking sooner than the one-second geometry TTL. No freshness limit was extended without runtime evidence.
