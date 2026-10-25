import { constants } from 'node:fs';
import { lstat, open } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';

const UUID = /^(?!00000000-0000-0000-0000-000000000000$)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export const validBotToken = token => typeof token === 'string' && token.length > 0 && token.length <= 4096 && /^[\x21-\x7e]+$/.test(token);

// Reads the existing host-owned credential format only when explicitly called.
// Never print/serialize the return value. No environment-token fallback.
export async function readBotToken(dataDirectory) {
  let file;
  try {
    if (process.platform !== 'linux' || typeof dataDirectory !== 'string' || !isAbsolute(dataDirectory)) throw new Error();
    const directory = await lstat(dataDirectory);
    if (!directory.isDirectory() || directory.isSymbolicLink() || directory.uid !== process.geteuid() || (directory.mode & 0o077)) throw new Error();
    file = await open(join(dataDirectory, 'credentials.json'), constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
    const stat = await file.stat();
    if (!stat.isFile() || stat.uid !== process.geteuid() || (stat.mode & 0o7777) !== 0o600 || stat.nlink !== 1 || stat.size > 32768) throw new Error();
    const buffer = Buffer.alloc(32769); let used = 0;
    while (used < buffer.length) {
      const { bytesRead } = await file.read(buffer, used, buffer.length - used, used);
      if (!bytesRead) break; used += bytesRead;
    }
    if (used > 32768) throw new Error();
    const saved = JSON.parse(new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(buffer.subarray(0, used)));
    if (!saved || Array.isArray(saved) || !UUID.test(saved.revision)
      || Object.keys(saved).some(key => !['revision', 'values', 'oauth_metadata'].includes(key))
      || saved.oauth_metadata != null || !saved.values || typeof saved.values !== 'object' || Array.isArray(saved.values)) throw new Error();
    const values = Object.entries(saved.values); let total = 0;
    if (values.length > 16) throw new Error();
    for (const [id, value] of values) {
      if (!/^[a-z][a-z0-9_]{0,63}$/.test(id) || typeof value !== 'string' || value.includes('\0') || Buffer.byteLength(value) > 4096) throw new Error();
      total += Buffer.byteLength(value);
    }
    if (total > 16384 || !validBotToken(saved.values.bot_token)) throw new Error();
    return saved.values.bot_token;
  } catch { throw new Error('Discord bot credentials are unavailable or invalid'); }
  finally {
    try { await file?.close(); }
    catch { throw new Error('Discord bot credentials are unavailable or invalid'); }
  }
}
