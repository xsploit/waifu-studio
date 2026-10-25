//! Native receive-only Twitch adapter, launched as an isolated extension child.
//! Token values remain in the integration's private file and HTTP headers.
use crate::{
    extension_credentials::{self, CredentialDefinition},
    extension_protocol::*,
    extension_settings::SettingValue,
};
use serde_json::{Value, json};
use std::net::TcpStream;
use std::{
    collections::{BTreeSet, VecDeque},
    io::{self, BufRead, Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tungstenite::{Message, WebSocket, stream::MaybeTlsStream};

const EVENT_URL: &str = "wss://eventsub.wss.twitch.tv/ws";
const TYPES: [&str; 3] = ["channel.subscribe", "channel.cheer", "channel.chat.message"];
const SCOPES: [&str; 3] = ["channel:read:subscriptions", "bits:read", "user:read:chat"];
const DEFAULT_CELEBRATION_MOTION: &str = "studio.integration.twitch/nod";

/// Event selection shared by authorization and the receive-only worker.
/// Unknown/missing legacy values preserve the original all-events default.
pub fn selected_event_types(settings: &crate::extension_settings::Settings) -> Vec<&'static str> {
    match settings.get("events") {
        Some(SettingValue::Text(value)) if value == "Alerts only" => TYPES[..2].to_vec(),
        Some(SettingValue::Text(value)) if value == "Chat only" => vec![TYPES[2]],
        _ => TYPES.to_vec(),
    }
}

/// Consent must request exactly the scopes the saved event selection needs.
pub fn required_scopes(settings: &crate::extension_settings::Settings) -> Vec<&'static str> {
    selected_event_types(settings)
        .into_iter()
        .map(|kind| {
            SCOPES[TYPES
                .iter()
                .position(|candidate| *candidate == kind)
                .unwrap()]
        })
        .collect()
}

fn celebration_motion(
    settings: &crate::extension_settings::Settings,
) -> Result<Option<String>, String> {
    match settings.get("celebration_motion") {
        None => Ok(Some(DEFAULT_CELEBRATION_MOTION.into())),
        Some(SettingValue::Text(id)) if id.is_empty() => Ok(None),
        Some(SettingValue::Text(id))
            if id.len() <= MAX_REGISTERED_ID_BYTES
                && !id.chars().any(|c| c.is_whitespace() || c.is_control()) =>
        {
            // This is an opaque registered motion ID. Only the host catalog
            // resolves it; the child never opens it or treats it as a path.
            Ok(Some(id.clone()))
        }
        _ => Err(format!(
            "Invalid Twitch celebration_motion: expected an empty string or an ID of at most {MAX_REGISTERED_ID_BYTES} bytes without whitespace or controls"
        )),
    }
}

fn celebration_expression(
    settings: &crate::extension_settings::Settings,
) -> Result<Option<String>, String> {
    match settings.get("celebration_expression") {
        None => Ok(None),
        Some(SettingValue::Text(name)) if name.is_empty() => Ok(None),
        Some(SettingValue::Text(name))
            if name.len() <= MAX_ID_BYTES
                && !name.trim().is_empty()
                && !name.chars().any(char::is_control)
                && !crate::behavior::reserved_expression(name) =>
        {
            Ok(Some(name.clone()))
        }
        _ => Err(format!(
            "Invalid Twitch celebration_expression: expected an empty string or a nonblank, unreserved expression name of at most {MAX_ID_BYTES} bytes without controls"
        )),
    }
}

struct ReactionSettings {
    motion: Option<String>,
    expression: Option<String>,
    chunk_chat: bool,
}

pub fn credential_definitions() -> Vec<CredentialDefinition> {
    vec![
        CredentialDefinition {
            id: "client_id".into(),
            label: "Twitch application ID".into(),
        },
        CredentialDefinition {
            id: "access_token".into(),
            label: "Twitch user access token".into(),
        },
        CredentialDefinition {
            id: "refresh_token".into(),
            label: "Twitch refresh token".into(),
        },
    ]
}

