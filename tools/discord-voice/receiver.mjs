import { EndBehaviorType, VoiceConnectionStatus } from '@discordjs/voice';

// Attaches only to an already established bot voice connection. This class
// never logs in, joins/rejoins, fetches members, or owns the connection itself.
export class ReceiverBinding {
  #connection;
  #bridge;
  #lookup;
  #clock;
  #silence;
  #stream;
  #closed = false;
  #muted = false;
  #outputSpeaking = false;
  #onSpeaking;
  #onState;
  #onError;

  constructor(connection, bridge, { lookupParticipant, now, silenceMs = 600, muted = false, outputSpeaking = false }) {
    if (connection?.state?.status !== VoiceConnectionStatus.Ready || !connection.receiver
      || typeof connection.receiver.subscribe !== 'function' || typeof connection.receiver.subscriptions?.has !== 'function'
      || typeof connection.receiver.speaking?.on !== 'function'
      || !bridge || bridge.stopped || bridge.active || typeof lookupParticipant !== 'function' || typeof now !== 'function'
      || !Number.isInteger(silenceMs) || silenceMs < 250 || silenceMs > 2000) {
      throw new Error('Invalid ready voice receiver configuration');
    }
    this.#connection = connection;
    this.#bridge = bridge;
    this.#lookup = lookupParticipant; // Synchronous cache lookup, never REST.
    this.#clock = now;
    this.#silence = silenceMs;
    this.#muted = Boolean(muted);
    this.#outputSpeaking = Boolean(outputSpeaking);
    this.#applyMute(); // The caller supplies retained user/output state on reattach.
    this.#onSpeaking = id => this.#start(id);
    this.#onState = (_, next) => { if (next.status !== VoiceConnectionStatus.Ready) this.close(); };
    this.#onError = () => this.close();
    connection.receiver.speaking.on('start', this.#onSpeaking);
    connection.on('stateChange', this.#onState);
    connection.on('error', this.#onError);
  }

  get closed() { return this.#closed; }
  get subscribed() { return Boolean(this.#stream); }

  #start(id) {
    if (this.#closed || this.#muted || this.#outputSpeaking || this.#stream
      || this.#connection.receiver.subscriptions.has(id)) return;
    let participant;
    try { participant = this.#lookup(id); } catch { return; }
    if (!participant || participant.id !== id || typeof participant.bot !== 'boolean') return;
    if (!this.#bridge.begin(participant, this.#clock())) return;
    let stream;
    try {
      stream = this.#connection.receiver.subscribe(id, {
        end: { behavior: EndBehaviorType.AfterSilence, duration: this.#silence },
        highWaterMark: 8,
      });
    } catch { this.#bridge.cancel(this.#clock()); return; }
    const active = () => !this.#closed && this.#stream?.stream === stream;
    const data = packet => {
      if (!active()) return;
      if (!this.#bridge.packet(id, packet, this.#clock())) this.#detach();
    };
    const end = () => {
      if (!active()) return;
      this.#bridge.end(id, this.#clock());
      this.#detach();
    };
    const failed = () => {
      if (!active()) return;
      this.#bridge.cancel(this.#clock());
      this.#detach();
    };
    this.#stream = { stream, data, end, failed };
    stream.on('error', failed);
    stream.on('close', failed);
    stream.on('end', end);
    stream.on('data', data); // Flow immediately; never pause an unbounded UDP source.
  }

  #detach() {
    const owned = this.#stream;
    if (!owned) return;
    this.#stream = undefined;
    owned.stream.off('data', owned.data);
    owned.stream.off('end', owned.end);
    owned.stream.off('close', owned.failed);
    owned.stream.off('error', owned.failed);
    // destroy(error) can schedule an error after teardown. Retain a sink on this
    // owned stream only; the library's close handler removes its subscription.
    owned.stream.on('error', () => {});
    owned.stream.destroy();
  }

  #applyMute() {
    this.#bridge.setMuted(this.#muted || this.#outputSpeaking, this.#clock());
    if (this.#muted || this.#outputSpeaking) this.#detach();
  }
  setMuted(muted) { if (!this.#closed) { this.#muted = Boolean(muted); this.#applyMute(); } }
  setOutputSpeaking(speaking) { if (!this.#closed) { this.#outputSpeaking = Boolean(speaking); this.#applyMute(); } }

  // The I/O driver owns the timer and keeps ticking/reading host receipts even
  // after a call disconnect, until cancellation settles or its deadline stops it.
  tick() {
    this.#bridge.tick(this.#clock());
    if (this.#stream && !this.#bridge.receiving) this.#detach();
    if (this.#bridge.stopped) this.close();
  }
  host(record) {
    this.#bridge.host(record, this.#clock());
    if (this.#stream && !this.#bridge.receiving) this.#detach();
    if (this.#bridge.stopped) this.close();
  }
  close() {
    if (this.#closed) return;
    this.#closed = true;
    this.#bridge.cancel(this.#clock());
    this.#detach();
    this.#connection.receiver.speaking.off('start', this.#onSpeaking);
    this.#connection.off('stateChange', this.#onState);
    this.#connection.off('error', this.#onError);
  }
}
