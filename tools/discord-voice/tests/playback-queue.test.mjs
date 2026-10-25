import test from 'node:test';
import assert from 'node:assert/strict';
import { isAbsolute, join } from 'node:path';
import { pathToFileURL } from 'node:url';
const bundle = process.env.WAIFU_VOICE_TEST_BUNDLE;
if (bundle) assert.ok(isAbsolute(bundle));
const { PlaybackQueue } = await import(bundle ? pathToFileURL(join(bundle, 'playback-queue.mjs')) : new URL('../playback-queue.mjs', import.meta.url));
const session = '11111111-1111-4111-8111-111111111111';
const id = '22222222-2222-4222-8222-222222222222';
const target = { uuid: '33333333-3333-4333-8333-333333333333', generation: 1 };
function fixture() {
  const q = new PlaybackQueue({ api_version: 1, type: 'host', sequence: 1, session_id: session, process_generation: 1,
    message: { type: 'handshake', capabilities: ['audio.playback'] } });
  let sequence = 1;
  const host = (message, now = 0, override = {}) => q.host({ api_version: 1, type: 'host', session_id: session,
    process_generation: 1, sequence: ++sequence, message, ...override }, now);
  const send = (phase, fields = {}, now = 0) => host({ type: 'audio.output', output: { phase, playback_id: id, target, ...fields } }, now);
  q.setTarget(target, 0);
  return { q, host, send };
}
const frame = sequence => ({ frame_sequence: sequence, pcm: Array.from({ length: 1920 }, (_, i) => i % 2 ? -123 : 456) });
test('audio grant is independent of lifecycle/text permissions', () => {
  assert.throws(() => new PlaybackQueue({ api_version: 1, type: 'host', sequence: 1, session_id: session, process_generation: 1,
    message: { type: 'handshake', capabilities: ['lifecycle.observe', 'reply.read'] } }));
});
test('queue preserves complete stereo PCM frames and distinguishes drained from played', () => {
  const { q, send } = fixture();
  assert.equal(send('start'), true); assert.equal(send('frame', frame(0)), true); assert.equal(send('finish', { frame_count: 1 }), true);
  assert.equal(q.bufferedFrames, 1); assert.equal(q.takeOutcome(), undefined);
  const pcm = q.pull(20); assert.equal(pcm.length, 3840); assert.equal(pcm.readInt16LE(0), 456); assert.equal(pcm.readInt16LE(2), -123);
  assert.equal(q.takeOutcome().outcome, 'drained'); assert.equal(q.pull(21), undefined); assert.equal(q.stopped, false);
});
test('wrong session and incarnation stop without retaining PCM', () => {
  for (const override of [{ session_id: id }, { process_generation: 2 }]) {
    const { q, host } = fixture();
    assert.equal(host({ type: 'audio.output', output: { phase: 'start', playback_id: id, target } }, 0, override), false);
    assert.equal(q.stopped, true); assert.equal(q.bufferedFrames, 0);
  }
});
test('bad frame length, value, unknown fields and ordering fail closed', () => {
  for (const fields of [{ ...frame(0), pcm: [0] }, { ...frame(0), pcm: Array(1920).fill(32768) },
    { ...frame(0), pcm: Array(1920).fill(NaN) }, { ...frame(0), filename: 'invalid' }, { ...frame(0), pcm: new Array(1920) }, frame(1)]) {
    const { q, send } = fixture(); send('start');
    assert.equal(send('frame', fields), false); assert.equal(q.stopped, true); assert.equal(q.bufferedFrames, 0);
  }
});
test('queue overflow is bounded to32 complete frames and cancels old media', () => {
  const { q, send } = fixture(); send('start');
  for (let i = 0; i < 32; i++) assert.equal(send('frame', frame(i)), true);
  assert.equal(q.bufferedFrames, 32); assert.equal(send('frame', frame(32)), false);
  assert.equal(q.stopped, true); assert.equal(q.bufferedFrames, 0); assert.equal(q.pull(1), undefined);
});
test('target replacement drops queued and late media without poisoning the new target', () => {
  const { q, send } = fixture(); send('start'); send('frame', frame(0));
  q.setTarget({ ...target, generation: 2 }, 1);
  assert.equal(q.bufferedFrames, 0); assert.equal(q.takeOutcome().reason, 'target_changed');
  assert.equal(send('frame', frame(1), 2), false); assert.equal(q.stopped, false);
  assert.equal(send('start', { playback_id: '44444444-4444-4444-8444-444444444444', target: { ...target, generation: 2 } }, 3), true);
});
test('mute cancels queued audio and unmute cannot resume the previous playback', () => {
  const { q, send } = fixture(); send('start'); send('frame', frame(0)); q.setMuted(true, 1);
  assert.equal(q.pull(1), undefined); assert.equal(q.takeOutcome().reason, 'muted');
  q.setMuted(false, 2); assert.equal(send('frame', frame(1), 2), false); assert.equal(send('start', {}, 2), false);
});
test('finish count, records after finish and replayed host sequence are refused', () => {
  const a = fixture(); a.send('start'); a.send('frame', frame(0)); assert.equal(a.send('finish', { frame_count: 2 }), false); assert.equal(a.q.stopped, true);
  const b = fixture(); b.send('start'); b.send('frame', frame(0)); b.send('finish', { frame_count: 1 }); assert.equal(b.send('frame', frame(1)), false); assert.equal(b.q.stopped, true);
  const c = fixture(); assert.equal(c.host({ type: 'event' }, 0, { sequence: 1 }), false); assert.equal(c.q.stopped, true);
});
test('stall, absolute lifetime and backwards clocks cannot retain a playback', () => {
  const a = fixture(); a.send('start'); a.q.tick(5000); assert.equal(a.q.stopped, false); assert.equal(a.q.active, false); assert.equal(a.q.takeOutcome().reason, 'source_stalled');
  const b = fixture(); b.send('start'); b.q.tick(22 * 60_000); assert.equal(b.q.stopped, true);
  const c = fixture(); c.send('start', {}, 10); c.q.pull(9); assert.equal(c.q.stopped, true);
});

