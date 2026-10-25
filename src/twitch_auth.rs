//! Bounded device authorization and single-attempt refresh. Neither opens a browser nor saves
//! credentials. The host must keep its global admission slot until is_finished,
//! then recheck installation/configuration revisions before committing tokens.
use reqwest::{Url, blocking::Client};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::Read,
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

const RESPONSE_LIMIT: usize = 64 * 1024;
const SECRET_LIMIT: usize = 4096;
const MAX_SCOPES: usize = 32;
const MAX_TTL: u64 = 1800;
const MAX_INTERVAL: u64 = 60;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_ACCESS_TTL: u64 = 86400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthError {
    InvalidClientId,
    InvalidScopes,
    WorkerUnavailable,
    Network,
    Timeout,
    ResponseTooLarge,
    MalformedResponse,
    InvalidResponse,
    UnexpectedHttpStatus,
    AccessDenied,
    Expired,
    ClientMismatch,
    InvalidUser,
    UserMismatch,
    MissingScopes,
    PollingLimit,
    Cancelled,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidClientId => "Invalid Twitch application ID",
            Self::InvalidScopes => "Invalid Twitch authorization scopes",
            Self::WorkerUnavailable => "Twitch authorization worker unavailable",
            Self::Network => "Twitch authorization could not connect",
            Self::Timeout => "Twitch authorization request timed out",
            Self::ResponseTooLarge => "Twitch authorization response exceeded its size limit",
            Self::MalformedResponse => "Twitch authorization returned malformed JSON",
            Self::InvalidResponse => "Twitch authorization returned an invalid response",
            Self::UnexpectedHttpStatus => "Twitch authorization request was rejected",
            Self::AccessDenied => "Twitch authorization was denied",
            Self::Expired => "Twitch authorization code expired",
            Self::ClientMismatch => "Twitch token belongs to another application",
            Self::InvalidUser => "Twitch authorization requires a valid user identity",
            Self::UserMismatch => "Twitch refreshed token belongs to another account",
            Self::MissingScopes => "Twitch authorization is missing required scopes",
            Self::PollingLimit => "Twitch authorization polling limit reached",
            Self::Cancelled => "Twitch authorization cancelled",
        })
    }
}
impl std::error::Error for AuthError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthPhase {
    Starting,
    AwaitingAuthorization,
    Validating,
    Completed,
    Cancelled,
    Failed,
}

// No Debug/serialization: the transient user code is for the consent UI only.
#[derive(Clone)]
pub struct DeviceChallenge {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_at: Instant,
}

#[derive(Clone)]
pub struct AuthStatus {
    pub phase: AuthPhase,
    pub challenge: Option<DeviceChallenge>,
    pub error: Option<AuthError>,
}

// Secrets never enter public status, diagnostics, or serialized plain settings.
pub struct TokenBundle {
    client_id: String,
    user_id: String,
    scopes: BTreeSet<String>,
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    validated_at: Instant,
}

