import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PlaybackQueue } from '../playback-queue.mjs';
import { PlaybackAdapter } from '../playback.mjs';
const session = '11111111-1111-4111-8111-111111111111';
const id = '22222222-2222-4222-8222-222222222222';
const target = { uuid: '33333333-3333-4333-8333-333333333333', generation: 1 };
function fixture(options = {}) {
  let time = 0;
  const queue = new PlaybackQueue({ api_version: 1, type: 'host', sequence: 1, session_id: session,
    process_generation: 1, message: { type: 'handshake', capabilities: ['audio.playback'] } });
  queue.setTarget(target, 0);
  const connection = new EventEmitter(); connection.state = { status: 'ready' };
  let unsubscribed = 0, stopped = 0, encoded = 0, closed = 0;
  connection.subscribe = () => {
    const subscription = { unsubscribe() { unsubscribed++; if (connection.state.subscription === subscription) delete connection.state.subscription; } };
    connection.state.subscription = subscription; return subscription;
  };
  const player = new EventEmitter(); player.state = { status: 'idle' };
  player.play = resource => { player.state = { status: 'playing', resource }; };
  const idle = () => { const old = player.state; player.state = { status: 'idle' }; player.emit('stateChange', old, player.state); };
  player.stop = () => { stopped++; idle(); };
  const speaking = [];
  const adapter = new PlaybackAdapter({ queue, connection, player, now: () => time,
    setOutputSpeaking: value => speaking.push(value),
    resourceFactory: stream => ({ playStream: stream }),
    codecFactory: () => ({ encode(pcm) { assert.equal(pcm.length, 3840); encoded++; return Buffer.from([1]); }, close() { closed++; } }), ...options });
  let sequence = 1;
  const send = (phase, fields = {}) => adapter.host({ api_version: 1, type: 'host', sequence: ++sequence,
    session_id: session, process_generation: 1,
    message: { type: 'audio.output', output: { phase, playback_id: id, target, ...fields } } });
  return { adapter, queue, connection, player, idle, speaking, send, setTime: value => { time = value; },
    counts: () => ({ unsubscribed, stopped, encoded, closed }) };
}
const frame = i => ({ frame_sequence: i, pcm: Array(1920).fill(0) });
test('drained queue keeps suppression until actual player Idle', async () => {
  const f = fixture(); f.send('start'); f.send('frame', frame(0)); f.send('finish', { frame_count: 1 });
  const stream = f.player.state.resource.playStream;
  stream.read(0); await new Promise(resolve => setImmediate(resolve));
  assert.equal(f.queue.bufferedFrames, 0); assert.deepEqual(f.speaking, [true]);
  assert.equal(f.adapter.takeReceipt(), undefined);
  f.idle(); assert.deepEqual(f.speaking, [true, false]);
  assert.equal(f.adapter.takeReceipt().outcome, 'completed');
  assert.equal(f.counts().closed, 1); f.adapter.close(); assert.equal(f.counts().unsubscribed, 1);
});
test('one packet buffer does not greedily consume queued PCM', async () => {
  const f = fixture(); f.send('start'); for (let i = 0; i < 10; i++) f.send('frame', frame(i));
  const stream = f.player.state.resource.playStream;
  stream.read(0); await new Promise(resolve => setImmediate(resolve));
  assert.equal(stream.readableLength, 1); assert.equal(f.queue.bufferedFrames, 9);
  f.adapter.close(); assert.equal(stream.destroyed, true);
});
test('mute and target replacement cancel output and cleanup is idempotent', () => {
  for (const cancel of [f => f.adapter.setMuted(true), f => f.adapter.setTarget(null)]) {
    const f = fixture(); f.send('start'); f.send('frame', frame(0)); cancel(f);
    assert.equal(f.adapter.takeReceipt().outcome, 'cancelled'); assert.deepEqual(f.speaking, [true, false]);
    f.adapter.close(); f.adapter.close(); assert.equal(f.counts().unsubscribed, 1);
  }
});
test('connection loss cancels without destroying the caller connection', () => {
  const f = fixture(); f.send('start'); f.connection.state.status = 'disconnected';
  f.connection.emit('stateChange'); assert.equal(f.adapter.closed, true);
  assert.equal(f.adapter.takeReceipt().reason, 'connection_changed');
  assert.equal(f.connection.listenerCount('stateChange'), 0);
});
test('foreign existing subscription and non-ready connections are refused', () => {
  for (const state of [{ status: 'ready', subscription: {} }, { status: 'disconnected' }]) {
    assert.throws(() => new PlaybackAdapter({ connection: { state }, player: {} }), /unsubscribed Ready/);
  }
});
test('controls still cancel after queue drain while SDK has buffered audio', async () => {
  for (const cancel of [f => f.adapter.setMuted(true), f => f.adapter.setTarget(null), f => f.send('cancel')]) {
    const f = fixture(); f.send('start'); f.send('frame', frame(0)); f.send('finish', { frame_count: 1 });
    f.player.state.resource.playStream.read(0); await new Promise(resolve => setImmediate(resolve));
    cancel(f); assert.equal(f.adapter.takeReceipt().outcome, 'cancelled');
    assert.deepEqual(f.speaking, [true, false]); f.adapter.close();
  }
});
test('premature SDK Idle terminates queued PCM', () => {
  const f = fixture(); f.send('start'); f.send('frame', frame(0)); f.idle();
  assert.equal(f.adapter.closed, true); assert.equal(f.queue.bufferedFrames, 0);
  assert.equal(f.adapter.takeReceipt().reason, 'player_stopped');
});