struct Api {
    client: reqwest::blocking::Client,
    client_id: String,
    token: String,
    kinds: Vec<&'static str>,
    validate_url: reqwest::Url,
    subscribe_url: reqwest::Url,
}
impl Api {
    fn new(client_id: String, token: String, kinds: Vec<&'static str>) -> Result<Self, String> {
        for value in [&client_id, &token] {
            if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
                return Err("Invalid Twitch credential format".into());
            }
        }
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| "Cannot create Twitch HTTP client")?;
        Ok(Self {
            client,
            client_id,
            token,
            kinds,
            validate_url: reqwest::Url::parse("https://id.twitch.tv/oauth2/validate").unwrap(),
            subscribe_url: reqwest::Url::parse(
                "https://api.twitch.tv/helix/eventsub/subscriptions",
            )
            .unwrap(),
        })
    }
    fn validate(&self) -> Result<String, String> {
        let response = self
            .client
            .get(self.validate_url.clone())
            .header("Authorization", format!("OAuth {}", self.token))
            .send()
            .map_err(|_| "Twitch token validation could not connect")?;
        let body = bounded_response(response)?;
        if body["client_id"].as_str() != Some(self.client_id.as_str()) {
            return Err("Twitch token belongs to another application".into());
        }
        let scopes: BTreeSet<_> = body["scopes"]
            .as_array()
            .ok_or("Twitch token has no scopes")?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        if self.kinds.iter().any(|kind| {
            let index = TYPES.iter().position(|value| value == kind).unwrap();
            !scopes.contains(SCOPES[index])
        }) {
            return Err("Twitch token is missing a required event scope".into());
        }
        let id = body["user_id"]
            .as_str()
            .ok_or("Twitch requires a user access token")?;
        if id.is_empty() || id.len() > 32 || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Twitch returned an invalid account identity".into());
        }
        Ok(id.into())
    }
    fn subscribe(&self, session: &str, broadcaster: &str) -> Result<(), String> {
        for kind in &self.kinds {
            let mut condition = json!({"broadcaster_user_id": broadcaster});
            if *kind == "channel.chat.message" {
                condition["user_id"] = json!(broadcaster);
            }
            let response = self
                .client
                .post(self.subscribe_url.clone())
                .bearer_auth(&self.token)
                .header("Client-Id", &self.client_id)
                .json(&json!({"type": kind,"version":"1","condition":condition,
                    "transport":{"method":"websocket","session_id":session}}))
                .send()
                .map_err(|_| "Twitch subscription request could not connect")?;
            let _ = bounded_response(response)?;
        }
        Ok(())
    }
}
fn bounded_response(response: reqwest::blocking::Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!(
            "Twitch returned HTTP {}; check account permissions and credentials",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read Twitch response")?;
    if bytes.len() > 65536 {
        return Err("Twitch response exceeded its size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Twitch returned malformed JSON".into())
}
fn host_line(reader: &mut impl BufRead) -> Result<Option<HostEnvelope>, String> {
    let mut bytes = Vec::new();
    let length = reader
        .take(65538)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| "Cannot read host message")?;
    if length == 0 {
        return Ok(None);
    }
    if !bytes.ends_with(b"\n") {
        return Err("Host message exceeded its line limit".into());
    }
    parse_host_line(&bytes)
        .map(Some)
        .map_err(|_| "Invalid host message".into())
}
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionStage {
    Message,
    Expression,
    Motion,
}

const MAX_NOTIFICATIONS: usize = 16;
const NOTIFICATION_LIFETIME: Duration = Duration::from_secs(15);
const BUSY_RETRY_DELAY: Duration = Duration::from_secs(1);
const MAX_DELIVERY_ATTEMPTS: u8 = 3;

struct Notification {
    text: String,
    chat: Option<ChatMessage>,
    celebrate: bool,
    target: Target,
    expires: Instant,
    not_before: Instant,
    attempts: u8,
}

struct Host {
    session: uuid::Uuid,
    generation: u64,
    grants: BTreeSet<Capability>,
    target: Option<Target>,
    sequence: u64,
    pending: Option<(String, Target, Instant, ActionStage)>,
    gesture: bool,
    last_gesture: Option<Instant>,
    celebration_motion: Option<String>,
    celebration_expression: Option<String>,
    chunk_chat: bool,
    queued: VecDeque<Notification>,
    delivery: Option<Notification>,
    output: Box<dyn Write>,
}
impl Host {
    fn action(&mut self, action: Action, target: Target) -> Result<String, String> {
        let stage = match &action {
            Action::MessageDeliver { .. } | Action::ChatOffer { .. } => ActionStage::Message,
            Action::ExpressionApply { .. } => ActionStage::Expression,
            Action::MotionPlay { .. } => ActionStage::Motion,
            _ => return Err("Unsupported Twitch reaction action".into()),
        };
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or("Action sequence exhausted")?;
        let id = format!("twitch-{}", self.sequence);
        let envelope = ActionEnvelope {
            api_version: API_VERSION,
            message_type: ActionEnvelopeType::Action,
            id: id.clone(),
            sequence: self.sequence,
            session_id: self.session,
            process_generation: self.generation,
            target,
            action,
        };
        let bytes = encode_action_line(&envelope).map_err(|_| "Cannot encode Twitch action")?;
        self.output
            .write_all(&bytes)
            .and_then(|()| self.output.flush())
            .map_err(|_| "Host output closed")?;
        self.pending = Some((id.clone(), target, Instant::now(), stage));
        Ok(id)
    }

    fn clear_reaction(&mut self) {
        self.pending = None;
        self.delivery = None;
        self.gesture = false;
    }

    fn expire_pending(&mut self, now: Instant) -> bool {
        if self.pending.as_ref().is_some_and(|(_, _, at, _)| {
            now.saturating_duration_since(*at) > Duration::from_secs(10)
        }) || self
            .delivery
            .as_ref()
            .is_some_and(|note| note.expires <= now)
        {
            self.clear_reaction();
            true
        } else {
            false
        }
    }

    fn waiting_for_motion(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|(_, _, _, stage)| *stage == ActionStage::Motion)
    }
    fn notification(&mut self, text: String, celebrate: bool) -> Result<(), String> {
        self.enqueue_notification(text, celebrate, None)
    }
    fn chat_notification(&mut self, chat: ChatMessage, formatted: String) -> Result<(), String> {
        if !self.chunk_chat {
            return self.notification(formatted, false);
        }
        if chat.validate().is_err() {
            eprintln!("Twitch chat dropped: candidate exceeds structured intake bounds");
            return Ok(());
        }
        self.enqueue_notification(chat.text.clone(), false, Some(chat))
    }
    fn enqueue_notification(
        &mut self,
        text: String,
        celebrate: bool,
        chat: Option<ChatMessage>,
    ) -> Result<(), String> {
        let Some(target) = self.target else {
            eprintln!("Twitch event dropped: no loaded character");
            return Ok(());
        };
        let now = Instant::now();
        let before = self.queued.len();
        self.queued
            .retain(|note| note.expires > now && Some(note.target) == self.target);
        if before != self.queued.len() {
            eprintln!("Twitch events dropped: queued notifications expired or target changed");
        }
        if text.is_empty() || text.len() > MAX_TEXT_BYTES || text.contains('\0') {
            eprintln!("Twitch event dropped: invalid notification text");
            return Ok(());
        }
        // Includes the in-flight message: at most 16 * 4096 text bytes.
        if self.queued.len() + usize::from(self.delivery.is_some()) >= MAX_NOTIFICATIONS {
            eprintln!("Twitch event dropped: notification queue full");
            return Ok(());
        }
        self.queued.push_back(Notification {
            text,
            chat,
            celebrate,
            target,
            expires: now + NOTIFICATION_LIFETIME,
            not_before: now,
            attempts: 0,
        });
        self.dispatch_notification(now)
    }

    fn dispatch_notification(&mut self, now: Instant) -> Result<(), String> {
        if self.pending.is_some() || self.delivery.is_some() {
            return Ok(());
        }
        while let Some(note) = self.queued.front() {
            if note.expires <= now || Some(note.target) != self.target {
                self.queued.pop_front();
                eprintln!("Twitch event dropped: queued notification expired or target changed");
                continue;
            }
            if note.not_before > now {
                return Ok(());
            }
            let mut note = self.queued.pop_front().expect("checked queue front");
            self.gesture = note.celebrate;
            if self.grants.contains(&Capability::MessageDeliver) {
                note.attempts += 1;
                let action = if let Some(message) = &note.chat {
                    Action::ChatOffer {
                        message: message.clone(),
                    }
                } else {
                    Action::MessageDeliver {
                        text: note.text.clone(),
                    }
                };
                self.action(action, note.target)?;
                self.delivery = Some(note);
            } else {
                self.start_reaction(note.target)?;
            }
            break;
        }
        Ok(())
    }

    fn reject_busy_delivery(&mut self, now: Instant) {
        let note = self.delivery.take();
        self.pending = None;
        self.gesture = false;
        if let Some(mut note) = note
            && note.attempts < MAX_DELIVERY_ATTEMPTS
            && note.expires > now + BUSY_RETRY_DELAY
            && Some(note.target) == self.target
        {
            note.not_before = now + BUSY_RETRY_DELAY;
            self.queued.push_front(note);
            eprintln!("Twitch delivery deferred: host busy; bounded retry pending");
        } else {
            eprintln!("Twitch event dropped: busy delivery exhausted or expired");
        }
    }
    fn start_reaction(&mut self, target: Target) -> Result<(), String> {
        let requested = std::mem::take(&mut self.gesture);
        if !requested
            || Some(target) != self.target
            || self
                .last_gesture
                .is_some_and(|at| at.elapsed() < Duration::from_secs(3))
        {
            return Ok(());
        }
        let expression = self
            .celebration_expression
            .clone()
            .filter(|_| self.grants.contains(&Capability::ExpressionApply));
        let motion_permitted =
            self.celebration_motion.is_some() && self.grants.contains(&Capability::MotionPlay);
        if expression.is_none() && !motion_permitted {
            return Ok(());
        }
        // Admit the whole reaction once, before its first permitted action.
        // Expression acknowledgement advances to motion without another timer.
        self.last_gesture = Some(Instant::now());
        if let Some(name) = expression {
            self.action(
                Action::ExpressionApply {
                    name,
                    strength: 0.75,
                    duration_ms: 2000,
                },
                target,
            )?;
        } else {
            self.play_motion(target)?;
        }
        Ok(())
    }

    fn play_motion(&mut self, target: Target) -> Result<(), String> {
        if Some(target) == self.target
            && self.grants.contains(&Capability::MotionPlay)
            && let Some(motion_id) = self.celebration_motion.clone()
        {
            self.action(
                Action::MotionPlay {
                    motion_id,
                    mode: PlaybackMode::OneShot,
                },
                target,
            )?;
        }
        Ok(())
    }
    fn message(&mut self, envelope: HostEnvelope) -> Result<(), String> {
        if envelope.session_id != self.session || envelope.process_generation != self.generation {
            return Err("Host session changed".into());
        }
        if self.expire_pending(Instant::now()) {
            eprintln!("Twitch action expired before host result; no automatic replay");
        }
        match envelope.message {
            HostMessage::Event {
                event: HostEvent::TargetChanged { target },
            } => {
                if self.target != target {
                    self.clear_reaction();
                    self.queued.clear();
                }
                self.target = target;
            }
            HostMessage::Result { id, target, result } => {
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|(expected, captured, _, _)| {
                        expected == &id && *captured == target
                    })
                {
                    let message_stage = self.pending.as_ref().expect("matching pending action").3
                        == ActionStage::Message;
                    if message_stage
                        && Some(target) == self.target
                        && matches!(
                            result,
                            ActionResult::Error {
                                code: ErrorCode::Busy,
                                ..
                            }
                        )
                    {
                        // Busy is an explicit pre-commit refusal. Never retry an
                        // accepted, timed-out, disconnected or otherwise uncertain delivery.
                        self.reject_busy_delivery(Instant::now());
                    } else if matches!(result, ActionResult::Ok { .. })
                        && Some(target) == self.target
                    {
                        match self.pending.as_ref().expect("matching pending action").3 {
                            ActionStage::Message => {
                                self.delivery = None;
                                self.pending = None;
                                self.start_reaction(target)?;
                            }
                            ActionStage::Expression => {
                                self.pending = None;
                                self.play_motion(target)?;
                            }
                            ActionStage::Motion => {} // Acceptance is not terminal.
                        }
                    } else {
                        self.clear_reaction();
                        eprintln!(
                            "Twitch action rejected; remaining reaction dropped, no automatic replay"
                        );
                    }
                }
            }
            HostMessage::Event {
                event:
                    HostEvent::MotionFinished {
                        target,
                        action_id,
                        outcome,
                        ..
                    },
            } if self.waiting_for_motion()
                && self.pending.as_ref().is_some_and(|(id, captured, _, _)| {
                    *id == action_id && *captured == target
                }) =>
            {
                self.clear_reaction();
                eprintln!("Twitch gesture terminal: {outcome:?}");
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn run() -> Result<(), String> {
    let mut reader = io::BufReader::new(io::stdin());
    let handshake = host_line(&mut reader)?.ok_or("Missing integration handshake")?;
    let HostMessage::Handshake {
        capabilities,
        settings,
        ..
    } = handshake.message
    else {
        return Err("Expected integration handshake".into());
    };
    if !capabilities.contains(&Capability::LifecycleObserve) {
        return Err("Twitch needs observation permission for target changes".into());
    }
    if settings.get("active") == Some(&SettingValue::Bool(false)) {
        return Ok(());
    }
    let reaction = ReactionSettings {
        motion: celebration_motion(&settings)?,
        expression: celebration_expression(&settings)?,
        chunk_chat: match settings.get("chat_mode") {
            None => true,
            Some(SettingValue::Text(value)) if value == "Chunks" => true,
            Some(SettingValue::Text(value)) if value == "Direct" => false,
            _ => return Err("Invalid Twitch chat intake setting".into()),
        },
    };
    let data = PathBuf::from(
        std::env::var_os("WAIFU_EXTENSION_DATA").ok_or("Missing integration data directory")?,
    );
    let mut secrets = extension_credentials::read(&data, &credential_definitions())?;
    let client_id = secrets
        .remove("client_id")
        .ok_or("Set the Twitch application ID in Credentials")?;
    let token = secrets
        .remove("access_token")
        .ok_or("Set the Twitch user access token in Credentials")?;
    let kinds = selected_event_types(&settings);
    let api = Api::new(client_id, token, kinds)?;
    let broadcaster = api.validate()?;
    run_connected(
        reader,
        handshake.session_id,
        handshake.process_generation,
        capabilities,
        api,
        broadcaster,
        reaction,
    )
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

#[derive(Debug, PartialEq, Eq)]
enum Received {
    Idle,
    Control,
    /// A validated EventSub envelope, including a suppressed duplicate.
    Event(Option<crate::twitch_eventsub::Effect>),
}

impl Received {
    fn record_activity(&self, last_event: &mut Instant, failures: &mut u32, now: Instant) {
        // Ping/Pong and socket timeouts do not satisfy Twitch's EventSub timer.
        if matches!(self, Self::Event(_)) {
            *last_event = now;
            *failures = 0;
        }
    }
}

fn receive(
    socket: &mut Socket,
    reducer: &mut crate::twitch_eventsub::EventSubReducer,
) -> Result<Received, String> {
    receive_at(socket, reducer, unix_now())
}

fn receive_at(
    socket: &mut Socket,
    reducer: &mut crate::twitch_eventsub::EventSubReducer,
    now_unix_secs: u64,
) -> Result<Received, String> {
    match socket.read() {
        Ok(Message::Text(text)) => reducer
            .receive(text.as_str(), now_unix_secs)
            .map(Received::Event),
        Ok(Message::Ping(_)) => {
            socket.flush().map_err(|_| "Twitch pong failed")?;
            Ok(Received::Control)
        }
        Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => Ok(Received::Control),
        Ok(Message::Close(_)) => Err("Twitch connection closed".into()),
        Ok(Message::Binary(_)) => Err("Unexpected binary Twitch message".into()),
        Err(tungstenite::Error::Io(error))
            if matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ) =>
        {
            Ok(Received::Idle)
        }
        Err(_) => Err("Twitch WebSocket read failed".into()),
    }
}
fn welcome(
    socket: &mut Socket,
    reducer: &mut crate::twitch_eventsub::EventSubReducer,
) -> Result<(String, u64), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match receive(socket, reducer)? {
            Received::Event(Some(crate::twitch_eventsub::Effect::Welcome {
                session_id,
                keepalive_timeout_seconds,
            })) => return Ok((session_id, keepalive_timeout_seconds)),
            Received::Event(_) => {
                return Err("Twitch sent an event before its welcome".into());
            }
            Received::Idle | Received::Control => {}
        }
    }
    Err("Twitch welcome timed out".into())
}

/// Only an idle read ends a successful drain. Duplicates/control frames can be
/// followed by unread notifications; all terminal effects and failures surface.
fn drain_old_socket(
    host: &mut Host,
    mut next: impl FnMut() -> Result<Received, String>,
) -> Result<(), String> {
    use crate::twitch_eventsub::Effect;
    for _ in 0..256 {
        match next().map_err(|error| {
            format!("Twitch reconnect drain failed; events may be lost: {error}")
        })? {
            Received::Idle => return Ok(()),
            Received::Control
            | Received::Event(None)
            | Received::Event(Some(Effect::Keepalive)) => {}
            Received::Event(Some(Effect::Notification {
                text,
                celebrate,
                chat,
                ..
            })) => {
                if let Some(chat) = chat {
                    host.chat_notification(chat, text)?;
                } else {
                    host.notification(text, celebrate)?;
                }
            }
            Received::Event(Some(Effect::Revoked { .. })) => {
                return Err(
                    "Twitch subscription revoked during reconnect; update Credentials".into(),
                );
            }
            Received::Event(Some(Effect::Welcome { .. } | Effect::Reconnect { .. })) => {
                return Err("Unexpected Twitch session event during reconnect drain".into());
            }
        }
    }
    Err("Twitch reconnect drain exceeded its bound; events may be lost".into())
}

const INPUT_RUNNING: u8 = 0;
const INPUT_EOF: u8 = 1;
const INPUT_FAILED: u8 = 2;
const INPUT_OVERFLOW: u8 = 3;

fn read_host_input(
    mut reader: impl BufRead,
    tx: mpsc::SyncSender<HostEnvelope>,
    status: &AtomicU8,
) {
    loop {
        let message = match host_line(&mut reader) {
            Ok(Some(message)) => message,
            Ok(None) => {
                status.store(INPUT_EOF, Ordering::Release);
                return;
            }
            Err(_) => {
                status.store(INPUT_FAILED, Ordering::Release);
                return;
            }
        };
        match tx.try_send(message) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                // This out-of-band flag survives a full channel, unlike an
                // attempted terminal message sent through the same queue.
                status.store(INPUT_OVERFLOW, Ordering::Release);
                return;
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return,
        }
    }
}

