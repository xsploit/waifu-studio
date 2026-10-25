//! Bounded, pure reduction of authenticated Twitch EventSub WebSocket messages.
//!
//! The caller owns TLS, token validation, subscription creation, socket deadlines,
//! and reporting connection loss (Twitch does not replay missed events).

use std::collections::VecDeque;

use serde::Deserialize;

pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_DEDUP_IDS: usize = 256;
pub const CAPACITY_ERROR: &str = "EventSub dedup capacity exhausted; delivery rejected to prevent replay until retained IDs expire";
pub const MAX_AGE_SECONDS: u64 = 5 * 60;
pub const MAX_FUTURE_SKEW_SECONDS: u64 = 30;

// Retain IDs by host admission time, independently of retry source timestamps.
// Even a maximally future-dated original is stale after this inclusive window.
const DEDUP_RETENTION_SECONDS: u64 = MAX_AGE_SECONDS + MAX_FUTURE_SKEW_SECONDS;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Welcome {
        session_id: String,
        keepalive_timeout_seconds: u64,
    },
    Notification {
        id: String,
        text: String,
        celebrate: bool,
        /// Exact chat fields for host-side batching; alerts remain None.
        chat: Option<crate::extension_protocol::ChatMessage>,
    },
    Reconnect {
        url: String,
    },
    Revoked {
        subscription_type: String,
        status: String,
    },
    Keepalive,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SeenDelivery {
    id: String,
    admitted_at: u64,
}

/// Opaque, bounded in-memory state. Contains identities, delivery IDs and host
/// times, no text. Snapshot restoration preserves dedup capacity and expiry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReducerSnapshot {
    broadcaster_id: String,
    user_id: String,
    seen: VecDeque<SeenDelivery>,
    latest_now_unix_secs: u64,
}

pub struct EventSubReducer {
    state: ReducerSnapshot,
}

impl EventSubReducer {
    pub fn new(broadcaster_id: String, user_id: String) -> Result<Self, String> {
        if !valid_user_id(&broadcaster_id) || !valid_user_id(&user_id) {
            return Err("EventSub identities must be bounded numeric Twitch user IDs".into());
        }
        Ok(Self {
            state: ReducerSnapshot {
                broadcaster_id,
                user_id,
                seen: VecDeque::new(),
                latest_now_unix_secs: 0,
            },
        })
    }

    pub fn snapshot(&self) -> ReducerSnapshot {
        self.state.clone()
    }

    pub fn from_snapshot(snapshot: ReducerSnapshot) -> Self {
        Self { state: snapshot }
    }

    /// `Ok(None)` is a validated duplicate. Errors must be surfaced as rejected
    /// input/loss by the caller, never silently dropped or logged with raw text.
    /// At capacity, unknown IDs fail closed until retained entries expire; live
    /// IDs are never evicted. Duplicates do not extend their retention window.
    pub fn receive(&mut self, raw: &str, now_unix_secs: u64) -> Result<Option<Effect>, String> {
        if raw.len() > MAX_MESSAGE_BYTES {
            return Err("EventSub message exceeds 64 KiB; delivery rejected".into());
        }
        let envelope: Envelope =
            serde_json::from_str(raw).map_err(|_| "Invalid EventSub JSON envelope".to_owned())?;
        let metadata = &envelope.metadata;
        if !valid_identifier(&metadata.message_id) {
            return Err("Invalid EventSub delivery ID".into());
        }
        let timestamp = parse_timestamp(&metadata.message_timestamp)
            .ok_or("Invalid EventSub RFC3339 timestamp")?;
        // Do not let a clock rollback reopen already expired source messages.
        // Only a fully validated envelope may commit this clock floor below.
        let now_unix_secs = now_unix_secs.max(self.state.latest_now_unix_secs);
        let now = i128::from(now_unix_secs) * 1_000_000_000;
        if timestamp < now - i128::from(MAX_AGE_SECONDS) * 1_000_000_000 {
            return Err("Stale EventSub message; delivery rejected".into());
        }
        if timestamp > now + i128::from(MAX_FUTURE_SKEW_SECONDS) * 1_000_000_000 {
            return Err("Future EventSub message; delivery rejected".into());
        }

        // Validate *all* source and event fields before consulting/changing dedup.
        // A spoofed duplicate must remain an error, and cannot poison the cache.
        let effect = self.effect(&envelope)?;
        self.state.latest_now_unix_secs = now_unix_secs;
        self.state.seen.retain(|delivery| {
            now_unix_secs.saturating_sub(delivery.admitted_at) <= DEDUP_RETENTION_SECONDS
        });
        if self
            .state
            .seen
            .iter()
            .any(|delivery| delivery.id == metadata.message_id)
        {
            return Ok(None);
        }
        if self.state.seen.len() >= MAX_DEDUP_IDS {
            return Err(CAPACITY_ERROR.into());
        }
        self.state.seen.push_back(SeenDelivery {
            id: metadata.message_id.clone(),
            admitted_at: now_unix_secs,
        });
        Ok(Some(effect))
    }

