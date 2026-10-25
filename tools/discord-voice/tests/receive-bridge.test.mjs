import test from 'node:test';
import assert from 'node:assert/strict';
import { ActionSequence } from '../action-sequence.mjs';
import { ReceiveBridge } from '../receive-bridge.mjs';
import { VoiceCodec, CALL_PCM_BYTES } from '../codec.mjs';

const session = '11111111-1111-4111-8111-111111111111';
const target = { uuid: '22222222-2222-4222-8222-222222222222', generation: 1 };
const speaker = { id: '123', name: 'Speaker', bot: false };
function fixture(emit) {
  const sent = [], log = [];
  const bridge = new ReceiveBridge({ api_version: 1, type: 'host', session_id: session,
    process_generation: 1, sequence: 1, message: { type: 'handshake', capabilities:
      ['audio.transcribe', 'message.deliver', 'lifecycle.observe'] } },
    { botUserId: '999', emit: emit ?? (record => { sent.push(record); log.push(structuredClone(record)); }) });
  let sequence = 1;
  const host = (message, now = 0) => bridge.host({ api_version: 1, type: 'host', session_id: session,
    process_generation: 1, sequence: ++sequence, message }, now);
  const event = (event, now = 0) => host({ type: 'event', event }, now);
  const ack = (status = 'ok', now = 0) => {
    const record = sent.shift(); assert.ok(record);
    host({ type: 'result', id: record.id, target: record.target,
      result: status === 'ok' ? { status, value: { type: 'accepted' } } : { status: 'error', code: status } }, now);
    return record;
  };
  event({ type: 'target.changed', target });
  return { bridge, sent, log, event, ack, host };
}
function packet() {
  const codec = new VoiceCodec();
  try { return codec.encode(Buffer.alloc(CALL_PCM_BYTES)); } finally { codec.close(); }
}

test('real Opus turns produce ordered frames and wait for private completion', () => {
  const f = fixture(), opus = packet();
  assert.equal(f.bridge.begin(speaker, 0), true);
  f.ack();
  for (let i = 0; i < 6; i++) assert.equal(f.bridge.packet('123', opus, 0), true);
  assert.equal(f.sent.length, 4);
  assert.equal(f.bridge.end('123', 0), true);
  while (f.sent.length) f.ack();
  assert.equal(f.bridge.active, true);
  const frames = f.log.filter(record => record.action.input.phase === 'frame');
  assert.deepEqual(frames.map(record => record.action.input.frame_sequence), [0, 1, 2, 3, 4, 5]);
  assert.ok(frames.every(record => record.action.input.pcm.length === 320));
  const finish = f.log.at(-1);
  assert.equal(finish.action.input.phase, 'finish');
  f.event({ type: 'audio.input.finished', target, turn_id: finish.action.input.turn_id, outcome: 'queued' });
  assert.equal(f.bridge.active, false);
  assert.equal(f.bridge.takeOutcome().outcome, 'queued');
  assert.equal(f.bridge.takeOutcome(), undefined);
});

test('first speaker owns the turn and bots/self/mute are excluded', () => {
  const f = fixture();
  assert.equal(f.bridge.begin({ ...speaker, bot: true }, 0), false);
  assert.equal(f.bridge.begin({ ...speaker, id: '999' }, 0), false);
  assert.equal(f.bridge.begin(speaker, 0), true);
  assert.equal(f.bridge.begin({ ...speaker, id: '124' }, 0), false);
  assert.equal(f.bridge.packet('124', packet(), 0), false);
  f.ack();
  f.bridge.setMuted(true, 0);
  assert.equal(f.sent[0].action.input.phase, 'cancel');
  f.ack();
  assert.equal(f.bridge.begin(speaker, 0), false);
  f.bridge.setMuted(false, 0);
  assert.equal(f.bridge.begin(speaker, 0), true);
  f.bridge.stop();
});

test('overflow cancels the whole turn without sending a partial Finish', () => {
  const f = fixture(), opus = packet();
  f.bridge.begin(speaker, 0); // Delay Start acknowledgement.
  for (let i = 0; i < 32; i++) assert.equal(f.bridge.packet('123', opus, 0), true);
  assert.equal(f.bridge.bufferedFrames, 32);
  assert.equal(f.bridge.packet('123', opus, 0), false);
  assert.equal(f.bridge.bufferedFrames, 0);
  assert.equal(f.bridge.end('123', 0), false);
  f.ack();
  assert.equal(f.sent[0].action.input.phase, 'cancel');
  f.ack();
  assert.equal(f.bridge.active, false);
  assert.ok(!f.log.some(record => record.action.input.phase === 'finish'));
});

