import test from 'node:test';
import assert from 'node:assert/strict';
import { ActionSequence } from '../action-sequence.mjs';
import { PlaybackFeedback } from '../playback-feedback.mjs';
const session = '11111111-1111-4111-8111-111111111111';
const id = '22222222-2222-4222-8222-222222222222';
const target = { uuid: '33333333-3333-4333-8333-333333333333', generation: 0 };
function fixture(emit) {
  const actions = new ActionSequence(), sent = [];
  const feedback = new PlaybackFeedback({ api_version: 1, type: 'host', session_id: session, process_generation: 1,
    sequence: 1, message: { type: 'handshake', capabilities: ['audio.playback'] } },
  { actions, emit: emit ?? (record => sent.push(record)) });
  let sequence = 1;
  const host = (message, now = 0, override = {}) => feedback.host({ api_version: 1, type: 'host', session_id: session,
    process_generation: 1, sequence: ++sequence, message, ...override }, now);
  const event = (phase, fields = {}, now = 0) => feedback.event({ phase, playback_id: id, target, ...fields }, now);
  const ack = (now = 0, fields = {}) => {
    const record = sent.at(-1);
    host({ type: 'result', id: record.id, target: record.target, result: { status: 'ok', value: { type: 'accepted' } }, ...fields }, now);
  };
  return { feedback, actions, sent, host, event, ack };
}
test('counts coalesce behind one ACK and terminal receipt follows consumed ACK', () => {
  const f = fixture(); f.actions.next('voice');
  f.event('ready');
  for (let n = 1; n <= 32; n++) f.event('consumed', { frame_count: n });
  f.event('finished', { frame_count: 32, outcome: 'completed' });
  assert.equal(f.sent.length, 1); assert.equal(f.feedback.pendingActions, 1);
  assert.equal(f.sent[0].sequence, 2); assert.equal(f.sent[0].id, 'playback-2');
  f.ack(); assert.deepEqual(f.sent[1].action.receipt, { phase: 'consumed', playback_id: id, frame_count: 32 });
  f.ack(); assert.equal(f.sent[2].action.receipt.phase, 'finished'); assert.equal(f.feedback.active, true);
  f.ack(); assert.equal(f.feedback.active, false); assert.equal(f.feedback.pendingActions, 0);
});
test('voice ACK cannot acknowledge playback and matching refusal stops without retry', () => {
  const f = fixture(); f.event('ready');
  f.host({ type: 'result', id: 'voice-999', target, result: { status: 'ok', value: { type: 'accepted' } } });
  assert.equal(f.feedback.pendingActions, 1); assert.equal(f.sent.length, 1);
  f.ack(0, { result: { status: 'error', code: 'permission_denied' } });
  assert.equal(f.feedback.stopped, true); f.feedback.tick(100); assert.equal(f.sent.length, 1);
});
test('uncertain writes consume shared identity and do not replay', () => {
  let attempts = 0; const f = fixture(() => { attempts++; throw new Error('private fake failure'); });
  assert.equal(f.event('ready'), false); assert.equal(f.feedback.stopped, true);
  f.feedback.tick(100); assert.equal(attempts, 1); assert.equal(f.actions.next('voice').sequence, 2);
});
test('ACK deadlines and backwards clocks stop and discard pending feedback', () => {
  const f = fixture(); f.event('ready'); f.feedback.tick(10000);
  assert.equal(f.feedback.stopped, true); assert.equal(f.feedback.pendingActions, 0);
  const g = fixture(); g.event('ready', {}, 10); g.feedback.tick(9); assert.equal(g.feedback.stopped, true);
});
test('target replacement discards obsolete pending ACKs and retired callbacks', () => {
  const f = fixture(); f.event('ready'); const pending = f.sent[0];
  f.host({ type: 'event', event: { type: 'target.changed', target: { ...target, generation: 1 } } });
  assert.equal(f.feedback.active, false); assert.equal(f.feedback.pendingActions, 0);
  f.host({ type: 'result', id: pending.id, target, result: { status: 'ok', value: { type: 'accepted' } } });
  assert.equal(f.event('finished', { frame_count: 0, outcome: 'cancelled' }), true);
  assert.equal(f.event('ready'), true); assert.equal(f.feedback.stopped, false);
  assert.equal(f.event('ready', { playback_id: '44444444-4444-4444-8444-444444444444', target: { ...target, generation: 1 } }), true);
});
test('setup failure can be reported before Ready, with zero consumption only', () => {
  const f = fixture(); assert.equal(f.event('finished', { frame_count: 0, outcome: 'failed' }), true);
  assert.deepEqual(f.sent[0].action.receipt, { phase: 'finished', playback_id: id, frame_count: 0, outcome: 'failed' });
  f.ack(); assert.equal(f.feedback.active, false);
  const g = fixture(); assert.equal(g.event('finished', { frame_count: 1, outcome: 'failed' }), false); assert.equal(g.sent.length, 0);
});
test('malformed counts, decreasing counts and unknown outcomes fail closed', () => {
  for (const fields of [{ frame_count: 0 }, { frame_count: 65537 }, { frame_count: 1.5 }]) {
    const f = fixture(); f.event('ready'); assert.equal(f.event('consumed', fields), false); assert.equal(f.feedback.stopped, true);
  }
  const f = fixture(); f.event('ready'); f.event('consumed', { frame_count: 2 });
  assert.equal(f.event('consumed', { frame_count: 1 }), false); assert.equal(f.feedback.stopped, true);
  const g = fixture(); g.event('ready'); assert.equal(g.event('finished', { frame_count: 0, outcome: 'heard' }), false);
});
test('foreign incarnation or a matching ACK with wrong target cannot release pending state', () => {
  for (const override of [{ session_id: id }, { process_generation: 2 }, { sequence: 1 }]) {
    const f = fixture(); f.event('ready'); f.host({ type: 'event', event: { type: 'unused' } }, 0, override);
    assert.equal(f.feedback.stopped, true);
  }
  const f = fixture(); f.event('ready'); f.ack(0, { target: { ...target, generation: 2 } }); assert.equal(f.feedback.stopped, true);
});

