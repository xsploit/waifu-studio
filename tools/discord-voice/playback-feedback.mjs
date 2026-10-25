import { ActionSequence } from './action-sequence.mjs';
const UUID = /^(?!00000000-0000-0000-0000-000000000000$)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const uuid = value => typeof value === 'string' && UUID.test(value);
const positive = value => Number.isSafeInteger(value) && value > 0;
const keys = (value, names) => value && typeof value === 'object' && !Array.isArray(value)
  && Object.keys(value).length === names.length && names.every(name => Object.hasOwn(value, name));
const targetValid = t => keys(t, ['uuid', 'generation']) && uuid(t.uuid)
  && Number.isSafeInteger(t.generation) && t.generation >= 0;
const sameTarget = (a, b) => a?.uuid?.toLowerCase() === b?.uuid?.toLowerCase() && a?.generation === b?.generation;
const countValid = n => Number.isInteger(n) && n >= 0 && n <= 65536;

// One action in flight, one cumulative count, one terminal marker. No PCM,
// timers, clients or transport ownership. Caller forwards every host record,
// ticks deadlines, and admits the next Start only after !active && !stopped.
export class PlaybackFeedback {
  #host; #sequence; #actions; #emit; #turn; #pending; #lastTime = 0; #stopped = false; #retired = [];
  constructor(handshake, { actions, emit }) {
    if (handshake?.api_version !== 1 || handshake.type !== 'host' || !uuid(handshake.session_id)
      || !positive(handshake.process_generation) || !positive(handshake.sequence)
      || handshake.message?.type !== 'handshake' || !Array.isArray(handshake.message.capabilities)
      || !handshake.message.capabilities.includes('audio.playback')
      || !(actions instanceof ActionSequence) || typeof emit !== 'function') throw new Error('Invalid playback feedback configuration');
    this.#host = { session_id: handshake.session_id.toLowerCase(), process_generation: handshake.process_generation };
    this.#sequence = handshake.sequence; this.#actions = actions; this.#emit = emit;
  }
  get stopped() { return this.#stopped; }
  get active() { return Boolean(this.#turn); }
  get pendingActions() { return this.#pending ? 1 : 0; }
  stop() { this.#stopped = true; this.#turn = undefined; this.#pending = undefined; }
  #clock(now) {
    if (!Number.isFinite(now) || now < this.#lastTime) { this.stop(); return false; }
    this.#lastTime = now; return !this.#stopped;
  }
  event(event, now) {
    if (!this.#clock(now)) return false;
    this.tick(now);
    if (this.#stopped) return false;
    const extra = event?.phase === 'ready' ? [] : event?.phase === 'consumed' ? ['frame_count'] : ['frame_count', 'outcome'];
    if (!['ready', 'consumed', 'finished'].includes(event?.phase)
      || !keys(event, ['phase', 'playback_id', 'target', ...extra]) || !uuid(event.playback_id) || !targetValid(event.target)) {
      this.stop(); return false;
    }
    if (event.phase !== 'ready' && (!countValid(event.frame_count)
      || (event.phase === 'consumed' && !event.frame_count)
      || (event.phase === 'finished' && !['completed', 'cancelled', 'failed'].includes(event.outcome)))) {
      this.stop(); return false;
    }
    // A host target event may retire feedback before the adapter emits the
    // matching cancellation. Stale callbacks are successful no-ops in either
    // forwarding order, so the caller does not mistake them for pipe failures.
    const eventId = event.playback_id.toLowerCase();
    if (this.#retired.includes(eventId) && this.#turn?.id !== eventId) return true;
    if (event.phase === 'ready') {
      if (this.#turn) { this.stop(); return false; }
      this.#retire(event.playback_id);
      this.#turn = { id: event.playback_id.toLowerCase(), target: { ...event.target, uuid: event.target.uuid.toLowerCase() },
        count: 0, acknowledged: 0, ready: false, ending: undefined, started: now };
    } else {
      // Codec/resource setup can fail before Ready exists. The host ledger
      // accepts a zero-consumption terminal failure for its Starting turn.
      if (!this.#turn && event.phase === 'finished' && event.frame_count === 0 && event.outcome !== 'completed') {
        this.#retire(event.playback_id);
        this.#turn = { id: event.playback_id.toLowerCase(), target: { ...event.target, uuid: event.target.uuid.toLowerCase() },
          count: 0, acknowledged: 0, ready: true, ending: undefined, started: now };
      }
      const turn = this.#turn;
      if (!turn || turn.id !== event.playback_id.toLowerCase() || !sameTarget(turn.target, event.target)) {
        this.stop(); return false;
      }
      if (turn.ending || event.frame_count < turn.count || (event.phase === 'consumed' && event.frame_count === turn.count)) {
        this.stop(); return false;
      }
      turn.count = event.frame_count;
      if (event.phase === 'finished') turn.ending = event.outcome;
    }
    this.#pump(now); return !this.#stopped;
  }
  #retire(id) {
    this.#retired.push(id.toLowerCase());
    if (this.#retired.length > 64) this.#retired.shift();
  }
  #pump(now) {
    const turn = this.#turn;
    if (this.#stopped || !turn || this.#pending) return;
    let receipt;
    if (!turn.ready) receipt = { phase: 'ready', playback_id: turn.id };
    else if (turn.count > turn.acknowledged) receipt = { phase: 'consumed', playback_id: turn.id, frame_count: turn.count };
    else if (turn.ending) receipt = { phase: 'finished', playback_id: turn.id, frame_count: turn.count, outcome: turn.ending };
    else return;
    try {
      const identity = this.#actions.next('playback');
      this.#pending = { ...identity, target: { ...turn.target }, receipt, deadline: now + 10_000 };
      this.#emit({ api_version: 1, type: 'action', ...this.#host, ...identity, target: { ...turn.target },
        action: { type: 'audio.playback.receipt', receipt: { ...receipt } } });
    } catch { this.stop(); } // Uncertain send consumes identity and is never replayed.
  }
  host(record, now) {
    if (!this.#clock(now)) return;
    this.tick(now);
    if (this.#stopped) return;
    if (record?.api_version !== 1 || record.type !== 'host' || !uuid(record.session_id)
      || record.session_id.toLowerCase() !== this.#host.session_id || record.process_generation !== this.#host.process_generation
      || !positive(record.sequence) || record.sequence <= this.#sequence || record.message?.type === 'handshake') { this.stop(); return; }
    this.#sequence = record.sequence;
    const message = record.message;
    if (message?.type === 'event' && message.event?.type === 'app.stopping') { this.stop(); return; }
    if (message?.type === 'event' && message.event?.type === 'target.changed') {
      if (!keys(message, ['type', 'event']) || !keys(message.event, ['type', 'target'])
        || (message.event.target !== null && !targetValid(message.event.target))) { this.stop(); return; }
      if (this.#turn && !sameTarget(this.#turn.target, message.event.target)) {
        this.#turn = undefined; this.#pending = undefined;
      }
    }
    if (message?.type !== 'result' || !this.#pending || message.id !== this.#pending.id) return;
    const pending = this.#pending;
    if (!sameTarget(message.target, pending.target) || message.result?.status !== 'ok'
      || message.result.value?.type !== 'accepted') { this.stop(); return; }
    this.#pending = undefined;
    if (pending.receipt.phase === 'ready') this.#turn.ready = true;
    else if (pending.receipt.phase === 'consumed') this.#turn.acknowledged = pending.receipt.frame_count;
    else { this.#turn = undefined; return; }
    this.#pump(now);
  }
  tick(now) {
    if (!this.#clock(now)) return;
    // The source queue expires at22min, but a drained SDK resource may need
    // up to5s to reach Idle. Once terminal feedback exists, its finite ACK
    // chain (Ready/Consumed/Finished) uses the individual10s deadlines.
    if ((this.#pending && now >= this.#pending.deadline)
      || (this.#turn && !this.#turn.ending && now - this.#turn.started >= 22 * 60_000 + 5000)) this.stop();
  }
}
