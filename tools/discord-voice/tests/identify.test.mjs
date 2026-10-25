import test from 'node:test';
import assert from 'node:assert/strict';
import { setImmediate as nextTurn } from 'node:timers/promises';
import { reserveWithHost, SharedIdentifyThrottler } from '../identify.mjs';

test('helper uses fixed args and accepts bounded token-free decisions', async () => {
  for (const value of [{ version: 1, status: 'reserved' }, { version: 1, status: 'wait', wait_ms: 5000 }]) {
    const result = await reserveWithHost('/owned/studio', { run(file, args, options, callback) {
      assert.equal(file, '/owned/studio'); assert.deepEqual(args, ['--discord-identify-admission']);
      assert.equal(options.timeout, 2000); assert.equal(options.maxBuffer, 1024);
      assert.ok(!Object.hasOwn(options.env, 'AI_GATEWAY_API_KEY'));
      callback(null, JSON.stringify(value), '');
    } });
    assert.deepEqual(result, value);
  }
});

test('invalid helper output and errors reject with fixed text', async () => {
  for (const text of ['bad', '{"version":1,"status":"wait","wait_ms":0}', '{"version":1,"status":"reserved","extra":true}']) {
    await assert.rejects(reserveWithHost('/owned/studio', { run: (_, __, ___, cb) => cb(null, text, '') }), /Invalid Identify admission/);
  }
  await assert.rejects(reserveWithHost('/owned/studio', { run: (_, __, ___, cb) => cb(new Error('private error'), '', 'private detail') }), /^Error: Identify admission helper failed$/);
});

test('every Identify obtains a fresh shared reservation after explicit waits', async () => {
  const outer = new AbortController(), shard = new AbortController(); let now = 0, calls = 0;
  const gate = new SharedIdentifyThrottler({ signal: outer.signal, now: () => now,
    reserve: async () => ++calls === 1 ? { version: 1, status: 'wait', wait_ms: 5000 } : { version: 1, status: 'reserved' },
    sleep: async ms => { now += ms; }, fatal: () => shard.abort() });
  await gate.waitForIdentify(0, shard.signal); await gate.waitForIdentify(0, shard.signal);
  assert.equal(calls, 3); assert.equal(now, 5000);
});

test('abort after reservation never returns admission or retries', async () => {
  const outer = new AbortController(), shard = new AbortController(); let calls = 0, closed = 0;
  const gate = new SharedIdentifyThrottler({ signal: outer.signal,
    reserve: async () => { calls++; outer.abort(); return { version: 1, status: 'reserved' }; },
    fatal: () => { closed++; shard.abort(); } });
  await assert.rejects(gate.waitForIdentify(0, shard.signal), /admission stopped/);
  assert.equal(calls, 1); assert.equal(closed, 1);
});

test('uncertain helper failure closes the shard before rejecting and never retries', async () => {
  const outer = new AbortController(), shard = new AbortController(); let closed = 0;
  const gate = new SharedIdentifyThrottler({ signal: outer.signal,
    reserve: async () => { throw new Error('private uncertain error'); }, fatal: () => { closed++; shard.abort(); } });
  await assert.rejects(gate.waitForIdentify(0, shard.signal), /^Error: Discord Identify admission stopped$/);
  await assert.rejects(gate.waitForIdentify(0, shard.signal), /admission stopped/);
  assert.equal(closed, 1);
});

test('failed cleanup does not release the library hook before shard cancellation', async () => {
  const outer = new AbortController(), shard = new AbortController(); let released = false;
  const gate = new SharedIdentifyThrottler({ signal: outer.signal,
    reserve: async () => { throw new Error('failure'); }, fatal: async () => { throw new Error('cleanup failed'); } });
  const waiting = gate.waitForIdentify(0, shard.signal).catch(() => { released = true; });
  await nextTurn(); assert.equal(released, false);
  shard.abort(); await waiting; assert.equal(released, true);
});

test('a reservation returned after the deadline is consumed but not admitted', async () => {
  const outer = new AbortController(), shard = new AbortController(); let now = 0;
  const gate = new SharedIdentifyThrottler({ signal: outer.signal, now: () => now,
    reserve: async () => { now = 15000; return { version: 1, status: 'reserved' }; }, fatal: () => shard.abort() });
  await assert.rejects(gate.waitForIdentify(0, shard.signal), /admission stopped/);
});
