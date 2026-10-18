//! Extensible HTTP speech adapters. Credentials are resolved only for a request,
//! never serialized. Local Fish Speech uses the model loaded by its server.
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
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
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    pub enabled: bool,
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
            enabled: false,
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
    pub fn preset(adapter: VoiceAdapter) -> Self {
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
                config.endpoint = "https://api.openai.com/v1/audio/speech".into();
                config.model = "tts-1".into();
                config.api_key_env = "OPENAI_API_KEY".into();
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
struct PendingSpeech {
    generation: u64,
    id: u64,
    config: VoiceConfig,
    text: String,
    session_key: Option<String>,
}
pub struct VoiceWorker {
    generation: Arc<AtomicU64>,
    sender: mpsc::Sender<(u64, VoiceEvent)>,
    receiver: mpsc::Receiver<(u64, VoiceEvent)>,
    busy: Arc<std::sync::atomic::AtomicBool>,
    pending: Mutex<Option<PendingSpeech>>,
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
            busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pending: Mutex::new(None),
        }
    }
    /// Cancellation clears queued speech and suppresses playback/events; an
    /// in-flight HTTP call retains the single worker slot until it finishes.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.pending.lock().unwrap() = None;
    }
    /// Newest wins: retain at most one pending request while a cancelled HTTP
    /// worker finishes. Polling starts it as soon as the worker slot is free.
    pub fn start(
        &self,
        id: u64,
        config: VoiceConfig,
        text: String,
        session_key: Option<String>,
    ) -> Result<(), String> {
        if config.adapter == VoiceAdapter::Vercel && config.delivery != VoiceDelivery::Buffered {
            return Err("Vercel speech currently requires buffered delivery".into());
        }
        if config.delivery == VoiceDelivery::TimestampSse || config.delivery.is_websocket() {
            return Err("Live speech requires its dedicated Fish transport".into());
        }
        if text.trim().is_empty() || text.chars().count() > 12_000 {
            return Err("Speech text must contain 1–12000 characters".into());
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.pending.lock().unwrap() = Some(PendingSpeech {
            generation,
            id,
            config,
            text,
            session_key,
        });
        self.start_pending();
        Ok(())
    }
    fn start_pending(&self) {
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(request) = self.pending.lock().unwrap().take() else {
            self.busy.store(false, Ordering::SeqCst);
            return;
        };
        let PendingSpeech {
            generation,
            id,
            config,
            text,
            session_key,
        } = request;
        let active = self.generation.clone();
        let busy = self.busy.clone();
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let cancelled = || active.load(Ordering::SeqCst) != generation;
            let result = if config.delivery == VoiceDelivery::HttpWavStream {
                let budget = crate::voice_stream::StreamBudget::default();
                prepare_response(&config, &text, session_key.as_deref(), cancelled)
                    .and_then(|(response, _)| {
                        crate::voice_stream::receive_wav_with_budget(
                            response,
                            active.clone(),
                            generation,
                            &budget,
                            |audio| {
                                if !cancelled() {
                                    let _ = sender
                                        .send((generation, VoiceEvent::Streaming { id, audio }));
                                }
                            },
                        )
                        .map_err(|error| format!("Fish HTTP WAV: {error}"))
                    })
                    .map(|()| None)
            } else {
                synthesize(&config, &text, session_key.as_deref(), cancelled).map(Some)
            };
            if !cancelled() {
                let event = match result {
                    Ok(Some(audio)) => Some(VoiceEvent::Ready { id, audio }),
                    Ok(None) => None,
                    Err(message) => Some(VoiceEvent::Failed { id, message }),
                };
                if let Some(event) = event {
                    let _ = sender.send((generation, event));
                }
            }
            busy.store(false, Ordering::SeqCst);
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
const MAX_AUDIO: u64 = 32 * 1024 * 1024;
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
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(90))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize speech HTTP client")?;
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
    fn live_pipeline_migration_preserves_voice_identity_and_is_idempotent() {
        for adapter in [
            VoiceAdapter::FishHosted,
            VoiceAdapter::FishLocal,
            VoiceAdapter::OpenAi,
            VoiceAdapter::Vercel,
        ] {
            let mut config = VoiceConfig {
                enabled: true,
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
    fn speech_presets_use_supported_transports_without_enabling_speech() {
        let hosted = VoiceConfig::default();
        assert!(!hosted.enabled);
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
            assert!(!preset.enabled);
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
        worker.busy.store(true, Ordering::SeqCst);
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
        assert_eq!(worker.pending.lock().unwrap().as_ref().unwrap().id, 2);
        worker.cancel();
        assert!(worker.pending.lock().unwrap().is_none());
        worker.busy.store(false, Ordering::SeqCst);
        assert!(worker.try_recv().is_none());
    }
    #[test]
    fn pending_speech_starts_after_previous_worker_releases_slot() {
        let worker = VoiceWorker::new();
        worker.busy.store(true, Ordering::SeqCst);
        let config = VoiceConfig {
            format: "unsupported".into(),
            delivery: VoiceDelivery::Buffered,
            ..Default::default()
        };
        worker.start(1, config.clone(), "old".into(), None).unwrap();
        worker.start(2, config, "new".into(), None).unwrap();
        // Invalid format returns locally without any network access, allowing
        // deterministic observation of the queued request that actually ran.
        worker.busy.store(false, Ordering::SeqCst);
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
        assert!(worker.pending.lock().unwrap().is_none());
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