    fn effect(&self, envelope: &Envelope) -> Result<Effect, String> {
        let payload = &envelope.payload;
        match envelope.metadata.message_type.as_str() {
            "notification" => {
                let subscription = self.subscription(envelope)?;
                if subscription.status != "enabled" || payload.session.is_some() {
                    return Err("EventSub notification subscription is not enabled".into());
                }
                let event = payload.event.as_ref().ok_or("Missing EventSub event")?;
                if event.broadcaster_user_id != self.state.broadcaster_id {
                    return Err("EventSub event broadcaster mismatch".into());
                }
                let (text, celebrate) = format_event(&subscription.kind, event)?;
                let chat = if subscription.kind == "channel.chat.message" {
                    let Some(EventMessage::Chat { text }) = &event.message else {
                        unreachable!("format_event validated chat")
                    };
                    Some(crate::extension_protocol::ChatMessage {
                        source_id: event.message_id.clone().expect("validated chat ID"),
                        author_id: event.chatter_user_id.clone().expect("validated chatter"),
                        author_name: event
                            .chatter_user_name
                            .clone()
                            .expect("validated chatter name"),
                        text: text.clone(),
                    })
                } else {
                    None
                };
                Ok(Effect::Notification {
                    id: envelope.metadata.message_id.clone(),
                    text,
                    celebrate,
                    chat,
                })
            }
            "revocation" => {
                let subscription = self.subscription(envelope)?;
                if payload.event.is_some()
                    || payload.session.is_some()
                    || subscription.status == "enabled"
                    || subscription.status.is_empty()
                    || subscription.status.len() > 128
                    || !subscription
                        .status
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b == b'_')
                {
                    return Err("Invalid EventSub revocation status or payload".into());
                }
                // Preserve unknown future machine statuses as explicit revocations.
                Ok(Effect::Revoked {
                    subscription_type: subscription.kind.clone(),
                    status: subscription.status.clone(),
                })
            }
            "session_welcome" | "session_reconnect" => {
                validate_control(envelope)?;
                let session = payload.session.as_ref().ok_or("Missing EventSub session")?;
                if !valid_identifier(&session.id) {
                    return Err("Invalid EventSub session ID".into());
                }
                if envelope.metadata.message_type == "session_welcome" {
                    let timeout = session
                        .keepalive_timeout_seconds
                        .ok_or("Missing EventSub keepalive timeout")?;
                    if session.status != "connected"
                        || !(10..=600).contains(&timeout)
                        || session.reconnect_url.is_some()
                    {
                        return Err("Invalid EventSub welcome session".into());
                    }
                    Ok(Effect::Welcome {
                        session_id: session.id.clone(),
                        keepalive_timeout_seconds: timeout,
                    })
                } else {
                    let url = session
                        .reconnect_url
                        .as_deref()
                        .ok_or("Missing EventSub reconnect URL")?;
                    if session.status != "reconnecting"
                        || session.keepalive_timeout_seconds.is_some()
                    {
                        return Err("Invalid EventSub reconnect session".into());
                    }
                    validate_reconnect_url(url)?;
                    Ok(Effect::Reconnect {
                        url: url.to_owned(),
                    })
                }
            }
            "session_keepalive" => {
                validate_control(envelope)?;
                if payload.session.is_some() {
                    return Err("Invalid EventSub keepalive payload".into());
                }
                Ok(Effect::Keepalive)
            }
            _ => Err("Unsupported EventSub message type; delivery rejected".into()),
        }
    }

    fn subscription<'a>(&self, envelope: &'a Envelope) -> Result<&'a Subscription, String> {
        let subscription = envelope
            .payload
            .subscription
            .as_ref()
            .ok_or("Missing EventSub subscription")?;
        if !matches!(
            subscription.kind.as_str(),
            "channel.subscribe" | "channel.cheer" | "channel.chat.message"
        ) || subscription.version != "1"
            || envelope.metadata.subscription_type.as_deref() != Some(subscription.kind.as_str())
            || envelope.metadata.subscription_version.as_deref()
                != Some(subscription.version.as_str())
        {
            return Err("EventSub subscription type/version mismatch or not allowed".into());
        }
        if subscription.condition.broadcaster_user_id != self.state.broadcaster_id {
            return Err("EventSub subscription broadcaster mismatch".into());
        }
        let reader = subscription.condition.user_id.as_deref();
        if (subscription.kind == "channel.chat.message"
            && reader != Some(self.state.user_id.as_str()))
            || (subscription.kind != "channel.chat.message" && reader.is_some())
        {
            return Err("EventSub subscription reader mismatch".into());
        }
        if !valid_identifier(&subscription.id)
            || subscription.transport.method != "websocket"
            || !valid_identifier(&subscription.transport.session_id)
        {
            return Err("Invalid EventSub subscription transport or ID".into());
        }
        Ok(subscription)
    }
}

