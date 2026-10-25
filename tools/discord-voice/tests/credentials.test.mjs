import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, chmod, rm, symlink, link } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { readBotToken } from '../credentials.mjs';
const value = { revision: '11111111-1111-4111-8111-111111111111', values: { bot_token: 'synthetic.bot.token' } };
async function fixture(callback) {
  const root = await mkdtemp(join(tmpdir(), 'waifu-voice-credentials-'));
  try { await writeFile(join(root, 'credentials.json'), JSON.stringify(value), { mode: 0o600 }); await callback(root); }
  finally { await rm(root, { recursive: true, force: true }); }
}
const rejected = promise => assert.rejects(promise, /^Error: Discord bot credentials are unavailable or invalid$/);

test('reads the existing private credential schema without modifying it', async () => {
  await fixture(async root => assert.equal(await readBotToken(root), 'synthetic.bot.token'));
});
test('public file or directory permissions are refused', async () => {
  await fixture(async root => { await chmod(join(root, 'credentials.json'), 0o644); await rejected(readBotToken(root)); });
  await fixture(async root => { await chmod(root, 0o755); await rejected(readBotToken(root)); });
});
test('symlinked and multiply-linked credential files are refused', async () => {
  for (const hard of [false, true]) await fixture(async root => {
    const other = join(root, 'other.json'); await writeFile(other, JSON.stringify(value), { mode: 0o600 });
    await rm(join(root, 'credentials.json'));
    if (hard) await link(other, join(root, 'credentials.json')); else await symlink(other, join(root, 'credentials.json'));
    await rejected(readBotToken(root));
  });
});
test('malformed, oversized and wrong-schema files reveal fixed errors', async () => {
  for (const content of ['private synthetic malformed text', 'x'.repeat(32769),
    JSON.stringify({ ...value, revision: '00000000-0000-0000-0000-000000000000' }),
    JSON.stringify({ ...value, unknown: 'synthetic private data' }),
    JSON.stringify({ ...value, values: { bot_token: 'invalid whitespace' } }),
    JSON.stringify({ ...value, values: { unrelated_key: 'synthetic' } })]) {
    await fixture(async root => { await writeFile(join(root, 'credentials.json'), content); await rejected(readBotToken(root)); });
  }
});