test('uncertain writes and expired acknowledgements stop without replay', () => {
  let attempts = 0;
  const uncertain = fixture(() => { attempts++; throw new Error('synthetic write failure'); });
  uncertain.bridge.begin(speaker, 0);
  uncertain.bridge.tick(1000);
  assert.equal(uncertain.bridge.stopped, true);
  assert.equal(attempts, 1);
  const missing = fixture(); missing.bridge.begin(speaker, 0); missing.bridge.tick(10_000);
  assert.equal(missing.bridge.stopped, true);
  assert.equal(missing.log.length, 1);
});

test('target replacement clears old audio without retargeting it', () => {
  const f = fixture(); f.bridge.begin(speaker, 0); f.ack();
  f.bridge.packet('123', packet(), 0);
  const old = f.sent.shift();
  f.event({ type: 'target.changed', target: { ...target, generation: 2 } });
  assert.equal(f.bridge.active, false);
  f.host({ type: 'result', id: old.id, target: old.target, result: { status: 'ok', value: { type: 'accepted' } } });
  assert.equal(f.bridge.begin(speaker, 0), true);
  assert.equal(f.sent[0].target.generation, 2);
  assert.equal(f.sent[0].action.input.phase, 'start');
  f.bridge.stop();
});

test('refused Finish is cancelled instead of automatically replayed', () => {
  const f = fixture(); f.bridge.begin(speaker, 0); f.ack();
  f.bridge.packet('123', packet(), 0); f.bridge.end('123', 0); f.ack();
  assert.equal(f.sent[0].action.input.phase, 'finish');
  f.ack('busy');
  assert.equal(f.sent[0].action.input.phase, 'cancel');
  f.ack();
  assert.equal(f.log.filter(record => record.action.input.phase === 'finish').length, 1);
  assert.equal(f.bridge.active, false);
});

test('cancelling submitted ASR waits for its correlated completion', () => {
  const f = fixture(); f.bridge.begin(speaker, 0); f.ack();
  f.bridge.packet('123', packet(), 0); f.bridge.end('123', 0); f.ack();
  const finish = f.ack();
  f.bridge.cancel(0); f.ack();
  assert.equal(f.bridge.active, true);
  f.event({ type: 'audio.input.finished', target, turn_id: finish.action.input.turn_id, outcome: 'cancelled' });
  assert.equal(f.bridge.active, false);
  assert.equal(f.bridge.takeOutcome().outcome, 'cancelled');
});

test('malformed audio cancels and revoked host authority stops', () => {
  const f = fixture(); f.bridge.begin(speaker, 0); f.ack();
  assert.equal(f.bridge.packet('123', Buffer.alloc(0), 0), false);
  assert.equal(f.sent[0].action.input.phase, 'cancel'); f.ack();
  f.bridge.begin(speaker, 0); f.ack('permission_denied');
  assert.equal(f.bridge.stopped, true);
});

test('shared producer identities preserve monotonic actions and private ACK matching', () => {
  const actions = new ActionSequence();
  const playback = actions.next('playback');
  const sent = [];
  const bridge = new ReceiveBridge({ api_version: 1, type: 'host', session_id: session,
    process_generation: 1, sequence: 1, message: { type: 'handshake', capabilities:
      ['audio.transcribe', 'message.deliver', 'lifecycle.observe'] } },
    { actions, botUserId: '999', emit: record => sent.push(record) });
  const host = (sequence, message) => bridge.host({ api_version: 1, type: 'host', session_id: session,
    process_generation: 1, sequence, message }, 0);
  host(2, { type: 'event', event: { type: 'target.changed', target } });
  assert.equal(bridge.begin(speaker, 0), true);
  assert.equal(playback.sequence, 1); assert.equal(sent[0].sequence, 2);
  assert.equal(sent[0].id, 'voice-2');
  host(3, { type: 'result', id: playback.id, target, result: { status: 'ok', value: { type: 'accepted' } } });
  assert.equal(bridge.receiving, true); // starting still admits bounded packets.
  bridge.end(speaker.id, 0); assert.equal(sent.length, 1);
  host(4, { type: 'result', id: sent[0].id, target, result: { status: 'ok', value: { type: 'accepted' } } });
  assert.equal(sent[1].action.input.phase, 'cancel');
  assert.equal(sent[1].sequence, 3); bridge.stop();
});


test('receive bridge refuses playback-only authority without emitting input', () => {
  let emitted = 0;
  assert.throws(() => new ReceiveBridge({ api_version: 1, type: 'host', session_id: session,
    process_generation: 1, sequence: 1, message: { type: 'handshake', capabilities: ['audio.playback'] } },
    { botUserId: '999', emit: () => emitted++ }), /Missing receive grants/);
  assert.equal(emitted, 0);
});
