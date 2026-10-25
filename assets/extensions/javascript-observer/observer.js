#!/usr/bin/env node
// MIT. No dependencies. stdout is reserved for host-action protocol records.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const decoder = new TextDecoder('utf-8', { fatal: true });
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const nonnil = value => typeof value === 'string' && uuid.test(value)
  && value !== '00000000-0000-0000-0000-000000000000';
const integer = value => Number.isSafeInteger(value) && value > 0;
let buffer = Buffer.alloc(0);
let identity;
let sequence = 0;
let stopping = false;
const folder = process.env.WAIFU_EXTENSION_DATA;
if (!folder) process.exit(1);
function record(name, value) {
  const destination = path.join(folder, name);
  const temporary = destination + '.tmp';
  fs.writeFileSync(temporary, JSON.stringify(value) + '\n', { mode: 0o600 });
  fs.renameSync(temporary, destination);
}
function receive(line) {
  const packet = JSON.parse(decoder.decode(line));
  if (packet.api_version !== 1 || packet.type !== 'host' || !nonnil(packet.session_id)
      || !integer(packet.process_generation) || !integer(packet.sequence)
      || packet.sequence <= sequence || !packet.message) throw Error('Invalid envelope');
  const current = packet.session_id + ':' + packet.process_generation;
  sequence = packet.sequence;
  const message = packet.message;
  if (!identity) {
    if (message.type !== 'handshake' || !Array.isArray(message.capabilities)
        || message.capabilities.length !== 1 || message.capabilities[0] !== 'lifecycle.observe')
      throw Error('Expected observation-only handshake');
    identity = current;
    record('started.json', { session_id: packet.session_id, process_generation: packet.process_generation });
    return;
  }
  if (current !== identity || message.type !== 'event' || !message.event)
    throw Error('Unexpected session or message');
  const event = message.event;
  if (event.type === 'reply.text') throw Error('Private text not requested');
  if (event.type === 'app.ready' || event.type === 'app.stopping') {
    if (!nonnil(event.app_run_id) || Object.keys(event).sort().join(',') !== 'app_run_id,type')
      throw Error('Invalid run metadata');
    record('latest-event.json', event);
    if (event.type === 'app.stopping') {
      record('latest-shutdown.json', event);
      stopping = true;
      process.stdin.pause();
      process.exit(0);
    }
  }
}
process.stdin.on('data', chunk => {
  try {
    buffer = Buffer.concat([buffer, chunk]);
    let newline;
    while ((newline = buffer.indexOf(10)) >= 0) {
      if (newline === 0 || newline + 1 > 65536) throw Error('Invalid line size');
      const line = buffer.subarray(0, newline);
      buffer = buffer.subarray(newline + 1);
      receive(line);
    }
    if (buffer.length >= 65536) throw Error('Oversized incomplete line');
  } catch (_) { process.exit(1); }
});
process.stdin.on('end', () => process.exit(stopping ? 0 : 1));
process.stdin.on('error', () => process.exit(1));
