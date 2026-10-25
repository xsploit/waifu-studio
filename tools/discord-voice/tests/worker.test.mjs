import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';

const handshake = `${JSON.stringify({ api_version: 1, type: 'host', sequence: 1,
  session_id: '11111111-1111-4111-8111-111111111111', process_generation: 1,
  message: { type: 'handshake', settings: {}, capabilities: ['audio.transcribe', 'message.deliver', 'lifecycle.observe'] } })}\n`;
async function run(mode) {
  const root = await mkdtemp(join(tmpdir(), 'waifu-voice-worker-')), marker = join(root, 'marker');
  const production = ['missing-env', 'bad-handshake', 'extra-args'].includes(mode);
  const path = fileURLToPath(new URL(production ? '../worker.mjs' : './fixtures/voice-worker.mjs', import.meta.url));
  const env = production ? (mode === 'missing-env' ? {} : { WAIFU_EXTENSION_DATA: root, WAIFU_STUDIO_EXECUTABLE: '/synthetic/studio' }) : { WAIFU_TEST_MARKER: marker };
  const child = spawn(process.execPath, [path, ...(production ? (mode === 'extra-args' ? ['--extra'] : []) : [mode])], { env, stdio: ['pipe', 'pipe', 'pipe'] });
  let output = '', errors = '', failure, spawned = false;
  const closed = new Promise(resolve => child.once('close', resolve));
  child.once('spawn', () => { spawned = true; }); child.once('error', () => { failure = new Error('Worker did not spawn'); });
  child.stdin.on('error', () => {});
  child.stdout.on('data', bytes => { output += bytes; if (output.length > 8192) child.kill(); });
  child.stderr.on('data', bytes => { errors += bytes; if (errors.length > 8192) child.kill(); });
  const timer = setTimeout(() => { failure = new Error('Worker exceeded parent deadline'); child.kill('SIGKILL'); }, 7000);
  let stoppedAt;
  try {
    if (production) {
      if (mode === 'bad-handshake') child.stdin.end('invalid\n'); else child.stdin.end();
    } else {
      child.stdin.write(handshake);
      const expected = ['startup-stall', 'late'].includes(mode) ? 'connecting' : 'ready';
      const deadline = performance.now() + 3000;
      for (;;) {
        let value; try { value = await readFile(marker, 'utf8'); } catch {}
        if (value === expected) break;
        assert.ok(performance.now() < deadline, 'Worker never reached its fixture phase');
        assert.equal(child.exitCode, null); await delay(5);
      }
      stoppedAt = performance.now();
      if (mode === 'signal') child.kill('SIGTERM');
      else if (mode !== 'fatal') child.stdin.end();
    }
    const code = await closed;
    if (failure) throw failure;
    assert.equal(spawned, true); assert.equal(child.signalCode, null); assert.equal(output, '');
    let state; try { state = await readFile(marker, 'utf8'); } catch {}
    const files = await readdir(root);
    return { code, errors, state, files, duration: stoppedAt === undefined ? 0 : performance.now() - stoppedAt };
  } finally {
    clearTimeout(timer);
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    await closed; await rm(root, { recursive: true, force: true });
  }
}

test('production entry rejects missing host environment without side effects', async () => {
  const result = await run('missing-env'); assert.equal(result.code, 1);
  assert.equal(result.errors, 'Launch the voice worker through Waifu Studio without arguments\n'); assert.deepEqual(result.files, []);
});
test('production entry rejects malformed input before reading credentials', async () => {
  const result = await run('bad-handshake'); assert.equal(result.code, 1);
  assert.equal(result.errors, 'Discord voice worker stopped or cleanup failed\n'); assert.deepEqual(result.files, []);
});
test('production entry rejects extra arguments before credential access', async () => {
  const result = await run('extra-args'); assert.equal(result.code, 1);
  assert.equal(result.errors, 'Launch the voice worker through Waifu Studio without arguments\n'); assert.deepEqual(result.files, []);
});
test('blocked startup is force-terminated after the shutdown budget', async () => {
  const result = await run('startup-stall'); assert.equal(result.code, 1);
  assert.equal(result.errors, 'Discord voice shutdown timed out\n'); assert.equal(result.state, 'connecting');
  assert.ok(result.duration >= 1800 && result.duration < 5000);
});
test('blocked cleanup is force-terminated after disposal actually starts', async () => {
  const result = await run('cleanup-stall'); assert.equal(result.code, 1);
  assert.equal(result.errors, 'Discord voice shutdown timed out\n'); assert.equal(result.state, 'disposed');
  assert.ok(result.duration >= 1800 && result.duration < 5000);
});
test('late startup is disposed and exits before the hard limit', async () => {
  const result = await run('late'); assert.equal(result.code, 0); assert.equal(result.errors, '');
  assert.equal(result.state, 'disposed'); assert.ok(result.duration < 1800);
});
test('SIGTERM performs clean owned-session disposal', async () => {
  const result = await run('signal'); assert.equal(result.code, 0); assert.equal(result.errors, ''); assert.equal(result.state, 'disposed');
});
test('unexpected errors stop with fixed text instead of raw exception detail', async () => {
  const result = await run('fatal'); assert.equal(result.code, 1);
  assert.equal(result.errors, 'Discord voice worker stopped or cleanup failed\n'); assert.equal(result.state, 'disposed');
});
