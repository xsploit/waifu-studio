//! Read-only Fish voice library search (`GET /model`). Picks a TTS reference ID;
//! never clones, deletes, or saves voices, and never persists API keys.
use crate::voice::VoiceConfig;
use bevy::prelude::Resource;
use serde::Deserialize;
use std::{
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
const MAX_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PAGE_SIZE: u8 = 100;
pub const MAX_PAGE_NUMBER: u32 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    #[default]
    Score,
    TaskCount,
    CreatedAt,
}
impl SortBy {
    pub const ALL: [Self; 3] = [Self::Score, Self::TaskCount, Self::CreatedAt];
    pub fn wire(self) -> &'static str {
        match self {
            Self::Score => "score",
            Self::TaskCount => "task_count",
            Self::CreatedAt => "created_at",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Score => "Relevance",
            Self::TaskCount => "Most used",
            Self::CreatedAt => "Newest",
        }
    }
}

/// Search filters mapped 1:1 to documented `GET /model` query parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryQuery {
    /// `self=true`: only voices owned by the active workspace.
    pub own_only: bool,
    pub title: String,
    pub tag: String,
    pub language: String,
    pub sort_by: SortBy,
    pub page_number: u32,
    pub page_size: u8,
}
impl Default for LibraryQuery {
    fn default() -> Self {
        Self {
            own_only: false,
            title: String::new(),
            tag: String::new(),
            language: String::new(),
            sort_by: SortBy::Score,
            page_number: 1,
            page_size: 20,
        }
    }
}
impl LibraryQuery {
    fn validate(&self) -> Result<(), String> {
        for (value, max, name) in [
            (&self.title, 128, "Title"),
            (&self.tag, 64, "Tag"),
            (&self.language, 16, "Language"),
        ] {
            if value.chars().count() > max || value.chars().any(char::is_control) {
                return Err(format!(
                    "{name} filter must be at most {max} plain characters"
                ));
            }
        }
        if !(1..=MAX_PAGE_SIZE).contains(&self.page_size)
            || !(1..=MAX_PAGE_NUMBER).contains(&self.page_number)
        {
            return Err(format!(
                "Use page size 1–{MAX_PAGE_SIZE} and page 1–{MAX_PAGE_NUMBER}"
            ));
        }
        Ok(())
    }
    fn url(&self, endpoint: &str) -> Result<reqwest::Url, String> {
        self.validate()?;
        validate_endpoint(endpoint)?;
        let mut url = reqwest::Url::parse(endpoint).map_err(|_| "Invalid voice library URL")?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("page_size", &self.page_size.to_string())
                .append_pair("page_number", &self.page_number.to_string())
                .append_pair("sort_by", self.sort_by.wire());
            if self.own_only {
                pairs.append_pair("self", "true");
            }
            for (name, value) in [
                ("title", &self.title),
                ("tag", &self.tag),
                ("language", &self.language),
            ] {
                if !value.trim().is_empty() {
                    pairs.append_pair(name, value.trim());
                }
            }
        }
        Ok(url)
    }
}

/// Sanitized, bounded display metadata for one usable TTS voice.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryVoice {
    pub id: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub languages: Vec<String>,
    pub author: String,
    pub visibility: String,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryPage {
    pub voices: Vec<LibraryVoice>,
    pub total: u64,
    /// Items dropped because they were not trained TTS models or had invalid IDs.
    pub skipped: usize,
}

#[derive(Deserialize)]
struct WirePage {
    total: u64,
    items: Vec<WireModel>,
}
#[derive(Deserialize)]
struct WireModel {
    #[serde(rename = "_id")]
    id: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    author: Option<WireAuthor>,
}
#[derive(Deserialize)]
struct WireAuthor {
    #[serde(default)]
    nickname: String,
}

type Message = (u64, Result<LibraryPage, String>);
struct Pending {
    generation: u64,
    receiver: Mutex<mpsc::Receiver<Message>>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Resource)]