test('absolute deadline expires despite regular frame consumption', () => {
  const { q, send } = fixture(); send('start');
  let sequence = 0;
  for (let now = 4000; now < 22 * 60_000; now += 4000) {
    assert.equal(send('frame', frame(sequence++), now), true);
    assert.ok(q.pull(now));
  }
  q.tick(22 * 60_000); assert.equal(q.stopped, true);
  assert.equal(q.takeOutcome().reason, 'playback_timeout');
});
test('zero target generation matches Rust schema and UUID spelling cannot bypass replay checks', () => {
  const { q, send } = fixture();
  const zero = { ...target, generation: 0 }; q.setTarget(zero, 0);
  const alphaId = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
  assert.equal(send('start', { playback_id: alphaId.toUpperCase(), target: zero }), true);
  assert.equal(send('finish', { playback_id: alphaId, target: zero, frame_count: 0 }), true);
  assert.equal(q.takeOutcome().playback_id, alphaId);
  assert.equal(send('start', { playback_id: alphaId, target: zero }), false);
});

test('duplicate handshake and app shutdown cannot keep queued media live', () => {
  const a = fixture(); a.send('start'); a.send('frame', frame(0));
  assert.equal(a.host({ type: 'handshake', capabilities: ['audio.playback'] }), false);
  assert.equal(a.q.stopped, true); assert.equal(a.q.pull(1), undefined);
  const b = fixture(); b.send('start'); b.send('frame', frame(0));
  assert.equal(b.host({ type: 'event', event: { type: 'app.stopping', app_run_id: session } }), true);
  assert.equal(b.q.stopped, true); assert.equal(b.q.takeOutcome().reason, 'host_stopping');
});

test('routing events require an explicit target field and reject unknown keys', () => {
  for (const event of [{ type: 'target.changed' }, { type: 'target.changed', target: undefined }, { type: 'target.changed', target, extra: true }]) {
    const { q, host, send } = fixture(); send('start'); send('frame', frame(0));
    assert.equal(host({ type: 'event', event }), false); assert.equal(q.stopped, true); assert.equal(q.bufferedFrames, 0);
  }
  const a = fixture(); a.send('start');
  assert.equal(a.host({ type: 'event', event: { type: 'target.changed', target }, extra: true }), false);
  assert.equal(a.q.stopped, true);
  const b = fixture(); b.send('start'); b.send('frame', frame(0));
  assert.equal(b.host({ type: 'event', event: { type: 'target.changed', target: null } }), true);
  assert.equal(b.q.stopped, false); assert.equal(b.q.bufferedFrames, 0); assert.equal(b.q.takeOutcome().reason, 'target_changed');
});

test('shutdown event requires its strict nonnil run identity', () => {
  for (const event of [{ type: 'app.stopping' }, { type: 'app.stopping', app_run_id: '00000000-0000-0000-0000-000000000000' },
    { type: 'app.stopping', app_run_id: session, extra: true }]) {
    const { q, host, send } = fixture(); send('start'); send('frame', frame(0));
    assert.equal(host({ type: 'event', event }), false); assert.equal(q.stopped, true); assert.equal(q.bufferedFrames, 0);
  }
});
test('first terminal queue reason survives later cleanup', () => {
  const { q } = fixture(); q.stop('host_stopping'); q.stop('stopped');
  assert.equal(q.stopReason, 'host_stopping'); assert.equal(q.stopped, true);
});
test('buffered frames without consumer progress fail instead of blaming the source', () => {
  const { q, send } = fixture(); send('start'); send('frame', frame(0)); q.tick(5000);
  assert.equal(q.stopped, true); assert.equal(q.stopReason, 'playback_stalled'); assert.equal(q.bufferedFrames, 0);
});
