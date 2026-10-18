//! OpenAI-compatible gateways, executed off the render thread.
use crate::attachments::{ImageAttachment, MAX_ATTACHMENTS, MAX_DATA_URL_BYTES};
use crate::data::Turn;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::Sender,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}
impl ReasoningEffort {
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Minimal => "Minimal",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::XHigh => "Extra high",
            Self::Max => "Maximum",
        }
    }
}

/// WebWaifu's two pinned LLM backends (src/lib/chat/types.ts `LlmProvider`).
/// Unknown provider names normalize to the Vercel gateway, as upstream does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatewayRoute {
    #[serde(rename = "vercel-gateway")]
    VercelGateway,
    #[serde(rename = "openrouter-responses")]
    OpenRouterResponses,
}
impl GatewayRoute {
    pub const ALL: [Self; 2] = [Self::VercelGateway, Self::OpenRouterResponses];
    pub fn normalize(value: &str) -> Self {
        if value == "openrouter-responses" {
            Self::OpenRouterResponses
        } else {
            Self::VercelGateway
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::VercelGateway => "Vercel AI Gateway",
            Self::OpenRouterResponses => "OpenRouter",
        }
    }
    pub fn base_url(self) -> &'static str {
        match self {
            Self::VercelGateway => "https://ai-gateway.vercel.sh/v1",
            Self::OpenRouterResponses => "https://openrouter.ai/api/v1",
        }
    }
    /// Primary key variable plus the upstream fallback (VERCEL_OIDC_TOKEN).
    pub fn key_envs(self) -> &'static [&'static str] {
        match self {
            Self::VercelGateway => &["AI_GATEWAY_API_KEY", "VERCEL_OIDC_TOKEN"],
            Self::OpenRouterResponses => &["OPENROUTER_API_KEY"],
        }
    }
    pub fn default_model(self) -> &'static str {
        match self {
            Self::VercelGateway => "openai/gpt-5-nano",
            Self::OpenRouterResponses => "openai/gpt-4o-mini",
        }
    }
    /// Preserve explicit routed model IDs, including premium tiers and routing suffixes.
    pub fn pin_model(self, model: &str) -> String {
        let model = model.trim();
        if model.is_empty() || !model.contains('/') {
            self.default_model().into()
        } else {
            model.into()
        }
    }
}

/// GPT-5 sampling support varies by version; use server defaults on pinned routes.
fn is_openai_gpt5(model: &str) -> bool {
    let normalized = model.trim().to_lowercase();
    let vendor = normalized
        .split_once('/')
        .map_or("openai", |(vendor, _)| vendor);
    let leaf = normalized
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .replace(['_', ' ', '.'], "-");
    vendor == "openai" && leaf.starts_with("gpt-5")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingSort {
    #[default]
    Default,
    Cost,
    Latency,
    Throughput,
}
impl RoutingSort {
    pub const ALL: [Self; 4] = [Self::Default, Self::Cost, Self::Latency, Self::Throughput];
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Gateway default",
            Self::Cost => "Lowest cost",
            Self::Latency => "Lowest first-token latency",
            Self::Throughput => "Highest token throughput",
        }
    }
}

