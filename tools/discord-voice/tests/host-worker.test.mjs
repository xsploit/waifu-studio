import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { isAbsolute, join } from 'node:path';

const session = '11111111-1111-4111-8111-111111111111';
const target = { uuid: '22222222-2222-4222-8222-222222222222', generation: 1 };
async function run(success) {
  const bundle = process.env.WAIFU_VOICE_TEST_BUNDLE;
  if (bundle) assert.ok(isAbsolute(bundle), 'Installed test bundle must be absolute');
  const child = spawn(bundle ? join(bundle, 'runtime', 'node') : process.execPath,
    [fileURLToPath(new URL('./fixtures/host-worker.mjs', import.meta.url)), ...(bundle ? [bundle] : [])],
    { env: {}, stdio: ['pipe', 'pipe', 'pipe'] });
  const closed = new Promise(resolve => child.once('close', resolve));
  let buffer = '', stderr = '', hostSequence = 0, failure, spawned = false;
  child.once('spawn', () => { spawned = true; });
  child.once('error', () => { failure = new Error('Synthetic child could not spawn'); });
  const records = [];
  const host = message => `${JSON.stringify({ api_version: 1, type: 'host', session_id: session,
    process_generation: 1, sequence: ++hostSequence, message })}\n`;
  child.stderr.on('data', chunk => { stderr += chunk.toString(); if (stderr.length > 8192) child.kill(); });
  child.stdin.on('error', () => {});
  child.stdout.on('data', chunk => {
    try {
      buffer += chunk.toString(); assert.ok(buffer.length < 65536);
      while (buffer.includes('\n')) {
        const end = buffer.indexOf('\n'), record = JSON.parse(buffer.slice(0, end)); buffer = buffer.slice(end + 1);
        assert.equal(record.type, 'action'); assert.equal(record.action.type, 'audio.input');
        assert.deepEqual(record.target, target); records.push(record);
        const ack = host({ type: 'result', id: record.id, target,
          result: { status: 'ok', value: { type: 'accepted' } } });
        if (record.action.input.phase === 'finish') {
          child.stdin.end(ack + host({ type: 'event', event: { type: 'audio.input.finished', target,
            turn_id: record.action.input.turn_id, outcome: 'queued' } }));
        } else child.stdin.write(ack);
      }
    } catch (error) { failure = error; child.kill(); }
  });
  const timer = setTimeout(() => { failure = new Error('Synthetic host worker timed out'); child.kill('SIGKILL'); }, 5000);
  try {
    if (success) {
      child.stdin.write(host({ type: 'handshake', capabilities: ['audio.transcribe', 'message.deliver', 'lifecycle.observe'] }));
      child.stdin.write(host({ type: 'event', event: { type: 'target.changed', target } }));
    } else child.stdin.end();
    const code = await closed;
    if (failure) throw failure;
    assert.equal(spawned, true);
    assert.equal(stderr, ''); assert.equal(buffer, '');
    return { code, records };
  } finally { clearTimeout(timer); if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); await closed; }
}

test('real child stdio carries a codec turn and exits cleanly after host EOF', async () => {
  const { code, records } = await run(true);
  assert.equal(code, 0, JSON.stringify(records.map(record => record.action.input.phase)));
  assert.deepEqual(records.map(record => record.action.input.phase), ['start', 'frame', 'finish']);
  assert.equal(records[0].action.input.participant_name, 'Pipe speaker');
  assert.equal(records[1].action.input.pcm.length, 320);
  assert.deepEqual(records.map(record => record.sequence), [1, 2, 3]);
});

test('EOF before handshake terminates without emitting actions', async () => {
  const { code, records } = await run(false);
  assert.equal(code, 78); assert.deepEqual(records, []);
});
