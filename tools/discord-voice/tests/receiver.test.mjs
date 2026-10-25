import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { setImmediate as nextTurn, setTimeout as delay } from 'node:timers/promises';
import { AudioReceiveStream, EndBehaviorType, VoiceConnectionStatus } from '@discordjs/voice';
import { VoiceCodec, CALL_PCM_BYTES } from '../codec.mjs';
import { ReceiveBridge } from '../receive-bridge.mjs';
import { ReceiverBinding } from '../receiver.mjs';

const session = '11111111-1111-4111-8111-111111111111';
const target = { uuid: '22222222-2222-4222-8222-222222222222', generation: 1 };
function fixture({ realStream = false } = {}) {
  const sent = [], streams = [], options = [];
  let hostSequence = 1, now = 0;
  const bridge = new ReceiveBridge({ api_version: 1, type: 'host', session_id: session,
    sequence: 1, process_generation: 1, message: { type: 'handshake', capabilities:
      ['lifecycle.observe', 'message.deliver', 'audio.transcribe'] } },
    { botUserId: '999', emit: record => sent.push(record) });
  const connection = new EventEmitter();
  connection.on('error', () => {}); // Connection owner retains error handling.
  connection.state = { status: VoiceConnectionStatus.Ready };
  connection.receiver = { speaking: new EventEmitter(), subscriptions: new Map(), subscribe(id, opts) {
    options.push(opts);
    const stream = realStream ? new AudioReceiveStream(opts) : new PassThrough({ objectMode: true });
    streams.push(stream); this.subscriptions.set(id, stream);
    stream.once('close', () => this.subscriptions.delete(id));
    return stream;
  } };
  const record = message => ({ api_version: 1, type: 'host', session_id: session,
    process_generation: 1, sequence: ++hostSequence, message });
  bridge.host(record({ type: 'event', event: { type: 'target.changed', target } }), now);
  const binding = new ReceiverBinding(connection, bridge, { silenceMs: 250, now: () => now,
    lookupParticipant: id => id === 'unresolved' ? undefined : { id, name: 'Speaker', bot: id === '888' } });
  const ack = () => {
    const action = sent.shift(); assert.ok(action);
    binding.host(record({ type: 'result', id: action.id, target: action.target,
      result: { status: 'ok', value: { type: 'accepted' } } }));
    return action;
  };
  return { bridge, binding, connection, sent, streams, options, ack,
    event: event => binding.host(record({ type: 'event', event })),
    setNow: value => { now = value; } };
}
function packet() {
  const codec = new VoiceCodec();
  try { return codec.encode(Buffer.alloc(CALL_PCM_BYTES)); } finally { codec.close(); }
}

test('receiver subscribes only to an admitted cached human participant', async () => {
  const f = fixture();
  try {
    for (const id of ['888', '999', 'unresolved']) f.connection.receiver.speaking.emit('start', id);
    assert.equal(f.streams.length, 0);
    f.connection.receiver.speaking.emit('start', '123');
    f.connection.receiver.speaking.emit('start', '124');
    assert.equal(f.streams.length, 1);
    assert.equal(f.options[0].end.behavior, EndBehaviorType.AfterSilence);
    assert.equal(f.options[0].end.duration, 250);
    assert.equal(f.options[0].highWaterMark, 8);
    f.ack(); f.streams[0].push(packet()); await nextTurn();
    assert.equal(f.sent[0].action.input.phase, 'frame');
    assert.equal(f.sent[0].action.input.pcm.length, 320);
  } finally { f.binding.close(); }
});

test('library silence endpoint ends a real receive stream and produces Finish', async () => {
  const f = fixture({ realStream: true });
  try {
    f.connection.receiver.speaking.emit('start', '123'); f.ack();
    f.streams[0].push(packet()); await nextTurn(); f.ack();
    const deadline = performance.now() + 1500;
    while (!f.sent.length) { assert.ok(performance.now() < deadline); await delay(5); }
    assert.equal(f.sent[0].action.input.phase, 'finish');
    assert.equal(f.binding.subscribed, false);
    f.ack();
  } finally { f.binding.close(); }
});

