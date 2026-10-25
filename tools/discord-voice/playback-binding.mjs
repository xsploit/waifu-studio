import { PlaybackQueue } from './playback-queue.mjs';
import { PlaybackFeedback } from './playback-feedback.mjs';
import { PlaybackAdapter } from './playback.mjs';
const sameTarget = (a, b) => a?.uuid?.toLowerCase() === b?.uuid?.toLowerCase() && a?.generation === b?.generation;

// Owns outgoing SDK resources/ACK forwarding, never the connection or TTS.
// Availability ACK precedes Start; Finished ACK precedes the next Start.
export class PlaybackBinding {
  #feedback; #adapter; #now; #closed = false;
  #authority; #actions; #emit; #target; #pendingAvailable; #available = false;
  constructor(handshake, connection, { actions, emit, now, setOutputSpeaking,
    muted = false, adapterFactory = options => new PlaybackAdapter(options) }) {
    this.#authority = { session_id: handshake.session_id, process_generation: handshake.process_generation };
    this.#now = now; this.#actions = actions; this.#emit = emit;
    this.#feedback = new PlaybackFeedback(handshake, { actions, emit });
    try {
      const queue = new PlaybackQueue(handshake);
      this.#adapter = adapterFactory({ queue, connection, now, setOutputSpeaking,
        onFeedback: event => {
          if (this.#closed) return;
          if (!this.#feedback.event(event, now())) throw new Error('Playback feedback failed');
        } });
      this.#adapter.setMuted(muted);
      this.#check();
    } catch { this.close(); throw new Error('Playback binding initialization failed'); }
  }
  get closed() { return this.#closed || this.#feedback.stopped || Boolean(this.#adapter?.closed); }
  get available() { return !this.closed && this.#available; }
  get awaitingFeedback() { return this.#feedback.active; }
  #check() { if (this.#feedback.stopped || this.#adapter?.closed) this.close(); }
  #announce(target) {
    try {
      const identity = this.#actions.next('playback');
      this.#pendingAvailable = { ...identity, target: { ...target }, deadline: this.#now() + 10_000 };
      this.#emit({ api_version: 1, type: 'action', ...this.#authority, ...identity,
        target: { ...target }, action: { type: 'audio.playback.available' } });
    } catch { this.close(); }
  }
  host(record) {
    if (this.#closed) return;
    if (this.#pendingAvailable && this.#now() >= this.#pendingAvailable.deadline) { this.close(); return; }
    this.#feedback.host(record, this.#now());
    this.#check(); if (this.#closed) return;
    const message = record.message;
    if (message?.type === 'audio.output' && message.output?.phase === 'start'
      && sameTarget(message.output.target, this.#target) && !this.#available) { this.close(); return; }
    const accepted = this.#adapter.host(record); this.#check(); if (this.#closed) return;
    if (accepted && message?.type === 'event' && message.event?.type === 'target.changed') {
      const target = message.event.target;
      if (!sameTarget(target, this.#target)) {
        this.#target = target ? { ...target } : undefined;
        this.#available = false; this.#pendingAvailable = undefined;
        if (target) this.#announce(target);
      }
    }
    if (message?.type === 'result' && this.#pendingAvailable && message.id === this.#pendingAvailable.id) {
      if (!sameTarget(message.target, this.#pendingAvailable.target) || message.result?.status !== 'ok'
        || message.result.value?.type !== 'accepted') { this.close(); return; }
      this.#pendingAvailable = undefined; this.#available = true;
    }
  }
  tick() {
    if (this.#closed) return;
    if (this.#pendingAvailable && this.#now() >= this.#pendingAvailable.deadline) { this.close(); return; }
    this.#feedback.tick(this.#now()); this.#check(); if (this.#closed) return;
    this.#adapter.tick(); this.#check();
  }
  setMuted(value) { if (!this.#closed) { this.#adapter.setMuted(value); this.#check(); } }
  close() {
    if (this.#closed) return;
    this.#closed = true; this.#available = false; this.#pendingAvailable = undefined;
    this.#feedback.stop(); this.#adapter?.close();
  }
}
