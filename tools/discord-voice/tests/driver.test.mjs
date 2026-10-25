import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { setImmediate as nextTurn } from 'node:timers/promises';
import { isAbsolute, join } from 'node:path';
import { pathToFileURL } from 'node:url';
const bundle = process.env.WAIFU_VOICE_TEST_BUNDLE;
if (bundle) assert.ok(isAbsolute(bundle));
const moduleUrl = name => bundle ? pathToFileURL(join(bundle, `${name}.mjs`)) : new URL(`../${name}.mjs`, import.meta.url);
const { PlaybackBinding } = await import(moduleUrl('playback-binding'));
const { PlaybackAdapter } = await import(moduleUrl('playback'));
const { VoiceDriver } = await import(moduleUrl('driver'));
const { VoiceCodec, CALL_PCM_BYTES } = await import(moduleUrl('codec'));

const sessionId = '11111111-1111-4111-8111-111111111111';
const target = { uuid: '22222222-2222-4222-8222-222222222222', generation: 2 };
function deferred() { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
function connectionSession() {
  let disposed = 0;
  const connection = new EventEmitter(), streams = [];
  connection.on('error', () => {});
  connection.state = { status: 'ready' };
  connection.receiver = { speaking: new EventEmitter(), subscriptions: new Map(), subscribe(id) {
    const stream = new PassThrough({ objectMode: true }); streams.push(stream);
    this.subscriptions.set(id, stream); stream.once('close', () => this.subscriptions.delete(id)); return stream;
  } };
  return { connection, streams, botUserId: '999', lookupParticipant: id => ({ id, name: 'Speaker', bot: false }),
    dispose() { disposed++; connection.state = { status: 'destroyed' }; }, get disposed() { return disposed; } };
}
function fixture(connect, options = {}) {
  const input = new PassThrough(), output = new PassThrough(), stops = [], actions = [];
  let now = 0, sequence = 0;
  output.on('data', bytes => actions.push(JSON.parse(bytes.toString())));
  const driver = new VoiceDriver(input, output, { connect, onStop: reason => stops.push(reason), now: () => now, autoTick: false, ...options });
  const send = message => input.write(`${JSON.stringify({ api_version: 1, type: 'host', session_id: sessionId,
    process_generation: 1, sequence: ++sequence, message })}\n`);
  const handshake = capabilities => send({ type: 'handshake', capabilities: capabilities ?? ['audio.transcribe', 'message.deliver', 'lifecycle.observe'], settings: { guild_id: '111', channel_id: '222' } });
  return { input, output, driver, stops, actions, send, handshake, setNow: value => { now = value; } };
}

test('queued target authority is applied before a ready receiver can capture', async () => {
  const pending = deferred(), session = connectionSession(); let calls = 0;
  const f = fixture(({ settings }) => { calls++; assert.deepEqual(settings, { guild_id: '111', channel_id: '222' }); return pending.promise; });
  try {
    f.handshake(); f.send({ type: 'event', event: { type: 'target.changed', target } });
    await nextTurn(); assert.equal(calls, 1); assert.equal(f.driver.ready, false);
    pending.resolve(session); await nextTurn(); assert.equal(f.driver.ready, true);
    session.connection.receiver.speaking.emit('start', '123');
    assert.equal(f.actions.length, 1); assert.deepEqual(f.actions[0].target, target);
    const start = f.actions[0];
    f.send({ type: 'result', id: start.id, target, result: { status: 'ok', value: { type: 'accepted' } } });
    const codec = new VoiceCodec();
    try { session.streams[0].push(codec.encode(Buffer.alloc(CALL_PCM_BYTES))); } finally { codec.close(); }
    await nextTurn(); assert.equal(f.actions[1].action.input.phase, 'frame');
    assert.equal(f.actions[1].action.input.pcm.length, 320);
  } finally { f.driver.stop(); }
  assert.equal(session.disposed, 1);
});

test('EOF aborts startup and disposes a late connection without capture', async () => {
  const pending = deferred(), session = connectionSession(); let signal;
  const f = fixture(options => { signal = options.signal; return pending.promise; });
  f.handshake(); await nextTurn(); f.input.end(); await nextTurn();
  assert.equal(signal.aborted, true); assert.deepEqual(f.stops, ['eof']);
  pending.resolve(session); await nextTurn();
  assert.equal(session.disposed, 1); assert.equal(f.driver.ready, false); assert.equal(f.driver.pendingStartup, false);
  assert.equal(session.connection.receiver.speaking.listenerCount('start'), 0);
  assert.deepEqual(f.actions, []); f.driver.stop(); assert.equal(session.disposed, 1);
});

test('startup deadline permits one attempt and rejects late success', async () => {
  const pending = deferred(), session = connectionSession(); let calls = 0, signal;
  const f = fixture(options => { calls++; signal = options.signal; return pending.promise; });
  f.handshake(); await nextTurn(); f.setNow(14999); f.driver.tick(); assert.equal(f.driver.stopped, false);
  f.setNow(15000); f.driver.tick(); assert.deepEqual(f.stops, ['startup_timeout']); assert.equal(signal.aborted, true);
  pending.resolve(session); await nextTurn(); assert.equal(calls, 1); assert.equal(session.disposed, 1);
  assert.equal(f.driver.ready, false); assert.deepEqual(f.actions, []);
});

test('host stopping in the handshake batch wins before the connector effect', async () => {
  let calls = 0;
  const f = fixture(() => { calls++; return connectionSession(); });
  f.handshake(); f.send({ type: 'event', event: { type: 'app.stopping' } }); await nextTurn();
  assert.equal(calls, 0); assert.deepEqual(f.stops, ['host_stopping']);
});

test('missing grants prevent connection startup', async () => {
  let calls = 0;
  const f = fixture(() => { calls++; return connectionSession(); });
  f.handshake(['message.deliver']); await nextTurn();
  assert.equal(calls, 0); assert.equal(f.driver.stopped, true); assert.deepEqual(f.actions, []);
});

test('startup metadata queue refuses overflow and disposes the late result', async () => {
  const pending = deferred(), session = connectionSession();
  const f = fixture(() => pending.promise); f.handshake(); await nextTurn();
  for (let i = 0; i < 17; i++) f.send({ type: 'event', event: { type: 'target.changed', target: { ...target, generation: i + 1 } } });
  assert.deepEqual(f.stops, ['startup_overflow']);
  pending.resolve(session); await nextTurn(); assert.equal(session.disposed, 1); assert.deepEqual(f.actions, []);
});

test('connector failures reveal fixed status without raw credential errors', async () => {
  const f = fixture(async () => { throw new Error('private synthetic credential failure'); });
  f.handshake(); await nextTurn(); assert.deepEqual(f.stops, ['startup_failed']); assert.deepEqual(f.actions, []);
  assert.equal(f.driver.pendingStartup, false);
});

test('disconnect stops the driver and disposes the owned connection once', async () => {
  const session = connectionSession(), f = fixture(async () => session);
  f.handshake(); await nextTurn(); assert.equal(f.driver.ready, true);
  session.connection.emit('stateChange', session.connection.state, { status: 'disconnected' });
  f.driver.tick(); assert.deepEqual(f.stops, ['receiver_stopped']); assert.equal(session.disposed, 1);
  f.driver.stop(); assert.equal(session.disposed, 1);
});

test('cleanup rejection is observed and retained as a failed-cleanup flag', async () => {
  const session = connectionSession(); session.dispose = async () => { throw new Error('private cleanup failure'); };
  const f = fixture(async () => session); f.handshake(); await nextTurn();
  f.driver.stop(); await nextTurn(); assert.equal(f.driver.cleanupFailed, true);
  assert.deepEqual(f.stops, ['stopped']);
});

test('a delayed startup effect cannot begin after its deadline without a tick', async () => {
  let calls = 0;
  const f = fixture(() => { calls++; return connectionSession(); });
  f.handshake(); f.setNow(15000); await nextTurn();
  assert.equal(calls, 0); assert.deepEqual(f.stops, ['startup_timeout']);
});

test('late success is rejected even when the periodic tick has not run', async () => {
  const pending = deferred(), session = connectionSession();
  const f = fixture(() => pending.promise); f.handshake(); await nextTurn();
  f.setNow(15000); pending.resolve(session); await nextTurn();
  assert.deepEqual(f.stops, ['startup_timeout']); assert.equal(session.disposed, 1); assert.equal(f.driver.ready, false);
});

test('startup byte accounting stops before the record-count limit', async () => {
  const pending = deferred(), session = connectionSession();
  const f = fixture(() => pending.promise); f.handshake(); await nextTurn();
  for (let i = 0; i < 9; i++) f.send({ type: 'event', event: { type: 'event.emitted', data: 'x'.repeat(8000) } });
  assert.deepEqual(f.stops, ['startup_overflow']);
  pending.resolve(session); await nextTurn(); assert.equal(session.disposed, 1);
});

test('shutdown readiness waits for the owned disposal promise', async () => {
  const cleanup = deferred(), session = connectionSession(); session.dispose = () => cleanup.promise;
  const f = fixture(async () => session); f.handshake(); await nextTurn();
  f.driver.stop(); assert.equal(f.driver.shutdownReady, false);
  cleanup.resolve(); await nextTurn(); assert.equal(f.driver.shutdownReady, true);
});

const playbackId = '33333333-3333-4333-8333-333333333333';
const duplexCaps = ['audio.transcribe', 'message.deliver', 'lifecycle.observe', 'audio.playback'];
function duplexFixture(connect, capabilities = duplexCaps) {
  const session = connectionSession(), player = new EventEmitter();
  let subscriptions = 0, unsubscribed = 0;
  session.connection.subscribe = () => {
    subscriptions++;
    const subscription = { unsubscribe() { unsubscribed++; if (session.connection.state.subscription === subscription) delete session.connection.state.subscription; } };
    session.connection.state.subscription = subscription; return subscription;
  };
  player.state = { status: 'idle' };
  const idle = () => { const old = player.state; player.state = { status: 'idle' }; player.emit('stateChange', old, player.state); };
  player.play = resource => { player.state = { status: 'playing', resource }; };
  player.stop = () => idle();
  const f = fixture(() => connect ? connect(session) : session, { createPlayback: (handshake, connection, options) => new PlaybackBinding(handshake, connection,
    { ...options, adapterFactory: config => new PlaybackAdapter({ ...config, player,
      resourceFactory: stream => ({ playStream: stream }), codecFactory: () => ({ encode: () => Buffer.from([1]), close() {} }) }) }) });
  const output = (phase, fields = {}) => f.send({ type: 'audio.output', output: { phase, playback_id: playbackId, target, ...fields } });
  const ack = action => f.send({ type: 'result', id: action.id, target: action.target, result: { status: 'ok', value: { type: 'accepted' } } });
  const start = async () => { f.handshake(capabilities); f.send({ type: 'event', event: { type: 'target.changed', target } }); await nextTurn(); assert.equal(f.driver.ready, true); assert.equal(f.driver.playbackReady, false); assert.equal(f.actions.at(-1).action.type, 'audio.playback.available'); ack(f.actions.at(-1)); assert.equal(f.driver.playbackReady, true); f.actions.length = 0; };
  return { ...f, session, player, idle, output, ack, start, subscriptions: () => subscriptions, unsubscribed: () => unsubscribed };
}
test('optional playback grant selects the optional output binding; receive-only never constructs it', async () => {
  let calls = 0; const session = connectionSession();
  const f = fixture(() => session, { createPlayback: () => { calls++; throw new Error('must not create'); } });
  f.handshake(); f.send({ type: 'event', event: { type: 'target.changed', target } }); await nextTurn();
  f.send({ type: 'audio.output', output: { phase: 'start', playback_id: playbackId, target } });
  assert.equal(calls, 0); assert.equal(f.driver.ready, true); assert.equal(f.actions.length, 0); f.driver.stop();
});
test('voice and playback actions share one counter and each ACK reaches its owner', async () => {
  const f = duplexFixture(); await f.start();
  f.session.connection.receiver.speaking.emit('start', '123'); const voice = f.actions[0];
  assert.equal(voice.id, 'voice-2'); f.output('start');
  const ready = f.actions.find(a => a.action.type === 'audio.playback.receipt'); assert.equal(ready.id, 'playback-3');
  f.ack(voice); const cancel = f.actions.find(a => a.action.type === 'audio.input' && a.action.input.phase === 'cancel');
  assert.equal(cancel.id, 'voice-4'); f.ack(cancel); f.ack(ready);
  f.output('frame', { frame_sequence: 0, pcm: Array(1920).fill(0) }); f.output('finish', { frame_count: 1 });
  f.player.state.resource.playStream.read(0); await nextTurn();
  const consumed = f.actions.at(-1); assert.equal(consumed.action.receipt.phase, 'consumed'); assert.equal(consumed.id, 'playback-5');
  f.idle(); assert.equal(f.actions.at(-1), consumed); f.ack(consumed);
  const finished = f.actions.at(-1); assert.equal(finished.action.receipt.phase, 'finished'); assert.equal(finished.action.receipt.outcome, 'completed');
  f.ack(finished); assert.equal(f.driver.ready, true); f.driver.stop();
  assert.equal(f.unsubscribed(), 1); assert.equal(f.session.disposed, 1);
});
test('output mute is retained before startup and does not mute participant input', async () => {
  const f = duplexFixture(); f.driver.setOutputMuted(true); await f.start(); f.output('start');
  assert.equal(f.actions.length, 0); f.session.connection.receiver.speaking.emit('start', '123');
  assert.equal(f.actions[0].action.type, 'audio.input'); f.driver.stop();
});
test('manual input mute survives completion of automatic output suppression', async () => {
  const f = duplexFixture(); await f.start(); f.driver.setMuted(true);
  f.output('start'); f.ack(f.actions.at(-1)); f.output('frame', { frame_sequence: 0, pcm: Array(1920).fill(0) });
  f.output('finish', { frame_count: 1 }); f.player.state.resource.playStream.read(0); await nextTurn(); f.idle();
  f.session.connection.receiver.speaking.emit('start', '123'); assert.equal(f.session.streams.length, 0);
  f.driver.setMuted(false); f.session.connection.receiver.speaking.emit('start', '123'); assert.equal(f.session.streams.length, 1); f.driver.stop();
});
test('external output suppression survives completion of automatic playback', async () => {
  const f = duplexFixture(); await f.start(); f.driver.setOutputSpeaking(true);
  f.output('start'); f.ack(f.actions.at(-1)); f.output('frame', { frame_sequence: 0, pcm: Array(1920).fill(0) });
  f.output('finish', { frame_count: 1 }); f.player.state.resource.playStream.read(0); await nextTurn(); f.idle();
  f.session.connection.receiver.speaking.emit('start', '123'); assert.equal(f.session.streams.length, 0);
  f.driver.setOutputSpeaking(false); f.session.connection.receiver.speaking.emit('start', '123'); assert.equal(f.session.streams.length, 1); f.driver.stop();
});
test('missing output ACK stops worker once and disposes owned connection', async () => {
  const f = duplexFixture(); await f.start(); f.output('start'); await nextTurn(); f.setNow(10000); f.driver.tick();
  assert.equal(f.driver.stopped, true); assert.deepEqual(f.stops, ['playback_stopped']);
  assert.equal(f.session.disposed, 1); assert.equal(f.unsubscribed(), 1); f.driver.stop(); assert.equal(f.unsubscribed(), 1);
});
test('EOF while speaking stops output, freezes feedback and closes once', async () => {
  const f = duplexFixture(); await f.start(); f.output('start'); const count = f.actions.length;
  f.input.end(); await nextTurn(); assert.equal(f.driver.stopped, true); assert.equal(f.session.disposed, 1);
  assert.equal(f.unsubscribed(), 1); assert.equal(f.actions.length, count); f.idle(); assert.equal(f.actions.length, count);
});
test('retarget during output discards old ACKs and allows playback on the new target', async () => {
  const f = duplexFixture(); await f.start(); f.output('start'); const oldReady = f.actions.at(-1);
  const next = { ...target, generation: 3 };
  f.send({ type: 'event', event: { type: 'target.changed', target: next } }); const available = f.actions.at(-1); f.ack(oldReady); f.ack(available);
  assert.equal(f.driver.ready, true);
  f.output('start', { playback_id: '44444444-4444-4444-8444-444444444444', target: next });
  assert.deepEqual(f.actions.at(-1).target, next); assert.equal(f.driver.ready, true); f.driver.stop();
});
test('next playback waits for prior terminal ACK and then starts on the same connection', async () => {
  const f = duplexFixture(); await f.start(); f.output('start'); f.ack(f.actions.at(-1));
  f.output('frame', { frame_sequence: 0, pcm: Array(1920).fill(0) }); f.output('finish', { frame_count: 1 });
  f.player.state.resource.playStream.read(0); await nextTurn(); const consumed = f.actions.at(-1);
  f.idle(); f.ack(consumed); const finished = f.actions.at(-1); f.ack(finished);
  f.output('start', { playback_id: '44444444-4444-4444-8444-444444444444' });
  assert.equal(f.actions.at(-1).action.receipt.phase, 'ready'); assert.equal(f.driver.ready, true);
  assert.equal(f.subscriptions(), 1); f.driver.stop();
});
test('a new turn before terminal ACK fails closed and never produces a second Ready', async () => {
  const f = duplexFixture(); await f.start(); f.output('start'); f.ack(f.actions.at(-1));
  f.output('frame', { frame_sequence: 0, pcm: Array(1920).fill(0) }); f.output('finish', { frame_count: 1 });
  f.player.state.resource.playStream.read(0); await nextTurn(); f.idle(); f.ack(f.actions.at(-1));
  f.output('start', { playback_id: '44444444-4444-4444-8444-444444444444' });
  assert.equal(f.driver.stopped, true); assert.equal(f.actions.filter(a => a.action.receipt?.phase === 'ready').length, 1);
  assert.equal(f.session.disposed, 1); assert.equal(f.unsubscribed(), 1);
});
test('generation zero is accepted by both input and output target routing', async () => {
  const f = duplexFixture(); await f.start(); const zero = { ...target, generation: 0 };
  f.send({ type: 'event', event: { type: 'target.changed', target: zero } }); f.ack(f.actions.at(-1));
  f.session.connection.receiver.speaking.emit('start', '123'); assert.deepEqual(f.actions.at(-1).target, zero);
  f.output('start', { target: zero }); assert.equal(f.driver.ready, true); assert.deepEqual(f.actions.at(-1).target, zero); f.driver.stop();
});
test('foreign SDK subscription is never replaced during startup failure', async () => {
  const f = duplexFixture(); let touched = 0;
  f.session.connection.state.subscription = { unsubscribe() { touched++; } };
  f.handshake(duplexCaps); f.send({ type: 'event', event: { type: 'target.changed', target } }); await nextTurn();
  assert.equal(f.driver.stopped, true); assert.equal(f.session.disposed, 1); assert.equal(touched, 0);
  assert.equal(f.subscriptions(), 0); assert.equal(f.unsubscribed(), 0);
});
test('receiver suppression lasts through buffered output until actual player Idle', async () => {
  const f = duplexFixture(); await f.start(); f.output('start'); f.ack(f.actions.at(-1));
  f.output('frame', { frame_sequence: 0, pcm: Array(1920).fill(0) }); f.output('finish', { frame_count: 1 });
  f.player.state.resource.playStream.read(0); await nextTurn();
  f.session.connection.receiver.speaking.emit('start', '123'); assert.equal(f.session.streams.length, 0);
  f.idle(); f.session.connection.receiver.speaking.emit('start', '123'); assert.equal(f.session.streams.length, 1); f.driver.stop();
});

test('queued output waits for the connection and replays target authority before Ready', async () => {
  const pending = deferred(), f = duplexFixture(() => pending.promise);
  f.handshake(duplexCaps); f.send({ type: 'event', event: { type: 'target.changed', target } });
  await nextTurn(); assert.equal(f.driver.ready, false); assert.equal(f.actions.length, 0);
  pending.resolve(f.session); await nextTurn(); assert.equal(f.driver.ready, true);
  assert.equal(f.actions.length, 1); assert.equal(f.actions[0].action.type, 'audio.playback.available');
  f.ack(f.actions[0]); f.output('start'); assert.equal(f.actions.at(-1).action.receipt.phase, 'ready');
  assert.deepEqual(f.actions[0].target, target); f.driver.stop(); assert.equal(f.unsubscribed(), 1);
});
test('async player failure is detected on tick and disposes once without raw error output', async () => {
  const f = duplexFixture(); await f.start(); f.output('start'); f.ack(f.actions.at(-1));
  f.player.emit('error', new Error('private fake SDK failure'));
  assert.equal(f.driver.ready, false); f.driver.tick();
  assert.equal(f.driver.stopped, true); assert.deepEqual(f.stops, ['playback_stopped']);
  assert.equal(f.session.disposed, 1); assert.equal(f.unsubscribed(), 1);
  assert.ok(!JSON.stringify(f.actions).includes('private fake SDK failure')); f.driver.stop();
});
test('host source starvation cancels output without disconnecting a healthy worker', async () => {
  const f = duplexFixture(); await f.start(); f.output('start'); f.ack(f.actions.at(-1)); await nextTurn();
  f.setNow(5000); f.driver.tick();
  const cancelled = f.actions.at(-1); assert.equal(cancelled.action.receipt.phase, 'finished');
  assert.equal(cancelled.action.receipt.outcome, 'cancelled'); assert.equal(f.driver.ready, true); assert.equal(f.session.disposed, 0);
  f.ack(cancelled); f.output('start', { playback_id: '44444444-4444-4444-8444-444444444444' });
  assert.equal(f.actions.at(-1).action.receipt.phase, 'ready'); assert.equal(f.driver.ready, true); f.driver.stop();
});
test('stalled player with buffered PCM fails and disposes the worker', async () => {
  const f = duplexFixture(); await f.start(); f.output('start'); f.ack(f.actions.at(-1));
  f.output('frame', { frame_sequence: 0, pcm: Array(1920).fill(0) }); await nextTurn();
  f.setNow(5000); f.driver.tick(); assert.equal(f.driver.stopped, true); assert.equal(f.session.disposed, 1);
  const receipt = f.actions.at(-1).action.receipt; assert.equal(receipt.phase, 'finished'); assert.equal(receipt.outcome, 'failed');
  assert.equal(f.unsubscribed(), 1);
});
test('speech Start before availability ACK fails without producing PCM feedback', async () => {
  const f = duplexFixture(); f.handshake(duplexCaps); f.send({ type: 'event', event: { type: 'target.changed', target } }); await nextTurn();
  assert.equal(f.driver.playbackReady, false); f.output('start'); assert.equal(f.driver.stopped, true);
  assert.equal(f.actions.filter(a => a.action.type === 'audio.playback.receipt').length, 0);
  assert.equal(f.session.disposed, 1);
});
test('availability refusal and missing ACK both stop the pending output binding', async () => {
  for (const timeout of [false, true]) {
    const f = duplexFixture(); f.handshake(duplexCaps); f.send({ type: 'event', event: { type: 'target.changed', target } }); await nextTurn();
    const available = f.actions.at(-1); assert.equal(available.action.type, 'audio.playback.available');
    if (timeout) { await nextTurn(); f.setNow(10000); f.driver.tick(); }
    else f.send({ type: 'result', id: available.id, target, result: { status: 'error', code: 'permission_denied' } });
    assert.equal(f.driver.stopped, true); assert.equal(f.driver.playbackReady, false); assert.equal(f.session.disposed, 1);
  }
});
test('readiness is target-specific and duplicate target events do not revoke it', async () => {
  const f = duplexFixture(); f.handshake(duplexCaps); f.send({ type: 'event', event: { type: 'target.changed', target } }); await nextTurn();
  const old = f.actions.at(-1); f.ack(old); assert.equal(f.driver.playbackReady, true);
  const next = { ...target, generation: 4 };
  f.send({ type: 'event', event: { type: 'target.changed', target: next } }); const fresh = f.actions.at(-1);
  assert.equal(f.driver.playbackReady, false); f.ack(old); assert.equal(f.driver.playbackReady, false);
  f.ack(fresh); assert.equal(f.driver.playbackReady, true); const count = f.actions.length;
  f.send({ type: 'event', event: { type: 'target.changed', target: next } });
  assert.equal(f.actions.length, count); assert.equal(f.driver.playbackReady, true);
  f.send({ type: 'event', event: { type: 'target.changed', target: null } });
  assert.equal(f.driver.playbackReady, false); assert.equal(f.driver.ready, true); f.driver.stop();
});


test('playback-only grant completes media without constructing receive resources', async () => {
  const f = duplexFixture(undefined, ['audio.playback']);
  const receiver = f.session.connection.receiver;
  delete f.session.connection.receiver;
  delete f.session.botUserId;
  delete f.session.lookupParticipant;
  try {
    await f.start();
    assert.equal(receiver.speaking.listenerCount('start'), 0);
    f.driver.setMuted(true); f.driver.setOutputSpeaking(true);
    f.output('start'); f.ack(f.actions.at(-1));
    f.output('frame', { frame_sequence: 0, pcm: Array(1920).fill(0) });
    f.output('finish', { frame_count: 1 });
    f.player.state.resource.playStream.read(0); await nextTurn();
    const consumed = f.actions.at(-1); assert.equal(consumed.action.receipt.phase, 'consumed');
    f.idle(); f.ack(consumed);
    assert.equal(f.actions.at(-1).action.receipt.outcome, 'completed'); f.ack(f.actions.at(-1));
    assert.ok(f.actions.every(action => action.action.type.startsWith('audio.playback.')));
    assert.equal(f.driver.ready, true);
  } finally { f.driver.stop(); }
  assert.equal(f.session.disposed, 1); assert.equal(f.unsubscribed(), 1);
});

test('driver passes immutable directions derived only from host grants', async () => {
  for (const [caps, expected] of [
    [['audio.playback'], { receive: false, playback: true }],
    [['audio.transcribe', 'message.deliver', 'lifecycle.observe'], { receive: true, playback: false }],
    [duplexCaps, { receive: true, playback: true }],
  ]) {
    let observed;
    const f = fixture(options => { observed = options.directions; throw new Error('fixture stop'); });
    f.handshake(caps); await nextTurn();
    assert.deepEqual(observed, expected); assert.ok(Object.isFrozen(observed));
    f.driver.stop();
  }
});

test('playback-only rejects malformed envelopes before output can act', async () => {
  for (const invalid of [{ api_version: 2, type: 'host' }, { api_version: 1, type: 'action' }]) {
    const f = duplexFixture(undefined, ['audio.playback']);
    try {
      await f.start();
      f.input.write(`${JSON.stringify({ ...invalid, session_id: sessionId,
        process_generation: 1, sequence: 999,
        message: { type: 'audio.output', output: { phase: 'start', playback_id: playbackId, target } } })}\n`);
      assert.equal(f.driver.stopped, true);
      assert.deepEqual(f.stops, ['invalid_input']);
      assert.deepEqual(f.actions, []);
      assert.equal(f.session.disposed, 1);
    } finally { f.driver.stop(); }
  }
});