impl TokenBundle {
    /// Offline-only input for the host's private credential-commit regressions.
    #[cfg(test)]
    pub(crate) fn fixture(client_id: &str) -> Self {
        Self {
            client_id: client_id.into(),
            user_id: "123456".into(),
            scopes: ["channel:read:subscriptions", "bits:read", "user:read:chat"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            access_token: "offline-only-access-token".into(),
            refresh_token: "offline-only-refresh-token".into(),
            expires_in: 14400,
            validated_at: Instant::now(),
        }
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }
    pub fn user_id(&self) -> &str {
        &self.user_id
    }
    pub fn scopes(&self) -> &BTreeSet<String> {
        &self.scopes
    }
    /// Conservative lifetime remaining at validated_at, capped to one day.
    /// Subtract elapsed time or use expires_at when scheduling a later commit.
    pub fn expires_in(&self) -> u64 {
        self.expires_in
    }
    pub fn validated_at(&self) -> Instant {
        self.validated_at
    }
    pub fn expires_at(&self) -> Instant {
        self.validated_at + Duration::from_secs(self.expires_in)
    }
    /// Consume only inside the host's private, serialized credential commit.
    pub fn into_credential_updates(self) -> BTreeMap<String, Option<String>> {
        BTreeMap::from([
            ("client_id".into(), Some(self.client_id)),
            ("access_token".into(), Some(self.access_token)),
            ("refresh_token".into(), Some(self.refresh_token)),
        ])
    }
}

struct State {
    cancelled: bool,
    status: AuthStatus,
    completion: Option<Result<TokenBundle, AuthError>>,
    taken: bool,
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn check(&self, deadline: Option<Instant>) -> Result<(), AuthError> {
        if self.lock().cancelled {
            Err(AuthError::Cancelled)
        } else if deadline.is_some_and(|end| Instant::now() >= end) {
            Err(AuthError::Expired)
        } else {
            Ok(())
        }
    }
    fn wait(&self, interval: Duration, deadline: Instant) -> Result<(), AuthError> {
        let until = Instant::now()
            .checked_add(interval)
            .ok_or(AuthError::InvalidResponse)?
            .min(deadline);
        let mut state = self.lock();
        loop {
            if state.cancelled {
                return Err(AuthError::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(AuthError::Expired);
            }
            if now >= until {
                return Ok(());
            }
            state = self
                .wake
                .wait_timeout(state, until - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
    fn phase(&self, phase: AuthPhase, challenge: Option<DeviceChallenge>) -> Result<(), AuthError> {
        let mut state = self.lock();
        if state.cancelled {
            return Err(AuthError::Cancelled);
        }
        state.status = AuthStatus {
            phase,
            challenge,
            error: None,
        };
        Ok(())
    }
    fn finish(&self, result: Result<TokenBundle, AuthError>) {
        let mut state = self.lock();
        if state.cancelled {
            // cancel already published its one-shot result; drop any tokens.
            return;
        }
        let error = result.as_ref().err().copied();
        state.status = AuthStatus {
            phase: if error.is_some() {
                AuthPhase::Failed
            } else {
                AuthPhase::Completed
            },
            challenge: None,
            error,
        };
        state.completion = Some(result);
    }
}

struct Endpoints {
    device: Url,
    token: Url,
    validate: Url,
}
impl Endpoints {
    fn production() -> Self {
        Self {
            device: Url::parse("https://id.twitch.tv/oauth2/device").expect("static URL"),
            token: Url::parse("https://id.twitch.tv/oauth2/token").expect("static URL"),
            validate: Url::parse("https://id.twitch.tv/oauth2/validate").expect("static URL"),
        }
    }
}

// Intentionally no Debug: Refresh contains a rotating credential.
enum Grant {
    Device {
        second: Duration,
    },
    Refresh {
        token: String,
        expected_user: String,
    },
}

/// One authorization attempt per handle. No retry, browser launch or persistence.
/// Drop cancels without blocking; an in-flight request has an eight-second bound.
pub struct TwitchAuth {
    shared: Arc<Shared>,
    worker: JoinHandle<()>,
}

impl TwitchAuth {
    /// Controlled offline job for controller admission/reaping regressions.
    /// Cancellation uses the real shared state but cannot finish this worker
    /// until release is set, or the two-second panic-safety deadline expires.
    #[cfg(test)]
    pub(crate) fn held_fixture() -> (Self, Arc<std::sync::atomic::AtomicBool>) {
        use std::sync::atomic::{AtomicBool, Ordering};

        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                cancelled: false,
                status: AuthStatus {
                    phase: AuthPhase::Validating,
                    challenge: None,
                    error: None,
                },
                completion: None,
                taken: false,
            }),
            wake: Condvar::new(),
        });
        let release = Arc::new(AtomicBool::new(false));
        let worker_release = release.clone();
        let job = shared.clone();
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !worker_release.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            job.finish(Err(AuthError::Cancelled));
        });
        (Self { shared, worker }, release)
    }

    pub fn begin(
        client_id: String,
        scopes: impl IntoIterator<Item = String>,
    ) -> Result<Self, AuthError> {
        Self::start(
            client_id,
            scopes,
            Endpoints::production(),
            Duration::from_secs(1),
            false,
        )
    }

    /// One rotating refresh grant, with no retry even after uncertain transport
    /// failure. Caller owns cross-process exclusion and durable rotation commit.
    pub fn refresh(
        client_id: String,
        refresh_token: String,
        expected_user_id: String,
        scopes: impl IntoIterator<Item = String>,
    ) -> Result<Self, AuthError> {
        Self::start_refresh(
            client_id,
            refresh_token,
            expected_user_id,
            scopes,
            Endpoints::production(),
            false,
        )
    }

    fn start_refresh(
        client_id: String,
        refresh_token: String,
        expected_user_id: String,
        scopes: impl IntoIterator<Item = String>,
        endpoints: Endpoints,
        no_proxy: bool,
    ) -> Result<Self, AuthError> {
        let requested = inputs(&client_id, scopes)?;
        if refresh_token.is_empty()
            || refresh_token.len() > SECRET_LIMIT
            || !refresh_token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(AuthError::InvalidResponse);
        }
        if !valid_user_id(&expected_user_id) {
            return Err(AuthError::InvalidUser);
        }
        Self::launch(
            client_id,
            requested,
            endpoints,
            Grant::Refresh {
                token: refresh_token,
                expected_user: expected_user_id,
            },
            no_proxy,
        )
    }

    fn start(
        client_id: String,
        scopes: impl IntoIterator<Item = String>,
        endpoints: Endpoints,
        second: Duration,
        no_proxy: bool,
    ) -> Result<Self, AuthError> {
        let requested = inputs(&client_id, scopes)?;
        Self::launch(
            client_id,
            requested,
            endpoints,
            Grant::Device { second },
            no_proxy,
        )
    }

    fn launch(
        client_id: String,
        requested: BTreeSet<String>,
        endpoints: Endpoints,
        grant: Grant,
        no_proxy: bool,
    ) -> Result<Self, AuthError> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                cancelled: false,
                status: AuthStatus {
                    phase: AuthPhase::Starting,
                    challenge: None,
                    error: None,
                },
                completion: None,
                taken: false,
            }),
            wake: Condvar::new(),
        });
        let job = shared.clone();
        let worker = std::thread::Builder::new()
            .name("waifu-twitch-auth".into())
            .spawn(move || {
                let result = authorize(&job, client_id, requested, endpoints, grant, no_proxy);
                job.finish(result);
            })
            .map_err(|_| AuthError::WorkerUnavailable)?;
        Ok(Self { shared, worker })
    }

    pub fn cancel(&self) {
        let mut state = self.shared.lock();
        state.cancelled = true;
        state.status = AuthStatus {
            phase: AuthPhase::Cancelled,
            challenge: None,
            error: Some(AuthError::Cancelled),
        };
        state.completion = if state.taken {
            None
        } else {
            Some(Err(AuthError::Cancelled))
        };
        self.shared.wake.notify_all();
    }

    pub fn status(&self) -> AuthStatus {
        self.shared.lock().status.clone()
    }

    /// Authoritative thread completion, including after a cancellation request.
    pub fn is_finished(&self) -> bool {
        self.worker.is_finished()
    }

    pub fn take_completion(&mut self) -> Option<Result<TokenBundle, AuthError>> {
        let mut state = self.shared.lock();
        let result = state.completion.take()?;
        state.taken = true;
        Some(result)
    }
}

