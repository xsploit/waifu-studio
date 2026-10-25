//! Native hosted Fish live WebSocket speech. MessagePack binary frames, bounded
//! text/PCM queues, one network/decoder worker, and explicit proxy routing.
use crate::{
    fish_live::{LiveEvent, LiveStats, WordTiming},
    voice::{VoiceAdapter, VoiceConfig},
    voice_stream::{StreamBudget, receive_wav_with_budget},
};
use base64::Engine;
use bevy::prelude::Resource;
use rmpv::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Cursor, Read, Write},
    net::{IpAddr, Shutdown, TcpStream, ToSocketAddrs},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tungstenite::{
    Message, WebSocket, client::IntoClientRequest, protocol::WebSocketConfig,
    stream::MaybeTlsStream,
};

const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_TEXT_CHARS: usize = 12_000;
const MAX_COMMANDS: usize = 256;
const MAX_MESSAGE: usize = 2 * 1024 * 1024;
const MAX_AUDIO: usize = 32 * 1024 * 1024;
const MAX_WIRE: usize = 64 * 1024 * 1024;
const MAX_WORDS: usize = 12_000;
/// A cancelled worker cannot be interrupted inside DNS or a blocking TCP
/// connect, so one extra slot lets the next generation start while it unwinds.
/// Further requests wait in `pending`; threads and sockets stay bounded.
const MAX_WORKERS: usize = 2;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 10 });
/// Each non-final resolved address gets at most this long, so one blackholed
/// address (often IPv6) cannot consume the whole connect deadline.
const ADDRESS_TIMEOUT: Duration = Duration::from_millis(1500);
type Mailbox = Arc<Mutex<VecDeque<(u64, LiveEvent)>>>;
type SocketControl = Arc<Mutex<Option<(u64, TcpStream)>>>;

