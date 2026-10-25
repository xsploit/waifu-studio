import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { reserveWithHost } from '../identify.mjs';

const executable = process.env.WAIFU_IDENTIFY_TEST_EXECUTABLE;
test('Node helper shares the real native admission file across calls', { skip: !executable }, async () => {
  const root = await mkdtemp(join(tmpdir(), 'waifu-node-identify-'));
  try {
    const run = (file, args, options, callback) => execFile(file, args,
      { ...options, env: { ...options.env, XDG_DATA_HOME: root } }, callback);
    const first = await reserveWithHost(executable, { run });
    assert.deepEqual(first, { version: 1, status: 'reserved' });
    const second = await reserveWithHost(executable, { run });
    assert.equal(second.status, 'wait'); assert.ok(second.wait_ms >= 1 && second.wait_ms <= 5000);
    const state = JSON.parse(await readFile(join(root, 'waifu-studio/discord-identify/identify.lock'), 'utf8'));
    assert.deepEqual(Object.keys(state).sort(), ['boot', 'next_ms', 'version']);
  } finally { await rm(root, { recursive: true, force: true }); }
});