impl Drop for TwitchAuth {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn valid_scope(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b':' | b'_')
        })
}

fn valid_user_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Syntactic application-ID validation only; this does not contact Twitch.
pub fn valid_client_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn inputs(
    client_id: &str,
    scopes: impl IntoIterator<Item = String>,
) -> Result<BTreeSet<String>, AuthError> {
    if !valid_client_id(client_id) {
        return Err(AuthError::InvalidClientId);
    }
    // Includes duplicates: at most MAX_SCOPES+1 iterator items are consumed.
    let mut requested = BTreeSet::new();
    for (index, scope) in scopes.into_iter().enumerate() {
        if index >= MAX_SCOPES || !valid_scope(&scope) {
            return Err(AuthError::InvalidScopes);
        }
        requested.insert(scope);
    }
    if requested.is_empty() {
        return Err(AuthError::InvalidScopes);
    }
    Ok(requested)
}

fn bounded_string<'a>(body: &'a Value, field: &str, limit: usize) -> Result<&'a str, AuthError> {
    body[field]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= limit)
        .ok_or(AuthError::InvalidResponse)
}

fn secret(body: &Value, field: &str) -> Result<String, AuthError> {
    let value = bounded_string(body, field, SECRET_LIMIT)?;
    if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(AuthError::InvalidResponse);
    }
    Ok(value.into())
}

fn scope_set(body: &Value, field: &str) -> Result<BTreeSet<String>, AuthError> {
    let values = body[field]
        .as_array()
        .filter(|values| values.len() <= MAX_SCOPES)
        .ok_or(AuthError::InvalidResponse)?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|scope| valid_scope(scope))
                .map(str::to_owned)
                .ok_or(AuthError::InvalidResponse)
        })
        .collect()
}

fn activation_uri(raw: &str, user_code: &str) -> Result<String, AuthError> {
    if raw.len() > 2048 || raw.chars().any(char::is_control) {
        return Err(AuthError::InvalidResponse);
    }
    let url = Url::parse(raw).map_err(|_| AuthError::InvalidResponse)?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("twitch.tv" | "www.twitch.tv"))
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.path() != "/activate"
        || raw.split("://").nth(1).is_some_and(|rest| {
            rest.split(['/', '?', '#'])
                .next()
                .is_some_and(|authority| authority.contains('@'))
        })
    {
        return Err(AuthError::InvalidResponse);
    }
    let mut seen = BTreeSet::new();
    for (key, value) in url.query_pairs() {
        if !seen.insert(key.to_string())
            || !matches!((key.as_ref(), value.as_ref()), ("public", "true"))
                && !(key == "device-code" && value == user_code)
        {
            return Err(AuthError::InvalidResponse);
        }
    }
    Ok(url.into())
}

fn request(
    shared: &Shared,
    request: reqwest::blocking::RequestBuilder,
    deadline: Option<Instant>,
) -> Result<(u16, Value), AuthError> {
    shared.check(deadline)?;
    let timeout = deadline.map_or(REQUEST_TIMEOUT, |end| {
        end.saturating_duration_since(Instant::now())
            .min(REQUEST_TIMEOUT)
    });
    if timeout.is_zero() {
        return Err(AuthError::Expired);
    }
    let response = match request.timeout(timeout).send() {
        Ok(response) => response,
        Err(error) => {
            shared.check(deadline)?;
            return Err(if error.is_timeout() {
                AuthError::Timeout
            } else {
                AuthError::Network
            });
        }
    };
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    let read = response
        .take(RESPONSE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes);
    shared.check(deadline)?;
    read.map_err(|_| AuthError::Network)?;
    if bytes.len() > RESPONSE_LIMIT {
        return Err(AuthError::ResponseTooLarge);
    }
    let body = serde_json::from_slice(&bytes).map_err(|_| AuthError::MalformedResponse)?;
    Ok((status, body))
}

