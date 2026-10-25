// Dependency/preflight check only: never constructs a Client or joins a call.
import { version, generateDependencyReport } from '@discordjs/voice';
import dave from '@snazzah/davey';
import { VoiceCodec, CALL_PCM_BYTES } from './codec.mjs';

const codec = new VoiceCodec();
try {
  const frames = codec.decode(codec.encode(Buffer.alloc(CALL_PCM_BYTES)));
  if (frames.length !== 1 || frames[0].length !== 320 || dave.DAVE_PROTOCOL_VERSION < 1) {
    throw new Error('Voice dependency preflight failed');
  }
  console.log(JSON.stringify({ status: 'ok', voice_version: version,
    dave_protocol_version: dave.DAVE_PROTOCOL_VERSION, opus_roundtrip: true,
    network_used: false, discord_login: false }));
  if (process.argv.includes('--dependencies')) console.log(generateDependencyReport());
} finally { codec.close(); }
