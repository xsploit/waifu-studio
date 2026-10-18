//! Hosted Fish timestamp SSE, with one cancellable worker and one incremental WAV decoder.
//! Network requests retain reqwest's environment/system proxy handling. No WebSocket transport.
use crate::{
    voice::{VoiceAdapter, VoiceConfig},
    voice_stream::{StreamBudget, StreamingVoice, receive_wav_with_budget},
};
use base64::Engine;
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, BufRead, BufReader, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

const MAX_EVENT: usize = 2 * 1024 * 1024;
const MAX_WIRE: usize = 64 * 1024 * 1024;
const MAX_AUDIO: usize = 32 * 1024 * 1024;
const MAX_WORDS: usize = 12_000;
const MAX_WORD_BYTES: usize = 256 * 1024;
const MAX_EVENTS: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveTransport {
    #[default]
    TimestampSse,
}
impl LiveTransport {
    pub const ALL: [Self; 1] = [Self::TimestampSse];
    pub fn label(self) -> &'static str {
        "Timestamp SSE"
    }
    pub fn capability(self) -> &'static str {
        "Hosted Fish SSE: progressive WAV and cumulative word timings. WebSocket text streaming is not implemented."
    }
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct WordTiming {
    pub text: String,
    pub start: f64,
    pub end: f64,
}
#[derive(Clone, Debug, Default)]
pub struct LiveStats {
    pub audio_bytes: usize,
    /// Number of SSE audio/timing payloads; these are not independently playable clips.
    pub frames: usize,
    pub first_audio_ms: Option<u64>,
    /// WebSocket-only diagnostics, relative to this session's worker start.
    pub connection_ready_ms: Option<u64>,
    /// First text command accepted by the socket writer, not server acknowledgement.
    pub first_text_written_ms: Option<u64>,
    /// First explicit Fish flush command accepted by the socket writer.
    pub first_flush_written_ms: Option<u64>,
}
#[derive(Debug)]
pub enum LiveEvent {
    Playing {
        id: u64,
        audio: StreamingVoice,
    },
    Timing {
        id: u64,
        words: Vec<WordTiming>,
    },
    /// Network and decoder finished; queued playback may still be draining.
    Finished {
        id: u64,
        stats: LiveStats,
    },
    Failed {
        id: u64,
        message: String,
    },
}
impl LiveEvent {
    pub fn id(&self) -> u64 {
        match self {
            Self::Playing { id, .. }
            | Self::Timing { id, .. }
            | Self::Finished { id, .. }
            | Self::Failed { id, .. } => *id,
        }
    }
}
struct Request {
    id: u64,
    generation: u64,
    url: reqwest::Url,
    config: VoiceConfig,
    text: String,
    key: String,
}
type Mailbox = Arc<Mutex<VecDeque<(u64, LiveEvent)>>>;

