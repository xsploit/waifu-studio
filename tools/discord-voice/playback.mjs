import { Readable } from 'node:stream';
import { createAudioPlayer, createAudioResource, AudioPlayerStatus, NoSubscriberBehavior,
  StreamType, VoiceConnectionStatus } from '@discordjs/voice';
import { VoiceCodec } from './codec.mjs';

// Inert until explicitly constructed around a caller-owned Ready connection.
// The caller owns the monotonic clock and must forward every host/control change.
export class PlaybackAdapter {
  #queue; #connection; #player; #subscription; #codec; #stream; #resource;
  #clock; #speaking; #createResource; #createCodec; #turn; #drained;
  #drainedAt; #demand = false; #closed = false; #receipts = [];
  #onPlayerState; #onConnectionState; #onError; #feedback;
  constructor({ queue, connection, now = () => performance.now(), setOutputSpeaking = () => {},
    player = createAudioPlayer({ behaviors: { noSubscriber: NoSubscriberBehavior.Stop, maxMissedFrames: 250 } }),
    resourceFactory = createAudioResource, codecFactory = () => new VoiceCodec(), onFeedback = () => {} }) {
    if (connection.state.status !== VoiceConnectionStatus.Ready || connection.state.subscription) {
      throw new Error('Playback requires an unsubscribed Ready connection');
    }
    if (typeof onFeedback !== 'function') throw new Error('Invalid playback feedback callback');
    this.#feedback = onFeedback;
    this.#queue = queue; this.#connection = connection; this.#player = player;
    this.#clock = now; this.#speaking = setOutputSpeaking;
    this.#createResource = resourceFactory; this.#createCodec = codecFactory;
    this.#onError = () => this.close('player_failed');
    this.#onPlayerState = (oldState, state) => {
      if (!this.#turn || oldState.resource !== this.#resource || state.status !== AudioPlayerStatus.Idle) return;
      if (this.#drained) this.#finish('completed', 'player_idle');
      else if (this.#queue.active && this.#queue.bufferedFrames === 0) {
        // An SDK underrun may win the timer race against the source-stall tick.
        // Cancel the empty turn, preserving the healthy call and its player.
        this.#queue.cancel('source_stalled'); this.#outcome();
      } else this.close('player_stopped');
    };
    this.#onConnectionState = () => {
      if (connection.state.status !== VoiceConnectionStatus.Ready
        || connection.state.subscription !== this.#subscription) this.close('connection_changed');
    };
    // Subscribe before observing state changes: subscribe synchronously emits one.
    this.#subscription = connection.subscribe(player);
    if (!this.#subscription || connection.state.subscription !== this.#subscription) {
      this.#subscription?.unsubscribe();
      throw new Error('Playback subscription unavailable');
    }
    player.on('stateChange', this.#onPlayerState); player.on('error', this.#onError);
    connection.on('stateChange', this.#onConnectionState);
  }
  get closed() { return this.#closed; }
  takeReceipt() { return this.#receipts.shift(); }
  host(record) {
    if (this.#closed) return false;
    const accepted = this.#queue.host(record, this.#clock());
    if (accepted && record.message?.type === 'audio.output' && record.message.output?.phase === 'start') {
      if (this.#turn) { this.close('overlapping_playback'); return false; }
      this.#start(record.message.output);
    }
    if (!this.#queue.stopped && this.#turn) {
      const message = record.message;
      if (accepted && message?.type === 'event' && message.event?.type === 'target.changed') {
        this.#cancelChangedTarget(message.event.target);
      }
      const output = message?.output;
      if (message?.type === 'audio.output' && output?.phase === 'cancel'
        && output.playback_id.toLowerCase() === this.#turn?.playback_id
        && this.#sameTarget(output.target)) this.#finish('cancelled', 'host_cancelled');
    }
    this.tick(); return accepted;
  }
  #sameTarget(target) {
    return target?.uuid?.toLowerCase() === this.#turn?.target.uuid.toLowerCase()
      && target?.generation === this.#turn?.target.generation;
  }
  #cancelChangedTarget(target) {
    if (this.#turn && !this.#sameTarget(target)) this.#finish('cancelled', 'target_changed');
  }
  setTarget(target) {
    if (!this.#closed) {
      if (this.#queue.setTarget(target, this.#clock())) this.#cancelChangedTarget(target);
      this.tick();
    }
  }
  setMuted(muted) { if (!this.#closed) { this.#queue.setMuted(muted, this.#clock()); if (muted && !this.#queue.stopped) this.#finish('cancelled', 'muted'); this.tick(); } }
  tick() {
    if (this.#closed) return;
    this.#queue.tick(this.#clock());
    if (this.#queue.stopped) { this.close(this.#queue.stopReason); return; }
    this.#outcome();
    if (this.#drained && this.#clock() - this.#drainedAt >= 5000) { this.close('player_idle_timeout'); return; }
    this.#pump();
  }
  #start(output) {
    this.#turn = { playback_id: output.playback_id.toLowerCase(), target: { ...output.target }, frame_count: 0 };
    this.#drained = false;
    try {
      this.#codec = this.#createCodec();
      const stream = new Readable({ objectMode: true, highWaterMark: 1,
        read: () => { this.#demand = true; this.#pump(); } });
      this.#stream = stream;
      stream.on('error', () => { if (this.#stream === stream) this.close('stream_failed'); });
      this.#resource = this.#createResource(stream, { inputType: StreamType.Opus, silencePaddingFrames: 5 });
      this.#speaking(true);
      this.#player.play(this.#resource);
      if (this.#turn) this.#notify('ready');
    } catch { this.close('playback_start_failed'); }
  }
  #outcome() {
    if (this.#queue.stopped) { this.close(this.#queue.stopReason); return; }
    const outcome = this.#queue.takeOutcome();
    if (!outcome) return;
    if (outcome.outcome === 'drained' && this.#turn) {
      this.#drained = true; this.#drainedAt = this.#clock(); this.#stream?.push(null);
    } else if (this.#turn) this.#finish('cancelled', outcome.reason);
  }
  #pump() {
    if (this.#closed || !this.#turn || !this.#demand || this.#drained) return;
    try {
      const pcm = this.#queue.pull(this.#clock());
      if (pcm) {
        // One packet for each demand. Never drain the queue on push(false).
        this.#demand = false;
        this.#stream.push(this.#codec.encode(pcm));
        this.#turn.frame_count++;
        this.#notify('consumed');
      }
      this.#outcome();
    } catch { this.close('encoding_failed'); }
  }
  #notify(phase, turn = this.#turn, outcome) {
    if (!turn) return;
    const event = { phase, playback_id: turn.playback_id, target: { ...turn.target } };
    if (phase !== 'ready') event.frame_count = turn.frame_count;
    if (phase === 'finished') event.outcome = outcome;
    try { this.#feedback(event); }
    catch { this.#feedback = () => {}; this.close('feedback_failed'); }
  }
  #finish(outcome, reason) {
    if (!this.#turn) return;
    const turn = this.#turn; this.#turn = undefined;
    const resource = this.#resource; this.#resource = undefined;
    if (this.#player.state.resource === resource) this.#player.stop(true);
    this.#stream?.destroy(); this.#stream = undefined;
    this.#codec?.close(); this.#codec = undefined;
    this.#demand = false; this.#drained = false;
    this.#receipts.push({ ...turn, outcome, reason });
    if (this.#receipts.length > 64) this.#receipts.shift();
    this.#speaking(false);
    this.#notify('finished', turn, outcome);
  }
  close(reason = 'stopped') {
    if (this.#closed) return;
    this.#closed = true; this.#queue.stop(reason); this.#queue.takeOutcome();
    this.#finish(['stopped', 'host_stopping', 'connection_changed'].includes(reason) ? 'cancelled' : 'failed', reason);
    this.#connection.off('stateChange', this.#onConnectionState);
    this.#player.off('stateChange', this.#onPlayerState);
    this.#player.off('error', this.#onError);
    // Never destroy/rejoin the caller's connection or stop a foreign player.
    this.#subscription.unsubscribe();
  }
}