#[derive(Clone)]
enum Command {
    Text(String),
    Flush,
    Stop,
}
impl Command {
    fn bytes(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            _ => 0,
        }
    }
}
#[derive(Default)]
struct TextQueue {
    commands: VecDeque<Command>,
    bytes: usize,
    total_chars: usize,
    since_flush: usize,
    first_flushed: bool,
    finishing: bool,
}
impl TextQueue {
    fn push(&mut self, text: &str, auto_flush: bool) -> Result<(), String> {
        if self.finishing {
            return Err("Fish speech input is already finished".into());
        }
        if text.is_empty() {
            return Ok(());
        }
        let chars = text.chars().count();
        if self.total_chars + chars > MAX_TEXT_CHARS || self.bytes + text.len() > MAX_TEXT_BYTES {
            return Err("Fish text exceeds 12000 characters or the 64 KiB pending limit".into());
        }
        if self.commands.len() + 2 > MAX_COMMANDS {
            return Err("Fish text command queue is full".into());
        }
        // WW4's early sentence / clause boundaries and 68-character hard
        // limit keep a long punctuation-free first sentence from stalling audio.
        // Split on UTF-8 character boundaries; text is forwarded exactly once.
        let mut count = self.since_flush;
        let mut first_flushed = self.first_flushed;
        let mut boundaries = Vec::new();
        for (index, ch) in text.char_indices() {
            count += 1;
            let sentence = count >= 10 && matches!(ch, '.' | '!' | '?' | '\n' | '。' | '！' | '？');
            let clause = count >= 44 && matches!(ch, ',' | ';' | ':' | '，' | '；');
            let first_phrase = !first_flushed && count >= 24 && ch.is_whitespace();
            if auto_flush && (first_phrase || sentence || clause || count >= 68) {
                boundaries.push(index + ch.len_utf8());
                count = 0;
                first_flushed = true;
            }
        }
        if self.commands.len() + boundaries.len() * 2 + 2 > MAX_COMMANDS {
            return Err("Fish text command queue is full".into());
        }
        self.total_chars += chars;
        self.bytes += text.len();
        let mut start = 0;
        for end in boundaries {
            self.append_text(&text[start..end]);
            self.flush()?;
            start = end;
        }
        self.append_text(&text[start..]);
        self.since_flush = count;
        Ok(())
    }
    fn append_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(Command::Text(last)) = self.commands.back_mut() {
            last.push_str(text);
        } else {
            self.commands.push_back(Command::Text(text.into()));
        }
    }
    fn flush(&mut self) -> Result<(), String> {
        if self.finishing {
            return Err("Fish speech input is already finished".into());
        }
        if self.commands.len() >= MAX_COMMANDS {
            return Err("Fish text command queue is full".into());
        }
        if !matches!(self.commands.back(), Some(Command::Flush)) {
            self.commands.push_back(Command::Flush);
        }
        self.since_flush = 0;
        self.first_flushed = true;
        Ok(())
    }
    fn finish(&mut self) -> Result<(), String> {
        if self.finishing {
            return Ok(());
        }
        if self.commands.len() >= MAX_COMMANDS {
            return Err("Fish text command queue is full".into());
        }
        self.commands.push_back(Command::Stop);
        self.finishing = true;
        Ok(())
    }
}
#[derive(Clone, Debug)]
enum Route {
    Direct,
    HttpConnect {
        host: String,
        port: u16,
        authorization: Option<String>,
    },
}
struct Request {
    id: u64,
    generation: u64,
    url: reqwest::Url,
    route: Route,
    key: String,
    config: VoiceConfig,
    timestamps: bool,
    input: Arc<Mutex<TextQueue>>,
}
#[derive(Resource)]
pub struct FishWsState {
    pub enabled: bool,
    pub timestamps: bool,
    pub auto_flush: bool,
    pub status: String,
    pub words: Vec<WordTiming>,
    pub last_stats: Option<LiveStats>,
    generation: Arc<AtomicU64>,
    audio_generation: Arc<AtomicU64>,
    workers: Arc<AtomicUsize>,
    mailbox: Mailbox,
    socket: SocketControl,
    input: Option<Arc<Mutex<TextQueue>>>,
    pending: Option<Request>,
    id: Option<u64>,
}
impl Default for FishWsState {
    fn default() -> Self {
        Self {
            enabled: false,
            timestamps: false,
            auto_flush: true,
            status: "Ready · live WebSocket".into(),
            words: Vec::new(),
            last_stats: None,
            generation: Arc::new(AtomicU64::new(0)),
            audio_generation: Arc::new(AtomicU64::new(0)),
            workers: Arc::new(AtomicUsize::new(0)),
            mailbox: Arc::new(Mutex::new(VecDeque::new())),
            socket: Arc::new(Mutex::new(None)),
            input: None,
            pending: None,
            id: None,
        }
    }
}
impl FishWsState {
    pub fn open(
        &mut self,
        id: u64,
        config: &VoiceConfig,
        session_key: Option<String>,
    ) -> Result<(), String> {
        self.open_with_env(id, config, session_key, |name| std::env::var(name).ok())
    }
    fn open_with_env(
        &mut self,
        id: u64,
        config: &VoiceConfig,
        session_key: Option<String>,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<(), String> {
        let url = validate(config, self.timestamps)?;
        let route = proxy_route(&url, lookup)?;
        let env_key = if config.api_key_env.trim().is_empty() {
            None
        } else {
            std::env::var(config.api_key_env.trim()).ok()
        };
        let key = session_key
            .filter(|key| !key.trim().is_empty())
            .or(env_key)
            .filter(|key| !key.trim().is_empty())
            .ok_or("Set a Fish session key or API key environment variable")?;
        if key.len() > 16 * 1024
            || tungstenite::http::HeaderValue::from_str(&format!("Bearer {key}")).is_err()
        {
            return Err("Invalid Fish API key header".into());
        }
        self.cancel();
        let generation = self.generation.load(Ordering::Acquire);
        let input = Arc::new(Mutex::new(TextQueue::default()));
        if !config.direction.trim().is_empty() {
            input
                .lock()
                .map_err(|_| "Fish input queue failed")?
                .push(&format!("[{}] ", config.direction.trim()), false)?;
        }
        self.input = Some(input.clone());
        self.id = Some(id);
        self.last_stats = None;
        self.pending = Some(Request {
            id,
            generation,
            url,
            route,
            key,
            config: config.clone(),
            timestamps: self.timestamps,
            input,
        });
        self.status = "Connecting · live WebSocket".into();
        self.start_pending();
        Ok(())
    }
    pub fn push_text(&mut self, delta: &str) -> Result<(), String> {
        self.input
            .as_ref()
            .ok_or("Open a Fish live session before sending text")?
            .lock()
            .map_err(|_| "Fish input queue failed")?
            .push(delta, self.auto_flush)
    }
    pub fn flush(&mut self) -> Result<(), String> {
        self.input
            .as_ref()
            .ok_or("No active Fish live session")?
            .lock()
            .map_err(|_| "Fish input queue failed")?
            .flush()
    }
    pub fn finish(&mut self) -> Result<(), String> {
        self.input
            .as_ref()
            .ok_or("No active Fish live session")?
            .lock()
            .map_err(|_| "Fish input queue failed")?
            .finish()
    }
    pub fn cancel(&mut self) {
        let next = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.audio_generation.store(next, Ordering::Release);
        self.input = None;
        self.pending = None;
        self.id = None;
        self.words.clear();
        if let Ok(mut queue) = self.mailbox.lock() {
            queue.clear();
        }
        if let Ok(mut socket) = self.socket.lock()
            && let Some((_, socket)) = socket.take()
        {
            let _ = socket.shutdown(Shutdown::Both);
        }
        self.status = "Cancelled".into();
    }
    pub fn is_busy(&self) -> bool {
        self.pending.is_some() || self.workers.load(Ordering::Acquire) > 0
    }
    pub fn active_id(&self) -> Option<u64> {
        self.id
    }
    fn start_pending(&mut self) {
        if self.pending.is_none()
            || self
                .workers
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                    (active < MAX_WORKERS).then_some(active + 1)
                })
                .is_err()
        {
            return;
        }
        let request = self.pending.take().unwrap();
        let expected = request.generation;
        let id = request.id;
        let active = self.generation.clone();
        let audio = self.audio_generation.clone();
        let workers = self.workers.clone();
        let mailbox = self.mailbox.clone();
        let control = self.socket.clone();
        let spawned = std::thread::Builder::new()
            .name("fish-live-websocket".into())
            .spawn(move || {
                struct Permit {
                    workers: Arc<AtomicUsize>,
                    control: SocketControl,
                    expected: u64,
                }
                impl Drop for Permit {
                    fn drop(&mut self) {
                        if let Ok(mut socket) = self.control.lock()
                            && socket
                                .as_ref()
                                .is_some_and(|(generation, _)| *generation == self.expected)
                        {
                            socket.take();
                        }
                        self.workers.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                let _permit = Permit {
                    workers,
                    control: control.clone(),
                    expected,
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(
                        request,
                        active.clone(),
                        audio.clone(),
                        mailbox.clone(),
                        control,
                    )
                }))
                .unwrap_or_else(|_| Err("Fish WebSocket worker failed".into()));
                let event = match result {
                    Ok(stats) => LiveEvent::Finished { id, stats },
                    Err(message) => {
                        let _ = audio.compare_exchange(
                            expected,
                            expected.wrapping_add(1),
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                        LiveEvent::Failed { id, message }
                    }
                };
                publish(&mailbox, &active, expected, event);
            });
        if spawned.is_err() {
            self.workers.fetch_sub(1, Ordering::AcqRel);
            publish(
                &self.mailbox,
                &self.generation,
                expected,
                LiveEvent::Failed {
                    id,
                    message: "Could not start Fish WebSocket worker".into(),
                },
            );
        }
    }
    pub fn poll(&mut self) -> Vec<LiveEvent> {
        self.start_pending();
        let current = self.generation.load(Ordering::Acquire);
        let events: Vec<_> = self
            .mailbox
            .lock()
            .map(|mut queue| {
                queue
                    .drain(..)
                    .filter(|(generation, _)| *generation == current)
                    .map(|(_, event)| event)
                    .collect()
            })
            .unwrap_or_default();
        for event in &events {
            match event {
                LiveEvent::Playing { .. } => self.status = "Playing · live WebSocket".into(),
                LiveEvent::Timing { words, .. } => self.words = words.clone(),
                LiveEvent::Finished { stats, .. } => {
                    self.status = format!(
                        "Received {} KiB · playback draining",
                        stats.audio_bytes / 1024
                    );
                    self.last_stats = Some(stats.clone());
                    self.input = None;
                    self.id = None;
                }
                LiveEvent::Failed { message, .. } => {
                    self.status = message.clone();
                    self.input = None;
                    self.id = None;
                }
            }
        }
        events
    }
}
impl Drop for FishWsState {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn publish(mailbox: &Mailbox, active: &AtomicU64, expected: u64, event: LiveEvent) {
    if active.load(Ordering::Acquire) != expected {
        return;
    }
    if let Ok(mut queue) = mailbox.lock() {
        if active.load(Ordering::Acquire) != expected {
            return;
        }
        if matches!(event, LiveEvent::Timing { .. })
            && let Some((_, pending)) = queue.iter_mut().find(|(generation, event)| {
                *generation == expected && matches!(event, LiveEvent::Timing { .. })
            })
        {
            *pending = event;
            return;
        }
        if queue.len() < 8 {
            queue.push_back((expected, event));
        }
    }
}
fn validate(config: &VoiceConfig, timestamps: bool) -> Result<reqwest::Url, String> {
    if config.adapter != VoiceAdapter::FishHosted {
        return Err("Fish WebSocket requires the hosted Fish adapter".into());
    }
    if !matches!(
        config.model.as_str(),
        "s1" | "s2-pro" | "s2.1-pro" | "s2.1-pro-free" | "drama-3"
    ) {
        return Err(
            "Choose a documented Fish model; unknown models can fall back to a paid model".into(),
        );
    }
    if !config.speed.is_finite()
        || !(0.5..=2.0).contains(&config.speed)
        || !(100..=300).contains(&config.chunk_length)
        || !matches!(config.latency.as_str(), "normal" | "balanced" | "low")
    {
        return Err("Invalid Fish speed, chunk length or latency".into());
    }
    config.validate_hosted_tuning()?;
    if config.direction.chars().count() > 2000
        || config.direction.contains(['[', ']'])
        || (!config.direction.trim().is_empty() && config.model != "drama-3")
    {
        return Err(
            "Voice direction requires Drama 3 and at most 2000 characters without brackets".into(),
        );
    }
    if config.endpoint.len() > 8192 || config.reference_id.len() > 4096 {
        return Err("Fish endpoint or reference ID exceeds its size limit".into());
    }
    let mut url =
        reqwest::Url::parse(&config.endpoint).map_err(|_| "Invalid Fish WebSocket endpoint")?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use a Fish endpoint without credentials, query or fragment".into());
    }
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    let scheme = match url.scheme() {
        "https" | "wss" => "wss",
        "http" | "ws" if loopback => "ws",
        _ => return Err("Fish WebSocket requires WSS, or WS on localhost".into()),
    };
    url.set_scheme(scheme)
        .map_err(|_| "Invalid Fish WebSocket scheme")?;
    if url.host_str().is_none() {
        return Err("Fish WebSocket endpoint requires a host".into());
    }
    url.set_path(if timestamps {
        "/v1/tts/live/with-timestamp"
    } else {
        "/v1/tts/live"
    });
    Ok(url)
}
fn proxy_route(
    url: &reqwest::Url,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Route, String> {
    let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
    let port = url
        .port_or_known_default()
        .unwrap_or(if url.scheme() == "wss" { 443 } else { 80 });
    let get = |lower, upper| {
        lookup(lower)
            .filter(|v| !v.trim().is_empty())
            .or_else(|| lookup(upper).filter(|v| !v.trim().is_empty()))
    };
    if get("no_proxy", "NO_PROXY").is_some_and(|list| {
        list.split(',')
            .any(|item| no_proxy_match(item.trim(), host, port))
    }) {
        return Ok(Route::Direct);
    }
    let selected = if url.scheme() == "wss" {
        get("https_proxy", "HTTPS_PROXY")
    } else {
        get("http_proxy", "HTTP_PROXY")
    }
    .or_else(|| get("all_proxy", "ALL_PROXY"));
    let Some(selected) = selected else {
        return Ok(Route::Direct);
    };
    if selected.len() > 16 * 1024 {
        return Err("Fish WebSocket proxy configuration exceeds its size limit".into());
    }
    let proxy = reqwest::Url::parse(selected.trim())
        .map_err(|_| "Invalid Fish WebSocket proxy configuration")?;
    if proxy.scheme() != "http" {
        return Err("Fish WebSocket supports HTTP CONNECT proxies only; HTTPS and SOCKS proxies are unsupported".into());
    }
    if proxy.query().is_some() || proxy.fragment().is_some() || proxy.path() != "/" {
        return Err(
            "Fish WebSocket HTTP proxy must contain only host, port and optional credentials"
                .into(),
        );
    }
    let host = proxy
        .host_str()
        .ok_or("Fish WebSocket proxy requires a host")?
        .trim_matches(['[', ']'])
        .to_owned();
    let authorization = if !proxy.username().is_empty() || proxy.password().is_some() {
        let user = percent_decode(proxy.username())?;
        let password = percent_decode(proxy.password().unwrap_or(""))?;
        Some(format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
        ))
    } else {
        None
    };
    Ok(Route::HttpConnect {
        host,
        port: proxy.port().unwrap_or(80),
        authorization,
    })
}
fn percent_decode(value: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    let raw = value.as_bytes();
    let mut cursor = 0;
    while cursor < raw.len() {
        if raw[cursor] == b'%' {
            if cursor + 2 >= raw.len() {
                return Err("Invalid proxy credential encoding".into());
            }
            let text = std::str::from_utf8(&raw[cursor + 1..cursor + 3])
                .map_err(|_| "Invalid proxy credential encoding")?;
            bytes.push(
                u8::from_str_radix(text, 16).map_err(|_| "Invalid proxy credential encoding")?,
            );
            cursor += 3;
        } else {
            bytes.push(raw[cursor]);
            cursor += 1;
        }
    }
    String::from_utf8(bytes).map_err(|_| "Proxy credentials must contain UTF-8 text".into())
}
fn no_proxy_match(item: &str, host: &str, port: u16) -> bool {
    if item == "*" {
        return true;
    }
    if item.is_empty() {
        return false;
    }
    if let Some((network, prefix)) = item.split_once('/') {
        if let (Ok(network), Ok(address), Ok(prefix)) = (
            network.parse::<IpAddr>(),
            host.parse::<IpAddr>(),
            prefix.parse::<u32>(),
        ) {
            return match (network, address) {
                (IpAddr::V4(network), IpAddr::V4(address)) if prefix <= 32 => {
                    let mask = if prefix == 0 {
                        0
                    } else {
                        u32::MAX << (32 - prefix)
                    };
                    (u32::from(network) & mask) == (u32::from(address) & mask)
                }
                (IpAddr::V6(network), IpAddr::V6(address)) if prefix <= 128 => {
                    let mask = if prefix == 0 {
                        0
                    } else {
                        u128::MAX << (128 - prefix)
                    };
                    (u128::from(network) & mask) == (u128::from(address) & mask)
                }
                _ => false,
            };
        }
        return false;
    }
    let (item, required_port) = if item.starts_with('[') {
        match item.find(']') {
            Some(end) => (
                &item[1..end],
                item.get(end + 1..)
                    .and_then(|tail| tail.strip_prefix(':'))
                    .and_then(|port| port.parse::<u16>().ok()),
            ),
            None => return false,
        }
    } else if item.matches(':').count() == 1 {
        match item.rsplit_once(':') {
            Some((host, port)) if port.parse::<u16>().is_ok() => (host, port.parse().ok()),
            _ => (item, None),
        }
    } else {
        (item, None)
    };
    if required_port.is_some_and(|required| port != required) {
        return false;
    }
    if let (Ok(left), Ok(right)) = (item.parse::<IpAddr>(), host.parse::<IpAddr>()) {
        return left == right;
    }
    let item = item
        .trim_start_matches("*.")
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    !item.is_empty()
        && (host == item
            || host
                .strip_suffix(&item)
                .is_some_and(|prefix| prefix.ends_with('.')))
}