/// Exact upstream provider slugs; gateway-specific wire names are applied at send time.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderRouting {
    pub order: Vec<String>,
    pub only: Vec<String>,
    pub sort: RoutingSort,
    /// OpenRouter only. Vercel restrictions are expressed by the explicit only list.
    pub allow_fallbacks: bool,
}
impl Default for ProviderRouting {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            only: Vec::new(),
            sort: RoutingSort::Default,
            allow_fallbacks: true,
        }
    }
}
impl ProviderRouting {
    fn body(&self, route: GatewayRoute) -> Value {
        let mut body = json!({});
        let order: Vec<_> = self
            .order
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        let only: Vec<_> = self
            .only
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !order.is_empty() {
            body["order"] = json!(order);
        }
        if !only.is_empty() {
            body["only"] = json!(only);
        }
        let sort = match (route, self.sort) {
            (_, RoutingSort::Default) => None,
            (GatewayRoute::VercelGateway, RoutingSort::Cost) => Some("cost"),
            (GatewayRoute::VercelGateway, RoutingSort::Latency) => Some("ttft"),
            (GatewayRoute::VercelGateway, RoutingSort::Throughput) => Some("tps"),
            (_, RoutingSort::Cost) => Some("price"),
            (_, RoutingSort::Latency) => Some("latency"),
            (_, RoutingSort::Throughput) => Some("throughput"),
        };
        if let Some(sort) = sort {
            body["sort"] = json!(sort);
        }
        if route == GatewayRoute::OpenRouterResponses && !self.allow_fallbacks {
            body["allow_fallbacks"] = json!(false);
        }
        body
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub key_env: String,
    pub temperature: f32,
    /// None delegates output length and sampling to the server.
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Pins base URL, key variables and model rules to a WebWaifu backend; None = generic.
    pub route: Option<GatewayRoute>,
    pub routing: ProviderRouting,
    /// Environment variable holding an OpenAI BYOK key for the Vercel gateway (name only).
    pub byok_openai_key_env: String,
    #[serde(skip)]
    pub session_key: String,
}
impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("name", &self.name)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}
impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: "Local / custom".into(),
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: String::new(),
            key_env: String::new(),
            temperature: 0.8,
            max_tokens: None,
            top_p: None,
            reasoning_effort: None,
            route: None,
            routing: ProviderRouting::default(),
            byok_openai_key_env: String::new(),
            session_key: String::new(),
        }
    }
}
impl ProviderConfig {
    pub fn presets() -> Vec<Self> {
        vec![
            Self::pinned(GatewayRoute::VercelGateway),
            Self::pinned(GatewayRoute::OpenRouterResponses),
            Self::default(),
        ]
    }
    pub fn pinned(route: GatewayRoute) -> Self {
        let mut config = Self {
            name: route.label().into(),
            ..Self::default()
        };
        config.set_route(Some(route));
        config
    }
    /// Upstream `applyLlmProviderSwitchDefaults`: switching backends resets to its default model.
    pub fn set_route(&mut self, route: Option<GatewayRoute>) {
        if self.route == route {
            return;
        }
        self.route = route;
        // Provider slugs and fallback semantics belong to the selected gateway.
        self.routing = ProviderRouting::default();
        if let Some(route) = route {
            self.base_url = route.base_url().into();
            self.key_env = route.key_envs()[0].into();
            self.model = route.default_model().into();
        }
    }
    /// Model actually sent; empty or unrouted IDs use the gateway default.
    pub fn effective_model(&self) -> String {
        self.route.map_or_else(
            || self.model.trim().to_owned(),
            |route| route.pin_model(&self.model),
        )
    }
    /// All presets speak OpenAI-compatible chat completions; no native Anthropic/Gemini adapter.
    pub fn catalog() -> Vec<Self> {
        let mut presets = Self::presets();
        for (name, base_url, key_env) in [
            ("OpenAI", "https://api.openai.com/v1", "OPENAI_API_KEY"),
            ("xAI / Grok", "https://api.x.ai/v1", "XAI_API_KEY"),
            (
                "Anthropic via OpenRouter",
                "https://openrouter.ai/api/v1",
                "OPENROUTER_API_KEY",
            ),
            (
                "Gemini via OpenRouter",
                "https://openrouter.ai/api/v1",
                "OPENROUTER_API_KEY",
            ),
            ("Ollama", "http://127.0.0.1:11434/v1", ""),
            ("LM Studio (enter server URL)", "", ""),
            ("llama.cpp", "http://127.0.0.1:8080/v1", ""),
            ("Custom OpenAI-compatible", "", ""),
        ] {
            presets.push(Self {
                name: name.into(),
                base_url: base_url.into(),
                key_env: key_env.into(),
                ..Self::default()
            });
        }
        presets
    }
    /// Some GPT-5 variants reject sampling controls; defer to gateway defaults.
    pub fn uses_server_sampling(&self) -> bool {
        self.route.is_some() && is_openai_gpt5(&self.effective_model())
    }
    fn chat_body(&self, turns: &[Turn]) -> Value {
        let model = self.effective_model();
        let omit_sampling = self.uses_server_sampling();
        let mut body = json!({
            "model": model, "messages": turns, "stream": true,
        });
        if !omit_sampling {
            body["temperature"] = json!(if self.temperature.is_finite() {
                self.temperature.clamp(0.0, 2.0)
            } else {
                0.8
            });
        }
        if let Some(tokens) = self.max_tokens {
            body["max_tokens"] = json!(tokens.clamp(1, 1_000_000));
        }
        if let Some(top_p) = self.top_p.filter(|p| p.is_finite() && !omit_sampling) {
            body["top_p"] = json!(top_p.clamp(0.0, 1.0));
        }
        let effort = self.reasoning_effort.or_else(|| {
            (self.route == Some(GatewayRoute::VercelGateway)
                && matches!(
                    model.as_str(),
                    "openai/gpt-5" | "openai/gpt-5-mini" | "openai/gpt-5-nano"
                ))
            .then_some(ReasoningEffort::Minimal)
        });
        if let Some(effort) = effort {
            body["reasoning_effort"] = json!(effort);
        }
        if let Some(route) = self.route {
            let mut routing = self.routing.body(route);
            if route == GatewayRoute::VercelGateway {
                let byok = env_value(&self.byok_openai_key_env);
                if !byok.is_empty() {
                    routing["byok"] = json!({"openai": [{"apiKey": byok}]});
                }
                if routing.as_object().is_some_and(|m| !m.is_empty()) {
                    body["providerOptions"] = json!({"gateway": routing});
                }
            } else if routing.as_object().is_some_and(|m| !m.is_empty()) {
                body["provider"] = routing;
            }
        }
        body
    }
    fn key(&self) -> String {
        if !self.session_key.is_empty() {
            return self.session_key.clone();
        }
        let key = env_value(&self.key_env);
        match self.route {
            Some(route) if key.is_empty() && route.key_envs().contains(&self.key_env.as_str()) => {
                route
                    .key_envs()
                    .iter()
                    .copied()
                    .map(env_value)
                    .find(|value| !value.is_empty())
                    .unwrap_or_default()
            }
            _ => key,
        }
    }
    /// Secrets that must never appear in surfaced errors.
    fn secrets(&self) -> Vec<String> {
        let mut secrets = vec![self.key()];
        if self.route == Some(GatewayRoute::VercelGateway) {
            secrets.push(env_value(&self.byok_openai_key_env));
        }
        secrets
    }
    pub fn has_key(&self) -> bool {
        !self.key().is_empty()
    }
    /// In-memory identity for discovery; secrets are never stored in UI state.
    pub fn catalog_identity(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.endpoint("models")
            .ok()
            .map(|url| url.to_string())
            .hash(&mut hash);
        self.route.map(GatewayRoute::label).hash(&mut hash);
        self.key().hash(&mut hash);
        hash.finish()
    }
    pub fn endpoint(&self, resource: &str) -> Result<reqwest::Url, String> {
        let base_url = match self.route {
            Some(route) => route.base_url(),
            None => self.base_url.as_str(),
        };
        let mut base = base_url.trim().trim_end_matches('/').to_owned();
        if base.ends_with("/chat/completions") {
            base.truncate(base.len() - "/chat/completions".len());
        }
        let url = reqwest::Url::parse(&format!("{base}/{resource}"))
            .map_err(|_| "Enter a valid HTTP(S) API base URL")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(
                "Use an HTTP(S) base URL without credentials, query parameters, or fragments"
                    .into(),
            );
        }
        Ok(url)
    }
}

fn env_value(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        String::new()
    } else {
        std::env::var(name).unwrap_or_default().trim().to_owned()
    }
}

/// Preserve references to surviving connections after removing one entry.
/// References to the deleted connection fall back to the first connection.
pub fn remap_removed_provider(index: usize, removed: usize) -> usize {
    if index == removed {
        0
    } else if index > removed {
        index - 1
    } else {
        index
    }
}

#[derive(Debug)]
pub enum ProviderEvent {
    Delta(Uuid, String),
    Finished(Uuid),
    Failed(Uuid, String),
    Models(usize, u64, Result<Vec<String>, String>),
}

pub struct ChatJob {
    pub id: Uuid,
    pub cancel: Arc<AtomicBool>,
}
impl ChatJob {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for ChatJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

// Canceled blocking reads may live until their HTTP timeout. Reserving permits
// before spawning prevents rapid cancel/retry or Refresh clicks accumulating threads.
static CHAT_WORKERS: AtomicUsize = AtomicUsize::new(0);
static MODEL_WORKERS: AtomicUsize = AtomicUsize::new(0);
struct WorkerPermit<'a>(&'a AtomicUsize);
impl<'a> WorkerPermit<'a> {
    fn acquire(counter: &'a AtomicUsize, maximum: usize) -> Option<Self> {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < maximum).then_some(active + 1)
            })
            .ok()
            .map(|_| Self(counter))
    }
}
impl Drop for WorkerPermit<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

