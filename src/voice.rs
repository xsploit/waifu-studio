//! Extensible HTTP speech adapters. Credentials are resolved only for a request,
//! never serialized. Local Fish Speech uses the model loaded by its server.
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceAdapter {
    #[default]
    FishHosted,
    FishLocal,
    OpenAi,
    Vercel,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceDelivery {
    #[default]
    Buffered,
    HttpWavStream,
    TimestampSse,
    LiveWebSocket,
    TimestampWebSocket,
}
impl VoiceDelivery {
    pub fn is_websocket(self) -> bool {
        matches!(self, Self::LiveWebSocket | Self::TimestampWebSocket)
    }
}
/// Speech delivery preference; call routing still requires an explicitly
/// permitted, ready extension. CallsOnly deliberately has no local fallback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceOutput {
    #[default]
    LocalAndCalls,
    LocalOnly,
    CallsOnly,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    pub enabled: bool,
    pub output: VoiceOutput,
    pub adapter: VoiceAdapter,
    /// Complete endpoint URL, including /v1/tts or /v1/audio/speech.
    pub endpoint: String,
    pub model: String,
    pub reference_id: String,
    pub voice: String,
    pub format: String,
    pub speed: f32,
    pub delivery: VoiceDelivery,
    pub latency: String,
    pub chunk_length: u16,
    /// Hosted Fish only: minimum characters before a new chunk (0–100).
    pub min_chunk_length: u16,
    /// Hosted Fish only: condition each chunk on previous audio for consistency.
    pub condition_on_previous_chunks: bool,
    /// Drama 3 natural-language performance direction, prepended to text.
    pub direction: String,
    pub api_key_env: String,
}
impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            output: VoiceOutput::default(),
            adapter: VoiceAdapter::FishHosted,
            endpoint: "https://api.fish.audio/v1/tts".into(),
            model: "s2.1-pro-free".into(),
            reference_id: String::new(),
            voice: "alloy".into(),
            format: "wav".into(),
            speed: 1.0,
            delivery: VoiceDelivery::LiveWebSocket,
            latency: "balanced".into(),
            chunk_length: 160,
            // Lower than Fish's default of 50, favoring first-audio latency.
            min_chunk_length: 20,
            condition_on_previous_chunks: true,
            direction: String::new(),
            api_key_env: "FISH_API_KEY".into(),
        }
    }
}
impl VoiceConfig {
    /// New companion setup only; serde fallback defaults remain compatible.
    pub fn companion_default() -> Self {
        Self {
            model: "drama-3".into(),
            delivery: VoiceDelivery::HttpWavStream,
            latency: "low".into(),
            ..Self::default()
        }
    }
    pub fn preset(adapter: VoiceAdapter) -> Self {
        if adapter == VoiceAdapter::FishHosted {
            return Self::companion_default();
        }
        let mut config = Self {
            adapter,
            ..Self::default()
        };
        match adapter {
            VoiceAdapter::FishLocal => {
                config.endpoint = "http://127.0.0.1:8080/v1/tts".into();
                config.model.clear();
                config.api_key_env.clear();
                config.delivery = VoiceDelivery::HttpWavStream;
            }
            VoiceAdapter::OpenAi => {
                config.endpoint = "http://127.0.0.1:8000/v1/audio/speech".into();
                config.model = "tts-1".into();
                config.api_key_env.clear();
                config.delivery = VoiceDelivery::Buffered;
            }
            VoiceAdapter::Vercel => {
                config.endpoint = crate::vercel_speech::ENDPOINT.into();
                config.model = "openai/tts-1".into();
                config.api_key_env = "AI_GATEWAY_API_KEY".into();
                config.delivery = VoiceDelivery::Buffered;
            }
            VoiceAdapter::FishHosted => {}
        }
        config
    }
    /// Apply the low-latency streaming preset without replacing the user's
    /// provider, model, voice, credentials, speaking speed or performance direction.
    pub fn use_live_pipeline(&mut self) {
        match self.adapter {
            VoiceAdapter::FishHosted => {
                self.delivery = VoiceDelivery::LiveWebSocket;
                self.format = "wav".into();
                self.latency = "balanced".into();
                self.chunk_length = 160;
                self.min_chunk_length = 20;
                self.condition_on_previous_chunks = true;
            }
            VoiceAdapter::FishLocal => {
                self.delivery = VoiceDelivery::HttpWavStream;
                self.format = "wav".into();
            }
            VoiceAdapter::OpenAi | VoiceAdapter::Vercel => {
                self.delivery = VoiceDelivery::Buffered;
            }
        }
    }
    /// Apply the hosted HTTP/Low tuning from the bounded pipeline comparisons.
    /// Identity, endpoint, credentials, direction and auto-speech opt-in stay intact.
    /// Other adapters are unchanged: these measurements do not cover them.
    pub fn use_http_low_latency(&mut self) -> bool {
        if self.adapter != VoiceAdapter::FishHosted {
            return false;
        }
        self.delivery = VoiceDelivery::HttpWavStream;
        self.format = "wav".into();
        self.latency = "low".into();
        self.chunk_length = 160;
        self.min_chunk_length = 20;
        self.condition_on_previous_chunks = true;
        true
    }
    /// Validates hosted-only generation tuning shared by every hosted transport.
    pub(crate) fn validate_hosted_tuning(&self) -> Result<(), String> {
        if self.min_chunk_length > 100 {
            return Err("Fish minimum chunk length must be between 0 and 100".into());
        }
        Ok(())
    }
}
#[derive(Debug)]
pub struct VoiceAudio {
    pub bytes: Vec<u8>,
    pub mime: String,
    pub format: String,
}
#[derive(Debug)]
pub enum VoiceEvent {
    Ready {
        id: u64,
        audio: VoiceAudio,
    },
    Streaming {
        id: u64,
        audio: crate::voice_stream::StreamingVoice,
    },
    Failed {
        id: u64,
        message: String,
    },
}
impl VoiceEvent {
    pub fn id(&self) -> u64 {
        match self {
            Self::Ready { id, .. } | Self::Streaming { id, .. } | Self::Failed { id, .. } => *id,
        }
    }
}
/// Concurrent HTTP speech requests, including cancelled ones still unwinding.
pub const HTTP_SLOTS: usize = 2;
/// Emission order for one speech turn. Requests in a turn download in
/// parallel, but each hands its audio to playback only after its predecessor.
#[derive(Default)]
struct TurnOrder {
    issued: AtomicU64,
    next: Mutex<u64>,
    advanced: Condvar,
}
impl TurnOrder {
    /// Blocks until `seq` is the head, or returns false once cancelled.
    fn wait(&self, seq: u64, cancelled: &impl Fn() -> bool) -> bool {
        let mut next = self.next.lock().unwrap();
        while *next != seq {
            if cancelled() {
                return false;
            }
            next = self
                .advanced
                .wait_timeout(next, Duration::from_millis(10))
                .unwrap()
                .0;
        }
        !cancelled()
    }
    fn advance(&self, seq: u64) {
        let mut next = self.next.lock().unwrap();
        if *next == seq {
            *next += 1;
            self.advanced.notify_all();
        }
    }
}
/// Releases an HTTP slot once every thread sharing it has exited.
struct Slot(Arc<AtomicUsize>);
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
/// Bounded body spool filled by a download thread while the request waits for
/// its turn, so the next sentence synthesizes during current playback.
#[derive(Default)]
struct Spool {
    state: Mutex<SpoolState>,
    changed: Condvar,
}
#[derive(Default)]
struct SpoolState {
    bytes: std::collections::VecDeque<u8>,
    received: u64,
    end: Option<Result<(), String>>,
}
struct SpoolReader<F> {
    spool: Arc<Spool>,
    cancelled: F,
}
impl<F: Fn() -> bool> Read for SpoolReader<F> {
    fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
        let mut state = self.spool.state.lock().unwrap();
        loop {
            if (self.cancelled)() {
                return Err(std::io::Error::other(crate::voice_stream::SafeStreamError(
                    "Speech cancelled".into(),
                )));
            }
            if !state.bytes.is_empty() {
                return state.bytes.read(target);
            }
            match &state.end {
                Some(Ok(())) => return Ok(0),
                Some(Err(message)) => {
                    return Err(std::io::Error::other(crate::voice_stream::SafeStreamError(
                        message.clone(),
                    )));
                }
                None => {}
            }
            state = self
                .spool
                .changed
                .wait_timeout(state, Duration::from_millis(10))
                .unwrap()
                .0;
        }
    }
}
fn download(
    mut response: reqwest::blocking::Response,
    spool: &Spool,
    cancelled: impl Fn() -> bool,
) {
    let mut buffer = [0; 16 * 1024];
    let end = loop {
        if cancelled() {
            break Err("Speech cancelled".to_owned());
        }
        match response.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Ok(n) => {
                let mut state = spool.state.lock().unwrap();
                state.received += n as u64;
                if state.received > MAX_AUDIO {
                    break Err("Speech audio exceeds 32 MiB".to_owned());
                }
                state.bytes.extend(&buffer[..n]);
                spool.changed.notify_all();
            }
            Err(_) => break Err("Could not receive speech stream".to_owned()),
        }
    };
    spool.state.lock().unwrap().end = Some(end);
    spool.changed.notify_all();
}
struct PendingSpeech {
    generation: u64,
    turn: Arc<TurnOrder>,
    seq: u64,
    id: u64,
    config: VoiceConfig,
    text: String,
    session_key: Option<String>,
}
pub struct VoiceWorker {
    generation: Arc<AtomicU64>,
    sender: mpsc::Sender<(u64, VoiceEvent)>,
    receiver: mpsc::Receiver<(u64, VoiceEvent)>,
    slots: Arc<AtomicUsize>,
    pending: Mutex<std::collections::VecDeque<PendingSpeech>>,
    turn: Mutex<Arc<TurnOrder>>,
}
impl Default for VoiceWorker {
    fn default() -> Self {
        Self::new()
    }
}
impl VoiceWorker {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            generation: Arc::new(AtomicU64::new(0)),
            sender,
            receiver,
            slots: Arc::new(AtomicUsize::new(0)),
            pending: Mutex::default(),
            turn: Mutex::default(),
        }
    }
    /// Cancels the whole turn: clears queued speech and suppresses playback and
    /// events. In-flight HTTP calls keep their slots until they unwind.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.pending.lock().unwrap().clear();
        *self.turn.lock().unwrap() = Arc::default();
    }
    /// Newest wins: cancels the current turn and speaks `text` as a new one.
    pub fn start(
        &self,
        id: u64,
        config: VoiceConfig,
        text: String,
        session_key: Option<String>,
    ) -> Result<(), String> {
        validate_request(&config, &text)?;
        self.cancel();
        self.enqueue(id, config, text, session_key)
    }
    /// True when another request in the current turn can start immediately.
    pub fn can_prefetch(&self) -> bool {
        self.slots.load(Ordering::SeqCst) < HTTP_SLOTS && self.pending.lock().unwrap().is_empty()
    }
    /// Appends `text` to the current turn without interrupting it. Its request
    /// starts as soon as a slot is free; its audio is delivered after every
    /// earlier request in the turn.
    pub fn enqueue(
        &self,
        id: u64,
        config: VoiceConfig,
        text: String,
        session_key: Option<String>,
    ) -> Result<(), String> {
        validate_request(&config, &text)?;
        {
            let mut pending = self.pending.lock().unwrap();
            if pending.len() >= HTTP_SLOTS {
                return Err("Speech prefetch queue is full".into());
            }
            let turn = self.turn.lock().unwrap().clone();
            pending.push_back(PendingSpeech {
                generation: self.generation.load(Ordering::SeqCst),
                seq: turn.issued.fetch_add(1, Ordering::SeqCst),
                turn,
                id,
                config,
                text,
                session_key,
            });
        }
        self.start_pending();
        Ok(())
    }
    fn claim_slot(&self) -> Option<Slot> {
        self.slots
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < HTTP_SLOTS).then_some(n + 1)
            })
            .ok()
            .map(|_| Slot(self.slots.clone()))
    }
    fn start_pending(&self) {
        loop {
            let Some(slot) = self.claim_slot() else {
                return;
            };
            let Some(request) = self.pending.lock().unwrap().pop_front() else {
                return;
            };
            self.spawn(request, slot);
        }
    }
    fn spawn(&self, request: PendingSpeech, slot: Slot) {
        let PendingSpeech {
            generation,
            turn,
            seq,
            id,
            config,
            text,
            session_key,
        } = request;
        let active = self.generation.clone();
        let sender = self.sender.clone();
        let slot = Arc::new(slot);
        std::thread::spawn(move || {
            let cancelled = || active.load(Ordering::SeqCst) != generation;
            let result = if config.delivery == VoiceDelivery::HttpWavStream {
                prepare_response(&config, &text, session_key.as_deref(), cancelled).map(
                    |(response, _)| {
                        let spool = Arc::new(Spool::default());
                        let (download_spool, download_active) = (spool.clone(), active.clone());
                        let download_slot = slot.clone();
                        std::thread::spawn(move || {
                            let _slot = download_slot;
                            download(response, &download_spool, || {
                                download_active.load(Ordering::SeqCst) != generation
                            });
                        });
                        Some(spool)
                    },
                )
            } else {
                synthesize(&config, &text, session_key.as_deref(), cancelled).map(|audio| {
                    // Buffered audio is complete; only its delivery is ordered.
                    if turn.wait(seq, &cancelled) {
                        let _ = sender.send((generation, VoiceEvent::Ready { id, audio }));
                    }
                    None
                })
            };
            let result = match result {
                Ok(Some(spool)) if turn.wait(seq, &cancelled) => {
                    let budget = crate::voice_stream::StreamBudget::default();
                    crate::voice_stream::receive_wav_with_budget(
                        SpoolReader { spool, cancelled },
                        active.clone(),
                        generation,
                        &budget,
                        |audio| {
                            if !cancelled() {
                                let _ =
                                    sender.send((generation, VoiceEvent::Streaming { id, audio }));
                            }
                        },
                    )
                    .map_err(|error| format!("Fish HTTP WAV: {error}"))
                }
                Ok(_) => Ok(()),
                // Failures report in turn order, so earlier audio still plays first.
                Err(message) => {
                    turn.wait(seq, &cancelled);
                    Err(message)
                }
            };
            if let Err(message) = result
                && !cancelled()
            {
                let _ = sender.send((generation, VoiceEvent::Failed { id, message }));
            }
            turn.advance(seq);
            drop(slot);
        });
    }
    pub fn try_recv(&self) -> Option<VoiceEvent> {
        self.start_pending();
        while let Ok((generation, event)) = self.receiver.try_recv() {
            if generation == self.generation.load(Ordering::SeqCst) {
                return Some(event);
            }
        }
        None
    }
}
impl Drop for VoiceWorker {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn validate_request(config: &VoiceConfig, text: &str) -> Result<(), String> {
    if config.adapter == VoiceAdapter::Vercel && config.delivery != VoiceDelivery::Buffered {
        return Err("Vercel speech currently requires buffered delivery".into());
    }
    if config.delivery == VoiceDelivery::TimestampSse || config.delivery.is_websocket() {
        return Err("Live speech requires its dedicated Fish transport".into());
    }
    if text.trim().is_empty() || text.chars().count() > 12_000 {
        return Err("Speech text must contain 1–12000 characters".into());
    }
    Ok(())
}
const MAX_AUDIO: u64 = 32 * 1024 * 1024;
/// Clone one credential-free client so successive chunks reuse proxy/TLS
/// connections. Authorization remains scoped to each individual request.
pub(crate) fn speech_client() -> Result<reqwest::blocking::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(90))
                .connect_timeout(Duration::from_secs(10))
                .pool_idle_timeout(Duration::from_secs(90))
                .pool_max_idle_per_host(2)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "Could not initialize speech HTTP client".into())
        })
        .clone()
}
fn synthesize(
    config: &VoiceConfig,
    text: &str,
    session_key: Option<&str>,
    cancelled: impl Fn() -> bool,
) -> Result<VoiceAudio, String> {
    if config.adapter == VoiceAdapter::Vercel {
        return crate::vercel_speech::synthesize(config, text, session_key, cancelled);
    }
    let (response, mime) = prepare_response(config, text, session_key, &cancelled)?;
    let mut bytes = Vec::new();
    response
        .take(MAX_AUDIO + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not receive speech audio")?;
    if cancelled() {
        return Err("Speech cancelled".into());
    }
    if bytes.is_empty() || bytes.len() as u64 > MAX_AUDIO {
        return Err("Speech audio is empty or exceeds 32 MiB".into());
    }
    Ok(VoiceAudio {
        bytes,
        mime: if mime.is_empty() {
            if config.format == "wav" {
                "audio/wav"
            } else {
                "audio/mpeg"
            }
            .into()
        } else {
            mime
        },
        format: config.format.clone(),
    })
}
fn prepare_response(
    config: &VoiceConfig,
    text: &str,
    session_key: Option<&str>,
    cancelled: impl Fn() -> bool,
) -> Result<(reqwest::blocking::Response, String), String> {
    if text.trim().is_empty() || text.chars().count() > 12_000 {
        return Err("Speech text must contain 1–12000 characters".into());
    }
    if !config.speed.is_finite() || !(0.5..=2.0).contains(&config.speed) {
        return Err("Speech speed must be between 0.5 and 2".into());
    }
    if !matches!(config.format.as_str(), "wav" | "mp3") {
        return Err("Playback currently supports WAV or MP3 speech".into());
    }
    if config.delivery == VoiceDelivery::HttpWavStream
        && (config.adapter == VoiceAdapter::OpenAi || config.format != "wav")
    {
        return Err("Progressive playback supports Fish HTTP WAV only".into());
    }
    if config.adapter != VoiceAdapter::OpenAi {
        let max_chunk = if config.adapter == VoiceAdapter::FishHosted {
            300
        } else {
            1000
        };
        if !(100..=max_chunk).contains(&config.chunk_length) {
            return Err("Fish chunk length is outside the adapter's supported range".into());
        }
        let valid_latency = matches!(config.latency.as_str(), "normal" | "balanced")
            || (config.adapter == VoiceAdapter::FishHosted && config.latency == "low");
        if !valid_latency {
            return Err("Unsupported Fish latency mode".into());
        }
    }
    if config.adapter == VoiceAdapter::FishHosted {
        config.validate_hosted_tuning()?;
    }
    if config.direction.chars().count() > 2000 || config.direction.contains(['[', ']']) {
        return Err("Voice direction must be at most 2000 characters without brackets".into());
    }
    if !config.direction.trim().is_empty()
        && !(config.model == "drama-3" && config.adapter == VoiceAdapter::FishHosted)
    {
        return Err("Voice direction currently requires hosted Drama 3".into());
    }
    let directed = if config.direction.trim().is_empty() {
        text.to_owned()
    } else {
        format!("[{}] {}", config.direction.trim(), text)
    };
    let url = reqwest::Url::parse(&config.endpoint).map_err(|_| "Invalid speech endpoint")?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use an endpoint without credentials, query, or fragment".into());
    }
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("Speech endpoint requires HTTPS, or HTTP on localhost".into());
    }
    let env_key = if config.api_key_env.trim().is_empty() {
        None
    } else {
        std::env::var(config.api_key_env.trim()).ok()
    };
    let key = session_key
        .filter(|key| !key.trim().is_empty())
        .or(env_key.as_deref());
    if config.adapter == VoiceAdapter::FishHosted && key.is_none() {
        return Err(
            "Set a session API key or the configured Fish API key environment variable".into(),
        );
    }
    let mut body = match config.adapter {
        VoiceAdapter::OpenAi => {
            serde_json::json!({"input": text, "model":config.model,"voice":config.voice,"response_format":config.format,"speed":config.speed})
        }
        _ => {
            serde_json::json!({"text":directed,"format":config.format,"normalize":true,"latency":config.latency,"chunk_length":config.chunk_length})
        }
    };
    if config.adapter == VoiceAdapter::FishLocal {
        body["streaming"] = (config.delivery == VoiceDelivery::HttpWavStream).into();
    }
    if config.adapter != VoiceAdapter::OpenAi {
        if !config.reference_id.trim().is_empty() {
            body["reference_id"] = config.reference_id.trim().into();
        }
        if config.adapter == VoiceAdapter::FishHosted {
            body["prosody"] = serde_json::json!({"speed":config.speed});
            body["min_chunk_length"] = config.min_chunk_length.into();
            body["condition_on_previous_chunks"] = config.condition_on_previous_chunks.into();
        }
    }
    if cancelled() {
        return Err("Speech cancelled".into());
    }
    let client = speech_client()?;
    let mut request = client.post(url).json(&body);
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    if config.adapter == VoiceAdapter::FishHosted {
        if !matches!(
            config.model.as_str(),
            "s1" | "s2-pro" | "s2.1-pro" | "s2.1-pro-free" | "drama-3"
        ) {
            return Err(
                "Choose a documented Fish model; unknown models can fall back to a paid model"
                    .into(),
            );
        }
        request = request.header("model", &config.model);
    }
    // Never include remote response text, credentials, or URL in an error.
    let response = request
        .send()
        .map_err(|_| "Speech request failed (connection or timeout)")?;
    if !response.status().is_success() {
        return Err(format!(
            "Speech provider returned HTTP {}",
            response.status().as_u16()
        ));
    }
    if response.content_length().is_some_and(|n| n > MAX_AUDIO) {
        return Err("Speech audio exceeds 32 MiB".into());
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .to_owned();
    if !(mime.starts_with("audio/") || mime == "application/octet-stream" || mime.is_empty()) {
        return Err("Speech endpoint returned a non-audio response".into());
    }
    Ok((response, mime))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::Instant,
    };
    #[test]
    fn speech_output_migrates_old_settings_and_preserves_explicit_local_only() {
        let old: VoiceConfig =
            serde_json::from_str(r#"{"adapter":"Vercel","model":"existing-model","speed":1.33}"#)
                .unwrap();
        assert_eq!(old.output, VoiceOutput::LocalAndCalls);
        assert_eq!(old.model, "existing-model");
        for output in [VoiceOutput::LocalOnly, VoiceOutput::CallsOnly] {
            let saved = VoiceConfig {
                output,
                ..old.clone()
            };
            let restored: VoiceConfig =
                serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
            assert_eq!(restored.output, output);
            assert_eq!(restored.adapter, VoiceAdapter::Vercel);
            assert_eq!(restored.speed, 1.33);
        }
    }
    #[test]
    fn new_custom_speech_preset_stays_local_without_hosted_credentials() {
        let config = VoiceConfig::preset(VoiceAdapter::OpenAi);
        assert_eq!(config.endpoint, "http://127.0.0.1:8000/v1/audio/speech");
        assert!(config.api_key_env.is_empty());
        let old: VoiceConfig = serde_json::from_str(r#"{"adapter":"OpenAi","endpoint":"https://custom.example/v1/audio/speech","api_key_env":"EXISTING_KEY"}"#).unwrap();
        assert_eq!(old.endpoint, "https://custom.example/v1/audio/speech");
        assert_eq!(old.api_key_env, "EXISTING_KEY");
    }
    #[test]
    fn consecutive_chunks_reuse_tcp_without_reusing_authorization() {
        fn read_request(stream: &mut std::net::TcpStream) -> std::io::Result<String> {
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            let mut bytes = Vec::new();
            loop {
                let mut buf = [0; 4096];
                let n = stream.read(&mut buf)?;
                if n == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|x| x == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&bytes[..end]);
                    let length = header
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|n| n.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        return Ok(String::from_utf8(bytes).unwrap());
                    }
                }
            }
        }
        fn respond(stream: &mut std::net::TcpStream) {
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: 4\r\nConnection: keep-alive\r\n\r\nRIFF").unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/tts", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let first = read_request(&mut socket).unwrap();
            respond(&mut socket);
            let (second, reused) = match read_request(&mut socket) {
                Ok(request) => (request, true),
                Err(_) => {
                    let (other, _) = listener.accept().unwrap();
                    socket = other;
                    (read_request(&mut socket).unwrap(), false)
                }
            };
            respond(&mut socket);
            (first, second, reused)
        });
        let config = VoiceConfig {
            adapter: VoiceAdapter::FishLocal,
            endpoint,
            ..Default::default()
        };
        synthesize(&config, "First phrase", Some("first-test-key"), || false).unwrap();
        synthesize(&config, "Second phrase", Some("second-test-key"), || false).unwrap();
        let (first, second, reused) = server.join().unwrap();
        assert!(
            reused,
            "Successive speech chunks must share a TCP connection"
        );
        assert!(first.contains("Bearer first-test-key"));
        assert!(second.contains("Bearer second-test-key"));
        assert!(!second.contains("first-test-key"));
    }
    fn server(status: u16, mime: &str, bytes: &[u8]) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/tts", listener.local_addr().unwrap());
        let mime = mime.to_owned();
        let bytes = bytes.to_vec();
        let task = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|x| x == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&request[..end]);
                    let length: usize = header
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|n| n.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len()).unwrap();
            stream.write_all(&bytes).unwrap();
            String::from_utf8(request).unwrap()
        });
        (url, task)
    }
    #[test]
    fn fake_http_delivers_playable_stream_before_response_finishes() {
        use bevy::audio::Decodable;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/tts", listener.local_addr().unwrap());
        let (release, wait) = mpsc::channel();
        let task = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 8192];
            let n = socket.read(&mut request).unwrap();
            assert!(n > 0);
            let mut wav = b"RIFF".to_vec();
            wav.extend(u32::MAX.to_le_bytes());
            wav.extend(b"WAVEfmt ");
            wav.extend(16u32.to_le_bytes());
            wav.extend(1u16.to_le_bytes());
            wav.extend(1u16.to_le_bytes());
            wav.extend(8000u32.to_le_bytes());
            wav.extend(16000u32.to_le_bytes());
            wav.extend(2u16.to_le_bytes());
            wav.extend(16u16.to_le_bytes());
            wav.extend(b"data");
            wav.extend(u32::MAX.to_le_bytes());
            for _ in 0..1600 {
                wav.extend(8192i16.to_le_bytes());
            }
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            socket.write_all(&wav).unwrap();
            socket.flush().unwrap();
            // EOF is withheld until the decoder has actually played a sample.
            wait.recv_timeout(Duration::from_secs(3)).unwrap();
            socket.write_all(&8192i16.to_le_bytes()).unwrap();
        });
        let config = VoiceConfig {
            endpoint,
            delivery: VoiceDelivery::HttpWavStream,
            ..VoiceConfig::preset(VoiceAdapter::FishLocal)
        };
        let (response, _) = prepare_response(&config, "Hello", None, || false).unwrap();
        let generation = Arc::new(AtomicU64::new(1));
        let mut count = 0;
        crate::voice_stream::receive_wav(
            response,
            generation,
            1,
            Instant::now() + Duration::from_secs(5),
            |audio| {
                count += 1;
                let mut decoder = audio.decoder();
                assert_eq!(decoder.next(), Some(0.25));
                release.send(()).unwrap();
            },
        )
        .unwrap();
        assert_eq!(count, 1);
        task.join().unwrap();
    }
    fn mono_wav_header() -> Vec<u8> {
        let mut wav = b"RIFF".to_vec();
        wav.extend(u32::MAX.to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(8000u32.to_le_bytes());
        wav.extend(16000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(u32::MAX.to_le_bytes());
        wav
    }
    fn tone(value: i16, frames: usize) -> Vec<u8> {
        (0..frames).flat_map(|_| value.to_le_bytes()).collect()
    }
    struct StreamServer {
        url: String,
        requests: mpsc::Receiver<String>,
        release: mpsc::Sender<()>,
        max_live: Arc<AtomicUsize>,
    }
    /// Offline Fish-local stand-in. "hold" streams 1600 frames of 0.25, then
    /// withholds the rest until released; "fail" answers HTTP 500; "tone N"
    /// streams 1600 frames of N. Records concurrent connections.
    fn stream_server() -> StreamServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/tts", listener.local_addr().unwrap());
        let (sender, requests) = mpsc::channel();
        let (release, held) = mpsc::channel::<()>();
        let held = Arc::new(Mutex::new(held));
        let live = Arc::new(AtomicUsize::new(0));
        let max_live = Arc::new(AtomicUsize::new(0));
        let observed = max_live.clone();
        std::thread::spawn(move || {
            for socket in listener.incoming() {
                let Ok(mut socket) = socket else { return };
                let (sender, held, live, max_live) =
                    (sender.clone(), held.clone(), live.clone(), observed.clone());
                std::thread::spawn(move || {
                    let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                    max_live.fetch_max(now, Ordering::SeqCst);
                    let mut request = Vec::new();
                    let mut buffer = [0; 4096];
                    let body = loop {
                        let n = socket.read(&mut buffer).unwrap_or(0);
                        assert!(n > 0, "request ended early");
                        request.extend_from_slice(&buffer[..n]);
                        let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                            continue;
                        };
                        let length: usize = String::from_utf8_lossy(&request[..end])
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|n| n.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break request[end + 4..end + 4 + length].to_vec();
                        }
                    };
                    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let text = json["text"].as_str().unwrap().to_owned();
                    let _ = sender.send(text.clone());
                    if text == "fail" {
                        let _ = socket.write_all(
                            b"HTTP/1.1 500 Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    } else {
                        let value = text
                            .strip_prefix("tone ")
                            .map_or(8192, |n| n.parse().unwrap());
                        let _ = socket.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nConnection: close\r\n\r\n",
                        );
                        let mut wav = mono_wav_header();
                        wav.extend(tone(value, 1600));
                        let _ = socket.write_all(&wav);
                        let _ = socket.flush();
                        if text == "hold" {
                            let _ = held.lock().unwrap().recv_timeout(Duration::from_secs(10));
                            let _ = socket.write_all(&tone(value, 1600));
                        }
                    }
                    drop(socket);
                    live.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        StreamServer {
            url,
            requests,
            release,
            max_live,
        }
    }
    /// Concurrent requests may arrive in either order; returns them sorted.
    fn arrivals(server: &StreamServer, count: usize) -> Vec<String> {
        let mut texts: Vec<String> = (0..count)
            .map(|_| {
                server
                    .requests
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap()
            })
            .collect();
        texts.sort();
        texts
    }
    fn stream_config(url: &str) -> VoiceConfig {
        VoiceConfig {
            endpoint: url.into(),
            delivery: VoiceDelivery::HttpWavStream,
            ..VoiceConfig::preset(VoiceAdapter::FishLocal)
        }
    }
    fn next_event(worker: &VoiceWorker) -> VoiceEvent {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(event) = worker.try_recv() {
                return event;
            }
            assert!(Instant::now() < deadline, "no voice event");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn next_stream(worker: &VoiceWorker) -> crate::voice_stream::StreamingVoice {
        match next_event(worker) {
            VoiceEvent::Streaming { audio, .. } => audio,
            other => panic!("expected stream, got {other:?}"),
        }
    }
    /// Plays a stream to its end, returning every non-silent sample.
    fn play(audio: &crate::voice_stream::StreamingVoice) -> Vec<f32> {
        use bevy::audio::Decodable;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut samples = Vec::new();
        for sample in audio.decoder() {
            assert!(Instant::now() < deadline, "stream never ended");
            if sample != 0.0 {
                samples.push(sample);
            }
        }
        samples
    }
    fn assert_quiet(worker: &VoiceWorker, wait: Duration) {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            assert!(worker.try_recv().is_none(), "unexpected voice event");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn http_prefetch_overlaps_playback_in_order_within_bounds() {
        let server = stream_server();
        let config = stream_config(&server.url);
        let worker = VoiceWorker::new();
        for text in ["hold", "tone 16384", "tone 4096", "tone 2048"] {
            worker
                .enqueue(7, config.clone(), text.into(), None)
                .unwrap();
        }
        // Bounded: two requests run, two wait, a fifth is refused.
        assert!(
            worker
                .enqueue(7, config.clone(), "tone 1024".into(), None)
                .unwrap_err()
                .contains("full")
        );
        let first = next_stream(&worker);
        // Overlap: the next sentence is requested while the first still streams.
        assert_eq!(arrivals(&server, 2), ["hold", "tone 16384"]);
        assert!(!first.is_finished() && !first.is_failed());
        assert!(!worker.can_prefetch());
        assert!(
            server
                .requests
                .recv_timeout(Duration::from_millis(200))
                .is_err()
        );
        // Ordered: nothing overtakes the stream that is still playing.
        assert_quiet(&worker, Duration::from_millis(200));
        server.release.send(()).unwrap();
        // Uninterrupted: every sample of the first stream plays.
        let samples = play(&first);
        assert_eq!(samples.len(), 3200);
        assert!(samples.iter().all(|&s| s == 0.25));
        assert!(!first.is_failed());
        for expected in [0.5, 0.125, 0.0625] {
            let samples = play(&next_stream(&worker));
            assert_eq!(samples.len(), 1600);
            assert!(samples.iter().all(|&s| s == expected));
        }
        assert_eq!(arrivals(&server, 2), ["tone 2048", "tone 4096"]);
        assert!(server.max_live.load(Ordering::SeqCst) <= HTTP_SLOTS);
        assert_quiet(&worker, Duration::from_millis(100));
    }
    #[test]
    fn http_prefetch_cancel_suppresses_whole_turn_and_frees_slots() {
        let server = stream_server();
        let config = stream_config(&server.url);
        let worker = VoiceWorker::new();
        worker
            .enqueue(1, config.clone(), "hold".into(), None)
            .unwrap();
        worker
            .enqueue(1, config.clone(), "tone 16384".into(), None)
            .unwrap();
        worker
            .enqueue(1, config.clone(), "tone 4096".into(), None)
            .unwrap();
        let first = next_stream(&worker);
        assert_eq!(arrivals(&server, 2), ["hold", "tone 16384"]);
        worker.cancel();
        assert!(first.is_failed());
        server.release.send(()).unwrap();
        assert_quiet(&worker, Duration::from_millis(300));
        // The queued third chunk belonged to the cancelled turn.
        assert!(
            server
                .requests
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        let wait = Duration::from_secs(10);
        let deadline = Instant::now() + wait;
        while !worker.can_prefetch() {
            assert!(Instant::now() < deadline, "slots never released");
            std::thread::sleep(Duration::from_millis(5));
        }
        worker.start(2, config, "tone 2048".into(), None).unwrap();
        let event = next_event(&worker);
        assert_eq!(event.id(), 2);
        let VoiceEvent::Streaming { audio, .. } = event else {
            panic!("expected stream");
        };
        assert!(play(&audio).iter().all(|&s| s == 0.0625));
        assert_eq!(server.requests.recv_timeout(wait).unwrap(), "tone 2048");
    }
    #[test]
    fn http_prefetch_failure_is_reported_after_earlier_audio() {
        let server = stream_server();
        let config = stream_config(&server.url);
        let worker = VoiceWorker::new();
        worker
            .enqueue(3, config.clone(), "hold".into(), None)
            .unwrap();
        worker.enqueue(3, config, "fail".into(), None).unwrap();
        let first = next_stream(&worker);
        assert_eq!(arrivals(&server, 2), ["fail", "hold"]);
        assert_quiet(&worker, Duration::from_millis(200));
        server.release.send(()).unwrap();
        assert_eq!(play(&first).len(), 3200);
        match next_event(&worker) {
            VoiceEvent::Failed { id, message } => {
                assert_eq!(id, 3);
                assert_eq!(message, "Speech provider returned HTTP 500");
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }
    #[test]
    fn hosted_request_and_audio() {
        let (endpoint, task) = server(200, "audio/wav", b"RIFFtest");
        let config = VoiceConfig {
            endpoint,
            delivery: VoiceDelivery::Buffered,
            reference_id: "speaker-id".into(),
            speed: 1.2,
            ..Default::default()
        };
        let audio = synthesize(&config, "Hello", Some("test-session-key"), || false).unwrap();
        assert_eq!(audio.bytes, b"RIFFtest");
        assert_eq!(audio.mime, "audio/wav");
        let request = task.join().unwrap();
        assert!(request.contains("model: s2.1-pro-free"));
        let body: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["reference_id"], "speaker-id");
        assert!(body.get("streaming").is_none());
        assert!((body["prosody"]["speed"].as_f64().unwrap() - 1.2).abs() < 0.0001);
        assert_eq!(body["min_chunk_length"], 20);
        assert_eq!(body["condition_on_previous_chunks"], true);
        assert!(
            serde_json::to_string(&config)
                .unwrap()
                .find("test-session-key")
                .is_none()
        );
    }
    #[test]
    fn local_omits_unsupported_prosody_and_model() {
        let (endpoint, task) = server(200, "audio/wav", b"RIFFtest");
        let config = VoiceConfig {
            endpoint,
            delivery: VoiceDelivery::Buffered,
            ..VoiceConfig::preset(VoiceAdapter::FishLocal)
        };
        synthesize(&config, "Hello", None, || false).unwrap();
        let request = task.join().unwrap();
        assert!(!request.contains("prosody"));
        assert!(!request.contains("min_chunk_length"));
        assert!(!request.contains("condition_on_previous_chunks"));
        assert!(!request.contains("model:"));
        assert!(!request.contains("authorization:"));
    }
    #[test]
    fn openai_payload() {
        let (endpoint, task) = server(200, "audio/mpeg", b"ID3test");
        let config = VoiceConfig {
            endpoint,
            format: "mp3".into(),
            ..VoiceConfig::preset(VoiceAdapter::OpenAi)
        };
        synthesize(&config, "Hello", None, || false).unwrap();
        let request = task.join().unwrap();
        assert!(request.contains("\"input\":\"Hello\""));
        assert!(request.contains("\"response_format\":\"mp3\""));
        assert!(!request.contains("min_chunk_length"));
        assert!(!request.contains("condition_on_previous_chunks"));
    }
    #[test]
    fn http_low_preset_preserves_identity_opt_in_and_unsupported_adapters() {
        for adapter in [
            VoiceAdapter::FishHosted,
            VoiceAdapter::FishLocal,
            VoiceAdapter::OpenAi,
            VoiceAdapter::Vercel,
        ] {
            for enabled in [false, true] {
                let mut config = VoiceConfig {
                    adapter,
                    enabled,
                    output: VoiceOutput::LocalOnly,
                    endpoint: "https://custom.example/speech".into(),
                    model: "drama-3".into(),
                    reference_id: "test-reference".into(),
                    voice: "test-voice".into(),
                    api_key_env: "TEST_VOICE_KEY".into(),
                    direction: "Warm".into(),
                    speed: 1.25,
                    format: "mp3".into(),
                    latency: "normal".into(),
                    delivery: VoiceDelivery::TimestampSse,
                    chunk_length: 250,
                    min_chunk_length: 80,
                    condition_on_previous_chunks: false,
                };
                let before = serde_json::to_value(&config).unwrap();
                assert_eq!(
                    config.use_http_low_latency(),
                    adapter == VoiceAdapter::FishHosted
                );
                let after = serde_json::to_value(&config).unwrap();
                if adapter == VoiceAdapter::FishHosted {
                    let mut expected = before;
                    expected["delivery"] = serde_json::json!(VoiceDelivery::HttpWavStream);
                    expected["format"] = serde_json::json!("wav");
                    expected["latency"] = serde_json::json!("low");
                    expected["chunk_length"] = serde_json::json!(160);
                    expected["min_chunk_length"] = serde_json::json!(20);
                    expected["condition_on_previous_chunks"] = serde_json::json!(true);
                    assert_eq!(after, expected);
                } else {
                    assert_eq!(after, before);
                }
                config.use_http_low_latency();
                assert_eq!(serde_json::to_value(&config).unwrap(), after);
            }
        }
    }

    #[test]
    fn live_pipeline_migration_preserves_voice_identity_and_is_idempotent() {
        for adapter in [
            VoiceAdapter::FishHosted,
            VoiceAdapter::FishLocal,
            VoiceAdapter::OpenAi,
            VoiceAdapter::Vercel,
        ] {
            let mut config = VoiceConfig {
                enabled: true,
                output: VoiceOutput::LocalOnly,
                adapter,
                endpoint: "https://custom.example/speech".into(),
                model: "my-model".into(),
                reference_id: "my-reference".into(),
                voice: "my-voice".into(),
                format: "mp3".into(),
                speed: 1.25,
                delivery: VoiceDelivery::TimestampSse,
                latency: "normal".into(),
                chunk_length: 250,
                min_chunk_length: 80,
                condition_on_previous_chunks: false,
                direction: "Warm and relaxed".into(),
                api_key_env: "MY_VOICE_KEY".into(),
            };
            let mut before = serde_json::to_value(&config).unwrap();
            config.use_live_pipeline();
            match adapter {
                VoiceAdapter::FishHosted => {
                    assert_eq!(config.delivery, VoiceDelivery::LiveWebSocket);
                    assert_eq!(config.format, "wav");
                    assert_eq!(config.latency, "balanced");
                    assert_eq!(config.chunk_length, 160);
                    assert_eq!(config.min_chunk_length, 20);
                    assert!(config.condition_on_previous_chunks);
                }
                VoiceAdapter::FishLocal => {
                    assert_eq!(config.delivery, VoiceDelivery::HttpWavStream);
                    assert_eq!(config.format, "wav");
                    assert_eq!(config.chunk_length, 250);
                }
                VoiceAdapter::OpenAi | VoiceAdapter::Vercel => {
                    assert_eq!(config.delivery, VoiceDelivery::Buffered);
                    assert_eq!(config.format, "mp3");
                }
            }
            let migrated = serde_json::to_value(&config).unwrap();
            config.use_live_pipeline();
            assert_eq!(serde_json::to_value(&config).unwrap(), migrated);
            let mut after = migrated;
            for field in [
                "delivery",
                "format",
                "latency",
                "chunk_length",
                "min_chunk_length",
                "condition_on_previous_chunks",
            ] {
                before.as_object_mut().unwrap().remove(field);
                after.as_object_mut().unwrap().remove(field);
            }
            assert_eq!(before, after, "voice identity changed for {adapter:?}");
        }
    }
    #[test]
    fn speech_presets_use_supported_transports_and_speak_by_default() {
        let hosted = VoiceConfig::default();
        assert!(hosted.enabled);
        assert_eq!(hosted.model, "s2.1-pro-free");
        assert_eq!(hosted.delivery, VoiceDelivery::LiveWebSocket);
        assert_eq!(hosted.latency, "balanced");
        assert_eq!(hosted.chunk_length, 160);
        assert_eq!(
            VoiceConfig::preset(VoiceAdapter::FishLocal).delivery,
            VoiceDelivery::HttpWavStream
        );
        for adapter in [VoiceAdapter::OpenAi, VoiceAdapter::Vercel] {
            let preset = VoiceConfig::preset(adapter);
            assert_eq!(preset.delivery, VoiceDelivery::Buffered);
            assert!(preset.enabled);
        }
    }
    #[test]
    fn hosted_tuning_migrates_old_configs_and_rejects_out_of_range() {
        let old: VoiceConfig =
            serde_json::from_str(r#"{"adapter":"FishHosted","chunk_length":250}"#).unwrap();
        assert_eq!(old.chunk_length, 250);
        assert_eq!(old.min_chunk_length, 20);
        assert!(old.condition_on_previous_chunks);
        let set: VoiceConfig =
            serde_json::from_str(r#"{"min_chunk_length":0,"condition_on_previous_chunks":false}"#)
                .unwrap();
        assert_eq!(set.min_chunk_length, 0);
        assert!(!set.condition_on_previous_chunks);
        assert!(serde_json::from_str::<VoiceConfig>(r#"{"min_chunk_length":-1}"#).is_err());
        assert!(
            serde_json::from_str::<VoiceConfig>(r#"{"condition_on_previous_chunks":"yes"}"#)
                .is_err()
        );
        let hosted = VoiceConfig {
            min_chunk_length: 101,
            ..Default::default()
        };
        // Fails locally before credentials or network are consulted.
        assert_eq!(
            synthesize(&hosted, "Hello", Some("offline-test-key"), || false).unwrap_err(),
            "Fish minimum chunk length must be between 0 and 100"
        );
        let unknown = VoiceConfig {
            model: "unknown".into(),
            ..Default::default()
        };
        assert!(
            synthesize(&unknown, "Hello", Some("offline-test-key"), || false)
                .unwrap_err()
                .contains("documented Fish model")
        );
    }
    #[test]
    fn rejects_http_errors_without_echoing_provider_body() {
        let (endpoint, task) = server(401, "application/json", b"secret-provider-detail");
        let config = VoiceConfig {
            endpoint,
            ..VoiceConfig::preset(VoiceAdapter::FishLocal)
        };
        let error = synthesize(&config, "Hello", None, || false).unwrap_err();
        task.join().unwrap();
        assert_eq!(error, "Speech provider returned HTTP 401");
    }
    #[test]
    fn busy_speech_keeps_only_newest_and_cancel_clears_queue() {
        let worker = VoiceWorker::new();
        worker.slots.store(HTTP_SLOTS, Ordering::SeqCst);
        worker
            .start(
                1,
                VoiceConfig {
                    delivery: VoiceDelivery::Buffered,
                    ..Default::default()
                },
                "old".into(),
                None,
            )
            .unwrap();
        worker
            .start(
                2,
                VoiceConfig {
                    delivery: VoiceDelivery::Buffered,
                    ..Default::default()
                },
                "new".into(),
                None,
            )
            .unwrap();
        let pending = worker.pending.lock().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, 2);
        drop(pending);
        worker.cancel();
        assert!(worker.pending.lock().unwrap().is_empty());
        worker.slots.store(0, Ordering::SeqCst);
        assert!(worker.try_recv().is_none());
    }
    #[test]
    fn pending_speech_starts_after_previous_worker_releases_slot() {
        let worker = VoiceWorker::new();
        worker.slots.store(HTTP_SLOTS, Ordering::SeqCst);
        let config = VoiceConfig {
            format: "unsupported".into(),
            delivery: VoiceDelivery::Buffered,
            ..Default::default()
        };
        worker.start(1, config.clone(), "old".into(), None).unwrap();
        worker.start(2, config, "new".into(), None).unwrap();
        // Invalid format returns locally without any network access, allowing
        // deterministic observation of the queued request that actually ran.
        worker.slots.store(0, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(event) = worker.try_recv() {
                assert_eq!(event.id(), 2);
                assert!(matches!(event, VoiceEvent::Failed { .. }));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "queued worker did not complete"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(worker.pending.lock().unwrap().is_empty());
    }
    #[test]
    fn cancel_and_stale_events_are_suppressed() {
        let worker = VoiceWorker::new();
        let generation = worker.generation.load(Ordering::SeqCst);
        worker
            .sender
            .send((
                generation,
                VoiceEvent::Failed {
                    id: 1,
                    message: "old".into(),
                },
            ))
            .unwrap();
        worker.cancel();
        assert!(worker.try_recv().is_none());
        assert_eq!(
            synthesize(
                &VoiceConfig::preset(VoiceAdapter::FishLocal),
                "Hello",
                None,
                || true
            )
            .unwrap_err(),
            "Speech cancelled"
        );
    }
    #[test]
    fn rejects_unbounded_or_non_audio_responses() {
        let (endpoint, task) = server(200, "application/json", b"{}");
        assert!(
            synthesize(
                &VoiceConfig {
                    endpoint,
                    ..VoiceConfig::preset(VoiceAdapter::FishLocal)
                },
                "Hello",
                None,
                || false
            )
            .is_err()
        );
        task.join().unwrap();
        assert!(synthesize(&VoiceConfig::default(), &"a".repeat(12001), None, || false).is_err());
    }
}