test('duplicate Ready on the active playback fails rather than being mistaken for stale', () => {
  const f = fixture(); f.event('ready'); assert.equal(f.event('ready'), false); assert.equal(f.feedback.stopped, true);
});
test('SDK finalization grace permits completion and ACKs across the source deadline', () => {
  const f = fixture(), limit = 22 * 60_000;
  f.event('ready'); f.ack();
  f.event('consumed', { frame_count: 1 }, limit - 1000);
  assert.equal(f.event('finished', { frame_count: 1, outcome: 'completed' }, limit + 1000), true);
  f.ack(limit + 1500); assert.equal(f.sent.at(-1).action.receipt.phase, 'finished');
  f.ack(limit + 2000); assert.equal(f.feedback.active, false); assert.equal(f.feedback.stopped, false);
});
test('non-ending turns and terminal ACKs both retain finite deadlines', () => {
  const f = fixture(); f.event('ready'); f.ack(); f.feedback.tick(22 * 60_000 + 5000);
  assert.equal(f.feedback.stopped, true);
  const g = fixture(); g.event('ready'); g.ack(); g.event('finished', { frame_count: 0, outcome: 'cancelled' });
  g.feedback.tick(10000); assert.equal(g.feedback.stopped, true); assert.equal(g.feedback.active, false);
});
test('non-retired unknown or wrong-target feedback stops instead of disappearing', () => {
  for (const fields of [{ playback_id: '44444444-4444-4444-8444-444444444444' }, { target: { ...target, generation: 1 } }]) {
    const f = fixture(); f.event('ready');
    assert.equal(f.event('consumed', { frame_count: 1, ...fields }), false); assert.equal(f.feedback.stopped, true);
  }
  const f = fixture(); assert.equal(f.event('consumed', { frame_count: 1 }), false); assert.equal(f.feedback.stopped, true);
});
