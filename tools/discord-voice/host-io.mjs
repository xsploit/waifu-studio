const LIMIT = 64 * 1024;

// Owns protocol framing, not Discord authentication or stream destruction.
// Callbacks are synchronous; a driver must separately bound asynchronous startup.
export class HostPipe {
  #input;
  #output;
  #onRecord;
  #onStop;
  #onTick;
  #now;
  #created;
  #lastTime;
  #buffer = Buffer.alloc(LIMIT);
  #used = 0;
  #partialSince;
  #handshake = false;
  #stopped = false;
  #blocked = false;
  #queue = [];
  #bytes = 0;
  #writes = 0;
  #outputProgress;
  #timer;
  #listeners;

  constructor(input, output, { onRecord, onStop, onTick = () => {}, now = () => performance.now(), autoTick = true }) {
    if (typeof onRecord !== 'function' || typeof onStop !== 'function' || typeof onTick !== 'function'
      || typeof now !== 'function' || typeof input?.on !== 'function' || typeof output?.write !== 'function') {
      throw new Error('Invalid host pipe configuration');
    }
    this.#input = input; this.#output = output;
    this.#onRecord = onRecord; this.#onStop = onStop; this.#onTick = onTick; this.#now = now;
    this.#created = this.#lastTime = now();
    if (!Number.isFinite(this.#created)) throw new Error('Invalid host pipe clock');
    this.#listeners = {
      data: chunk => this.#read(chunk), end: () => this.stop(this.#used ? 'partial_eof' : 'eof'),
      inputError: () => this.stop('input_error'), close: () => this.stop('input_closed'),
      outputError: () => this.stop('output_error'), outputClose: () => this.stop('output_closed'),
      drain: () => { this.#blocked = false; this.#pump(); },
    };
    input.on('data', this.#listeners.data); input.on('end', this.#listeners.end);
    input.on('error', this.#listeners.inputError); input.on('close', this.#listeners.close);
    output.on('error', this.#listeners.outputError); output.on('close', this.#listeners.outputClose);
    output.on('drain', this.#listeners.drain);
    if (autoTick) { this.#timer = setInterval(() => this.tick(), 20); this.#timer.unref(); }
  }

  get stopped() { return this.#stopped; }
  get pendingBytes() { return this.#bytes; }
  get bufferedInputBytes() { return this.#used; }

  #time() {
    let now;
    try { now = this.#now(); } catch { this.stop('clock_error'); return undefined; }
    if (!Number.isFinite(now) || now < this.#lastTime) { this.stop('clock_error'); return undefined; }
    this.#lastTime = now;
    return now;
  }

  #read(chunk) {
    if (this.#stopped) return;
    if (!Buffer.isBuffer(chunk)) { this.stop('invalid_input'); return; }
    const now = this.#time(); if (now === undefined) return;
    let start = 0;
    while (!this.#stopped && start < chunk.length) {
      const newline = chunk.indexOf(10, start);
      const end = newline < 0 ? chunk.length : newline;
      const size = end - start;
      if (this.#used + size >= LIMIT) { this.stop('input_limit'); return; }
      if (this.#used === 0 && size) this.#partialSince = now;
      chunk.copy(this.#buffer, this.#used, start, end); this.#used += size;
      if (newline < 0) return;
      let record;
      try {
        record = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(this.#buffer.subarray(0, this.#used)));
      } catch { this.stop('invalid_input'); return; }
      this.#used = 0; this.#partialSince = undefined;
      if (!record || typeof record !== 'object' || Array.isArray(record) || record.api_version !== 1 || record.type !== 'host') {
        this.stop('invalid_input'); return;
      }
      if (!this.#handshake && record.message?.type !== 'handshake') { this.stop('missing_handshake'); return; }
      if (this.#handshake && record.message?.type === 'handshake') { this.stop('duplicate_handshake'); return; }
      this.#handshake = true;
      try {
        const result = this.#onRecord(record);
        if (result && typeof result.then === 'function') {
          // Do not leave a rejected callback promise unobserved after stopping.
          Promise.resolve(result).catch(() => {});
          this.stop('async_callback'); return;
        }
      } catch { this.stop('callback_error'); return; }
      start = newline + 1;
    }
  }

  send(record) {
    if (this.#stopped) throw new Error('Host pipe is stopped');
    const now = this.#time();
    if (now === undefined) throw new Error('Host pipe clock failed');
    if (!this.#handshake || !record || record.api_version !== 1 || record.type !== 'action') {
      this.stop('invalid_output'); throw new Error('Invalid host output');
    }
    let line;
    try { line = Buffer.from(`${JSON.stringify(record)}\n`); }
    catch { this.stop('invalid_output'); throw new Error('Invalid host output'); }
    if (line.length > LIMIT || this.#writes >= 32 || this.#bytes + line.length > LIMIT) {
      this.stop('output_limit'); throw new Error('Host output budget exceeded');
    }
    if (this.#bytes === 0) this.#outputProgress = now;
    this.#bytes += line.length; this.#writes++;
    this.#queue.push(line); this.#pump();
    if (this.#stopped) throw new Error('Host output failed');
  }

  #pump() {
    while (!this.#stopped && !this.#blocked && this.#queue.length) {
      const line = this.#queue.shift();
      let called = false;
      const written = error => {
        if (called) return; called = true;
        this.#bytes -= line.length; this.#writes--;
        if (error) this.stop('output_error');
        else if (!this.#stopped) this.#outputProgress = this.#time();
      };
      try { this.#blocked = !this.#output.write(line, written); }
      catch { written(); this.stop('output_error'); }
    }
  }

  tick() {
    if (this.#stopped) return;
    const now = this.#time(); if (now === undefined) return;
    if (!this.#handshake && now - this.#created >= 8000) { this.stop('handshake_timeout'); return; }
    if (this.#used && now - this.#partialSince >= 5000) { this.stop('partial_timeout'); return; }
    if (this.#bytes && now - this.#outputProgress >= 10_000) { this.stop('output_timeout'); return; }
    try {
      const result = this.#onTick(now);
      if (result && typeof result.then === 'function') {
        Promise.resolve(result).catch(() => {}); this.stop('async_callback');
      }
    } catch { this.stop('callback_error'); }
  }

  stop(reason = 'stopped') {
    if (this.#stopped) return;
    this.#stopped = true; clearInterval(this.#timer);
    for (const line of this.#queue) { this.#bytes -= line.length; this.#writes--; }
    this.#queue = []; this.#used = 0; this.#buffer = Buffer.alloc(0);
    const l = this.#listeners;
    this.#input.pause();
    this.#input.off('data', l.data); this.#input.off('end', l.end);
    this.#input.off('error', l.inputError); this.#input.off('close', l.close);
    this.#output.off('error', l.outputError); this.#output.off('close', l.outputClose); this.#output.off('drain', l.drain);
    // Caller-owned streams can report a queued error after stop. No raw error
    // text or protocol input is logged; the process driver owns final teardown.
    this.#input.on('error', () => {}); this.#output.on('error', () => {});
    try {
      const result = this.#onStop(reason);
      if (result && typeof result.then === 'function') Promise.resolve(result).catch(() => {});
    } catch { /* terminal cleanup is sticky */ }
  }
}
