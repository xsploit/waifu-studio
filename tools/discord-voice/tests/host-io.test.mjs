import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { setImmediate as nextTurn } from 'node:timers/promises';
import { HostPipe } from '../host-io.mjs';

const handshake = { api_version: 1, type: 'host', message: { type: 'handshake' } };
const line = value => Buffer.from(`${JSON.stringify(value)}\n`);
function fixture(output = new PassThrough(), overrides = {}) {
  const input = new PassThrough(), records = [], stops = [];
  let now = 0;
  const pipe = new HostPipe(input, output, { now: () => now, autoTick: false,
    onRecord: record => records.push(record), onStop: reason => stops.push(reason), ...overrides });
  return { pipe, input, output, records, stops, setNow: value => { now = value; } };
}

test('fragmented UTF-8 and multiple lines preserve exact records', async () => {
  const f = fixture();
  const second = { api_version: 1, type: 'host', message: { type: 'event', text: 'hello 🌸' } };
  const bytes = Buffer.concat([line(handshake), line(second)]);
  try {
    for (const byte of bytes) f.input.write(Buffer.from([byte]));
    await nextTurn(); assert.deepEqual(f.records, [handshake, second]);
    assert.equal(f.pipe.bufferedInputBytes, 0);
    f.input.end(); await nextTurn();
    assert.deepEqual(f.stops, ['eof']);
    assert.equal(f.input.listenerCount('data'), 0);
  } finally { f.pipe.stop(); }
});

test('bad UTF-8, oversized lines and partial EOF fail closed', async () => {
  for (const [bytes, reason] of [[Buffer.from([0xff, 10]), 'invalid_input'],
    [Buffer.alloc(65536, 32), 'input_limit'], [Buffer.from('{'), 'partial_eof']]) {
    const f = fixture(); f.input.write(bytes); f.input.end(); await nextTurn();
    assert.deepEqual(f.stops, [reason]); assert.equal(f.records.length, 0);
    assert.equal(f.pipe.bufferedInputBytes, 0);
  }
});

test('handshake and absolute partial-line deadlines do not reset on drip input', () => {
  const absent = fixture(); absent.setNow(8000); absent.pipe.tick();
  assert.deepEqual(absent.stops, ['handshake_timeout']);
  const drip = fixture(); drip.input.write(line(handshake)); drip.input.write('{');
  drip.setNow(4999); drip.input.write(' '); drip.pipe.tick(); assert.equal(drip.pipe.stopped, false);
  drip.setNow(5000); drip.pipe.tick(); assert.deepEqual(drip.stops, ['partial_timeout']);
});

test('a handshake cannot be omitted or repeated', () => {
  const missing = fixture(); missing.input.write(line({ api_version: 1, type: 'host', message: { type: 'event' } }));
  assert.deepEqual(missing.stops, ['missing_handshake']);
  const duplicate = fixture(); duplicate.input.write(Buffer.concat([line(handshake), line(handshake)]));
  assert.deepEqual(duplicate.stops, ['duplicate_handshake']); assert.equal(duplicate.records.length, 1);
});

class HeldOutput extends EventEmitter {
  writes = [];
  write(bytes, callback) { this.writes.push({ bytes: Buffer.from(bytes), callback }); return false; }
}
const action = id => ({ api_version: 1, type: 'action', id });

test('output backpressure queues bounded records and never replays a write', () => {
  const output = new HeldOutput(), f = fixture(output); f.input.write(line(handshake));
  f.pipe.send(action('one')); f.pipe.send(action('two'));
  assert.equal(output.writes.length, 1);
  output.writes[0].callback(); output.emit('drain');
  assert.equal(output.writes.length, 2);
  assert.deepEqual(output.writes.map(write => JSON.parse(write.bytes)), [action('one'), action('two')]);
  output.writes[1].callback(); assert.equal(f.pipe.pendingBytes, 0); f.pipe.stop();
});

test('output count, byte limits and write stalls stop once', () => {
  const count = fixture(new HeldOutput()); count.input.write(line(handshake));
  for (let i = 0; i < 32; i++) count.pipe.send(action(String(i)));
  assert.throws(() => count.pipe.send(action('overflow')), /budget/);
  assert.deepEqual(count.stops, ['output_limit']);
  const bytes = fixture(new HeldOutput()); bytes.input.write(line(handshake));
  bytes.pipe.send({ ...action('large'), text: 'x'.repeat(60000) });
  assert.throws(() => bytes.pipe.send({ ...action('large2'), text: 'x'.repeat(6000) }), /budget/);
  assert.deepEqual(bytes.stops, ['output_limit']);
  const stall = fixture(new HeldOutput()); stall.input.write(line(handshake)); stall.pipe.send(action('stalled'));
  stall.setNow(10000); stall.pipe.tick(); assert.deepEqual(stall.stops, ['output_timeout']);
});

test('write failure, callback errors and clock reversal reveal no raw payloads', () => {
  const output = new HeldOutput(), f = fixture(output); f.input.write(line(handshake)); f.pipe.send(action('one'));
  output.writes[0].callback(new Error('private synthetic text'));
  output.emit('error', new Error('another private error')); f.pipe.stop();
  assert.deepEqual(f.stops, ['output_error']);
  const callback = fixture(undefined, { onRecord: () => { throw new Error('private callback text'); } });
  callback.input.write(line(handshake)); assert.deepEqual(callback.stops, ['callback_error']);
  const reversed = fixture(); reversed.setNow(-1); reversed.pipe.tick(); assert.deepEqual(reversed.stops, ['clock_error']);
});

test('async callbacks stop without unhandled rejections', async () => {
  const f = fixture(undefined, { onRecord: async () => { throw new Error('synthetic rejection'); } });
  f.input.write(line(handshake)); await nextTurn(); assert.deepEqual(f.stops, ['async_callback']);
});
