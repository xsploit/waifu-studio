//! Explicit microphone recording with OpenAI-compatible, Fish and Vercel transcription.
//! VAD uses local energy or optional Silero ONNX inference. Audio is
//! downmixed and band-limited to 16 kHz with a streaming polyphase FIR resampler.
pub(crate) mod resample;
use base64::Engine;
use cpal::{
    Sample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};
const RATE: usize = 16_000;
const MAX_SAMPLES: usize = RATE * 30;

/// One loaded model per microphone state. A changed configuration evicts it;
/// a failed replacement never falls back to a model from the old configuration.
struct VadCache<T> {
    entry: Option<(String, String, T)>,
}
impl<T> Default for VadCache<T> {
    fn default() -> Self {
        Self { entry: None }
    }
}
impl<T> VadCache<T> {
    fn get_or_load(
        &mut self,
        model: &str,
        runtime: &str,
        load: impl FnOnce(&str, &str) -> Result<T, String>,
    ) -> Result<&mut T, String> {
        if !self
            .entry
            .as_ref()
            .is_some_and(|(m, r, _)| m == model && r == runtime)
        {
            self.entry = None;
            self.entry = Some((model.into(), runtime.into(), load(model, runtime)?));
        }
        Ok(&mut self.entry.as_mut().expect("model loaded").2)
    }
}

