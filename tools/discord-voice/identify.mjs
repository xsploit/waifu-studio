import { execFile } from 'node:child_process';
import { isAbsolute } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

function decision(value) {
  if (!value || value.version !== 1 || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid Identify admission');
  if (value.status === 'reserved' && Object.keys(value).length === 2) return value;
  if (value.status === 'wait' && Object.keys(value).length === 3 && Number.isInteger(value.wait_ms)
    && value.wait_ms >= 1 && value.wait_ms <= 5000) return value;
  throw new Error('Invalid Identify admission');
}

// No token is passed to this local helper. Keep the real app's data-directory
// environment so reservations are shared with native workers across profiles.
export function reserveWithHost(executable, { signal, run = execFile } = {}) {
  if (typeof executable !== 'string' || !isAbsolute(executable) || executable.includes('\0')) return Promise.reject(new Error('Invalid admission executable'));
  if (signal?.aborted) return Promise.reject(new Error('Identify admission cancelled'));
  const env = {};
  for (const key of ['HOME', 'XDG_DATA_HOME', 'PATH', 'LD_LIBRARY_PATH']) {
    if (process.env[key] !== undefined) env[key] = process.env[key];
  }
  return new Promise((resolve, reject) => {
    try {
      run(executable, ['--discord-identify-admission'], { encoding: 'utf8', timeout: 2000, maxBuffer: 1024,
        windowsHide: true, signal, env }, (error, stdout, stderr) => {
        if (error || signal?.aborted || typeof stdout !== 'string' || typeof stderr !== 'string' || stderr.trim()) { reject(new Error('Identify admission helper failed')); return; }
        try { resolve(decision(JSON.parse(stdout))); }
        catch { reject(new Error('Invalid Identify admission')); }
      });
    } catch { reject(new Error('Identify admission helper failed')); }
  });
}

// Implements @discordjs/ws IIdentifyThrottler for one shard. Configure this via
// ws.buildIdentifyThrottler so every Identify, including retries, is gated.
export class SharedIdentifyThrottler {
  #reserve;
  #fatal;
  #outer;
  #now;
  #sleep;
  #before;
  #busy = false;
  #failed = false;
  #fatalPromise;
  #abort = new AbortController();

  constructor({ reserve, fatal, signal, beforeIdentify = async () => {}, now = () => performance.now(), sleep = (ms, signal) => delay(ms, undefined, { signal }) }) {
    if (typeof reserve !== 'function' || typeof fatal !== 'function' || !(signal instanceof AbortSignal)) throw new Error('Invalid Identify throttler');
    this.#reserve = reserve; this.#fatal = fatal; this.#outer = signal; this.#now = now; this.#sleep = sleep; this.#before = beforeIdentify;
  }

  async #fail(shardSignal) {
    this.#failed = true; this.#abort.abort();
    this.#fatalPromise ??= Promise.resolve().then(() => this.#fatal()).catch(() => {});
    await this.#fatalPromise;
    // The library catches a throttle rejection. Keep it inside this hook until
    // the actual shard-close signal is observed, rather than relying on a thrown
    // error alone to prevent downstream Identify logic from continuing.
    if (!shardSignal.aborted) await new Promise(resolve => shardSignal.addEventListener('abort', resolve, { once: true }));
    throw new Error('Discord Identify admission stopped');
  }

  async waitForIdentify(shardId, shardSignal) {
    if (!shardSignal || typeof shardSignal.addEventListener !== 'function') throw new Error('Invalid Identify signal');
    if (shardId !== 0 || this.#busy || this.#failed) return this.#fail(shardSignal);
    this.#busy = true;
    const signal = AbortSignal.any([shardSignal, this.#outer, this.#abort.signal]);
    try {
      const started = this.#now(); let previous = started;
      if (!Number.isFinite(started)) throw new Error('Invalid admission clock');
      if (signal.aborted) throw new Error('Admission stopped');
      await this.#before({ signal });
      for (;;) {
        const now = this.#now();
        if (signal.aborted || !Number.isFinite(now) || now < previous || now - started >= 15000) throw new Error('Admission stopped');
        previous = now;
        const result = decision(await this.#reserve({ signal }));
        const after = this.#now();
        if (signal.aborted || !Number.isFinite(after) || after < previous || after - started >= 15000) throw new Error('Admission stopped');
        previous = after;
        if (result.status === 'reserved') return;
        await this.#sleep(Math.min(Math.max(result.wait_ms, 25), 15000 - (after - started)), signal);
      }
    } catch { return this.#fail(shardSignal); }
    finally { this.#busy = false; }
  }
}