fn authorize(
    shared: &Shared,
    client_id: String,
    requested: BTreeSet<String>,
    endpoints: Endpoints,
    grant: Grant,
    no_proxy: bool,
) -> Result<TokenBundle, AuthError> {
    shared.check(None)?;
    let mut builder = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(REQUEST_TIMEOUT);
    // Only the private fixture constructor disables inherited proxy routing.
    if no_proxy {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|_| AuthError::WorkerUnavailable)?;
    let second = match grant {
        Grant::Device { second } => second,
        Grant::Refresh {
            token,
            expected_user,
        } => {
            let (status, response) = request(
                shared,
                client.post(endpoints.token).form(&[
                    ("client_id", client_id.as_str()),
                    ("refresh_token", token.as_str()),
                    ("grant_type", "refresh_token"),
                ]),
                None,
            )?;
            if matches!(status, 400 | 401 | 403) {
                return Err(AuthError::AccessDenied);
            }
            if status != 200 {
                return Err(AuthError::UnexpectedHttpStatus);
            }
            return accept_tokens(
                shared,
                &client,
                client_id,
                &requested,
                response,
                TokenCheck {
                    url: endpoints.validate,
                    expected_user: Some(expected_user),
                    deadline: None,
                },
            );
        }
    };
    let scopes = requested
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let (status, device) = request(
        shared,
        client.post(endpoints.device).form(&[
            ("client_id", client_id.as_str()),
            ("scopes", scopes.as_str()),
        ]),
        None,
    )?;
    if status != 200 {
        return Err(AuthError::UnexpectedHttpStatus);
    }
    let ttl = device["expires_in"]
        .as_u64()
        .filter(|value| (1..=MAX_TTL).contains(value))
        .ok_or(AuthError::InvalidResponse)?;
    let mut interval = device["interval"]
        .as_u64()
        .filter(|value| (1..=MAX_INTERVAL).contains(value))
        .ok_or(AuthError::InvalidResponse)?;
    let deadline = Instant::now() + second * ttl as u32;
    let device_code = secret(&device, "device_code")?;
    let user_code = bounded_string(&device, "user_code", 64)?;
    if !user_code
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(AuthError::InvalidResponse);
    }
    let uri = activation_uri(
        bounded_string(&device, "verification_uri", 2048)?,
        user_code,
    )?;
    shared.phase(
        AuthPhase::AwaitingAuthorization,
        Some(DeviceChallenge {
            user_code: user_code.into(),
            verification_uri: uri,
            expires_at: deadline,
        }),
    )?;
    loop {
        shared.wait(second * interval as u32, deadline)?;
        let (status, token) = request(
            shared,
            client.post(endpoints.token.clone()).form(&[
                ("client_id", client_id.as_str()),
                ("scopes", scopes.as_str()),
                ("device_code", device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ]),
            Some(deadline),
        )?;
        if status == 400 {
            // Twitch uses message rather than the RFC error field for pending.
            match token["error"]
                .as_str()
                .filter(|value| {
                    matches!(
                        *value,
                        "authorization_pending"
                            | "slow_down"
                            | "access_denied"
                            | "expired_token"
                            | "expired_device_code"
                    )
                })
                .or_else(|| token["message"].as_str())
            {
                Some("authorization_pending") => continue,
                Some("slow_down") => {
                    interval = interval
                        .checked_add(5)
                        .filter(|value| *value <= MAX_INTERVAL)
                        .ok_or(AuthError::PollingLimit)?;
                    continue;
                }
                Some("access_denied") => return Err(AuthError::AccessDenied),
                Some("expired_token" | "expired_device_code" | "invalid device code") => {
                    return Err(AuthError::Expired);
                }
                _ => return Err(AuthError::UnexpectedHttpStatus),
            }
        }
        if status == 403 {
            return Err(AuthError::AccessDenied);
        }
        if status != 200 {
            return Err(AuthError::UnexpectedHttpStatus);
        }
        return accept_tokens(
            shared,
            &client,
            client_id,
            &requested,
            token,
            TokenCheck {
                url: endpoints.validate,
                expected_user: None,
                deadline: Some(deadline),
            },
        );
    }
}

struct TokenCheck {
    url: Url,
    expected_user: Option<String>,
    deadline: Option<Instant>,
}

fn access_ttl(body: &Value) -> Result<u64, AuthError> {
    body["expires_in"]
        .as_u64()
        .filter(|value| *value > 0)
        .map(|value| value.min(MAX_ACCESS_TTL))
        .ok_or(AuthError::InvalidResponse)
}