#[derive(Resource)]
pub struct FishLiveState {
    pub enabled: bool,
    pub transport: LiveTransport,
    pub status: String,
    pub words: Vec<WordTiming>,
    generation: Arc<AtomicU64>,
    audio_generation: Arc<AtomicU64>,
    busy: Arc<AtomicBool>,
    mailbox: Mailbox,
    pending: Option<Request>,
}
impl Default for FishLiveState {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: LiveTransport::TimestampSse,
            status: "Ready · timestamp SSE".into(),
            words: Vec::new(),
            generation: Arc::new(AtomicU64::new(0)),
            audio_generation: Arc::new(AtomicU64::new(0)),
            busy: Arc::new(AtomicBool::new(false)),
            mailbox: Arc::new(Mutex::new(VecDeque::new())),
            pending: None,
        }
    }
}
impl FishLiveState {
    pub fn speak(
        &mut self,
        id: u64,
        config: &VoiceConfig,
        text: String,
        session_key: Option<String>,
    ) -> Result<(), String> {
        let url = validate(config, &text)?;
        let env_key = if config.api_key_env.trim().is_empty() {
            None
        } else {
            std::env::var(config.api_key_env.trim()).ok()
        };
        let key = session_key
            .filter(|s| !s.trim().is_empty())
            .or(env_key)
            .filter(|s| !s.trim().is_empty())
            .ok_or("Set a Fish session key or API key environment variable")?;
        if key.len() > 16 * 1024
            || reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")).is_err()
        {
            return Err("Invalid Fish API key header".into());
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.audio_generation.store(generation, Ordering::Release);
        if let Ok(mut mailbox) = self.mailbox.lock() {
            mailbox.clear();
        }
        self.words.clear();
        self.pending = Some(Request {
            id,
            generation,
            url,
            config: config.clone(),
            text,
            key,
        });
        self.status = "Connecting · timestamp SSE".into();
        self.start_pending();
        Ok(())
    }
    pub fn cancel(&mut self) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.audio_generation.store(generation, Ordering::Release);
        self.pending = None;
        if let Ok(mut mailbox) = self.mailbox.lock() {
            mailbox.clear();
        }
        self.words.clear();
        self.status = "Cancelled".into();
    }
    pub fn is_busy(&self) -> bool {
        self.pending.is_some() || self.busy.load(Ordering::Acquire)
    }
    fn start_pending(&mut self) {
        if self.pending.is_none() || self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let request = self.pending.take().unwrap();
        let generation = self.generation.clone();
        let audio_generation = self.audio_generation.clone();
        let busy = self.busy.clone();
        let mailbox = self.mailbox.clone();
        let id = request.id;
        let expected = request.generation;
        let spawned = std::thread::Builder::new()
            .name("fish-timestamp-sse".into())
            .spawn(move || {
                struct Permit(Arc<AtomicBool>);
                impl Drop for Permit {
                    fn drop(&mut self) {
                        self.0.store(false, Ordering::Release);
                    }
                }
                let _permit = Permit(busy);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(
                        request,
                        generation.clone(),
                        audio_generation.clone(),
                        mailbox.clone(),
                    )
                }))
                .unwrap_or_else(|_| Err("Fish streaming worker failed".into()));
                let event = match result {
                    Ok(stats) => LiveEvent::Finished { id, stats },
                    Err(message) => {
                        let _ = audio_generation.compare_exchange(
                            expected,
                            expected.wrapping_add(1),
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                        LiveEvent::Failed { id, message }
                    }
                };
                publish(&mailbox, &generation, expected, event);
            });
        if spawned.is_err() {
            self.busy.store(false, Ordering::Release);
            publish(
                &self.mailbox,
                &self.generation,
                expected,
                LiveEvent::Failed {
                    id,
                    message: "Could not start Fish streaming worker".into(),
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
                LiveEvent::Playing { .. } => self.status = "Playing · timestamp SSE".into(),
                LiveEvent::Timing { words, .. } => self.words = words.clone(),
                LiveEvent::Finished { stats, .. } => {
                    self.status = format!(
                        "Received {} KiB · playback draining",
                        stats.audio_bytes / 1024
                    )
                }
                LiveEvent::Failed { message, .. } => self.status = message.clone(),
            }
        }
        events
    }
}
impl Drop for FishLiveState {
    fn drop(&mut self) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.audio_generation.store(generation, Ordering::Release);
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
        // Timing is a replacement snapshot; retain one unconsumed snapshot.
        if matches!(event, LiveEvent::Timing { .. })
            && let Some((_, pending)) = queue.iter_mut().find(|(generation, event)| {
                *generation == expected && matches!(event, LiveEvent::Timing { .. })
            })
        {
            *pending = event;
            return;
        }
        if queue.len() >= MAX_EVENTS {
            if let Some(index) = queue
                .iter()
                .position(|(_, event)| matches!(event, LiveEvent::Timing { .. }))
            {
                queue.remove(index);
            } else {
                return;
            }
        }
        queue.push_back((expected, event));
    }
}
fn validate(config: &VoiceConfig, text: &str) -> Result<reqwest::Url, String> {
    if config.adapter != VoiceAdapter::FishHosted {
        return Err("Timestamp SSE requires the hosted Fish adapter".into());
    }
    if text.trim().is_empty() || text.chars().count() > 12_000 {
        return Err("Speech text must contain 1–12000 characters".into());
    }
    if !matches!(
        config.model.as_str(),
        "s1" | "s2-pro" | "s2.1-pro" | "s2.1-pro-free" | "drama-3"
    ) {
        return Err(
            "Choose a documented Fish model; unknown models can fall back to a paid model".into(),
        );
    }
    if !config.speed.is_finite() || !(0.5..=2.0).contains(&config.speed) {
        return Err("Speech speed must be between 0.5 and 2".into());
    }
    if !(100..=300).contains(&config.chunk_length)
        || !matches!(config.latency.as_str(), "normal" | "balanced" | "low")
    {
        return Err("Invalid hosted Fish chunk length or latency".into());
    }
    config.validate_hosted_tuning()?;
    if config.direction.chars().count() > 2000 || config.direction.contains(['[', ']']) {
        return Err("Voice direction must contain at most 2000 characters without brackets".into());
    }
    if !config.direction.trim().is_empty() && config.model != "drama-3" {
        return Err("Voice direction requires hosted Drama 3".into());
    }
    if config.reference_id.len() > 4096 {
        return Err("Fish reference ID exceeds its size limit".into());
    }
    let mut url = reqwest::Url::parse(&config.endpoint).map_err(|_| "Invalid Fish endpoint")?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use an endpoint without credentials, query or fragment".into());
    }
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("Fish requires HTTPS, or HTTP on localhost".into());
    }
    if url.host().is_none() {
        return Err("Fish endpoint requires a host".into());
    }
    // The hosted API contract defines this absolute route, regardless of the ordinary TTS path.
    url.set_path("/v1/tts/stream/with-timestamp");
    Ok(url)
}
fn run(
    request: Request,
    generation: Arc<AtomicU64>,
    audio_generation: Arc<AtomicU64>,
    mailbox: Mailbox,
) -> Result<LiveStats, String> {
    let expected = request.generation;
    let id = request.id;
    if generation.load(Ordering::Acquire) != expected {
        return Err("Speech cancelled".into());
    }
    let budget = StreamBudget::default();
    let text = if request.config.direction.trim().is_empty() {
        request.text
    } else {
        format!("[{}] {}", request.config.direction.trim(), request.text)
    };
    let mut body = serde_json::json!({"text":text,"format":"wav","normalize":true,"latency":request.config.latency,"chunk_length":request.config.chunk_length,"min_chunk_length":request.config.min_chunk_length,"condition_on_previous_chunks":request.config.condition_on_previous_chunks,"prosody":{"speed":request.config.speed}});
    if !request.config.reference_id.trim().is_empty() {
        body["reference_id"] = request.config.reference_id.trim().into();
    }
    // Do not use no_proxy(): normal reqwest routing honors the user's proxy configuration.
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize Fish streaming client")?;
    let response = client
        .post(request.url)
        .bearer_auth(request.key)
        .header("model", request.config.model)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(&body)
        .send()
        .map_err(|_| "Fish stream connection failed or timed out")?;
    if !response.status().is_success() {
        return Err(format!(
            "Fish stream returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    if !mime.eq_ignore_ascii_case("text/event-stream") {
        return Err("Fish stream returned an unsupported content type".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_WIRE as u64)
    {
        return Err("Fish SSE response exceeds 64 MiB".into());
    }
    let mut input = TimestampReader::new(
        response,
        generation.clone(),
        expected,
        budget.clone(),
        |words| {
            publish(
                &mailbox,
                &generation,
                expected,
                LiveEvent::Timing { id, words },
            )
        },
    );
    receive_wav_with_budget(&mut input, audio_generation, expected, &budget, |audio| {
        publish(
            &mailbox,
            &generation,
            expected,
            LiveEvent::Playing { id, audio },
        )
    })
    .map_err(|error| format!("Fish timestamp SSE WAV: {error}"))?;
    // A known-length WAV may finish before final alignment snapshots arrive.
    // Drain the SSE framing separately, rather than losing those snapshots.
    input.drain_to_eof().map_err(|e| e.to_string())?;
    Ok(input.stats)
}

#[derive(Deserialize)]
struct Alignment {
    audio_duration: f64,
    segments: Vec<WordTiming>,
}
#[derive(Deserialize)]
struct TimestampFrame {
    audio_base64: String,
    content: String,
    alignment: Option<Alignment>,
    chunk_seq: u64,
    chunk_audio_offset_sec: f64,
}
struct TimestampReader<R, F> {
    reader: BufReader<R>,
    active: Arc<AtomicU64>,
    expected: u64,
    budget: StreamBudget,
    wire: usize,
    audio: Vec<u8>,
    cursor: usize,
    snapshots: BTreeMap<u64, Vec<WordTiming>>,
    timing: F,
    stats: LiveStats,
    eof: bool,
    first_line: bool,
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
impl<R: Read, F: FnMut(Vec<WordTiming>)> TimestampReader<R, F> {
    fn new(
        reader: R,
        active: Arc<AtomicU64>,
        expected: u64,
        budget: StreamBudget,
        timing: F,
    ) -> Self {
        Self {
            reader: BufReader::with_capacity(8192, reader),
            active,
            expected,
            budget,
            wire: 0,
            audio: Vec::new(),
            cursor: 0,
            snapshots: BTreeMap::new(),
            timing,
            stats: LiveStats::default(),
            eof: false,
            first_line: true,
        }
    }
    fn check(&self) -> io::Result<()> {
        if self.active.load(Ordering::Acquire) != self.expected {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Speech cancelled",
            ));
        }
        self.budget
            .check()
            .map_err(|message| io::Error::new(io::ErrorKind::TimedOut, message))?;
        Ok(())
    }
    fn line(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut line = Vec::new();
        loop {
            self.check()?;
            let available = self
                .reader
                .fill_buf()
                .map_err(|_| invalid("Could not receive Fish SSE stream"))?;
            if available.is_empty() {
                return if line.is_empty() {
                    Ok(None)
                } else {
                    Err(invalid("Truncated Fish SSE line"))
                };
            }
            let length = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |n| n + 1);
            if line.len() + length > MAX_EVENT || self.wire + length > MAX_WIRE {
                return Err(invalid("Fish SSE data exceeds its size limit"));
            }
            line.extend_from_slice(&available[..length]);
            self.reader.consume(length);
            self.wire += length;
            if line.last() == Some(&b'\n') {
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(line));
            }
        }
    }
    fn next_payload(&mut self) -> io::Result<bool> {
        let mut data = Vec::new();
        let mut event = String::new();
        loop {
            let Some(mut line) = self.line()? else {
                if !data.is_empty() {
                    return Err(invalid("Fish SSE ended before its event delimiter"));
                }
                self.eof = true;
                return Ok(false);
            };
            if self.first_line {
                self.first_line = false;
                if line.starts_with(&[0xef, 0xbb, 0xbf]) {
                    line.drain(..3);
                }
            }
            if line.is_empty() {
                if data.is_empty() {
                    event.clear();
                    continue;
                }
                if event == "error" {
                    return Err(invalid("Fish provider reported a stream error"));
                }
                if !event.is_empty() && event != "message" {
                    data.clear();
                    event.clear();
                    continue;
                }
                self.process(&data)?;
                return Ok(true);
            }
            if line[0] == b':' {
                continue;
            }
            let (field, value) = match line.iter().position(|b| *b == b':') {
                Some(n) => (&line[..n], &line[n + 1..]),
                None => (&line[..], &[][..]),
            };
            let value = value.strip_prefix(b" ").unwrap_or(value);
            match field {
                b"data" => {
                    if !data.is_empty() {
                        data.push(b'\n');
                    }
                    if data.len() + value.len() > MAX_EVENT {
                        return Err(invalid("Fish SSE event exceeds 2 MiB"));
                    }
                    data.extend_from_slice(value);
                }
                b"event" => {
                    if value.len() > 128 {
                        return Err(invalid("Invalid Fish SSE event name"));
                    }
                    event = std::str::from_utf8(value)
                        .map_err(|_| invalid("Invalid Fish SSE event name"))?
                        .into();
                }
                _ => {}
            }
        }
    }
    fn process(&mut self, data: &[u8]) -> io::Result<()> {
        let frame: TimestampFrame =
            serde_json::from_slice(data).map_err(|_| invalid("Invalid Fish timestamp event"))?;
        if frame.chunk_seq > 4095
            || !frame.chunk_audio_offset_sec.is_finite()
            || !(0.0..=3600.0).contains(&frame.chunk_audio_offset_sec)
            || frame.content.len() > 64 * 1024
        {
            return Err(invalid("Invalid Fish timestamp metadata"));
        }
        self.stats.frames += 1;
        if self.stats.frames > 100_000 {
            return Err(invalid("Fish SSE has too many events"));
        }
        let audio = base64::engine::general_purpose::STANDARD
            .decode(frame.audio_base64.as_bytes())
            .map_err(|_| invalid("Invalid Fish audio base64"))?;
        self.stats.audio_bytes += audio.len();
        if self.stats.audio_bytes > MAX_AUDIO {
            return Err(invalid("Fish audio exceeds 32 MiB"));
        }
        if !audio.is_empty() && self.stats.first_audio_ms.is_none() {
            self.stats.first_audio_ms = Some(
                self.budget
                    .started()
                    .elapsed()
                    .as_millis()
                    .min(u64::MAX as u128) as u64,
            );
        }
        // Audio ordering is independent of chunk_seq: some frames align the previous chunk.
        self.audio = audio;
        self.cursor = 0;
        if let Some(alignment) = frame.alignment {
            if !alignment.audio_duration.is_finite()
                || !(0.0..=3600.0).contains(&alignment.audio_duration)
                || alignment.segments.len() > MAX_WORDS
            {
                return Err(invalid("Fish alignment exceeds supported bounds"));
            }
            let mut words = alignment.segments;
            for word in &mut words {
                if word.text.len() > 2048
                    || !word.start.is_finite()
                    || !word.end.is_finite()
                    || word.start < 0.0
                    || word.end < word.start
                    || word.end > 3600.0
                {
                    return Err(invalid("Invalid Fish word timing"));
                }
                word.start += frame.chunk_audio_offset_sec;
                word.end += frame.chunk_audio_offset_sec;
            }
            let mut count = words.len();
            let mut bytes = words.iter().map(|word| word.text.len()).sum::<usize>();
            for (seq, snapshot) in &self.snapshots {
                if *seq != frame.chunk_seq {
                    count += snapshot.len();
                    bytes += snapshot.iter().map(|word| word.text.len()).sum::<usize>();
                }
            }
            if count > MAX_WORDS || bytes > MAX_WORD_BYTES {
                return Err(invalid("Fish word alignment exceeds its total size limit"));
            }
            self.snapshots.insert(frame.chunk_seq, words);
            let mut snapshot: Vec<_> = self.snapshots.values().flatten().cloned().collect();
            snapshot.sort_by(|a, b| a.start.total_cmp(&b.start));
            (self.timing)(snapshot);
        }
        Ok(())
    }
    fn drain_to_eof(&mut self) -> io::Result<()> {
        if self.cursor < self.audio.len() {
            return Err(invalid("Fish sent audio beyond the completed WAV"));
        }
        while !self.eof && self.next_payload()? {
            if !self.audio.is_empty() {
                return Err(invalid("Fish sent audio beyond the completed WAV"));
            }
        }
        Ok(())
    }
}
impl<R: Read, F: FnMut(Vec<WordTiming>)> Read for TimestampReader<R, F> {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        if target.is_empty() {
            return Ok(0);
        }
        while self.cursor == self.audio.len() {
            if self.eof || !self.next_payload()? {
                return Ok(0);
            }
        }
        let count = target.len().min(self.audio.len() - self.cursor);
        target[..count].copy_from_slice(&self.audio[self.cursor..self.cursor + count]);
        self.cursor += count;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice_stream::receive_wav;
    use bevy::audio::Decodable;
    use serde_json::json;
    use std::{
        io::{Cursor, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc,
        time::Instant,
    };

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
    fn frame(audio: &[u8], seq: u64, offset: f64, words: Option<Vec<WordTiming>>) -> Vec<u8> {
        let alignment=words.map(|words|json!({"audio_duration":0.5,"segments":words.into_iter().map(|word|json!({"start":word.start,"end":word.end,"text":word.text})).collect::<Vec<_>>() }));
        let json = json!({"audio_base64":base64::engine::general_purpose::STANDARD.encode(audio),"content":"Hello world","alignment":alignment,"chunk_seq":seq,"chunk_audio_offset_sec":offset});
        format!("event: message\r\ndata: {json}\r\n\r\n").into_bytes()
    }
    fn word(text: &str, start: f64, end: f64) -> WordTiming {
        WordTiming {
            text: text.into(),
            start,
            end,
        }
    }
    struct Fragmented(Cursor<Vec<u8>>);
    impl Read for Fragmented {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let size = out.len().min(1);
            self.0.read(&mut out[..size])
        }
    }
    fn reader<'a>(
        bytes: Vec<u8>,
        words: &'a mut Vec<Vec<WordTiming>>,
    ) -> TimestampReader<Fragmented, impl FnMut(Vec<WordTiming>) + 'a> {
        TimestampReader::new(
            Fragmented(Cursor::new(bytes)),
            Arc::new(AtomicU64::new(1)),
            1,
            crate::voice_stream::StreamBudget::with_limits(
                Duration::from_secs(3),
                Duration::from_secs(3),
                Duration::from_secs(3),
            ),
            |snapshot| words.push(snapshot),
        )
    }
    #[test]
    fn fragmented_sse_concatenates_audio_and_replaces_cumulative_alignment() {
        let audio = wav(2, true);
        let mut wire = b"\xef\xbb\xbf: heartbeat\r\n\r\n".to_vec();
        wire.extend(frame(&audio[..1], 0, 0.0, None));
        wire.extend(frame(
            &audio[1..13],
            0,
            0.0,
            Some(vec![word("Hello", 0.0, 0.1)]),
        ));
        wire.extend(frame(
            &audio[13..],
            0,
            0.0,
            Some(vec![word("Hello", 0.0, 0.1), word("world", 0.1, 0.2)]),
        ));
        wire.extend(frame(&[], 1, 0.2, Some(vec![word("again", 0.0, 0.1)])));
        let mut snapshots = Vec::new();
        let mut stream = reader(wire, &mut snapshots);
        let mut received = Vec::new();
        stream.read_to_end(&mut received).unwrap();
        assert_eq!(received, audio);
        assert_eq!(stream.stats.audio_bytes, audio.len());
        assert_eq!(stream.stats.frames, 4);
        assert!(stream.stats.first_audio_ms.is_some());
        drop(stream);
        assert_eq!(snapshots.last().unwrap().len(), 3);
        assert_eq!(
            snapshots.last().unwrap()[2],
            word("again", 0.2, 0.30000000000000004)
        );
        assert_eq!(snapshots[1].len(), 2); // replacement, not Hello duplicated twice
    }

    #[test]
    fn actual_hosted_placeholder_survives_fragmented_timestamp_sse_and_pcm_decode() {
        use bevy::audio::Decodable;
        let mut audio = wav(16, false);
        audio[4..8].copy_from_slice(&0xffff_ff24u32.to_le_bytes());
        audio[40..44].copy_from_slice(&0xffff_ff00u32.to_le_bytes());
        let mut wire = frame(&audio[..17], 0, 0.0, None);
        wire.extend(frame(&audio[17..], 0, 0.0, None));
        let mut words = Vec::new();
        let mut stream = reader(wire, &mut words);
        let mut playable = None;
        crate::voice_stream::receive_wav_with_budget(
            &mut stream,
            Arc::new(AtomicU64::new(1)),
            1,
            &StreamBudget::default(),
            |audio| playable = Some(audio),
        )
        .unwrap();
        let decoded: Vec<_> = playable.unwrap().decoder().collect();
        assert_eq!(decoded.len(), 16);
        assert!(decoded.iter().all(|sample| *sample == 0.25));
    }
    #[test]
    fn finite_wav_still_receives_final_alignment_and_truncation_fails_closed() {
        let audio = wav(2, false);
        let mut wire = frame(&audio, 0, 0.0, None);
        wire.extend(frame(&[], 0, 0.0, Some(vec![word("final", 0.0, 0.2)])));
        let mut words = Vec::new();
        let mut input = reader(wire, &mut words);
        let mut played = None;
        receive_wav(
            &mut input,
            Arc::new(AtomicU64::new(1)),
            1,
            Instant::now() + Duration::from_secs(2),
            |audio| played = Some(audio),
        )
        .unwrap();
        input.drain_to_eof().unwrap();
        drop(input);
        assert_eq!(words.last().unwrap()[0].text, "final");
        let mut decoder = played.unwrap().decoder();
        assert_eq!(decoder.next(), Some(0.25));
        assert_eq!(decoder.next(), Some(0.25));
        assert_eq!(decoder.next(), None);
        let mut wire = frame(&audio, 0, 0.0, None);
        wire.truncate(wire.len() - 2);
        let mut words = Vec::new();
        let mut input = reader(wire, &mut words);
        assert!(input.read_to_end(&mut Vec::new()).is_err());
    }
    #[test]
    fn limits_invalid_audio_provider_errors_and_cancellation_are_redacted() {
        let mut invalid_frame = json!({"audio_base64":"%%%","content":"secret provider text","alignment":null,"chunk_seq":0,"chunk_audio_offset_sec":0.0});
        let mut snapshots = Vec::new();
        for malformed in [
            format!("data: {invalid_frame}\n\n"),
            "event: error\ndata: secret-provider-and-key\n\n".into(),
        ] {
            let mut input = reader(malformed.into_bytes(), &mut snapshots);
            let error = input.read_to_end(&mut Vec::new()).unwrap_err().to_string();
            assert!(!error.contains("secret"));
        }
        invalid_frame["audio_base64"] = json!("");
        invalid_frame["chunk_seq"] = json!(4096);
        let mut input = reader(
            format!("data: {invalid_frame}\n\n").into_bytes(),
            &mut snapshots,
        );
        assert!(input.read_to_end(&mut Vec::new()).is_err());
        drop(input);
        let mut input = reader(vec![b'x'; MAX_EVENT + 1], &mut snapshots);
        assert!(input.read_to_end(&mut Vec::new()).is_err());
        drop(input);
        let mut input = reader(frame(&wav(2, true), 0, 0.0, None), &mut snapshots);
        input.active.store(2, Ordering::Release);
        assert!(input.read(&mut [0]).is_err());
    }
    fn request(socket: &mut TcpStream) -> String {
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut all = Vec::new();
        let mut bytes = [0; 4096];
        loop {
            let n = socket.read(&mut bytes).unwrap();
            assert_ne!(n, 0);
            all.extend_from_slice(&bytes[..n]);
            if let Some(end) = all.windows(4).position(|b| b == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&all[..end]);
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|n| n.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if all.len() >= end + 4 + length {
                    return String::from_utf8(all).unwrap();
                }
            }
        }
    }
    fn send_chunk(socket: &mut TcpStream, bytes: &[u8]) {
        // HTTP chunk boundaries deliberately do not match SSE lines or WAV headers.
        for chunk in bytes.chunks(17) {
            write!(socket, "{:x}\r\n", chunk.len()).unwrap();
            socket.write_all(chunk).unwrap();
            socket.write_all(b"\r\n").unwrap();
        }
        socket.flush().unwrap();
    }
    #[test]
    fn offline_http_sse_plays_before_eof_and_keeps_exact_hosted_contract() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/tts", listener.local_addr().unwrap());
        let (release, wait) = mpsc::channel();
        let task = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let request = request(&mut socket);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
            let audio = wav(1600, true);
            let mut offset = 0;
            for end in [1, 7, 13, 35, 44, audio.len()] {
                send_chunk(&mut socket, &frame(&audio[offset..end], 0, 0.0, None));
                offset = end;
            }
            // Server cannot finish until the client has received and consumed playable PCM.
            wait.recv_timeout(Duration::from_secs(4)).unwrap();
            send_chunk(
                &mut socket,
                &frame(
                    &[],
                    0,
                    0.0,
                    Some(vec![word("Hello", 0.0, 0.1), word("world", 0.1, 0.2)]),
                ),
            );
            send_chunk(
                &mut socket,
                &frame(&[], 1, 0.2, Some(vec![word("again", 0.0, 0.1)])),
            );
            socket.write_all(b"0\r\n\r\n").unwrap();
            request
        });
        let mut state = FishLiveState::default();
        let config = VoiceConfig {
            endpoint,
            format: "mp3".into(),
            reference_id: "test-voice".into(),
            min_chunk_length: 35,
            condition_on_previous_chunks: false,
            ..Default::default()
        };
        state
            .speak(
                42,
                &config,
                "Hello world".into(),
                Some("offline-test-key".into()),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut released = false;
        let mut finished = false;
        let mut decoder = None;
        while Instant::now() < deadline && !finished {
            for event in state.poll() {
                assert_eq!(event.id(), 42);
                match event {
                    LiveEvent::Playing { audio, .. } => {
                        assert!(!released);
                        let mut playing = audio.decoder();
                        assert_eq!(playing.next(), Some(0.25));
                        decoder = Some(playing);
                        release.send(()).unwrap();
                        released = true;
                    }
                    LiveEvent::Finished { stats, .. } => {
                        assert_eq!(stats.audio_bytes, 3244);
                        assert!(stats.first_audio_ms.is_some());
                        finished = true;
                    }
                    LiveEvent::Failed { message, .. } => panic!("{message}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(released && finished);
        assert_eq!(state.words.len(), 3);
        assert_eq!(state.words[2].start, 0.2);
        assert!(decoder.unwrap().take(2000).all(|sample| sample == 0.25));
        let request = task.join().unwrap();
        assert!(request.starts_with("POST /v1/tts/stream/with-timestamp HTTP/1.1"));
        assert!(request.contains("model: s2.1-pro-free"));
        let body: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["format"], "wav");
        assert_eq!(body["reference_id"], "test-voice");
        assert_eq!(body["min_chunk_length"], 35);
        assert_eq!(body["condition_on_previous_chunks"], false);
        assert!(body.get("streaming").is_none());
    }
    #[test]
    fn validation_mailbox_coalescing_and_cancel_reject_stale_events() {
        let config = VoiceConfig::default();
        assert_eq!(
            validate(&config, "Hi").unwrap().path(),
            "/v1/tts/stream/with-timestamp"
        );
        let mut bad = config.clone();
        bad.model = "unknown".into();
        assert!(validate(&bad, "Hi").is_err());
        let mut bad = config.clone();
        bad.min_chunk_length = 101;
        assert_eq!(
            validate(&bad, "Hi").unwrap_err(),
            "Fish minimum chunk length must be between 0 and 100"
        );
        let mut edge = config.clone();
        edge.min_chunk_length = 100;
        assert!(validate(&edge, "Hi").is_ok());
        let mut bad = config.clone();
        bad.adapter = VoiceAdapter::FishLocal;
        assert!(validate(&bad, "Hi").is_err());
        let mut bad = config;
        bad.endpoint = "https://secret:key@api.fish.audio/v1/tts".into();
        assert!(validate(&bad, "Hi").is_err());
        let mut state = FishLiveState::default();
        for n in 0..100 {
            publish(
                &state.mailbox,
                &state.generation,
                0,
                LiveEvent::Timing {
                    id: 1,
                    words: vec![word("Hi", n as f64, n as f64 + 0.1)],
                },
            );
        }
        assert_eq!(state.mailbox.lock().unwrap().len(), 1);
        assert_eq!(state.poll().len(), 1);
        assert_eq!(state.words[0].start, 99.0);
        state.cancel();
        publish(
            &state.mailbox,
            &state.generation,
            0,
            LiveEvent::Failed {
                id: 1,
                message: "stale".into(),
            },
        );
        assert!(state.poll().is_empty());
        assert!(state.words.is_empty());
    }
}
