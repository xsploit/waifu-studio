import { isAbsolute, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { VoiceDriver } from './driver.mjs';
import { createBotConnector } from './connector.mjs';

// Executable stdio worker. Import is inert; the caller must explicitly run it.
// The injected connector exists for owned offline process fixtures only.
export function runVoiceWorker({ input = process.stdin, output = process.stdout, errorOutput = process.stderr,
  env = process.env, args = process.argv.slice(2), connect, exit = code => process.exit(code) } = {}) {
  let driver, polling, finishing = false, finished = false, failed = false, started;
  const handlers = [];
  errorOutput.on('error', () => {});
  const finish = (code, message) => {
    if (finished) return; finished = true; clearTimeout(polling);
    const removeHandlers = () => { for (const [event, handler] of handlers) process.off(event, handler); };
    if (!message) { removeHandlers(); exit(code); return; }
    let exited = false;
    const leave = () => { if (exited) return; exited = true; clearTimeout(force); removeHandlers(); exit(code); };
    const force = setTimeout(leave, 50);
    try { errorOutput.write(`${message}\n`, leave); } catch { leave(); }
  };
  const stopped = reason => {
    failed ||= !['eof', 'host_stopping', 'stopped'].includes(reason);
    if (finishing) return;
    finishing = true; started = performance.now();
    const poll = () => {
      // The callback may be invoked while the driver constructor is unwinding.
      if (driver?.shutdownReady) {
        const bad = failed || driver.cleanupFailed;
        finish(bad ? 1 : 0, bad ? 'Discord voice worker stopped or cleanup failed' : undefined);
      } else if (performance.now() - started >= 2000) {
        finish(1, 'Discord voice shutdown timed out');
      } else polling = setTimeout(poll, 20);
    };
    polling = setTimeout(poll, 0);
  };
  if (args.length || typeof env.WAIFU_EXTENSION_DATA !== 'string' || !isAbsolute(env.WAIFU_EXTENSION_DATA)
    || typeof env.WAIFU_STUDIO_EXECUTABLE !== 'string' || !isAbsolute(env.WAIFU_STUDIO_EXECUTABLE)) {
    finish(1, 'Launch the voice worker through Waifu Studio without arguments');
    return undefined;
  }
  try {
    driver = new VoiceDriver(input, output, {
      connect: connect ?? createBotConnector({ dataDirectory: env.WAIFU_EXTENSION_DATA, appExecutable: env.WAIFU_STUDIO_EXECUTABLE }),
      onStop: stopped,
    });
  } catch { finish(1, 'Discord voice worker initialization failed'); return undefined; }
  for (const event of ['SIGINT', 'SIGTERM']) {
    const handler = () => driver.stop('host_stopping'); handlers.push([event, handler]); process.on(event, handler);
  }
  for (const event of ['uncaughtException', 'unhandledRejection']) {
    const handler = () => { failed = true; driver.stop('worker_error'); stopped('worker_error'); };
    handlers.push([event, handler]); process.on(event, handler);
  }
  return driver;
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) runVoiceWorker();
