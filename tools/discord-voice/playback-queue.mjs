// Private host PCM queue. No bot/player, sockets, timers or provider calls.
const UUID = /^(?!00000000-0000-0000-0000-000000000000$)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const uuidValid = value => typeof value === 'string' && UUID.test(value);
const positive = n => Number.isSafeInteger(n) && n > 0;
const targetValid = t => keys(t, ['uuid', 'generation']) && uuidValid(t.uuid)
  && Number.isSafeInteger(t.generation) && t.generation >= 0;
const sameTarget = (a, b) => a?.uuid?.toLowerCase() === b?.uuid?.toLowerCase() && a?.generation === b?.generation;
const keys = (value, expected) => value && typeof value === 'object' && !Array.isArray(value)
  && Object.keys(value).length === expected.length && expected.every(k => Object.hasOwn(value, k));

const pcmValid = value => {
  if (!Array.isArray(value) || value.length !== 1920) return false;
  for (let i = 0; i < value.length; i++) {
    if (!Number.isInteger(value[i]) || value[i] < -32768 || value[i] > 32767) return false;
  }
  return true;
};

export class PlaybackQueue {
  #session; #generation; #sequence; #target; #turn; #muted = false;
  #stopped = false; #stopReason; #lastTime = 0; #outcome; #retired = [];
  constructor(handshake) {
    if (handshake?.api_version !== 1 || handshake.type !== 'host' || !uuidValid(handshake.session_id)
      || !positive(handshake.process_generation) || !positive(handshake.sequence)
      || handshake.message?.type !== 'handshake' || !Array.isArray(handshake.message.capabilities)
      || !handshake.message.capabilities.includes('audio.playback')) throw new Error('Invalid audio playback handshake');
    this.#session = handshake.session_id.toLowerCase(); this.#generation = handshake.process_generation; this.#sequence = handshake.sequence;
  }
  get stopped() { return this.#stopped; }
  get stopReason() { return this.#stopReason; }
  get active() { return Boolean(this.#turn); }
  get bufferedFrames() { return this.#turn?.queue.length ?? 0; }
  takeOutcome() { const value = this.#outcome; this.#outcome = undefined; return value; }
  #clock(now) {
    if (!Number.isFinite(now) || now < this.#lastTime) { this.stop('invalid_clock'); return false; }
    this.#lastTime = now;
    return !this.#stopped;
  }
  #end(outcome, reason) {
    if (!this.#turn) return;
    this.#outcome = { playback_id: this.#turn.id, target: { ...this.#turn.target }, outcome, reason };
    this.#turn = undefined;
  }
  cancel(reason = 'cancelled') { this.#end('cancelled', reason); }
  stop(reason = 'stopped') { this.#end('cancelled', reason); this.#stopReason ??= reason; this.#stopped = true; }
  setMuted(value, now) {
    if (!this.#clock(now)) return;
    this.#muted = Boolean(value);
    if (this.#muted) this.#end('cancelled', 'muted');
  }
  setTarget(target, now) {
    if (!this.#clock(now)) return false;
    if (target != null && !targetValid(target)) { this.stop('invalid_target'); return false; }
    if (!sameTarget(this.#target, target)) this.#end('cancelled', 'target_changed');
    this.#target = target == null ? undefined : { ...target, uuid: target.uuid.toLowerCase() };
    return true;
  }
  host(record, now) {
    if (!this.#clock(now)) return false;
    this.tick(now);
    if (this.#stopped) return false;
    if (record?.api_version !== 1 || record.type !== 'host' || (!uuidValid(record.session_id) || record.session_id.toLowerCase() !== this.#session)
      || record.process_generation !== this.#generation || !positive(record.sequence) || record.sequence <= this.#sequence) {
      this.stop('invalid_host_identity'); return false;
    }
    this.#sequence = record.sequence;
    if (record.message?.type === 'handshake') { this.stop('duplicate_handshake'); return false; }
    if (record.message?.type === 'event' && record.message.event?.type === 'app.stopping') {
      if (!keys(record.message, ['type', 'event']) || !keys(record.message.event, ['type', 'app_run_id'])
        || !uuidValid(record.message.event.app_run_id)) { this.stop('invalid_shutdown'); return false; }
      this.stop('host_stopping'); return true;
    }
    if (record.message?.type === 'event' && record.message.event?.type === 'target.changed') {
      if (!keys(record.message, ['type', 'event']) || !keys(record.message.event, ['type', 'target'])
        || (record.message.event.target !== null && !targetValid(record.message.event.target))) {
        this.stop('invalid_target'); return false;
      }
      return this.setTarget(record.message.event.target, now);
    }
    if (record.message?.type !== 'audio.output') return true;
    if (!keys(record.message, ['type', 'output'])) { this.stop('invalid_output'); return false; }
    const output = record.message.output;
    const common = ['phase', 'playback_id', 'target'];
    const extra = output?.phase === 'frame' ? ['frame_sequence', 'pcm'] : output?.phase === 'finish' ? ['frame_count'] : [];
    if (!['start', 'frame', 'finish', 'cancel'].includes(output?.phase)
      || !keys(output, [...common, ...extra]) || !uuidValid(output.playback_id) || !targetValid(output.target)) {
      this.stop('invalid_output'); return false;
    }
    if (output.phase === 'frame' && (!Number.isSafeInteger(output.frame_sequence) || output.frame_sequence < 0 || output.frame_sequence >= 65536
      || !pcmValid(output.pcm))) {
      this.stop('invalid_frame'); return false;
    }
    if (output.phase === 'finish' && (!Number.isSafeInteger(output.frame_count) || output.frame_count < 0 || output.frame_count > 65536)) {
      this.stop('invalid_finish'); return false;
    }
    // Late records for a replaced target are discarded without affecting a new turn.
    if (!sameTarget(this.#target, output.target) || this.#muted) return false;
    const playbackId = output.playback_id.toLowerCase();
    if (output.phase === 'start') {
      if (this.#turn) { this.stop('overlapping_playback'); return false; }
      if (this.#outcome || this.#retired.includes(playbackId)) return false;
      this.#retired.push(playbackId);
      if (this.#retired.length > 64) this.#retired.shift();
      this.#turn = { id: playbackId, target: { ...this.#target }, queue: [], next: 0,
        started: now, progress: now, ending: false };
      return true;
    }
    const turn = this.#turn;
    if (!turn || turn.id !== playbackId) return false;
    if (output.phase === 'cancel') { this.#end('cancelled', 'host_cancelled'); return true; }
    if (turn.ending) { this.stop('record_after_finish'); return false; }
    if (output.phase === 'finish') {
      if (output.frame_count !== turn.next) { this.stop('invalid_frame_count'); return false; }
      turn.ending = true; turn.progress = now;
      if (!turn.queue.length) this.#end('drained', 'queue_drained');
      return true;
    }
    if (output.frame_sequence !== turn.next) { this.stop('invalid_frame_sequence'); return false; }
    if (turn.queue.length >= 32) { this.stop('playback_overflow'); return false; }
    const pcm = Buffer.allocUnsafe(3840);
    for (let i = 0; i < output.pcm.length; i++) pcm.writeInt16LE(output.pcm[i], i * 2);
    turn.queue.push(pcm); turn.next++; turn.progress = now;
    return true;
  }
  // One complete frame per pull. This is consumption by an adapter, NOT proof
  // of remote delivery or player/device playback; SDK Idle needs its own receipt.
  pull(now) {
    if (!this.#clock(now)) return undefined;
    this.tick(now);
    const turn = this.#turn;
    if (!turn || this.#stopped) return undefined;
    const frame = turn.queue.shift();
    if (frame) turn.progress = now;
    if (turn.ending && !turn.queue.length) this.#end('drained', 'queue_drained');
    return frame;
  }
  tick(now) {
    if (!this.#clock(now)) return;
    if (this.#turn && now - this.#turn.started >= 22 * 60_000) this.stop('playback_timeout');
    else if (this.#turn && now - this.#turn.progress >= 5000) {
      if (this.#turn.queue.length) this.stop('playback_stalled');
      else this.cancel('source_stalled');
    }
  }
}