fn valid_user_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

#[derive(Deserialize)]
struct Envelope {
    metadata: Metadata,
    payload: Payload,
}

#[derive(Deserialize)]
struct Metadata {
    message_id: String,
    message_type: String,
    message_timestamp: String,
    subscription_type: Option<String>,
    subscription_version: Option<String>,
}

#[derive(Deserialize)]
struct Payload {
    session: Option<Session>,
    subscription: Option<Subscription>,
    event: Option<Event>,
}

#[derive(Deserialize)]
struct Session {
    id: String,
    status: String,
    keepalive_timeout_seconds: Option<u64>,
    reconnect_url: Option<String>,
}

#[derive(Deserialize)]
struct Subscription {
    id: String,
    status: String,
    #[serde(rename = "type")]
    kind: String,
    version: String,
    condition: Condition,
    transport: Transport,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Condition {
    broadcaster_user_id: String,
    user_id: Option<String>,
}

#[derive(Deserialize)]
struct Transport {
    method: String,
    session_id: String,
}

#[derive(Deserialize)]
struct Event {
    broadcaster_user_id: String,
    user_id: Option<String>,
    user_name: Option<String>,
    tier: Option<String>,
    is_gift: Option<bool>,
    is_anonymous: Option<bool>,
    bits: Option<u64>,
    chatter_user_id: Option<String>,
    chatter_user_name: Option<String>,
    message_id: Option<String>,
    message: Option<EventMessage>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EventMessage {
    Cheer(String),
    Chat { text: String },
}

fn validate_control(envelope: &Envelope) -> Result<(), String> {
    if envelope.metadata.subscription_type.is_some()
        || envelope.metadata.subscription_version.is_some()
        || envelope.payload.subscription.is_some()
        || envelope.payload.event.is_some()
    {
        return Err("Invalid EventSub control envelope".into());
    }
    Ok(())
}

fn validate_reconnect_url(raw: &str) -> Result<(), String> {
    // Url normalizes whitespace, slashes, hosts and empty userinfo. Reject such
    // ambiguities before parsing, and return the original string, never Url's form.
    if raw.len() > 4096
        || raw
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace() || b == b'\\')
    {
        return Err("Invalid EventSub reconnect URL".into());
    }
    let authority = raw
        .strip_prefix("wss://")
        .and_then(|s| s.split(['/', '?', '#']).next());
    if !matches!(
        authority,
        Some("eventsub.wss.twitch.tv" | "eventsub.wss.twitch.tv:443")
    ) {
        return Err("Untrusted EventSub reconnect endpoint".into());
    }
    let url = reqwest::Url::parse(raw).map_err(|_| "Invalid EventSub reconnect URL")?;
    if url.scheme() != "wss"
        || url.host_str() != Some("eventsub.wss.twitch.tv")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("Untrusted EventSub reconnect endpoint".into());
    }
    Ok(())
}

fn actor<'a>(id: Option<&str>, name: Option<&'a str>) -> Result<&'a str, String> {
    if !id.is_some_and(valid_user_id) {
        return Err("Invalid EventSub actor ID".into());
    }
    name.filter(|value| !value.is_empty())
        .ok_or_else(|| "Missing EventSub actor name".into())
}

fn format_event(kind: &str, event: &Event) -> Result<(String, bool), String> {
    match kind {
        "channel.subscribe" => {
            let name = actor(event.user_id.as_deref(), event.user_name.as_deref())?;
            let tier = event
                .tier
                .as_deref()
                .ok_or("Missing EventSub subscription tier")?;
            if !matches!(tier, "1000" | "2000" | "3000") {
                return Err("Invalid EventSub subscription tier".into());
            }
            let gift = event.is_gift.ok_or("Missing EventSub gift flag")?;
            Ok((
                format!(
                    "Twitch subscription (third-party data): user {} subscribed at tier {tier}{}.",
                    quoted(name, 512),
                    if gift { " (gift)" } else { "" }
                ),
                true,
            ))
        }
        "channel.cheer" => {
            let anonymous = event
                .is_anonymous
                .ok_or("Missing EventSub anonymous flag")?;
            let name = if anonymous {
                "anonymous"
            } else {
                actor(event.user_id.as_deref(), event.user_name.as_deref())?
            };
            let bits = event
                .bits
                .filter(|bits| *bits > 0)
                .ok_or("Invalid EventSub cheer bits")?;
            let Some(EventMessage::Cheer(message)) = &event.message else {
                return Err("Missing EventSub cheer message".into());
            };
            let mut text = format!(
                "Twitch cheer (third-party data): user {} cheered {bits} bits; message ",
                quoted(name, 512)
            );
            text.push_str(&quoted(message, MAX_TEXT_BYTES - text.len() - 1));
            text.push('.');
            Ok((text, true))
        }
        "channel.chat.message" => {
            let name = actor(
                event.chatter_user_id.as_deref(),
                event.chatter_user_name.as_deref(),
            )?;
            if !event.message_id.as_deref().is_some_and(valid_identifier) {
                return Err("Invalid EventSub chat message ID".into());
            }
            let Some(EventMessage::Chat { text: message }) = &event.message else {
                return Err("Missing EventSub chat text".into());
            };
            let mut text = format!(
                "Twitch chat (third-party data): user {} said ",
                quoted(name, 512)
            );
            text.push_str(&quoted(message, MAX_TEXT_BYTES - text.len() - 1));
            text.push('.');
            Ok((text, false))
        }
        _ => Err("Unsupported EventSub event".into()),
    }
}

