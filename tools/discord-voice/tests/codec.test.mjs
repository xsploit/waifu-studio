import test from 'node:test';
import assert from 'node:assert/strict';
import opus from '@discordjs/opus';
const { OpusEncoder } = opus;
import dave from '@snazzah/davey';
import { VoiceCodec, CALL_PCM_BYTES } from '../codec.mjs';

test('real Opus roundtrip supplies exact 16kHz mono host frames', () => {
  const codec = new VoiceCodec();
  try {
    let power = 0;
    for (let frame = 0; frame < 6; frame++) {
      const pcm = Buffer.alloc(CALL_PCM_BYTES);
      for (let i = 0; i < 960; i++) {
        const sample = Math.round(Math.sin((frame * 960 + i) * 2 * Math.PI * 440 / 48_000) * 12_000);
        pcm.writeInt16LE(sample, i * 4); pcm.writeInt16LE(sample, i * 4 + 2);
      }
      const packet = codec.encode(pcm);
      assert.ok(packet.length > 0 && packet.length <= 3828);
      const frames = codec.decode(packet);
      assert.equal(frames.length, 1);
      assert.equal(frames[0].length, 320);
      for (const sample of frames[0]) {
        assert.ok(Number.isInteger(sample) && sample >= -32768 && sample <= 32767);
        power += sample ** 2;
      }
    }
    assert.ok(Math.sqrt(power / 1920) > 1000);
  } finally { codec.close(); }
});

test('short Opus frames accumulate into bounded complete host frames', () => {
  const encoder = new OpusEncoder(48_000, 2);
  const codec = new VoiceCodec();
  try {
    assert.equal(codec.decode(encoder.encode(Buffer.alloc(480 * 4))).length, 0);
    assert.equal(codec.decode(encoder.encode(Buffer.alloc(480 * 4))).length, 1);
  } finally { codec.close(); }
});

test('invalid sizes and closed codecs fail without native error details', () => {
  const codec = new VoiceCodec();
  assert.throws(() => codec.encode(Buffer.alloc(1)), /Expected one 20ms/);
  for (const packet of [Buffer.alloc(0), Buffer.alloc(3829), 'packet']) {
    assert.throws(() => codec.decode(packet), /Invalid Opus packet size/);
  }
  codec.close(); codec.close();
  assert.throws(() => codec.encode(Buffer.alloc(CALL_PCM_BYTES)), /closed/);
  assert.throws(() => codec.decode(Buffer.from([1])), /closed/);
});

test('DAVE native dependency loads and creates a local synthetic session', () => {
  assert.ok(dave.DAVE_PROTOCOL_VERSION >= 1);
  const session = new dave.DAVESession(dave.DAVE_PROTOCOL_VERSION, '111', '222');
  session.reset();
});

test('several participant codecs coexist without corrupting each other', () => {
  const codecs = Array.from({ length: 4 }, () => new VoiceCodec());
  try {
    for (let iteration = 0; iteration < 6; iteration++) {
      for (const codec of codecs) {
        assert.equal(codec.decode(codec.encode(Buffer.alloc(CALL_PCM_BYTES))).length, 1);
      }
    }
    codecs[0].close();
    assert.equal(codecs[1].decode(codecs[1].encode(Buffer.alloc(CALL_PCM_BYTES))).length, 1);
  } finally { for (const codec of codecs) codec.close(); }
});

test('60ms packets split into three host frames', () => {
  const encoder = new OpusEncoder(48_000, 2);
  const codec = new VoiceCodec();
  try {
    const frames = codec.decode(encoder.encode(Buffer.alloc(2880 * 4)));
    assert.equal(frames.length, 3);
    assert.ok(frames.every(frame => frame.length === 320));
  } finally { codec.close(); }
});

test('invalid packets discard partial audio before continuing', () => {
  const encoder = new OpusEncoder(48_000, 2);
  const codec = new VoiceCodec();
  try {
    assert.equal(codec.decode(encoder.encode(Buffer.alloc(480 * 4))).length, 0);
    assert.throws(() => codec.decode(Buffer.alloc(0)), /Invalid Opus packet size/);
    assert.equal(codec.decode(encoder.encode(Buffer.alloc(480 * 4))).length, 0);
    assert.equal(codec.decode(encoder.encode(Buffer.alloc(480 * 4))).length, 1);
  } finally { codec.close(); }
});

test('ending preserves a short final packet with bounded silence padding', () => {
  const encoder = new OpusEncoder(48_000, 2);
  const codec = new VoiceCodec();
  assert.equal(codec.decode(encoder.encode(Buffer.alloc(480 * 4))).length, 0);
  const frames = codec.finish();
  assert.equal(frames.length, 1);
  assert.equal(frames[0].length, 320);
  assert.ok(frames[0].slice(160).every(sample => sample === 0));
  assert.throws(() => codec.decode(Buffer.from([1])), /closed/);
});
