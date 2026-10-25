//! Speech model discovery, performed once per configuration on a background worker.
//! Catalogs advertise capabilities; users may still enter an unlisted model ID.
use bevy::prelude::Resource;
use reqwest::Url;
use serde_json::Value;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    io::Read,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub const FISH_SPEECH_MODELS: [&str; 5] = ["s1", "s2-pro", "s2.1-pro", "s2.1-pro-free", "drama-3"];
pub const FISH_ASR_MODELS: [&str; 2] = ["transcribe-1", "transcribe-1-pro"];
const MAX_BYTES: u64 = 5_000_000;
static WORKERS: AtomicUsize = AtomicUsize::new(0);
struct Worker;
impl Worker {
    fn acquire() -> Option<Self> {
        WORKERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 4).then_some(count + 1)
            })
            .ok()
            .map(|_| Self)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CatalogKind {
    Speech,
    Transcription,
    /// Vercel evaluation-protocol models (`type: evaluation`): Jev, Decision-1, Decider…
    Decision,
}

#[derive(Resource, Default)]
pub struct SpeechCatalogs {
    pub tts: ModelCatalog,
    pub stt: ModelCatalog,
}

type CatalogResult = (u64, Result<Vec<String>, String>);
struct PendingCatalog {
    endpoint: String,
    key_env: String,
    session_key: String,
    kind: CatalogKind,
    due: Instant,
}

pub struct ModelCatalog {
    pub models: Vec<String>,
    pub status: String,
    pub loading: bool,
    /// Endpoint, key environment variable name and modality. Contains no credential.
    pub fingerprint: String,
    signature: Option<u64>,
    generation: u64,
    pending: Option<PendingCatalog>,
    tx: mpsc::Sender<CatalogResult>,
    rx: Mutex<mpsc::Receiver<CatalogResult>>,
}
impl Default for ModelCatalog {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            models: Vec::new(),
            status: "Choose a connection to load models".into(),
            loading: false,
            fingerprint: String::new(),
            signature: None,
            generation: 0,
            pending: None,
            tx,
            rx: Mutex::new(rx),
        }
    }
}
impl ModelCatalog {
    /// Safe to call each frame. Failures remain until configuration changes or Retry.
    pub fn sync(&mut self, endpoint: &str, key_env: &str, session_key: &str, kind: CatalogKind) {
        self.poll();
        let mut hash = DefaultHasher::new();
        (endpoint, key_env, session_key, kind).hash(&mut hash);
        if session_key.trim().is_empty() {
            // Observe credential rotation without retaining the environment value.
            resolve_key(key_env.trim(), "").hash(&mut hash);
        }
        let signature = hash.finish();
        if self.signature != Some(signature) {
            let first = self.signature.is_none();
            self.signature = Some(signature);
            self.pending = None;
            let fish =
                catalog_url(endpoint).is_ok_and(|url| url.host_str() == Some("api.fish.audio"));
            if first || fish {
                self.start(endpoint, key_env, session_key, kind);
            } else {
                // Editing a URL or key field should not create a request per keystroke.
                self.generation = self.generation.wrapping_add(1);
                self.models.clear();
                self.loading = true;
                self.status = "Waiting for provider settings…".into();
                self.pending = Some(PendingCatalog {
                    endpoint: endpoint.to_owned(),
                    key_env: key_env.to_owned(),
                    session_key: session_key.to_owned(),
                    kind,
                    due: Instant::now() + Duration::from_millis(400),
                });
            }
        }
    }

    pub fn retry(&mut self, endpoint: &str, key_env: &str, session_key: &str, kind: CatalogKind) {
        if self.loading {
            return;
        }
        self.signature = None;
        self.sync(endpoint, key_env, session_key, kind);
    }