// Coalesce bursts so token-heavy streams do not flood the UI event queue, but
// forward the first spoken phrase token-by-token: speech starts once Fish sees
// a first-phrase boundary, so waiting out the window there only delays audio.
// The eager phase is bounded by Fish's 68-character hard flush and an event cap.
const COALESCE_WINDOW: Duration = Duration::from_millis(32);
const COALESCE_BYTES: usize = 1024;
const FIRST_PHRASE_CHARS: usize = 68;
const FIRST_PHRASE_EVENTS: usize = 24;

#[derive(Default)]
struct DeltaCoalescer {
    pending: String,
    last_emit: Option<Instant>,
    first_phrase_chars: usize,
    first_phrase_events: usize,
    first_phrase_done: bool,
}
impl DeltaCoalescer {
    fn push(&mut self, text: &str, now: Instant) -> Option<String> {
        self.pending.push_str(text);
        let eager = !self.first_phrase_done;
        if eager {
            for ch in text.chars() {
                self.first_phrase_chars += 1;
                let count = self.first_phrase_chars;
                if (count >= 10 && matches!(ch, '.' | '!' | '?' | '\n' | '。' | '！' | '？'))
                    || (count >= 24 && ch.is_whitespace())
                    || count >= FIRST_PHRASE_CHARS
                {
                    self.first_phrase_done = true;
                    break;
                }
            }
            self.first_phrase_events += 1;
            self.first_phrase_done |= self.first_phrase_events >= FIRST_PHRASE_EVENTS;
        }
        let due = self
            .last_emit
            .is_none_or(|last| now.duration_since(last) >= COALESCE_WINDOW);
        if self.pending.is_empty() || !(eager || due || self.pending.len() >= COALESCE_BYTES) {
            return None;
        }
        self.last_emit = Some(now);
        Some(std::mem::take(&mut self.pending))
    }
    fn finish(self) -> Option<String> {
        (!self.pending.is_empty()).then_some(self.pending)
    }
}

pub fn spawn_chat(config: ProviderConfig, turns: Vec<Turn>, tx: Sender<ProviderEvent>) -> ChatJob {
    spawn_chat_with_images(config, turns, Vec::new(), tx)
}

pub fn spawn_chat_with_images(
    config: ProviderConfig,
    turns: Vec<Turn>,
    images: Vec<ImageAttachment>,
    tx: Sender<ProviderEvent>,
) -> ChatJob {
    let id = Uuid::new_v4();
    let cancel = Arc::new(AtomicBool::new(false));
    let stopped = cancel.clone();
    let Some(permit) = WorkerPermit::acquire(&CHAT_WORKERS, 2) else {
        let _ = tx.send(ProviderEvent::Failed(
            id,
            "Previous connections are still closing. Please retry shortly.".into(),
        ));
        return ChatJob { id, cancel };
    };
    let failures = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("studio-chat".into())
        .spawn(move || {
            let _permit = permit;
            let secrets = config.secrets();
            let mut deltas = DeltaCoalescer::default();
            let result = stream_chat_with_images(&config, &turns, &images, &stopped, |text| {
                if stopped.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(delta) = deltas.push(&text, Instant::now())
                    && tx.send(ProviderEvent::Delta(id, delta)).is_err()
                {
                    stopped.store(true, Ordering::Relaxed);
                }
            });
            if stopped.load(Ordering::Relaxed) {
                return;
            }
            if let Some(delta) = deltas.finish() {
                let _ = tx.send(ProviderEvent::Delta(id, delta));
            }
            let event = match result {
                Ok(()) => ProviderEvent::Finished(id),
                Err(error) => ProviderEvent::Failed(id, if images.is_empty() { redact_all(error, &secrets) } else { "Image chat request failed. Check the connection and choose a vision-capable model.".into() }),
            };
            let _ = tx.send(event);
        });
    if let Err(error) = spawned {
        let _ = failures.send(ProviderEvent::Failed(
            id,
            format!("Cannot start chat worker: {error}"),
        ));
    }
    ChatJob { id, cancel }
}

fn client() -> Result<reqwest::blocking::Client, String> {
    // Clone the shared handle instead of starting a new HTTP runtime for each turn.
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(180))
                .pool_idle_timeout(Duration::from_secs(30))
                .pool_max_idle_per_host(2)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("WaifuStudio/0.1")
                .build()
                .map_err(|e| e.to_string())
        })
        .clone()
}

fn redact(error: String, key: &str) -> String {
    if key.is_empty() {
        error
    } else {
        error.replace(key, "[redacted]")
    }
}

fn redact_all(error: String, secrets: &[String]) -> String {
    secrets
        .iter()
        .fold(error, |error, secret| redact(error, secret))
}

fn checked_response(
    response: reqwest::blocking::Response,
) -> Result<reqwest::blocking::Response, String> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let mut body = String::new();
    response
        .take(8192)
        .read_to_string(&mut body)
        .map_err(|e| e.to_string())?;
    let message = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.chars().take(400).collect());
    Err(format!("Provider returned HTTP {status}: {message}"))
}

pub fn stream_chat(
    config: &ProviderConfig,
    turns: &[Turn],
    cancel: &AtomicBool,
    on_delta: impl FnMut(String),
) -> Result<(), String> {
    stream_chat_with_images(config, turns, &[], cancel, on_delta)
}

fn stream_chat_with_images(
    config: &ProviderConfig,
    turns: &[Turn],
    images: &[ImageAttachment],
    cancel: &AtomicBool,
    on_delta: impl FnMut(String),
) -> Result<(), String> {
    stream_chat_observed_inner(config, turns, images, cancel, on_delta, |_| {})
}

/// Production chat with an optional bounded-event observer for diagnostics.
/// Events can contain user text: callers must whitelist fields before logging.
pub fn stream_chat_observed(
    config: &ProviderConfig,
    turns: &[Turn],
    cancel: &AtomicBool,
    on_delta: impl FnMut(String),
    on_event: impl FnMut(&Value),
) -> Result<(), String> {
    stream_chat_observed_inner(config, turns, &[], cancel, on_delta, on_event)
}

