// Owned offline lifecycle fixture. No token reader, bot client or sockets.
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { existsSync, writeFileSync } from 'node:fs';
import { spawn } from 'node:child_process';
import { join } from 'node:path';
import { runVoiceWorker } from '../../worker.mjs';

const mode = process.argv[2], marker = process.env.WAIFU_TEST_MARKER
  ?? join(process.env.WAIFU_EXTENSION_DATA, 'marker');
let driver;
function block() {
  writeFileSync(marker, 'blocked');
  // Block timers, signal callbacks and stdin handling without burning CPU.
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 60_000);
  writeFileSync(join(process.env.WAIFU_EXTENSION_DATA, 'continued'), 'late continuation');
}
const session = () => {
  const connection = new EventEmitter(); connection.state = { status: 'ready' };
  connection.receiver = { speaking: new EventEmitter(), subscriptions: new Map(), subscribe() { return new PassThrough({ objectMode: true }); } };
  return { connection, botUserId: '999', lookupParticipant: id => ({ id, name: 'Fixture', bot: false }),
    dispose() {
      writeFileSync(marker, 'disposed');
      if (mode === 'cleanup-stall') return new Promise(() => {});
      connection.state = { status: 'destroyed' };
    } };
};
driver = runVoiceWorker({ args: [], env: { WAIFU_EXTENSION_DATA: '/synthetic/data', WAIFU_STUDIO_EXECUTABLE: '/synthetic/studio' },
  connect: async () => {
    writeFileSync(marker, 'connecting');
    if (mode === 'startup-stall') return new Promise(() => {});
    if (mode === 'late') return new Promise(resolve => setTimeout(() => resolve(session()), 100));
    return session();
  } });
if (!['startup-stall', 'late'].includes(mode)) {
  const ready = setInterval(() => {
    if (driver.ready) {
      clearInterval(ready); writeFileSync(marker, 'ready');
      if (mode === 'sync-block') {
        block();
      }
      if (mode === 'sync-block-tree') {
        const root = process.env.WAIFU_EXTENSION_DATA;
        // The descendant inherits the supervisor-owned process group. No shell,
        // credentials, sockets or detached process group are involved.
        const child = spawn(process.execPath, ['--eval', `
          const fs = require('node:fs'), path = require('node:path'), root = process.argv[1];
          fs.writeFileSync(path.join(root, 'descendant-pid'), String(process.pid));
          setInterval(() => fs.writeFileSync(path.join(root, 'heartbeat'), String(Date.now())), 10);
        `, root], { env: {}, stdio: 'ignore' });
        child.on('error', () => process.exit(79));
        const waiting = setInterval(() => {
          if (existsSync(join(root, 'heartbeat'))) {
            clearInterval(waiting);
            block();
          }
        }, 5);
      }
      if (mode === 'fatal') setTimeout(() => { throw new Error('synthetic private exception'); }, 20);
    }
  }, 5);
  ready.unref();
}