test('disconnect cancels and detaches owned listeners and subscriptions', async () => {
  const f = fixture(); f.connection.receiver.speaking.emit('start', '123'); f.ack();
  f.connection.emit('stateChange', f.connection.state, { status: VoiceConnectionStatus.Disconnected });
  assert.equal(f.binding.closed, true);
  assert.equal(f.streams[0].destroyed, true);
  assert.equal(f.connection.receiver.speaking.listenerCount('start'), 0);
  assert.equal(f.connection.listenerCount('stateChange'), 0);
  assert.equal(f.sent[0].action.input.phase, 'cancel');
  f.ack(); await nextTurn();
  assert.equal(f.connection.receiver.subscriptions.size, 0);
  f.connection.receiver.speaking.emit('start', '124');
  assert.equal(f.streams.length, 1);
});

test('user mute remains active when output-speaking suppression clears', () => {
  const f = fixture();
  try {
    f.binding.setMuted(true); f.binding.setOutputSpeaking(true); f.binding.setOutputSpeaking(false);
    f.connection.receiver.speaking.emit('start', '123'); assert.equal(f.streams.length, 0);
    f.binding.setMuted(false); f.connection.receiver.speaking.emit('start', '123');
    assert.equal(f.streams.length, 1); f.ack();
    f.binding.setOutputSpeaking(true);
    assert.equal(f.streams[0].destroyed, true);
    assert.equal(f.sent[0].action.input.phase, 'cancel'); f.ack();
  } finally { f.binding.close(); }
});

test('target changes destroy the subscription and do not consume late packets', () => {
  const f = fixture();
  try {
    f.connection.receiver.speaking.emit('start', '123'); f.ack();
    f.event({ type: 'target.changed', target: { ...target, generation: 2 } });
    assert.equal(f.streams[0].destroyed, true);
    const before = f.sent.length; f.streams[0].emit('data', packet());
    assert.equal(f.sent.length, before);
    assert.equal(f.bridge.active, false);
  } finally { f.binding.close(); }
});

test('stream errors cancel instead of completing partial audio', async () => {
  const f = fixture();
  try {
    f.connection.receiver.speaking.emit('start', '123'); f.ack();
    f.streams[0].destroy(new Error('synthetic receive failure')); await nextTurn();
    assert.equal(f.sent[0].action.input.phase, 'cancel');
    assert.equal(f.binding.subscribed, false); f.ack();
  } finally { f.binding.close(); }
});

test('foreign subscriptions remain owned by their original consumer', () => {
  const f = fixture(); const foreign = new PassThrough({ objectMode: true });
  f.connection.receiver.subscriptions.set('123', foreign);
  f.connection.receiver.speaking.emit('start', '123');
  f.binding.close(); assert.equal(foreign.destroyed, false);
  assert.equal(f.sent.length, 0); foreign.destroy();
});

test('subscription creation failure cancels the admitted turn', () => {
  const f = fixture();
  try {
    f.connection.receiver.subscribe = () => { throw new Error('synthetic subscribe failure'); };
    f.connection.receiver.speaking.emit('start', '123');
    assert.equal(f.binding.subscribed, false);
    f.ack();
    assert.equal(f.sent[0].action.input.phase, 'cancel'); f.ack();
    assert.equal(f.bridge.active, false);
  } finally { f.binding.close(); }
});

test('idle acknowledgement timeout closes and cleans up the binding', () => {
  const f = fixture(); f.connection.receiver.speaking.emit('start', '123');
  f.setNow(10_000); f.binding.tick();
  assert.equal(f.bridge.stopped, true);
  assert.equal(f.binding.closed, true);
  assert.equal(f.streams[0].destroyed, true);
  assert.equal(f.connection.receiver.speaking.listenerCount('start'), 0);
});

test('reattach requires Ready and accepts retained mute state explicitly', () => {
  const f = fixture(); f.binding.close();
  const options = { lookupParticipant: id => ({ id, name: 'Speaker', bot: false }), now: () => 0, muted: true };
  f.connection.state = { status: VoiceConnectionStatus.Connecting };
  assert.throws(() => new ReceiverBinding(f.connection, f.bridge, options), /Invalid ready/);
  assert.equal(f.connection.receiver.speaking.listenerCount('start'), 0);
  f.connection.state = { status: VoiceConnectionStatus.Ready };
  const binding = new ReceiverBinding(f.connection, f.bridge, options);
  try {
    binding.setOutputSpeaking(false);
    f.connection.receiver.speaking.emit('start', '123');
    assert.equal(f.streams.length, 0);
    binding.setMuted(false);
    f.connection.receiver.speaking.emit('start', '123');
    assert.equal(f.streams.length, 1);
  } finally { binding.close(); }
});
