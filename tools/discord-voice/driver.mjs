import { ActionSequence } from './action-sequence.mjs';
import { HostPipe } from './host-io.mjs';
import { ReceiveBridge, validateVoiceHandshake } from './receive-bridge.mjs';
import { PlaybackBinding } from './playback-binding.mjs';
import { ReceiverBinding } from './receiver.mjs';

// One host incarnation and one async connection attempt. The connector owns
// login/join resources until returning { connection, botUserId, lookupParticipant,
// dispose }; it must honor AbortSignal and make cleanup idempotent.
export class VoiceDriver {
  #pipe;
  #connect;
  #now;
  #notify;
  #abort = new AbortController();
  #handshake;
  #directions;
  #hostSequence;
  #started;
  #pendingStartup = false;
  #queued = [];
  #queuedBytes = 0;
  #bridge;
  #actions = new ActionSequence();
  #binding;
  #playback;
  #createPlayback;
  #outputMuted = false;
  #playbackSpeaking = false;
  #session;
  #disposed = new WeakSet();
  #stopped = false;
  #cleanupFailed = false;
  #cleanupPending = 0;
  #muted = false;
  #outputSpeaking = false;

  constructor(input, output, { connect, onStop, now = () => performance.now(), autoTick = true,
    createPlayback = (...args) => new PlaybackBinding(...args) }) {
    if (typeof connect !== 'function' || typeof onStop !== 'function' || typeof createPlayback !== 'function') throw new Error('Invalid voice driver configuration');
    this.#createPlayback = createPlayback;
    this.#connect = connect; this.#now = now; this.#notify = onStop;
    this.#pipe = new HostPipe(input, output, { now, autoTick,
      onRecord: record => this.#record(record), onTick: time => this.#tick(time), onStop: reason => this.stop(reason) });
  }