    pub fn poll(&mut self) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| Instant::now() >= pending.due)
        {
            let pending = self.pending.take().expect("checked pending catalog");
            self.start(
                &pending.endpoint,
                &pending.key_env,
                &pending.session_key,
                pending.kind,
            );
        }
        let Ok(rx) = self.rx.lock() else { return };
        while let Ok((generation, result)) = rx.try_recv() {
            if generation != self.generation {
                continue;
            }
            self.loading = false;
            match result {
                Ok(models) => {
                    self.status = if models.is_empty() {
                        "No models listed; use an exact model ID".into()
                    } else {
                        format!("{} models available", models.len())
                    };
                    self.models = models;
                }
                Err(message) => self.status = message,
            }
        }
    }

    fn start(&mut self, endpoint: &str, key_env: &str, session_key: &str, kind: CatalogKind) {
        self.generation = self.generation.wrapping_add(1);
        self.fingerprint = format!("{kind:?}|invalid endpoint");
        self.models.clear();
        self.loading = false;
        let endpoint = match catalog_url(endpoint) {
            Ok(url) => url,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        self.fingerprint = format!("{kind:?}|{endpoint}|{}", key_env.trim());
        if endpoint.host_str() == Some("api.fish.audio") {
            self.models = match kind {
                CatalogKind::Speech => FISH_SPEECH_MODELS.as_slice(),
                CatalogKind::Transcription => FISH_ASR_MODELS.as_slice(),
                CatalogKind::Decision => &[],
            }
            .iter()
            .map(|s| (*s).into())
            .collect();
            self.status = "Fish models · documented list".into();
            return;
        }
        let key_env = key_env.trim().to_owned();
        let session_key = session_key.trim().to_owned();
        let generation = self.generation;
        let tx = self.tx.clone();
        let Some(permit) = Worker::acquire() else {
            self.status = "Model discovery busy · retry shortly".into();
            return;
        };
        self.loading = true;
        self.status = "Loading models…".into();
        if std::thread::Builder::new()
            .name("speech-catalog".into())
            .spawn(move || {
                let _permit = permit;
                let key = resolve_key(&key_env, &session_key);
                let _ = tx.send((generation, fetch_models(endpoint, &key, kind)));
            })
            .is_err()
        {
            self.loading = false;
            self.status = "Could not start model discovery; retry".into();
        }
    }
}

fn resolve_key(key_env: &str, session_key: &str) -> String {
    if !session_key.trim().is_empty() {
        return session_key.trim().into();
    }
    std::env::var(key_env)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            (key_env == "AI_GATEWAY_API_KEY")
                .then(|| std::env::var("VERCEL_OIDC_TOKEN").ok())
                .flatten()
        })
        .unwrap_or_default()
}

fn catalog_url(endpoint: &str) -> Result<Url, String> {
    let mut url = Url::parse(endpoint.trim()).map_err(|_| "Enter a valid speech endpoint URL")?;
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if !(url.scheme() == "https" || (url.scheme() == "http" && local))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Models require HTTPS (or localhost HTTP), without URL credentials or query".into(),
        );
    }
    let path = url.path().trim_end_matches('/');
    let models = if path.ends_with("/models") {
        path.to_owned()
    } else if path == "/v4/ai/transcription-model"
        || path == "/v4/ai/speech-model"
        || path == "/v4/ai/evaluation-model"
    {
        "/v1/models".into()
    } else if let Some(base) = path
        .strip_suffix("/audio/speech")
        .or_else(|| path.strip_suffix("/audio/transcriptions"))
        .or_else(|| path.strip_suffix("/tts"))
        .or_else(|| path.strip_suffix("/asr"))
    {
        format!("{base}/models")
    } else {
        format!("{path}/models")
    };
    url.set_path(&models);
    Ok(url)
}

fn fetch_models(url: Url, key: &str, kind: CatalogKind) -> Result<Vec<String>, String> {
    let host = url.host_str().unwrap_or_default().to_owned();
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "Could not create model discovery client")?;
    let mut request = client.get(url);
    if !key.trim().is_empty() {
        request = request.bearer_auth(key.trim());
    }
    let response = request
        .send()
        .map_err(|_| "Could not reach model catalog; retry")?;
    if !response.status().is_success() {
        return Err(format!(
            "Model catalog returned HTTP {}; retry or check credentials",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read model catalog")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Model catalog exceeds size limit".into());
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Model catalog returned invalid JSON")?;
    model_ids(&value, &host, kind)
}

fn contains_modality(model: &Value, pointer: &str, modality: &str) -> bool {
    model
        .pointer(pointer)
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(modality)))
}

