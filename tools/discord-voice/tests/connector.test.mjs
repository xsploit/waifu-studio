import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { setImmediate as nextTurn } from 'node:timers/promises';
import { Events, GatewayIntentBits, ChannelType } from 'discord.js';
import { createBotConnector } from '../connector.mjs';

function fixture({ remaining = 10, shards = 1, maxConcurrency = 1, channelType = ChannelType.GuildVoice, privateVoice = true, failedLogin = false } = {}) {
  const controller = new AbortController(), shard = new AbortController();
  const client = new EventEmitter(), connection = new EventEmitter();
  let options, joinOptions, readCalls = 0, reservations = 0, destroyed = 0, voiceDestroyed = 0, quotaFetches = 0;
  const guild = { voiceAdapterCreator() {}, channels: { cache: new Map() }, members: { cache: new Map() } };
  guild.channels.cache.set('222', { type: channelType, guildId: '111' });
  guild.members.cache.set('123', { voice: { channelId: '222' }, user: { id: '123', bot: false, username: 'User' }, displayName: 'Speaker' });
  client.user = { id: '999', bot: true }; client.guilds = { cache: new Map([['111', guild]]) };
  client.isReady = () => true;
  client.destroy = async () => { destroyed++; shard.abort(); };
  client.login = async token => {
    assert.equal(token, 'synthetic.bot.token');
    const throttler = options.ws.buildIdentifyThrottler({ async fetchGatewayInformation(force) {
      assert.equal(force, true); quotaFetches++; return { shards, session_start_limit: { remaining, max_concurrency: maxConcurrency } };
    } });
    await throttler.waitForIdentify(0, shard.signal);
    if (failedLogin) throw new Error('private synthetic login error');
  };
  connection.state = { status: 'ready' };
  connection.voicePrivacyCode = privateVoice ? 'synthetic-dave-code' : undefined;
  connection.destroy = () => { voiceDestroyed++; connection.state = { status: 'destroyed' }; };
  const connect = createBotConnector({ appExecutable: '/owned/studio', readToken: async () => { readCalls++; return 'synthetic.bot.token'; },
    dependencies: { makeClient: value => { options = value; return client; },
      join: value => { joinOptions = value; return connection; },
      reserve: async () => { reservations++; return { version: 1, status: 'reserved' }; } } });
  return { connect, controller, client, connection, guild, get options() { return options; }, get joinOptions() { return joinOptions; },
    counts: () => ({ readCalls, reservations, destroyed, voiceDestroyed, quotaFetches }) };
}
const settings = () => ({ guild_id: '111', channel_id: '222' });

test('connector requests bot voice intents, fresh quota checks and DAVE receive', async () => {
  const f = fixture(), requested = settings();
  const pending = f.connect({ settings: requested, signal: f.controller.signal, directions: { receive: true, playback: false } });
  requested.channel_id = '333'; // Pin routing before asynchronous credential/login work.
  const result = await pending;
  assert.deepEqual(f.options.intents, [GatewayIntentBits.Guilds, GatewayIntentBits.GuildVoiceStates]);
  assert.deepEqual(f.options.shards, [0]); assert.equal(f.options.shardCount, 1);
  assert.equal(f.joinOptions.selfDeaf, false); assert.equal(f.joinOptions.daveEncryption, true);
  assert.equal(f.joinOptions.guildId, '111'); assert.equal(f.joinOptions.channelId, '222');
  assert.deepEqual(result.lookupParticipant('123'), { id: '123', name: 'Speaker', bot: false });
  assert.equal(result.lookupParticipant('unknown'), undefined);
  f.guild.members.cache.get('123').voice.channelId = '333'; assert.equal(result.lookupParticipant('123'), undefined);
  assert.equal(f.counts().quotaFetches, 1); assert.equal(f.counts().reservations, 1);
  await result.dispose(); await result.dispose();
  assert.equal(f.counts().destroyed, 1); assert.equal(f.counts().voiceDestroyed, 1);
});

test('invalid configuration and prior cancellation do not read credentials', async () => {
  const f = fixture();
  await assert.rejects(f.connect({ settings: { guild_id: 'bad', channel_id: '222' }, signal: f.controller.signal, directions: { receive: true, playback: false } }), /configuration is invalid/);
  f.controller.abort(); await assert.rejects(f.connect({ settings: settings(), signal: f.controller.signal, directions: { receive: true, playback: false } }), /connection failed/);
  assert.equal(f.counts().readCalls, 0);
});