fn map(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (Value::from(key), value))
            .collect(),
    )
}
fn encoded(value: &Value) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, value)
        .map_err(|_| "Could not encode Fish WebSocket message")?;
    Ok(bytes)
}
fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value
        .as_map()?
        .iter()
        .find(|(name, _)| name.as_str() == Some(key))
        .map(|(_, value)| value)
}
fn decode(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_MESSAGE {
        return Err("Fish WebSocket message exceeds 2 MiB".into());
    }
    let mut reader = Cursor::new(bytes);
    let value = rmpv::decode::read_value_with_max_depth(&mut reader, 32)
        .map_err(|_| "Invalid Fish MessagePack message")?;
    if reader.position() != bytes.len() as u64 || value.as_map().is_none() {
        return Err("Invalid Fish MessagePack envelope".into());
    }
    Ok(value)
}
fn check(active: &AtomicU64, expected: u64, budget: &StreamBudget) -> Result<(), String> {
    if active.load(Ordering::Acquire) != expected {
        return Err("Speech cancelled".into());
    }
    budget.check()?;
    Ok(())
}
fn authority(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}
fn connect_tcp(
    host: &str,
    port: u16,
    active: &AtomicU64,
    expected: u64,
    budget: &StreamBudget,
) -> Result<TcpStream, String> {
    let connect_deadline = Instant::now() + CONNECT_TIMEOUT;
    let addresses: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|_| "Could not resolve Fish WebSocket connection host")?
        .take(8)
        .collect();
    for (index, address) in addresses.iter().enumerate() {
        check(active, expected, budget)?;
        let mut timeout = connect_deadline
            .saturating_duration_since(Instant::now())
            .min(budget.remaining());
        if index + 1 < addresses.len() {
            timeout = timeout.min(ADDRESS_TIMEOUT);
        }
        if timeout.is_zero() {
            return Err("Fish WebSocket connection timed out".into());
        }
        if let Ok(socket) =
            TcpStream::connect_timeout(address, timeout.max(Duration::from_millis(1)))
        {
            return Ok(socket);
        }
    }
    Err("Could not connect Fish WebSocket socket".into())
}
fn tunnel(
    socket: &mut TcpStream,
    target: &str,
    authorization: Option<&str>,
    active: &AtomicU64,
    expected: u64,
    budget: &StreamBudget,
) -> Result<(), String> {
    let mut request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some(auth) = authorization {
        request.push_str("Proxy-Authorization: ");
        request.push_str(auth);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    socket
        .write_all(request.as_bytes())
        .map_err(|_| "Fish HTTP CONNECT proxy write failed")?;
    let mut header = Vec::new();
    let mut byte = [0];
    while !header.ends_with(b"\r\n\r\n") {
        check(active, expected, budget)?;
        if header.len() >= 16 * 1024 {
            return Err("Fish HTTP CONNECT proxy header exceeds its size limit".into());
        }
        socket
            .read_exact(&mut byte)
            .map_err(|_| "Fish HTTP CONNECT proxy read failed")?;
        header.push(byte[0]);
    }
    let header =
        std::str::from_utf8(&header).map_err(|_| "Invalid Fish HTTP CONNECT proxy response")?;
    let status = header
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or("Invalid Fish HTTP CONNECT proxy response")?;
    if status != 200 {
        return Err(format!("Fish HTTP CONNECT proxy returned HTTP {status}"));
    }
    Ok(())
}
fn connect(
    request: &Request,
    active: &AtomicU64,
    control: &SocketControl,
    budget: &StreamBudget,
) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, String> {
    let host = request
        .url
        .host_str()
        .ok_or("Fish WebSocket endpoint requires a host")?
        .trim_matches(['[', ']']);
    let port = request
        .url
        .port_or_known_default()
        .unwrap_or(if request.url.scheme() == "wss" {
            443
        } else {
            80
        });
    let mut socket = match &request.route {
        Route::Direct => connect_tcp(host, port, active, request.generation, budget)?,
        Route::HttpConnect { host, port, .. } => {
            connect_tcp(host, *port, active, request.generation, budget)?
        }
    };
    socket
        .set_nodelay(true)
        .map_err(|_| "Could not configure Fish WebSocket socket")?;
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|_| "Could not configure Fish WebSocket socket")?;
    socket
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|_| "Could not configure Fish WebSocket socket")?;
    let cancel_handle = socket
        .try_clone()
        .map_err(|_| "Could not prepare Fish WebSocket cancellation")?;
    {
        let mut stored = control.lock().map_err(|_| "Fish socket control failed")?;
        check(active, request.generation, budget)?;
        *stored = Some((request.generation, cancel_handle));
    }
    if let Route::HttpConnect { authorization, .. } = &request.route {
        tunnel(
            &mut socket,
            &authority(host, port),
            authorization.as_deref(),
            active,
            request.generation,
            budget,
        )?;
    }
    check(active, request.generation, budget)?;
    let mut upgrade = request
        .url
        .as_str()
        .into_client_request()
        .map_err(|_| "Invalid Fish WebSocket upgrade request")?;
    upgrade.headers_mut().insert(
        "Authorization",
        tungstenite::http::HeaderValue::from_str(&format!("Bearer {}", request.key))
            .map_err(|_| "Invalid Fish API key header")?,
    );
    upgrade.headers_mut().insert(
        "model",
        tungstenite::http::HeaderValue::from_str(&request.config.model)
            .map_err(|_| "Invalid Fish model header")?,
    );
    let config = WebSocketConfig::default()
        .read_buffer_size(8192)
        .write_buffer_size(0)
        .max_write_buffer_size(128 * 1024)
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE));
    let (mut ws, _) = tungstenite::client_tls_with_config(upgrade, socket, Some(config), None)
        .map_err(|_| "Fish WebSocket upgrade or TLS handshake failed")?;
    let mut body = map([
        ("text", Value::from("")),
        ("format", Value::from("wav")),
        ("normalize", Value::Boolean(true)),
        ("latency", Value::from(request.config.latency.as_str())),
        ("chunk_length", Value::from(request.config.chunk_length)),
        (
            "min_chunk_length",
            Value::from(request.config.min_chunk_length),
        ),
        (
            "condition_on_previous_chunks",
            Value::Boolean(request.config.condition_on_previous_chunks),
        ),
        (
            "prosody",
            map([("speed", Value::F64(request.config.speed as f64))]),
        ),
    ]);
    if !request.config.reference_id.trim().is_empty()
        && let Value::Map(entries) = &mut body
    {
        entries.push((
            Value::from("reference_id"),
            Value::from(request.config.reference_id.trim()),
        ));
    }
    let start = map([("event", Value::from("start")), ("request", body)]);
    ws.send(Message::Binary(encoded(&start)?.into()))
        .map_err(|_| "Could not send Fish WebSocket start event")?;
    let socket = match ws.get_mut() {
        MaybeTlsStream::Plain(socket) => socket,
        MaybeTlsStream::Rustls(stream) => &mut stream.sock,
        _ => return Err("Unsupported Fish WebSocket TLS backend".into()),
    };
    socket
        .set_nonblocking(true)
        .map_err(|_| "Could not configure Fish WebSocket polling")?;
    Ok(ws)
}

/// Receive-only integration transport using the same explicit proxy routing as
/// native speech. No Fish headers, credentials, or initialization are sent.
/// A selected proxy failure never falls back to a direct connection.
pub(crate) fn observation_socket(
    url: &reqwest::Url,
) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, String> {
    observation_socket_with_env(url, |name| std::env::var(name).ok())
}

fn observation_socket_with_env(
    url: &reqwest::Url,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, String> {
    observation_socket_configured(url, lookup, None)
}

fn observation_socket_configured(
    url: &reqwest::Url,
    lookup: impl Fn(&str) -> Option<String>,
    connector: Option<tungstenite::Connector>,
) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, String> {
    let route = proxy_route(url, lookup).map_err(|_| "Invalid integration proxy configuration")?;
    let host = url
        .host_str()
        .ok_or("Missing integration socket host")?
        .trim_matches(['[', ']']);
    let port = url.port_or_known_default().ok_or("Missing socket port")?;
    let active = AtomicU64::new(1);
    let budget = StreamBudget::default();
    let mut socket = match &route {
        Route::Direct => connect_tcp(host, port, &active, 1, &budget),
        Route::HttpConnect { host, port, .. } => connect_tcp(host, *port, &active, 1, &budget),
    }
    .map_err(|_| "Integration socket connection failed")?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .and_then(|()| socket.set_write_timeout(Some(Duration::from_secs(5))))
        .map_err(|_| "Cannot configure integration socket")?;
    if let Route::HttpConnect { authorization, .. } = &route {
        tunnel(
            &mut socket,
            &authority(host, port),
            authorization.as_deref(),
            &active,
            1,
            &budget,
        )
        .map_err(|_| "Integration proxy tunnel failed")?;
    }
    let config = WebSocketConfig::default()
        .write_buffer_size(0)
        .max_write_buffer_size(128 * 1024)
        .max_message_size(Some(64 * 1024))
        .max_frame_size(Some(64 * 1024));
    let (mut ws, _) =
        tungstenite::client_tls_with_config(url.as_str(), socket, Some(config), connector)
            .map_err(|_| "Integration WebSocket handshake failed")?;
    let socket = match ws.get_mut() {
        MaybeTlsStream::Plain(socket) => socket,
        MaybeTlsStream::Rustls(socket) => &mut socket.sock,
        _ => return Err("Unsupported integration TLS transport".into()),
    };
    socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|_| "Cannot configure integration socket polling")?;
    Ok(ws)
}
/// Test-only fixture trust is scoped to one local connector. Production callers
/// keep the normal platform/webpki roots through observation_socket_with_env.
#[cfg(test)]
pub(crate) fn observation_socket_fixture_tls(
    url: &reqwest::Url,
    connector: Option<tungstenite::Connector>,
) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, String> {
    assert_eq!(url.scheme(), "wss");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    observation_socket_configured(url, |_| None, connector)
}

