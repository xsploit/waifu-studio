import test from 'node:test';
import assert from 'node:assert/strict';
import { ActionSequence } from '../action-sequence.mjs';
test('interleaved producers share one counter and invalid producers consume nothing', () => {
  const actions = new ActionSequence();
  assert.deepEqual(actions.next('voice'), { id: 'voice-1', sequence: 1 });
  assert.throws(() => actions.next('foreign'));
  assert.deepEqual(actions.next('playback'), { id: 'playback-2', sequence: 2 });
  assert.deepEqual(actions.next('voice'), { id: 'voice-3', sequence: 3 });
  assert.deepEqual(new ActionSequence().next('voice'), { id: 'voice-1', sequence: 1 });
});
