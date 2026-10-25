//! Native receive-only Discord bot worker. Only run as a supervised extension.
//! No outgoing Discord chat, voice transport, or user-account tokens.
use crate::{
    discord_gateway::{Effect, Gateway},
    discord_host::HostBridge,
    extension_credentials::{self, CredentialDefinition},
    extension_protocol::*,
};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, stream::MaybeTlsStream};

const API: &str = "https://discord.com/api/v10/gateway/bot";
const INTENTS: u64 = 1 | 512 | 32768;
const RUNNING: u8 = 0;
const EOF: u8 = 1;
const FAILED: u8 = 2;

pub fn credential_definitions() -> Vec<CredentialDefinition> {
    vec![CredentialDefinition {
        id: "bot_token".into(),
        label: "Discord bot token".into(),
    }]
}
fn token_valid(token: &str) -> bool {
    !token.is_empty() && token.len() <= 4096 && token.bytes().all(|b| b.is_ascii_graphic())
}
struct GatewayInfo {
    url: reqwest::Url,
    remaining: u64,
    reset_after: Duration,
}
fn gateway_info(body: &[u8]) -> Result<GatewayInfo, &'static str> {
    if body.len() > 65536 {
        return Err("Discord Gateway response exceeds its limit");
    }
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "Malformed Discord Gateway response")?;
    let endpoint = value["url"]
        .as_str()
        .filter(|url| crate::discord_gateway::resume_url(url))
        .ok_or("Invalid Discord Gateway endpoint")?;
    // This first adapter supports one shard. Do not silently drop other shards.
    if value["shards"].as_u64() != Some(1) {
        return Err("Discord adapter currently requires a single-shard bot");
    }
    let limit = &value["session_start_limit"];
    let total = limit["total"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("Invalid Discord session-start allowance")?;
    let remaining = limit["remaining"]
        .as_u64()
        .filter(|n| *n <= total)
        .ok_or("Invalid Discord session-start allowance")?;
    let reset = limit["reset_after"]
        .as_u64()
        .filter(|ms| *ms > 0 && *ms <= 7 * 24 * 3600 * 1000)
        .ok_or("Invalid Discord session-start reset")?;
    if limit["max_concurrency"].as_u64().is_none_or(|n| n == 0) {
        return Err("Invalid Discord identify concurrency");
    }
    let mut url = reqwest::Url::parse(endpoint).map_err(|_| "Invalid Discord Gateway endpoint")?;
    url.set_query(Some("v=10&encoding=json"));
    Ok(GatewayInfo {
        url,
        remaining,
        reset_after: Duration::from_millis(reset),
    })
}
fn discovery_client_builder() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
}
fn discover(token: &str) -> Result<GatewayInfo, String> {
    let client = discovery_client_builder()
        .build()
        .map_err(|_| "Cannot create Discord HTTP client")?;
    request_discovery(&client, API, token, Duration::from_secs(8))
}
// Endpoint injection stays private to local fixtures. Production always passes API.
fn request_discovery(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    token: &str,
    deadline: Duration,
) -> Result<GatewayInfo, String> {
    let response = client
        .get(endpoint)
        // A request deadline also bounds the async body beneath blocking Read;
        // Client::timeout alone restarts on each successful body read.
        .timeout(deadline)
        .header("Authorization", format!("Bot {token}"))
        .header("User-Agent", "WaifuStudio/0.1")
        .send()
        .map_err(|_| "Discord Gateway discovery could not connect")?;
    if !response.status().is_success() {
        return Err("Discord Gateway discovery failed; check bot token and service limits".into());
    }
    let mut bytes = Vec::new();
    response
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read Discord Gateway discovery")?;
    gateway_info(&bytes).map_err(Into::into)
}
/// Private wire payload only; never log or pass this to the host protocol.
fn control(effect: &Effect, token: &str) -> Option<Value> {
    match effect {
        Effect::Identify => Some(json!({"op":2,"d":{"token":token,"intents":INTENTS,
            "properties":{"os":std::env::consts::OS,"browser":"WaifuStudio","device":"WaifuStudio"},"shard":[0,1]}})),
        Effect::Resume {
            session_id,
            sequence,
        } => Some(json!({"op":6,"d":{"token":token,"session_id":session_id,"seq":sequence}})),
        Effect::Heartbeat { sequence } => Some(json!({"op":1,"d":sequence})),
        _ => None,
    }
}
fn host_line(reader: &mut impl BufRead) -> Result<Option<HostEnvelope>, &'static str> {
    let mut bytes = Vec::new();
    if reader
        .take((MAX_LINE_BYTES + 2) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| "Cannot read Discord host input")?
        == 0
    {
        return Ok(None);
    }
    if !bytes.ends_with(b"\n") {
        return Err("Discord host input exceeds its line limit");
    }
    parse_host_line(&bytes)
        .map(Some)
        .map_err(|_| "Invalid Discord host message")
}
struct Input {
    rx: mpsc::Receiver<HostEnvelope>,
    status: Arc<AtomicU8>,
}
impl Input {
    fn start(mut reader: impl BufRead + Send + 'static) -> Result<Self, &'static str> {
        let (tx, rx) = mpsc::sync_channel(32);
        let status = Arc::new(AtomicU8::new(RUNNING));
        let reader_status = status.clone();
        std::thread::Builder::new()
            .name("discord-host-input".into())
            .spawn(move || {
                loop {
                    match host_line(&mut reader) {
                        Ok(Some(message)) => {
                            if tx.try_send(message).is_err() {
                                reader_status.store(FAILED, Ordering::Release);
                                break;
                            }
                        }
                        Ok(None) => {
                            reader_status.store(EOF, Ordering::Release);
                            break;
                        }
                        Err(_) => {
                            reader_status.store(FAILED, Ordering::Release);
                            break;
                        }
                    }
                }
            })
            .map_err(|_| "Cannot start Discord host input")?;
        Ok(Self { rx, status })
    }
    fn poll(&self, host: &mut HostBridge, now: Duration) -> Result<bool, &'static str> {
        for _ in 0..32 {
            match self.status.load(Ordering::Acquire) {
                EOF => {
                    host.stop();
                    return Ok(false);
                }
                FAILED => {
                    host.stop();
                    return Err("Discord host input failed or overflowed");
                }
                _ => {}
            }
            match self.rx.try_recv() {
                Ok(message) => {
                    host.host(message, now)?;
                    if host.stopped() {
                        return Ok(false);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(!host.stopped()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    host.stop();
                    return if self.status.load(Ordering::Acquire) == EOF {
                        Ok(false)
                    } else {
                        Err("Discord host input disconnected")
                    };
                }
            }
        }
        Ok(!host.stopped())
    }
}
type Socket = WebSocket<MaybeTlsStream<std::net::TcpStream>>;

/// Handle policy effects in order. At most one chat offer is reserved per pump.
/// Result is the reconnect delay; transport never retries a failed control write.
fn apply(
    effects: Vec<Effect>,
    host: &mut HostBridge,
    now: Duration,
    token: &str,
    mut send: impl FnMut(Value) -> Result<(), String>,
) -> Result<Option<Duration>, String> {
    for effect in effects {
        if let Some(value) = control(&effect, token) {
            send(value)?;
        }
        match effect {
            Effect::Chat(message) => {
                if let Err(reason) = host.offer(message, now) {
                    eprintln!("Discord chat not queued: {reason:?}");
                }
            }
            Effect::ChatRejected { reason } => eprintln!("Discord chat rejected: {reason}"),
            Effect::Ready { .. } => eprintln!("Discord bot session ready; channel intake enabled"),
            Effect::Reconnect { minimum_delay } => return Ok(Some(minimum_delay)),
            Effect::Stopped { reason } => {
                host.stop();
                return Err(reason.into());
            }
            _ => {}
        }
    }
    Ok(None)
}
fn send(socket: &mut Socket, value: Value) -> Result<(), String> {
    socket
        .send(Message::Text(value.to_string().into()))
        .map_err(|_| "Discord control write failed".into())
}
fn output(host: &mut HostBridge, now: Duration, writer: &mut impl Write) -> Result<(), String> {
    if let Some(action) = host.next_action(now)? {
        let bytes = encode_action_line(&action).map_err(|_| "Cannot encode Discord host offer")?;
        if writer
            .write_all(&bytes)
            .and_then(|()| writer.flush())
            .is_err()
        {
            host.stop();
            return Err("Discord host output closed; action outcome uncertain".into());
        }
    }
    Ok(())
}
fn jitter() -> f64 {
    let value = uuid::Uuid::new_v4().as_u128() as u64;
    (value % 1_000_000) as f64 / 1_000_000.0
}
fn wait(
    input: &Input,
    host: &mut HostBridge,
    start: Instant,
    delay: Duration,
) -> Result<bool, String> {
    let deadline = Instant::now()
        .checked_add(delay)
        .ok_or("Discord reconnect delay overflow")?;
    while Instant::now() < deadline {
        if !input.poll(host, start.elapsed())? {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(input.poll(host, start.elapsed())?)
}
/// Only for operations with their own enforced upper bound. Host termination
/// wins over a late success or error; a returned socket/value is dropped on stop.
/// This observes cancellation at the boundary, not inside a blocked syscall.
fn while_host_active<T>(
    input: &Input,
    host: &mut HostBridge,
    start: Instant,
    operation: impl FnOnce() -> T,
) -> Result<Option<T>, &'static str> {
    if !input.poll(host, start.elapsed())? {
        return Ok(None);
    }
    let result = operation();
    if !input.poll(host, start.elapsed())? {
        return Ok(None);
    }
    Ok(Some(result))
}

pub fn run() -> Result<(), String> {
    let mut reader = io::BufReader::new(io::stdin());
    let handshake = host_line(&mut reader)?.ok_or("Missing Discord host handshake")?;
    let mut host = HostBridge::new(&handshake)?;
    let HostMessage::Handshake {
        settings,
        capabilities,
        ..
    } = &handshake.message
    else {
        unreachable!()
    };
    if !capabilities.contains(&Capability::LifecycleObserve)
        || !capabilities.contains(&Capability::MessageDeliver)
    {
        return Err("Discord needs observation and external-message permissions".into());
    }
    let filter = crate::discord_host::configured_channel(settings)?;
    let data = PathBuf::from(
        std::env::var_os("WAIFU_EXTENSION_DATA")
            .ok_or("Missing Discord integration data directory")?,
    );
    let mut credentials = extension_credentials::read(&data, &credential_definitions())?;
    let token = credentials
        .remove("bot_token")
        .filter(|token| token_valid(token))
        .ok_or("Set a valid Discord bot token in Credentials")?;
    let input = Input::start(reader)?;
    let start = Instant::now();
    let mut stdout = io::stdout();
    let mut gateway = Gateway::new(filter);
    let mut last_identify: Option<Instant> = None;
    let mut failures = 0u32;
    let mut identify_coordinator = crate::discord_identify::Coordinator::open_default()?;
    let mut identify_gate = || identify_coordinator.reserve(crate::discord_identify::Stamp::now()?);
    loop {
        if !input.poll(&mut host, start.elapsed())? {
            return Ok(());
        }
        // Discovery validates the bot's remaining session starts, and is bounded.
        let Some(info) = while_host_active(&input, &mut host, start, || discover(&token))? else {
            return Ok(());
        };
        let info = info?;
        if gateway.resume_endpoint().is_none() && info.remaining == 0 {
            eprintln!("Discord session-start allowance exhausted; waiting for reset");
            if !wait(
                &input,
                &mut host,
                start,
                info.reset_after + Duration::from_secs(1),
            )? {
                return Ok(());
            }
            continue;
        }
        let mut url = if let Some(endpoint) = gateway.resume_endpoint() {
            reqwest::Url::parse(endpoint).map_err(|_| "Invalid Discord resume endpoint")?
        } else {
            info.url
        };
        url.set_query(Some("v=10&encoding=json"));
        let Some(socket) = while_host_active(&input, &mut host, start, || {
            crate::fish_ws::observation_socket(&url)
        })?
        else {
            return Ok(());
        };
        let mut socket = match socket {
            Ok(socket) => socket,
            Err(_) => {
                failures += 1;
                if failures >= 8 {
                    return Err("Discord connection repeatedly failed; restart to retry".into());
                }
                if !wait(
                    &input,
                    &mut host,
                    start,
                    Duration::from_secs((1u64 << failures.min(5)) + 1),
                )? {
                    return Ok(());
                }
                continue;
            }
        };
        gateway.connected(start.elapsed())?;
        let outcome = run_session(
            &mut socket,
            Session {
                input: &input,
                host: &mut host,
                gateway: &mut gateway,
                start,
                token: &token,
                writer: &mut stdout,
                last_identify: &mut last_identify,
                failures: &mut failures,
                heartbeat_jitter: jitter,
                identify_gate: &mut identify_gate,
            },
        )?;
        drop(socket);
        let delay = match outcome {
            SessionExit::Stopped => return Ok(()),
            SessionExit::IdentifyWait(delay) => delay,
            SessionExit::Reconnect(delay) => {
                failures += 1;
                if failures >= 8 {
                    return Err("Discord reconnect repeatedly failed; restart to retry".into());
                }
                let backoff =
                    Duration::from_secs((1u64 << failures.min(5)) + (jitter() * 4.0) as u64);
                delay.max(backoff)
            }
        };
        if !wait(&input, &mut host, start, delay)? {
            return Ok(());
        }
    }
}

/// Explicit offline demo entry point. Embedded synthetic packets only: no
/// discovery, credentials, socket connector or private service controls are used.
pub fn run_replay() -> Result<(), String> {
    let mut reader = io::BufReader::new(io::stdin());
    let handshake = host_line(&mut reader)?.ok_or("Missing Discord replay handshake")?;
    let mut host = HostBridge::new(&handshake)?;
    let HostMessage::Handshake { capabilities, .. } = &handshake.message else {
        unreachable!()
    };
    if !capabilities.contains(&Capability::LifecycleObserve)
        || !capabilities.contains(&Capability::MessageDeliver)
    {
        return Err("Discord replay needs observation and external-message permissions".into());
    }
    let mut packets: Vec<Value> = serde_json::from_slice(include_bytes!(
        "../assets/extensions/discord-replay/fixtures.json"
    ))
    .map_err(|_| "Invalid embedded Discord replay fixture")?;
    // Each explicit start creates one new synthetic event; duplicates in this
    // start keep the same identity. This does not rewrite real Discord IDs.
    let source_id = (uuid::Uuid::new_v4().as_u128() as u64).max(1).to_string();
    for packet in &mut packets {
        if packet["t"] == "MESSAGE_CREATE" && packet["d"]["id"] == "333" {
            packet["d"]["id"] = json!(source_id);
        }
    }
    let input = Input::start(reader)?;
    let start = Instant::now();
    let mut stdout = io::stdout();
    let mut replayed = false;
    loop {
        if !input.poll(&mut host, start.elapsed())? {
            return Ok(());
        }
        if !replayed && host.ready_for_chat() {
            replayed = true;
            let mut gateway = Gateway::new(crate::discord_messages::ChannelFilter::new(
                "111".into(),
                "222".into(),
            )?);
            gateway.connected(Duration::ZERO)?;
            for packet in &packets {
                if !input.poll(&mut host, start.elapsed())? {
                    return Ok(());
                }
                let bytes = serde_json::to_vec(packet)
                    .map_err(|_| "Cannot encode Discord replay fixture")?;
                for effect in gateway.packet(&bytes, Duration::ZERO, 0.5)? {
                    if let Effect::Chat(message) = effect {
                        host.offer(message, start.elapsed())
                            .map_err(|_| "Discord replay candidate was not admitted")?;
                    }
                }
            }
        }
        output(&mut host, start.elapsed(), &mut stdout)?;
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SessionExit {
    Stopped,
    Reconnect(Duration),
    IdentifyWait(Duration),
}
struct Session<'a, W: Write> {
    input: &'a Input,
    host: &'a mut HostBridge,
    gateway: &'a mut Gateway,
    start: Instant,
    token: &'a str,
    writer: &'a mut W,
    last_identify: &'a mut Option<Instant>,
    failures: &'a mut u32,
    heartbeat_jitter: fn() -> f64,
    identify_gate: &'a mut dyn FnMut() -> Result<crate::discord_identify::Admission, &'static str>,
}
fn run_session(
    socket: &mut Socket,
    session: Session<'_, impl Write>,
) -> Result<SessionExit, String> {
    let Session {
        input,
        host,
        gateway,
        start,
        token,
        writer,
        last_identify,
        failures,
        heartbeat_jitter,
        identify_gate,
    } = session;
    let mut healthy_since: Option<Instant> = None;
    let result = (|| loop {
        if healthy_since.is_some_and(|at| at.elapsed() >= Duration::from_secs(60)) {
            *failures = 0;
        }
        if !input.poll(host, start.elapsed())? {
            return Ok(SessionExit::Stopped);
        }
        let effects =
            match socket.read() {
                Ok(Message::Text(text)) => {
                    match gateway.packet(text.as_bytes(), start.elapsed(), (heartbeat_jitter)()) {
                        Ok(effects) => effects,
                        Err(_) => return Err(
                            "Discord Gateway protocol failed; restart after checking configuration"
                                .into(),
                        ),
                    }
                }
                Ok(Message::Ping(_)) => {
                    socket.flush().map_err(|_| "Discord pong write failed")?;
                    gateway.tick(start.elapsed())
                }
                Ok(Message::Pong(_)) => gateway.tick(start.elapsed()),
                Ok(Message::Close(frame)) => {
                    gateway.disconnected(frame.map(|frame| u16::from(frame.code)))
                }
                Ok(Message::Binary(_) | Message::Frame(_)) => {
                    return Err("Unsupported compressed or binary Discord Gateway frame".into());
                }
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    gateway.tick(start.elapsed())
                }
                Err(_) => gateway.disconnected(None),
            };
        if !input.poll(host, start.elapsed())? {
            return Ok(SessionExit::Stopped);
        }
        if effects
            .iter()
            .any(|effect| matches!(effect, Effect::Identify))
        {
            // One shard, no more than one Identify per five seconds in this worker.
            if let Some(at) = last_identify.as_ref() {
                let remaining = Duration::from_secs(5).saturating_sub(at.elapsed());
                if !remaining.is_zero() {
                    gateway.disconnected(None);
                    return Ok(SessionExit::IdentifyWait(remaining));
                }
            }
            if let crate::discord_identify::Admission::Wait(delay) = identify_gate()? {
                gateway.disconnected(None);
                return Ok(SessionExit::IdentifyWait(delay));
            }
            if !input.poll(host, start.elapsed())? {
                return Ok(SessionExit::Stopped);
            }
            *last_identify = Some(Instant::now());
        }
        if effects
            .iter()
            .any(|effect| matches!(effect, Effect::Ready { .. }))
        {
            healthy_since = Some(Instant::now());
        }
        let reconnect = apply(effects, host, start.elapsed(), token, |value| {
            send(socket, value)
        })?;
        if let Some(delay) = reconnect {
            return Ok(SessionExit::Reconnect(delay));
        }
        if !input.poll(host, start.elapsed())? {
            return Ok(SessionExit::Stopped);
        }
        output(host, start.elapsed(), writer)?;
    })();
    if result.is_err() {
        host.stop();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn envelope(sequence: u64, message: HostMessage) -> HostEnvelope {
        HostEnvelope {
            api_version: API_VERSION,
            message_type: HostEnvelopeType::Host,
            session_id: uuid::Uuid::from_u128(1),
            process_generation: 3,
            sequence,
            message,
        }
    }
    pub(super) fn host() -> HostBridge {
        let mut host = HostBridge::new(&envelope(
            1,
            HostMessage::Handshake {
                origin: IntegrationOrigin {
                    integration_id: "discord-test".into(),
                    display_name: "Discord".into(),
                },
                settings: Default::default(),
                capabilities: vec![Capability::LifecycleObserve, Capability::MessageDeliver],
            },
        ))
        .unwrap();
        host.host(
            envelope(
                2,
                HostMessage::Event {
                    event: HostEvent::TargetChanged {
                        target: Some(Target {
                            uuid: uuid::Uuid::from_u128(2),
                            generation: 4,
                        }),
                    },
                },
            ),
            Duration::ZERO,
        )
        .unwrap();
        host
    }
    fn info() -> Value {
        json!({"url":"wss://gateway.discord.gg","shards":1,
        "session_start_limit":{"total":1000,"remaining":900,"reset_after":60000,"max_concurrency":1}})
    }
    fn parse_info(value: &Value) -> Result<GatewayInfo, &'static str> {
        gateway_info(&serde_json::to_vec(value).unwrap())
    }
    #[test]
    fn discovery_is_bounded_and_validates_endpoints_shards_and_quotas() {
        let parsed = parse_info(&info()).unwrap();
        assert_eq!(parsed.remaining, 900);
        assert_eq!(
            parsed.url.as_str(),
            "wss://gateway.discord.gg/?v=10&encoding=json"
        );
        for (key, value) in [("url", json!("wss://evil.example")), ("shards", json!(2))] {
            let mut data = info();
            data[key] = value;
            assert!(parse_info(&data).is_err());
        }
        for (key, value) in [
            ("remaining", json!(1001)),
            ("reset_after", json!(0)),
            ("max_concurrency", json!(0)),
        ] {
            let mut data = info();
            data["session_start_limit"][key] = value;
            assert!(parse_info(&data).is_err());
        }
        assert!(gateway_info(&vec![b' '; 65537]).is_err());
        assert!(gateway_info(b"secret-looking invalid input").is_err());
        let mut data = info();
        data["session_start_limit"]["remaining"] = json!(0);
        assert_eq!(parse_info(&data).unwrap().remaining, 0);
    }
    #[test]
    fn private_wire_controls_use_bot_intents_and_never_send_chat() {
        let token = "SyntheticPrivateBotToken";
        let identify = control(&Effect::Identify, token).unwrap();
        assert_eq!(identify["op"], 2);
        assert_eq!(identify["d"]["token"], token);
        assert_eq!(identify["d"]["intents"], INTENTS);
        assert_eq!(identify["d"]["shard"], json!([0, 1]));
        let resume = control(
            &Effect::Resume {
                session_id: "session1".into(),
                sequence: 9,
            },
            token,
        )
        .unwrap();
        assert_eq!(resume["op"], 6);
        assert_eq!(resume["d"]["seq"], 9);
        assert_eq!(
            control(&Effect::Heartbeat { sequence: None }, token).unwrap(),
            json!({"op":1,"d":null})
        );
        assert!(
            control(
                &Effect::Ready {
                    bot_id: "444".into()
                },
                token
            )
            .is_none()
        );
        for token in ["", "has space", "bad\nvalue", "é"] {
            assert!(!token_valid(token));
        }
        assert!(token_valid("SyntheticPrivateBotToken"));
    }
    #[test]
    fn decoded_gateway_chat_reaches_encoded_host_offer_without_token() {
        let mut gateway = Gateway::new(
            crate::discord_messages::ChannelFilter::new("111".into(), "222".into()).unwrap(),
        );
        gateway.connected(Duration::ZERO).unwrap();
        gateway
            .packet(
                br#"{"op":10,"d":{"heartbeat_interval":10000}}"#,
                Duration::ZERO,
                0.5,
            )
            .unwrap();
        gateway.packet(br#"{"op":0,"t":"READY","s":1,"d":{"session_id":"s","resume_gateway_url":"wss://gateway.discord.gg","user":{"id":"444","bot":true}}}"#,Duration::ZERO,0.5).unwrap();
        let effects=gateway.packet(br#"{"op":0,"t":"MESSAGE_CREATE","s":2,"d":{"id":"333","guild_id":"111","channel_id":"222","type":0,"content":"original message","author":{"id":"555","username":"Viewer"}}}"#,Duration::ZERO,0.5).unwrap();
        let mut host = host();
        apply(
            effects,
            &mut host,
            Duration::ZERO,
            "SyntheticPrivateBotToken",
            |_| panic!("chat must not write to Discord"),
        )
        .unwrap();
        let mut bytes = Vec::new();
        output(&mut host, Duration::ZERO, &mut bytes).unwrap();
        let line = std::str::from_utf8(&bytes).unwrap();
        assert!(!line.contains("SyntheticPrivateBotToken"));
        let action = parse_action_line(&bytes).unwrap();
        let Action::ChatOffer { message } = action.action else {
            panic!("expected chat offer")
        };
        assert_eq!(message.text, "original message");
        assert_eq!(message.source_id, "discord:111:222:333");
        assert_eq!(action.target.generation, 4);
        let mut second = Vec::new();
        output(&mut host, Duration::from_secs(10), &mut second).unwrap();
        assert!(second.is_empty());
    }
    #[test]
    fn host_output_failure_stops_without_replaying_uncertain_offer() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("synthetic failure"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut host = host();
        host.offer(
            ChatMessage {
                source_id: "discord:111:222:333".into(),
                author_id: "555".into(),
                author_name: "Viewer".into(),
                text: "hello".into(),
            },
            Duration::ZERO,
        )
        .unwrap();
        assert!(output(&mut host, Duration::ZERO, &mut Broken).is_err());
        assert!(host.stopped());
        let mut bytes = Vec::new();
        output(&mut host, Duration::from_secs(20), &mut bytes).unwrap();
        assert!(bytes.is_empty());
    }
    #[test]
    fn control_write_failure_surfaces_without_automatic_retry() {
        let mut host = host();
        let mut count = 0;
        let error = apply(
            vec![Effect::Identify],
            &mut host,
            Duration::ZERO,
            "SyntheticPrivateBotToken",
            |_| {
                count += 1;
                Err("fixed failure".into())
            },
        );
        assert!(error.is_err());
        assert_eq!(count, 1);
    }
    #[test]
    fn host_line_bounds_and_out_of_band_eof_prevent_late_actions() {
        assert!(host_line(&mut io::Cursor::new(vec![b'x'; MAX_LINE_BYTES + 2])).is_err());
        assert!(host_line(&mut io::Cursor::new(b"not json\n")).is_err());
        let input = Input::start(io::Cursor::new(Vec::<u8>::new())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while input.status.load(Ordering::Acquire) == RUNNING {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let mut host = host();
        assert!(!input.poll(&mut host, Duration::ZERO).unwrap());
        assert!(host.stopped());
    }
    #[test]
    fn host_input_overflow_is_terminal_even_with_queued_messages() {
        let mut bytes = Vec::new();
        for sequence in 1..=40 {
            bytes.extend(
                encode_host_line(&envelope(
                    sequence,
                    HostMessage::Event {
                        event: HostEvent::AppReady {
                            app_run_id: uuid::Uuid::from_u128(3),
                        },
                    },
                ))
                .unwrap(),
            );
        }
        let input = Input::start(io::Cursor::new(bytes)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while input.status.load(Ordering::Acquire) == RUNNING {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let mut host = host();
        assert!(input.poll(&mut host, Duration::ZERO).is_err());
        assert!(host.stopped());
    }
}

#[cfg(test)]
#[path = "discord_socket_tests.rs"]
mod socket_tests;

#[cfg(test)]
#[path = "discord_discovery_tests.rs"]
mod discovery_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "discord_supervision_tests.rs"]
mod supervision_tests;

#[cfg(test)]
#[path = "discord_tls_tests.rs"]
mod tls_tests;