struct WsReader {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    /// Session key, used only to mask provider error text.
    key: String,
    input: Arc<Mutex<TextQueue>>,
    active: Arc<AtomicU64>,
    expected: u64,
    id: u64,
    mailbox: Mailbox,
    budget: StreamBudget,
    started: Instant,
    timestamps: bool,
    outgoing: Option<Command>,
    stop_sent: bool,
    finished: bool,
    audio: Vec<u8>,
    cursor: usize,
    wire: usize,
    stats: LiveStats,
    snapshots: BTreeMap<u64, Vec<WordTiming>>,
}
fn would_block(error: &tungstenite::Error) -> bool {
    matches!(error,tungstenite::Error::Io(error) if error.kind()==io::ErrorKind::WouldBlock)
}
impl WsReader {
    fn pump(&mut self) -> Result<(), String> {
        for _ in 0..8 {
            check(&self.active, self.expected, &self.budget)?;
            if self.outgoing.is_none() {
                self.outgoing = self
                    .input
                    .lock()
                    .map_err(|_| "Fish input queue failed")?
                    .commands
                    .pop_front();
            }
            let Some(command) = &self.outgoing else {
                break;
            };
            let value = match command {
                Command::Text(text) => map([
                    ("event", Value::from("text")),
                    ("text", Value::from(text.as_str())),
                ]),
                Command::Flush => map([("event", Value::from("flush"))]),
                Command::Stop => map([("event", Value::from("stop"))]),
            };
            match self.ws.write(Message::Binary(encoded(&value)?.into())) {
                Ok(()) => {}
                Err(error) if would_block(&error) => {}
                Err(tungstenite::Error::WriteBufferFull(_)) => break,
                Err(_) => return Err("Fish WebSocket send failed".into()),
            }
            let command = self.outgoing.take().unwrap();
            let elapsed = self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            match &command {
                Command::Text(_) => {
                    self.stats.first_text_written_ms.get_or_insert(elapsed);
                }
                Command::Flush => {
                    self.stats.first_flush_written_ms.get_or_insert(elapsed);
                }
                Command::Stop => {}
            }
            if matches!(command, Command::Stop) {
                self.stop_sent = true;
            }
            let mut queue = self.input.lock().map_err(|_| "Fish input queue failed")?;
            queue.bytes = queue.bytes.saturating_sub(command.bytes());
        }
        match self.ws.flush() {
            Ok(()) => Ok(()),
            Err(error) if would_block(&error) => Ok(()),
            Err(_) => Err("Fish WebSocket flush failed".into()),
        }
    }
    fn process(&mut self, value: Value) -> Result<(), String> {
        let event = field(&value, "event")
            .and_then(Value::as_str)
            .ok_or("Fish WebSocket event type is missing")?;
        match event {
            "audio" => {
                let audio = match field(&value, "audio") {
                    Some(Value::Binary(audio)) => audio.clone(),
                    _ => return Err("Fish WebSocket audio must be MessagePack binary bytes".into()),
                };
                self.stats.frames += 1;
                self.stats.audio_bytes += audio.len();
                if self.stats.frames > 100_000 || self.stats.audio_bytes > MAX_AUDIO {
                    return Err("Fish WebSocket audio exceeds its size limit".into());
                }
                if !audio.is_empty() && self.stats.first_audio_ms.is_none() {
                    self.stats.first_audio_ms =
                        Some(self.started.elapsed().as_millis().min(u64::MAX as u128) as u64);
                }
                self.audio = audio;
                self.cursor = 0;
                if self.timestamps {
                    self.alignment(&value)?;
                }
            }
            "finish" => match field(&value, "reason").and_then(Value::as_str) {
                Some("stop") if self.stop_sent => self.finished = true,
                Some("error") => {
                    return Err("Fish provider reported a WebSocket synthesis error".into());
                }
                _ => return Err("Fish WebSocket finished unexpectedly".into()),
            },
            // Terminal: the server closes the socket; never retry or switch models.
            "error" => return Err(provider_error(&value, &self.key)),
            _ => return Err("Unknown Fish WebSocket event type".into()),
        }
        Ok(())
    }
    fn alignment(&mut self, value: &Value) -> Result<(), String> {
        let Some(alignment) = field(value, "alignment").filter(|value| !value.is_nil()) else {
            return Ok(());
        };
        let seq = field(value, "chunk_seq")
            .and_then(Value::as_u64)
            .filter(|seq| *seq < 4096)
            .ok_or("Invalid Fish timestamp chunk sequence")?;
        let offset = field(value, "chunk_audio_offset_sec")
            .and_then(number)
            .filter(|offset| offset.is_finite() && (0.0..=3600.0).contains(offset))
            .ok_or("Invalid Fish timestamp offset")?;
        let duration = field(alignment, "audio_duration")
            .and_then(number)
            .ok_or("Missing Fish alignment duration")?;
        if !duration.is_finite() || !(0.0..=3600.0).contains(&duration) {
            return Err("Invalid Fish alignment duration".into());
        }
        let segments = field(alignment, "segments")
            .and_then(Value::as_array)
            .ok_or("Invalid Fish alignment segments")?;
        if segments.len() > MAX_WORDS {
            return Err("Fish word alignment exceeds its size limit".into());
        }
        let mut words = Vec::with_capacity(segments.len());
        for segment in segments {
            let text = field(segment, "text")
                .and_then(Value::as_str)
                .filter(|text| text.len() <= 2048)
                .ok_or("Invalid Fish aligned word")?;
            let start = field(segment, "start")
                .and_then(number)
                .ok_or("Invalid Fish word start time")?;
            let end = field(segment, "end")
                .and_then(number)
                .ok_or("Invalid Fish word end time")?;
            if !start.is_finite() || !end.is_finite() || start < 0.0 || end < start || end > 3600.0
            {
                return Err("Invalid Fish word timestamp bounds".into());
            }
            words.push(WordTiming {
                text: text.into(),
                start: start + offset,
                end: end + offset,
            });
        }
        let mut count = words.len();
        let mut bytes = words.iter().map(|word| word.text.len()).sum::<usize>();
        for (other, snapshot) in &self.snapshots {
            if *other != seq {
                count += snapshot.len();
                bytes += snapshot.iter().map(|word| word.text.len()).sum::<usize>();
            }
        }
        if count > MAX_WORDS || bytes > 256 * 1024 {
            return Err("Fish word alignment exceeds its total size limit".into());
        }
        self.snapshots.insert(seq, words);
        let mut words: Vec<_> = self.snapshots.values().flatten().cloned().collect();
        words.sort_by(|a, b| a.start.total_cmp(&b.start));
        publish(
            &self.mailbox,
            &self.active,
            self.expected,
            LiveEvent::Timing { id: self.id, words },
        );
        Ok(())
    }
}
/// Untrusted provider detail: control characters dropped, the session key and
/// secret-shaped words masked before the length cap, then bounded to 200 chars.
fn provider_error(value: &Value, key: &str) -> String {
    let raw: String = field(value, "error")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(4096)
        .collect();
    let detail: String = crate::mcp::redact(raw.trim(), Some(key))
        .chars()
        .take(200)
        .collect();
    let mut message = "Fish provider reported a WebSocket error".to_owned();
    if !detail.trim().is_empty() {
        message = format!("{message}: {}", detail.trim());
    }
    if let Some(limit) = field(value, "max_concurrency")
        .and_then(Value::as_u64)
        .filter(|limit| *limit <= 1_000_000)
    {
        message = format!("{message} (account max concurrency {limit})");
    }
    message
}
fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_u64().map(|value| value as f64))
}
/// Only controlled local messages or redacted provider detail belong here.
fn safe_stream_error(message: impl Into<String>) -> io::Error {
    io::Error::other(crate::voice_stream::SafeStreamError(message.into()))
}

/// Preserve already typed source errors; turn only an ordinary short read into
/// a local framing diagnostic. Raw network/IO detail stays opaque to the decoder.
fn wav_exact<R: Read>(reader: &mut R, bytes: &mut [u8], context: &'static str) -> io::Result<()> {
    reader.read_exact(bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof
            && !error
                .get_ref()
                .is_some_and(|inner| inner.is::<crate::voice_stream::SafeStreamError>())
        {
            safe_stream_error(context)
        } else {
            error
        }
    })
}
impl Read for WsReader {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        if target.is_empty() {
            return Ok(0);
        }
        loop {
            check(&self.active, self.expected, &self.budget).map_err(safe_stream_error)?;
            // Feed text/control input between decoder reads even while a large
            // audio message still has buffered bytes to deliver.
            if !self.finished
                && let Err(send_error) = self.pump()
            {
                // A peer may send its terminal error while we write the last
                // text/stop. Prefer a buffered, sanitized provider reason over
                // a generic broken pipe; never retry or wait for more input.
                for _ in 0..8 {
                    match self.ws.read() {
                        Ok(Message::Binary(bytes)) => {
                            if let Ok(value) = decode(&bytes)
                                && field(&value, "event").and_then(Value::as_str) == Some("error")
                            {
                                return Err(safe_stream_error(provider_error(&value, &self.key)));
                            }
                        }
                        Ok(Message::Ping(_) | Message::Pong(_)) => {}
                        _ => break,
                    }
                }
                return Err(safe_stream_error(send_error));
            }
            if self.cursor < self.audio.len() {
                let count = target.len().min(self.audio.len() - self.cursor);
                target[..count].copy_from_slice(&self.audio[self.cursor..self.cursor + count]);
                self.cursor += count;
                return Ok(count);
            }
            if self.finished {
                return Ok(0);
            }
            match self.ws.read() {
                Ok(Message::Binary(bytes)) => {
                    self.wire += bytes.len();
                    if self.wire > MAX_WIRE {
                        return Err(safe_stream_error("Fish WebSocket wire data exceeds 64 MiB"));
                    }
                    let value = decode(&bytes).map_err(safe_stream_error)?;
                    self.process(value).map_err(safe_stream_error)?;
                }
                Ok(Message::Ping(_) | Message::Pong(_)) => {}
                Ok(Message::Close(_))
                | Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    return Err(safe_stream_error(
                        "Fish WebSocket closed before the finish event",
                    ));
                }
                Ok(_) => {
                    return Err(safe_stream_error(
                        "Fish WebSocket requires binary MessagePack messages",
                    ));
                }
                Err(error) if would_block(&error) => std::thread::sleep(Duration::from_millis(2)),
                Err(_) => return Err(safe_stream_error("Fish WebSocket receive failed")),
            }
        }
    }
}

