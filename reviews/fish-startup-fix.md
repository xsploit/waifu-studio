# Fish startup pipeline fix — 2026-10-19

The recovered WebWaifu4 forwards streaming audio promptly; its browser PCM scheduler starts with 180 ms lead and later schedules with at least 80 ms. It does not establish a 150–200 ms request-to-speaker result. The public fast WebSocket benchmark excludes the handshake and measures first compressed audio, not playback. Historical measurements remain in [the investigation](Fish%20audio%20latency%20investigation.md).

## Changes

- Reuse a credential-free HTTP client for HTTP and timestamp SSE synthesis; authorization remains per request and proxy behavior is preserved.
- Open reply WebSockets while model text is still being generated, retaining cancellation and bounded workers.
- Send an initial HTTP/SSE phrase at an earlier sentence boundary or word boundary; retain later sentence grouping and metadata filtering.
- Start WAV playback after 40 ms of decoded PCM instead of 180 ms. This reduces buffering but may be less tolerant of network stalls; real playback must establish the tradeoff.
- Expose speech connection queued, first speech text queued and first audio queued under the composer's Latest reply timing. Queued audio is not physical speaker onset.

## Verification

587 offline tests passed, strict all-target Clippy clean, touched-file formatting and whitespace clean, normal binary built. One low-priority Cargo job; no new paid/model benchmarks, profile edits or live synthesis. No WebWaifu4 source/assets copied.

## Acceptance still needed

Compare an actual reply using the preserved profile, inspect the timing rows, and listen for gaps. No measured speedup or 150 ms speaker-onset claim is made. HTTP benefits from client reuse, earlier text and the smaller buffer; WebSocket handshake overlap applies when WebSocket delivery is selected.
