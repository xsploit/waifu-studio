// Synthetic stdio fixture. No Discord client, credentials or network calls.
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
// Only this owned fixture accepts a relocated bundle root. Production worker
// arguments/authentication behavior are unchanged.
const root = process.argv[2] ? pathToFileURL(resolve(process.argv[2]) + '/') : new URL('../../', import.meta.url);
const [{ HostPipe }, { ReceiveBridge }, { VoiceCodec, CALL_PCM_BYTES }] = await Promise.all([
  import(new URL('host-io.mjs', root)), import(new URL('receive-bridge.mjs', root)), import(new URL('codec.mjs', root)),
]);

let pipe, bridge, started = false, completed = false;
pipe = new HostPipe(process.stdin, process.stdout, {
  onRecord(record) {
    const now = performance.now();
    if (!bridge) bridge = new ReceiveBridge(record, { botUserId: '999', emit: action => pipe.send(action) });
    else bridge.host(record, now);
    if (!started && record.message?.event?.type === 'target.changed' && record.message.event.target) {
      started = true;
      if (!bridge.begin({ id: '123', name: 'Pipe speaker', bot: false }, now)) throw new Error('Fixture turn refused');
      const codec = new VoiceCodec();
      try { bridge.packet('123', codec.encode(Buffer.alloc(CALL_PCM_BYTES)), now); }
      finally { codec.close(); }
      bridge.end('123', now);
    }
    completed ||= bridge.takeOutcome()?.outcome === 'queued';
    if (bridge.stopped) pipe.stop('bridge_stopped');
  },
  onTick(now) { bridge?.tick(now); if (bridge?.stopped) pipe.stop('bridge_stopped'); },
  onStop(reason) {
    bridge?.stop();
    process.exitCode = reason === 'eof' ? (completed ? 0 : 78) : 1;
    if (reason !== 'eof') process.stderr.write(`Synthetic fixture stopped: ${reason}\n`);
  },
});
