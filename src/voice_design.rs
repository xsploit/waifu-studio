//! Explicit paid Fish Voice Design previews; no persistent voice-model creation.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
const MAX_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Serialize)]
struct Request {
    instruction: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    reference_text: String,
    n: u8,
    speed: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<i64>,
}
impl Request {
    fn validate(&self) -> Result<(), String> {
        if self.instruction.trim().is_empty() || self.instruction.chars().count() > 2000 {
            return Err("Describe a voice in 1–2000 characters".into());
        }
        if self.reference_text.chars().count() > 150 {
            return Err("Preview text exceeds 150 characters".into());
        }
        if !(1..=4).contains(&self.n)
            || !self.speed.is_finite()
            || self.speed <= 0.0
            || self.speed > 3.0
        {
            return Err("Use 1–4 candidates and speed greater than 0, at most 3".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct VoiceCandidate {
    pub id: String,
    pub index: u32,
    pub sample_rate: u32,
    pub duration_ms: u64,
    pub audio: Vec<u8>,
}
#[derive(Deserialize)]
struct Response {
    candidates: Vec<WireCandidate>,
}
#[derive(Deserialize)]
struct WireCandidate {
    id: String,
    index: u32,
    audio_base64: String,
    sample_rate: u32,
    duration_ms: u64,
}
struct Pending {
    receiver: Mutex<mpsc::Receiver<Result<Vec<VoiceCandidate>, String>>>,
    cancelled: Arc<AtomicBool>,
}
#[derive(Resource)]
pub struct VoiceDesignState {
    pub endpoint: String,
    pub session_key: String,
    pub api_key_env: String,
    pub instruction: String,
    pub reference_text: String,
    pub n: u8,
    pub speed: f32,
    pub seed: Option<i64>,
    pub candidates: Vec<VoiceCandidate>,
    pub selected: usize,
    pub status: String,
    pending: Option<Pending>,
    preview: Option<Vec<u8>>,
}
impl Default for VoiceDesignState {
    fn default() -> Self {
        Self {
            endpoint: "https://api.fish.audio/v1/voice-design".into(),
            session_key: String::new(),
            api_key_env: "FISH_API_KEY".into(),
            instruction: String::new(),
            reference_text: String::new(),
            n: 2,
            speed: 1.0,
            seed: None,
            candidates: vec![],
            selected: 0,
            status: "Previews only; no voice has been generated".into(),
            pending: None,
            preview: None,
        }
    }
}
impl VoiceDesignState {
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }
    /// Explicit Generate UI action only. Never call automatically on text changes.
    pub fn generate(&mut self) -> Result<(), String> {
        if self.is_busy() {
            return Err("Wait for the existing request to finish".into());
        }
        let request = Request {
            instruction: self.instruction.clone(),
            reference_text: self.reference_text.clone(),
            n: self.n,
            speed: self.speed,
            seed: self.seed,
        };
        request.validate()?;
        validate_endpoint(&self.endpoint)?;
        let key = if self.session_key.trim().is_empty() {
            std::env::var(&self.api_key_env).unwrap_or_default()
        } else {
            self.session_key.clone()
        };
        if key.trim().is_empty() || key.len() > 8192 || key.chars().any(char::is_control) {
            return Err(
                "Provide a valid session key or configured key environment variable".into(),
            );
        }
        let endpoint = self.endpoint.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let (tx, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("fish-voice-design".into())
            .spawn(move || {
                let result = execute(&endpoint, &key, &request, &flag);
                let _ = tx.send(result);
            })
            .map_err(|_| "Cannot start Voice Design worker")?;
        self.pending = Some(Pending {
            receiver: Mutex::new(receiver),
            cancelled,
        });
        self.preview = None;
        self.candidates.clear();
        self.status = "Generating paid previews…".into();
        Ok(())
    }
    pub fn cancel(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancelled.store(true, Ordering::SeqCst);
            self.status = "Preview cancelled; waiting for bounded request to finish. Server billing may still occur.".into();
        }
        self.preview = None;
    }
    pub fn poll(&mut self) {
        let Some(pending) = &self.pending else {
            return;
        };
        let result = match pending.receiver.lock().unwrap().try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(_) => Err("Voice Design worker stopped".into()),
        };
        let cancelled = pending.cancelled.load(Ordering::SeqCst);
        self.pending = None;
        if cancelled {
            self.status = "Preview cancelled; stale result discarded".into();
            return;
        }
        match result {
            Ok(candidates) => {
                self.candidates = candidates;
                self.selected = 0;
                self.status =
                    "Preview candidates ready; candidate IDs are not TTS reference IDs".into();
            }
            Err(error) => self.status = error,
        }
    }
    pub fn preview_selected(&mut self) {
        if let Some(candidate) = self.candidates.get(self.selected) {
            self.preview = Some(candidate.audio.clone());
        }
    }
    pub fn take_preview(&mut self) -> Option<Vec<u8>> {
        self.preview.take()
    }
}
impl Drop for VoiceDesignState {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancelled.store(true, Ordering::SeqCst);
        }
    }
}
fn validate_endpoint(value: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid Voice Design URL")?;
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/v1/voice-design"
    {
        return Err(
            "Use HTTPS (or local HTTP) with path /v1/voice-design and no URL credentials/query"
                .into(),
        );
    }
    Ok(())
}
fn execute(
    endpoint: &str,
    key: &str,
    request: &Request,
    cancelled: &AtomicBool,
) -> Result<Vec<VoiceCandidate>, String> {
    request.validate()?;
    validate_endpoint(endpoint)?;
    if cancelled.load(Ordering::SeqCst) {
        return Err("Preview cancelled".into());
    }
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|_| "Cannot initialize Voice Design client")?;
    let mut response = client
        .post(endpoint)
        .bearer_auth(key)
        .header("model", "voice-design-1")
        .json(request)
        .send()
        .map_err(|_| "Voice Design request failed or timed out; no automatic retry")?;
    if !response.status().is_success() {
        return Err(format!(
            "Voice Design returned HTTP {}; no automatic retry",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err("Voice Design response exceeds 32 MiB".into());
    }
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err("Preview cancelled".into());
        }
        let n = response
            .read(&mut chunk)
            .map_err(|_| "Cannot read Voice Design response")?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > MAX_BYTES {
            return Err("Voice Design response exceeds 32 MiB".into());
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    decode(&bytes, request.n)
}
fn decode(bytes: &[u8], requested: u8) -> Result<Vec<VoiceCandidate>, String> {
    if bytes.len() > MAX_BYTES {
        return Err("Voice Design response exceeds 32 MiB".into());
    }
    let response: Response =
        serde_json::from_slice(bytes).map_err(|_| "Invalid Voice Design response schema")?;
    if response.candidates.is_empty() || response.candidates.len() > usize::from(requested.min(4)) {
        return Err("Unexpected Voice Design candidate count".into());
    }
    let mut total = 0usize;
    let mut results = Vec::new();
    for candidate in response.candidates {
        if candidate.id.is_empty()
            || candidate.id.len() > 1024
            || candidate.sample_rate == 0
            || candidate.sample_rate > 384000
            || candidate.duration_ms == 0
            || candidate.duration_ms > 600000
            || candidate.index >= u32::from(requested)
            || results
                .iter()
                .any(|c: &VoiceCandidate| c.index == candidate.index || c.id == candidate.id)
        {
            return Err("Invalid Voice Design candidate metadata".into());
        }
        let audio = STANDARD
            .decode(candidate.audio_base64)
            .map_err(|_| "Invalid preview audio base64")?;
        total = total
            .checked_add(audio.len())
            .ok_or("Preview audio size overflow")?;
        if audio.is_empty() || total > MAX_BYTES {
            return Err("Preview audio exceeds limits".into());
        }
        results.push(VoiceCandidate {
            id: candidate.id,
            index: candidate.index,
            sample_rate: candidate.sample_rate,
            duration_ms: candidate.duration_ms,
            audio,
        });
    }
    results.sort_by_key(|c| c.index);
    Ok(results)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Request {
        Request {
            instruction: "Warm adult voice".into(),
            reference_text: "Hello".into(),
            n: 1,
            speed: 1.0,
            seed: Some(5),
        }
    }
    #[test]
    fn validation_and_schema_reject_errors() {
        let mut r = request();
        r.speed = 0.0;
        assert!(r.validate().is_err());
        r = request();
        r.instruction = "x".repeat(2001);
        assert!(r.validate().is_err());
        assert!(validate_endpoint("http://example.com/v1/voice-design").is_err());
        assert!(validate_endpoint("https://user:secret@api.fish.audio/v1/voice-design").is_err());
        assert!(decode(br#"{"candidates":[]}"#, 1).is_err());
        assert!(decode(br#"{"candidates":[{"id":"x","index":0,"audio_base64":"!!","sample_rate":44100,"duration_ms":10}]}"#,1).is_err());
    }
    #[test]
    fn fake_http_checks_contract_without_credentials_or_paid_calls() {
        use std::{io::Write, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/voice-design", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 1024];
            loop {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(split) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..split]).to_lowercase();
                    let size: usize = headers
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap();
                    if bytes.len() >= split + 4 + size {
                        assert!(headers.starts_with("post /v1/voice-design "));
                        assert!(headers.contains("model: voice-design-1"));
                        assert!(headers.contains("authorization: bearer fake-test-key"));
                        let body: serde_json::Value =
                            serde_json::from_slice(&bytes[split + 4..]).unwrap();
                        assert_eq!(body["instruction"], "Warm adult voice");
                        assert_eq!(body["n"], 1);
                        assert!(body.get("reference_id").is_none());
                        break;
                    }
                }
            }
            let body = r#"{"candidates":[{"id":"preview-only","index":0,"audio_base64":"UklGRg==","sample_rate":44100,"duration_ms":10}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let result = execute(
            &endpoint,
            "fake-test-key",
            &request(),
            &AtomicBool::new(false),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(result[0].audio, b"RIFF");
    }
    #[test]
    fn fake_http_errors_and_cancellation_are_terminal() {
        use std::{io::Write, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/voice-design", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut buf = [0; 4096];
            let _ = stream.read(&mut buf).unwrap();
            stream.write_all(b"HTTP/1.1 402 Payment Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let error = execute(
            &endpoint,
            "fake-test-key",
            &request(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.contains("402"));
        server.join().unwrap();
        assert!(
            execute(
                &endpoint,
                "fake-test-key",
                &request(),
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
}