fn model_ids(value: &Value, host: &str, kind: CatalogKind) -> Result<Vec<String>, String> {
    let rows = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .ok_or("Expected a model list in catalog")?;
    if rows.len() > 10_000 {
        return Err("Model catalog exceeds 10,000 entries".into());
    }
    let mut ids = Vec::new();
    for model in rows {
        let Some(id) = model.get("id").and_then(Value::as_str) else {
            continue;
        };
        if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
            continue;
        }
        let declared = model.get("type").and_then(Value::as_str);
        let lower = id.to_ascii_lowercase();
        let transcription_name =
            lower.contains("transcrib") || lower.contains("whisper") || lower.ends_with("/asr");
        let speech_name = lower.contains("tts")
            || lower.contains("speech")
            || lower.contains("kokoro")
            || lower.contains("orpheus")
            || lower.contains("sonic")
            || lower.contains("dia-");
        let matches = match kind {
            CatalogKind::Speech => {
                declared == Some("speech")
                    || declared == Some("tts")
                    || contains_modality(model, "/architecture/output_modalities", "audio")
                    || (host != "ai-gateway.vercel.sh" && speech_name && !transcription_name)
            }
            CatalogKind::Transcription => {
                declared == Some("transcription")
                    || declared == Some("asr")
                    || (contains_modality(model, "/architecture/input_modalities", "audio")
                        && contains_modality(model, "/architecture/output_modalities", "text")
                        && !contains_modality(model, "/architecture/output_modalities", "audio"))
                    || (host != "ai-gateway.vercel.sh" && transcription_name)
            }
            CatalogKind::Decision => declared == Some("evaluation"),
        };
        if matches {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{io::Write, net::TcpListener};

    #[test]
    fn endpoints_derive_catalog_without_losing_provider_base() {
        assert_eq!(
            catalog_url("https://openrouter.ai/api/v1/audio/speech")
                .unwrap()
                .as_str(),
            "https://openrouter.ai/api/v1/models"
        );
        assert_eq!(
            catalog_url("https://ai-gateway.vercel.sh/v4/ai/transcription-model")
                .unwrap()
                .as_str(),
            "https://ai-gateway.vercel.sh/v1/models"
        );
        assert!(catalog_url("http://example.com/audio/speech").is_err());
        assert!(catalog_url("https://key@example.com/v1/audio/speech").is_err());
        assert!(catalog_url("https://example.com/v1/audio/speech?key=hidden").is_err());
        assert!(catalog_url("http://[::1]:8080/v1/audio/speech").is_ok());
    }

    #[test]
    fn mixed_catalogs_only_advertise_relevant_models() {
        let value = json!({"data":[
            {"id":"openai/gpt-4o", "type":"language"},
            {"id":"openai/gpt-4o-mini-tts", "type":"speech"},
            {"id":"fish-audio/transcribe-1", "type":"transcription"},
            {"id":"embedding", "type":"embedding"}
        ]});
        assert_eq!(
            model_ids(&value, "ai-gateway.vercel.sh", CatalogKind::Speech).unwrap(),
            ["openai/gpt-4o-mini-tts"]
        );
        assert_eq!(
            model_ids(&value, "ai-gateway.vercel.sh", CatalogKind::Transcription).unwrap(),
            ["fish-audio/transcribe-1"]
        );
        let router = json!({"data":[
            {"id":"vendor/voice", "architecture":{"input_modalities":["text"], "output_modalities":["audio"]}},
            {"id":"vendor/listen", "architecture":{"input_modalities":["audio"], "output_modalities":["text"]}},
            {"id":"vendor/chat", "architecture":{"output_modalities":["text"]}}
        ]});
        assert_eq!(
            model_ids(&router, "openrouter.ai", CatalogKind::Speech).unwrap(),
            ["vendor/voice"]
        );
        assert_eq!(
            model_ids(&router, "openrouter.ai", CatalogKind::Transcription).unwrap(),
            ["vendor/listen"]
        );
        let gateway = json!({"data":[
            {"id":"typesafe-ai/jev", "type":"evaluation"},
            {"id":"microsoft/microsoft-decision-1", "type":"evaluation"},
            {"id":"openai/gpt-6-luna", "type":"language"}
        ]});
        assert_eq!(
            model_ids(&gateway, "ai-gateway.vercel.sh", CatalogKind::Decision).unwrap(),
            ["microsoft/microsoft-decision-1", "typesafe-ai/jev"]
        );
        assert_eq!(
            catalog_url("https://ai-gateway.vercel.sh/v4/ai/evaluation-model")
                .unwrap()
                .as_str(),
            "https://ai-gateway.vercel.sh/v1/models"
        );
    }

    #[test]
    fn fish_models_are_immediate_and_no_network_is_started() {
        let mut catalog = ModelCatalog::default();
        catalog.sync(
            "https://api.fish.audio/v1/tts",
            "FISH_API_KEY",
            "",
            CatalogKind::Speech,
        );
        assert_eq!(catalog.models, FISH_SPEECH_MODELS);
        assert!(!catalog.loading);
        catalog.sync(
            "https://api.fish.audio/v1/asr",
            "FISH_API_KEY",
            "",
            CatalogKind::Transcription,
        );
        assert_eq!(catalog.models, FISH_ASR_MODELS);
    }

    #[test]
    fn edited_endpoints_are_debounced_and_only_latest_is_kept() {
        let mut catalog = ModelCatalog::default();
        catalog.sync("invalid", "UNUSED", "", CatalogKind::Speech);
        catalog.sync(
            "http://127.0.0.1:9/v1/audio/speech",
            "UNUSED",
            "",
            CatalogKind::Speech,
        );
        assert!(catalog.loading);
        assert!(catalog.pending.is_some());
        catalog.sync(
            "http://127.0.0.1:8/v1/audio/speech",
            "UNUSED",
            "",
            CatalogKind::Speech,
        );
        assert!(catalog.pending.as_ref().unwrap().endpoint.contains(":8/"));
        catalog.sync(
            "https://api.fish.audio/v1/tts",
            "FISH_API_KEY",
            "",
            CatalogKind::Speech,
        );
        assert!(catalog.pending.is_none());
        assert!(!catalog.loading);
        assert_eq!(catalog.models, FISH_SPEECH_MODELS);
    }

    fn fake_server(
        body: &'static str,
        status: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/audio/speech", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            String::from_utf8(request).unwrap()
        });
        (endpoint, handle)
    }

    fn complete(catalog: &mut ModelCatalog) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while catalog.loading && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            catalog.poll();
        }
        assert!(!catalog.loading);
    }

    #[test]
    fn discovery_uses_model_route_and_session_auth() {
        let (endpoint, server) = fake_server(
            r#"{"data":[{"id":"tts-1"},{"id":"whisper-1"},{"id":"chat"},{"id":"tts-1"}]}"#,
            "200 OK",
        );
        let mut catalog = ModelCatalog::default();
        catalog.sync(&endpoint, "UNUSED", "fake-test-key", CatalogKind::Speech);
        complete(&mut catalog);
        assert_eq!(catalog.models, ["tts-1"]);
        let request = server.join().unwrap().to_lowercase();
        assert!(request.starts_with("get /v1/models "));
        assert!(request.contains("authorization: bearer fake-test-key"));
        assert!(!catalog.fingerprint.contains("fake-test-key"));
    }

    #[test]
    fn failure_is_not_retried_each_frame_or_exposed() {
        let (endpoint, server) = fake_server("private-response-body", "401 Unauthorized");
        let mut catalog = ModelCatalog::default();
        catalog.sync(&endpoint, "UNUSED", "fake-test-key", CatalogKind::Speech);
        complete(&mut catalog);
        server.join().unwrap();
        for _ in 0..100 {
            catalog.sync(&endpoint, "UNUSED", "fake-test-key", CatalogKind::Speech);
        }
        assert!(!catalog.loading);
        assert!(catalog.status.contains("401"));
        assert!(!catalog.status.contains("private-response-body"));
        assert!(!catalog.status.contains("fake-test-key"));
    }
}