/// One continuous WAV for the existing decoder. Finite per-segment WAV headers
/// are checked against the first format and removed. Unknown-length data is
/// concatenated as raw continuation; arbitrary byte fragments are never decoded alone.
struct JoinedWav<R> {
    source: R,
    header: Vec<u8>,
    cursor: usize,
    format: Option<Vec<u8>>,
    riff_length: u32,
    data_length: usize,
    data_received: usize,
    remaining: Option<usize>,
    need_header: bool,
    padding: bool,
    eof: bool,
}
impl<R: Read> JoinedWav<R> {
    fn new(source: R) -> Self {
        Self {
            source,
            header: Vec::new(),
            cursor: 0,
            format: None,
            riff_length: 0,
            data_length: 0,
            data_received: 0,
            remaining: None,
            need_header: true,
            padding: false,
            eof: false,
        }
    }
    fn header(&mut self) -> io::Result<bool> {
        let mut riff = [0; 12];
        let n = self.source.read(&mut riff[..1])?;
        if n == 0 {
            return Ok(false);
        }
        wav_exact(
            &mut self.source,
            &mut riff[1..],
            "Truncated Fish WAV header",
        )?;
        if &riff[..4] != b"RIFF" || &riff[8..] != b"WAVE" {
            return Err(safe_stream_error("Fish WebSocket requires RIFF WAV audio"));
        }
        self.riff_length = u32::from_le_bytes(riff[4..8].try_into().unwrap());
        let mut format = None;
        let mut scanned = 12usize;
        let length = loop {
            let mut chunk = [0; 8];
            wav_exact(
                &mut self.source,
                &mut chunk,
                "Truncated Fish WAV chunk header",
            )?;
            let length = u32::from_le_bytes(chunk[4..].try_into().unwrap()) as usize;
            scanned += 8;
            if &chunk[..4] == b"data" {
                break length;
            }
            if length > 64 * 1024 || scanned + length > 64 * 1024 {
                return Err(safe_stream_error("Fish WAV header exceeds 64 KiB"));
            }
            let mut bytes = vec![0; length];
            wav_exact(
                &mut self.source,
                &mut bytes,
                "Truncated Fish WAV header chunk",
            )?;
            scanned += length;
            if length % 2 == 1 {
                let mut pad = [0];
                wav_exact(
                    &mut self.source,
                    &mut pad,
                    "Truncated Fish WAV chunk padding",
                )?;
                scanned += 1;
            }
            if &chunk[..4] == b"fmt " {
                if format.is_some() || !(16..=4096).contains(&length) {
                    return Err(safe_stream_error("Invalid Fish WAV format header"));
                }
                format = Some(bytes);
            }
        };
        let format =
            format.ok_or_else(|| safe_stream_error("Fish WAV data precedes its format"))?;
        // Hosted Fish's live encoder emits this exact 44-byte streaming header:
        // data = 2^32 - 256, RIFF = data + 36. It cannot be a finite clip under
        // our 32 MiB limit. Recognize only the observed pair/layout here, rather
        // than interpreting arbitrary large finite lengths as streaming audio.
        // WsReader still requires finish(reason=stop) before it returns EOF.
        let hosted_placeholder =
            crate::voice_stream::fish_unknown_wav_length(self.riff_length, length as u32, scanned)
                && format.len() == 16;
        let unknown = length == 0 || length == u32::MAX as usize || hosted_placeholder;
        let block = u16::from_le_bytes(format[12..14].try_into().unwrap()) as usize;
        if block == 0 || (!unknown && length % block != 0) {
            return Err(safe_stream_error(
                "Fish WAV data ends inside a sample frame",
            ));
        }
        if let Some(first) = &self.format {
            if *first != format {
                return Err(safe_stream_error(
                    "Fish WAV segment format changed during the session",
                ));
            }
        } else {
            self.header = b"RIFF".to_vec();
            self.header.extend(u32::MAX.to_le_bytes());
            self.header.extend(b"WAVEfmt ");
            self.header.extend((format.len() as u32).to_le_bytes());
            self.header.extend(&format);
            if format.len() % 2 == 1 {
                self.header.push(0);
            }
            self.header.extend(b"data");
            self.header.extend(u32::MAX.to_le_bytes());
            self.format = Some(format);
        }
        self.remaining = if unknown { None } else { Some(length) };
        self.data_length = length;
        self.data_received = 0;
        self.padding = length % 2 == 1 && !unknown;
        self.need_header = false;
        Ok(true)
    }
}
impl<R: Read> Read for JoinedWav<R> {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        if target.is_empty() {
            return Ok(0);
        }
        loop {
            if self.cursor < self.header.len() {
                let count = target.len().min(self.header.len() - self.cursor);
                target[..count].copy_from_slice(&self.header[self.cursor..self.cursor + count]);
                self.cursor += count;
                return Ok(count);
            }
            if self.eof {
                return Ok(0);
            }
            if self.need_header {
                if !self.header()? {
                    self.eof = true;
                    return Ok(0);
                }
                continue;
            }
            if self.remaining == Some(0) {
                if self.padding {
                    let mut byte = [0];
                    wav_exact(
                        &mut self.source,
                        &mut byte,
                        "Truncated Fish WAV segment padding",
                    )?;
                    self.padding = false;
                }
                self.need_header = true;
                continue;
            }
            let limit = target.len().min(self.remaining.unwrap_or(target.len()));
            let count = self.source.read(&mut target[..limit])?;
            if count == 0 {
                if self.remaining.is_some_and(|left| left > 0) {
                    return Err(safe_stream_error(format!(
                        "Truncated Fish WAV segment (RIFF size {} [0x{:08x}], data declared {} [0x{:08x}], received {} bytes)",
                        self.riff_length,
                        self.riff_length,
                        self.data_length,
                        self.data_length,
                        self.data_received
                    )));
                }
                self.eof = true;
                return Ok(0);
            }
            if let Some(left) = self.remaining.as_mut() {
                *left -= count;
            }
            self.data_received += count;
            return Ok(count);
        }
    }
}
fn run(
    request: Request,
    active: Arc<AtomicU64>,
    audio: Arc<AtomicU64>,
    mailbox: Mailbox,
    control: SocketControl,
) -> Result<LiveStats, String> {
    let budget = StreamBudget::default();
    let started = budget.started();
    check(&active, request.generation, &budget)?;
    let ws = connect(&request, &active, &control, &budget)?;
    let expected = request.generation;
    let id = request.id;
    let mut source = WsReader {
        ws,
        key: request.key.clone(),
        input: request.input,
        active: active.clone(),
        expected,
        id,
        mailbox: mailbox.clone(),
        budget: budget.clone(),
        started,
        timestamps: request.timestamps,
        outgoing: None,
        stop_sent: false,
        finished: false,
        audio: Vec::new(),
        cursor: 0,
        wire: 0,
        stats: LiveStats {
            connection_ready_ms: Some(
                started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
            ),
            ..Default::default()
        },
        snapshots: BTreeMap::new(),
    };
    let mut joined = JoinedWav::new(&mut source);
    receive_wav_with_budget(&mut joined, audio, expected, &budget, |audio| {
        publish(
            &mailbox,
            &active,
            expected,
            LiveEvent::Playing { id, audio },
        )
    })
    .map_err(|error| format!("Fish WebSocket WAV: {error}"))?;
    if !source.finished {
        return Err("Fish WebSocket ended without a finish event".into());
    }
    Ok(source.stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice_stream::receive_wav;
    use bevy::audio::Decodable;
    use std::{collections::HashMap, net::TcpListener, sync::mpsc};
    fn wav(samples: usize, unknown: bool) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend(
            if unknown {
                u32::MAX
            } else {
                36 + samples as u32 * 2
            }
            .to_le_bytes(),
        );
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(8000u32.to_le_bytes());
        bytes.extend(16000u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(
            if unknown {
                u32::MAX
            } else {
                samples as u32 * 2
            }
            .to_le_bytes(),
        );
        for _ in 0..samples {
            bytes.extend(8192i16.to_le_bytes());
        }
        bytes
    }
    struct Fragmented(Cursor<Vec<u8>>);
    impl Read for Fragmented {
        fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
            let limit = target.len().min(1);
            self.0.read(&mut target[..limit])
        }
    }
    fn audio(bytes: &[u8], seq: u64, offset: f64, words: Option<Vec<(&str, f64, f64)>>) -> Value {
        let alignment = words.map_or(Value::Nil, |words| {
            map([
                ("audio_duration", Value::F64(0.5)),
                (
                    "segments",
                    Value::Array(
                        words
                            .into_iter()
                            .map(|(text, start, end)| {
                                map([
                                    ("text", Value::from(text)),
                                    ("start", Value::F64(start)),
                                    ("end", Value::F64(end)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])
        });
        map([
            ("event", Value::from("audio")),
            ("audio", Value::Binary(bytes.into())),
            ("content", Value::from("Hello world")),
            ("chunk_seq", Value::from(seq)),
            ("chunk_audio_offset_sec", Value::F64(offset)),
            ("alignment", alignment),
        ])
    }
    fn send(socket: &mut WebSocket<TcpStream>, value: Value) {
        socket
            .send(Message::Binary(encoded(&value).unwrap().into()))
            .unwrap();
    }
    fn receive(socket: &mut WebSocket<TcpStream>) -> Value {
        loop {
            match socket.read().unwrap() {
                Message::Binary(bytes) => return decode(&bytes).unwrap(),
                Message::Ping(_) | Message::Pong(_) => {}
                other => panic!("Unexpected client message: {other:?}"),
            }
        }
    }
    fn endpoint(listener: &TcpListener) -> String {
        format!("http://{}/v1/tts", listener.local_addr().unwrap())
    }
    fn config(endpoint: String) -> VoiceConfig {
        VoiceConfig {
            endpoint,
            reference_id: "test-speaker".into(),
            ..Default::default()
        }
    }
    fn open(state: &mut FishWsState, id: u64, config: &VoiceConfig) {
        state
            .open_with_env(id, config, Some("offline-test-key".into()), |_| None)
            .unwrap();
    }
    fn socket(listener: TcpListener) -> TcpStream {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        socket
    }
    #[test]
    fn joins_fragmented_finite_wav_segments_and_rejects_changed_formats() {
        let first = wav(2, false);
        let second = wav(3, false);
        let mut stream = first.clone();
        stream.extend(&second);
        let mut joined = JoinedWav::new(Fragmented(Cursor::new(stream)));
        let mut playable = None;
        receive_wav(
            &mut joined,
            Arc::new(AtomicU64::new(1)),
            1,
            Instant::now() + Duration::from_secs(2),
            |audio| playable = Some(audio),
        )
        .unwrap();
        assert_eq!(
            playable.unwrap().decoder().collect::<Vec<_>>(),
            vec![0.25; 5]
        );
        let mut changed = second;
        changed[24..28].copy_from_slice(&16000u32.to_le_bytes());
        let mut stream = first;
        stream.extend(changed);
        assert!(
            JoinedWav::new(Cursor::new(stream))
                .read_to_end(&mut Vec::new())
                .is_err()
        );
        let mut incomplete = wav(2, false);
        incomplete.pop();
        assert!(
            JoinedWav::new(Cursor::new(incomplete))
                .read_to_end(&mut Vec::new())
                .is_err()
        );
    }
    fn hosted_placeholder_wav(samples: usize) -> Vec<u8> {
        let mut bytes = wav(samples, false);
        bytes[4..8].copy_from_slice(&0xffff_ff24u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&0xffff_ff00u32.to_le_bytes());
        bytes
    }
    #[test]
    fn actual_hosted_wav_placeholder_decodes_fragmented_pcm_and_rejects_partial_frames() {
        let mut played = None;
        receive_wav(
            JoinedWav::new(Fragmented(Cursor::new(hosted_placeholder_wav(2)))),
            Arc::new(AtomicU64::new(1)),
            1,
            Instant::now() + Duration::from_secs(1),
            |audio| played = Some(audio),
        )
        .unwrap();
        assert_eq!(played.unwrap().decoder().collect::<Vec<_>>(), vec![0.25; 2]);

        let mut partial = hosted_placeholder_wav(2);
        partial.pop();
        assert_eq!(
            receive_wav(
                JoinedWav::new(Fragmented(Cursor::new(partial))),
                Arc::new(AtomicU64::new(1)),
                1,
                Instant::now() + Duration::from_secs(1),
                |_| panic!("A partial frame must not start short-clip playback"),
            )
            .unwrap_err(),
            "Truncated WAV sample frame"
        );

        // Nearby finite declarations, including a mismatched RIFF/data pair,
        // must still reject truncation rather than gaining sentinel semantics.
        for (riff, data) in [(0xffff_ff25u32, 0xffff_ff00u32), (0xffff_ff24, 0xffff_fefe)] {
            let mut bytes = hosted_placeholder_wav(2);
            bytes[4..8].copy_from_slice(&riff.to_le_bytes());
            bytes[40..44].copy_from_slice(&data.to_le_bytes());
            let error = JoinedWav::new(Cursor::new(bytes))
                .read_to_end(&mut Vec::new())
                .unwrap_err();
            assert!(error.to_string().starts_with("Truncated Fish WAV segment"));
        }
    }
    #[test]
    fn hosted_wav_placeholder_requires_normal_websocket_finish() {
        for normal_finish in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let config = config(endpoint(&listener));
            let task = std::thread::spawn(move || {
                let mut ws = tungstenite::accept(socket(listener)).unwrap();
                receive(&mut ws); // start
                loop {
                    let event = receive(&mut ws);
                    if field(&event, "event").and_then(Value::as_str) == Some("stop") {
                        break;
                    }
                }
                send(&mut ws, audio(&hosted_placeholder_wav(2), 0, 0.0, None));
                if normal_finish {
                    send(
                        &mut ws,
                        map([
                            ("event", Value::from("finish")),
                            ("reason", Value::from("stop")),
                        ]),
                    );
                } else {
                    ws.close(None).unwrap();
                }
            });
            let mut state = FishWsState::default();
            open(&mut state, 1, &config);
            state.push_text("Hello").unwrap();
            state.finish().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut result = None;
            while result.is_none() && Instant::now() < deadline {
                for event in state.poll() {
                    match event {
                        LiveEvent::Finished { .. } => result = Some(Ok(())),
                        LiveEvent::Failed { message, .. } => result = Some(Err(message)),
                        _ => {}
                    }
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            let result = result.expect("Offline WebSocket test did not finish");
            if normal_finish {
                result.unwrap();
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .contains("closed before the finish event")
                );
            }
            task.join().unwrap();
        }
    }
    #[test]
    fn joined_wav_preserves_controlled_framing_diagnostics() {
        let mut truncated_data = wav(2, false);
        truncated_data.pop();
        for (bytes, expected) in [
            (vec![0; 12], "Fish WebSocket requires RIFF WAV audio"),
            (b"RI".to_vec(), "Truncated Fish WAV header"),
            (
                wav(2, false)[..16].to_vec(),
                "Truncated Fish WAV chunk header",
            ),
            (
                wav(2, false)[..28].to_vec(),
                "Truncated Fish WAV header chunk",
            ),
            (
                truncated_data,
                "Truncated Fish WAV segment (RIFF size 40 [0x00000028], data declared 4 [0x00000004], received 3 bytes)",
            ),
        ] {
            let error = receive_wav(
                JoinedWav::new(Fragmented(Cursor::new(bytes))),
                Arc::new(AtomicU64::new(1)),
                1,
                Instant::now() + Duration::from_secs(1),
                |_| panic!("Invalid short WAV must not start playback"),
            )
            .unwrap_err();
            assert_eq!(error, expected);
        }
    }
    #[test]
    fn joined_wav_preserves_typed_source_errors_and_hides_raw_io_detail() {
        struct FailureAfterR {
            initial: bool,
            error: Option<io::Error>,
        }
        impl Read for FailureAfterR {
            fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
                if !self.initial {
                    target[0] = b'R';
                    self.initial = true;
                    return Ok(1);
                }
                Err(self.error.take().unwrap())
            }
        }
        for (error, expected) in [
            (
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    crate::voice_stream::SafeStreamError("Sanitized provider reason".into()),
                ),
                "Sanitized provider reason",
            ),
            (
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "secret-key-and-proxy-address",
                ),
                "Could not receive speech stream",
            ),
            (
                io::Error::new(io::ErrorKind::UnexpectedEof, "secret-key-and-proxy-address"),
                "Truncated Fish WAV header",
            ),
        ] {
            let error = receive_wav(
                JoinedWav::new(FailureAfterR {
                    initial: false,
                    error: Some(error),
                }),
                Arc::new(AtomicU64::new(1)),
                1,
                Instant::now() + Duration::from_secs(1),
                |_| panic!("Failed header must not start playback"),
            )
            .unwrap_err();
            assert_eq!(error, expected);
        }
    }
    #[test]
    // Tungstenite's handshake callback requires the large HTTP error response type.
    #[allow(clippy::result_large_err)]
    fn offline_ws_binary_contract_ping_timestamps_and_audio_before_stop() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = config(endpoint(&listener));
        let task = std::thread::spawn(move || {
            let mut seen = None;
            let mut ws = tungstenite::accept_hdr(
                socket(listener),
                |request: &tungstenite::handshake::server::Request,
                 response: tungstenite::handshake::server::Response| {
                    seen = Some((
                        request.uri().path().to_owned(),
                        request.headers()["model"].to_str().unwrap().to_owned(),
                        request.headers()["authorization"]
                            .to_str()
                            .unwrap()
                            .to_owned(),
                    ));
                    Ok(response)
                },
            )
            .unwrap();
            let start = receive(&mut ws);
            assert_eq!(
                field(&start, "event").and_then(Value::as_str),
                Some("start")
            );
            let body = field(&start, "request").unwrap();
            assert_eq!(field(body, "text").and_then(Value::as_str), Some(""));
            assert_eq!(field(body, "format").and_then(Value::as_str), Some("wav"));
            assert_eq!(
                field(body, "reference_id").and_then(Value::as_str),
                Some("test-speaker")
            );
            assert!(field(body, "streaming").is_none());
            assert_eq!(
                field(body, "min_chunk_length").and_then(Value::as_u64),
                Some(20)
            );
            assert_eq!(
                field(body, "condition_on_previous_chunks").and_then(Value::as_bool),
                Some(true)
            );
            let mut text = String::new();
            let mut flushed = false;
            while !flushed {
                let message = receive(&mut ws);
                match field(&message, "event").and_then(Value::as_str).unwrap() {
                    "text" => {
                        text.push_str(field(&message, "text").and_then(Value::as_str).unwrap())
                    }
                    "flush" => flushed = true,
                    other => panic!("Unexpected input event {other}"),
                }
            }
            assert_eq!(text, "Hello world.");
            let first = wav(1600, false);
            let mut position = 0;
            for end in [1, 7, 13, 35, 44, first.len()] {
                send(
                    &mut ws,
                    audio(
                        &first[position..end],
                        0,
                        0.0,
                        Some(vec![("Hello", 0.0, 0.1)]),
                    ),
                );
                position = end;
            }
            ws.send(Message::Ping(vec![1, 2, 3].into())).unwrap();
            let mut pong = false;
            let mut stopped = false;
            while !pong || !stopped {
                match ws.read().unwrap() {
                    Message::Pong(bytes) => {
                        assert_eq!(&bytes[..], &[1, 2, 3]);
                        pong = true;
                    }
                    Message::Binary(bytes) => {
                        let event = decode(&bytes).unwrap();
                        match field(&event, "event").and_then(Value::as_str).unwrap() {
                            "stop" => stopped = true,
                            "flush" => {}
                            "text" => text
                                .push_str(field(&event, "text").and_then(Value::as_str).unwrap()),
                            other => panic!("Unexpected input {other}"),
                        }
                    }
                    other => panic!("Unexpected client message {other:?}"),
                }
            }
            assert_eq!(text, "Hello world. This is live speech.");
            // These bytes belong to the next audio segment, with the previous chunk's alignment.
            let next = wav(2, false);
            send(
                &mut ws,
                audio(
                    &next[..11],
                    0,
                    0.0,
                    Some(vec![("Hello", 0.0, 0.1), ("world", 0.1, 0.2)]),
                ),
            );
            send(
                &mut ws,
                audio(&next[11..], 1, 0.2, Some(vec![("again", 0.0, 0.1)])),
            );
            send(
                &mut ws,
                map([
                    ("event", Value::from("finish")),
                    ("reason", Value::from("stop")),
                ]),
            );
            seen.unwrap()
        });
        let mut state = FishWsState::default();
        state.timestamps = true;
        open(&mut state, 42, &config);
        state.push_text("Hello world. ").unwrap();
        state.push_text("This is live speech.").unwrap();
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut playing = None;
        let mut finished = false;
        while Instant::now() < deadline && !finished {
            for event in state.poll() {
                assert_eq!(event.id(), 42);
                match event {
                    LiveEvent::Playing { audio, .. } => {
                        assert!(playing.is_none());
                        let mut decoder = audio.decoder();
                        assert_eq!(decoder.next(), Some(0.25));
                        playing = Some(decoder);
                        state.finish().unwrap();
                    }
                    LiveEvent::Finished { stats, .. } => {
                        assert_eq!(stats.audio_bytes, 3292);
                        assert!(stats.first_audio_ms.is_some());
                        finished = true;
                    }
                    LiveEvent::Failed { message, .. } => panic!("{message}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(finished);
        assert_eq!(state.words.len(), 3);
        assert_eq!(state.words[2].start, 0.2);
        assert_eq!(playing.unwrap().collect::<Vec<_>>(), vec![0.25; 1601]);
        let (path, model, key) = task.join().unwrap();
        assert_eq!(path, "/v1/tts/live/with-timestamp");
        assert_eq!(model, "s2.1-pro-free");
        assert_eq!(key, "Bearer offline-test-key");
    }
    #[test]
    fn proxy_env_priority_no_proxy_domains_ips_cidrs_and_rejection() {
        let url = reqwest::Url::parse("wss://api.fish.audio/v1/tts/live").unwrap();
        let env = HashMap::from([
            ("https_proxy", "http://lower.example:8080"),
            ("HTTPS_PROXY", "http://upper.example:8080"),
            ("ALL_PROXY", "http://all.example:8080"),
        ]);
        let route = proxy_route(&url, |key| env.get(key).map(|v| (*v).into())).unwrap();
        assert!(matches!(route,Route::HttpConnect{host,..} if host=="lower.example"));
        for skip in ["*", "fish.audio", ".fish.audio", "api.fish.audio:443"] {
            assert!(matches!(
                proxy_route(&url, |key| match key {
                    "no_proxy" => Some(skip.into()),
                    "https_proxy" => Some("socks5://unused.invalid:1".into()),
                    _ => None,
                })
                .unwrap(),
                Route::Direct
            ));
        }
        for (item, host, expected) in [
            ("10.0.0.0/8", "10.2.3.4", true),
            ("10.0.0.0/8", "11.2.3.4", false),
            ("::1/128", "::1", true),
            ("[::1]:443", "::1", true),
            ("fish.audio", "notfish.audio", false),
            ("fish.audio:80", "api.fish.audio", false),
        ] {
            assert_eq!(no_proxy_match(item, host, 443), expected);
        }
        for proxy in ["socks5://127.0.0.1:1080", "https://127.0.0.1:8443"] {
            let error = proxy_route(&url, |key| {
                if key == "HTTPS_PROXY" {
                    Some(proxy.into())
                } else {
                    None
                }
            })
            .unwrap_err();
            assert!(error.contains("unsupported"));
            assert!(!error.contains("127.0.0.1"));
        }
        let route = proxy_route(&url, |key| {
            if key == "all_proxy" {
                Some("http://fallback.example:8080".into())
            } else {
                None
            }
        })
        .unwrap();
        assert!(matches!(route,Route::HttpConnect{host,..} if host=="fallback.example"));
    }
    #[test]
    fn unsupported_proxy_fails_before_connection_without_direct_bypass() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let config = config(endpoint(&listener));
        let mut state = FishWsState::default();
        let error = state
            .open_with_env(1, &config, Some("test-key".into()), |name| {
                if name == "http_proxy" {
                    Some("socks5://127.0.0.1:1080".into())
                } else {
                    None
                }
            })
            .unwrap_err();
        assert!(error.contains("unsupported"));
        assert!(!state.is_busy());
        assert!(state.active_id().is_none());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
    #[test]
    fn offline_http_connect_proxy_auth_uses_tunnel_and_binary_frames() {
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_url = format!("http://test-user:p%40ss@{}", proxy.local_addr().unwrap());
        let task = std::thread::spawn(move || {
            let mut socket = socket(proxy);
            let mut header = Vec::new();
            let mut byte = [0];
            while !header.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            let header = String::from_utf8(header).unwrap();
            assert!(header.starts_with("CONNECT 127.0.0.1:9 HTTP/1.1"));
            assert!(header.contains(&format!(
                "Proxy-Authorization: Basic {}",
                base64::engine::general_purpose::STANDARD.encode("test-user:p@ss")
            )));
            socket
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .unwrap();
            let mut ws = tungstenite::accept(socket).unwrap();
            let start = receive(&mut ws);
            assert_eq!(
                field(&start, "event").and_then(Value::as_str),
                Some("start")
            );
            loop {
                let event = receive(&mut ws);
                if field(&event, "event").and_then(Value::as_str) == Some("stop") {
                    break;
                }
            }
            send(&mut ws, audio(&wav(2, true), 0, 0.0, None));
            send(
                &mut ws,
                map([
                    ("event", Value::from("finish")),
                    ("reason", Value::from("stop")),
                ]),
            );
        });
        let mut state = FishWsState::default();
        let config = config("http://127.0.0.1:9/v1/tts".into());
        state
            .open_with_env(7, &config, Some("test-key".into()), |name| {
                if name == "http_proxy" {
                    Some(proxy_url.clone())
                } else {
                    None
                }
            })
            .unwrap();
        state.push_text("Hello").unwrap();
        state.finish().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut finished = false;
        while Instant::now() < deadline && !finished {
            for event in state.poll() {
                match event {
                    LiveEvent::Finished { .. } => finished = true,
                    LiveEvent::Failed { message, .. } => panic!("{message}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(finished);
        task.join().unwrap();
    }
    #[test]
    fn text_queue_bounds_auto_flush_finish_and_messagepack_limits() {
        let mut queue = TextQueue::default();
        queue.push("Sentence with enough words.", true).unwrap();
        assert!(matches!(queue.commands.back(), Some(Command::Flush)));
        queue.finish().unwrap();
        assert!(queue.push("late", false).is_err());
        assert!(queue.flush().is_err());
        assert!(queue.finish().is_ok());
        let mut queue = TextQueue::default();
        queue.push(&"x".repeat(MAX_TEXT_CHARS), false).unwrap();
        assert!(queue.push("x", false).is_err());
        assert!(decode(b"not messagepack").is_err());
        assert!(decode(&[0x81, 0xa1, b'x', 0xc6, 0xff, 0xff, 0xff, 0xff]).is_err());
        let mut nested = Value::Nil;
        for _ in 0..40 {
            nested = Value::Array(vec![nested]);
        }
        assert!(decode(&encoded(&map([("nested", nested)])).unwrap()).is_err());
    }
    #[test]
    fn streaming_chunks_flush_short_sentences_and_bound_long_unicode_text() {
        let mut queue = TextQueue::default();
        queue.push("Hello chat!", true).unwrap();
        assert!(matches!(queue.commands.back(), Some(Command::Flush)));
        let text = "猫".repeat(180);
        queue.push(&text, true).unwrap();
        let combined: String = queue
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(combined, format!("Hello chat!{text}"));
        assert_eq!(
            queue
                .commands
                .iter()
                .filter(|c| matches!(c, Command::Flush))
                .count(),
            3
        );
        assert_eq!(queue.since_flush, 44);
        assert_eq!(queue.bytes, combined.len());
        let mut manual = TextQueue::default();
        manual.push(&text, false).unwrap();
        assert_eq!(manual.commands.len(), 1);
    }
    #[test]
    fn first_phrase_can_start_early_without_forcing_every_later_word_group() {
        let mut queue = TextQueue::default();
        queue
            .push(
                "We are starting a wonderful game together and having lots of fun",
                true,
            )
            .unwrap();
        let chunks: Vec<_> = queue
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            chunks,
            vec![
                "We are starting a wonderful ",
                "game together and having lots of fun"
            ]
        );
        assert!(queue.first_flushed);
        assert_eq!(
            queue
                .commands
                .iter()
                .filter(|c| matches!(c, Command::Flush))
                .count(),
            1
        );
    }
    #[test]
    fn rejected_chunk_push_keeps_queue_and_accounting_unchanged() {
        let mut queue = TextQueue::default();
        queue
            .commands
            .extend((0..MAX_COMMANDS - 3).map(|_| Command::Flush));
        let before = (
            queue.commands.len(),
            queue.bytes,
            queue.total_chars,
            queue.since_flush,
        );
        assert!(queue.push(&"x".repeat(200), true).is_err());
        assert_eq!(
            (
                queue.commands.len(),
                queue.bytes,
                queue.total_chars,
                queue.since_flush
            ),
            before
        );
    }
    #[test]
    fn cancel_shuts_socket_and_invalidates_already_playing_pcm() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = config(endpoint(&listener));
        let (closed, observed) = mpsc::channel();
        let task = std::thread::spawn(move || {
            let mut ws = tungstenite::accept(socket(listener)).unwrap();
            receive(&mut ws);
            send(&mut ws, audio(&wav(1600, true), 0, 0.0, None));
            loop {
                match ws.read() {
                    Ok(Message::Close(_)) | Err(_) => {
                        closed.send(()).unwrap();
                        break;
                    }
                    _ => {}
                }
            }
        });
        let mut state = FishWsState::default();
        open(&mut state, 1, &config);
        state.push_text("Hello").unwrap();
        state.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut playing = None;
        while Instant::now() < deadline && playing.is_none() {
            for event in state.poll() {
                match event {
                    LiveEvent::Playing { audio, .. } => playing = Some(audio.decoder()),
                    LiveEvent::Failed { message, .. } => panic!("{message}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut decoder = playing.expect("progressive playback");
        assert_eq!(decoder.next(), Some(0.25));
        state.cancel();
        assert_eq!(decoder.next(), None);
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..50 {
            if !state.is_busy() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!state.is_busy());
        assert!(state.poll().is_empty());
        assert!(state.active_id().is_none());
        task.join().unwrap();
    }
    #[test]
    fn provider_errors_are_terminal_and_do_not_echo_remote_payloads() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = config(endpoint(&listener));
        let task = std::thread::spawn(move || {
            let mut ws = tungstenite::accept(socket(listener)).unwrap();
            receive(&mut ws);
            send(
                &mut ws,
                map([
                    ("event", Value::from("error")),
                    (
                        "error",
                        Value::from(
                            "Concurrency limit exceeded\napi_key: secretvalue offline-test-key",
                        ),
                    ),
                    ("max_concurrency", Value::from(8u64)),
                ]),
            );
            // Complete a valid WebSocket close rather than dropping a TCP socket
            // with unread client text (which can reset and discard the error).
            ws.close(None).unwrap();
            loop {
                match ws.read() {
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
        });
        let mut state = FishWsState::default();
        open(&mut state, 9, &config);
        state.push_text("Hello").unwrap();
        state.finish().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut failed = false;
        while Instant::now() < deadline && !failed {
            for event in state.poll() {
                match event {
                    LiveEvent::Failed { message, .. } => {
                        assert!(!message.contains("secret"));
                        assert!(!message.contains("offline-test-key"));
                        assert!(!message.contains('\n'));
                        assert!(
                            message.contains("Concurrency limit exceeded"),
                            "Unexpected error: {message}"
                        );
                        assert!(message.contains("account max concurrency 8"));
                        failed = true;
                    }
                    LiveEvent::Playing { .. } | LiveEvent::Finished { .. } => {
                        panic!("Error stream must fail")
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(failed);
        assert!(state.active_id().is_none());
        task.join().unwrap();
    }
    /// Listener whose accept queue is full: the kernel drops further SYNs, so a
    /// client connect stalls exactly like one to a blackholed address.
    #[cfg(target_os = "linux")]
    fn saturated_listener() -> (TcpListener, Vec<TcpStream>) {
        use std::os::fd::FromRawFd;
        // SAFETY: plain socket syscalls on a fresh descriptor whose ownership
        // moves into the returned listener.
        let listener = unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
            assert!(fd >= 0);
            let mut address: libc::sockaddr_in = std::mem::zeroed();
            address.sin_family = libc::AF_INET as libc::sa_family_t;
            address.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
            let listener = TcpListener::from_raw_fd(fd);
            assert_eq!(
                libc::bind(
                    fd,
                    (&raw const address).cast(),
                    size_of::<libc::sockaddr_in>() as libc::socklen_t,
                ),
                0
            );
            assert_eq!(libc::listen(fd, 0), 0);
            listener
        };
        let address = listener.local_addr().unwrap();
        let mut held = Vec::new();
        while let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_millis(200)) {
            held.push(stream);
            assert!(held.len() < 16, "listener backlog never filled");
        }
        (listener, held)
    }
    fn serve_once(listener: TcpListener) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut ws = tungstenite::accept(socket(listener)).unwrap();
            while field(&receive(&mut ws), "event").and_then(Value::as_str) != Some("stop") {}
            send(&mut ws, audio(&wav(2, true), 0, 0.0, None));
            send(
                &mut ws,
                map([
                    ("event", Value::from("finish")),
                    ("reason", Value::from("stop")),
                ]),
            );
        })
    }
    /// Polls until `id` finishes, returning the most workers seen at once.
    fn finish_within(state: &mut FishWsState, id: u64, limit: Duration) -> usize {
        let deadline = Instant::now() + limit;
        let mut peak = 0;
        loop {
            peak = peak.max(state.workers.load(Ordering::Acquire));
            for event in state.poll() {
                match event {
                    LiveEvent::Finished { id: done, .. } if done == id => return peak,
                    LiveEvent::Failed { message, .. } => panic!("{message}"),
                    _ => {}
                }
            }
            assert!(Instant::now() < deadline, "generation {id} did not finish");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn workers_drain(state: &FishWsState, limit: Duration) {
        let deadline = Instant::now() + limit;
        while state.is_busy() {
            assert!(Instant::now() < deadline, "cancelled workers did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(state.socket.lock().unwrap().is_none());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn cancel_during_stalled_tcp_connect_does_not_block_next_generation() {
        let (stalled, _held) = saturated_listener();
        let mut state = FishWsState::default();
        open(&mut state, 1, &config(endpoint(&stalled)));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(state.workers.load(Ordering::Acquire), 1);
        state.cancel();
        let live = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = config(endpoint(&live));
        let server = serve_once(live);
        let started = Instant::now();
        open(&mut state, 2, &config);
        state.push_text("Hello").unwrap();
        state.finish().unwrap();
        let peak = finish_within(&mut state, 2, Duration::from_secs(3));
        // The stale connect still holds its slot for most of CONNECT_TIMEOUT;
        // the next turn must not wait for it.
        assert!(
            started.elapsed() < CONNECT_TIMEOUT / 2,
            "next generation waited {:?}",
            started.elapsed()
        );
        assert_eq!(peak, 2);
        server.join().unwrap();
        workers_drain(&state, CONNECT_TIMEOUT + Duration::from_secs(1));
        assert!(state.poll().is_empty());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn stalled_cancelled_workers_stay_bounded_and_admission_resumes() {
        let (stalled, _held) = saturated_listener();
        let stalled = config(endpoint(&stalled));
        let mut state = FishWsState::default();
        // Let each worker get past its first cancellation check into connect.
        open(&mut state, 1, &stalled);
        std::thread::sleep(Duration::from_millis(100));
        state.cancel();
        open(&mut state, 2, &stalled);
        std::thread::sleep(Duration::from_millis(100));
        state.cancel();
        assert_eq!(state.workers.load(Ordering::Acquire), MAX_WORKERS);
        let live = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = config(endpoint(&live));
        let server = serve_once(live);
        open(&mut state, 3, &config);
        assert!(state.pending.is_some(), "a third worker must not spawn");
        state.push_text("Hello").unwrap();
        state.finish().unwrap();
        let peak = finish_within(&mut state, 3, CONNECT_TIMEOUT + Duration::from_secs(2));
        assert_eq!(peak, MAX_WORKERS);
        server.join().unwrap();
        workers_drain(&state, CONNECT_TIMEOUT + Duration::from_secs(1));
    }
    #[test]
    fn cancel_during_stalled_proxy_connect_closes_tunnel_without_direct_bypass() {
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
        let direct = TcpListener::bind("127.0.0.1:0").unwrap();
        direct.set_nonblocking(true).unwrap();
        let (requested, seen_request) = mpsc::channel();
        let (closed, seen_close) = mpsc::channel();
        let task = std::thread::spawn(move || {
            let mut socket = socket(proxy);
            let mut header = Vec::new();
            let mut byte = [0];
            while !header.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            requested.send(()).unwrap();
            // Never answer CONNECT; wait for the client to drop the tunnel.
            assert!(matches!(socket.read(&mut byte), Ok(0) | Err(_)));
            closed.send(()).unwrap();
        });
        let mut state = FishWsState::default();
        let target = config(endpoint(&direct));
        state
            .open_with_env(1, &target, Some("test-key".into()), |name| {
                (name == "http_proxy").then(|| proxy_url.clone())
            })
            .unwrap();
        seen_request.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        state.cancel();
        seen_close.recv_timeout(Duration::from_secs(1)).unwrap();
        workers_drain(&state, Duration::from_secs(1));
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(state.poll().is_empty());
        assert_eq!(
            direct.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        task.join().unwrap();
    }

    #[test]
    fn realistic_timestamp_mailbox_preserves_playback_and_terminal() {
        for failed in [false, true] {
            let mut state = FishWsState::default();
            state.id = Some(9);
            state.input = Some(Arc::new(Mutex::new(TextQueue::default())));
            let generation = state.generation.load(Ordering::Acquire);
            let mut audio = None;
            crate::voice_stream::receive_wav(
                std::io::Cursor::new(wav(32, false)),
                state.generation.clone(),
                generation,
                Instant::now() + Duration::from_secs(2),
                |voice| audio = Some(voice),
            )
            .unwrap();
            publish(
                &state.mailbox,
                &state.generation,
                generation,
                LiveEvent::Playing {
                    id: 9,
                    audio: audio.unwrap(),
                },
            );
            for index in 0..500 {
                publish(
                    &state.mailbox,
                    &state.generation,
                    generation,
                    LiveEvent::Timing {
                        id: 9,
                        words: vec![WordTiming {
                            text: format!("word{index}"),
                            start: 0.0,
                            end: 0.1,
                        }],
                    },
                );
            }
            let terminal = if failed {
                LiveEvent::Failed {
                    id: 9,
                    message: "provider failure".into(),
                }
            } else {
                LiveEvent::Finished {
                    id: 9,
                    stats: LiveStats::default(),
                }
            };
            publish(&state.mailbox, &state.generation, generation, terminal);
            // One playback initialization, one coalesced timing snapshot,
            // and one terminal: the eight-slot queue cannot overflow here.
            assert_eq!(state.mailbox.lock().unwrap().len(), 3);
            let events = state.poll();
            assert!(matches!(&events[0], LiveEvent::Playing { .. }));
            assert!(matches!(&events[1], LiveEvent::Timing { words, .. }
                if words[0].text == "word499"));
            assert!(matches!(
                &events[2],
                LiveEvent::Finished { .. } | LiveEvent::Failed { .. }
            ));
            assert_eq!(state.active_id(), None);
            assert!(state.input.is_none());
            publish(
                &state.mailbox,
                &state.generation,
                generation.wrapping_sub(1),
                LiveEvent::Failed {
                    id: 8,
                    message: "stale".into(),
                },
            );
            assert!(state.poll().is_empty());
        }
    }
}

#[cfg(test)]
mod observation_socket_tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    #[allow(clippy::result_large_err)] // tungstenite fixes the callback error type.
    fn observation_proxy_upgrade_sends_no_provider_headers_or_initialization() {
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = proxy.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = proxy.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            assert!(header.starts_with(b"CONNECT fixture.invalid:80 HTTP/1.1\r\n"));
            socket
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .unwrap();
            let mut ws = tungstenite::accept_hdr(
                socket,
                |request: &tungstenite::handshake::server::Request, response| {
                    assert!(!request.headers().contains_key("authorization"));
                    assert!(!request.headers().contains_key("model"));
                    Ok(response)
                },
            )
            .unwrap();
            ws.send(Message::Text("event-fixture".into())).unwrap();
            assert!(matches!(ws.read(), Err(tungstenite::Error::Io(_))));
        });
        let url = reqwest::Url::parse("ws://fixture.invalid/ws").unwrap();
        let mut ws = observation_socket_with_env(&url, |name| {
            (name == "http_proxy").then(|| format!("http://{address}"))
        })
        .unwrap();
        assert_eq!(ws.read().unwrap().into_text().unwrap(), "event-fixture");
        // No startup traffic is sent. Drop closes the stream after server timeout.
        server.join().unwrap();
    }

    #[test]
    fn observation_proxy_rejection_never_falls_back_to_direct_target() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_address = proxy.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = proxy.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = [0; 2048];
            let _ = socket.read(&mut bytes).unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n",
                )
                .unwrap();
        });
        let url =
            reqwest::Url::parse(&format!("ws://{}/ws", target.local_addr().unwrap())).unwrap();
        let error = observation_socket_with_env(&url, |name| {
            (name == "http_proxy").then(|| format!("http://{proxy_address}"))
        })
        .unwrap_err();
        assert_eq!(error, "Integration proxy tunnel failed");
        assert_eq!(
            target.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        server.join().unwrap();
    }
}