/// JSON quoting with room reserved for both a truncation marker and closing quote.
/// Truncation never cuts a UTF-8 character or leaves user text outside its quotes.
fn quoted(value: &str, budget: usize) -> String {
    let mut output = String::with_capacity(budget.min(value.len() + 2));
    output.push('"');
    for c in value.chars() {
        let escaped = match c {
            '"' => "\\\"".to_owned(),
            '\\' => "\\\\".to_owned(),
            c if c.is_control() => format!("\\u{:04x}", u32::from(c)),
            c => c.to_string(),
        };
        if output.len() + escaped.len() + 4 > budget {
            output.push('…');
            break;
        }
        output.push_str(&escaped);
    }
    output.push('"');
    output
}

/// Strict RFC3339 calendar parser (1..=9999, nanosecond precision). Twitch does
/// not produce leap seconds; unsupported leap-second/unknown-offset timestamps
/// are rejected explicitly rather than interpreted using local time.
fn parse_timestamp(value: &str) -> Option<i128> {
    let b = value.as_bytes();
    if !(20..=35).contains(&b.len())
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    fn number(b: &[u8]) -> Option<i64> {
        b.iter().try_fold(0i64, |n, b| {
            b.is_ascii_digit().then(|| n * 10 + i64::from(b - b'0'))
        })
    }
    let year = number(&b[..4])?;
    let month = number(&b[5..7])?;
    let day = number(&b[8..10])?;
    let hour = number(&b[11..13])?;
    let minute = number(&b[14..16])?;
    let second = number(&b[17..19])?;
    if year == 0 || !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=days_in_month).contains(&day) {
        return None;
    }
    let mut cursor = 19;
    let mut nanos = 0i64;
    if b.get(cursor) == Some(&b'.') {
        cursor += 1;
        let start = cursor;
        while b.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        let digits = cursor - start;
        if !(1..=9).contains(&digits) {
            return None;
        }
        nanos = number(&b[start..cursor])? * 10i64.pow(9 - digits as u32);
    }
    let offset = match b.get(cursor)? {
        b'Z' | b'z' if cursor + 1 == b.len() => 0,
        sign @ (b'+' | b'-') if cursor + 6 == b.len() && b[cursor + 3] == b':' => {
            let hours = number(&b[cursor + 1..cursor + 3])?;
            let minutes = number(&b[cursor + 4..cursor + 6])?;
            if hours > 23 || minutes > 59 || (*sign == b'-' && hours == 0 && minutes == 0) {
                return None;
            }
            (hours * 3600 + minutes * 60) * if *sign == b'-' { -1 } else { 1 }
        }
        _ => return None,
    };
    // Gregorian days from civil date, relative to 1970-01-01.
    let y = year - i64::from(month <= 2);
    let era = y / 400;
    let yoe = y - era * 400;
    let m = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let seconds = days * 86400 + hour * 3600 + minute * 60 + second - offset;
    Some(i128::from(seconds) * 1_000_000_000 + i128::from(nanos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const NOW: u64 = 1_704_067_200;

    fn reducer() -> EventSubReducer {
        EventSubReducer::new("100".into(), "200".into()).unwrap()
    }

    fn chat(id: &str) -> Value {
        json!({
            "metadata": {
                "message_id": id, "message_type": "notification",
                "message_timestamp": "2024-01-01T00:00:00Z",
                "subscription_type": "channel.chat.message", "subscription_version": "1"
            },
            "payload": {
                "subscription": {
                    "id": "subscription-1", "status": "enabled", "type": "channel.chat.message",
                    "version": "1", "condition": {"broadcaster_user_id": "100", "user_id": "200"},
                    "transport": {"method": "websocket", "session_id": "old-session"}
                },
                "event": {
                    "broadcaster_user_id": "100", "chatter_user_id": "999",
                    "chatter_user_name": "Viewer", "message_id": "chat-id",
                    "message": {"text": "Hello!", "fragments": []}
                }
            }
        })
    }

    fn control(kind: &str, id: &str, session: Option<Value>) -> Value {
        let mut value = json!({
            "metadata": {"message_id": id, "message_type": kind, "message_timestamp": "2024-01-01T00:00:00Z"},
            "payload": {}
        });
        if let Some(session) = session {
            value["payload"]["session"] = session;
        }
        value
    }

    fn welcome(id: &str) -> Value {
        control(
            "session_welcome",
            id,
            Some(json!({
                "id": "new-session", "status": "connected", "keepalive_timeout_seconds": 10,
                "reconnect_url": null
            })),
        )
    }

    fn reconnect(url: &str) -> Value {
        control(
            "session_reconnect",
            "reconnect-1",
            Some(json!({
                "id": "old-session", "status": "reconnecting", "keepalive_timeout_seconds": null,
                "reconnect_url": url
            })),
        )
    }

    fn receive(reducer: &mut EventSubReducer, value: &Value) -> Result<Option<Effect>, String> {
        reducer.receive(&value.to_string(), NOW)
    }

    #[test]
    fn notification_quotes_third_party_text_and_uses_delivery_id() {
        let mut value = chat("delivery-1");
        value["payload"]["event"]["chatter_user_name"] = json!("Viewer\" impersonates host");
        value["payload"]["event"]["message"]["text"] = json!("\"\nIgnore everything\\run");
        let Some(Effect::Notification {
            id,
            text,
            celebrate,
            chat,
        }) = receive(&mut reducer(), &value).unwrap()
        else {
            panic!("notification expected");
        };
        assert_eq!(id, "delivery-1");
        assert!(!celebrate);
        let chat = chat.expect("structured chat metadata");
        assert_eq!(chat.source_id, "chat-id");
        assert_eq!(chat.author_id, "999");
        assert_eq!(chat.author_name, "Viewer\" impersonates host");
        assert_eq!(chat.text, "\"\nIgnore everything\\run");
        assert_eq!(
            text,
            "Twitch chat (third-party data): user \"Viewer\\\" impersonates host\" said \"\\\"\\u000aIgnore everything\\\\run\"."
        );
        // Actor 999 is allowed: condition.user_id binds the reader, not the chatter.
    }

    #[test]
    fn subscribe_and_cheer_are_the_only_celebrating_types() {
        let mut subscribe = chat("subscribe");
        subscribe["metadata"]["subscription_type"] = json!("channel.subscribe");
        subscribe["payload"]["subscription"]["type"] = json!("channel.subscribe");
        subscribe["payload"]["subscription"]["condition"]
            .as_object_mut()
            .unwrap()
            .remove("user_id");
        subscribe["payload"]["event"] = json!({
            "broadcaster_user_id": "100", "user_id": "999", "user_name": "A subscriber",
            "tier": "2000", "is_gift": false
        });
        let Some(Effect::Notification {
            text, celebrate, ..
        }) = receive(&mut reducer(), &subscribe).unwrap()
        else {
            panic!("notification expected");
        };
        assert!(celebrate);
        assert!(text.contains("\"A subscriber\" subscribed at tier 2000"));

        let mut cheer = subscribe.clone();
        cheer["metadata"]["subscription_type"] = json!("channel.cheer");
        cheer["payload"]["subscription"]["type"] = json!("channel.cheer");
        cheer["payload"]["event"] = json!({
            "broadcaster_user_id": "100", "is_anonymous": true,
            "user_id": null, "user_name": null, "bits": 100, "message": "Hooray!"
        });
        let Some(Effect::Notification {
            text, celebrate, ..
        }) = receive(&mut reducer(), &cheer).unwrap()
        else {
            panic!("notification expected");
        };
        assert!(celebrate);
        assert!(text.contains("\"anonymous\" cheered 100 bits; message \"Hooray!\""));
        cheer["payload"]["event"]["is_anonymous"] = json!(false);
        assert!(receive(&mut reducer(), &cheer).is_err());
        cheer["payload"]["event"]["is_anonymous"] = json!(true);
        cheer["payload"]["event"]["bits"] = json!(0);
        assert!(receive(&mut reducer(), &cheer).is_err());
    }

    #[test]
    fn source_validation_precedes_dedup_and_invalid_source_cannot_poison_it() {
        let original = chat("delivery-1");
        let mutations = [
            ("/metadata/subscription_type", json!("channel.cheer")),
            ("/metadata/subscription_version", json!("2")),
            ("/payload/subscription/type", json!("channel.follow")),
            ("/payload/subscription/version", json!("2")),
            (
                "/payload/subscription/status",
                json!("authorization_revoked"),
            ),
            (
                "/payload/subscription/condition/broadcaster_user_id",
                json!("101"),
            ),
            ("/payload/subscription/condition/user_id", json!("201")),
            ("/payload/event/broadcaster_user_id", json!("101")),
            ("/payload/subscription/transport/method", json!("webhook")),
            ("/payload/event/chatter_user_id", json!("not-an-id")),
            ("/payload/event/message", json!("not-a-chat-message")),
        ];
        for (path, replacement) in mutations {
            let mut value = original.clone();
            *value.pointer_mut(path).unwrap() = replacement;
            let mut reducer = reducer();
            assert!(receive(&mut reducer, &value).is_err(), "{path}");
            assert!(reducer.state.seen.is_empty(), "{path}");
            assert!(receive(&mut reducer, &original).unwrap().is_some());
            assert!(
                receive(&mut reducer, &value).is_err(),
                "duplicate spoof: {path}"
            );
            assert_eq!(receive(&mut reducer, &original).unwrap(), None);
        }
        let mut missing_reader = original;
        missing_reader["payload"]["subscription"]["condition"]
            .as_object_mut()
            .unwrap()
            .remove("user_id");
        assert!(receive(&mut reducer(), &missing_reader).is_err());
    }

    #[test]
    fn rejects_invalid_configured_identities() {
        for invalid in [
            "",
            "name",
            "100\n",
            "１２３",
            "12345678901234567890123456789012345",
        ] {
            assert!(EventSubReducer::new(invalid.into(), "200".into()).is_err());
            assert!(EventSubReducer::new("100".into(), invalid.into()).is_err());
        }
    }

    #[test]
    fn dedup_survives_snapshot_reconnect_and_welcome() {
        let mut reducer = reducer();
        let notification = chat("delivery-1");
        assert!(receive(&mut reducer, &notification).unwrap().is_some());
        let url = "wss://eventsub.wss.twitch.tv:443/ws?token=A%2fb&x=1+2";
        assert_eq!(
            receive(&mut reducer, &reconnect(url)).unwrap(),
            Some(Effect::Reconnect { url: url.into() })
        );
        let mut replacement = EventSubReducer::from_snapshot(reducer.snapshot());
        assert_eq!(
            receive(&mut replacement, &welcome("welcome-1")).unwrap(),
            Some(Effect::Welcome {
                session_id: "new-session".into(),
                keepalive_timeout_seconds: 10
            })
        );
        assert_eq!(receive(&mut replacement, &notification).unwrap(), None);
        let mut wrong_source = notification;
        wrong_source["payload"]["event"]["broadcaster_user_id"] = json!("101");
        assert!(receive(&mut replacement, &wrong_source).is_err());
    }

    #[test]
    fn dedup_capacity_rejects_unknown_notifications_and_controls_without_eviction() {
        let mut reducer = reducer();
        for index in 0..MAX_DEDUP_IDS {
            assert!(
                receive(&mut reducer, &chat(&format!("id-{index}")))
                    .unwrap()
                    .is_some()
            );
        }
        let full = reducer.snapshot();
        for unknown in [
            chat("extra"),
            welcome("extra-welcome"),
            control("session_keepalive", "extra-keepalive", None),
        ] {
            let error = receive(&mut reducer, &unknown).unwrap_err();
            assert!(error.contains("dedup capacity exhausted"));
            assert_eq!(reducer.snapshot(), full);
        }
        for index in 0..MAX_DEDUP_IDS {
            assert_eq!(
                receive(&mut reducer, &chat(&format!("id-{index}"))).unwrap(),
                None
            );
        }
        assert_eq!(reducer.snapshot(), full);

        let mut restored = EventSubReducer::from_snapshot(full.clone());
        assert!(receive(&mut restored, &chat("extra")).is_err());
        // A retry with a newer source timestamp cannot bypass the retained ID.
        let mut retry = chat("id-0");
        retry["metadata"]["message_timestamp"] = json!("2024-01-01T00:05:00Z");
        assert_eq!(
            restored
                .receive(&retry.to_string(), NOW + MAX_AGE_SECONDS)
                .unwrap(),
            None
        );
        assert_eq!(restored.state.seen, full.seen);
    }

    #[test]
    fn dedup_window_expires_inclusively_and_snapshot_prevents_clock_rollback_replay() {
        let mut reducer = reducer();
        for index in 0..MAX_DEDUP_IDS {
            receive(&mut reducer, &chat(&format!("id-{index}"))).unwrap();
        }
        let mut restored = EventSubReducer::from_snapshot(reducer.snapshot());
        let mut fresh = chat("fresh");
        fresh["metadata"]["message_timestamp"] = json!("2024-01-01T00:05:30Z");
        assert!(
            restored
                .receive(&fresh.to_string(), NOW + DEDUP_RETENTION_SECONDS)
                .unwrap_err()
                .contains("dedup capacity exhausted")
        );
        fresh["metadata"]["message_timestamp"] = json!("2024-01-01T00:05:31Z");
        assert!(
            restored
                .receive(&fresh.to_string(), NOW + DEDUP_RETENTION_SECONDS + 1)
                .unwrap()
                .is_some()
        );
        assert_eq!(restored.state.seen.len(), 1);
        let mut restored = EventSubReducer::from_snapshot(restored.snapshot());
        assert_eq!(restored.receive(&fresh.to_string(), NOW).unwrap(), None);
        let error = receive(&mut restored, &chat("id-0")).unwrap_err();
        assert!(error.contains("Stale EventSub message"));
    }

    #[test]
    fn dedup_expiry_retains_later_admissions() {
        let mut reducer = reducer();
        for index in 0..MAX_DEDUP_IDS - 1 {
            receive(&mut reducer, &chat(&format!("id-{index}"))).unwrap();
        }
        let mut later = chat("later");
        later["metadata"]["message_timestamp"] = json!("2024-01-01T00:00:30Z");
        reducer.receive(&later.to_string(), NOW + 30).unwrap();
        let mut fresh = chat("fresh");
        fresh["metadata"]["message_timestamp"] = json!("2024-01-01T00:05:31Z");
        assert!(
            reducer
                .receive(&fresh.to_string(), NOW + DEDUP_RETENTION_SECONDS + 1)
                .unwrap()
                .is_some()
        );
        later["metadata"]["message_timestamp"] = json!("2024-01-01T00:05:31Z");
        assert_eq!(
            reducer
                .receive(&later.to_string(), NOW + DEDUP_RETENTION_SECONDS + 1)
                .unwrap(),
            None
        );
        assert_eq!(reducer.state.seen.len(), 2);
    }

    #[test]
    fn invalid_source_cannot_expire_ids_or_advance_snapshot_clock() {
        let mut reducer = reducer();
        receive(&mut reducer, &chat("original")).unwrap();
        let before = reducer.snapshot();
        for id in ["original", "unknown"] {
            let mut invalid = chat(id);
            invalid["metadata"]["message_timestamp"] = json!("2024-01-01T00:05:31Z");
            invalid["payload"]["event"]["broadcaster_user_id"] = json!("101");
            assert!(
                reducer
                    .receive(&invalid.to_string(), NOW + DEDUP_RETENTION_SECONDS + 1)
                    .unwrap_err()
                    .contains("broadcaster mismatch")
            );
            assert_eq!(reducer.snapshot(), before);
        }
        assert_eq!(receive(&mut reducer, &chat("original")).unwrap(), None);
    }

    #[test]
    fn freshness_has_exact_nanosecond_boundaries_and_rejects_stale_duplicates() {
        for (timestamp, accepted) in [
            ("2023-12-31T23:55:00Z", true),
            ("2023-12-31T23:54:59.999999999Z", false),
            ("2024-01-01T00:00:30Z", true),
            ("2024-01-01T00:00:30.000000001Z", false),
            ("2024-01-01T01:00:00+01:00", true),
            ("2023-12-31T23:00:00-01:00", true),
            ("2024-01-01T00:00:00-00:00", false),
            ("2024-01-01", false),
        ] {
            let mut value = chat("time-test");
            value["metadata"]["message_timestamp"] = json!(timestamp);
            assert_eq!(
                receive(&mut reducer(), &value).is_ok(),
                accepted,
                "{timestamp}"
            );
        }
        let value = chat("stale-duplicate").to_string();
        let mut reducer = reducer();
        assert!(reducer.receive(&value, NOW).unwrap().is_some());
        assert!(reducer.receive(&value, NOW + MAX_AGE_SECONDS + 1).is_err());
        assert!(reducer.receive(&value, u64::MAX).is_err());
    }

    #[test]
    fn rfc3339_parser_validates_calendar_and_offsets() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("1969-12-31T23:59:59.999999999Z"), Some(-1));
        assert_eq!(
            parse_timestamp("2000-03-01T00:00:00Z").unwrap()
                - parse_timestamp("2000-02-28T00:00:00Z").unwrap(),
            2 * 86400 * 1_000_000_000
        );
        assert_eq!(
            parse_timestamp("2024-01-01t00:00:00.123456789z"),
            Some(i128::from(NOW) * 1_000_000_000 + 123_456_789)
        );
        for invalid in [
            "1900-02-29T00:00:00Z",
            "2023-02-29T00:00:00Z",
            "2024-04-31T00:00:00Z",
            "0000-01-01T00:00:00Z",
            "2024-13-01T00:00:00Z",
            "2024-00-01T00:00:00Z",
            "2024-01-00T00:00:00Z",
            "2024-01-01T24:00:00Z",
            "2024-01-01T00:60:00Z",
            "2024-01-01T00:00:60Z",
            "2024-01-01T00:00:00.Z",
            "2024-01-01T00:00:00.1234567890Z",
            "2024-01-01T00:00:00+24:00",
            "2024-01-01T00:00:00+00:60",
            "2024-01-01T00:00:00Zextra",
            "💥💥💥💥💥",
            "2024-01-01T00:00:00",
            "2024-01-01T00:00:00+00:0",
        ] {
            assert_eq!(parse_timestamp(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn reconnect_rejects_endpoint_confusion_and_preserves_cache_on_error() {
        for url in [
            "ws://eventsub.wss.twitch.tv/ws",
            "https://eventsub.wss.twitch.tv/ws",
            "wss://eventsub.wss.twitch.tv.evil.example/ws",
            "wss://evil.example/eventsub.wss.twitch.tv",
            "wss://user@eventsub.wss.twitch.tv/ws",
            "wss://@eventsub.wss.twitch.tv/ws",
            "wss://eventsub.wss.twitch.tv:444/ws",
            "wss://eventsub.wss.twitch.tv/ws#fragment",
            "wss://eventsub.wss.twitch.tv/ws#",
            "wss://eventsub.wss.twitch.tv./ws",
            "wss://eventsub.wss.twitch.tv\\@evil.example/ws",
            " wss://eventsub.wss.twitch.tv/ws",
            "wss://eventsub.wss.twitch.tv/w\ns",
            "wss://eventsub%2ewss.twitch.tv/ws",
        ] {
            let mut reducer = reducer();
            assert!(receive(&mut reducer, &reconnect(url)).is_err(), "{url:?}");
            assert!(reducer.state.seen.is_empty());
            assert!(
                receive(
                    &mut reducer,
                    &reconnect("wss://eventsub.wss.twitch.tv/ws?token=ok")
                )
                .unwrap()
                .is_some()
            );
        }
    }

    #[test]
    fn controls_and_revocations_are_explicit() {
        let mut reducer = reducer();
        assert_eq!(
            receive(
                &mut reducer,
                &control("session_keepalive", "keepalive", None)
            )
            .unwrap(),
            Some(Effect::Keepalive)
        );
        for timeout in [0, 9, 601] {
            let mut value = welcome("welcome");
            value["payload"]["session"]["keepalive_timeout_seconds"] = json!(timeout);
            assert!(receive(&mut reducer, &value).is_err());
        }
        for status in [
            "authorization_revoked",
            "user_removed",
            "version_removed",
            "future_reason",
        ] {
            let mut revoked = chat("revoked");
            revoked["metadata"]["message_type"] = json!("revocation");
            revoked["payload"].as_object_mut().unwrap().remove("event");
            revoked["payload"]["subscription"]["status"] = json!(status);
            assert_eq!(
                receive(
                    &mut EventSubReducer::new("100".into(), "200".into()).unwrap(),
                    &revoked
                )
                .unwrap(),
                Some(Effect::Revoked {
                    subscription_type: "channel.chat.message".into(),
                    status: status.into()
                })
            );
            revoked["payload"]["subscription"]["condition"]["user_id"] = json!("201");
            assert!(receive(&mut reducer, &revoked).is_err());
        }
        let mut invalid = chat("bad-revocation");
        invalid["metadata"]["message_type"] = json!("revocation");
        invalid["payload"].as_object_mut().unwrap().remove("event");
        assert!(receive(&mut reducer, &invalid).is_err()); // enabled is not revoked
        assert!(receive(&mut reducer, &control("unknown", "unknown", None)).is_err());
    }

    #[test]
    fn json_and_text_are_bounded_and_utf8_safe_even_with_escape_expansion() {
        let mut reducer = reducer();
        let value = chat("exact-bound").to_string();
        let padded = format!("{value}{}", " ".repeat(MAX_MESSAGE_BYTES - value.len()));
        assert!(reducer.receive(&padded, NOW).unwrap().is_some());
        assert!(reducer.receive(&(padded + " "), NOW).is_err());
        assert!(reducer.receive("{\"metadata\":", NOW).is_err());
        for text in ["🍵é漢字".repeat(2000), "\"\\\n".repeat(2000)] {
            let mut value = chat("large-text");
            value["payload"]["event"]["chatter_user_name"] = json!("漢".repeat(500));
            value["payload"]["event"]["message"]["text"] = json!(text);
            let Some(Effect::Notification { text, .. }) = receive(
                &mut EventSubReducer::new("100".into(), "200".into()).unwrap(),
                &value,
            )
            .unwrap() else {
                panic!("notification expected");
            };
            assert!(text.len() <= MAX_TEXT_BYTES);
            assert!(text.ends_with("…\"."));
            let quoted_message = text
                .split_once(" said ")
                .unwrap()
                .1
                .strip_suffix('.')
                .unwrap();
            let recovered: String = serde_json::from_str(quoted_message).unwrap();
            assert!(recovered.ends_with('…'));
        }
        for budget in 5..40 {
            let text = quoted("🍵\"\\\n漢字".repeat(20).as_str(), budget);
            assert!(text.len() <= budget);
            assert!(serde_json::from_str::<String>(&text).is_ok());
        }
    }

    #[test]
    fn malformed_or_duplicate_identity_fields_do_not_enter_snapshot() {
        let mut reducer = reducer();
        let raw = chat("duplicate-key").to_string();
        let duplicate = raw.replace(
            "\"broadcaster_user_id\":\"100\"",
            "\"broadcaster_user_id\":\"101\",\"broadcaster_user_id\":\"100\"",
        );
        assert!(reducer.receive(&duplicate, NOW).is_err());
        assert!(reducer.state.seen.is_empty());
        assert!(reducer.receive(&raw, NOW).unwrap().is_some());
    }
}