test('empty finish cannot wait forever for SDK Idle', () => {
  const f = fixture(); f.send('start'); f.send('finish', { frame_count: 0 });
  f.setTime(5000); f.adapter.tick();
  assert.equal(f.adapter.closed, true);
  assert.equal(f.adapter.takeReceipt().reason, 'player_idle_timeout');
});
test('SDK resource receives a real native-encoded Opus packet', async () => {
  const { createAudioResource } = await import('@discordjs/voice');
  const { VoiceCodec } = await import('../codec.mjs');
  const f = fixture({ resourceFactory: createAudioResource, codecFactory: () => new VoiceCodec() });
  f.send('start'); f.send('frame', frame(0)); f.send('finish', { frame_count: 1 });
  const resource = f.player.state.resource;
  resource.playStream.read(0); await new Promise(resolve => setImmediate(resolve));
  const packet = resource.playStream.read();
  assert.ok(Buffer.isBuffer(packet)); assert.ok(packet.length > 0 && packet.length < 3840);
  assert.equal(resource.playStream.readableObjectMode, true);
  f.idle(); assert.equal(f.adapter.takeReceipt().outcome, 'completed'); f.adapter.close();
});
test('late target and retired starts cannot interrupt the buffered SDK tail', async () => {
  const f = fixture(); f.send('start'); f.send('frame', frame(0)); f.send('finish', { frame_count: 1 });
  f.player.state.resource.playStream.read(0); await new Promise(resolve => setImmediate(resolve));
  assert.equal(f.send('start'), false);
  assert.equal(f.send('start', { target: { ...target, generation: 0 } }), false);
  assert.equal(f.adapter.closed, false); assert.equal(f.adapter.takeReceipt(), undefined);
  f.idle(); assert.equal(f.adapter.takeReceipt().outcome, 'completed'); f.adapter.close();
});
test('subscription replacement leaves the new subscription untouched', () => {
  const f = fixture(); f.send('start');
  const foreign = { unsubscribe() { throw new Error('foreign subscription touched'); } };
  f.connection.state.subscription = foreign; f.connection.emit('stateChange');
  assert.equal(f.adapter.closed, true); assert.equal(f.connection.state.subscription, foreign);
  assert.equal(f.counts().unsubscribed, 1);
});
test('delayed errors from an old stream cannot cancel the next playback', () => {
  const f = fixture(); f.send('start'); const old = f.player.state.resource.playStream;
  f.adapter.setMuted(true); f.adapter.takeReceipt(); f.adapter.setMuted(false);
  assert.equal(f.send('start', { playback_id: '44444444-4444-4444-8444-444444444444' }), true);
  old.emit('error', new Error('old fixture error'));
  assert.equal(f.adapter.closed, false); assert.deepEqual(f.speaking, [true, false, true]);
  f.adapter.close();
});
test('adapter feedback reports Ready, bounded consumption and actual Idle separately', async () => {
  const events = [];
  const f = fixture({ onFeedback: event => events.push(structuredClone(event)) });
  f.send('start'); f.send('frame', frame(0)); f.send('finish', { frame_count: 1 });
  assert.deepEqual(events, [{ phase: 'ready', playback_id: id, target }]);
  f.player.state.resource.playStream.read(0); await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(events.map(e => e.phase), ['ready', 'consumed']); assert.equal(events[1].frame_count, 1);
  f.idle(); assert.deepEqual(events.at(-1), { phase: 'finished', playback_id: id, target, frame_count: 1, outcome: 'completed' });
  f.adapter.close(); assert.equal(events.filter(e => e.phase === 'finished').length, 1);
});
test('adapter setup failure reports a zero-count failure without a false Ready', () => {
  const events = [];
  const f = fixture({ codecFactory: () => { throw new Error('private fake codec error'); }, onFeedback: event => events.push(event) });
  f.send('start'); assert.equal(f.adapter.closed, true);
  assert.deepEqual(events, [{ phase: 'finished', playback_id: id, target, frame_count: 0, outcome: 'failed' }]);
  assert.equal(f.counts().unsubscribed, 1);
});
test('feedback callback failure closes owned playback without duplicate cleanup', () => {
  let calls = 0;
  const f = fixture({ onFeedback: () => { calls++; throw new Error('private fake pipe error'); } });
  f.send('start'); assert.equal(f.adapter.closed, true); assert.equal(calls, 1);
  assert.equal(f.counts().unsubscribed, 1); assert.deepEqual(f.speaking, [true, false]);
  f.adapter.close(); assert.equal(f.counts().unsubscribed, 1);
});
test('adapter and feedback sender preserve host ACK ordering through completion', async () => {
  const { PlaybackFeedback } = await import('../playback-feedback.mjs');
  const { ActionSequence } = await import('../action-sequence.mjs');
  const sent = [], actions = new ActionSequence();
  const feedback = new PlaybackFeedback({ api_version: 1, type: 'host', session_id: session,
    process_generation: 1, sequence: 1, message: { type: 'handshake', capabilities: ['audio.playback'] } },
  { actions, emit: record => sent.push(record) });
  const f = fixture({ onFeedback: event => assert.equal(feedback.event(event, 0), true) });
  f.send('start'); f.send('frame', frame(0)); f.send('finish', { frame_count: 1 });
  f.player.state.resource.playStream.read(0); await new Promise(resolve => setImmediate(resolve)); f.idle();
  assert.equal(sent.length, 1); assert.equal(sent[0].action.receipt.phase, 'ready');
  for (let i = 0; i < 3; i++) {
    const action = sent[i];
    feedback.host({ api_version: 1, type: 'host', session_id: session, process_generation: 1, sequence: i + 5,
      message: { type: 'result', id: action.id, target: action.target, result: { status: 'ok', value: { type: 'accepted' } } } }, 0);
  }
  assert.deepEqual(sent.map(a => a.action.receipt.phase), ['ready', 'consumed', 'finished']);
  assert.equal(feedback.active, false); assert.equal(feedback.pendingActions, 0); f.adapter.close();
});
test('invalid-clock mute reports the clock failure rather than an applied mute', () => {
  const f = fixture(); f.send('start'); f.setTime(-1); f.adapter.setMuted(true);
  const receipt = f.adapter.takeReceipt(); assert.equal(receipt.reason, 'invalid_clock'); assert.equal(receipt.outcome, 'failed');
  assert.equal(f.adapter.closed, true);
});
test('host shutdown while SDK tail is buffered preserves the shutdown reason', async () => {
  const f = fixture(); f.send('start'); f.send('frame', frame(0)); f.send('finish', { frame_count: 1 });
  f.player.state.resource.playStream.read(0); await new Promise(resolve => setImmediate(resolve));
  f.adapter.host({ api_version: 1, type: 'host', session_id: session, process_generation: 1, sequence: 99,
    message: { type: 'event', event: { type: 'app.stopping', app_run_id: id } } });
  const receipt = f.adapter.takeReceipt(); assert.equal(receipt.reason, 'host_stopping'); assert.equal(receipt.outcome, 'cancelled');
});
test('target changes work in either feedback/adapter forwarding order with consumed audio', async () => {
  const { PlaybackFeedback } = await import('../playback-feedback.mjs');
  const { ActionSequence } = await import('../action-sequence.mjs');
  for (const feedbackFirst of [true, false]) {
    const feedback = new PlaybackFeedback({ api_version: 1, type: 'host', session_id: session,
      process_generation: 1, sequence: 1, message: { type: 'handshake', capabilities: ['audio.playback'] } },
    { actions: new ActionSequence(), emit: () => {} });
    const f = fixture({ onFeedback: event => { if (!feedback.event(event, 0)) throw new Error('Feedback rejected'); } });
    f.send('start'); f.send('frame', frame(0));
    f.player.state.resource.playStream.read(0); await new Promise(resolve => setImmediate(resolve));
    const record = { api_version: 1, type: 'host', session_id: session, process_generation: 1, sequence: 99,
      message: { type: 'event', event: { type: 'target.changed', target: { ...target, generation: 2 } } } };
    if (feedbackFirst) { feedback.host(record, 0); f.adapter.host(record); }
    else { f.adapter.host(record); feedback.host(record, 0); }
    assert.equal(f.adapter.closed, false); assert.equal(feedback.stopped, false);
    assert.equal(feedback.active, false); assert.equal(f.adapter.takeReceipt().outcome, 'cancelled');
    const start = { ...record, sequence: 100, message: { type: 'audio.output', output: { phase: 'start',
      playback_id: '44444444-4444-4444-8444-444444444444', target: { ...target, generation: 2 } } } };
    feedback.host(start, 0); assert.equal(f.adapter.host(start), true);
    assert.equal(feedback.active, true); assert.equal(f.adapter.closed, false); f.adapter.close();
  }
});
test('empty-source SDK underrun cancels just the turn and preserves the subscription', () => {
  const f = fixture(); f.send('start'); f.idle();
  const outcome = f.adapter.takeReceipt(); assert.equal(outcome.outcome, 'cancelled'); assert.equal(outcome.reason, 'source_stalled');
  assert.equal(f.adapter.closed, false); assert.equal(f.queue.stopped, false); assert.equal(f.counts().unsubscribed, 0); f.adapter.close();
});
