import { ActionSequence } from './action-sequence.mjs';
import { randomUUID } from 'node:crypto';
import { VoiceCodec } from './codec.mjs';

const UUID = /^(?!00000000-0000-0000-0000-000000000000$)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const positive = value => Number.isSafeInteger(value) && value > 0;
const targetValid = t => t && UUID.test(t.uuid) && Number.isSafeInteger(t.generation) && t.generation >= 0;
const sameTarget = (a, b) => a?.uuid === b?.uuid && a?.generation === b?.generation;
const snowflake = id => typeof id === 'string' && /^[1-9][0-9]{0,19}$/.test(id) && BigInt(id) <= 18446744073709551615n;

// Validate host authority before starting any asynchronous bot connection.
export function validateVoiceHandshake(handshake) {
  const caps = handshake?.message?.capabilities;
  if (handshake?.api_version !== 1 || handshake.type !== 'host' || !UUID.test(handshake.session_id)
    || !positive(handshake.process_generation) || !positive(handshake.sequence)
    || handshake.message?.type !== 'handshake' || !Array.isArray(caps)
    || !(caps.includes('audio.playback')
      || ['audio.transcribe', 'message.deliver', 'lifecycle.observe'].every(cap => caps.includes(cap)))) {
    throw new Error('Invalid voice host handshake');
  }
  return Object.freeze({
    receive: ['audio.transcribe', 'message.deliver', 'lifecycle.observe'].every(cap => caps.includes(cap)),
    playback: caps.includes('audio.playback'),
  });
}

// Transport-independent first-speaker ownership. It consumes decrypted Opus,
// emits bounded host records, and never authenticates or opens a socket.
export class ReceiveBridge {
  #host;
  #emit;
  #bot;
  #target;
  #turn;
  #pending = new Map();
  #actions;
  #hostSequence;
  #lastTime = 0;
  #stopped = false;
  #muted = false;
  #outcome;

  constructor(handshake, { emit, botUserId, actions = new ActionSequence() }) {
    if (!validateVoiceHandshake(handshake).receive) throw new Error('Missing receive grants');
    if (typeof emit !== 'function' || !snowflake(botUserId)) throw new Error('Invalid voice host handshake');
    this.#host = { session_id: handshake.session_id, process_generation: handshake.process_generation };
    this.#hostSequence = handshake.sequence;
    if (!(actions instanceof ActionSequence)) throw new Error('Invalid action sequence');
    this.#actions = actions;
    this.#emit = emit;
    this.#bot = botUserId;
  }