fn stream_chat_observed_inner(
    config: &ProviderConfig,
    turns: &[Turn],
    images: &[ImageAttachment],
    cancel: &AtomicBool,
    mut on_delta: impl FnMut(String),
    mut on_event: impl FnMut(&Value),
) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    if images.len() > MAX_ATTACHMENTS
        || images.iter().any(|image| {
            image.data_url.len() > MAX_DATA_URL_BYTES
                || !image.data_url.starts_with("data:image/jpeg;base64,")
        })
    {
        return Err("Attach at most two prepared images within the size limit".into());
    }
    if config.effective_model().is_empty() {
        return Err("Choose a model in Connections first".into());
    }
    let context_bytes = turns.iter().try_fold(0usize, |total, turn| {
        total
            .checked_add(turn.role.len())?
            .checked_add(turn.content.len())
    });
    if turns.len() > 256 || context_bytes.is_none_or(|size| size > 1_000_000) {
        return Err("Chat context is too large; shorten the persona or recent messages".into());
    }
    let mut body = config.chat_body(turns);
    if !images.is_empty() {
        let index = turns
            .iter()
            .rposition(|turn| turn.role == "user")
            .ok_or("Images require a user message")?;
        let mut content = vec![json!({"type": "text", "text": turns[index].content})];
        content.extend(
            images
                .iter()
                .map(|image| json!({"type": "image_url", "image_url": {"url": image.data_url}})),
        );
        body["messages"][index]["content"] = json!(content);
    }
    let key = config.key();
    let mut request = client()?
        .post(config.endpoint("chat/completions")?)
        .json(&body);
    if !key.is_empty() {
        request = request.bearer_auth(&key);
    }
    let response = checked_response(request.send().map_err(|e| e.to_string())?)?;
    let json_response = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| h.contains("application/json"));
    if json_response {
        let value: Value = read_json(response, 2_000_000)?;
        on_event(&value);
        let text = value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or("Provider response has no assistant text")?;
        if !cancel.load(Ordering::Relaxed) {
            on_delta(text.to_owned());
        }
        return Ok(());
    }
    parse_sse_observed(
        BufReader::new(response),
        cancel,
        &mut on_delta,
        &mut on_event,
    )
}

fn read_json(response: impl Read, limit: u64) -> Result<Value, String> {
    let mut bytes = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Provider response exceeded its size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid provider JSON: {e}"))
}

#[cfg(test)]
fn parse_sse(
    reader: impl BufRead,
    cancel: &AtomicBool,
    on_delta: &mut impl FnMut(String),
) -> Result<(), String> {
    parse_sse_observed(reader, cancel, on_delta, &mut |_| {})
}

fn parse_sse_observed(
    mut reader: impl BufRead,
    cancel: &AtomicBool,
    on_delta: &mut impl FnMut(String),
    on_event: &mut impl FnMut(&Value),
) -> Result<(), String> {
    const MAX_RESPONSE: usize = 2_000_000;
    const MAX_LINE: usize = 256_000;
    let mut data = String::new();
    let mut finished = false;
    let mut received_text = false;
    let mut total = 0;
    let mut first_line = true;
    let mut bytes = Vec::with_capacity(4096);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        // Bound allocation before reading a newline, including hostile endless lines.
        bytes.clear();
        let limit = MAX_LINE.min(MAX_RESPONSE - total);
        let read = reader
            .by_ref()
            .take((limit + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .map_err(|e| e.to_string())?;
        if read > limit {
            return Err("Provider stream exceeded its line or 2 MB response limit".into());
        }
        total += read;
        let eof = read == 0;
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        let mut line =
            std::str::from_utf8(&bytes).map_err(|_| "Provider stream contains invalid UTF-8")?;
        if first_line {
            line = line.trim_start_matches('\u{feff}');
            first_line = false;
        }
        if let Some(payload) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            // SSE removes exactly one optional space after the colon.
            data.push_str(payload.strip_prefix(' ').unwrap_or(payload));
        }
        if (line.is_empty() || eof) && !data.is_empty() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            if data.trim() == "[DONE]" {
                return if received_text {
                    Ok(())
                } else {
                    Err("Provider finished without assistant text".into())
                };
            }
            let value: Value =
                serde_json::from_str(&data).map_err(|e| format!("Invalid stream event: {e}"))?;
            data.clear();
            on_event(&value);
            if let Some(error) = value.get("error") {
                return Err(format!("Provider stream error: {error}"));
            }
            if let Some(text) = value
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                && !text.is_empty()
                && !cancel.load(Ordering::Relaxed)
            {
                received_text = true;
                on_delta(text.to_owned());
            }
            if value
                .pointer("/choices/0/finish_reason")
                .is_some_and(|r| !r.is_null())
            {
                finished = true;
            }
        }
        if eof {
            break;
        }
    }
    if finished && received_text {
        Ok(())
    } else {
        Err("Provider stream ended before completion".into())
    }
}