// Keep-alive survives conversational pauses; auth remains request-scoped.
fn transcription_client() -> Result<&'static reqwest::blocking::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(120))
                .connect_timeout(Duration::from_secs(10))
                .pool_idle_timeout(Duration::from_secs(90))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "Cannot create transcription client".into())
        })
        .as_ref()
        .map_err(Clone::clone)
}
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
    /// Saves from before backends existed stored an OpenAI-compatible endpoint, so a
    /// missing field keeps meaning that; brand-new configs use the gateway preset.
    #[serde(default)]
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
    pub stop_on_silence: bool,
    pub trim_silence: bool,
    pub auto_send: bool,
}
impl Default for ListenConfig {
    fn default() -> Self {
        let mut config = Self {
            backend: SttBackend::default(),
            language: String::new(),
            silero_enabled: true,
            silero_model: String::new(),
            silero_runtime: String::new(),
            silero_threshold: 0.5,
            enabled: true,
            endpoint: String::new(),
            model: String::new(),
            api_key_env: String::new(),
            vad_threshold: 0.015,
            trailing_silence_ms: 3000,
            stop_on_silence: true,
            trim_silence: false,
            auto_send: true,
        };
        // Same gateway key as chat; OpenAI models are reached through it, not directly.
        config.preset(SttBackend::VercelGateway);
        config
    }
}
impl ListenConfig {
    /// Changes routing only; microphone and VAD preferences are preserved.
    pub fn preset(&mut self, backend: SttBackend) {
        self.backend = backend;
        let (endpoint, model, key) = match backend {
            SttBackend::OpenAiCompatible => (
                "http://127.0.0.1:8000/v1/audio/transcriptions",
                "whisper-1",
                "",
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
/// One completed manual recording. Automatic send intent is captured at Start,
/// never inferred from the transcript or enabled retroactively.
#[derive(Clone, Debug)]
pub struct Transcript {
    pub text: String,
    pub character: Option<uuid::Uuid>,
    pub auto_send: bool,
}
/// Receiver is mutex-wrapped so this state can be a Bevy resource.
#[derive(bevy::prelude::Resource)]
pub struct ListeningState {
    vad_cache: Arc<Mutex<VadCache<crate::silero::SileroVad>>>,
    pub config: ListenConfig,
    /// Session-only credential; never part of serialized config.
    pub session_key: String,
    origin: Option<(u64, Option<uuid::Uuid>, bool)>,
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
            vad_cache: Arc::new(Mutex::new(VadCache::default())),
            config: ListenConfig::default(),
            session_key: String::new(),
            origin: None,
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
        self.start_for(None, false);
    }
    /// Host supplies the selected character and whether its draft is empty.
    /// Existing context-free callers remain review-only.
    pub fn start_for(&mut self, character: Option<uuid::Uuid>, composer_empty: bool) {
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
        let vad_cache = self.vad_cache.clone();
        self.busy.store(true, Ordering::SeqCst);
        self.stop.store(false, Ordering::SeqCst);
        let id = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.origin = Some((
            id,
            character,
            self.config.auto_send && composer_empty && character.is_some(),
        ));
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
            let result = capture(
                &config,
                &stop,
                &recording,
                &cancelled,
                &vad_status,
                &vad_cache,
            )
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
    /// A service switch must not forward the previous provider's session key.
    pub fn change_service(&mut self, configure: impl FnOnce(&mut ListenConfig)) {
        self.cancel();
        self.session_key.clear();
        configure(&mut self.config);
    }
    /// In-flight HTTP finishes within its timeout; its result is discarded.
    pub fn cancel(&mut self) {
        self.origin = None;
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
        self.status = "Cancelled; worker may still be finishing".into();
    }
    pub fn poll(&mut self) -> Option<String> {
        self.poll_transcript().map(|result| result.text)
    }
    pub fn poll_transcript(&mut self) -> Option<Transcript> {
        let rx = self.rx.lock().ok()?;
        while let Ok((id, result)) = rx.try_recv() {
            if id != self.generation.load(Ordering::SeqCst) {
                continue;
            }
            match result {
                Ok(text) => {
                    self.status = "Transcript added to composer; review before sending".into();
                    let (_, character, auto_send) = self
                        .origin
                        .take()
                        .filter(|(generation, _, _)| *generation == id)
                        .unwrap_or((id, None, false));
                    return Some(Transcript {
                        text,
                        character,
                        auto_send,
                    });
                }
                Err(e) => {
                    self.origin = None;
                    self.status = e;
                }
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
    resampler: resample::Resampler,
    channel_sum: f32,
    channel_fill: usize,
    channels: usize,
    threshold: f32,
    silence_limit: usize,
    stop_on_silence: bool,
    silence: usize,
    speech: bool,
    onset: usize,
    speech_start: Option<usize>,
    speech_end: Option<usize>,
    done: bool,
    neural: bool,
    energy: f64,
    window: usize,
}
impl Recording {
    fn new(rate: u32, channels: usize, c: &ListenConfig) -> Self {
        Self {
            samples: Vec::with_capacity(MAX_SAMPLES),
            resampler: resample::Resampler::new(rate),
            channel_sum: 0.0,
            channel_fill: 0,
            channels,
            threshold: if c.vad_threshold.is_finite() {
                c.vad_threshold.clamp(0.0001, 1.0)
            } else {
                0.015
            },
            silence_limit: RATE * c.trailing_silence_ms.clamp(200, 5000) as usize / 1000,
            stop_on_silence: c.stop_on_silence,
            silence: 0,
            speech: false,
            onset: 0,
            speech_start: None,
            speech_end: None,
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
        for sample in data {
            if self.done {
                break;
            }
            self.channel_sum += f32::from_sample(*sample);
            self.channel_fill += 1;
            if self.channel_fill != self.channels {
                continue;
            }
            let value = self.channel_sum / self.channels as f32;
            self.channel_sum = 0.0;
            self.channel_fill = 0;
            let value = if value.is_finite() {
                value.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let (output, count) = self.resampler.push(value);
            for &value in &output[..count] {
                if self.done {
                    break;
                }
                self.samples.push((value * i16::MAX as f32) as i16);
                self.energy += (value as f64).powi(2);
                self.window += 1;
                if self.window == 160 {
                    if !self.neural {
                        let rms = (self.energy / 160.).sqrt() as f32;
                        self.activity(rms >= self.threshold, rms >= self.threshold * 0.7, 160, 480);
                    }
                    self.energy = 0.;
                    self.window = 0;
                }
                self.done = self.samples.len() >= MAX_SAMPLES
                    || (self.stop_on_silence && self.speech && self.silence >= self.silence_limit);
            }
        }
    }
    /// Require sustained onset before arming automatic endpointing. Once armed,
    /// a lower hold threshold avoids chattering on quiet syllables.
    fn activity(&mut self, onset: bool, hold: bool, frames: usize, minimum: usize) {
        self.activity_at(onset, hold, frames, minimum, self.samples.len());
    }
    /// `end` is the analyzed frame's end, not the callback's current buffer
    /// length: neural inference may lag behind audio capture.
    fn activity_at(&mut self, onset: bool, hold: bool, frames: usize, minimum: usize, end: usize) {
        if !self.speech {
            self.onset = if onset {
                self.onset.saturating_add(frames)
            } else {
                0
            };
            if self.onset >= minimum {
                self.speech = true;
                self.silence = 0;
                self.speech_start = Some(end.saturating_sub(self.onset));
                self.speech_end = Some(end);
            }
        } else if hold {
            self.silence = 0;
            self.speech_end = Some(end);
        } else {
            self.silence = self.silence.saturating_add(frames);
        }
    }
    /// Trim only the outside of the confirmed speech span. Keep contextual
    /// audio around its edges; uncertain/missing bounds preserve all samples.
    fn take_samples(&mut self, trim: bool, analyzed_end: usize) -> Vec<i16> {
        if trim
            && self.samples.len().saturating_sub(analyzed_end) < 512
            && let (Some(start), Some(end)) = (self.speech_start, self.speech_end)
            && start < end
            && end <= self.samples.len()
        {
            let padding = RATE / 5;
            let start = start.saturating_sub(padding);
            let end = end.saturating_add(padding).min(self.samples.len());
            self.samples.truncate(end);
            self.samples.drain(..start);
        }
        std::mem::take(&mut self.samples)
    }
}
fn neural_thresholds(value: f32) -> (f32, f32) {
    let onset = if value.is_finite() {
        value.clamp(0.01, 1.0)
    } else {
        0.5
    };
    // Preserve the full UI range; the hold level must stay below even a very
    // low explicit onset threshold, and silence (probability zero) never holds.
    (onset, (onset - 0.15).max(onset * 0.5))
}
fn capture(
    c: &ListenConfig,
    stop: &AtomicBool,
    recording: &AtomicBool,
    cancelled: &impl Fn() -> bool,
    vad_status: &Mutex<String>,
    vad_cache: &Mutex<VadCache<crate::silero::SileroVad>>,
) -> Result<Vec<i16>, String> {
    let mut cache = vad_cache
        .lock()
        .map_err(|_| "Silero session cache failed")?;
    let mut silero = if c.silero_enabled {
        match cache.get_or_load(
            &c.silero_model,
            &c.silero_runtime,
            crate::silero::SileroVad::load,
        ) {
            Ok(model) => {
                model.reset();
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
    if config.channels == 0 || !(4000..=384_000).contains(&config.sample_rate) {
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
    let mut silero_failed = false;
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
                    let (onset, hold) = neural_thresholds(c.silero_threshold);
                    d.activity_at(p >= onset, p >= hold, 512, 1024, processed);
                    if d.stop_on_silence && d.speech && d.silence >= d.silence_limit {
                        d.done = true;
                    }
                }
                Err(error) => {
                    d.neural = false;
                    d.silence = 0;
                    d.onset = 0;
                    silero = None;
                    silero_failed = true;
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
    if silero_failed {
        cache.entry = None;
    }
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
    let analyzed_end = if data.neural {
        processed
    } else {
        data.samples.len()
    };
    // Inference failure or a backlog at manual Stop leaves the clip intact;
    // unexamined audio may contain a final word.
    Ok(data.take_samples(c.trim_silence && !silero_failed, analyzed_end))
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
/// Transcribe a bounded recording through the same adapter used by the microphone.
/// This does not capture audio, perform endpointing, or send the resulting text to chat.
pub fn transcribe_wav(
    config: &ListenConfig,
    audio: &[u8],
    session: Option<&str>,
    cancelled: &impl Fn() -> bool,
) -> Result<String, String> {
    transcribe(config, audio, session, cancelled)
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
    let client = transcription_client()?;
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
    #[test]
    fn changing_transcription_service_clears_session_key_and_custom_stays_local() {
        let mut state = ListeningState::default();
        state.session_key = "synthetic-provider-key".into();
        state.config.auto_send = false;
        state.change_service(|config| config.preset(SttBackend::OpenAiCompatible));
        assert!(state.session_key.is_empty());
        assert!(!state.config.auto_send);
        assert_eq!(
            state.config.endpoint,
            "http://127.0.0.1:8000/v1/audio/transcriptions"
        );
        assert!(state.config.api_key_env.is_empty());
        let old: ListenConfig = serde_json::from_str(
            r#"{"endpoint":"https://custom.example/asr","api_key_env":"EXISTING_KEY"}"#,
        )
        .unwrap();
        assert_eq!(old.endpoint, "https://custom.example/asr");
        assert_eq!(old.api_key_env, "EXISTING_KEY");
    }
    #[test]
    fn automatic_send_intent_is_scoped_consumed_once_and_cancelled() {
        let mut state = ListeningState::default();
        let character = uuid::Uuid::new_v4();
        state.generation.store(7, Ordering::SeqCst);
        state.origin = Some((7, Some(character), true));
        state.tx.send((6, Ok("stale".into()))).unwrap();
        state.tx.send((7, Ok("new transcript".into()))).unwrap();
        let result = state.poll_transcript().unwrap();
        assert_eq!(result.character, Some(character));
        assert_eq!(result.text, "new transcript");
        assert!(result.auto_send);
        assert!(state.origin.is_none());
        state.tx.send((7, Ok("no armed intent".into()))).unwrap();
        assert!(!state.poll_transcript().unwrap().auto_send);
        state.origin = Some((7, Some(character), true));
        state.cancel();
        assert!(state.origin.is_none());
        state.tx.send((7, Ok("cancelled".into()))).unwrap();
        assert!(state.poll_transcript().is_none());
        let old: ListenConfig = serde_json::from_str("{}").unwrap();
        assert!(old.auto_send, "on by default");
    }

    #[test]
    fn vad_session_reuse_is_scoped_to_configuration_and_failed_replacements_evict() {
        use std::cell::Cell;
        let loads = Cell::new(0);
        let mut cache = super::VadCache::default();
        let load = |_: &str, _: &str| {
            loads.set(loads.get() + 1);
            Ok::<_, String>(loads.get())
        };
        assert_eq!(*cache.get_or_load("model-a", "runtime-a", load).unwrap(), 1);
        assert_eq!(*cache.get_or_load("model-a", "runtime-a", load).unwrap(), 1);
        assert_eq!(loads.get(), 1, "the session survives successive recordings");
        assert_eq!(*cache.get_or_load("model-b", "runtime-a", load).unwrap(), 2);
        assert_eq!(*cache.get_or_load("model-b", "runtime-b", load).unwrap(), 3);
        assert!(
            cache
                .get_or_load("broken", "runtime-b", |_, _| Err("bad model".into()))
                .is_err()
        );
        assert!(
            cache.entry.is_none(),
            "never reuse the previous model after a failed replacement"
        );
        assert_eq!(*cache.get_or_load("model-b", "runtime-b", load).unwrap(), 4);
        let mut independent = super::VadCache::default();
        assert_eq!(
            *independent
                .get_or_load("model-b", "runtime-b", load)
                .unwrap(),
            5
        );
    }

    use super::*;

    #[test]
    fn patient_endpointing_preserves_pauses_and_manual_mode_only_hits_the_hard_cap() {
        let config = ListenConfig::default();
        assert_eq!(config.trailing_silence_ms, 3000);
        assert!(config.stop_on_silence);
        let mut automatic = Recording::new(16_000, 1, &config);
        automatic.feed(&vec![0.2_f32; 8_000]);
        automatic.feed(&vec![0_f32; 32_000]);
        assert!(
            !automatic.done,
            "a two-second thinking pause must not end the turn"
        );
        automatic.feed(&vec![0.2_f32; 8_000]);
        automatic.feed(&vec![0_f32; 64_000]);
        assert!(automatic.done);
        let mut manual = Recording::new(
            16_000,
            1,
            &ListenConfig {
                stop_on_silence: false,
                ..config
            },
        );
        manual.feed(&vec![0.2_f32; 8_000]);
        manual.feed(&vec![0_f32; 96_000]);
        assert!(!manual.done, "manual recording must survive a long pause");
        manual.feed(&vec![0_f32; MAX_SAMPLES]);
        assert!(manual.done);
        assert_eq!(manual.samples.len(), MAX_SAMPLES);
        let old: ListenConfig = serde_json::from_str(r#"{"trailing_silence_ms":800}"#).unwrap();
        assert_eq!(old.trailing_silence_ms, 800);
        assert!(
            old.stop_on_silence,
            "old saved silence preferences retain their meaning"
        );
    }

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
        // Include FIR settling plus the 10 ms energy-window quantization.
        r.feed(&vec![0f32; 24000]);
        assert!(
            r.speech && r.done,
            "speech={} done={} samples={} onset={} silence={}",
            r.speech,
            r.done,
            r.samples.len(),
            r.onset,
            r.silence
        );
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
    fn resampled_tone(rate: u32, frequency: f32) -> Vec<i16> {
        let mut recording = Recording::new(rate, 1, &ListenConfig::default());
        recording.neural = true;
        let signal: Vec<f32> = (0..rate / 4)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * frequency * i as f32 / rate as f32).sin())
            .collect();
        recording.feed(&signal);
        assert_eq!(recording.samples.len(), RATE / 4);
        recording.samples
    }
    fn rms(samples: &[i16]) -> f64 {
        (samples
            .iter()
            .map(|&v| (v as f64 / i16::MAX as f64).powi(2))
            .sum::<f64>()
            / samples.len() as f64)
            .sqrt()
    }
    #[test]
    fn resampler_retains_speech_band_and_rejects_out_of_band_aliasing() {
        for rate in [44_100, 48_000, 96_000, 192_000, 384_000] {
            for frequency in [1000.0, 6000.0] {
                let pass = resampled_tone(rate, frequency);
                assert!(
                    (rms(&pass[200..]) - 0.35355).abs() < 0.005,
                    "passband {frequency} Hz at {rate}"
                );
            }
            for frequency in [8500.0, 12_000.0] {
                let reject = resampled_tone(rate, frequency);
                assert!(
                    rms(&reject[200..]) < 0.002,
                    "alias rejection {frequency} Hz at {rate}: {}",
                    rms(&reject[200..])
                );
            }
        }
    }
    #[test]
    fn resampler_preserves_stereo_frames_and_filter_history_across_arbitrary_callbacks() {
        let source: Vec<f32> = (0..8820)
            .flat_map(|i| {
                let sample = 0.3 * (i as f32 * 0.17).sin();
                [sample, sample * 0.5]
            })
            .collect();
        let mut whole = Recording::new(44_100, 2, &ListenConfig::default());
        let mut split = Recording::new(44_100, 2, &ListenConfig::default());
        whole.neural = true;
        split.neural = true;
        whole.feed(&source);
        for chunk in source.chunks(137) {
            split.feed(chunk);
        }
        assert_eq!(whole.samples.len(), 3200);
        assert_eq!(split.samples, whole.samples);
    }
    #[test]
    fn resampler_upsamples_without_images_and_native_rate_is_exact() {
        let samples = resampled_tone(8000, 1000.0);
        assert!((rms(&samples[300..]) - 0.35355).abs() < 0.005);
        let samples = &samples[400..];
        let amplitude = |frequency: f64| {
            let (mut real, mut imag) = (0.0, 0.0);
            for (i, &sample) in samples.iter().enumerate() {
                let phase = 2.0 * std::f64::consts::PI * frequency * i as f64 / RATE as f64;
                real += sample as f64 * phase.cos();
                imag += sample as f64 * phase.sin();
            }
            (real.hypot(imag) * 2.0 / samples.len() as f64) / i16::MAX as f64
        };
        assert!(amplitude(1000.0) > 0.49);
        assert!(amplitude(7000.0) < 0.002);
        let mut recording = Recording::new(16_000, 1, &ListenConfig::default());
        recording.neural = true;
        recording.feed(&[0.25_f32, -0.5, 1.0, f32::NAN]);
        assert_eq!(recording.samples, [8191, -16383, 32767, 0]);
    }
    #[test]
    fn endpointing_ignores_short_clicks_and_holds_quiet_syllables() {
        let config = ListenConfig {
            trailing_silence_ms: 200,
            vad_threshold: 0.02,
            ..Default::default()
        };
        let mut recording = Recording::new(16_000, 1, &config);
        recording.feed(&[0.8_f32; 160]);
        recording.feed(&[0.0_f32; 4000]);
        assert!(
            !recording.speech && !recording.done,
            "one click must not arm endpointing"
        );
        recording.feed(&[0.1_f32; 480]);
        assert!(recording.speech);
        recording.feed(&[0.016_f32; 4000]);
        assert!(
            !recording.done && recording.silence == 0,
            "hold threshold retains quiet speech"
        );
        recording.feed(&[0.0_f32; 3200]);
        assert!(
            recording.done,
            "configured trailing silence still ends recording"
        );
    }
    #[test]
    fn neural_endpointing_requires_sustained_onset_and_respects_hold_threshold() {
        let mut recording = Recording::new(16_000, 1, &ListenConfig::default());
        recording.activity(true, true, 512, 1024);
        recording.activity(false, false, 512, 1024);
        assert!(!recording.speech);
        recording.activity(true, true, 512, 1024);
        recording.activity(true, true, 512, 1024);
        assert!(recording.speech);
        recording.activity(false, true, 512, 1024);
        assert_eq!(recording.silence, 0);
        recording.activity(false, false, 512, 1024);
        assert_eq!(recording.silence, 512);
    }
    #[test]
    fn neural_hysteresis_preserves_low_threshold_settings_and_never_holds_silence() {
        for value in [0.01, 0.05, 0.5, 0.99, 1.0] {
            let (onset, hold) = neural_thresholds(value);
            assert_eq!(onset, value);
            assert!(hold > 0.0 && hold < onset);
        }
        assert_eq!(neural_thresholds(f32::NAN).0, 0.5);
    }
    #[test]
    fn optional_trim_keeps_context_internal_pauses_and_original_default() {
        let config: ListenConfig = serde_json::from_str("{}").unwrap();
        assert!(!config.trim_silence);
        let original: Vec<i16> = (0..RATE * 3).map(|i| (i % 1000) as i16).collect();
        let make = || {
            let mut recording = Recording::new(RATE as u32, 1, &config);
            recording.samples = original.clone();
            for end in [8160, 8320, 8480] {
                recording.activity_at(true, true, 160, 480, end);
            }
            recording.activity_at(false, true, 160, 480, 20000);
            recording.activity_at(false, false, 160, 480, 21000);
            recording.activity_at(false, true, 160, 480, 25000);
            recording
        };
        assert_eq!(make().take_samples(false, original.len()), original);
        assert_eq!(
            make().take_samples(true, original.len()),
            original[4800..28200]
        );
        // A capture buffer ahead of neural inference must never lose its tail.
        assert_eq!(make().take_samples(true, 25000), original);
    }

    #[test]
    fn trim_uses_analyzed_frame_positions_and_preserves_unknown_bounds() {
        let config = ListenConfig::default();
        let original = vec![42; RATE];
        let mut recording = Recording::new(RATE as u32, 1, &config);
        recording.samples = original.clone();
        assert_eq!(recording.take_samples(true, RATE), original);
        recording.samples = original.clone();
        // Neural inference analyzes an early frame while capture is far ahead.
        recording.activity_at(true, true, 512, 1024, 512);
        recording.activity_at(true, true, 512, 1024, 1024);
        assert_eq!(recording.speech_start, Some(0));
        assert_eq!(recording.speech_end, Some(1024));
        assert_eq!(recording.take_samples(true, RATE), original[..4224]);
        recording.samples = original.clone();
        recording.speech_start = Some(RATE - 100);
        recording.speech_end = Some(RATE);
        assert_eq!(recording.take_samples(true, RATE), original[RATE - 3300..]);
        recording.samples = original.clone();
        recording.speech_end = Some(RATE + 1);
        assert_eq!(recording.take_samples(true, RATE), original);
    }
}