test('exhausted service allowance closes before shared reservation or join', async () => {
  for (const metadata of [{ remaining: 0 }, { shards: 2 }, { maxConcurrency: 0 }]) {
    const f = fixture(metadata);
    await assert.rejects(f.connect({ settings: settings(), signal: f.controller.signal, directions: { receive: true, playback: false } }), /^Error: Discord voice connection failed$/);
    assert.equal(f.counts().reservations, 0); assert.equal(f.joinOptions, undefined); assert.equal(f.counts().destroyed, 1);
  }
});

test('text or stage destinations are refused without a voice join', async () => {
  for (const channelType of [ChannelType.GuildText, ChannelType.GuildStageVoice]) {
    const f = fixture({ channelType });
    await assert.rejects(f.connect({ settings: settings(), signal: f.controller.signal, directions: { receive: true, playback: false } }), /connection failed/);
    assert.equal(f.joinOptions, undefined); assert.equal(f.counts().destroyed, 1);
  }
});

test('a missing encryption session cannot become an accepted connection', async () => {
  const f = fixture({ privateVoice: false });
  const pending = f.connect({ settings: settings(), signal: f.controller.signal, directions: { receive: true, playback: false } });
  await nextTurn(); f.controller.abort();
  await assert.rejects(pending, /connection failed/);
  assert.equal(f.counts().destroyed, 1); assert.equal(f.counts().voiceDestroyed, 1);
});

test('login failures are redacted and dispose resources', async () => {
  const f = fixture({ failedLogin: true });
  await assert.rejects(f.connect({ settings: settings(), signal: f.controller.signal, directions: { receive: true, playback: false } }), /^Error: Discord voice connection failed$/);
  assert.equal(f.counts().destroyed, 1);
});

test('gateway loss after readiness disposes bot and voice without rejoining', async () => {
  const f = fixture(); const result = await f.connect({ settings: settings(), signal: f.controller.signal, directions: { receive: true, playback: false } });
  f.client.emit(Events.ShardDisconnect, { code: 4000 }); await nextTurn();
  assert.equal(f.counts().destroyed, 1); assert.equal(f.counts().voiceDestroyed, 1);
  assert.equal(result.lookupParticipant('123'), undefined); await result.dispose(); assert.equal(f.counts().destroyed, 1);
});

test('a credential result arriving after cancellation cannot create a client', async () => {
  const controller = new AbortController(); let resolve, clients = 0;
  const token = new Promise(done => { resolve = done; });
  const connect = createBotConnector({ appExecutable: '/owned/studio', readToken: () => token,
    dependencies: { makeClient: () => { clients++; throw new Error('Cancelled startup created a client'); } } });
  const pending = connect({ settings: settings(), signal: controller.signal, directions: { receive: true, playback: false } });
  controller.abort(); resolve('synthetic.bot.token');
  await assert.rejects(pending, /^Error: Discord voice connection failed$/);
  assert.equal(clients, 0);
});


test('connector pins explicit receive/playback directions before login awaits', async () => {
  for (const directions of [{ receive: false, playback: true }, { receive: true, playback: false }, { receive: true, playback: true }]) {
    const f = fixture(), requested = { ...directions };
    const pending = f.connect({ settings: settings(), signal: f.controller.signal, directions: requested });
    requested.receive = !requested.receive; requested.playback = !requested.playback;
    const session = await pending;
    assert.equal(f.joinOptions.selfDeaf, !directions.receive);
    assert.equal(f.joinOptions.selfMute, !directions.playback);
    if (!directions.receive) assert.equal(session.lookupParticipant('123'), undefined);
    await session.dispose();
  }
});

test('missing invalid or empty directions refuse before credentials are read', async () => {
  for (const directions of [undefined, null, {}, { receive: false, playback: false }, { receive: 1, playback: true }]) {
    const f = fixture();
    await assert.rejects(f.connect({ settings: settings(), signal: f.controller.signal, directions }), /configuration is invalid/);
    assert.equal(f.counts().readCalls, 0);
  }
});
