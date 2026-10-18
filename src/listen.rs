//! Explicit microphone recording with OpenAI-compatible, Fish and Vercel transcription.
//! VAD uses local energy or optional Silero ONNX inference. Audio is
//! downmixed and nearest-neighbor resampled to 16 kHz (no anti-alias filter).
use base64::Engine;
use cpal::{
    Sample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};
const RATE: usize = 16_000;
const MAX_SAMPLES: usize = RATE * 30;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttBackend {
    #[default]
    OpenAiCompatible,
    FishAsr,
    VercelGateway,
}
impl SttBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "OpenAI-compatible / OpenRouter / local",
            Self::FishAsr => "Fish ASR",
            Self::VercelGateway => "Vercel Gateway",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ListenConfig {
    pub backend: SttBackend,
    pub language: String,
    pub silero_enabled: bool,
    pub silero_model: String,
    pub silero_runtime: String,
    pub silero_threshold: f32,
    pub enabled: bool,
    pub endpoint: String,
    pub model: String,
    pub api_key_env: String,
    pub vad_threshold: f32,
    pub trailing_silence_ms: u32,
}
impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            backend: SttBackend::default(),
            language: String::new(),
            silero_enabled: false,
            silero_model: String::new(),
            silero_runtime: String::new(),
            silero_threshold: 0.5,
            enabled: false,
            endpoint: "https://api.openai.com/v1/audio/transcriptions".into(),
            model: "whisper-1".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            vad_threshold: 0.015,
            trailing_silence_ms: 1200,
        }
    }
}
impl ListenConfig {
    /// Changes routing only; microphone and VAD preferences are preserved.
    pub fn preset(&mut self, backend: SttBackend) {
        self.backend = backend;
        let (endpoint, model, key) = match backend {
            SttBackend::OpenAiCompatible => (
                "https://api.openai.com/v1/audio/transcriptions",
                "whisper-1",
                "OPENAI_API_KEY",
            ),
            SttBackend::FishAsr => (
                "https://api.fish.audio/v1/asr",
                "transcribe-1",
                "FISH_API_KEY",
            ),
            SttBackend::VercelGateway => (
                "https://ai-gateway.vercel.sh/v4/ai/transcription-model",
                "fish-audio/transcribe-1",
                "AI_GATEWAY_API_KEY",
            ),
        };
        self.endpoint = endpoint.into();
        self.model = model.into();
        self.api_key_env = key.into();
    }
    pub fn openrouter_preset(&mut self) {
        self.backend = SttBackend::OpenAiCompatible;
        self.endpoint = "https://openrouter.ai/api/v1/audio/transcriptions".into();
        self.model = "fish-audio/transcribe-1".into();
        self.api_key_env = "OPENROUTER_API_KEY".into();
    }
}
/// Receiver is mutex-wrapped so this state can be a Bevy resource.
#[derive(bevy::prelude::Resource)]
pub struct ListeningState {
    pub config: ListenConfig,
    /// Session-only credential; never part of serialized config.
    pub session_key: String,
    pub status: String,
    pub vad_status: Arc<Mutex<String>>,
    busy: Arc<AtomicBool>,
    recording: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    tx: mpsc::Sender<(u64, Result<String, String>)>,
    rx: Mutex<mpsc::Receiver<(u64, Result<String, String>)>>,
}
impl Default for ListeningState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            config: ListenConfig::default(),
            session_key: String::new(),
            status: "Microphone idle".into(),
            vad_status: Arc::new(Mutex::new("Energy VAD".into())),
            busy: Arc::new(AtomicBool::new(false)),
            recording: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            tx,
            rx: Mutex::new(rx),
        }
    }
}
impl ListeningState {
    /// Disabling immediately invalidates queued transcripts and prevents a
    /// recording that has not yet uploaded from starting transcription.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
        if !enabled {
            self.cancel();
        }
    }
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::SeqCst)
    }
    pub fn start(&mut self) {
        if !self.config.enabled || self.is_busy() {
            return;
        }
        if !self.config.vad_threshold.is_finite()
            || !(0.001..=1.0).contains(&self.config.vad_threshold)
        {
            self.status = "Choose a VAD threshold between 0.001 and 1".into();
            return;
        }
        if self.config.silero_enabled
            && (!self.config.silero_threshold.is_finite()
                || !(0.01..=0.99).contains(&self.config.silero_threshold))
        {
            self.status = "Choose a Silero probability threshold between 0.01 and 0.99".into();
            return;
        }
        let vad_status = self.vad_status.clone();
        self.busy.store(true, Ordering::SeqCst);
        self.stop.store(false, Ordering::SeqCst);
        let id = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (config, key) = (self.config.clone(), self.session_key.clone());
        let (busy, recording, stop, generation, tx) = (
            self.busy.clone(),
            self.recording.clone(),
            self.stop.clone(),
            self.generation.clone(),
            self.tx.clone(),
        );
        self.status =
            "Recording; speech followed by silence stops automatically (30 s maximum)".into();
        std::thread::spawn(move || {
            let cancelled = || generation.load(Ordering::SeqCst) != id;
            let result = capture(&config, &stop, &recording, &cancelled, &vad_status)
                .and_then(|samples| transcribe(&config, &wav(&samples), Some(&key), &cancelled));
            recording.store(false, Ordering::SeqCst);
            if !cancelled() {
                let _ = tx.send((id, result));
            }
            busy.store(false, Ordering::SeqCst);
        });
    }
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.status = "Finishing recording / transcribing…".into();
    }
    /// In-flight HTTP finishes within its timeout; its result is discarded.
    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
        self.status = "Cancelled; worker may still be finishing".into();
    }
    pub fn poll(&mut self) -> Option<String> {
        let rx = self.rx.lock().ok()?;
        while let Ok((id, result)) = rx.try_recv() {
            if id != self.generation.load(Ordering::SeqCst) {
                continue;
            }
            match result {
                Ok(text) => {
                    self.status = "Transcript added to composer; review before sending".into();
                    return Some(text);
                }
                Err(e) => self.status = e,
            }
        }
        None
    }
}
impl Drop for ListeningState {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
    }
}
struct Recording {
    samples: Vec<i16>,
    phase: u64,
    source_rate: u32,
    channels: usize,
    threshold: f32,
    silence_limit: usize,
    silence: usize,
    speech: bool,
    done: bool,
    neural: bool,
    energy: f64,
    window: usize,
}
impl Recording {
    fn new(rate: u32, channels: usize, c: &ListenConfig) -> Self {
        Self {
            samples: Vec::with_capacity(MAX_SAMPLES),
            phase: 0,
            source_rate: rate,
            channels,
            threshold: c.vad_threshold,
            silence_limit: RATE * c.trailing_silence_ms.clamp(200, 5000) as usize / 1000,
            silence: 0,
            speech: false,
            done: false,
            neural: false,
            energy: 0.,
            window: 0,
        }
    }
    fn feed<T: cpal::Sample>(&mut self, data: &[T])
    where
        f32: cpal::FromSample<T>,
    {
        for frame in data.chunks_exact(self.channels) {
            if self.done {
                break;
            }
            self.phase += RATE as u64;
            if self.phase < self.source_rate as u64 {
                continue;
            }
            let value =
                frame.iter().map(|x| f32::from_sample(*x)).sum::<f32>() / self.channels as f32;
            let value = if value.is_finite() {
                value.clamp(-1., 1.)
            } else {
                0.
            };
            while self.phase >= self.source_rate as u64 && !self.done {
                self.phase -= self.source_rate as u64;
                self.samples.push((value * i16::MAX as f32) as i16);
                self.energy += (value as f64).powi(2);
                self.window += 1;
                if self.window == 160 {
                    let loud = (self.energy / 160.).sqrt() >= self.threshold as f64;
                    if !self.neural && loud {
                        self.speech = true;
                        self.silence = 0;
                    } else if !self.neural && self.speech {
                        self.silence += 160;
                    }
                    self.energy = 0.;
                    self.window = 0;
                }
                self.done = self.samples.len() >= MAX_SAMPLES
                    || (self.speech && self.silence >= self.silence_limit);
            }
        }
    }
}
fn capture(
    c: &ListenConfig,
    stop: &AtomicBool,
    recording: &AtomicBool,
    cancelled: &impl Fn() -> bool,
    vad_status: &Mutex<String>,
) -> Result<Vec<i16>, String> {
    let mut silero = if c.silero_enabled {
        match crate::silero::SileroVad::load(&c.silero_model, &c.silero_runtime) {
            Ok(model) => {
                if let Ok(mut status) = vad_status.lock() {
                    *status = "Silero CPU VAD active (16 kHz)".into();
                }
                Some(model)
            }
            Err(error) => {
                if let Ok(mut status) = vad_status.lock() {
                    *status = format!("Energy VAD fallback: {error}");
                }
                None
            }
        }
    } else {
        if let Ok(mut status) = vad_status.lock() {
            *status = "Energy VAD active".into();
        }
        None
    };
    if cancelled() {
        return Err("Recording cancelled".into());
    }

    let device = cpal::default_host()
        .default_input_device()
        .ok_or("No microphone input device available")?;
    let supported = device
        .default_input_config()
        .map_err(|_| "Cannot read microphone format")?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    if config.channels == 0 || config.sample_rate == 0 {
        return Err("Invalid microphone format".into());
    }
    let data = Arc::new(Mutex::new(Recording::new(
        config.sample_rate,
        config.channels as usize,
        c,
    )));
    data.lock().map_err(|_| "Microphone buffer failed")?.neural = silero.is_some();
    let mut processed = 0usize;
    let failed = Arc::new(AtomicBool::new(false));
    macro_rules! stream {
        ($t:ty) => {{
            let d = data.clone();
            let f = failed.clone();
            device.build_input_stream(
                &config,
                move |samples: &[$t], _| {
                    if let Ok(mut d) = d.lock() {
                        d.feed(samples);
                    }
                },
                move |_| {
                    f.store(true, Ordering::SeqCst);
                },
                None,
            )
        }};
    }
    let stream = match format {
        cpal::SampleFormat::F32 => stream!(f32),
        cpal::SampleFormat::I16 => stream!(i16),
        cpal::SampleFormat::U16 => stream!(u16),
        cpal::SampleFormat::I32 => stream!(i32),
        _ => return Err("Microphone sample format is unsupported".into()),
    }
    .map_err(|_| "Cannot open microphone stream")?;
    if cancelled() {
        return Err("Recording cancelled".into());
    }
    stream.play().map_err(|_| "Cannot start microphone")?;
    recording.store(true, Ordering::SeqCst);
    let started = std::time::Instant::now();
    while !stop.load(Ordering::SeqCst)
        && !cancelled()
        && !failed.load(Ordering::SeqCst)
        && started.elapsed() < Duration::from_secs(30)
    {
        // Copy one bounded frame under the lock; inference never holds the callback lock.
        while silero.is_some() && !cancelled() {
            let frame: Option<[i16; 512]> = {
                let d = data.lock().map_err(|_| "Microphone buffer failed")?;
                d.samples
                    .get(processed..processed + 512)
                    .and_then(|s| s.try_into().ok())
            };
            let Some(frame) = frame else { break };
            let result = silero.as_mut().unwrap().probability(&frame);
            processed += 512;
            let mut d = data.lock().map_err(|_| "Microphone buffer failed")?;
            match result {
                Ok(p) => {
                    if p >= c.silero_threshold {
                        d.speech = true;
                        d.silence = 0;
                    } else if d.speech {
                        d.silence += 512;
                    }
                    if d.speech && d.silence >= d.silence_limit {
                        d.done = true;
                    }
                }
                Err(error) => {
                    d.neural = false;
                    d.silence = 0;
                    silero = None;
                    if let Ok(mut status) = vad_status.lock() {
                        *status = format!("Energy VAD fallback: {error}");
                    }
                }
            }
            if d.done {
                break;
            }
        }
        if data.lock().map_err(|_| "Microphone buffer failed")?.done {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
    recording.store(false, Ordering::SeqCst);
    if cancelled() {
        return Err("Recording cancelled".into());
    }
    if failed.load(Ordering::SeqCst) {
        return Err("Microphone stream failed".into());
    }
    let mut data = data.lock().map_err(|_| "Microphone buffer failed")?;
    if !data.speech {
        return Err("No speech detected; adjust the selected VAD threshold and retry".into());
    }
    Ok(std::mem::take(&mut data.samples))
}
fn wav(samples: &[i16]) -> Vec<u8> {
    let samples = &samples[..samples.len().min(MAX_SAMPLES)];
    let size = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(size as usize + 44);
    out.extend(b"RIFF");
    out.extend((size + 36).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((RATE as u32).to_le_bytes());
    out.extend((RATE as u32 * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(size.to_le_bytes());
    for s in samples {
        out.extend(s.to_le_bytes());
    }
    out
}
fn transcribe(
    c: &ListenConfig,
    audio: &[u8],
    session: Option<&str>,
    cancelled: &impl Fn() -> bool,
) -> Result<String, String> {
    if audio.len() > MAX_SAMPLES * 2 + 44 || audio.len() <= 44 {
        return Err("Recording is empty or too large".into());
    }
    let url = reqwest::Url::parse(&c.endpoint).map_err(|_| "Invalid transcription endpoint")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use an HTTP(S) endpoint without credentials, query, or fragment".into());
    }
    if c.model.trim().is_empty() {
        return Err("Set a transcription model".into());
    }
    if c.backend == SttBackend::FishAsr {
        if !matches!(c.model.as_str(), "transcribe-1" | "transcribe-1-pro") {
            return Err("Fish ASR model must be transcribe-1 or transcribe-1-pro".into());
        }
        if !c.language.is_empty()
            && (c.language.len() != 2 || !c.language.bytes().all(|b| b.is_ascii_lowercase()))
        {
            return Err("Fish language must be a lowercase two-letter ISO language code".into());
        }
    }
    if cancelled() {
        return Err("Transcription cancelled".into());
    }
    let env = if c.api_key_env.trim().is_empty() {
        None
    } else {
        std::env::var(c.api_key_env.trim()).ok()
    };
    let oidc = if c.backend == SttBackend::VercelGateway
        && c.api_key_env.trim() == "AI_GATEWAY_API_KEY"
        && env.as_ref().is_none_or(|value| value.trim().is_empty())
    {
        std::env::var("VERCEL_OIDC_TOKEN").ok()
    } else {
        None
    };
    let key = session
        .filter(|x| !x.trim().is_empty())
        .or(env.as_deref().filter(|x| !x.trim().is_empty()))
        .or(oidc.as_deref().filter(|x| !x.trim().is_empty()));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Cannot create transcription client")?;
    let request = client.post(url);
    let mut request = match c.backend {
        SttBackend::VercelGateway => request
            .header("ai-model-id", &c.model)
            .header("ai-gateway-protocol-version", "0.0.1")
            .header("ai-transcription-model-specification-version", "4")
            .json(&serde_json::json!({"audio": base64::engine::general_purpose::STANDARD.encode(audio), "mediaType": "audio/wav"})),
        backend => {
            let part = reqwest::blocking::multipart::Part::bytes(audio.to_vec())
                .file_name("recording.wav")
                .mime_str("audio/wav")
                .map_err(|_| "Cannot prepare recording")?;
            let mut form = reqwest::blocking::multipart::Form::new();
            if !c.language.trim().is_empty() {
                form = form.text("language", c.language.trim().to_owned());
            }
            if backend == SttBackend::FishAsr {
                form = form.part("audio", part).text("ignore_timestamps", "true");
                if c.model == "transcribe-1-pro" {
                    form = form.text("tag_audio_events", "false").text("diarize", "false");
                }
                request.header("model", &c.model).multipart(form)
            } else {
                request.multipart(form.text("model", c.model.clone()).text("response_format", "json").part("file", part))
            }
        }
    };
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .map_err(|_| "Transcription connection failed or timed out")?;
    if !response.status().is_success() {
        return Err(format!(
            "Transcription provider returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read transcription response")?;
    if cancelled() {
        return Err("Transcription cancelled".into());
    }
    if bytes.len() > 65536 {
        return Err("Transcription response exceeds limit".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid transcription response")?;
    let text = value
        .get("text")
        .and_then(|x| x.as_str())
        .ok_or("Transcription response has no text")?
        .trim();
    if text.is_empty() || text.chars().count() > 12000 {
        return Err("Transcript is empty or exceeds 12000 characters".into());
    }
    let text = if c.backend == SttBackend::FishAsr {
        clean_fish_transcript(text)
    } else {
        text.to_owned()
    };
    if text.is_empty() {
        return Err("Transcript contains no spoken text".into());
    }
    Ok(text)
}
fn clean_fish_transcript(text: &str) -> String {
    // Remove documented speaker markers and event cues, preserving UTF-8 speech.
    let mut rest = text;
    let mut out = String::new();
    while !rest.is_empty() {
        if rest.starts_with("<|speaker:") {
            if let Some(end) = rest.find("|>") {
                rest = &rest[end + 2..];
                continue;
            }
        } else if rest.starts_with('[')
            && let Some(end) = rest.find(']')
        {
            rest = &rest[end + 1..];
            continue;
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neural_mode_callback_only_buffers_and_respects_recording_bound() {
        let c = ListenConfig::default();
        let mut recording = Recording::new(16000, 1, &c);
        recording.neural = true;
        recording.feed(&[0.8_f32; 512]);
        assert!(
            !recording.speech,
            "Energy must not decide speech in neural mode"
        );
        assert_eq!(recording.samples.len(), 512);
        recording.feed(&vec![0.8_f32; MAX_SAMPLES]);
        assert!(recording.done);
        assert_eq!(recording.samples.len(), MAX_SAMPLES);
    }
    #[test]
    fn vad_silence_and_bound() {
        let c = ListenConfig {
            trailing_silence_ms: 200,
            ..Default::default()
        };
        let mut r = Recording::new(48000, 2, &c);
        r.feed(&vec![0f32; 48000]);
        assert!(!r.done);
        r.feed(&vec![0.2f32; 4800]);
        r.feed(&vec![0f32; 20000]);
        assert!(r.speech && r.done);
        assert!(r.samples.len() < MAX_SAMPLES);
        let mut r = Recording::new(16000, 1, &c);
        r.feed(&vec![0f32; MAX_SAMPLES + 100]);
        assert_eq!(r.samples.len(), MAX_SAMPLES);
        assert!(r.done);
    }
    #[test]
    fn wav_header_and_config_secrets() {
        let bytes = wav(&[1, -2]);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[24..28], &16000u32.to_le_bytes());
        assert_eq!(bytes.len(), 48);
        let mut state = ListeningState::default();
        state.session_key = "private".into();
        assert!(
            !serde_json::to_string(&state.config)
                .unwrap()
                .contains("private")
        );
    }
    #[test]
    fn stale_events_suppressed() {
        let mut s = ListeningState::default();
        s.tx.send((0, Ok("old".into()))).unwrap();
        s.cancel();
        assert!(s.poll().is_none());
    }
    #[test]
    fn disabling_invalidates_completed_transcripts_and_requests_stop() {
        let mut state = ListeningState::default();
        state.set_enabled(true);
        state.tx.send((0, Ok("previous recording".into()))).unwrap();
        state.busy.store(true, Ordering::SeqCst);
        state.set_enabled(false);
        assert!(!state.config.enabled);
        assert!(state.stop.load(Ordering::SeqCst));
        assert!(state.poll().is_none());
        // Keep the busy permit until the old worker actually exits.
        assert!(state.is_busy());
    }
    #[test]
    fn fake_http_multipart() {
        use std::{io::Write, net::TcpListener};
        for backend in [
            SttBackend::OpenAiCompatible,
            SttBackend::FishAsr,
            SttBackend::VercelGateway,
        ] {
            let server = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!(
                "http://{}/v1/audio/transcriptions",
                server.local_addr().unwrap()
            );
            let task = std::thread::spawn(move || {
                let (mut stream, _) = server.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut data = Vec::new();
                loop {
                    let mut buf = [0; 4096];
                    let n = stream.read(&mut buf).unwrap();
                    data.extend_from_slice(&buf[..n]);
                    if let Some(end) = data.windows(4).position(|x| x == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&data[..end]);
                        let len = head
                            .lines()
                            .find_map(|x| {
                                x.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|x| x.trim().parse::<usize>().ok())
                            })
                            .unwrap();
                        if data.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let body = b"{\"text\":\"hello test\"}";
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
                data
            });
            let mut c = ListenConfig::default();
            c.preset(backend);
            c.endpoint = endpoint;
            c.language = "en".into();
            assert_eq!(
                transcribe(&c, &wav(&[1; 160]), Some("fake-key"), &|| false).unwrap(),
                "hello test"
            );
            let request = task.join().unwrap();
            let text = String::from_utf8_lossy(&request);
            assert!(text.contains("authorization: Bearer fake-key"));
            match backend {
                SttBackend::OpenAiCompatible => {
                    assert!(text.contains("name=\"model\""));
                    assert!(text.contains("name=\"file\""));
                    assert!(request.windows(4).any(|x| x == b"RIFF"));
                }
                SttBackend::FishAsr => {
                    assert!(text.contains("model: transcribe-1\r\n"));
                    assert!(text.contains("name=\"audio\""));
                    assert!(text.contains("name=\"ignore_timestamps\""));
                    assert!(!text.contains("name=\"model\""));
                    assert!(!text.contains("response_format"));
                    assert!(request.windows(4).any(|x| x == b"RIFF"));
                }
                SttBackend::VercelGateway => {
                    assert!(text.contains("ai-model-id: fish-audio/transcribe-1\r\n"));
                    assert!(text.contains("ai-gateway-protocol-version: 0.0.1\r\n"));
                    assert!(text.contains("ai-transcription-model-specification-version: 4\r\n"));
                    let body = text.split_once("\r\n\r\n").unwrap().1;
                    let body: serde_json::Value = serde_json::from_str(body).unwrap();
                    assert_eq!(body["mediaType"], "audio/wav");
                    assert_eq!(
                        base64::engine::general_purpose::STANDARD
                            .decode(body["audio"].as_str().unwrap())
                            .unwrap(),
                        wav(&[1; 160])
                    );
                }
            }
        }
    }
    #[test]
    fn backend_migration_validation_and_fish_cleanup() {
        let old: ListenConfig = serde_json::from_str(r#"{"model":"whisper-1"}"#).unwrap();
        assert_eq!(old.backend, SttBackend::OpenAiCompatible);
        let mut config = ListenConfig::default();
        config.preset(SttBackend::FishAsr);
        config.model = "typo".into();
        assert!(
            transcribe(&config, &wav(&[1; 160]), None, &|| false)
                .unwrap_err()
                .contains("Fish ASR model")
        );
        config.model = "transcribe-1-pro".into();
        config.language = "en-US".into();
        assert!(
            transcribe(&config, &wav(&[1; 160]), None, &|| false)
                .unwrap_err()
                .contains("language")
        );
        assert_eq!(
            clean_fish_transcript("<|speaker:0|> hello [laughter] 世界"),
            "hello 世界"
        );
    }
    #[test]
    fn provider_error_body_is_not_exposed() {
        use std::{io::Write, net::TcpListener};
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "http://{}/v1/audio/transcriptions",
            server.local_addr().unwrap()
        );
        let task = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut buffer = [0; 4096];
            let mut request = Vec::new();
            loop {
                let n = stream.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|x| x == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&request[..end]);
                    let length = header
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 13\r\nConnection: close\r\n\r\nsecret-detail").unwrap();
        });
        let c = ListenConfig {
            endpoint,
            ..Default::default()
        };
        assert_eq!(
            transcribe(&c, &wav(&[1; 160]), Some("secret"), &|| false).unwrap_err(),
            "Transcription provider returned HTTP 401"
        );
        task.join().unwrap();
    }
}