  get stopped() { return this.#stopped; }
  get ready() { return Boolean((this.#binding || this.#playback) && !this.#binding?.closed && !this.#playback?.closed && !this.#stopped); }
  get playbackReady() { return !this.#stopped && Boolean(this.#playback?.available); }
  get pendingStartup() { return this.#pendingStartup; }
  get cleanupFailed() { return this.#cleanupFailed; }
  get shutdownReady() { return this.#stopped && !this.#pendingStartup && this.#cleanupPending === 0 && this.#pipe.pendingBytes === 0; }
  tick() { this.#pipe.tick(); }
  setMuted(value) { this.#muted = Boolean(value); this.#binding?.setMuted(this.#muted); }
  setOutputMuted(value) { this.#outputMuted = Boolean(value); this.#playback?.setMuted(this.#outputMuted); }
  #applyOutputSuppression() { this.#binding?.setOutputSpeaking(this.#outputSpeaking || this.#playbackSpeaking); }
  setOutputSpeaking(value) { this.#outputSpeaking = Boolean(value); this.#applyOutputSuppression(); }

  #record(record) {
    if (this.#stopped) return;
    if (!this.#handshake) {
      this.#directions = validateVoiceHandshake(record);
      this.#handshake = structuredClone(record);
      this.#hostSequence = record.sequence;
      this.#started = this.#now();
      this.#pendingStartup = true;
      // Defer the effect so EOF/stop in this input batch takes precedence.
      Promise.resolve().then(() => {
        if (this.#stopped) return undefined;
        if (this.#now() - this.#started >= 15_000) { this.stop('startup_timeout'); return undefined; }
        return this.#connect({ settings: structuredClone(record.message.settings ?? {}), directions: this.#directions, signal: this.#abort.signal });
      }).then(session => {
        this.#pendingStartup = false;
        if (this.#stopped || this.#abort.signal.aborted) { this.#dispose(session); return; }
        this.#session = session;
        try {
          if (this.#now() - this.#started >= 15_000) { this.stop('startup_timeout'); return; }
          if (!session || typeof session.dispose !== 'function') throw new Error('Invalid connector result');
          if (this.#directions.receive) {
            this.#bridge = new ReceiveBridge(this.#handshake, { botUserId: session.botUserId, actions: this.#actions, emit: action => this.#pipe.send(action) });
            for (const queued of this.#queued) this.#bridge.host(queued, this.#now());
            if (this.#bridge.stopped) throw new Error('Invalid startup host records');
            this.#binding = new ReceiverBinding(session.connection, this.#bridge, {
              lookupParticipant: session.lookupParticipant, now: this.#now,
              muted: this.#muted, outputSpeaking: this.#outputSpeaking,
            });
          }
          if (this.#directions.playback) {
            this.#playback = this.#createPlayback(this.#handshake, session.connection, {
              actions: this.#actions, emit: action => this.#pipe.send(action), now: this.#now,
              muted: this.#outputMuted, setOutputSpeaking: value => {
                this.#playbackSpeaking = Boolean(value); this.#applyOutputSuppression();
              },
            });
            for (const queued of this.#queued) this.#playback.host(queued);
            if (this.#playback.closed) throw new Error('Playback startup failed');
          }
          this.#queued = []; this.#queuedBytes = 0;
        } catch { this.stop('startup_failed'); }
      }, () => {
        this.#pendingStartup = false;
        if (!this.#stopped) this.stop('startup_failed');
      }).catch(() => this.stop('startup_failed'));
      return;
    }
    if (record.session_id !== this.#handshake.session_id || record.process_generation !== this.#handshake.process_generation
      || !Number.isSafeInteger(record.sequence) || record.sequence <= this.#hostSequence) {
      this.stop('host_identity'); return;
    }
    this.#hostSequence = record.sequence;
    if (record.message?.event?.type === 'app.stopping') { this.stop('host_stopping'); return; }
    if (this.#binding || this.#playback) {
      this.#playback?.host(record);
      if (this.#playback?.closed) { this.stop('playback_stopped'); return; }
      this.#binding?.host(record);
      if (this.#binding?.closed || this.#bridge?.stopped) this.stop('receiver_stopped');
    } else {
      const bytes = Buffer.byteLength(JSON.stringify(record));
      if (this.#queued.length >= 16 || this.#queuedBytes + bytes > 65536) { this.stop('startup_overflow'); return; }
      this.#queued.push(record); this.#queuedBytes += bytes;
    }
  }

  #tick(now) {
    if (this.#stopped) return;
    if (this.#pendingStartup && now - this.#started >= 15_000) { this.stop('startup_timeout'); return; }
    this.#playback?.tick();
    if (this.#playback?.closed) { this.stop('playback_stopped'); return; }
    this.#binding?.tick();
    if (this.#binding?.closed || this.#bridge?.stopped) this.stop('receiver_stopped');
  }

  #dispose(session) {
    if (!session || typeof session !== 'object' || this.#disposed.has(session)) return;
    this.#disposed.add(session);
    try {
      const result = typeof session.dispose === 'function' ? session.dispose() : session.connection?.destroy?.();
      if (result && typeof result.then === 'function') {
        this.#cleanupPending++;
        Promise.resolve(result).catch(() => { this.#cleanupFailed = true; }).finally(() => { this.#cleanupPending--; });
      }
    } catch { this.#cleanupFailed = true; }
  }

  stop(reason = 'stopped') {
    if (this.#stopped) return;
    this.#stopped = true; this.#abort.abort();
    try { this.#binding?.close(); } catch { this.#cleanupFailed = true; }
    this.#bridge?.stop();
    try { this.#playback?.close(); } catch { this.#cleanupFailed = true; }
    this.#queued = []; this.#queuedBytes = 0;
    this.#pipe?.stop(reason); this.#dispose(this.#session); this.#session = undefined;
    try {
      const result = this.#notify(reason);
      if (result && typeof result.then === 'function') Promise.resolve(result).catch(() => { this.#cleanupFailed = true; });
    } catch { this.#cleanupFailed = true; }
  }
}
