//! Explicit, bounded live benchmark of the production adapters. No speaker opens.
//! --phase llm|speech|pipeline --models ID,ID --speech-models ID,ID
//! --modes buffered,http,sse,ws,ws-timestamps --repeat 3 --run --output FILE.json
//! All requests retain the configured proxy; results contain no keys or chat history.
use bevy::audio::Decodable;
use rodio::Source;
use serde_json::{Value, json};
use std::{
    io::Cursor,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use waifu_studio::{
    credentials,
    data::{Library, Turn},
    fish_live::{FishLiveState, LiveEvent},
    fish_ws::FishWsState,
    provider::{self, GatewayRoute, ProviderConfig, ReasoningEffort, RoutingSort},
    voice::{VoiceAdapter, VoiceConfig, VoiceDelivery, VoiceEvent, VoiceWorker},
    voice_stream::StreamingVoice,
};

const TEXT: &str = "Hey chat, this is a quick voice test. Let's keep things moving!";

fn arg(name: &str, fallback: &str) -> String {
    let args: Vec<_> = std::env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .unwrap_or_else(|| fallback.into())
}
fn list(name: &str, fallback: &str) -> Vec<String> {
    arg(name, fallback)
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
fn mode(name: &str) -> Result<VoiceDelivery, String> {
    match name {
        "buffered" => Ok(VoiceDelivery::Buffered),
        "http" => Ok(VoiceDelivery::HttpWavStream),
        "sse" => Ok(VoiceDelivery::TimestampSse),
        "ws" => Ok(VoiceDelivery::LiveWebSocket),
        "ws-timestamps" => Ok(VoiceDelivery::TimestampWebSocket),
        _ => Err("Unknown speech mode".into()),
    }
}
struct Speech {
    config: VoiceConfig,
    ws: FishWsState,
    sse: FishLiveState,
    http: VoiceWorker,
    started: bool,
    decoder: Option<Box<dyn Iterator<Item = f32> + Send>>,
    streaming: Option<StreamingVoice>,
    first_ready: Option<f64>,
    first_nonzero: Option<f64>,
    network_end: Option<f64>,
    playback_start: Option<Instant>,
    samples: usize,
    nonzero: usize,
    channels: usize,
    rate: u32,
    done: bool,
    error: Option<String>,
    first_wire: Option<u64>,
}
impl Speech {
    fn new(config: VoiceConfig) -> Self {
        Self {
            config,
            ws: FishWsState::default(),
            sse: FishLiveState::default(),
            http: VoiceWorker::new(),
            started: false,
            decoder: None,
            streaming: None,
            first_ready: None,
            first_nonzero: None,
            network_end: None,
            playback_start: None,
            samples: 0,
            nonzero: 0,
            channels: 1,
            rate: 1,
            done: false,
            error: None,
            first_wire: None,
        }
    }
    fn open(&mut self) -> Result<(), String> {
        if self.config.delivery.is_websocket() && !self.started {
            self.ws.timestamps = self.config.delivery == VoiceDelivery::TimestampWebSocket;
            self.ws.open(1, &self.config, None)?;
            self.started = true;
        }
        Ok(())
    }
    fn text(&mut self, text: &str) -> Result<(), String> {
        self.open()?;
        if self.config.delivery.is_websocket() {
            self.ws.push_text(text)?;
        }
        Ok(())
    }
    fn finish_text(&mut self, text: &str) -> Result<(), String> {
        if self.config.delivery.is_websocket() {
            self.open()?;
            self.ws.finish()
        } else {
            self.started = true;
            if self.config.delivery == VoiceDelivery::TimestampSse {
                self.sse.speak(1, &self.config, text.into(), None)
            } else {
                self.http.start(1, self.config.clone(), text.into(), None)
            }
        }
    }
    fn ready(&mut self, audio: StreamingVoice, start: Instant) {
        let decoder = audio.decoder();
        self.rate = decoder.sample_rate().get();
        self.channels = usize::from(decoder.channels().get());
        self.decoder = Some(Box::new(decoder));
        self.streaming = Some(audio);
        self.first_ready = Some(ms(start));
        self.playback_start = Some(Instant::now());
    }
    fn poll(&mut self, start: Instant) {
        for event in self.ws.poll().into_iter().chain(self.sse.poll()) {
            match event {
                LiveEvent::Playing { audio, .. } => self.ready(audio, start),
                LiveEvent::Finished { stats, .. } => {
                    self.network_end = Some(ms(start));
                    self.first_wire = stats.first_audio_ms;
                }
                LiveEvent::Failed { message, .. } => self.error = Some(message),
                _ => {}
            }
        }
        while let Some(event) = self.http.try_recv() {
            match event {
                VoiceEvent::Streaming { audio, .. } => self.ready(audio, start),
                VoiceEvent::Ready { audio, .. } => {
                    self.network_end = Some(ms(start));
                    match rodio::Decoder::try_from(Cursor::new(audio.bytes)) {
                        Ok(decoder) => {
                            self.rate = decoder.sample_rate().get();
                            self.channels = usize::from(decoder.channels().get());
                            self.decoder = Some(Box::new(decoder));
                            self.first_ready = Some(ms(start));
                            self.playback_start = Some(Instant::now());
                        }
                        Err(_) => {
                            self.error =
                                Some("Buffered audio failed production codec decoding".into())
                        }
                    }
                }
                VoiceEvent::Failed { message, .. } => self.error = Some(message),
            }
        }
        if let Some(audio) = &self.streaming {
            if audio.is_failed() {
                self.error.get_or_insert("Streaming decoder failed".into());
            }
            if audio.is_finished() {
                self.network_end.get_or_insert_with(|| ms(start));
            }
        }
        // Drain only published PCM, with no artificial underflow zeroes. This
        // measures decode readiness/transfer, not speaker scheduling or playback quality.
        let available = self
            .streaming
            .as_ref()
            .map(|audio| audio.buffered_frames() * self.channels);
        if let Some(decoder) = self.decoder.as_mut() {
            for _ in 0..available.unwrap_or(4096).min(4096) {
                match decoder.next() {
                    Some(sample) => {
                        self.samples += 1;
                        if sample.abs() > 0.00001 {
                            self.nonzero += 1;
                            self.first_nonzero.get_or_insert_with(|| ms(start));
                        }
                    }
                    None => {
                        self.done = true;
                        break;
                    }
                }
            }
            if let Some(audio) = &self.streaming
                && audio.is_finished()
                && audio.buffered_frames() == 0
            {
                self.done = true;
            }
        }
    }
    fn cancel(&mut self) {
        self.ws.cancel();
        self.sse.cancel();
        self.http.cancel();
    }
    fn result(&self) -> Value {
        let duration = self.samples as f64 / (f64::from(self.rate) * self.channels as f64) * 1000.0;
        json!({"first_decoded_audio_ms":self.first_ready,"first_nonzero_audio_ms":self.first_nonzero,
            "network_done_ms":self.network_end,"audio_duration_ms":duration,
            "earliest_playback_end_ms":self.first_ready.map(|t| t+duration),
            "nonzero_samples":self.nonzero,"transport_first_wire_audio_ms":self.first_wire,
            "error":self.error})
    }
}

fn main() -> Result<(), String> {
    if !std::env::args().any(|arg| arg == "--run") {
        println!(
            "Explicit live benchmark: --run --phase llm|speech|pipeline --repeat 3 --models google/gemini-3.5-flash --speech-models s2.1-pro-free --modes ws,http,sse,buffered --output /absolute/results.json. No speaker or saved-setting changes."
        );
        return Ok(());
    }
    if !["https_proxy", "HTTPS_PROXY", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|key| std::env::var(key).is_ok_and(|value| !value.trim().is_empty()))
    {
        return Err("No proxy route configured; benchmark skipped".into());
    }
    let dirs = directories::ProjectDirs::from("", "", "waifu-studio").ok_or("No app directory")?;
    let directory = PathBuf::from(arg("--data-dir", &dirs.data_local_dir().to_string_lossy()));
    // SAFETY: startup occurs before any workers or HTTP clients are created.
    unsafe { credentials::load_startup(&directory.join("credentials.env")) }
        .map_err(|_| "Could not load private benchmark credentials")?;
    let bytes =
        std::fs::read(directory.join("library.json")).map_err(|_| "Could not read app settings")?;
    let library: Library = serde_json::from_slice(&bytes).map_err(|_| "Invalid app settings")?;
    let mut voice = library
        .selected_character()
        .and_then(|c| c.voice.clone())
        .unwrap_or(library.voice);
    voice.adapter = VoiceAdapter::FishHosted;
    voice.endpoint = "https://api.fish.audio/v1/tts".into();
    voice.api_key_env = "FISH_API_KEY".into();
    voice.enabled = true;
    voice.format = "wav".into();
    voice.direction.clear();
    if voice.reference_id.is_empty() {
        voice.reference_id = std::env::var("FISH_REFERENCE_ID").unwrap_or_default();
    }
    voice.latency = arg("--latency", "balanced");
    voice.chunk_length = arg("--chunk-length", "160")
        .parse()
        .map_err(|_| "Invalid chunk length")?;
    voice.min_chunk_length = arg("--min-chunk", "20")
        .parse()
        .map_err(|_| "Invalid minimum chunk")?;
    voice.condition_on_previous_chunks = arg("--condition", "true") == "true";
    let repeat = arg("--repeat", "3")
        .parse::<usize>()
        .map_err(|_| "Invalid repeat")?;
    if !(1..=5).contains(&repeat) {
        return Err("Repeat must be 1–5".into());
    }
    let phase = arg("--phase", "speech");
    if !["speech", "llm", "pipeline"].contains(&phase.as_str()) {
        return Err("Unknown phase".into());
    }
    let models = list("--models", "google/gemini-3.5-flash");
    let speech_models = list(
        "--speech-models",
        "s1,s2-pro,s2.1-pro,s2.1-pro-free,drama-3",
    );
    let modes = list("--modes", "buffered,http,sse,ws,ws-timestamps");
    let output = PathBuf::from(arg("--output", "pipeline-benchmark.json"));
    let mut rows = Vec::new();
    let llm_models = if phase == "speech" {
        vec![String::new()]
    } else {
        models
    };
    let speech_models = if phase == "llm" {
        vec![String::new()]
    } else {
        speech_models
    };
    let modes = if phase == "llm" {
        vec!["ws".into()]
    } else {
        modes
    };
    if llm_models.len() * speech_models.len() * modes.len() * repeat > 150 {
        return Err("Benchmark matrix too large".into());
    }
    // Interleave settings between repeats rather than warming one candidate first.
    for iteration in 0..repeat {
        for llm_model in &llm_models {
            for speech_model in &speech_models {
                for delivery in &modes {
                    let mut config = voice.clone();
                    config.model.clone_from(speech_model);
                    config.delivery = mode(delivery)?;
                    let mut speech = Speech::new(config);
                    let prewarm_ms = arg("--prewarm-ms", "0")
                        .parse::<u64>()
                        .map_err(|_| "Invalid prewarm duration")?
                        .min(3000);
                    // Explicit experimental setup before Send; report its cost
                    // separately rather than disguising it as a cold request.
                    if phase == "pipeline"
                        && speech.config.delivery.is_websocket()
                        && prewarm_ms > 0
                    {
                        let warming = Instant::now();
                        speech.open()?;
                        while warming.elapsed() < Duration::from_millis(prewarm_ms) {
                            speech.poll(warming);
                            if speech.error.is_some() {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(1));
                        }
                    }
                    let start = Instant::now();
                    let cancel = Arc::new(AtomicBool::new(false));
                    let mut first_token = None;
                    let mut llm_end = None;
                    let mut text = String::new();
                    let (tx, rx) = mpsc::channel();
                    let mut llm_worker = None;
                    if phase != "speech" {
                        let mut config = ProviderConfig::pinned(GatewayRoute::VercelGateway);
                        config.model.clone_from(llm_model);
                        config.max_tokens = Some(
                            arg("--max-tokens", "1024")
                                .parse::<u32>()
                                .map_err(|_| "Invalid max tokens")?
                                .clamp(256, 4096),
                        );
                        config.reasoning_effort = Some(match arg("--reasoning", "low").as_str() {
                            "none" => ReasoningEffort::None,
                            "low" => ReasoningEffort::Low,
                            "minimal" => ReasoningEffort::Minimal,
                            _ => {
                                return Err(
                                    "Benchmark reasoning must be none, minimal or low".into()
                                );
                            }
                        });
                        if arg("--routing", "default") == "latency" {
                            config.routing.sort = RoutingSort::Latency;
                        }
                        let stop = cancel.clone();
                        llm_worker = Some(std::thread::spawn(move || {
                            let turns = vec![Turn { role:"system".into(), content:"You are a friendly VTuber. Answer in two brief spoken sentences, 20 to 30 words total. No formatting, stage directions, or metadata.".into() },
                        Turn { role:"user".into(), content:"Say hello to chat and say you are excited to play a game together.".into() }];
                            let result = provider::stream_chat(&config, &turns, &stop, |delta| {
                                let _ = tx.send(Ok(Some(delta)));
                            });
                            let _ = tx.send(result.map(|_| None));
                        }));
                        if phase == "pipeline" {
                            speech.open()?;
                        }
                    } else {
                        text = TEXT.into();
                        speech.text(&text)?;
                        speech.finish_text(&text)?;
                    }
                    while start.elapsed() < Duration::from_secs(40) {
                        for event in rx.try_iter() {
                            match event {
                                Ok(Some(delta)) => {
                                    if !delta.is_empty() {
                                        first_token.get_or_insert_with(|| ms(start));
                                    }
                                    if phase == "pipeline"
                                        && let Err(error) = speech.text(&delta)
                                    {
                                        speech.error = Some(error);
                                    }
                                    text.push_str(&delta);
                                }
                                Ok(None) => {
                                    llm_end = Some(ms(start));
                                    if phase == "pipeline"
                                        && let Err(error) = speech.finish_text(&text)
                                    {
                                        speech.error = Some(error);
                                    }
                                }
                                Err(error) => speech.error = Some(error),
                            }
                        }
                        if phase != "llm" {
                            speech.poll(start);
                        }
                        if speech.error.is_some()
                            || (phase == "llm" && llm_end.is_some())
                            || speech.done
                        {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    if speech.error.is_none()
                        && ((phase == "llm" && llm_end.is_none())
                            || (phase != "llm" && !speech.done))
                    {
                        speech.error = Some("Benchmark exceeded 40 seconds; no retry".into());
                    }
                    cancel.store(true, Ordering::Relaxed);
                    let mut row = speech.result();
                    row["phase"] = phase.clone().into();
                    row["llm_model"] = llm_model.clone().into();
                    row["speech_model"] = speech_model.clone().into();
                    row["mode"] = delivery.clone().into();
                    row["run"] = (iteration + 1).into();
                    row["first_token_ms"] = json!(first_token);
                    row["llm_done_ms"] = json!(llm_end);
                    row["text_chars"] = text.chars().count().into();
                    row["decoded_done_ms"] = json!(ms(start));
                    // Only known local diagnostics/status errors; redact before saving.
                    if let Some(error) = &speech.error {
                        row["error"] = waifu_studio::mcp::redact(error, None).into();
                    }
                    println!("{}", serde_json::to_string(&row).unwrap());
                    speech.cancel();
                    if let Some(worker) = llm_worker {
                        worker
                            .join()
                            .map_err(|_| "Benchmark chat worker panicked")?;
                    }
                    rows.push(row);
                    let result = json!({"version":1,"method":"production adapters; configured proxy; sequential requests; no speakers; streamed PCM drained without artificial silence; first audio means decoder readiness, not audible speaker onset",
                "voice_tuning":{"latency":voice.latency,"chunk_length":voice.chunk_length,"min_chunk_length":voice.min_chunk_length,"condition_on_previous_chunks":voice.condition_on_previous_chunks},
                "routing":arg("--routing","default"),"max_tokens":arg("--max-tokens","1024"),"reasoning":arg("--reasoning","low"),"prewarm_ms":arg("--prewarm-ms","0"),"rows":rows});
                    std::fs::write(&output, serde_json::to_vec_pretty(&result).unwrap())
                        .map_err(|_| "Could not save benchmark")?;
                }
            }
        }
    }
    Ok(())
}
