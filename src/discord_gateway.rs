//! Deterministic Gateway connection policy. No network or token handling.
//! Transport owns TLS, uncompressed frame limits, write success, backoff jitter,
//! identify/session-start quotas and cancellation. Times are monotonic caller time.
use crate::{discord_messages::MAX_GATEWAY_BYTES, extension_protocol::ChatMessage};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const AUTH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Disconnected,
    Hello,
    Identifying,
    Resuming,
    Ready,
    Stopped,
}

/// Never includes a token. Caller adds its private bot token only at transport write.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Identify,
    Resume { session_id: String, sequence: u64 },
    Heartbeat { sequence: Option<u64> },
    Ready { bot_id: String },
    Chat(ChatMessage),
    ChatRejected { reason: &'static str },
    Reconnect { minimum_delay: Duration },
    Stopped { reason: &'static str },
}

struct Session {
    id: String,
    url: String,
}

pub struct Gateway {
    filter: crate::discord_messages::ChannelFilter,
    phase: Phase,
    session: Option<Session>,
    sequence: Option<u64>,
    interval: Option<Duration>,
    next_heartbeat: Option<Duration>,
    ack_deadline: Option<Duration>,
    phase_deadline: Option<Duration>,
    last_now: Duration,
}

#[derive(Deserialize)]
struct Packet {
    op: u8,
    #[serde(default)]
    t: Option<String>,
    #[serde(default)]
    s: Option<u64>,
    d: Value,
}

pub(crate) fn resume_url(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    url.scheme() == "wss"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(url.path(), "" | "/")
        && url.host_str().is_some_and(|host| {
            host == "gateway.discord.gg"
                || (host.starts_with("gateway-") && host.ends_with(".discord.gg"))
        })
}

impl Gateway {
    pub fn new(filter: crate::discord_messages::ChannelFilter) -> Self {
        Self {
            filter,
            phase: Phase::Disconnected,
            session: None,
            sequence: None,
            interval: None,
            next_heartbeat: None,
            ack_deadline: None,
            phase_deadline: None,
            last_now: Duration::ZERO,
        }
    }
    /// A validated resume endpoint only. Caller appends v10/JSON query parameters.
    pub fn resume_endpoint(&self) -> Option<&str> {
        self.session.as_ref().map(|s| s.url.as_str())
    }

    fn time(&mut self, now: Duration) -> Duration {
        self.last_now = self.last_now.max(now);
        self.last_now
    }
    fn clear_socket(&mut self) {
        self.interval = None;
        self.next_heartbeat = None;
        self.ack_deadline = None;
        self.phase_deadline = None;
    }
    fn clear_session(&mut self) {
        self.session = None;
        self.sequence = None;
    }
    fn reconnect(&mut self, delay: Duration) -> Vec<Effect> {
        self.phase = Phase::Disconnected;
        self.clear_socket();
        vec![Effect::Reconnect {
            minimum_delay: delay,
        }]
    }
    fn stop(&mut self, reason: &'static str) -> Vec<Effect> {
        self.phase = Phase::Stopped;
        self.clear_socket();
        self.clear_session();
        vec![Effect::Stopped { reason }]
    }
    /// Call only after the previous socket is closed and the new TLS connection succeeds.
    /// Stopped sessions require an explicit new Gateway after configuration is repaired.
    pub fn connected(&mut self, now: Duration) -> Result<(), &'static str> {
        if self.phase != Phase::Disconnected {
            return Err("Discord connection is not awaiting a socket");
        }
        let now = self.time(now);
        self.clear_socket();
        self.phase = Phase::Hello;
        self.phase_deadline = Some(now.saturating_add(HELLO_TIMEOUT));
        Ok(())
    }
    pub fn disconnected(&mut self, code: Option<u16>) -> Vec<Effect> {
        if matches!(self.phase, Phase::Stopped | Phase::Disconnected) {
            return vec![];
        }
        if matches!(code, Some(4004 | 4010 | 4011 | 4012 | 4013 | 4014)) {
            return self
                .stop("Discord rejected bot authentication, intents or shard configuration");
        }
        if matches!(code, Some(1000 | 1001 | 4007 | 4009)) {
            self.clear_session();
        }
        self.reconnect(if code == Some(4008) {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(1)
        })
    }
    /// Missing ACK at its deadline takes precedence over another heartbeat.
    pub fn tick(&mut self, now: Duration) -> Vec<Effect> {
        let now = self.time(now);
        if matches!(self.phase, Phase::Disconnected | Phase::Stopped) {
            return vec![];
        }
        if self.phase_deadline.is_some_and(|deadline| now >= deadline)
            || self.ack_deadline.is_some_and(|deadline| now >= deadline)
        {
            return self.reconnect(Duration::from_secs(1));
        }
        if self.next_heartbeat.is_some_and(|deadline| now >= deadline) {
            let interval = self.interval.expect("scheduled heartbeat has interval");
            self.next_heartbeat = Some(now.saturating_add(interval));
            return self.heartbeat(now);
        }
        vec![]
    }
    fn heartbeat(&mut self, now: Duration) -> Vec<Effect> {
        let interval = self.interval.expect("heartbeat follows Hello");
        // An extra server-requested heartbeat must not extend an outstanding ACK deadline.
        self.ack_deadline
            .get_or_insert(now.saturating_add(interval));
        vec![Effect::Heartbeat {
            sequence: self.sequence,
        }]
    }
    /// first_heartbeat_jitter is a caller-randomized fraction in [0,1), used only on Hello.
    /// On Err the transport must close the socket and call disconnected; do not continue.
    pub fn packet(
        &mut self,
        bytes: &[u8],
        now: Duration,
        first_heartbeat_jitter: f64,
    ) -> Result<Vec<Effect>, &'static str> {
        if matches!(self.phase, Phase::Disconnected | Phase::Stopped) {
            return Err("Discord packet without an active socket");
        }
        if bytes.len() > MAX_GATEWAY_BYTES {
            return Err("Discord Gateway packet exceeds the input limit");
        }
        let packet: Packet =
            serde_json::from_slice(bytes).map_err(|_| "Malformed Discord Gateway packet")?;
        let now = self.time(now);
        // ACK only the heartbeat already outstanding on arrival. An unsolicited
        // or duplicate ACK must not acknowledge a heartbeat tick emits now.
        if packet.op == 11 && self.ack_deadline.is_some_and(|deadline| now < deadline) {
            self.ack_deadline = None;
        }
        let timeout = self.tick(now);
        if !timeout.is_empty() {
            // A due heartbeat alone may accompany this packet. Terminal timeout must
            // discard it; the caller closes this socket before processing more input.
            if self.phase == Phase::Disconnected {
                return Ok(timeout);
            }
        }
        let mut effects = timeout;
        match packet.op {
            10 => {
                if self.phase != Phase::Hello {
                    return Err("Duplicate Discord Hello");
                }
                let ms = packet
                    .d
                    .get("heartbeat_interval")
                    .and_then(Value::as_u64)
                    .filter(|ms| (1000..=300_000).contains(ms))
                    .ok_or("Invalid Discord heartbeat interval")?;
                if !first_heartbeat_jitter.is_finite()
                    || !(0.0..1.0).contains(&first_heartbeat_jitter)
                {
                    return Err("Invalid Discord heartbeat jitter");
                }
                let interval = Duration::from_millis(ms);
                self.interval = Some(interval);
                self.next_heartbeat =
                    Some(now.saturating_add(interval.mul_f64(first_heartbeat_jitter)));
                self.phase_deadline = Some(now.saturating_add(AUTH_TIMEOUT));
                if let (Some(session), Some(sequence)) = (&self.session, self.sequence) {
                    effects.push(Effect::Resume {
                        session_id: session.id.clone(),
                        sequence,
                    });
                    self.phase = Phase::Resuming;
                } else {
                    effects.push(Effect::Identify);
                    self.phase = Phase::Identifying;
                }
            }
            1 => {
                if self.interval.is_none() {
                    return Err("Discord heartbeat before Hello");
                }
                // tick may already have emitted a heartbeat at this exact time.
                if !effects
                    .iter()
                    .any(|effect| matches!(effect, Effect::Heartbeat { .. }))
                {
                    effects.extend(self.heartbeat(now));
                }
            }
            11 => {
                if self.interval.is_none() {
                    return Err("Discord ACK before Hello");
                }
            }
            7 => return Ok(self.reconnect(Duration::from_secs(1))),
            9 => {
                let resumable = packet
                    .d
                    .as_bool()
                    .ok_or("Invalid Discord invalid-session packet")?;
                if !resumable {
                    self.clear_session();
                }
                // Caller adds randomized backoff; never Identify again on this socket.
                return Ok(self.reconnect(Duration::from_secs(5)));
            }
            0 => {
                let sequence = packet.s.ok_or("Discord dispatch lacks sequence")?;
                if self.sequence.is_some_and(|previous| sequence <= previous) {
                    return Ok(effects);
                }
                let event = packet
                    .t
                    .as_deref()
                    .ok_or("Discord dispatch lacks event name")?;
                if event == "READY" {
                    if self.phase != Phase::Identifying {
                        return Err("Unexpected Discord READY");
                    }
                    let id = packet
                        .d
                        .get("session_id")
                        .and_then(Value::as_str)
                        .filter(|id| {
                            !id.is_empty()
                                && id.len() <= 128
                                && !id.chars().any(|c| c.is_control() || c.is_whitespace())
                        })
                        .ok_or("Invalid Discord session identity")?;
                    let url = packet
                        .d
                        .get("resume_gateway_url")
                        .and_then(Value::as_str)
                        .filter(|url| resume_url(url))
                        .ok_or("Invalid Discord resume endpoint")?;
                    let user = packet.d.get("user").ok_or("Missing Discord bot identity")?;
                    let bot = user
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|id| {
                            !id.starts_with('0')
                                && id.parse::<u64>().is_ok()
                                && id.bytes().all(|b| b.is_ascii_digit())
                        })
                        .ok_or("Invalid Discord bot identity")?;
                    if user.get("bot").and_then(Value::as_bool) != Some(true) {
                        return Err("Discord requires a bot account");
                    }
                    self.session = Some(Session {
                        id: id.into(),
                        url: url.into(),
                    });
                    self.phase = Phase::Ready;
                    self.phase_deadline = None;
                    effects.push(Effect::Ready { bot_id: bot.into() });
                } else if event == "RESUMED" {
                    if self.phase != Phase::Resuming {
                        return Err("Unexpected Discord RESUMED");
                    }
                    self.phase = Phase::Ready;
                    self.phase_deadline = None;
                } else {
                    if !matches!(self.phase, Phase::Ready | Phase::Resuming) {
                        return Err("Discord dispatch before authenticated session");
                    }
                    match self.filter.decode(bytes) {
                        Ok(Some(chat)) => effects.push(Effect::Chat(chat)),
                        Ok(None) => {}
                        Err(reason) => effects.push(Effect::ChatRejected { reason }),
                    }
                }
                self.sequence = Some(sequence);
            }
            _ => {}
        }
        Ok(effects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn gateway() -> Gateway {
        let mut gateway = Gateway::new(
            crate::discord_messages::ChannelFilter::new("111".into(), "222".into()).unwrap(),
        );
        gateway.connected(Duration::ZERO).unwrap();
        gateway
    }
    fn packet(g: &mut Gateway, value: Value, ms: u64) -> Vec<Effect> {
        g.packet(
            &serde_json::to_vec(&value).unwrap(),
            Duration::from_millis(ms),
            0.5,
        )
        .unwrap()
    }
    fn hello(g: &mut Gateway, ms: u64) -> Vec<Effect> {
        packet(g, json!({"op":10,"d":{"heartbeat_interval":10000}}), ms)
    }
    fn ready(g: &mut Gateway) {
        packet(
            g,
            json!({"op":0,"t":"READY","s":1,"d":{"session_id":"session1",
            "resume_gateway_url":"wss://gateway.discord.gg","user":{"id":"444","bot":true}}}),
            1,
        );
    }
    #[test]
    fn hello_identifies_then_heartbeats_and_requires_ack() {
        let mut g = gateway();
        assert_eq!(hello(&mut g, 0), vec![Effect::Identify]);
        ready(&mut g);
        assert!(g.tick(Duration::from_millis(4999)).is_empty());
        assert_eq!(
            g.tick(Duration::from_secs(5)),
            vec![Effect::Heartbeat { sequence: Some(1) }]
        );
        packet(&mut g, json!({"op":11,"d":null}), 6000);
        assert_eq!(
            g.tick(Duration::from_secs(15)),
            vec![Effect::Heartbeat { sequence: Some(1) }]
        );
        assert!(matches!(
            g.tick(Duration::from_secs(25)).as_slice(),
            [Effect::Reconnect { .. }]
        ));
        assert!(g.tick(Duration::from_secs(30)).is_empty());
    }
    #[test]
    fn resume_retains_sequence_and_validated_endpoint() {
        let mut g = gateway();
        hello(&mut g, 0);
        ready(&mut g);
        g.disconnected(None);
        assert_eq!(g.resume_endpoint(), Some("wss://gateway.discord.gg"));
        g.connected(Duration::from_secs(2)).unwrap();
        assert_eq!(
            hello(&mut g, 2000),
            vec![Effect::Resume {
                session_id: "session1".into(),
                sequence: 1
            }]
        );
        packet(&mut g, json!({"op":0,"t":"RESUMED","s":2,"d":{}}), 2001);
        assert_eq!(g.phase, Phase::Ready);
    }
    #[test]
    fn invalid_session_and_close_codes_distinguish_retry_from_fatal() {
        for code in [4004, 4010, 4011, 4012, 4013, 4014] {
            let mut g = gateway();
            hello(&mut g, 0);
            ready(&mut g);
            assert!(matches!(
                g.disconnected(Some(code)).as_slice(),
                [Effect::Stopped { .. }]
            ));
            assert!(g.connected(Duration::from_secs(3)).is_err());
            assert!(g.resume_endpoint().is_none());
        }
        for code in [1000, 1001, 4007, 4009] {
            let mut g = gateway();
            hello(&mut g, 0);
            ready(&mut g);
            g.disconnected(Some(code));
            assert!(g.resume_endpoint().is_none());
            g.connected(Duration::from_secs(2)).unwrap();
            assert_eq!(hello(&mut g, 2000), vec![Effect::Identify]);
        }
        for resumable in [false, true] {
            let mut g = gateway();
            hello(&mut g, 0);
            ready(&mut g);
            assert_eq!(
                packet(&mut g, json!({"op":9,"d":resumable}), 2),
                vec![Effect::Reconnect {
                    minimum_delay: Duration::from_secs(5)
                }]
            );
            assert_eq!(g.resume_endpoint().is_some(), resumable);
        }
    }
    #[test]
    fn server_heartbeats_do_not_postpone_ack_deadline() {
        let mut g = gateway();
        hello(&mut g, 0);
        ready(&mut g);
        assert!(matches!(
            packet(&mut g, json!({"op":1,"d":null}), 1000).as_slice(),
            [Effect::Heartbeat { .. }]
        ));
        packet(&mut g, json!({"op":1,"d":null}), 2000);
        assert!(matches!(
            g.tick(Duration::from_secs(11)).as_slice(),
            [Effect::Reconnect { .. }]
        ));
    }
    #[test]
    fn hello_and_authentication_have_bounded_deadlines() {
        let mut g = gateway();
        assert!(matches!(
            g.tick(Duration::from_secs(10)).as_slice(),
            [Effect::Reconnect { .. }]
        ));
        let mut g = gateway();
        hello(&mut g, 0);
        // ACKs cannot keep an unready session alive indefinitely.
        for ms in [5000, 10000, 15000, 20000, 25000] {
            packet(&mut g, json!({"op":11,"d":null}), ms);
        }
        assert!(matches!(
            g.tick(Duration::from_secs(30)).as_slice(),
            [Effect::Reconnect { .. }]
        ));
    }
    #[test]
    fn rejects_untrusted_resume_urls_and_non_bot_ready() {
        for url in [
            "ws://gateway.discord.gg",
            "wss://evil.example",
            "wss://gateway.discord.gg.evil.example",
            "wss://user@gateway.discord.gg",
            "wss://gateway.discord.gg/path",
            "wss://gateway.discord.gg?token=x",
        ] {
            assert!(!resume_url(url));
        }
        assert!(resume_url("wss://gateway-us-east1-b.discord.gg"));
        let mut g = gateway();
        hello(&mut g, 0);
        let bad = json!({"op":0,"t":"READY","s":1,"d":{"session_id":"s","resume_gateway_url":"wss://gateway.discord.gg","user":{"id":"444","bot":false}}});
        assert!(
            g.packet(&serde_json::to_vec(&bad).unwrap(), Duration::ZERO, 0.5)
                .is_err()
        );
        assert!(g.resume_endpoint().is_none());
    }
    #[test]
    fn replay_dispatch_is_attributed_and_duplicate_sequences_are_ignored() {
        let mut g = gateway();
        hello(&mut g, 0);
        ready(&mut g);
        let chat = json!({"op":0,"t":"MESSAGE_CREATE","s":2,"d":{"id":"555","guild_id":"111","channel_id":"222","type":0,
            "content":"hello","author":{"id":"666","username":"Viewer"}}});
        assert!(matches!(
            packet(&mut g, chat.clone(), 2).as_slice(),
            [Effect::Chat(_)]
        ));
        assert!(packet(&mut g, chat, 3).is_empty());
        let mut g = gateway();
        hello(&mut g, 0);
        assert!(
            g.packet(
                br#"{"op":0,"t":"MESSAGE_CREATE","s":2,"d":{}}"#,
                Duration::ZERO,
                0.5
            )
            .is_err()
        );
    }
    #[test]
    fn oversized_chat_is_rejected_without_killing_or_replaying_session() {
        let mut g = gateway();
        hello(&mut g, 0);
        ready(&mut g);
        let mut chat = json!({"op":0,"t":"MESSAGE_CREATE","s":2,"d":{"id":"555","guild_id":"111","channel_id":"222","type":0,
            "content":"x".repeat(2049),"author":{"id":"666","username":"Viewer"}}});
        assert!(matches!(
            packet(&mut g, chat.clone(), 2).as_slice(),
            [Effect::ChatRejected { .. }]
        ));
        assert_eq!(g.phase, Phase::Ready);
        assert!(packet(&mut g, chat.clone(), 3).is_empty());
        chat["s"] = json!(3);
        chat["d"]["id"] = json!("556");
        chat["d"]["content"] = json!("next valid message");
        assert!(matches!(
            packet(&mut g, chat, 4).as_slice(),
            [Effect::Chat(_)]
        ));
    }

    #[test]
    fn unsolicited_ack_cannot_acknowledge_a_newly_scheduled_heartbeat() {
        let mut g = gateway();
        hello(&mut g, 0);
        ready(&mut g);
        assert_eq!(
            packet(&mut g, json!({"op":11,"d":null}), 5000),
            vec![Effect::Heartbeat { sequence: Some(1) }]
        );
        assert!(matches!(
            g.tick(Duration::from_secs(15)).as_slice(),
            [Effect::Reconnect { .. }]
        ));
    }

    #[test]
    fn resumed_replay_advances_checkpoint_before_resumed_confirmation() {
        let mut g = gateway();
        hello(&mut g, 0);
        ready(&mut g);
        g.disconnected(None);
        g.connected(Duration::from_secs(1)).unwrap();
        hello(&mut g, 1000);
        let chat = json!({"op":0,"t":"MESSAGE_CREATE","s":2,"d":{"id":"555","guild_id":"111","channel_id":"222","type":0,
            "content":"original replay","author":{"id":"666","username":"Viewer"}}});
        assert!(matches!(
            packet(&mut g, chat.clone(), 1001).as_slice(),
            [Effect::Chat(_)]
        ));
        assert_eq!(g.phase, Phase::Resuming);
        assert!(packet(&mut g, chat, 1002).is_empty());
        g.disconnected(None);
        g.connected(Duration::from_secs(2)).unwrap();
        assert_eq!(
            hello(&mut g, 2000),
            vec![Effect::Resume {
                session_id: "session1".into(),
                sequence: 2
            }]
        );
        packet(&mut g, json!({"op":0,"t":"RESUMED","s":3,"d":{}}), 2001);
        assert_eq!(g.phase, Phase::Ready);
    }

    #[test]
    fn server_reconnect_and_rate_limit_request_bounded_delays() {
        let mut g = gateway();
        hello(&mut g, 0);
        ready(&mut g);
        assert_eq!(
            packet(&mut g, json!({"op":7,"d":null}), 2),
            vec![Effect::Reconnect {
                minimum_delay: Duration::from_secs(1)
            }]
        );
        assert!(g.tick(Duration::from_secs(100)).is_empty());
        assert!(g.disconnected(None).is_empty());
        g.connected(Duration::from_secs(101)).unwrap();
        hello(&mut g, 101000);
        assert_eq!(
            g.disconnected(Some(4008)),
            vec![Effect::Reconnect {
                minimum_delay: Duration::from_secs(60)
            }]
        );
        assert!(g.resume_endpoint().is_some());
    }

    #[test]
    fn rejects_duplicate_hello_bad_intervals_and_oversized_frames() {
        let mut g = gateway();
        hello(&mut g, 0);
        assert!(
            g.packet(
                br#"{"op":10,"d":{"heartbeat_interval":1000}}"#,
                Duration::ZERO,
                0.5
            )
            .is_err()
        );
        for interval in [0, 999, 300001] {
            let mut g = gateway();
            assert!(
                g.packet(
                    &serde_json::to_vec(&json!({"op":10,"d":{"heartbeat_interval":interval}}))
                        .unwrap(),
                    Duration::ZERO,
                    0.5
                )
                .is_err()
            );
        }
        let mut g = gateway();
        assert!(
            g.packet(&vec![b' '; MAX_GATEWAY_BYTES + 1], Duration::ZERO, 0.5)
                .is_err()
        );
    }
}
