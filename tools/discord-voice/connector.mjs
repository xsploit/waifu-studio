import { randomUUID } from 'node:crypto';
import { isAbsolute } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { Client, Events, GatewayIntentBits, ChannelType } from 'discord.js';
import { joinVoiceChannel, entersState, VoiceConnectionStatus } from '@discordjs/voice';
import { readBotToken, validBotToken } from './credentials.mjs';
import { reserveWithHost, SharedIdentifyThrottler } from './identify.mjs';

const snowflake = value => typeof value === 'string' && /^[1-9][0-9]{0,19}$/.test(value) && BigInt(value) <= 18446744073709551615n;

async function encrypted(connection, signal) {
  while (!signal.aborted) {
    if (connection.state.status !== VoiceConnectionStatus.Ready) throw new Error('Voice lost readiness');
    if (typeof connection.voicePrivacyCode === 'string' && connection.voicePrivacyCode.length) return;
    await delay(20, undefined, { signal });
  }
  throw new Error('Voice encryption cancelled');
}

// Importing this module performs no authentication or credential reads.
export function createBotConnector({ dataDirectory, appExecutable, readToken = () => readBotToken(dataDirectory), dependencies = {} }) {
  const makeClient = dependencies.makeClient ?? (options => new Client(options));
  const join = dependencies.join ?? joinVoiceChannel;
  const ready = dependencies.ready ?? ((connection, signal) => entersState(connection, VoiceConnectionStatus.Ready, signal));
  const verifyEncryption = dependencies.encrypted ?? encrypted;
  const reserve = dependencies.reserve ?? (options => reserveWithHost(appExecutable, options));

  return async ({ settings, signal, directions }) => {
    if (!snowflake(settings?.guild_id) || !snowflake(settings?.channel_id) || typeof appExecutable !== 'string'
      || !directions || typeof directions.receive !== 'boolean' || typeof directions.playback !== 'boolean'
      || (!directions.receive && !directions.playback)
      || appExecutable.includes('\0') || !isAbsolute(appExecutable) || !(signal instanceof AbortSignal)) {
      throw new Error('Discord voice configuration is invalid');
    }
    const guildId = settings.guild_id, channelId = settings.channel_id;
    const receive = directions.receive, playback = directions.playback;
    let client, connection, closing, disposed = false;
    const local = new AbortController();
    const activeSignal = AbortSignal.any([signal, local.signal]);
    const check = () => { if (activeSignal.aborted || disposed) throw new Error('Voice setup cancelled'); };
    const dispose = () => {
      if (disposed) return closing ?? Promise.resolve();
      disposed = true; local.abort(); signal.removeEventListener('abort', abort);
      closing = (async () => {
        let failed = false;
        try { if (connection && connection.state.status !== VoiceConnectionStatus.Destroyed) connection.destroy(); } catch { failed = true; }
        try { await client?.destroy(); } catch { failed = true; }
        if (failed) throw new Error('Discord voice cleanup failed');
      })();
      return closing;
    };
    const fatal = () => { const result = dispose(); result.catch(() => {}); return result; };
    const abort = () => { void fatal(); };
    signal.addEventListener('abort', abort, { once: true });
    try {
      check(); const token = await readToken(); check();
      if (!validBotToken(token)) throw new Error('Invalid token');
      client = makeClient({ intents: [GatewayIntentBits.Guilds, GatewayIntentBits.GuildVoiceStates],
        shards: [0], shardCount: 1, closeTimeout: 1000, rest: { timeout: 8000, retries: 0 },
        allowedMentions: { parse: [], repliedUser: false },
        ws: { buildIdentifyThrottler(manager) {
          return new SharedIdentifyThrottler({ signal: activeSignal, reserve, fatal,
            beforeIdentify: async () => {
              check(); const info = await manager.fetchGatewayInformation(true); check();
              if (info.shards !== 1 || !Number.isInteger(info.session_start_limit?.remaining) || info.session_start_limit.remaining < 1
                || !Number.isInteger(info.session_start_limit.max_concurrency) || info.session_start_limit.max_concurrency < 1) throw new Error('Gateway allowance unavailable');
            } });
        } } });
      for (const event of [Events.Error, Events.ShardError, Events.ShardDisconnect, Events.Invalidated]) client.on(event, fatal);
      await client.login(token); check();
      if (!client.isReady() || client.user?.bot !== true || !snowflake(client.user.id)) throw new Error('Expected ready bot');
      const guild = client.guilds.cache.get(guildId);
      const channel = guild?.channels.cache.get(channelId);
      if (!guild || channel?.type !== ChannelType.GuildVoice || channel.guildId !== guildId || typeof guild.voiceAdapterCreator !== 'function') throw new Error('Voice destination unavailable');
      connection = join({ guildId, channelId,
        adapterCreator: guild.voiceAdapterCreator, selfDeaf: !receive, selfMute: !playback,
        daveEncryption: true, debug: false, group: `waifu-${randomUUID()}` });
      connection.on('error', fatal);
      const voiceSignal = AbortSignal.any([activeSignal, AbortSignal.timeout(10000)]);
      await ready(connection, voiceSignal); check();
      await verifyEncryption(connection, voiceSignal); check();
      return { connection, botUserId: client.user.id, dispose,
        lookupParticipant(id) {
          if (!receive) return undefined;
          const member = guild.members.cache.get(id);
          if (disposed || !member || member.voice.channelId !== channelId || member.user.id !== id) return undefined;
          return { id, bot: member.user.bot, name: member.displayName || member.user.globalName || member.user.username };
        } };
    } catch {
      try { await dispose(); } catch { /* caller/supervisor owns hard cleanup */ }
      throw new Error('Discord voice connection failed');
    }
  };
}