pub struct VoiceLibraryState {
    /// Fish model listing endpoint; HTTPS, or HTTP on loopback for testing.
    pub endpoint: String,
    pub query: LibraryQuery,
    pub page: LibraryPage,
    pub status: String,
    generation: u64,
    pending: Option<Pending>,
    selected: Option<String>,
}
impl Default for VoiceLibraryState {
    fn default() -> Self {
        Self {
            endpoint: "https://api.fish.audio/model".into(),
            query: LibraryQuery::default(),
            page: LibraryPage::default(),
            status: "Search the Fish voice library to pick a reference voice".into(),
            generation: 0,
            pending: None,
            selected: None,
        }
    }
}
impl VoiceLibraryState {
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }
    /// Explicit Search action only. The key is used for this request and never stored.
    pub fn fetch(&mut self, config: &VoiceConfig, session_key: Option<&str>) -> Result<(), String> {
        if self.is_busy() {
            return Err("Wait for the current search to finish or cancel it".into());
        }
        let url = self.query.url(&self.endpoint)?;
        let env_key = (!config.api_key_env.trim().is_empty())
            .then(|| std::env::var(config.api_key_env.trim()).ok())
            .flatten();
        let key = session_key
            .filter(|key| !key.trim().is_empty())
            .map(str::to_owned)
            .or(env_key)
            .unwrap_or_default();
        if key.trim().is_empty() || key.len() > 8192 || key.chars().any(char::is_control) {
            return Err(
                "Provide a valid session key or configured key environment variable".into(),
            );
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let limit = usize::from(self.query.page_size);
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let (tx, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("fish-voice-library".into())
            .spawn(move || {
                let _ = tx.send((generation, execute(url, &key, limit, &flag)));
            })
            .map_err(|_| "Cannot start voice library worker")?;
        self.pending = Some(Pending {
            generation,
            receiver: Mutex::new(receiver),
            cancelled,
        });
        self.status = "Searching voice library…".into();
        Ok(())
    }
    /// Marks the in-flight search stale; its result is discarded when the worker ends.
    pub fn cancel(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancelled.store(true, Ordering::SeqCst);
            self.generation = self.generation.wrapping_add(1);
            self.status = "Search cancelled; waiting for bounded request to stop".into();
        }
    }
    /// Non-blocking; safe to call every frame.
    pub fn poll(&mut self) {
        let Some(pending) = &self.pending else {
            return;
        };
        let (generation, result) = match pending.receiver.lock().unwrap().try_recv() {
            Ok(message) => message,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(_) => (
                pending.generation,
                Err("Voice library worker stopped".into()),
            ),
        };
        let stale = pending.cancelled.load(Ordering::SeqCst)
            || generation != pending.generation
            || generation != self.generation;
        self.pending = None;
        if stale {
            self.status = "Search cancelled; stale result discarded".into();
            return;
        }
        match result {
            Ok(page) => {
                self.status = format!(
                    "Showing {} of {} voices{}",
                    page.voices.len(),
                    page.total,
                    if page.skipped > 0 {
                        format!(" ({} unusable entries hidden)", page.skipped)
                    } else {
                        String::new()
                    }
                );
                self.page = page;
            }
            Err(error) => self.status = error,
        }
    }
    /// Explicit user choice of a listed voice; only IDs from the current results are accepted.
    pub fn select(&mut self, id: &str) -> bool {
        let found = self.page.voices.iter().any(|voice| voice.id == id);
        if found {
            self.selected = Some(id.to_owned());
        }
        found
    }
    /// Root applies this to `VoiceConfig::reference_id`.
    pub fn take_selected(&mut self) -> Option<String> {
        self.selected.take()
    }
}
impl Drop for VoiceLibraryState {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancelled.store(true, Ordering::SeqCst);
        }
    }
}

fn validate_endpoint(value: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid voice library URL")?;
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.path().ends_with("/model")
    {
        return Err(
            "Use HTTPS (or local HTTP) ending in /model, without URL credentials/query".into(),
        );
    }
    Ok(())
}

