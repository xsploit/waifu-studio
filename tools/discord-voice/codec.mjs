import opus from '@discordjs/opus';
const { OpusEncoder } = opus;

export const CALL_PCM_BYTES = 960 * 2 * 2; // 20ms, 48kHz stereo PCM16.
export const ASR_FRAME_SAMPLES = 320; // 20ms, 16kHz mono PCM16.

// Decryption belongs to @discordjs/voice. Feed only its decrypted Opus packets.
// Opus's own decoder performs resampling/downmixing; no naive sample dropping.
export class VoiceCodec {
  #encoder;
  #decoder;
  #partial = Buffer.alloc(0);
  #closed = false;

  constructor() {
    this.#encoder = new OpusEncoder(48_000, 2);
    try {
      this.#encoder.setBitrate(96_000);
      this.#decoder = new OpusEncoder(16_000, 1);
    } catch {
      this.#encoder = undefined;
      throw new Error('Voice codec initialization failed');
    }
  }

  encode(pcm) {
    if (this.#closed) throw new Error('Voice codec is closed');
    if (!Buffer.isBuffer(pcm) || pcm.length !== CALL_PCM_BYTES) {
      throw new Error('Expected one 20ms stereo 48kHz PCM16 frame');
    }
    try { return Buffer.from(this.#encoder.encode(pcm)); }
    catch { throw new Error('Voice encoding failed'); }
  }

  // Returns complete host frames only. At most 60ms decoded per input packet.
  // A fresh instance is required after participant/session replacement.
  decode(packet) {
    if (this.#closed) throw new Error('Voice codec is closed');
    if (!Buffer.isBuffer(packet) || packet.length === 0 || packet.length > 3828) {
      this.#partial = Buffer.alloc(0);
      throw new Error('Invalid Opus packet size');
    }
    let pcm;
    try { pcm = Buffer.from(this.#decoder.decode(packet)); }
    catch { this.#partial = Buffer.alloc(0); throw new Error('Voice decoding failed'); }
    if (pcm.length === 0 || pcm.length > 1920 || pcm.length % 2 !== 0) {
      this.#partial = Buffer.alloc(0);
      throw new Error('Unsupported decoded voice duration');
    }
    const joined = this.#partial.length ? Buffer.concat([this.#partial, pcm]) : pcm;
    const frames = [];
    let offset = 0;
    while (offset + 640 <= joined.length) {
      const samples = new Array(ASR_FRAME_SAMPLES);
      for (let i = 0; i < samples.length; i++) samples[i] = joined.readInt16LE(offset + i * 2);
      frames.push(samples);
      offset += 640;
    }
    this.#partial = Buffer.from(joined.subarray(offset));
    return frames;
  }

  close() {
    if (this.#closed) return;
    this.#closed = true;
    this.#partial = Buffer.alloc(0);
    this.#encoder = undefined;
    this.#decoder = undefined;
  }

  // Preserve the final short packet, padded with silence to one host frame.
  finish() {
    if (this.#closed) throw new Error('Voice codec is closed');
    const frames = [];
    if (this.#partial.length) {
      const samples = new Array(ASR_FRAME_SAMPLES).fill(0);
      for (let i = 0; i < this.#partial.length / 2; i++) samples[i] = this.#partial.readInt16LE(i * 2);
      frames.push(samples);
    }
    this.close();
    return frames;
  }
}