/// Only advertise chat-capable entries from the mixed gateway catalogs.
fn chat_model_ids(value: &Value, route: Option<GatewayRoute>) -> Result<Vec<String>, String> {
    let models = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Expected a data array from /models")?;
    if models.len() > 10_000 {
        return Err("Model list exceeds 10,000 entries".into());
    }
    let mut ids: Vec<String> = models
        .iter()
        .filter(|model| match route {
            Some(GatewayRoute::VercelGateway) => {
                model.get("type").and_then(Value::as_str) == Some("language")
            }
            Some(GatewayRoute::OpenRouterResponses) => model
                .pointer("/architecture/output_modalities")
                .and_then(Value::as_array)
                .is_some_and(|outputs| outputs.iter().any(|v| v.as_str() == Some("text"))),
            None => true,
        })
        .filter_map(|model| model.get("id").and_then(Value::as_str))
        .filter(|id| route.is_none() || id.contains('/'))
        .map(str::to_owned)
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

pub fn fetch_models(
    index: usize,
    generation: u64,
    config: ProviderConfig,
    tx: Sender<ProviderEvent>,
) {
    let Some(permit) = WorkerPermit::acquire(&MODEL_WORKERS, 2) else {
        let _ = tx.send(ProviderEvent::Models(
            index,
            generation,
            Err("Model discovery is already busy. Please retry shortly.".into()),
        ));
        return;
    };
    let failures = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("studio-models".into())
        .spawn(move || {
            let _permit = permit;
            let key = config.key();
            let result = (|| {
                let mut request = client()?
                    .get(config.endpoint("models")?)
                    .timeout(Duration::from_secs(30));
                if !key.is_empty() {
                    request = request.bearer_auth(&key);
                }
                let response = checked_response(request.send().map_err(|e| e.to_string())?)?;
                let value: Value = read_json(response, 5_000_000)?;
                chat_model_ids(&value, config.route)
            })()
            .map_err(|error| redact(error, &key));
            let _ = tx.send(ProviderEvent::Models(index, generation, result));
        });
    if let Err(error) = spawned {
        let _ = failures.send(ProviderEvent::Models(
            index,
            generation,
            Err(format!("Cannot start model discovery: {error}")),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observer_preserves_final_metadata_and_text_without_duplicate_deltas() {
        let wire = concat!(
            "data: {\"id\":\"gen_test\",\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"providerMetadata\":{\"gateway\":{\"routing\":{\"finalProvider\":\"baseten\"}}}}\n\n",
            "data: [DONE]\n\n"
        );
        let mut text = String::new();
        let mut served = None;
        parse_sse_observed(
            std::io::Cursor::new(wire),
            &AtomicBool::new(false),
            &mut |delta| text.push_str(&delta),
            &mut |event| {
                if let Some(provider) = event
                    .pointer("/providerMetadata/gateway/routing/finalProvider")
                    .and_then(Value::as_str)
                {
                    served = Some(provider.to_owned());
                }
            },
        )
        .unwrap();
        assert_eq!(text, "Hello");
        assert_eq!(served.as_deref(), Some("baseten"));
    }
    #[test]
    fn catalog_identity_tracks_connection_and_auth_but_not_generation_settings() {
        let mut config = ProviderConfig::pinned(GatewayRoute::VercelGateway);
        config.session_key = "catalog-fixture-a".into();
        let original = config.catalog_identity();
        config.model = "another/model".into();
        config.temperature = 0.25;
        config.reasoning_effort = Some(ReasoningEffort::High);
        assert_eq!(config.catalog_identity(), original);
        config.session_key = "catalog-fixture-b".into();
        assert_ne!(config.catalog_identity(), original);
        let authed = config.catalog_identity();
        config.set_route(Some(GatewayRoute::OpenRouterResponses));
        assert_ne!(config.catalog_identity(), authed);
    }
    #[test]
    fn explicit_routed_models_and_suffixes_are_never_silently_replaced() {
        for route in GatewayRoute::ALL {
            let mut config = ProviderConfig::pinned(route);
            for id in [
                "openai/o1",
                "openai/o1-pro",
                "openai/gpt-5-pro",
                "openai/gpt-5-pro:nitro",
                "vendor/model:floor",
                "vendor/model:free",
            ] {
                config.model = format!(" {id} ");
                assert_eq!(config.effective_model(), id);
                assert_eq!(config.chat_body(&[])["model"], id);
            }
            config.model.clear();
            assert_eq!(config.effective_model(), route.default_model());
        }
    }

    #[test]
    fn reasoning_effort_wire_names_preserve_auto_vs_explicit_none() {
        let old: ProviderConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(old.reasoning_effort, None);
        for (effort, wire) in ReasoningEffort::ALL
            .into_iter()
            .zip(["none", "minimal", "low", "medium", "high", "xhigh", "max"])
        {
            for route in GatewayRoute::ALL {
                let mut config = ProviderConfig::pinned(route);
                config.reasoning_effort = Some(effort);
                assert_eq!(config.chat_body(&[])["reasoning_effort"], wire);
                let saved = serde_json::to_value(&config).unwrap();
                assert_eq!(saved["reasoning_effort"], wire);
                let restored: ProviderConfig = serde_json::from_value(saved).unwrap();
                assert_eq!(restored.reasoning_effort, Some(effort));
            }
        }
    }

    #[test]
    fn routing_wire_formats_are_gateway_specific_and_defaults_omit_extensions() {
        let old: ProviderConfig = serde_json::from_str("{}").unwrap();
        assert!(old.routing.allow_fallbacks);
        for route in GatewayRoute::ALL {
            let mut config = ProviderConfig::pinned(route);
            assert!(config.chat_body(&[]).get("provider").is_none());
            assert!(config.chat_body(&[]).get("providerOptions").is_none());
            config.routing = ProviderRouting {
                order: vec!["bedrock".into(), "anthropic".into()],
                only: vec!["anthropic".into()],
                sort: RoutingSort::Throughput,
                allow_fallbacks: false,
            };
            let body = config.chat_body(&[]);
            let routing = if route == GatewayRoute::VercelGateway {
                assert!(body.get("provider").is_none());
                &body["providerOptions"]["gateway"]
            } else {
                assert!(body.get("providerOptions").is_none());
                &body["provider"]
            };
            assert_eq!(routing["order"], json!(["bedrock", "anthropic"]));
            assert_eq!(routing["only"], json!(["anthropic"]));
            if route == GatewayRoute::VercelGateway {
                assert_eq!(routing["sort"], "tps");
                assert!(routing.get("allow_fallbacks").is_none());
            } else {
                assert_eq!(routing["sort"], "throughput");
                assert_eq!(routing["allow_fallbacks"], false);
            }
            let saved = serde_json::to_string(&config).unwrap();
            let loaded: ProviderConfig = serde_json::from_str(&saved).unwrap();
            assert_eq!(loaded.routing.order, config.routing.order);
            config.set_route(None);
            assert!(config.routing.order.is_empty());
            assert!(config.chat_body(&[]).get("provider").is_none());
        }
        for (sort, vercel, router) in [
            (RoutingSort::Cost, "cost", "price"),
            (RoutingSort::Latency, "ttft", "latency"),
        ] {
            let config = ProviderRouting {
                sort,
                ..Default::default()
            };
            assert_eq!(config.body(GatewayRoute::VercelGateway)["sort"], vercel);
            assert_eq!(
                config.body(GatewayRoute::OpenRouterResponses)["sort"],
                router
            );
        }
    }

    #[test]
    fn pinned_gpt5_sampling_and_automatic_reasoning_are_conservative() {
        for route in GatewayRoute::ALL {
            let mut config = ProviderConfig::pinned(route);
            config.top_p = Some(0.9);
            for model in [
                "openai/gpt-5",
                "openai/gpt-5-mini",
                "openai/gpt-5-nano",
                "openai/gpt-5-codex",
                "openai/gpt-5.2-codex",
                "openai/gpt-5.6-luna",
            ] {
                config.model = model.into();
                let body = config.chat_body(&[]);
                assert!(body.get("temperature").is_none(), "{model}");
                assert!(body.get("top_p").is_none(), "{model}");
                let expected = route == GatewayRoute::VercelGateway
                    && matches!(
                        model,
                        "openai/gpt-5" | "openai/gpt-5-mini" | "openai/gpt-5-nano"
                    );
                assert_eq!(body.get("reasoning_effort").is_some(), expected, "{model}");
            }
            config.reasoning_effort = Some(ReasoningEffort::High);
            assert_eq!(config.chat_body(&[])["reasoning_effort"], "high");
            config.model = "anthropic/claude-sonnet-4".into();
            assert!(config.chat_body(&[]).get("temperature").is_some());
            assert!(config.chat_body(&[]).get("top_p").is_some());
        }
    }

    #[test]
    fn gateway_catalogs_exclude_non_chat_models_and_unknown_capabilities() {
        let value = json!({"data": [
            {"id":"openai/gpt-5-nano","type":"language","architecture":{"output_modalities":["text"]}},
            {"id":"openai/whisper-1","type":"transcription","architecture":{"output_modalities":["audio"]}},
            {"id":"vendor/image","type":"image","architecture":{"output_modalities":["image"]}},
            {"id":"vendor/embed","type":"embedding"},
            {"id":"vendor/unknown"},
            {"id":"vendor/multimodal","type":"language","architecture":{"output_modalities":["text","image"]}},
            {"id":"openai/gpt-5-pro","type":"language","architecture":{"output_modalities":["text"]}},
            {"id":"openai/gpt-5-nano","type":"language","architecture":{"output_modalities":["text"]}}
        ]});
        for route in GatewayRoute::ALL {
            assert_eq!(
                chat_model_ids(&value, Some(route)).unwrap(),
                vec!["openai/gpt-5-nano", "openai/gpt-5-pro", "vendor/multimodal"]
            );
        }
        assert!(
            chat_model_ids(&value, None)
                .unwrap()
                .contains(&"vendor/unknown".into())
        );
        assert!(chat_model_ids(&json!({}), None).is_err());
    }

    #[test]
    fn old_profiles_migrate_and_optional_parameters_are_omitted() {
        let config: ProviderConfig =
            serde_json::from_str(r#"{"name":"old","session_key":"must not load"}"#).unwrap();
        assert!(config.session_key.is_empty());
        assert_eq!(config.max_tokens, None);
        assert_eq!(config.top_p, None);
        assert_eq!(config.reasoning_effort, None);
        let body = config.chat_body(&[]);
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("top_p").is_none());
        assert!(body.get("reasoning_effort").is_none());
        let config = ProviderConfig {
            reasoning_effort: Some(ReasoningEffort::High),
            max_tokens: Some(2048),
            top_p: Some(0.7),
            session_key: "secret".into(),
            ..config
        };
        assert_eq!(config.chat_body(&[])["max_tokens"], 2048);
        assert_eq!(config.chat_body(&[])["reasoning_effort"], "high");
        let loaded: ProviderConfig =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(loaded.reasoning_effort, Some(ReasoningEffort::High));
        assert!((config.chat_body(&[])["top_p"].as_f64().unwrap() - 0.7).abs() < 0.0001);
        assert!(!serde_json::to_string(&config).unwrap().contains("secret"));
    }
    #[test]
    fn pinned_routes_match_webwaifu_provider_rules() {
        assert_eq!(
            GatewayRoute::normalize("openrouter-responses"),
            GatewayRoute::OpenRouterResponses
        );
        for other in ["openai-responses", "deepseek", "bad"] {
            assert_eq!(GatewayRoute::normalize(other), GatewayRoute::VercelGateway);
        }
        let gateway = GatewayRoute::VercelGateway;
        assert_eq!(gateway.pin_model(""), "openai/gpt-5-nano");
        assert_eq!(gateway.pin_model("gpt-4o"), "openai/gpt-5-nano");
        assert_eq!(gateway.pin_model("openai/o1-preview"), "openai/o1-preview");
        assert_eq!(gateway.pin_model("openai/gpt-5-pro"), "openai/gpt-5-pro");
        assert_eq!(
            gateway.pin_model(" anthropic/claude-sonnet-4 "),
            "anthropic/claude-sonnet-4"
        );
        assert_eq!(
            GatewayRoute::OpenRouterResponses.pin_model("x"),
            "openai/gpt-4o-mini"
        );
        assert!(is_openai_gpt5("openai/gpt-5.1-mini") && is_openai_gpt5("gpt-5"));
        assert!(!is_openai_gpt5("azure/gpt-5"));

        let mut config = ProviderConfig::pinned(gateway);
        config.base_url = "http://elsewhere.invalid/v1".into();
        assert_eq!(
            config.endpoint("chat/completions").unwrap().as_str(),
            "https://ai-gateway.vercel.sh/v1/chat/completions"
        );
        let body = config.chat_body(&[]);
        assert_eq!(body["model"], "openai/gpt-5-nano");
        assert_eq!(body["reasoning_effort"], "minimal");
        assert!(body.get("providerOptions").is_none());
        config.reasoning_effort = Some(ReasoningEffort::High);
        assert_eq!(config.chat_body(&[])["reasoning_effort"], "high");
        config.model = "anthropic/claude-sonnet-4".into();
        config.reasoning_effort = None;
        assert!(config.chat_body(&[]).get("reasoning_effort").is_none());

        // OpenRouter ignores the OpenAI reasoning option upstream, so nothing is injected.
        let mut router = ProviderConfig::pinned(GatewayRoute::OpenRouterResponses);
        router.model = "openai/gpt-5-mini".into();
        router.byok_openai_key_env = "PATH".into();
        let body = router.chat_body(&[]);
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("providerOptions").is_none());
        assert_eq!(router.key_env, "OPENROUTER_API_KEY");

        // Switching routes resets the model; clearing keeps a usable generic connection.
        router.set_route(Some(gateway));
        assert_eq!(router.model, "openai/gpt-5-nano");
        assert_eq!(router.key_env, "AI_GATEWAY_API_KEY");
        router.set_route(None);
        assert_eq!(router.base_url, "https://ai-gateway.vercel.sh/v1");
        assert_eq!(router.effective_model(), "openai/gpt-5-nano");
    }
    #[test]
    fn route_serialization_is_additive_and_byok_stays_an_env_name() {
        let old: ProviderConfig = serde_json::from_str(
            r#"{"name":"Custom","base_url":"http://127.0.0.1:8080/v1","model":"llama"}"#,
        )
        .unwrap();
        assert_eq!(old.route, None);
        assert_eq!(old.effective_model(), "llama");
        assert!(old.chat_body(&[]).get("reasoning_effort").is_none());
        let mut pinned = ProviderConfig::pinned(GatewayRoute::VercelGateway);
        pinned.byok_openai_key_env = "WAIFU_TEST_BYOK_ENV".into();
        let saved = serde_json::to_value(&pinned).unwrap();
        assert_eq!(saved["route"], "vercel-gateway");
        assert_eq!(saved["byok_openai_key_env"], "WAIFU_TEST_BYOK_ENV");
        let loaded: ProviderConfig = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded.route, Some(GatewayRoute::VercelGateway));
        let router: ProviderConfig =
            serde_json::from_str(r#"{"route":"openrouter-responses"}"#).unwrap();
        assert_eq!(
            router.endpoint("models").unwrap().as_str(),
            "https://openrouter.ai/api/v1/models"
        );
        assert!(serde_json::from_str::<ProviderConfig>(r#"{"route":"deepseek"}"#).is_err());
        assert_eq!(
            redact_all(
                "a k1 b k2".into(),
                &["k1".into(), String::new(), "k2".into()]
            ),
            "a [redacted] b [redacted]"
        );
    }
    #[test]
    fn removal_preserves_survivors_and_falls_back_for_deleted_connection() {
        let refs = [0, 1, 2, 3, 4].map(|i| remap_removed_provider(i, 2));
        assert_eq!(refs, [0, 1, 0, 2, 3]);
        assert_eq!(remap_removed_provider(0, 0), 0);
        assert_eq!(remap_removed_provider(4, 4), 0);
    }
    #[test]
    fn accepts_unicode_sse_and_rejects_truncated_or_error_streams() {
        let cancel = AtomicBool::new(false);
        let input =
            "data: {\"choices\":[{\"delta\":{\"content\":\"こんにちは\"}}]}\n\ndata: [DONE]\n\n";
        let mut text = String::new();
        parse_sse(input.as_bytes(), &cancel, &mut |s| text.push_str(&s)).unwrap();
        assert_eq!(text, "こんにちは");
        assert!(
            parse_sse(
                b"data: {\"choices\":[]}\n\n".as_slice(),
                &cancel,
                &mut |_| {}
            )
            .is_err()
        );
        assert!(
            parse_sse(
                b"data: {\"error\":{\"message\":\"no quota\"}}\n\n".as_slice(),
                &cancel,
                &mut |_| {}
            )
            .is_err()
        );
    }
    #[test]
    fn normalizes_endpoints_and_redacts_secrets() {
        let config = ProviderConfig {
            base_url: "http://localhost:1234/v1/chat/completions".into(),
            ..Default::default()
        };
        assert_eq!(
            config.endpoint("models").unwrap().as_str(),
            "http://localhost:1234/v1/models"
        );
        assert_eq!(
            redact("key secret failed".into(), "secret"),
            "key [redacted] failed"
        );
    }
    #[test]
    fn real_http_client_posts_model_messages_and_stream_flag_to_local_fixture() {
        use std::{io::Write, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut header = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if line.to_lowercase().starts_with("content-length:") {
                    length = line.split(':').nth(1).unwrap().trim().parse().unwrap();
                }
                header.push_str(&line);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert!(header.starts_with("POST /v1/chat/completions"));
            assert_eq!(body["model"], "local-test");
            assert_eq!(body["stream"], true);
            assert_eq!(body["messages"][0]["content"], "hello");
            let stream =
                "data: {\"choices\":[{\"delta\":{\"content\":\"reply\"}}]}\n\ndata: [DONE]\n\n";
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", stream.len(), stream).unwrap();
        });
        let config = ProviderConfig {
            base_url: format!("http://{addr}/v1"),
            model: "local-test".into(),
            ..Default::default()
        };
        let mut result = String::new();
        stream_chat(
            &config,
            &[Turn {
                role: "user".into(),
                content: "hello".into(),
            }],
            &AtomicBool::new(false),
            |s| result.push_str(&s),
        )
        .unwrap();
        assert_eq!(result, "reply");
        thread.join().unwrap();
    }
    #[test]
    fn real_http_client_posts_image_content_to_local_fixture() {
        use std::{io::Write, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut header = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if line.to_lowercase().starts_with("content-length:") {
                    length = line.split(':').nth(1).unwrap().trim().parse().unwrap();
                }
                header.push_str(&line);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert!(header.starts_with("POST /v1/chat/completions"));
            assert_eq!(body["model"], "local-test");
            assert_eq!(body["stream"], true);
            assert_eq!(
                body["messages"][0]["content"][0],
                json!({"type":"text","text":"hello"})
            );
            assert_eq!(
                body["messages"][0]["content"][1],
                json!({"type":"image_url","image_url":{"url":"data:image/jpeg;base64,dGVzdA=="}})
            );
            assert!(body.get("session_key").is_none());
            let stream =
                "data: {\"choices\":[{\"delta\":{\"content\":\"reply\"}}]}\n\ndata: [DONE]\n\n";
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", stream.len(), stream).unwrap();
        });
        let config = ProviderConfig {
            base_url: format!("http://{addr}/v1"),
            model: "local-test".into(),
            ..Default::default()
        };
        let mut result = String::new();
        stream_chat_with_images(
            &config,
            &[Turn {
                role: "user".into(),
                content: "hello".into(),
            }],
            &[ImageAttachment {
                name: "selected.jpg".into(),
                data_url: "data:image/jpeg;base64,dGVzdA==".into(),
            }],
            &AtomicBool::new(false),
            |s| result.push_str(&s),
        )
        .unwrap();
        assert_eq!(result, "reply");
        thread.join().unwrap();
    }
    #[test]
    fn sse_handles_multiline_crlf_bom_and_final_event_without_newline() {
        let input = "\u{feff}: keepalive\r\nevent: message\r\ndata: {\r\ndata: \"choices\":[{\"delta\":{\"content\":\"你好 🌸\"},\"finish_reason\":\"stop\"}]}";
        let mut output = String::new();
        parse_sse(
            BufReader::with_capacity(1, input.as_bytes()),
            &AtomicBool::new(false),
            &mut |s| output.push_str(&s),
        )
        .unwrap();
        assert_eq!(output, "你好 🌸");
    }

    #[test]
    fn sse_enforces_limits_before_large_allocation_and_rejects_invalid_utf8() {
        let mut input = vec![b'x'; 256_001];
        assert!(
            parse_sse(input.as_slice(), &AtomicBool::new(false), &mut |_| {})
                .unwrap_err()
                .contains("limit")
        );
        input = vec![0xff, b'\n'];
        assert!(
            parse_sse(input.as_slice(), &AtomicBool::new(false), &mut |_| {})
                .unwrap_err()
                .contains("UTF-8")
        );
        let input = ": heartbeat\n".repeat(200_000);
        assert!(
            parse_sse(input.as_bytes(), &AtomicBool::new(false), &mut |_| {})
                .unwrap_err()
                .contains("limit")
        );
        assert!(
            read_json(b"{} trailing too long".as_slice(), 2)
                .unwrap_err()
                .contains("limit")
        );
    }

    #[test]
    fn first_spoken_phrase_streams_immediately_then_later_tokens_coalesce() {
        let start = Instant::now();
        let mut deltas = DeltaCoalescer::default();
        // Every first-phrase token is forwarded at once, even inside the window.
        for token in ["Hel", "lo", " the", "re!"] {
            assert_eq!(deltas.push(token, start).as_deref(), Some(token));
        }
        // After the sentence boundary, bursts wait out the window.
        assert_eq!(deltas.push(" How", start + Duration::from_millis(1)), None);
        assert_eq!(deltas.push(" are", start + Duration::from_millis(31)), None);
        assert_eq!(
            deltas
                .push(" you", start + Duration::from_millis(32))
                .as_deref(),
            Some(" How are you")
        );
        assert_eq!(deltas.push("?", start + Duration::from_millis(33)), None);
        // The byte bound still flushes inside the window.
        let big = "x".repeat(COALESCE_BYTES);
        assert_eq!(
            deltas.push(&big, start + Duration::from_millis(34)),
            Some(format!("?{big}"))
        );
        assert_eq!(deltas.push(" bye", start + Duration::from_millis(35)), None);
        assert_eq!(deltas.finish().as_deref(), Some(" bye"));
    }

    #[test]
    fn first_phrase_eager_delivery_is_bounded_without_punctuation() {
        let start = Instant::now();
        let mut deltas = DeltaCoalescer::default();
        // Single-character tokens with no boundary: the event cap ends the eager phase.
        for _ in 0..FIRST_PHRASE_EVENTS {
            assert_eq!(deltas.push("a", start).as_deref(), Some("a"));
        }
        assert_eq!(deltas.push("a", start), None);
        // A long unbroken first token hits Fish's hard flush length instead.
        let mut deltas = DeltaCoalescer::default();
        let long = "b".repeat(FIRST_PHRASE_CHARS);
        assert_eq!(deltas.push(&long, start), Some(long));
        assert_eq!(deltas.push("c", start), None);
        // A first-phrase word boundary after 24 characters also ends it.
        let mut deltas = DeltaCoalescer::default();
        let words = format!("{} ", "d".repeat(23));
        assert_eq!(deltas.push(&words, start), Some(words));
        assert_eq!(deltas.push("e", start), None);
        // Short early punctuation does not end the eager phase.
        let mut deltas = DeltaCoalescer::default();
        assert_eq!(deltas.push("Oh.", start).as_deref(), Some("Oh."));
        assert_eq!(deltas.push(" I", start).as_deref(), Some(" I"));
    }

    #[test]
    fn spawned_chat_delivers_first_phrase_tokens_and_stops_on_cancel() {
        use std::{io::Write, net::TcpListener, sync::mpsc};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (release, gate) = mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut line = String::new();
            while line != "\r\n" {
                line.clear();
                reader.read_line(&mut line).unwrap();
            }
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            for token in ["Hi", " there", "!"] {
                write!(
                    socket,
                    "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{token}\"}}}}]}}\n\n"
                )
                .unwrap();
                socket.flush().unwrap();
            }
            // Hold the stream open so the first phrase must arrive before completion.
            let _ = gate.recv_timeout(Duration::from_secs(10));
            let _ = write!(
                socket,
                "data: {{\"choices\":[{{\"delta\":{{\"content\":\" late\"}}}}]}}\n\ndata: [DONE]\n\n"
            );
        });
        let config = ProviderConfig {
            base_url: format!("http://{addr}/v1"),
            model: "local-test".into(),
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        let job = spawn_chat(config, Vec::new(), tx);
        let mut text = String::new();
        while text != "Hi there!" {
            match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                ProviderEvent::Delta(id, delta) if id == job.id => text.push_str(&delta),
                other => panic!("unexpected event before stream end: {other:?}"),
            }
        }
        job.cancel();
        release.send(()).unwrap();
        server.join().unwrap();
        // Cancellation suppresses the coalesced tail and the terminal event.
        if let Ok(event) = rx.recv_timeout(Duration::from_millis(500)) {
            panic!("event after cancellation: {event:?}");
        }
    }

    #[test]
    fn cancellation_suppresses_callbacks_and_stops_before_network_access() {
        let cancel = AtomicBool::new(true);
        let mut count = 0;
        parse_sse(b"data: broken\n\n".as_slice(), &cancel, &mut |_| count += 1).unwrap();
        stream_chat(&ProviderConfig::default(), &[], &cancel, |_| count += 1).unwrap();
        assert_eq!(count, 0);
        let cancel = AtomicBool::new(false);
        let input = "data: {\"choices\":[{\"delta\":{\"content\":\"first\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"second\"}}]}\n\n";
        parse_sse(input.as_bytes(), &cancel, &mut |_| {
            count += 1;
            cancel.store(true, Ordering::Relaxed);
        })
        .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn endpoints_reject_embedded_credentials_and_non_http_schemes() {
        for base_url in [
            "https://secret@example.com/v1",
            "https://example.com/v1?api_key=secret",
            "file:///tmp/api",
            "https://example.com/v1#secret",
        ] {
            let config = ProviderConfig {
                base_url: base_url.into(),
                ..Default::default()
            };
            assert!(config.endpoint("models").is_err(), "accepted {base_url}");
        }
    }
    #[test]
    fn worker_permits_bound_concurrency_and_release_on_drop() {
        let active = AtomicUsize::new(0);
        let first = WorkerPermit::acquire(&active, 2).unwrap();
        let second = WorkerPermit::acquire(&active, 2).unwrap();
        assert!(WorkerPermit::acquire(&active, 2).is_none());
        drop(first);
        let replacement = WorkerPermit::acquire(&active, 2).unwrap();
        assert_eq!(active.load(Ordering::Acquire), 2);
        drop(second);
        drop(replacement);
        assert_eq!(active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn oversized_image_batches_are_rejected_before_network_access() {
        let config = ProviderConfig {
            model: "test".into(),
            base_url: "invalid".into(),
            ..Default::default()
        };
        let images = vec![
            ImageAttachment {
                name: "image".into(),
                data_url: "data:image/jpeg;base64,dGVzdA==".into()
            };
            3
        ];
        assert!(
            stream_chat_with_images(&config, &[], &images, &AtomicBool::new(false), |_| {})
                .unwrap_err()
                .contains("at most two")
        );
    }

    #[test]
    fn oversized_context_is_rejected_before_any_network_access() {
        let config = ProviderConfig {
            model: "test".into(),
            base_url: "invalid-url".into(),
            ..Default::default()
        };
        let turns = vec![Turn {
            role: "user".into(),
            content: "x".repeat(1_000_001),
        }];
        assert!(
            stream_chat(&config, &turns, &AtomicBool::new(false), |_| {})
                .unwrap_err()
                .contains("context is too large")
        );
    }
}