fn check_host_input(status: &AtomicU8) -> Result<(), String> {
    match status.load(Ordering::Acquire) {
        INPUT_RUNNING | INPUT_EOF => Ok(()),
        INPUT_OVERFLOW => Err("Twitch host input queue overflowed; messages were lost".into()),
        _ => Err("Twitch host input failed; connection stopped".into()),
    }
}

/// Returns false only for normal EOF after queued messages have been consumed.
fn poll_host_input(
    rx: &mpsc::Receiver<HostEnvelope>,
    status: &AtomicU8,
    host: &mut Host,
) -> Result<bool, String> {
    check_host_input(status)?;
    for _ in 0..32 {
        match rx.try_recv() {
            Ok(message) => {
                check_host_input(status)?;
                host.message(message)?;
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                check_host_input(status)?;
                return if status.load(Ordering::Acquire) == INPUT_EOF {
                    Ok(false)
                } else {
                    Err("Twitch host input disconnected unexpectedly".into())
                };
            }
        }
    }
    check_host_input(status)?;
    Ok(true)
}

fn run_connected(
    reader: io::BufReader<io::Stdin>,
    session: uuid::Uuid,
    generation: u64,
    grants: Vec<Capability>,
    api: Api,
    broadcaster: String,
    reaction: ReactionSettings,
) -> Result<(), String> {
    use crate::twitch_eventsub::{Effect, EventSubReducer};
    let (tx, rx) = mpsc::sync_channel(32);
    let input_status = Arc::new(AtomicU8::new(INPUT_RUNNING));
    let reader_status = Arc::clone(&input_status);
    std::thread::Builder::new()
        .name("twitch-host-input".into())
        .spawn(move || read_host_input(reader, tx, &reader_status))
        .map_err(|_| "Cannot start Twitch host input")?;
    let mut host = Host {
        session,
        generation,
        grants: grants.into_iter().collect(),
        target: None,
        sequence: 0,
        pending: None,
        gesture: false,
        last_gesture: None,
        celebration_motion: reaction.motion,
        celebration_expression: reaction.expression,
        chunk_chat: reaction.chunk_chat,
        queued: VecDeque::new(),
        delivery: None,
        output: Box::new(io::stdout()),
    };
    let mut reducer = EventSubReducer::new(broadcaster.clone(), broadcaster.clone())?;
    let mut socket = crate::fish_ws::observation_socket(&reqwest::Url::parse(EVENT_URL).unwrap())?;
    let (session_id, mut keepalive) = welcome(&mut socket, &mut reducer)?;
    api.subscribe(&session_id, &broadcaster)?;
    eprintln!("Twitch connected: selected events enabled");
    let mut last_event = Instant::now();
    let mut validated = Instant::now();
    let mut failures = 0u32;
    loop {
        if !poll_host_input(&rx, &input_status, &mut host)? {
            return Ok(());
        }
        if host.expire_pending(Instant::now()) {
            eprintln!("Twitch action timed out; no automatic replay");
        }
        host.dispatch_notification(Instant::now())?;
        if validated.elapsed() >= Duration::from_secs(3600) {
            if api.validate()? != broadcaster {
                return Err("Twitch account identity changed".into());
            }
            validated = Instant::now();
        }
        let effect = if last_event.elapsed() > Duration::from_secs(keepalive + 1) {
            Err("Twitch keepalive was lost".into())
        } else {
            receive(&mut socket, &mut reducer)
        };
        if let Ok(received) = &effect {
            received.record_activity(&mut last_event, &mut failures, Instant::now());
        }
        match effect {
            Ok(Received::Event(Some(Effect::Notification {
                text,
                celebrate,
                chat,
                ..
            }))) => {
                if let Some(chat) = chat {
                    host.chat_notification(chat, text)?;
                } else {
                    host.notification(text, celebrate)?;
                }
            }
            Ok(Received::Event(Some(Effect::Keepalive))) => {}
            Ok(Received::Event(Some(Effect::Reconnect { url }))) => {
                // Keep the original connection alive until the new welcome.
                let mut replacement = crate::fish_ws::observation_socket(
                    &reqwest::Url::parse(&url).map_err(|_| "Invalid Twitch reconnect URL")?,
                )?;
                let (_, timeout) = welcome(&mut replacement, &mut reducer)?;
                // Drain notifications already buffered on the old socket; dedup
                // is shared across both. Cap work and surface overflow explicitly.
                drain_old_socket(&mut host, || receive(&mut socket, &mut reducer))?;
                let _ = socket.close(None);
                socket = replacement;
                keepalive = timeout;
                last_event = Instant::now();
                eprintln!("Twitch reconnected with existing subscriptions");
            }
            Ok(Received::Event(Some(Effect::Revoked { .. }))) => {
                return Err("Twitch authorization was revoked; update Credentials".into());
            }
            Ok(Received::Event(Some(Effect::Welcome { .. }))) => {
                return Err("Unexpected repeated Twitch welcome".into());
            }
            Ok(Received::Idle | Received::Control | Received::Event(None)) => {}
            Err(error) if error == crate::twitch_eventsub::CAPACITY_ERROR => {
                return Err(error);
            }
            Err(_) => {
                failures += 1;
                if failures > 5 {
                    return Err("Twitch reconnect limit reached; use Restart to try again".into());
                }
                eprintln!("Twitch connection lost; events during the gap cannot be replayed");
                let _ = socket.close(None);
                // This wait occurs only in the isolated child. The supervisor
                // can terminate it immediately on disable or process replacement.
                std::thread::sleep(Duration::from_secs(1u64 << failures.min(4)));
                match crate::fish_ws::observation_socket(&reqwest::Url::parse(EVENT_URL).unwrap()) {
                    Ok(mut replacement) => {
                        let (session_id, timeout) = welcome(&mut replacement, &mut reducer)?;
                        api.subscribe(&session_id, &broadcaster)?;
                        socket = replacement;
                        keepalive = timeout;
                        last_event = Instant::now();
                    }
                    Err(_) => {
                        last_event = Instant::now() - Duration::from_secs(keepalive + 2);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn authorization_scopes_match_saved_receiver_event_selection() {
        use crate::extension_settings::Settings;
        let mut settings = Settings::new();
        assert_eq!(required_scopes(&settings), SCOPES);
        for (selection, expected_types, expected_scopes) in [
            ("Chat only", vec![TYPES[2]], vec![SCOPES[2]]),
            ("Alerts only", TYPES[..2].to_vec(), SCOPES[..2].to_vec()),
            ("Chat and alerts", TYPES.to_vec(), SCOPES.to_vec()),
            ("legacy", TYPES.to_vec(), SCOPES.to_vec()),
        ] {
            settings.insert("events".into(), SettingValue::Text(selection.into()));
            assert_eq!(selected_event_types(&settings), expected_types);
            assert_eq!(required_scopes(&settings), expected_scopes);
        }
    }
    use super::*;
    use std::sync::{Arc, Mutex};
    #[derive(Clone)]
    struct Output(Arc<Mutex<Vec<u8>>>);
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    fn host() -> (Host, Output) {
        let output = Output(Arc::default());
        (
            Host {
                session: uuid::Uuid::new_v4(),
                generation: 7,
                grants: [
                    Capability::LifecycleObserve,
                    Capability::MessageDeliver,
                    Capability::MotionPlay,
                ]
                .into_iter()
                .collect(),
                target: Some(Target {
                    uuid: uuid::Uuid::new_v4(),
                    generation: 2,
                }),
                sequence: 0,
                pending: None,
                gesture: false,
                last_gesture: None,
                celebration_motion: celebration_motion(&Default::default()).unwrap(),
                celebration_expression: celebration_expression(&Default::default()).unwrap(),
                chunk_chat: true,
                queued: VecDeque::new(),
                delivery: None,
                output: Box::new(output.clone()),
            },
            output,
        )
    }
    #[test]
    fn invalid_chat_does_not_stop_later_alert_delivery_and_motion() {
        let (mut host, output) = host();
        for text in ["invalid\0text".into(), "x".repeat(MAX_CHAT_TEXT_BYTES + 1)] {
            let message = ChatMessage {
                source_id: "bad-id".into(),
                author_id: "999".into(),
                author_name: "Viewer".into(),
                text,
            };
            host.chat_notification(message, "legacy display text".into())
                .unwrap();
        }
        assert!(recorded_actions(&output).is_empty());
        host.notification("valid subscription alert".into(), true)
            .unwrap();
        acknowledge(&mut host);
        let actions = recorded_actions(&output);
        assert_eq!(actions.len(), 2);
        assert!(matches!(actions[0].action, Action::MessageDeliver { .. }));
        assert!(matches!(actions[1].action, Action::MotionPlay { .. }));
    }

    #[test]
    fn chat_chunks_keep_original_fields_and_direct_mode_retains_existing_path() {
        for chunks in [false, true] {
            let (mut host, output) = host();
            host.chunk_chat = chunks;
            let message = ChatMessage {
                source_id: "chat-id".into(),
                author_id: "999".into(),
                author_name: "Viewer".into(),
                text: "Hello 🍵\nquoted \"data\"".into(),
            };
            host.chat_notification(message.clone(), "formatted legacy chat".into())
                .unwrap();
            let actions = recorded_actions(&output);
            assert_eq!(actions.len(), 1);
            if chunks {
                assert_eq!(actions[0].action, Action::ChatOffer { message });
            } else {
                assert_eq!(
                    actions[0].action,
                    Action::MessageDeliver {
                        text: "formatted legacy chat".into()
                    }
                );
            }
            acknowledge(&mut host);
            assert!(host.pending.is_none() && !host.gesture);
        }
    }

    #[test]
    fn notification_backlog_is_bounded_fifo_and_expires_without_replay() {
        let (mut host, output) = host();
        for n in 0..MAX_NOTIFICATIONS + 4 {
            host.notification(format!("message-{n}"), false).unwrap();
        }
        assert_eq!(host.queued.len(), MAX_NOTIFICATIONS - 1);
        assert_eq!(recorded_actions(&output).len(), 1);
        host.dispatch_notification(Instant::now()).unwrap();
        assert_eq!(
            recorded_actions(&output).len(),
            1,
            "pending delivery owns admission"
        );
        acknowledge(&mut host);
        host.dispatch_notification(Instant::now()).unwrap();
        let actions = recorded_actions(&output);
        assert!(
            matches!(&actions[1].action, Action::MessageDeliver { text } if text == "message-1")
        );
        assert_ne!(actions[0].id, actions[1].id);
        acknowledge(&mut host);
        host.dispatch_notification(Instant::now() + NOTIFICATION_LIFETIME)
            .unwrap();
        assert!(host.queued.is_empty() && host.delivery.is_none() && host.pending.is_none());
        assert_eq!(recorded_actions(&output).len(), 2);
        host.notification("x".repeat(MAX_TEXT_BYTES + 1), false)
            .unwrap();
        host.notification("invalid\0text".into(), false).unwrap();
        assert_eq!(recorded_actions(&output).len(), 2);
    }

    #[test]
    fn late_host_results_cannot_revive_deadline_or_total_notification_expiry() {
        for total_expiry in [false, true] {
            let (mut host, output) = host();
            host.notification("one celebration".into(), true).unwrap();
            let (id, target, _, _) = host.pending.clone().unwrap();
            if total_expiry {
                host.delivery.as_mut().unwrap().expires = Instant::now();
            } else {
                host.pending.as_mut().unwrap().2 = Instant::now() - Duration::from_secs(11);
            }
            // Exercise the real result-consumption entry point, without a
            // separate timeout tick that could hide polling-order regressions.
            host.message(success(&host, &id, target)).unwrap();
            assert!(host.pending.is_none() && host.delivery.is_none() && !host.gesture);
            assert!(host.queued.is_empty());
            assert_eq!(
                recorded_actions(&output).len(),
                1,
                "late acceptance started a gesture"
            );
        }
    }

    fn reject(host: &mut Host, code: ErrorCode) {
        let (id, target, _, _) = host.pending.clone().unwrap();
        host.message(envelope(
            host,
            HostMessage::Result {
                id,
                target,
                result: ActionResult::Error {
                    code,
                    message: "fixed refusal".into(),
                },
            },
        ))
        .unwrap();
    }

    #[test]
    fn explicit_busy_refusal_retries_only_message_with_backoff_and_attempt_bound() {
        let (mut host, output) = host();
        host.notification("chat".into(), false).unwrap();
        let first_id = host.pending.as_ref().unwrap().0.clone();
        for attempt in 1..=MAX_DELIVERY_ATTEMPTS {
            assert_eq!(host.delivery.as_ref().unwrap().attempts, attempt);
            reject(&mut host, ErrorCode::Busy);
            assert!(host.pending.is_none());
            host.dispatch_notification(Instant::now()).unwrap();
            assert!(
                host.pending.is_none(),
                "backoff must prevent immediate retry"
            );
            if attempt < MAX_DELIVERY_ATTEMPTS {
                let ready = host.queued.front().unwrap().not_before;
                host.dispatch_notification(ready).unwrap();
                let current_id = host.pending.as_ref().unwrap().0.clone();
                assert_ne!(current_id, first_id);
                let target = host.target.unwrap();
                host.message(success(&host, &first_id, target)).unwrap();
                assert_eq!(
                    host.pending.as_ref().unwrap().0,
                    current_id,
                    "late earlier ACK must not advance retry"
                );
            }
        }
        assert!(host.queued.is_empty() && host.delivery.is_none());
        assert_eq!(
            recorded_actions(&output).len(),
            usize::from(MAX_DELIVERY_ATTEMPTS)
        );
        for code in [
            ErrorCode::Unavailable,
            ErrorCode::InvalidRequest,
            ErrorCode::InternalError,
        ] {
            host.notification("new chat".into(), false).unwrap();
            reject(&mut host, code);
            assert!(host.queued.is_empty() && host.pending.is_none() && host.delivery.is_none());
        }
        host.notification("uncertain delivery".into(), false)
            .unwrap();
        let at = host.pending.as_ref().unwrap().2;
        assert!(host.expire_pending(at + Duration::from_secs(11)));
        assert!(host.queued.is_empty() && host.delivery.is_none());
        let count = recorded_actions(&output).len();
        host.dispatch_notification(Instant::now() + NOTIFICATION_LIFETIME)
            .unwrap();
        assert_eq!(recorded_actions(&output).len(), count);
    }

    #[test]
    fn queued_notifications_never_follow_a_replacement_target_or_retry_expression_busy() {
        let (mut host, output) = host();
        host.notification("first".into(), false).unwrap();
        host.notification("waiting".into(), true).unwrap();
        reject(&mut host, ErrorCode::Busy);
        assert_eq!(host.queued.len(), 2);
        let replacement = Target {
            generation: host.target.unwrap().generation + 1,
            ..host.target.unwrap()
        };
        host.message(envelope(
            &host,
            HostMessage::Event {
                event: HostEvent::TargetChanged {
                    target: Some(replacement),
                },
            },
        ))
        .unwrap();
        host.dispatch_notification(Instant::now() + BUSY_RETRY_DELAY)
            .unwrap();
        assert!(host.queued.is_empty() && host.delivery.is_none() && host.pending.is_none());
        assert_eq!(recorded_actions(&output).len(), 1);
        host.grants.insert(Capability::ExpressionApply);
        host.celebration_expression = Some("happy".into());
        host.notification("celebration".into(), true).unwrap();
        acknowledge(&mut host);
        assert_eq!(host.pending.as_ref().unwrap().3, ActionStage::Expression);
        reject(&mut host, ErrorCode::Busy);
        assert!(host.queued.is_empty() && host.delivery.is_none() && host.pending.is_none());
    }

    fn envelope(host: &Host, message: HostMessage) -> HostEnvelope {
        HostEnvelope {
            sequence: 1,
            api_version: API_VERSION,
            message_type: HostEnvelopeType::Host,
            session_id: host.session,
            process_generation: host.generation,
            message,
        }
    }
    fn success(host: &Host, id: &str, target: Target) -> HostEnvelope {
        envelope(
            host,
            HostMessage::Result {
                id: id.into(),
                target,
                result: ActionResult::Ok {
                    value: ResultValue::Accepted {},
                },
            },
        )
    }

    fn recorded_actions(output: &Output) -> Vec<ActionEnvelope> {
        output
            .0
            .lock()
            .unwrap()
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| parse_action_line(line).unwrap())
            .collect()
    }

    fn acknowledge(host: &mut Host) -> String {
        let (id, target, _, _) = host.pending.clone().expect("pending action");
        host.message(success(host, &id, target)).unwrap();
        id
    }

    #[test]
    fn expression_setting_is_optional_bounded_exact_and_excludes_reserved_channels() {
        assert_eq!(celebration_expression(&Default::default()).unwrap(), None);
        for name in [
            "".to_owned(),
            "Smiling Eyes 笑顔".into(),
            "  Happy Face  ".into(),
            "x".repeat(MAX_ID_BYTES),
            format!("{}ab", "笑".repeat(42)),
        ] {
            let settings = serde_json::from_value(json!({"celebration_expression": name})).unwrap();
            assert_eq!(
                celebration_expression(&settings).unwrap(),
                if name.is_empty() { None } else { Some(name) }
            );
        }
        for invalid in [
            SettingValue::Bool(false),
            SettingValue::Integer(1),
            SettingValue::Text("x".repeat(MAX_ID_BYTES + 1)),
            SettingValue::Text("笑".repeat(43)),
            SettingValue::Text("   ".into()),
            SettingValue::Text("\u{2003}".into()),
            SettingValue::Text("Happy\nFace".into()),
            SettingValue::Text("Happy\tFace".into()),
            SettingValue::Text("Happy\0Face".into()),
            SettingValue::Text("Happy\u{7f}Face".into()),
            SettingValue::Text("BlinkLeft".into()),
            SettingValue::Text("LOOKright".into()),
            SettingValue::Text("mouthSmile".into()),
            SettingValue::Text("aa".into()),
        ] {
            let settings = crate::extension_settings::Settings::from([(
                "celebration_expression".into(),
                invalid,
            )]);
            assert!(celebration_expression(&settings).is_err());
        }
    }

    #[test]
    fn expression_then_motion_has_separate_ack_stages_and_one_shared_cooldown() {
        let (mut host, output) = host();
        host.grants.insert(Capability::ExpressionApply);
        host.celebration_expression = Some("Smiling Eyes 笑顔".into());
        host.celebration_motion = Some("org.example.gestures/wave".into());
        let target = host.target.unwrap();
        host.notification("subscription".into(), true).unwrap();
        let delivery = acknowledge(&mut host);
        assert_eq!(host.pending.as_ref().unwrap().3, ActionStage::Expression);
        let admitted = host.last_gesture.unwrap();
        host.message(success(&host, &delivery, target)).unwrap();
        assert_eq!(
            host.sequence, 2,
            "duplicate delivery ACK cannot advance expression"
        );
        host.notification("busy".into(), true).unwrap();
        assert_eq!(host.sequence, 2);
        let expression = acknowledge(&mut host);
        assert!(
            host.waiting_for_motion(),
            "expression cooldown must not suppress the paired motion"
        );
        assert_eq!(host.last_gesture, Some(admitted));
        host.message(success(&host, &expression, target)).unwrap();
        assert_eq!(
            host.sequence, 3,
            "duplicate expression ACK cannot repeat motion"
        );
        let motion = acknowledge(&mut host);
        assert!(
            host.waiting_for_motion(),
            "motion acceptance is not terminal"
        );
        let actions = recorded_actions(&output);
        assert_eq!(actions.len(), 3);
        assert_eq!(
            actions[1].action,
            Action::ExpressionApply {
                name: "Smiling Eyes 笑顔".into(),
                strength: 0.75,
                duration_ms: 2000
            }
        );
        assert_eq!(
            actions[2].action,
            Action::MotionPlay {
                motion_id: "org.example.gestures/wave".into(),
                mode: PlaybackMode::OneShot
            }
        );
        assert!(actions.iter().all(|action| action.target == target));
        host.message(envelope(
            &host,
            HostMessage::Event {
                event: HostEvent::MotionFinished {
                    target,
                    action_id: motion,
                    motion_id: "org.example.gestures/wave".into(),
                    outcome: MotionOutcome::Completed,
                },
            },
        ))
        .unwrap();
        assert!(host.pending.is_none());
        host.notification("another subscription".into(), true)
            .unwrap();
        acknowledge(&mut host);
        assert!(
            host.pending.is_none(),
            "shared cooldown suppresses the whole next reaction"
        );
        assert_eq!(recorded_actions(&output).len(), 4);
        assert_eq!(host.last_gesture, Some(admitted));
    }

    #[test]
    fn expression_only_none_and_missing_grants_take_only_permitted_reaction_steps() {
        for expression_granted in [false, true] {
            for motion_granted in [false, true] {
                for motion_configured in [false, true] {
                    let (mut host, output) = host();
                    host.celebration_expression = Some("Happy Face".into());
                    host.celebration_motion =
                        motion_configured.then(|| "org.example.gestures/wave".into());
                    if expression_granted {
                        host.grants.insert(Capability::ExpressionApply);
                    }
                    if !motion_granted {
                        host.grants.remove(&Capability::MotionPlay);
                    }
                    host.notification("subscription".into(), true).unwrap();
                    acknowledge(&mut host); // Delivery.
                    if expression_granted {
                        acknowledge(&mut host);
                    }
                    let actions = recorded_actions(&output);
                    let motion = motion_granted && motion_configured;
                    assert_eq!(
                        actions.len(),
                        1 + usize::from(expression_granted) + usize::from(motion)
                    );
                    assert_eq!(host.waiting_for_motion(), motion);
                    assert_eq!(host.last_gesture.is_some(), expression_granted || motion);
                    if expression_granted {
                        assert!(matches!(actions[1].action, Action::ExpressionApply { .. }));
                    }
                    if motion {
                        assert!(matches!(
                            actions.last().unwrap().action,
                            Action::MotionPlay { .. }
                        ));
                    } else {
                        assert!(host.pending.is_none());
                    }
                }
            }
        }
        let (mut host, output) = host();
        host.grants.insert(Capability::ExpressionApply);
        host.celebration_motion = None;
        host.celebration_expression = None;
        host.notification("subscription".into(), true).unwrap();
        acknowledge(&mut host);
        assert_eq!(recorded_actions(&output).len(), 1);
        assert!(host.last_gesture.is_none() && host.pending.is_none());

        // Without delivery permission the same reaction stages still work.
        host.grants.remove(&Capability::MessageDeliver);
        host.celebration_expression = Some("Happy Face".into());
        host.notification("subscription".into(), true).unwrap();
        assert_eq!(host.pending.as_ref().unwrap().3, ActionStage::Expression);
        acknowledge(&mut host);
        assert!(host.pending.is_none() && host.last_gesture.is_some());
    }

    #[test]
    fn rejection_target_change_and_timeout_clear_every_stage_without_retry_or_inheritance() {
        for stage in [
            ActionStage::Message,
            ActionStage::Expression,
            ActionStage::Motion,
        ] {
            for cancellation in ["rejection", "target", "timeout"] {
                let (mut host, output) = host();
                host.grants.insert(Capability::ExpressionApply);
                host.celebration_expression = Some("Happy Face".into());
                host.notification("subscription".into(), true).unwrap();
                if stage != ActionStage::Message {
                    acknowledge(&mut host);
                }
                if stage == ActionStage::Motion {
                    acknowledge(&mut host);
                }
                let (id, target, at, actual) = host.pending.clone().unwrap();
                assert_eq!(actual, stage);
                let count = host.sequence;
                match cancellation {
                    "rejection" => host
                        .message(envelope(
                            &host,
                            HostMessage::Result {
                                id: id.clone(),
                                target,
                                result: ActionResult::Error {
                                    code: ErrorCode::UnknownExpression,
                                    message: "unavailable".into(),
                                },
                            },
                        ))
                        .unwrap(),
                    "target" => host
                        .message(envelope(
                            &host,
                            HostMessage::Event {
                                event: HostEvent::TargetChanged {
                                    target: Some(Target {
                                        generation: target.generation + 1,
                                        ..target
                                    }),
                                },
                            },
                        ))
                        .unwrap(),
                    _ => assert!(host.expire_pending(at + Duration::from_secs(11))),
                }
                assert!(host.pending.is_none() && !host.gesture);
                host.message(success(&host, &id, target)).unwrap();
                assert_eq!(
                    host.sequence, count,
                    "stale ACK must not restart cancelled steps"
                );
                host.notification("ordinary chat".into(), false).unwrap();
                acknowledge(&mut host);
                assert!(host.pending.is_none() && !host.gesture);
                let actions = recorded_actions(&output);
                assert_eq!(actions.len() as u64, count + 1);
                assert!(matches!(
                    actions.last().unwrap().action,
                    Action::MessageDeliver { .. }
                ));
            }
        }
    }

    #[test]
    fn noncelebrating_notifications_never_start_configured_reactions() {
        for deliver in [false, true] {
            let (mut host, output) = host();
            host.grants.insert(Capability::ExpressionApply);
            host.celebration_expression = Some("Happy Face".into());
            if !deliver {
                host.grants.remove(&Capability::MessageDeliver);
            }
            host.notification("ordinary chat".into(), false).unwrap();
            if deliver {
                acknowledge(&mut host);
            }
            assert_eq!(recorded_actions(&output).len(), usize::from(deliver));
            assert!(host.pending.is_none() && host.last_gesture.is_none() && !host.gesture);
        }
    }

    #[test]
    fn celebration_setting_selects_opaque_motion_and_preserves_absent_default() {
        for (settings, expected) in [
            (json!({}), DEFAULT_CELEBRATION_MOTION.to_owned()),
            (
                json!({"celebration_motion": "org.example.gestures/wave"}),
                "org.example.gestures/wave".to_owned(),
            ),
            (
                json!({"celebration_motion": "x".repeat(MAX_REGISTERED_ID_BYTES)}),
                "x".repeat(MAX_REGISTERED_ID_BYTES),
            ),
            (
                json!({"celebration_motion": format!("{}a", "é".repeat(96))}),
                format!("{}a", "é".repeat(96)),
            ),
        ] {
            let settings = serde_json::from_value(settings).unwrap();
            let (mut host, output) = host();
            host.celebration_motion = celebration_motion(&settings).unwrap();
            let target = host.target.unwrap();
            host.notification("subscription".into(), true).unwrap();
            let delivery = host.pending.as_ref().unwrap().0.clone();
            host.message(success(&host, &delivery, target)).unwrap();
            let motion = host.pending.as_ref().unwrap().0.clone();
            assert!(host.waiting_for_motion());
            host.message(success(&host, &motion, target)).unwrap();
            assert!(host.pending.is_some(), "acceptance is not terminal");

            let actions: Vec<_> = output
                .0
                .lock()
                .unwrap()
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .map(|line| parse_action_line(line).unwrap())
                .collect();
            assert_eq!(actions.len(), 2);
            assert!(matches!(actions[0].action, Action::MessageDeliver { .. }));
            let Action::MotionPlay { motion_id, mode } = &actions[1].action else {
                panic!("motion action expected");
            };
            assert_eq!(motion_id, &expected);
            assert_eq!(*mode, PlaybackMode::OneShot);
            assert_eq!(actions[1].target, target);
            host.message(envelope(
                &host,
                HostMessage::Event {
                    event: HostEvent::MotionFinished {
                        target,
                        action_id: motion,
                        motion_id: expected,
                        outcome: MotionOutcome::Completed,
                    },
                },
            ))
            .unwrap();
            assert!(host.pending.is_none() && !host.waiting_for_motion());
        }
    }

    #[test]
    fn empty_celebration_motion_disables_only_gesture_and_does_not_consume_cooldown() {
        let settings = serde_json::from_value(json!({"celebration_motion": ""})).unwrap();
        assert_eq!(celebration_motion(&settings).unwrap(), None);
        for deliver in [false, true] {
            let (mut host, output) = host();
            host.celebration_motion = celebration_motion(&settings).unwrap();
            if !deliver {
                host.grants.remove(&Capability::MessageDeliver);
            }
            let target = host.target.unwrap();
            host.notification("subscription".into(), true).unwrap();
            if deliver {
                let delivery = host.pending.as_ref().unwrap().0.clone();
                host.message(success(&host, &delivery, target)).unwrap();
                let bytes = output.0.lock().unwrap();
                assert!(matches!(
                    parse_action_line(&bytes).unwrap().action,
                    Action::MessageDeliver { .. }
                ));
            } else {
                assert!(output.0.lock().unwrap().is_empty());
            }
            assert!(host.pending.is_none());
            assert!(!host.gesture && !host.waiting_for_motion());
            assert!(host.last_gesture.is_none());
            assert_eq!(host.sequence, u64::from(deliver));
        }
    }

    #[test]
    fn celebration_motion_rejects_mistyped_oversized_or_whitespace_control_ids() {
        for invalid in [
            SettingValue::Bool(false),
            SettingValue::Bool(true),
            SettingValue::Integer(0),
            SettingValue::Text("x".repeat(MAX_REGISTERED_ID_BYTES + 1)),
            SettingValue::Text("é".repeat(97)),
            SettingValue::Text(" motion".into()),
            SettingValue::Text("motion ".into()),
            SettingValue::Text("\t".into()),
            SettingValue::Text("org.example.pack/no\nd".into()),
            SettingValue::Text("org.example.pack/no\0d".into()),
            SettingValue::Text("org.example.pack/no\u{7f}d".into()),
            SettingValue::Text("org.example.pack/no\u{85}d".into()),
            SettingValue::Text("org.example.pack/no\u{a0}d".into()),
            SettingValue::Text("org.example.pack/no\u{2003}d".into()),
            SettingValue::Text("org.example.pack/no\u{2028}d".into()),
        ] {
            let settings =
                crate::extension_settings::Settings::from([("celebration_motion".into(), invalid)]);
            assert!(
                celebration_motion(&settings)
                    .unwrap_err()
                    .contains("celebration_motion")
            );
        }
    }

    fn socket_pair() -> (Socket, WebSocket<TcpStream>) {
        use tungstenite::protocol::Role;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        server
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        (
            WebSocket::from_raw_socket(MaybeTlsStream::Plain(client), Role::Client, None),
            WebSocket::from_raw_socket(server, Role::Server, None),
        )
    }

    fn eventsub_chat(id: &str) -> String {
        json!({
            "metadata": {
                "message_id": id, "message_type": "notification",
                "message_timestamp": "2024-01-01T00:00:00Z",
                "subscription_type": "channel.chat.message", "subscription_version": "1"
            },
            "payload": {
                "subscription": {
                    "id": "subscription", "status": "enabled", "type": "channel.chat.message", "version": "1",
                    "condition": {"broadcaster_user_id": "100", "user_id": "100"},
                    "transport": {"method": "websocket", "session_id": "session"}
                },
                "event": {
                    "broadcaster_user_id": "100", "chatter_user_id": "999", "chatter_user_name": "Viewer",
                    "message_id": "chat-message", "message": {"text": id}
                }
            }
        }).to_string()
    }

    #[test]
    fn reconnect_drain_reads_past_real_duplicate_ping_and_pong_to_later_notification() {
        use crate::twitch_eventsub::{Effect, EventSubReducer};
        const NOW: u64 = 1_704_067_200;
        let (mut socket, mut server) = socket_pair();
        let mut reducer = EventSubReducer::new("100".into(), "100".into()).unwrap();
        let original = eventsub_chat("original");
        server.send(Message::Text(original.clone().into())).unwrap();
        assert!(matches!(
            receive_at(&mut socket, &mut reducer, NOW).unwrap(),
            Received::Event(Some(Effect::Notification { .. }))
        ));
        server.send(Message::Text(original.into())).unwrap();
        server.send(Message::Ping(vec![1, 2].into())).unwrap();
        server.send(Message::Pong(vec![3].into())).unwrap();
        server
            .send(Message::Text(eventsub_chat("later-notification").into()))
            .unwrap();

        let (mut host, output) = host();
        let mut received = Vec::new();
        drain_old_socket(&mut host, || {
            let next = receive_at(&mut socket, &mut reducer, NOW)?;
            received.push(format!("{next:?}"));
            Ok(next)
        })
        .unwrap();
        assert_eq!(
            received.len(),
            5,
            "duplicate, Ping, Pong, notification, then idle"
        );
        assert_eq!(received[0], "Event(None)");
        assert_eq!(received[1], "Control");
        assert_eq!(received[2], "Control");
        assert_eq!(received[4], "Idle");
        let bytes = output.0.lock().unwrap();
        let action = parse_action_line(&bytes).unwrap();
        let Action::ChatOffer { message } = action.action else {
            panic!("structured chat offer expected");
        };
        assert!(message.text.contains("later-notification"));
        assert_eq!(message.author_id, "999");
        assert_eq!(message.author_name, "Viewer");
        assert_eq!(host.sequence, 1, "duplicate must not be delivered twice");
    }

    #[test]
    fn only_validated_eventsub_activity_resets_liveness_and_failures() {
        use crate::twitch_eventsub::EventSubReducer;
        const NOW: u64 = 1_704_067_200;
        let (mut socket, mut server) = socket_pair();
        let mut reducer = EventSubReducer::new("100".into(), "100".into()).unwrap();
        let message = eventsub_chat("duplicate");
        reducer.receive(&message, NOW).unwrap();
        server.send(Message::Text(message.into())).unwrap();
        server.send(Message::Ping(vec![1].into())).unwrap();
        server.send(Message::Pong(vec![2].into())).unwrap();
        let start = Instant::now();
        let duplicate_at = start + Duration::from_secs(9);
        let mut last_event = start;
        let mut failures = 3;
        let duplicate = receive_at(&mut socket, &mut reducer, NOW).unwrap();
        assert_eq!(duplicate, Received::Event(None));
        duplicate.record_activity(&mut last_event, &mut failures, duplicate_at);
        assert_eq!(last_event, duplicate_at);
        assert_eq!(failures, 0);
        assert!(
            (start + Duration::from_secs(12)).duration_since(last_event) < Duration::from_secs(10)
        );

        for expected in [Received::Control, Received::Control, Received::Idle] {
            let received = receive_at(&mut socket, &mut reducer, NOW).unwrap();
            assert_eq!(received, expected);
            failures = 2;
            received.record_activity(
                &mut last_event,
                &mut failures,
                start + Duration::from_secs(20),
            );
            assert_eq!(last_event, duplicate_at);
            assert_eq!(failures, 2);
        }
        server.send(Message::Text("invalid JSON".into())).unwrap();
        assert!(receive_at(&mut socket, &mut reducer, NOW).is_err());
    }

    #[test]
    fn reconnect_drain_surfaces_revocation_read_errors_and_work_bound() {
        use crate::twitch_eventsub::Effect;
        let (mut host, _) = host();
        let error = drain_old_socket(&mut host, || {
            Ok(Received::Event(Some(Effect::Revoked {
                subscription_type: "channel.chat.message".into(),
                status: "authorization_revoked".into(),
            })))
        })
        .unwrap_err();
        assert!(error.contains("revoked"));
        let error =
            drain_old_socket(&mut host, || Err("Twitch WebSocket read failed".into())).unwrap_err();
        assert!(error.contains("events may be lost"));
        let mut reads = 0;
        let error = drain_old_socket(&mut host, || {
            reads += 1;
            Ok(Received::Control)
        })
        .unwrap_err();
        assert_eq!(reads, 256);
        assert!(error.contains("exceeded its bound"));
    }

    #[test]
    fn full_host_input_queue_retains_overflow_and_rejects_queued_actions() {
        let (mut host, _) = host();
        let line = encode_host_line(&envelope(
            &host,
            HostMessage::Event {
                event: HostEvent::TargetChanged { target: None },
            },
        ))
        .unwrap();
        let mut input = Vec::new();
        for _ in 0..33 {
            input.extend_from_slice(&line);
        }
        let (tx, rx) = mpsc::sync_channel(32);
        let status = AtomicU8::new(INPUT_RUNNING);
        read_host_input(io::Cursor::new(input), tx, &status);
        assert_eq!(status.load(Ordering::Acquire), INPUT_OVERFLOW);
        let target = host.target;
        let error = poll_host_input(&rx, &status, &mut host).unwrap_err();
        assert!(error.contains("overflowed") && error.contains("lost"));
        assert_eq!(
            host.target, target,
            "loss must stop processing queued target changes"
        );
        assert_eq!(rx.try_iter().count(), 32);
        assert!(
            poll_host_input(&rx, &status, &mut host).is_err(),
            "failure survives queue drain and disconnection"
        );
    }

    #[test]
    fn host_input_read_failure_is_distinct_from_normal_eof() {
        let (mut host, _) = host();
        let (tx, rx) = mpsc::sync_channel(32);
        let status = AtomicU8::new(INPUT_RUNNING);
        read_host_input(io::Cursor::new(b"malformed host input\n"), tx, &status);
        assert_eq!(status.load(Ordering::Acquire), INPUT_FAILED);
        assert!(
            poll_host_input(&rx, &status, &mut host)
                .unwrap_err()
                .contains("input failed")
        );

        let line = encode_host_line(&envelope(
            &host,
            HostMessage::Event {
                event: HostEvent::TargetChanged { target: None },
            },
        ))
        .unwrap();
        let (tx, rx) = mpsc::sync_channel(32);
        let status = AtomicU8::new(INPUT_RUNNING);
        read_host_input(io::Cursor::new(line), tx, &status);
        assert_eq!(status.load(Ordering::Acquire), INPUT_EOF);
        assert!(!poll_host_input(&rx, &status, &mut host).unwrap());
        assert_eq!(
            host.target, None,
            "normal EOF drains accepted host messages"
        );
    }

    #[test]
    fn message_then_motion_waits_for_terminal_and_ignores_duplicate_acknowledgements() {
        let (mut host, output) = host();
        let target = host.target.unwrap();
        host.notification("external subscription".into(), true)
            .unwrap();
        let delivery = host.pending.as_ref().unwrap().0.clone();
        host.message(success(&host, &delivery, target)).unwrap();
        let motion = host.pending.as_ref().unwrap().0.clone();
        assert_ne!(motion, delivery);
        assert!(host.waiting_for_motion());
        host.message(success(&host, &delivery, target)).unwrap();
        host.message(success(&host, &motion, target)).unwrap();
        assert!(host.pending.is_some(), "acceptance is not completion");
        host.notification("busy event".into(), true).unwrap();
        assert_eq!(host.sequence, 2);
        let terminal = envelope(
            &host,
            HostMessage::Event {
                event: HostEvent::MotionFinished {
                    target,
                    action_id: motion,
                    motion_id: "studio.integration.twitch/nod".into(),
                    outcome: MotionOutcome::Completed,
                },
            },
        );
        host.message(terminal).unwrap();
        assert!(host.pending.is_none() && !host.waiting_for_motion());
        let bytes = output.0.lock().unwrap();
        let actions: Vec<_> = bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| parse_action_line(line).unwrap())
            .collect();
        assert_eq!(actions.len(), 2);
        assert!(matches!(actions[0].action, Action::MessageDeliver { .. }));
        assert!(matches!(actions[1].action, Action::MotionPlay { .. }));
        assert!(actions.iter().all(|action| action.target == target));
    }
    #[test]
    fn target_switch_reject_and_missing_avatar_do_not_replay_or_retarget_effects() {
        let (mut host, _) = host();
        let original = host.target.unwrap();
        host.notification("alert".into(), true).unwrap();
        let id = host.pending.as_ref().unwrap().0.clone();
        let replacement = Target {
            generation: original.generation + 1,
            ..original
        };
        host.message(envelope(
            &host,
            HostMessage::Event {
                event: HostEvent::TargetChanged {
                    target: Some(replacement),
                },
            },
        ))
        .unwrap();
        host.message(success(&host, &id, original)).unwrap();
        assert_eq!(host.sequence, 1);
        assert!(host.pending.is_none());
        host.notification("rejected alert".into(), true).unwrap();
        let id = host.pending.as_ref().unwrap().0.clone();
        host.message(envelope(
            &host,
            HostMessage::Result {
                id,
                target: replacement,
                result: ActionResult::Error {
                    code: ErrorCode::Busy,
                    message: "busy".into(),
                },
            },
        ))
        .unwrap();
        assert_eq!(host.sequence, 2);
        assert!(host.pending.is_none() && !host.gesture);
        host.message(envelope(
            &host,
            HostMessage::Event {
                event: HostEvent::TargetChanged { target: None },
            },
        ))
        .unwrap();
        host.notification("no avatar".into(), true).unwrap();
        assert_eq!(host.sequence, 2);
    }
    #[test]
    fn http_validation_and_subscriptions_bind_account_and_keep_errors_redacted() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for index in 0..5 {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let header = String::from_utf8(request).unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(|v| v.parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                let mut body = vec![0; length];
                socket.read_exact(&mut body).unwrap();
                if index == 0 {
                    assert!(
                        header
                            .to_ascii_lowercase()
                            .contains("authorization: oauth fake-token")
                    );
                } else if index < 4 {
                    assert!(
                        header
                            .to_ascii_lowercase()
                            .contains("authorization: bearer fake-token")
                    );
                    let value: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(value["type"], TYPES[index - 1]);
                    assert_eq!(value["condition"]["broadcaster_user_id"], "123");
                    assert_eq!(value["transport"]["session_id"], "session-fixture");
                    if index == 3 {
                        assert_eq!(value["condition"]["user_id"], "123");
                    }
                }
                let (status, body) = if index == 0 {
                    (
                        "200 OK",
                        json!({"client_id":"fake-client","user_id":"123","scopes":SCOPES})
                            .to_string(),
                    )
                } else if index == 4 {
                    ("401 Unauthorized", "sensitive response fake-token".into())
                } else {
                    ("202 Accepted", "{}".into())
                };
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let api = Api {
            client: reqwest::blocking::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            client_id: "fake-client".into(),
            token: "fake-token".into(),
            kinds: TYPES.to_vec(),
            validate_url: reqwest::Url::parse(&format!("{root}/validate")).unwrap(),
            subscribe_url: reqwest::Url::parse(&format!("{root}/subscribe")).unwrap(),
        };
        assert_eq!(api.validate().unwrap(), "123");
        api.subscribe("session-fixture", "123").unwrap();
        let error = api.validate().unwrap_err();
        assert!(
            error.contains("401") && !error.contains("fake-token") && !error.contains("sensitive")
        );
        server.join().unwrap();
    }
}