fn execute(
    url: reqwest::Url,
    key: &str,
    limit: usize,
    cancelled: &AtomicBool,
) -> Result<LibraryPage, String> {
    if cancelled.load(Ordering::SeqCst) {
        return Err("Search cancelled".into());
    }
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "Cannot initialize voice library client")?;
    // Never include remote response text, credentials, or URL in an error.
    let mut response = client
        .get(url)
        .bearer_auth(key)
        .send()
        .map_err(|_| "Voice library request failed or timed out")?;
    if !response.status().is_success() {
        return Err(format!(
            "Voice library returned HTTP {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err("Voice library response exceeds 4 MiB".into());
    }
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err("Search cancelled".into());
        }
        let n = response
            .read(&mut chunk)
            .map_err(|_| "Cannot read voice library response")?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > MAX_BYTES {
            return Err("Voice library response exceeds 4 MiB".into());
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    decode(&bytes, limit)
}

fn decode(bytes: &[u8], limit: usize) -> Result<LibraryPage, String> {
    let wire: WirePage =
        serde_json::from_slice(bytes).map_err(|_| "Invalid voice library response schema")?;
    let mut page = LibraryPage {
        total: wire.total,
        ..LibraryPage::default()
    };
    for model in wire.items {
        let usable = model.kind == "tts"
            && model.state == "trained"
            && valid_id(&model.id)
            && !page.voices.iter().any(|v| v.id == model.id);
        if !usable || page.voices.len() >= limit.min(usize::from(MAX_PAGE_SIZE)) {
            page.skipped += 1;
            continue;
        }
        let list = |values: Vec<String>| {
            values
                .iter()
                .map(|v| clean(v, 32))
                .filter(|v| !v.is_empty())
                .take(12)
                .collect()
        };
        page.voices.push(LibraryVoice {
            title: match clean(&model.title, 120) {
                title if title.is_empty() => model.id.clone(),
                title => title,
            },
            description: clean(&model.description, 300),
            tags: list(model.tags),
            languages: list(model.languages),
            author: model
                .author
                .map(|a| clean(&a.nickname, 64))
                .unwrap_or_default(),
            visibility: clean(&model.visibility, 16),
            id: model.id,
        });
    }
    Ok(page)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Strips control and bidi-override characters and bounds length for display.
fn clean(value: &str, max: usize) -> String {
    let text: String = value
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| {
            !c.is_control() && !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > max {
        text.chars().take(max - 1).chain(['…']).collect()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, net::TcpListener, time::Instant};

    const BODY: &str = r#"{"total":42,"items":[
        {"_id":"abc123","type":"tts","state":"trained","title":"Warm\u202e voice\n","tags":["calm"],"languages":["en"],"visibility":"public","author":{"nickname":"amy"}},
        {"_id":"bad id","type":"tts","state":"trained","title":"x"},
        {"_id":"svc1","type":"svc","state":"trained","title":"y"},
        {"_id":"t2","type":"tts","state":"training","title":"z"}]}"#;

    /// Serves one canned response and returns the request head it received.
    fn serve(status: &str, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/model", listener.local_addr().unwrap());
        let status = status.to_owned();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 1024];
            while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            String::from_utf8_lossy(&bytes).to_lowercase()
        });
        (endpoint, server)
    }

    fn config() -> VoiceConfig {
        VoiceConfig {
            api_key_env: String::new(),
            ..VoiceConfig::default()
        }
    }

    fn wait(state: &mut VoiceLibraryState) {
        let start = Instant::now();
        while state.is_busy() {
            assert!(start.elapsed() < Duration::from_secs(10));
            state.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn query_and_endpoint_validation() {
        assert!(validate_endpoint("http://example.com/model").is_err());
        assert!(validate_endpoint("https://u:p@api.fish.audio/model").is_err());
        assert!(validate_endpoint("https://api.fish.audio/model?x=1").is_err());
        assert!(validate_endpoint("https://api.fish.audio/v1/tts").is_err());
        assert!(validate_endpoint("https://api.fish.audio/model").is_ok());
        let mut query = LibraryQuery {
            page_size: 101,
            ..LibraryQuery::default()
        };
        assert!(query.validate().is_err());
        query.page_size = 10;
        query.title = "a\nb".into();
        assert!(query.validate().is_err());
        assert!(decode(b"{}", 10).is_err());
        assert_eq!(clean("a\u{202e}b\u{7}  c", 10), "ab c");
        assert_eq!(clean("abcdef", 4), "abc…");
    }

    #[test]
    fn fake_http_search_contract_and_selection() {
        let (endpoint, server) = serve("200 OK", BODY);
        let mut state = VoiceLibraryState::default();
        state.endpoint = endpoint;
        state.query = LibraryQuery {
            own_only: true,
            title: "warm voice".into(),
            tag: "calm".into(),
            language: "en".into(),
            sort_by: SortBy::CreatedAt,
            page_number: 2,
            page_size: 5,
        };
        state.fetch(&config(), Some("fake-test-key")).unwrap();
        assert!(state.fetch(&config(), Some("fake-test-key")).is_err());
        wait(&mut state);
        let head = server.join().unwrap();
        let line = head.lines().next().unwrap();
        assert!(line.starts_with("get /model?"));
        for part in [
            "page_size=5",
            "page_number=2",
            "sort_by=created_at",
            "self=true",
            "title=warm+voice",
            "tag=calm",
            "language=en",
        ] {
            assert!(line.contains(part), "{line}");
        }
        assert!(head.contains("authorization: bearer fake-test-key"));
        assert_eq!(state.page.total, 42);
        assert_eq!(state.page.skipped, 3);
        let voice = &state.page.voices[0];
        assert_eq!(
            (voice.id.as_str(), voice.title.as_str()),
            ("abc123", "Warm voice")
        );
        assert_eq!(voice.author, "amy");
        assert!(state.take_selected().is_none());
        assert!(!state.select("not-listed"));
        assert!(state.select("abc123"));
        assert_eq!(state.take_selected().as_deref(), Some("abc123"));
        assert!(state.take_selected().is_none());
    }

    #[test]
    fn errors_redirects_and_stale_results_are_rejected() {
        let mut state = VoiceLibraryState::default();
        assert!(state.fetch(&config(), None).is_err());

        let (endpoint, server) = serve("401 Unauthorized", "secret detail");
        state.endpoint = endpoint;
        state.fetch(&config(), Some("fake-test-key")).unwrap();
        wait(&mut state);
        server.join().unwrap();
        assert!(state.status.contains("401") && !state.status.contains("secret"));

        let (endpoint, server) = serve("302 Found\r\nLocation: http://127.0.0.1:1/model", "");
        state.endpoint = endpoint;
        state.fetch(&config(), Some("fake-test-key")).unwrap();
        wait(&mut state);
        server.join().unwrap();
        assert!(state.status.contains("302"));

        let (endpoint, server) = serve("200 OK", BODY);
        state.endpoint = endpoint;
        state.fetch(&config(), Some("fake-test-key")).unwrap();
        state.cancel();
        wait(&mut state);
        drop(server); // worker may stop before connecting
        assert!(state.page.voices.is_empty());
        assert!(state.status.contains("stale"));
    }
}
