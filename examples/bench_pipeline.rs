//! Explicit, bounded live benchmark of the production adapters. No speaker opens.
//! --phase llm|speech|pipeline --models ID,ID --speech-models ID,ID
//! --modes buffered,http,sse,ws,ws-timestamps --repeat 3 --run --output FILE.json
//! --app-path drives spawn_chat ProviderEvents polled every --frame-ms (default 16)
//! like the Bevy app; pipeline HTTP/buffered use its shared prefetch queue and
//! SSE uses its sequential sentence queue, with simulated real-time consumption. The default direct path reads per ms
//! and retains the older full-reply HTTP baseline. No render/audio device is modelled.
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
    provider::{
        self, ChatJob, GatewayRoute, ProviderConfig, ProviderEvent, ReasoningEffort, RoutingSort,
    },
    reply_speech::{HttpReplySpeech, SseReplySpeech},
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
enum Chat {
    Direct(
        mpsc::Receiver<Result<Option<String>, String>>,
        std::thread::JoinHandle<()>,
    ),
    App(mpsc::Receiver<ProviderEvent>, ChatJob),
}
impl Chat {
    fn events(&self) -> Vec<Result<Option<String>, String>> {
        match self {
            Chat::Direct(rx, _) => rx.try_iter().collect(),
            Chat::App(rx, job) => rx
                .try_iter()
                .filter_map(|event| match event {
                    ProviderEvent::Delta(id, delta) if id == job.id => Some(Ok(Some(delta))),
                    ProviderEvent::Finished(id) if id == job.id => Some(Ok(None)),
                    ProviderEvent::Failed(id, error) if id == job.id => Some(Err(error)),
                    _ => None,
                })
                .collect(),
        }
    }
    // Cancel, then wait for the worker. The app worker is detached, so it is closed
    // once it drops its sender; a canceled blocking read may outlive the bound.
    fn close(self, cancel: &AtomicBool) -> Result<bool, String> {
        cancel.store(true, Ordering::Relaxed);
        match self {
            Chat::Direct(_, worker) => worker
                .join()
                .map(|_| true)
                .map_err(|_| "Benchmark chat worker panicked".into()),
            Chat::App(rx, job) => {
                job.cancel();
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(left) {
                        Ok(_) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(true),
                        Err(mpsc::RecvTimeoutError::Timeout) => return Ok(false),
                    }
                }
            }
        }
    }
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
    reply: Option<HttpReplySpeech>,
    sse_reply: Option<SseReplySpeech>,
    last_drain: Option<Instant>,
    drain_fraction: f64,
    clip_network_done: bool,
    audio_duration_ms: f64,
    chunk_ready_ms: Vec<f64>,
    chunk_gaps_ms: Vec<f64>,
    last_clip_end: Option<Instant>,
    started: bool,
    decoder: Option<Box<dyn Iterator<Item = f32> + Send>>,
    streaming: Option<StreamingVoice>,
    first_ready: Option<f64>,
    first_nonzero: Option<f64>,
    network_end: Option<f64>,
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
    fn new(config: VoiceConfig, app_reply: bool) -> Self {
        Self {
            reply: (app_reply
                && matches!(
                    config.delivery,
                    VoiceDelivery::Buffered | VoiceDelivery::HttpWavStream
                ))
            .then(|| HttpReplySpeech::new(1, config.clone(), None)),
            sse_reply: (app_reply && config.delivery == VoiceDelivery::TimestampSse)
                .then(|| SseReplySpeech::new(1, config.clone(), None)),
            last_drain: None,
            drain_fraction: 0.0,
            clip_network_done: false,
            audio_duration_ms: 0.0,
            chunk_ready_ms: Vec::new(),
            chunk_gaps_ms: Vec::new(),
            last_clip_end: None,
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
        if let Some(reply) = &mut self.reply {
            reply.push(text);
            return Ok(());
        }
        if let Some(reply) = &mut self.sse_reply {
            reply.push(text);
            return Ok(());
        }
        self.open()?;
        if self.config.delivery.is_websocket() {
            self.ws.push_text(text)?;
        }
        Ok(())
    }
    fn finish_text(&mut self, text: &str) -> Result<(), String> {
        if let Some(reply) = &mut self.reply {
            reply.finish();
            return Ok(());
        }
        if let Some(reply) = &mut self.sse_reply {
            reply.finish();
            return Ok(());
        }
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
        self.clip_started(start);
    }
    fn poll(&mut self, start: Instant) {
        if let Some(reply) = &mut self.sse_reply
            && let Err(error) = reply.pump(&mut self.sse, self.decoder.is_some())
        {
            self.error = Some(error);
        }
        for event in self.ws.poll().into_iter().chain(self.sse.poll()) {
            if let Some(reply) = &mut self.sse_reply
                && let Err(error) = reply.receive(&event)
            {
                self.error = Some(error);
            }
            match event {
                LiveEvent::Playing { audio, .. } => self.ready(audio, start),
                LiveEvent::Finished { stats, .. } => {
                    self.network_end = Some(ms(start));
                    if self.first_wire.is_none() {
                        self.first_wire = stats.first_audio_ms;
                    }
                    self.connection_ready = stats.connection_ready_ms;
                    self.first_text_written = stats.first_text_written_ms;
                    self.first_flush_written = stats.first_flush_written_ms;
                }
                LiveEvent::Failed { message, .. } => self.error = Some(message),
                _ => {}
            }
        }
        while let Some(event) = self.http.try_recv() {
            if let Some(reply) = &mut self.reply {
                if let Err(error) = reply.receive(event) {
                    self.error = Some(error);
                }
            } else {
                self.http_ready(event, start);
            }
        }
        if let Some(reply) = &mut self.reply {
            match reply.pump(&mut self.http, self.decoder.is_some()) {
                Ok(Some(event)) => self.http_ready(event, start),
                Ok(None) => {}
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(audio) = &self.streaming {
            if audio.is_failed() {
                self.error.get_or_insert("Streaming decoder failed".into());
            }
            if audio.is_finished() && !self.clip_network_done {
                self.network_end = Some(ms(start));
                self.clip_network_done = true;
            }
        }
        // Drain only published PCM, with no artificial underflow zeroes. This
        // measures decode readiness/transfer, not speaker scheduling or playback quality.
        let available = self
            .streaming
            .as_ref()
            .map(|audio| audio.buffered_frames() * self.channels);
        let budget = if self.reply.is_some() || self.sse_reply.is_some() {
            // Simulated real-time consumption preserves Studio's playback
            // backpressure without opening a device. Never consume underflow zeros.
            let now = Instant::now();
            let elapsed = self
                .last_drain
                .replace(now)
                .map_or(0.0, |at| now.duration_since(at).as_secs_f64());
            let due = elapsed * self.rate as f64 * self.channels as f64 + self.drain_fraction;
            self.drain_fraction = due.fract();
            due as usize
        } else {
            4096
        };
        let mut clip_done = false;
        if let Some(decoder) = self.decoder.as_mut() {
            for _ in 0..available.unwrap_or(4096).min(4096).min(budget) {
                match decoder.next() {
                    Some(sample) => {
                        self.samples += 1;
                        self.audio_duration_ms +=
                            1000.0 / (self.rate as f64 * self.channels as f64);
                        if sample.abs() > 0.00001 {
                            self.nonzero += 1;
                            self.first_nonzero.get_or_insert_with(|| ms(start));
                        }
                    }
                    None => {
                        clip_done = true;
                        break;
                    }
                }
            }
            if let Some(audio) = &self.streaming
                && audio.is_finished()
                && audio.buffered_frames() == 0
            {
                clip_done = true;
            }
        }
        if clip_done {
            if self.reply.is_some() || self.sse_reply.is_some() {
                self.decoder = None;
                self.streaming = None;
                self.last_clip_end = Some(Instant::now());
            } else {
                self.done = true;
            }
        }
        if let Some(reply) = &self.reply {
            self.done = reply.is_done() && self.decoder.is_none();
        }
        if let Some(reply) = &self.sse_reply {
            self.done = reply.is_done() && self.decoder.is_none();
        }
    }
    fn clip_started(&mut self, start: Instant) {
        let now = Instant::now();
        self.first_ready.get_or_insert_with(|| ms(start));
        self.chunk_ready_ms.push(ms(start));
        if let Some(end) = self.last_clip_end.take() {
            self.chunk_gaps_ms
                .push(now.duration_since(end).as_secs_f64() * 1000.0);
        }
        self.last_drain = Some(now);
        self.drain_fraction = 0.0;
        self.clip_network_done = false;
    }
    fn http_ready(&mut self, event: VoiceEvent, start: Instant) {
        match event {
            VoiceEvent::Streaming { audio, .. } => self.ready(audio, start),
            VoiceEvent::Ready { audio, .. } => {
                self.network_end = Some(ms(start));
                match rodio::Decoder::try_from(Cursor::new(audio.bytes)) {
                    Ok(decoder) => {
                        self.rate = decoder.sample_rate().get();
                        self.channels = usize::from(decoder.channels().get());
                        self.decoder = Some(Box::new(decoder));
                        self.streaming = None;
                        self.clip_started(start);
                    }
                    Err(_) => {
                        self.error = Some("Buffered audio failed production codec decoding".into())
                    }
                }
            }
            VoiceEvent::Failed { message, .. } => self.error = Some(message),
        }
    }
    fn cancel(&mut self) {
        self.ws.cancel();
        self.sse.cancel();
        self.http.cancel();
    }
    fn result(&self) -> Value {
        let duration = self.audio_duration_ms;
        // A later prefetched stream can finish before it becomes the active
        // decoder. Observing it only on activation is not a wire completion time.
        let network_done = self.reply.is_none().then_some(self.network_end).flatten();
        json!({"first_decoded_audio_ms":self.first_ready,"first_nonzero_audio_ms":self.first_nonzero,
            "network_done_ms":network_done,
            "last_clip_finished_observed_ms":self.reply.as_ref().and(self.network_end),"audio_duration_ms":duration,
            "speech_path":if self.reply.is_some() { "shared-studio-http-reply" } else if self.sse_reply.is_some() { "shared-studio-sse-reply" } else if self.config.delivery.is_websocket() { "live-websocket-adapter" } else { "full-reply-adapter" },
            "simulated_realtime_drain":self.reply.is_some() || self.sse_reply.is_some(),
            "chunk_ready_ms":self.chunk_ready_ms,"simulated_chunk_gaps_ms":self.chunk_gaps_ms,
            "earliest_playback_end_ms":self.first_ready.map(|t| t+duration),
            "nonzero_samples":self.nonzero,"transport_first_wire_audio_ms":self.first_wire,
            "ws_connection_ready_ms":self.connection_ready,"ws_first_text_written_ms":self.first_text_written,"ws_first_flush_written_ms":self.first_flush_written,
            "error":self.error})
    }
}

fn main() -> Result<(), String> {
    if !std::env::args().any(|arg| arg == "--run") {
        println!(
            "Explicit live benchmark: --run --phase llm|speech|pipeline --repeat 3 --models google/gemini-3.5-flash --speech-models s2.1-pro-free --modes ws,http,sse,buffered [--app-path --frame-ms 16] --output /absolute/results.json. No speaker or saved-setting changes."
        );
        return Ok(());
    }
    let app_path = std::env::args().any(|arg| arg == "--app-path");
    let frame_ms = arg("--frame-ms", "16")
        .parse::<u64>()
        .map_err(|_| "Invalid frame duration")?;
    if !(1..=100).contains(&frame_ms) {
        return Err("Frame duration must be 1–100 ms".into());
    }
    let (result_path, tick) = if app_path {
        ("app", Duration::from_millis(frame_ms))
    } else {
        ("direct", Duration::from_millis(1))
    };
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
                        let app_reply = app_path && phase == "pipeline";
                        let mut speech = Speech::new(config, app_reply);
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
                        let mut chat = None;
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
                            let turns = vec![Turn { role:"system".into(), content:"You are a friendly VTuber. Answer in two brief spoken sentences, 20 to 30 words total. No formatting, stage directions, or metadata.".into() },
                        Turn { role:"user".into(), content:user_text }];
                            if app_path {
                                let (tx, rx) = mpsc::channel();
                                chat = Some(Chat::App(rx, provider::spawn_chat(config, turns, tx)));
                            } else {
                                let (tx, rx) = mpsc::channel();
                                let stop = cancel.clone();
                                let observed = observation.clone();
                                chat = Some(Chat::Direct(
                                    rx,
                                    std::thread::spawn(move || {
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
                                    }),
                                ));
                            }
                            // Studio defers opening until filtered text arrives.
                            // Keep the historical eager-open policy only on the
                            // direct baseline or for explicit prewarm experiments.
                            if phase == "pipeline" && (!app_path || prewarm_ms > 0) {
                                speech.open()?;
                            }
                        } else if phase == "speech" {
                            text = TEXT.into();
                            speech.text(&text)?;
                            speech.finish_text(&text)?;
                        }
                        while start.elapsed() < Duration::from_secs(40) {
                            // App path: events and speech are only serviced once per frame.
                            for event in chat.as_ref().map(Chat::events).unwrap_or_default() {
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
                            std::thread::sleep(tick);
                        }
                        if speech.error.is_none()
                            && ((phase == "llm" && llm_end.is_none())
                                || (phase != "llm" && !speech.done))
                        {
                            speech.error = Some("Benchmark exceeded 40 seconds; no retry".into());
                        }
                        cancel.store(true, Ordering::Relaxed);
                        if let Some(Chat::App(_, job)) = &chat {
                            job.cancel();
                        }
                        let mut row = speech.result();
                        row["phase"] = phase.clone().into();
                        row["llm_model"] = llm_model.clone().into();
                        row["speech_model"] = speech_model.clone().into();
                        row["mode"] = delivery.clone().into();
                        row["routing_case"] = route.name.clone().into();
                        row["provider_routing"] = json!(route.routing);
                        row["result_path"] = result_path.into();
                        row["frame_ms"] = json!(app_path.then_some(frame_ms));
                        row["ws_open_policy"] = json!(if prewarm_ms > 0 {
                            "explicit-prewarm"
                        } else if app_path {
                            "first-text"
                        } else {
                            "send"
                        });
                        // spawn_chat exposes no raw events; never fabricate observation metadata.
                        if !app_path {
                            row["llm_observation"] = observation.lock().unwrap().clone();
                        }
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
                        speech.cancel();
                        if let Some(chat) = chat {
                            row["chat_worker_closed"] = chat.close(&cancel)?.into();
                        }
                        println!("{}", serde_json::to_string(&row).unwrap());
                        rows.push(row);
                        let result = json!({"version":2,"method":"production adapters; configured proxy; sequential benchmark rows; no speakers; app pipeline HTTP/SSE use shared Studio queues and simulated real-time consumption; other paths drain PCM; first audio means decoder readiness, not audible onset",
                "voice_tuning":{"latency":voice.latency,"chunk_length":voice.chunk_length,"min_chunk_length":voice.min_chunk_length,"condition_on_previous_chunks":voice.condition_on_previous_chunks},
                "prerecorded_asr":input_audio.is_some(),"asr_backend":arg("--asr-backend","fish"),"asr_model":asr_config.model,
                "routing":arg("--routing","default"),"routing_matrix":arg("--routing-matrix",""),"provider_only":arg("--provider-only",""),"provider_order":arg("--provider-order",""),"max_tokens":arg("--max-tokens","1024"),"reasoning":arg("--reasoning","low"),"prewarm_ms":arg("--prewarm-ms","0"),"result_path":result_path,"frame_ms":app_path.then_some(frame_ms),"rows":rows});
                        std::fs::write(&output, serde_json::to_vec_pretty(&result).unwrap())
                            .map_err(|_| "Could not save benchmark")?;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_http_path_speaks_before_finish_and_drains_every_chunk() {
        check_sentence_path(false);
    }
    #[test]
    fn app_sse_path_speaks_before_finish_and_drains_every_chunk() {
        check_sentence_path(true);
    }
    fn check_sentence_path(sse: bool) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut config = VoiceConfig::preset(if sse {
            VoiceAdapter::FishHosted
        } else {
            VoiceAdapter::FishLocal
        });
        config.delivery = if sse {
            VoiceDelivery::TimestampSse
        } else {
            VoiceDelivery::HttpWavStream
        };
        config.endpoint = format!("http://{}/v1/tts", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut requests = vec![];
            for incoming in listener.incoming().take(2) {
                let mut stream = incoming.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let body = loop {
                    let mut buffer = [0; 4096];
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0 && request.len() < 65536);
                    request.extend_from_slice(&buffer[..n]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let length: usize = String::from_utf8_lossy(&request[..end])
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap();
                        if request.len() >= end + 4 + length {
                            break request[end + 4..end + 4 + length].to_vec();
                        }
                    }
                };
                let body: Value = serde_json::from_slice(&body).unwrap();
                let text = body["text"].as_str().unwrap().to_owned();
                let rate: u32 = if text.starts_with("The first") {
                    8000
                } else {
                    16000
                };
                requests.push(text);
                let mut wav = b"RIFF".to_vec();
                wav.extend(1636u32.to_le_bytes());
                wav.extend(b"WAVEfmt ");
                wav.extend(16u32.to_le_bytes());
                wav.extend(1u16.to_le_bytes());
                wav.extend(1u16.to_le_bytes());
                wav.extend(rate.to_le_bytes());
                wav.extend((rate * 2).to_le_bytes());
                wav.extend(2u16.to_le_bytes());
                wav.extend(16u16.to_le_bytes());
                wav.extend(b"data");
                wav.extend(1600u32.to_le_bytes());
                wav.extend((0..800).flat_map(|_| 8192i16.to_le_bytes()));
                let (mime, body) = if sse {
                    use base64::Engine;
                    let payload = json!({"audio_base64":base64::engine::general_purpose::STANDARD.encode(&wav),
                        "content":"fixture", "alignment":null, "chunk_seq":0, "chunk_audio_offset_sec":0.0});
                    (
                        "text/event-stream",
                        format!("event: message\ndata: {payload}\n\n").into_bytes(),
                    )
                } else {
                    ("audio/wav", wav)
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
            requests
        });
        let first = "The first sentence is long enough to synthesize before completion.";
        let second = "The second sentence must play in full after the first sentence.";
        let mut speech = Speech::new(config.clone(), true);
        if sse {
            speech.sse_reply = Some(SseReplySpeech::new(
                1,
                config,
                Some("offline-test-key".into()),
            ));
        }
        let start = Instant::now();
        speech.text(&format!("{first} {second} ")).unwrap();
        while speech.first_ready.is_none() {
            speech.poll(start);
            assert!(speech.error.is_none(), "{:?}", speech.error);
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!speech.done, "unfinished text must keep the reply open");
        let first_ready = speech.first_ready;
        speech.finish_text("").unwrap();
        while !speech.done {
            speech.poll(start);
            assert!(speech.error.is_none(), "{:?}", speech.error);
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            speech.first_ready, first_ready,
            "later clips must not overwrite first audio"
        );
        assert_eq!(speech.samples, 1600);
        assert_eq!(speech.nonzero, 1600);
        assert_eq!(speech.chunk_ready_ms.len(), 2);
        assert_eq!(speech.chunk_gaps_ms.len(), 1);
        assert!((speech.audio_duration_ms - 150.0).abs() < 0.001);
        assert!(start.elapsed() >= Duration::from_millis(150));
        let mut requests = server.join().unwrap();
        requests.sort();
        assert_eq!(requests, [first.to_owned(), second.to_owned()]);
        speech.cancel();
    }
}
