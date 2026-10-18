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
        Arc, Mutex,
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
    listen::{ListenConfig, SttBackend},
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
// Timings and whitelisted diagnostics only; never retain content or reasoning.
fn observe_event(event: &Value, start: Instant, observation: &Mutex<Value>) {
    let mut output = observation.lock().unwrap();
    if output["first_event_ms"].is_null() {
        output["first_event_ms"] = json!(ms(start));
    }
    for pointer in [
        "/choices/0/delta/reasoning",
        "/choices/0/delta/reasoning_content",
    ] {
        if event
            .pointer(pointer)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
            && output["first_reasoning_ms"].is_null()
        {
            output["first_reasoning_ms"] = json!(ms(start));
        }
    }
    for (key, pointers) in [
        ("response_id", &["/id"][..]),
        (
            "generation_id",
            &[
                "/providerMetadata/gateway/generationId",
                "/provider_metadata/gateway/generationId",
            ][..],
        ),
        (
            "served_provider",
            &[
                "/providerMetadata/gateway/routing/finalProvider",
                "/providerMetadata/gateway/finalProvider",
                "/provider_metadata/gateway/routing/finalProvider",
                "/provider",
            ][..],
        ),
    ] {
        for pointer in pointers {
            if let Some(value) = event.pointer(pointer).and_then(Value::as_str)
                && value.len() <= 128
                && valid_slug(value)
            {
                output[key] = json!(value);
            }
        }
    }
    for (key, pointer) in [
        ("input_tokens", "/usage/prompt_tokens"),
        ("output_tokens", "/usage/completion_tokens"),
        (
            "reasoning_tokens",
            "/usage/completion_tokens_details/reasoning_tokens",
        ),
    ] {
        if let Some(value) = event.pointer(pointer).and_then(Value::as_u64) {
            output[key] = json!(value);
        }
    }
}
#[derive(Clone)]
struct RouteCase {
    name: String,
    routing: provider::ProviderRouting,
}
fn route_sort(value: &str) -> Result<RoutingSort, String> {
    match value {
        "default" => Ok(RoutingSort::Default),
        "latency" | "ttft" => Ok(RoutingSort::Latency),
        "throughput" | "tps" => Ok(RoutingSort::Throughput),
        "cost" => Ok(RoutingSort::Cost),
        _ => Err("Unknown routing sort".into()),
    }
}
fn provider_slugs(name: &str) -> Result<Vec<String>, String> {
    let slugs = list(name, "");
    if slugs.len() > 16 || slugs.iter().any(|s| !valid_slug(s)) {
        return Err("Invalid provider slug list".into());
    }
    Ok(slugs)
}
fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 128
        && slug
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_/.:".contains(&b))
}
fn route_cases() -> Result<Vec<RouteCase>, String> {
    let base = provider::ProviderRouting {
        sort: route_sort(&arg("--routing", "default"))?,
        only: provider_slugs("--provider-only")?,
        order: provider_slugs("--provider-order")?,
        ..Default::default()
    };
    let matrix = list("--routing-matrix", "");
    if matrix.is_empty() {
        return Ok(vec![RouteCase {
            name: arg("--routing", "default"),
            routing: base,
        }]);
    }
    if matrix.len() > 16 || !base.only.is_empty() || !base.order.is_empty() {
        return Err("Routing matrix must have at most 16 cases and no global provider pins".into());
    }
    matrix
        .into_iter()
        .map(|name| {
            let mut routing = provider::ProviderRouting::default();
            if let Some(slug) = name.strip_prefix("pin:") {
                if !valid_slug(slug) {
                    return Err("Invalid pinned provider slug".into());
                }
                routing.only = vec![slug.into()];
            } else {
                routing.sort = route_sort(&name)?;
            }
            Ok(RouteCase { name, routing })
        })
        .collect()
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
    connection_ready: Option<u64>,
    first_text_written: Option<u64>,
    first_flush_written: Option<u64>,
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
            connection_ready: None,
            first_text_written: None,
            first_flush_written: None,
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
                    self.connection_ready = stats.connection_ready_ms;
                    self.first_text_written = stats.first_text_written_ms;
                    self.first_flush_written = stats.first_flush_written_ms;
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
            "ws_connection_ready_ms":self.connection_ready,"ws_first_text_written_ms":self.first_text_written,"ws_first_flush_written_ms":self.first_flush_written,
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
    let routes = route_cases()?;
    let audio_path = arg("--input-audio", "");
    let input_audio = if audio_path.is_empty() {
        None
    } else {
        if phase != "pipeline" && phase != "llm" {
            return Err("Prerecorded ASR input requires llm or pipeline phase".into());
        }
        let metadata = std::fs::metadata(&audio_path).map_err(|_| "Cannot read ASR test clip")?;
        if !metadata.is_file() || !(45..=960044).contains(&metadata.len()) {
            return Err(
                "ASR test clip must be a WAV recording no larger than 30 s at 16 kHz mono PCM16"
                    .into(),
            );
        }
        let audio = std::fs::read(&audio_path).map_err(|_| "Cannot read ASR test clip")?;
        if !audio.starts_with(b"RIFF") || audio.get(8..12) != Some(b"WAVE") {
            return Err("ASR test clip must be WAV".into());
        }
        Some(audio)
    };
    let mut asr_config = ListenConfig::default();
    asr_config.preset(match arg("--asr-backend", "fish").as_str() {
        "fish" => SttBackend::FishAsr,
        "vercel" => SttBackend::VercelGateway,
        _ => return Err("ASR backend must be fish or vercel".into()),
    });
    asr_config.model = arg("--asr-model", &asr_config.model);
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
    if llm_models.len() * speech_models.len() * modes.len() * routes.len() * repeat > 150 {
        return Err("Benchmark matrix too large".into());
    }
    // Interleave settings between repeats rather than warming one candidate first.
    for iteration in 0..repeat {
        for llm_model in &llm_models {
            for speech_model in &speech_models {
                for delivery in &modes {
                    for route in &routes {
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
                        let observation = Arc::new(Mutex::new(json!({})));
                        let mut asr_done = None;
                        let mut transcript_chars = None;
                        let user_text = if let Some(audio) = &input_audio {
                            match waifu_studio::listen::transcribe_wav(
                                &asr_config,
                                audio,
                                None,
                                &|| false,
                            ) {
                                Ok(transcript) => {
                                    asr_done = Some(ms(start));
                                    transcript_chars = Some(transcript.chars().count());
                                    transcript
                                }
                                Err(error) => {
                                    speech.error = Some(error);
                                    String::new()
                                }
                            }
                        } else {
                            "Say hello to chat and say you are excited to play a game together."
                                .into()
                        };
                        if phase != "speech" && speech.error.is_none() {
                            let mut config = ProviderConfig::pinned(GatewayRoute::VercelGateway);
                            config.model.clone_from(llm_model);
                            config.max_tokens = Some(
                                arg("--max-tokens", "1024")
                                    .parse::<u32>()
                                    .map_err(|_| "Invalid max tokens")?
                                    .clamp(256, 4096),
                            );
                            config.reasoning_effort =
                                Some(match arg("--reasoning", "low").as_str() {
                                    "none" => ReasoningEffort::None,
                                    "low" => ReasoningEffort::Low,
                                    "minimal" => ReasoningEffort::Minimal,
                                    _ => {
                                        return Err(
                                            "Benchmark reasoning must be none, minimal or low"
                                                .into(),
                                        );
                                    }
                                });
                            config.routing = route.routing.clone();
                            let stop = cancel.clone();
                            let observed = observation.clone();
                            llm_worker = Some(std::thread::spawn(move || {
                                let turns = vec![Turn { role:"system".into(), content:"You are a friendly VTuber. Answer in two brief spoken sentences, 20 to 30 words total. No formatting, stage directions, or metadata.".into() },
                        Turn { role:"user".into(), content:user_text }];
                                let result = provider::stream_chat_observed(
                                    &config,
                                    &turns,
                                    &stop,
                                    |delta| {
                                        let _ = tx.send(Ok(Some(delta)));
                                    },
                                    |event| observe_event(event, start, &observed),
                                );
                                let _ = tx.send(result.map(|_| None));
                            }));
                            if phase == "pipeline" {
                                speech.open()?;
                            }
                        } else if phase == "speech" {
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
                        row["routing_case"] = route.name.clone().into();
                        row["provider_routing"] = json!(route.routing);
                        row["llm_observation"] = observation.lock().unwrap().clone();
                        row["run"] = (iteration + 1).into();
                        row["first_token_ms"] = json!(first_token);
                        row["llm_done_ms"] = json!(llm_end);
                        row["asr_done_ms"] = json!(asr_done);
                        row["transcript_chars"] = json!(transcript_chars);
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
                "prerecorded_asr":input_audio.is_some(),"asr_backend":arg("--asr-backend","fish"),"asr_model":asr_config.model,
                "routing":arg("--routing","default"),"routing_matrix":arg("--routing-matrix",""),"provider_only":arg("--provider-only",""),"provider_order":arg("--provider-order",""),"max_tokens":arg("--max-tokens","1024"),"reasoning":arg("--reasoning","low"),"prewarm_ms":arg("--prewarm-ms","0"),"rows":rows});
                        std::fs::write(&output, serde_json::to_vec_pretty(&result).unwrap())
                            .map_err(|_| "Could not save benchmark")?;
                    }
                }
            }
        }
    }
    Ok(())
}