  get stopped() { return this.#stopped; }
  get active() { return Boolean(this.#turn); }
  get receiving() { return Boolean(this.#turn && !this.#stopped && !this.#muted && !this.#turn.ending && ['starting', 'frames'].includes(this.#turn.phase)); }
  get bufferedFrames() { return (this.#turn?.queue.length ?? 0) + [...this.#pending.values()].filter(p => p.phase === 'frame').length; }
  takeOutcome() { const value = this.#outcome; this.#outcome = undefined; return value; }

  #clock(now) {
    if (!Number.isFinite(now) || now < this.#lastTime) { this.stop(); return false; }
    this.#lastTime = now;
    return !this.#stopped;
  }

  begin(participant, now) {
    if (!this.#clock(now) || this.#muted || this.#turn || !this.#target || participant?.bot
      || !snowflake(participant?.id) || participant.id === this.#bot
      || typeof participant.name !== 'string' || !participant.name.trim()
      || Buffer.byteLength(participant.name) > 128 || /[\x00-\x1f\x7f-\x9f]/.test(participant.name)) return false;
    this.#turn = { id: randomUUID(), participant: { id: participant.id, name: participant.name }, target: { ...this.#target },
      codec: new VoiceCodec(), queue: [], nextFrame: 0, total: 0, started: now, phase: 'start', ending: false, finishAdmitted: false };
    this.tick(now);
    return !this.#stopped;
  }

  packet(participantId, opus, now) {
    if (!this.#clock(now) || !this.#turn || this.#turn.participant.id !== participantId
      || this.#turn.ending || !['starting', 'frames'].includes(this.#turn.phase)) return false;
    let frames;
    try { frames = this.#turn.codec.decode(opus); }
    catch { this.cancel(now); return false; }
    if (!this.#append(frames, now)) return false;
    this.tick(now);
    return this.receiving;
  }

  #append(frames, now) {
    if (this.bufferedFrames + frames.length > 32 || this.#turn.total + frames.length > 1500) {
      this.cancel(now); return false;
    }
    this.#turn.total += frames.length;
    this.#turn.queue.push(...frames);
    return true;
  }

  end(participantId, now) {
    if (!this.#clock(now) || !this.#turn || this.#turn.participant.id !== participantId
      || this.#turn.ending || !['starting', 'frames'].includes(this.#turn.phase)) return false;
    if (!this.#append(this.#turn.codec.finish(), now)) return false;
    this.#turn.ending = true;
    this.tick(now);
    return true;
  }

  cancel(now) {
    if (!this.#clock(now) || !this.#turn) return;
    this.#turn.codec.close(); this.#turn.queue = []; this.#turn.phase = 'cancel';
    this.tick(now);
  }
  setMuted(muted, now) { this.#muted = Boolean(muted); if (this.#muted) this.cancel(now); }
  stop() { this.#stopped = true; this.#discard(); }
  #discard() { this.#turn?.codec.close(); this.#turn = undefined; this.#pending.clear(); }

  #send(input, now) {
    let identity;
    try { identity = this.#actions.next('voice'); } catch { this.stop(); return; }
    const { id } = identity;
    const record = { api_version: 1, type: 'action', ...this.#host, ...identity,
      target: this.#turn.target, action: { type: 'audio.input', input } };
    this.#pending.set(id, { phase: input.phase, target: this.#turn.target, deadline: now + 10_000 });
    try { this.#emit(record); } catch { this.stop(); } // Uncertain write: never replay.
  }

  tick(now) {
    if (!this.#clock(now) || !this.#turn) return;
    if ([...this.#pending.values()].some(p => now >= p.deadline)) { this.stop(); return; }
    const turn = this.#turn;
    if (turn.phase === 'completion') { if (now - turn.started >= 80_000) this.stop(); return; }
    if (now - turn.started >= 35_000 && turn.phase !== 'cancel') { this.cancel(now); return; }
    if (turn.phase === 'start') {
      if (!this.#pending.size) {
        turn.phase = 'starting';
        this.#send({ phase: 'start', turn_id: turn.id, participant_id: turn.participant.id, participant_name: turn.participant.name }, now);
      }
      return;
    }
    if (turn.phase === 'cancel') {
      if (!this.#pending.size) { turn.phase = 'cancelling'; this.#send({ phase: 'cancel', turn_id: turn.id }, now); }
      return;
    }
    if (turn.phase !== 'frames') return;
    while (!this.#stopped && turn.queue.length && this.#pending.size < 4) {
      this.#send({ phase: 'frame', turn_id: turn.id, frame_sequence: turn.nextFrame++, pcm: turn.queue.shift() }, now);
    }
    if (!this.#stopped && turn.ending && !turn.queue.length && !this.#pending.size) {
      if (turn.total === 0) { this.cancel(now); return; }
      turn.phase = 'finishing'; this.#send({ phase: 'finish', turn_id: turn.id }, now);
    }
  }

  host(record, now) {
    if (!this.#clock(now)) return;
    if (record?.api_version !== 1 || record.type !== 'host' || record.session_id !== this.#host.session_id
      || record.process_generation !== this.#host.process_generation || !positive(record.sequence)
      || record.sequence <= this.#hostSequence) { this.stop(); return; }
    this.#hostSequence = record.sequence;
    const message = record.message;
    if (message?.type === 'event') {
      const event = message.event;
      if (event?.type === 'target.changed') {
        if (event.target !== null && !targetValid(event.target)) { this.stop(); return; }
        if (!sameTarget(this.#target, event.target)) this.#discard();
        this.#target = event.target ? { uuid: event.target.uuid, generation: event.target.generation } : undefined;
      } else if (event?.type === 'app.stopping') this.stop();
      else if (event?.type === 'audio.input.finished' && this.#turn && event.turn_id === this.#turn.id
        && sameTarget(event.target, this.#turn.target)) {
        if (!['queued', 'failed', 'cancelled', 'rejected'].includes(event.outcome)) { this.stop(); return; }
        this.#outcome = { turnId: event.turn_id, outcome: event.outcome };
        this.#discard();
      }
    } else if (message?.type === 'result') {
      const pending = this.#pending.get(message.id);
      if (!pending) return; // Late result for an explicitly discarded target.
      if (!sameTarget(message.target, pending.target)) { this.stop(); return; }
      this.#pending.delete(message.id);
      if (['permission_denied', 'stale_session', 'stale_process', 'unsupported_version'].includes(message.result?.code)) {
        this.stop(); return;
      }
      if (pending.phase === 'cancel') {
        if (this.#turn.finishAdmitted) this.#turn.phase = 'completion';
        else this.#discard();
        return;
      }
      if (message.result?.status !== 'ok' || message.result.value?.type !== 'accepted') {
        this.cancel(now); // No uncertain action is replayed; discard host buffers.
        return;
      }
      if (pending.phase === 'start' && this.#turn.phase === 'starting') this.#turn.phase = 'frames';
      if (pending.phase === 'finish') {
        this.#turn.finishAdmitted = true;
        if (this.#turn.phase === 'finishing') this.#turn.phase = 'completion';
      }
      this.tick(now);
    } else if (message?.type !== 'audio.output') this.stop(); // Private outbound media belongs to the playback binding.
  }
}
