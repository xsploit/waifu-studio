# Fable 5.1 appraisal review

2026-10-18. Direct Claude CLI worker coordinated through Squad. Read-only source review of commit `5300e4f`; no tests or live requests performed by reviewer.

## Parent disposition

- Late body-motion hints: documented intentional policy; completed replies are considered once and late appraisal must not replay motion. Same-frame readiness remains a scheduling limitation, not evidence of stale affect being applied.
- Failed provider status: safe stale-result rejection confirmed; cancellation cannot release a blocking worker slot early. No slot-leak claim established.
- Thread creation: replaced infallible thread spawn with named Builder spawn returning a fixed error; failed spawn drops its captured permit.
- Slow body hypothesis remains unverified; no live endpoint experiment performed. Scene affect-before-line ordering is intentional for incoming-message appraisal.

## Original Squad review messages

```text
  [2026-10-19T00:53:27Z] fable-review -> manager: Acknowledged. Reviewing commit 5300e4f appraisal path (appraisal.rs, reply_affect.rs, studio.rs, scene_chat.rs, reply_speech.rs, main.rs) read-only; findings to follow.
  [2026-10-19T00:57:48Z] fable-review -> manager: REVIEW of 5300e4f appraisal path (read-only; nothing compiled or run; all file:line are outputs/waifu-studio/src).
  | 
  | VERIFIED (by code reading):
  | V1. Late appraisal never reaches completed_affect. studio.rs:1172-1176 copies appraised_affect into completed_affect only inside the ProviderEvent::Finished arm, and poll_appraisal runs after the provider-event loop (studio.rs:1237). If the Jev result lands in the same frame as Finished or any later frame, apply_reply still updates mood (studio.rs:877) but completed_affect stays None, so motion_decision::job_affect (ui.rs:1472, motion_decision.rs:431-439) returns None for that turn. Expression drive still happens only if bridge.owned is still alive at behavior_runtime.rs:180-185. Trigger: short/fast LLM reply (local model, one-liner) finishing before the <=1.5 s Jev call. Fix sketch: in the Finished arm, read a result from pending.job before computing completed_affect, or set completed_affect from poll_appraisal when completed_chat already matches pending.turn. Limit: I did not review how auto.frame consumes CompletedReply, so impact may be a single lost motion hint rather than a visible bug.
  | V2. Provider Failed path. studio.rs:1219-1233 does not cancel the appraisal; next frame studio.rs:851-864 sees completed_chat=Err, current_turn=None, and discards it with status 'Stale appraisal discarded'. Behaviour is correct (no mood applied), but the status text is misleading and the worker keeps the HTTP call running to completion, holding one of the two slots. Minor.
  | 
  | CHECKED AND OK: strict schema parse (appraisal.rs:80-104; unoffered/abstain/wrong type/missing all rejected, EMOTIONS list matches every AffectEmotion variant); source exclusivity (reply_affect.rs:229-234; studio send_chat 967/984; scene run 643/660; Finished arm 1151-1153); stale invalidation in focused flow covers character, turn, affect config and decision config every frame (studio.rs:857-860), UI edits call cancel_appraisal (ui.rs:1089), send_chat cancels first (978); scene flow gates Affect events on availability, active recipient, config equality and captured decision (scene_chat.rs:437-445) and tick_affect drops entries whose config changed (226-233); cancel drops the job so results are ignored (appraisal.rs:227-229, 252-256); two-slot admission is atomic and cancelled workers keep their slot as documented (appraisal.rs:215-219, test 364-392); scene post-Line wait is bounded by the 1.5 s poll deadline (scene_chat.rs:786-794, appraisal.rs:238-241); completed_chat is read-only elsewhere (orchestration.rs:270/295, behavior_runtime.rs:160), so a late result after a successful Finished still matches current_turn. reply_speech.rs terminal paths: SseReplySpeech::fail marks finished+clears so late push/pump cannot restart (55-60, tests 295-338); HttpReplySpeech stale events ignored by id (142-145); studio pump_sse/receive on Err do fish.cancel+stop_voice (studio.rs:556-568, 1331-1340). No defect found there.
  | 
  | HYPOTHESES (not verified):
  | H1. Slot pinning by a slow endpoint. appraisal.rs:55-56 sets client timeout/connect_timeout 1.5 s, then read_to_end at 172-176. If reqwest 0.12.28 blocking Response applies the timeout per read rather than to the whole body, a server dripping bytes under 1.5 s apart can hold a worker for the full 16 KB cap, and two such requests block every later appraisal with 'previous requests are still closing' (219). Same pattern already exists in recipient_decision.rs, so not new to this commit. Needs a check of reqwest blocking Response::read; mitigation is a wall-clock deadline inside the read loop or a chunked read that re-checks cancelled().
  | H2. appraisal.rs:224 uses std::thread::spawn, which panics if the OS cannot create a thread; scene_chat.rs:290/389 use Builder and map the error. On the focused path that panic would be on the main thread. Low likelihood.
  | H3. In the scene flow the Affect event can precede the Line event (poll_appraisal inside the streaming loop, scene_chat.rs:705), so receipts/mood show before the reply text. Looks intentional ('incoming message' appraisal); flagging only in case the UI expects reply-first ordering.

```

## Opus Fish mailbox investigation

Opus 5.5 inspected the historical overflow hypothesis and proposed reserved terminal admission. Its own producer audit established that current sessions publish exactly one Playing, coalesced Timing and one terminal event, so at most three entries coexist. Parent declined the speculative production policy and duplicate-Playing pressure fixtures. A realistic regression retained from that investigation exercises both terminal outcomes, 500 coalesced timing updates, playback initialization, session cleanup and stale-generation rejection against the unchanged production implementation. This clears the current overflow hypothesis; it is not a reproduced user-visible stall fix.