fn accept_tokens(
    shared: &Shared,
    client: &Client,
    client_id: String,
    requested: &BTreeSet<String>,
    token: Value,
    check: TokenCheck,
) -> Result<TokenBundle, AuthError> {
    if token["token_type"]
        .as_str()
        .is_none_or(|value| !value.eq_ignore_ascii_case("bearer"))
    {
        return Err(AuthError::InvalidResponse);
    }
    let token_expires_at = Instant::now() + Duration::from_secs(access_ttl(&token)?);
    let access_token = secret(&token, "access_token")?;
    let refresh_token = secret(&token, "refresh_token")?;
    if !requested.is_subset(&scope_set(&token, "scope")?) {
        return Err(AuthError::MissingScopes);
    }
    shared.phase(AuthPhase::Validating, None)?;
    let mut authorization =
        reqwest::header::HeaderValue::from_str(&format!("OAuth {access_token}"))
            .map_err(|_| AuthError::InvalidResponse)?;
    authorization.set_sensitive(true);
    let (status, validated) = request(
        shared,
        client
            .get(check.url)
            .header(reqwest::header::AUTHORIZATION, authorization),
        check.deadline,
    )?;
    if status == 401 {
        return Err(AuthError::AccessDenied);
    }
    if status != 200 {
        return Err(AuthError::UnexpectedHttpStatus);
    }
    if validated["client_id"].as_str() != Some(client_id.as_str()) {
        return Err(AuthError::ClientMismatch);
    }
    let user_id = validated["user_id"]
        .as_str()
        .filter(|value| valid_user_id(value))
        .ok_or(AuthError::InvalidUser)?;
    if check
        .expected_user
        .as_deref()
        .is_some_and(|expected| expected != user_id)
    {
        return Err(AuthError::UserMismatch);
    }
    let validated_at = Instant::now();
    let expires_in = access_ttl(&validated)?.min(
        token_expires_at
            .saturating_duration_since(validated_at)
            .as_secs(),
    );
    if expires_in == 0 {
        return Err(AuthError::InvalidResponse);
    }
    let granted = scope_set(&validated, "scopes")?;
    if !requested.is_subset(&granted) {
        return Err(AuthError::MissingScopes);
    }
    shared.check(check.deadline)?;
    Ok(TokenBundle {
        client_id,
        user_id: user_id.into(),
        scopes: granted,
        access_token,
        refresh_token,
        expires_in,
        validated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::atomic::{AtomicBool, Ordering},
    };

    struct Reply {
        path: &'static str,
        status: u16,
        body: String,
        release: Option<Arc<AtomicBool>>,
    }
    impl Reply {
        fn json(path: &'static str, status: u16, body: Value) -> Self {
            Self {
                path,
                status,
                body: body.to_string(),
                release: None,
            }
        }
    }
    struct Received {
        path: String,
        headers: String,
        body: String,
        at: Instant,
    }
    struct Server {
        base: String,
        received: Arc<Mutex<Vec<Received>>>,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }
    impl Server {
        fn new(replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let received = Arc::new(Mutex::new(Vec::new()));
            let capture = received.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let cancelled = stop.clone();
            let worker = std::thread::spawn(move || {
                for reply in replies {
                    let mut stream = loop {
                        if cancelled.load(Ordering::Acquire) {
                            return;
                        }
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(1));
                            }
                            Err(_) => panic!("local authorization fixture accept failed"),
                        }
                    };
                    // Windows accepted sockets inherit the listener's mode.
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = BufReader::new(&mut stream);
                    let mut headers = String::new();
                    loop {
                        let mut line = String::new();
                        assert!(reader.read_line(&mut line).unwrap() > 0);
                        assert!(headers.len() + line.len() <= 16 * 1024);
                        if line == "\r\n" {
                            break;
                        }
                        headers.push_str(&line);
                    }
                    let path = headers
                        .lines()
                        .next()
                        .unwrap()
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .to_owned();
                    assert!(path == reply.path, "unexpected fixture endpoint");
                    let length: usize = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map_or(0, |(_, value)| value.trim().parse().unwrap());
                    assert!(length <= 16 * 1024);
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    capture.lock().unwrap().push(Received {
                        path,
                        headers,
                        body: String::from_utf8(body).unwrap(),
                        at: Instant::now(),
                    });
                    let deadline = Instant::now() + Duration::from_secs(2);
                    while reply
                        .release
                        .as_ref()
                        .is_some_and(|release| !release.load(Ordering::Acquire))
                    {
                        if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    if reply.status == 0 {
                        // Simulate an uncertain grant: request consumed, response
                        // lost. A subsequent scripted reply detects any retry.
                        continue;
                    }
                    // Cancellation may close the request before the reply.
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                        reply.status,
                        reply.body.len(),
                        reply.body
                    );
                }
            });
            Self {
                base,
                received,
                stop,
                worker: Some(worker),
            }
        }
        fn endpoints(&self) -> Endpoints {
            Endpoints {
                device: Url::parse(&format!("{}/device", self.base)).unwrap(),
                token: Url::parse(&format!("{}/token", self.base)).unwrap(),
                validate: Url::parse(&format!("{}/validate", self.base)).unwrap(),
            }
        }
        fn begin(&self) -> TwitchAuth {
            TwitchAuth::start(
                "fixtureClient".into(),
                ["user:read:chat".into()],
                self.endpoints(),
                Duration::from_millis(5),
                true,
            )
            .unwrap()
        }
        fn refresh(&self) -> TwitchAuth {
            TwitchAuth::start_refresh(
                "fixtureClient".into(),
                "fixture-old+refresh%/token".into(),
                "123456".into(),
                ["user:read:chat".into()],
                self.endpoints(),
                true,
            )
            .unwrap()
        }
        fn wait_requests(&self, count: usize) {
            let deadline = Instant::now() + Duration::from_secs(4);
            while self.received.lock().unwrap().len() < count {
                assert!(
                    Instant::now() < deadline,
                    "local authorization request missing"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            assert!(
                self.worker.take().unwrap().join().is_ok(),
                "authorization fixture failed"
            );
        }
    }
    fn device() -> Value {
        json!({"device_code":"fixture-private-device", "expires_in":1800, "interval":1,
            "user_code":"ABCD-EFGH", "verification_uri":"https://www.twitch.tv/activate?public=true&device-code=ABCD-EFGH"})
    }
    fn tokens() -> Value {
        json!({"access_token":"fixture-private-access", "refresh_token":"fixture-private-refresh",
            "expires_in":14400, "scope":["user:read:chat"], "token_type":"bearer"})
    }
    fn validation() -> Value {
        json!({"client_id":"fixtureClient", "user_id":"123456", "expires_in":14400, "scopes":["user:read:chat"]})
    }
    fn finish(job: &mut TwitchAuth) -> Result<TokenBundle, AuthError> {
        let deadline = Instant::now() + Duration::from_secs(4);
        while !job.is_finished() {
            assert!(
                Instant::now() < deadline,
                "authorization worker did not finish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let result = job.take_completion().expect("missing one-shot completion");
        assert!(job.take_completion().is_none());
        result
    }

    #[test]
    fn local_device_poll_slowdown_and_validation_return_only_verified_bundle() {
        let server = Server::new(vec![
            Reply::json("/device", 200, device()),
            Reply::json("/token", 400, json!({"message":"authorization_pending"})),
            Reply::json(
                "/token",
                400,
                json!({"error":"slow_down", "message":"wait longer"}),
            ),
            Reply::json("/token", 200, tokens()),
            Reply::json("/validate", 200, validation()),
        ]);
        let mut job = server.begin();
        let bundle = finish(&mut job).unwrap();
        assert_eq!(job.status().phase, AuthPhase::Completed);
        assert!(job.status().challenge.is_none());
        assert_eq!(bundle.client_id(), "fixtureClient");
        assert_eq!(bundle.user_id(), "123456");
        assert!(bundle.scopes().contains("user:read:chat"));
        let updates = bundle.into_credential_updates();
        assert_eq!(updates.len(), 3);
        assert!(updates["access_token"].as_deref() == Some("fixture-private-access"));
        assert!(updates["refresh_token"].as_deref() == Some("fixture-private-refresh"));
        let received = server.received.lock().unwrap();
        assert_eq!(received.len(), 5);
        assert!(received[3].at.duration_since(received[2].at) >= Duration::from_millis(30));
        for request in &received[..4] {
            let mut url = Url::parse("http://fixture/").unwrap();
            url.set_query(Some(&request.body));
            let fields: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
            assert!(fields["client_id"] == "fixtureClient");
            assert!(fields["scopes"] == "user:read:chat");
            if request.path == "/token" {
                assert!(fields["device_code"] == "fixture-private-device");
                assert!(fields["grant_type"] == "urn:ietf:params:oauth:grant-type:device_code");
            }
        }
        assert!(
            received[4]
                .headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .any(|(key, value)| key.eq_ignore_ascii_case("authorization")
                    && value.trim() == "OAuth fixture-private-access")
        );
    }

    #[test]
    fn cancellation_wakes_wait_and_discards_late_or_completed_tokens() {
        let mut long_wait = device();
        long_wait["interval"] = json!(60);
        let server = Server::new(vec![Reply::json("/device", 200, long_wait)]);
        let mut job = server.begin();
        let deadline = Instant::now() + Duration::from_secs(4);
        while job.status().phase != AuthPhase::AwaitingAuthorization {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(job.status().challenge.is_some());
        job.cancel();
        assert!(job.status().challenge.is_none());
        assert!(matches!(finish(&mut job), Err(AuthError::Cancelled)));
        job.cancel();
        assert!(
            job.take_completion().is_none(),
            "repeated cancellation replayed completion"
        );
        assert_eq!(server.received.lock().unwrap().len(), 1);

        let mut delayed = Reply::json("/validate", 200, validation());
        let release = Arc::new(AtomicBool::new(false));
        delayed.release = Some(release.clone());
        let server = Server::new(vec![
            Reply::json("/device", 200, device()),
            Reply::json("/token", 200, tokens()),
            delayed,
        ]);
        let mut job = server.begin();
        server.wait_requests(3);
        job.cancel();
        assert!(
            !job.is_finished(),
            "cancellation must not release an in-flight worker slot"
        );
        release.store(true, Ordering::Release);
        assert!(matches!(finish(&mut job), Err(AuthError::Cancelled)));

        let server = Server::new(vec![
            Reply::json("/device", 200, device()),
            Reply::json("/token", 200, tokens()),
            Reply::json("/validate", 200, validation()),
        ]);
        let mut job = server.begin();
        let deadline = Instant::now() + Duration::from_secs(4);
        while !job.is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        job.cancel();
        assert!(matches!(
            job.take_completion(),
            Some(Err(AuthError::Cancelled))
        ));
    }

    #[test]
    fn local_validation_rejects_other_client_invalid_user_and_missing_scopes() {
        for (field, value, expected) in [
            (
                "client_id",
                json!("anotherClient"),
                AuthError::ClientMismatch,
            ),
            ("user_id", json!(null), AuthError::InvalidUser),
            ("user_id", json!("123 private"), AuthError::InvalidUser),
            ("scopes", json!([]), AuthError::MissingScopes),
            (
                "scopes",
                json!(["user:read:chat", 4]),
                AuthError::InvalidResponse,
            ),
            ("expires_in", json!(0), AuthError::InvalidResponse),
        ] {
            let mut body = validation();
            body[field] = value;
            let server = Server::new(vec![
                Reply::json("/device", 200, device()),
                Reply::json("/token", 200, tokens()),
                Reply::json("/validate", 200, body),
            ]);
            let mut job = server.begin();
            assert!(matches!(finish(&mut job), Err(error) if error == expected));
            assert_eq!(job.status().error, Some(expected));
            assert_eq!(job.status().phase, AuthPhase::Failed);
        }
    }

    #[test]
    fn local_denial_expiry_bad_json_and_oversize_have_fixed_errors() {
        for (reply, expected) in [
            (
                Reply::json(
                    "/token",
                    400,
                    json!({"error":"access_denied", "message":"private provider detail"}),
                ),
                AuthError::AccessDenied,
            ),
            (
                Reply::json("/token", 400, json!({"message":"expired_token"})),
                AuthError::Expired,
            ),
            (
                Reply::json("/token", 400, json!({"message":"invalid device code"})),
                AuthError::Expired,
            ),
            (
                Reply::json("/token", 500, json!({"message":"fixture-private-access"})),
                AuthError::UnexpectedHttpStatus,
            ),
            (
                Reply {
                    path: "/token",
                    status: 200,
                    body: "fixture-private-access".into(),
                    release: None,
                },
                AuthError::MalformedResponse,
            ),
            (
                Reply {
                    path: "/token",
                    status: 200,
                    body: "x".repeat(RESPONSE_LIMIT + 1),
                    release: None,
                },
                AuthError::ResponseTooLarge,
            ),
        ] {
            let server = Server::new(vec![Reply::json("/device", 200, device()), reply]);
            let mut job = server.begin();
            assert!(matches!(finish(&mut job), Err(error) if error == expected));
            let display = job.status().error.unwrap().to_string();
            assert!(!display.contains("private"));
            assert!(display.len() <= 128);
        }
        let mut body = device();
        body["expires_in"] = json!(1);
        let server = Server::new(vec![Reply::json("/device", 200, body)]);
        assert!(matches!(
            finish(&mut server.begin()),
            Err(AuthError::Expired)
        ));
        assert_eq!(
            server.received.lock().unwrap().len(),
            1,
            "polled beyond expiry"
        );
    }

    #[test]
    fn activation_url_is_exact_https_twitch_path_without_credentials_or_fragment() {
        for bad in [
            "http://www.twitch.tv/activate",
            "https://twitch.tv.attacker.invalid/activate",
            "https://name@twitch.tv/activate",
            "https://@twitch.tv/activate",
            "https://twitch.tv:444/activate",
            "https://twitch.tv/activate#code",
            "https://twitch.tv/other",
            "https://twitch.tv/activate/",
            "https://twitch.tv/activate?device-code=DIFFERENT",
            "https://twitch.tv/activate?access_token=private",
            "https://twitch.tv/activate?public=true&public=true",
        ] {
            assert!(activation_uri(bad, "ABCD-EFGH").is_err());
        }
        assert!(activation_uri("https://twitch.tv:443/activate", "ABCD-EFGH").is_ok());
        assert!(
            activation_uri(device()["verification_uri"].as_str().unwrap(), "ABCD-EFGH").is_ok()
        );
        let mut bad = device();
        bad["verification_uri"] = json!("https://attacker.invalid/activate");
        let server = Server::new(vec![Reply::json("/device", 200, bad)]);
        assert!(matches!(
            finish(&mut server.begin()),
            Err(AuthError::InvalidResponse)
        ));
    }

    #[test]
    fn inputs_server_ttl_interval_and_token_fields_are_bounded_before_use() {
        assert!(matches!(
            TwitchAuth::begin("not an ID".into(), ["user:read:chat".into()]),
            Err(AuthError::InvalidClientId)
        ));
        assert!(matches!(
            TwitchAuth::begin("valid".into(), ["read\nsecret".into()]),
            Err(AuthError::InvalidScopes)
        ));
        assert!(matches!(
            TwitchAuth::begin(
                "valid".into(),
                std::iter::repeat_with(|| "user:read:chat".to_owned())
            ),
            Err(AuthError::InvalidScopes)
        ));
        for (field, value) in [
            ("expires_in", 0),
            ("expires_in", MAX_TTL + 1),
            ("interval", 0),
            ("interval", MAX_INTERVAL + 1),
        ] {
            let mut bad = device();
            bad[field] = json!(value);
            let server = Server::new(vec![Reply::json("/device", 200, bad)]);
            assert!(matches!(
                finish(&mut server.begin()),
                Err(AuthError::InvalidResponse)
            ));
        }
        for (field, value, expected) in [
            (
                "access_token",
                json!("x".repeat(SECRET_LIMIT + 1)),
                AuthError::InvalidResponse,
            ),
            ("refresh_token", json!(""), AuthError::InvalidResponse),
            ("token_type", json!("other"), AuthError::InvalidResponse),
            ("scope", json!([]), AuthError::MissingScopes),
        ] {
            let mut bad = tokens();
            bad[field] = value;
            let server = Server::new(vec![
                Reply::json("/device", 200, device()),
                Reply::json("/token", 200, bad),
            ]);
            assert!(matches!(finish(&mut server.begin()), Err(error) if error == expected));
            assert_eq!(
                server.received.lock().unwrap().len(),
                2,
                "invalid tokens reached validation"
            );
        }
    }

    #[test]
    fn refresh_posts_once_and_returns_same_account_rotated_pair_with_conservative_expiry() {
        let mut token = tokens();
        token["expires_in"] = json!(7200);
        let mut checked = validation();
        checked["expires_in"] = json!(3600);
        let server = Server::new(vec![
            Reply::json("/token", 200, token),
            Reply::json("/validate", 200, checked),
        ]);
        let mut job = server.refresh();
        assert!(job.status().challenge.is_none());
        let bundle = finish(&mut job).unwrap();
        assert_eq!(job.status().phase, AuthPhase::Completed);
        assert!(job.status().challenge.is_none());
        assert_eq!(bundle.user_id(), "123456");
        assert_eq!(bundle.client_id(), "fixtureClient");
        assert_eq!(bundle.expires_in(), 3600);
        assert_eq!(
            bundle.expires_at(),
            bundle.validated_at() + Duration::from_secs(bundle.expires_in())
        );
        let updates = bundle.into_credential_updates();
        assert!(updates["refresh_token"].as_deref() == Some("fixture-private-refresh"));
        let received = server.received.lock().unwrap();
        assert_eq!(received.len(), 2);
        assert!(received[0].path == "/token" && received[1].path == "/validate");
        let mut form = Url::parse("http://fixture/").unwrap();
        form.set_query(Some(&received[0].body));
        let fields: BTreeMap<_, _> = form.query_pairs().into_owned().collect();
        assert_eq!(fields.len(), 3);
        assert!(fields["client_id"] == "fixtureClient");
        assert!(fields["grant_type"] == "refresh_token");
        assert!(fields["refresh_token"] == "fixture-old+refresh%/token");
    }

    #[test]
    fn refresh_expiry_uses_both_responses_and_caps_untrusted_lifetimes() {
        for (token_ttl, validated_ttl, cap) in [
            (60, 3600, 60),
            (3600, 60, 60),
            (u64::MAX, u64::MAX, MAX_ACCESS_TTL),
        ] {
            let mut token = tokens();
            token["expires_in"] = json!(token_ttl);
            let mut checked = validation();
            checked["expires_in"] = json!(validated_ttl);
            let server = Server::new(vec![
                Reply::json("/token", 200, token),
                Reply::json("/validate", 200, checked),
            ]);
            let bundle = finish(&mut server.refresh()).unwrap();
            assert!(bundle.expires_in() > 0 && bundle.expires_in() <= cap);
            assert!(
                bundle.expires_at() <= bundle.validated_at() + Duration::from_secs(MAX_ACCESS_TTL)
            );
        }
    }

    #[test]
    fn refresh_rejects_different_account_client_or_missing_scopes_without_another_grant() {
        for (field, value, expected) in [
            ("user_id", json!("999999"), AuthError::UserMismatch),
            ("user_id", json!(null), AuthError::InvalidUser),
            (
                "client_id",
                json!("anotherClient"),
                AuthError::ClientMismatch,
            ),
            ("scopes", json!([]), AuthError::MissingScopes),
            ("expires_in", json!(0), AuthError::InvalidResponse),
        ] {
            let mut checked = validation();
            checked[field] = value;
            let server = Server::new(vec![
                Reply::json("/token", 200, tokens()),
                Reply::json("/validate", 200, checked),
            ]);
            let mut job = server.refresh();
            assert!(matches!(finish(&mut job), Err(error) if error == expected));
            assert!(job.status().challenge.is_none());
            assert_eq!(
                server
                    .received
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|request| request.path == "/token")
                    .count(),
                1
            );
        }
        let mut token = tokens();
        token["scope"] = json!([]);
        let server = Server::new(vec![Reply::json("/token", 200, token)]);
        assert!(matches!(
            finish(&mut server.refresh()),
            Err(AuthError::MissingScopes)
        ));
        assert_eq!(server.received.lock().unwrap().len(), 1);
    }

    #[test]
    fn refresh_uncertain_network_malformed_http_and_bounds_fail_without_repeating_grant() {
        for (reply, expected) in [
            (Reply::json("/token", 0, json!({})), AuthError::Network),
            (
                Reply::json("/token", 400, json!({"message":"private refresh detail"})),
                AuthError::AccessDenied,
            ),
            (
                Reply::json("/token", 401, json!({"message":"private token detail"})),
                AuthError::AccessDenied,
            ),
            (
                Reply::json("/token", 503, json!({"message":"private provider detail"})),
                AuthError::UnexpectedHttpStatus,
            ),
            (
                Reply {
                    path: "/token",
                    status: 200,
                    body: "private invalid JSON".into(),
                    release: None,
                },
                AuthError::MalformedResponse,
            ),
            (
                Reply {
                    path: "/token",
                    status: 200,
                    body: "x".repeat(RESPONSE_LIMIT + 1),
                    release: None,
                },
                AuthError::ResponseTooLarge,
            ),
            (
                Reply::json(
                    "/token",
                    200,
                    json!({"access_token":"private incomplete token"}),
                ),
                AuthError::InvalidResponse,
            ),
        ] {
            let server = Server::new(vec![
                reply,
                Reply::json("/token", 500, json!({"message":"must not retry"})),
            ]);
            let mut job = server.refresh();
            assert!(matches!(finish(&mut job), Err(error) if error == expected));
            assert_eq!(job.status().phase, AuthPhase::Failed);
            assert!(!job.status().error.unwrap().to_string().contains("private"));
            assert_eq!(
                server.received.lock().unwrap().len(),
                1,
                "rotating refresh grant retried"
            );
        }
    }

    #[test]
    fn refresh_cancellation_discards_late_tokens_and_validation_without_releasing_job_early() {
        for hold_validation in [false, true] {
            let release = Arc::new(AtomicBool::new(false));
            let mut token = Reply::json("/token", 200, tokens());
            let mut checked = Reply::json("/validate", 200, validation());
            if hold_validation {
                checked.release = Some(release.clone());
            } else {
                token.release = Some(release.clone());
            }
            let server = Server::new(vec![token, checked]);
            let mut job = server.refresh();
            server.wait_requests(if hold_validation { 2 } else { 1 });
            assert!(job.status().challenge.is_none());
            job.cancel();
            assert!(!job.is_finished());
            release.store(true, Ordering::Release);
            assert!(matches!(finish(&mut job), Err(AuthError::Cancelled)));
            assert_eq!(
                server.received.lock().unwrap().len(),
                if hold_validation { 2 } else { 1 }
            );
        }
    }

    #[test]
    fn refresh_input_identity_and_secret_are_bounded_before_spawning() {
        assert!(matches!(
            TwitchAuth::refresh(
                "valid".into(),
                "offline".into(),
                "other user".into(),
                ["user:read:chat".into()]
            ),
            Err(AuthError::InvalidUser)
        ));
        for invalid in [
            String::new(),
            "x".repeat(SECRET_LIMIT + 1),
            "secret\nline".into(),
        ] {
            assert!(matches!(
                TwitchAuth::refresh(
                    "valid".into(),
                    invalid,
                    "123456".into(),
                    ["user:read:chat".into()]
                ),
                Err(AuthError::InvalidResponse)
            ));
        }
    }
}
