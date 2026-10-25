//! Bounded Discord chat-to-host bridge. No I/O, credentials or service calls.
//! A transport must stop on host/protocol errors and must not retry an action
//! whose write or result is uncertain. Only an explicit Busy refusal is retried.
use crate::extension_protocol::*;
use std::{collections::VecDeque, time::Duration};
use uuid::Uuid;

const CAPACITY: usize = 16;
const SEEN_CAPACITY: usize = 256;
const TTL: Duration = Duration::from_secs(15);
const SEEN_TTL: Duration = Duration::from_secs(60);
const RESULT_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_DELAY: Duration = Duration::from_secs(1);

/// Read explicit saved server/channel IDs; never infer a Discord destination from
/// a host avatar, display name, or arbitrary message payload.
pub fn configured_channel(
    settings: &crate::extension_settings::Settings,
) -> Result<crate::discord_messages::ChannelFilter, &'static str> {
    use crate::extension_settings::SettingValue;
    let value = |key| match settings.get(key) {
        Some(SettingValue::Text(value)) => Ok(value.clone()),
        _ => Err("Discord requires explicit server and channel IDs"),
    };
    crate::discord_messages::ChannelFilter::new(value("guild_id")?, value("channel_id")?)
}

struct Offer {
    message: ChatMessage,
    target: Target,
    expires: Duration,
    not_before: Duration,
    attempts: u8,
}
struct Pending {
    id: String,
    offer: Offer,
    deadline: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    Stopped,
    NoTarget,
    NoPermission,
    Invalid,
    Duplicate,
    Full,
}

pub struct HostBridge {
    session: Uuid,
    generation: u64,
    host_sequence: u64,
    action_sequence: u64,
    allowed: bool,
    target: Option<Target>,
    stopped: bool,
    queue: VecDeque<Offer>,
    pending: Option<Pending>,
    seen: VecDeque<(String, Duration)>,
    last_now: Duration,
}
impl HostBridge {
    pub fn new(handshake: &HostEnvelope) -> Result<Self, &'static str> {
        handshake
            .validate()
            .map_err(|_| "Invalid Discord host handshake")?;
        let HostMessage::Handshake { capabilities, .. } = &handshake.message else {
            return Err("Discord worker requires a host handshake");
        };
        Ok(Self {
            session: handshake.session_id,
            generation: handshake.process_generation,
            host_sequence: handshake.sequence,
            action_sequence: 0,
            allowed: capabilities.contains(&Capability::MessageDeliver),
            target: None,
            stopped: false,
            queue: VecDeque::new(),
            pending: None,
            seen: VecDeque::new(),
            last_now: Duration::ZERO,
        })
    }
    fn time(&mut self, now: Duration) -> Duration {
        self.last_now = self.last_now.max(now);
        self.last_now
    }
    fn expire(&mut self, now: Duration) {
        self.queue.retain(|offer| offer.expires > now);
        self.seen
            .retain(|(_, at)| now.saturating_sub(*at) < SEEN_TTL);
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| now >= pending.deadline || now >= pending.offer.expires)
        {
            // Uncertain acceptance: drop rather than resend after timeout.
            self.pending = None;
        }
    }
    /// Sticky quiescence for host EOF, failed output, cancellation or app shutdown.
    pub fn stop(&mut self) {
        self.stopped = true;
        self.target = None;
        self.queue.clear();
        self.pending = None;
    }
    pub fn stopped(&self) -> bool {
        self.stopped
    }
    /// A status query only; offer() still validates live admission and target.
    pub fn ready_for_chat(&self) -> bool {
        !self.stopped && self.allowed && self.target.is_some()
    }
    pub fn offer(&mut self, message: ChatMessage, now: Duration) -> Result<(), Rejection> {
        let now = self.time(now);
        self.expire(now);
        if self.stopped {
            return Err(Rejection::Stopped);
        }
        if !self.allowed {
            return Err(Rejection::NoPermission);
        }
        let target = self.target.ok_or(Rejection::NoTarget)?;
        if message.validate().is_err() {
            return Err(Rejection::Invalid);
        }
        if self.seen.iter().any(|(id, _)| id == &message.source_id) {
            return Err(Rejection::Duplicate);
        }
        if self.queue.len() + usize::from(self.pending.is_some()) >= CAPACITY
            || self.seen.len() >= SEEN_CAPACITY
        {
            return Err(Rejection::Full);
        }
        self.seen.push_back((message.source_id.clone(), now));
        self.queue.push_back(Offer {
            message,
            target,
            expires: now.saturating_add(TTL),
            not_before: now,
            attempts: 0,
        });
        Ok(())
    }
    /// Reserves one correlated action before transport write. Failed/partial writes
    /// must call stop(); obtaining this envelope is not evidence of host acceptance.
    pub fn next_action(&mut self, now: Duration) -> Result<Option<ActionEnvelope>, &'static str> {
        let now = self.time(now);
        self.expire(now);
        if self.stopped || !self.allowed || self.pending.is_some() {
            return Ok(None);
        }
        while self
            .queue
            .front()
            .is_some_and(|offer| Some(offer.target) != self.target)
        {
            self.queue.pop_front();
        }
        if self
            .queue
            .front()
            .is_none_or(|offer| offer.not_before > now)
        {
            return Ok(None);
        }
        let mut offer = self.queue.pop_front().expect("checked front");
        let Some(sequence) = self.action_sequence.checked_add(1) else {
            self.stop();
            return Err("Discord host action sequence exhausted");
        };
        self.action_sequence = sequence;
        offer.attempts += 1;
        let id = format!("discord-{sequence}");
        let action = ActionEnvelope {
            api_version: API_VERSION,
            message_type: ActionEnvelopeType::Action,
            id: id.clone(),
            sequence,
            session_id: self.session,
            process_generation: self.generation,
            target: offer.target,
            action: Action::ChatOffer {
                message: offer.message.clone(),
            },
        };
        self.pending = Some(Pending {
            id,
            deadline: now.saturating_add(RESULT_TIMEOUT),
            offer,
        });
        Ok(Some(action))
    }
    pub fn host(&mut self, envelope: HostEnvelope, now: Duration) -> Result<(), &'static str> {
        if self.stopped {
            return Ok(());
        }
        if envelope.validate().is_err()
            || envelope.session_id != self.session
            || envelope.process_generation != self.generation
        {
            self.stop();
            return Err("Discord host protocol or incarnation changed");
        }
        if envelope.sequence <= self.host_sequence {
            return Ok(());
        }
        self.host_sequence = envelope.sequence;
        let now = self.time(now);
        self.expire(now);
        match envelope.message {
            HostMessage::Handshake { .. } => {
                self.stop();
                return Err("Duplicate Discord host handshake");
            }
            HostMessage::Event {
                event: HostEvent::TargetChanged { target },
            } => {
                if target != self.target {
                    self.queue.clear();
                    self.pending = None;
                }
                self.target = target;
                // Keep source dedup across avatar changes: resumed old chat must
                // not acquire a fresh target merely because the avatar changed.
            }
            HostMessage::Event {
                event: HostEvent::AppStopping { .. },
            } => self.stop(),
            HostMessage::Result { id, target, result }
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.id == id && pending.offer.target == target) =>
            {
                let mut pending = self.pending.take().expect("matched pending");
                if matches!(
                    result,
                    ActionResult::Error {
                        code: ErrorCode::Busy,
                        ..
                    }
                ) && Some(target) == self.target
                    && pending.offer.attempts < 3
                    && pending.offer.expires > now.saturating_add(RETRY_DELAY)
                {
                    pending.offer.not_before = now.saturating_add(RETRY_DELAY);
                    self.queue.push_front(pending.offer);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    fn target(generation: u64) -> Target {
        Target {
            uuid: Uuid::from_u128(2),
            generation,
        }
    }
    fn envelope(sequence: u64, message: HostMessage) -> HostEnvelope {
        HostEnvelope {
            api_version: API_VERSION,
            message_type: HostEnvelopeType::Host,
            session_id: Uuid::from_u128(1),
            sequence,
            process_generation: 7,
            message,
        }
    }
    fn bridge(allowed: bool) -> HostBridge {
        HostBridge::new(&envelope(
            1,
            HostMessage::Handshake {
                origin: IntegrationOrigin {
                    integration_id: "discord-fixture".into(),
                    display_name: "Discord".into(),
                },
                settings: Default::default(),
                capabilities: if allowed {
                    vec![Capability::MessageDeliver]
                } else {
                    vec![]
                },
            },
        ))
        .unwrap()
    }
    fn select(bridge: &mut HostBridge, seq: u64, generation: u64, now: u64) {
        bridge
            .host(
                envelope(
                    seq,
                    HostMessage::Event {
                        event: HostEvent::TargetChanged {
                            target: Some(target(generation)),
                        },
                    },
                ),
                Duration::from_secs(now),
            )
            .unwrap();
    }
    fn chat(id: usize) -> ChatMessage {
        ChatMessage {
            source_id: format!("discord:111:222:{id}"),
            author_id: "444".into(),
            author_name: "Viewer".into(),
            text: "original text".into(),
        }
    }
    fn result(
        bridge: &mut HostBridge,
        seq: u64,
        action: &ActionEnvelope,
        code: Option<ErrorCode>,
        now: u64,
    ) {
        bridge
            .host(
                envelope(
                    seq,
                    HostMessage::Result {
                        id: action.id.clone(),
                        target: action.target,
                        result: match code {
                            Some(code) => ActionResult::Error {
                                code,
                                message: "Fixed test refusal".into(),
                            },
                            None => ActionResult::Ok {
                                value: ResultValue::Accepted {},
                            },
                        },
                    },
                ),
                Duration::from_secs(now),
            )
            .unwrap();
    }
    #[test]
    fn configuration_requires_explicit_numeric_saved_destinations() {
        use crate::extension_settings::{SettingValue, Settings};
        let mut settings = Settings::new();
        assert!(configured_channel(&settings).is_err());
        settings.insert("guild_id".into(), SettingValue::Text("111".into()));
        settings.insert("channel_id".into(), SettingValue::Text("222".into()));
        assert!(configured_channel(&settings).is_ok());
        settings.insert("channel_id".into(), SettingValue::Text("general".into()));
        assert!(configured_channel(&settings).is_err());
        settings.insert("channel_id".into(), SettingValue::Integer(222));
        assert!(configured_channel(&settings).is_err());
    }

    #[test]
    fn requires_permission_and_target_then_emits_valid_attributed_offer() {
        let mut b = bridge(false);
        select(&mut b, 2, 1, 0);
        assert_eq!(
            b.offer(chat(1), Duration::ZERO),
            Err(Rejection::NoPermission)
        );
        let mut b = bridge(true);
        assert_eq!(b.offer(chat(1), Duration::ZERO), Err(Rejection::NoTarget));
        select(&mut b, 2, 1, 0);
        b.offer(chat(1), Duration::ZERO).unwrap();
        let action = b.next_action(Duration::ZERO).unwrap().unwrap();
        assert_eq!(action.target, target(1));
        assert_eq!(action.sequence, 1);
        assert_eq!(action.action, Action::ChatOffer { message: chat(1) });
        assert!(encode_action_line(&action).is_ok());
        assert!(b.next_action(Duration::ZERO).unwrap().is_none());
    }
    #[test]
    fn explicit_busy_retries_are_bounded_and_keep_original_expiry() {
        let mut b = bridge(true);
        select(&mut b, 2, 1, 0);
        b.offer(chat(1), Duration::ZERO).unwrap();
        let mut ids = BTreeSet::new();
        for attempt in 0..3 {
            let now = Duration::from_secs(attempt);
            let action = b.next_action(now).unwrap().unwrap();
            ids.insert(action.id.clone());
            result(&mut b, 3 + attempt, &action, Some(ErrorCode::Busy), attempt);
            assert!(b.next_action(now).unwrap().is_none());
        }
        assert_eq!(ids.len(), 3);
        assert!(b.next_action(Duration::from_secs(4)).unwrap().is_none());
    }
    #[test]
    fn accepted_errors_and_missing_results_are_never_replayed() {
        for code in [
            None,
            Some(ErrorCode::Unavailable),
            Some(ErrorCode::PermissionDenied),
        ] {
            let mut b = bridge(true);
            select(&mut b, 2, 1, 0);
            b.offer(chat(1), Duration::ZERO).unwrap();
            let action = b.next_action(Duration::ZERO).unwrap().unwrap();
            result(&mut b, 3, &action, code, 1);
            assert!(b.next_action(Duration::from_secs(2)).unwrap().is_none());
            assert_eq!(
                b.offer(chat(1), Duration::from_secs(2)),
                Err(Rejection::Duplicate)
            );
        }
        let mut b = bridge(true);
        select(&mut b, 2, 1, 0);
        b.offer(chat(1), Duration::ZERO).unwrap();
        let action = b.next_action(Duration::ZERO).unwrap().unwrap();
        assert!(b.next_action(Duration::from_secs(10)).unwrap().is_none());
        result(&mut b, 3, &action, Some(ErrorCode::Busy), 11);
        assert!(b.next_action(Duration::from_secs(12)).unwrap().is_none());
    }
    #[test]
    fn target_switch_discards_pending_and_queue_but_preserves_dedup() {
        let mut b = bridge(true);
        select(&mut b, 2, 1, 0);
        b.offer(chat(1), Duration::ZERO).unwrap();
        let action = b.next_action(Duration::ZERO).unwrap().unwrap();
        b.offer(chat(2), Duration::ZERO).unwrap();
        select(&mut b, 3, 2, 1);
        result(&mut b, 4, &action, Some(ErrorCode::Busy), 2);
        assert!(b.next_action(Duration::from_secs(3)).unwrap().is_none());
        assert_eq!(
            b.offer(chat(1), Duration::from_secs(3)),
            Err(Rejection::Duplicate)
        );
        b.offer(chat(3), Duration::from_secs(3)).unwrap();
        assert_eq!(
            b.next_action(Duration::from_secs(3))
                .unwrap()
                .unwrap()
                .target,
            target(2)
        );
    }
    #[test]
    fn capacity_includes_inflight_and_queued_messages_expire() {
        let mut b = bridge(true);
        select(&mut b, 2, 1, 0);
        for id in 0..16 {
            b.offer(chat(id), Duration::ZERO).unwrap();
        }
        b.next_action(Duration::ZERO).unwrap();
        assert_eq!(b.offer(chat(16), Duration::ZERO), Err(Rejection::Full));
        assert!(b.next_action(Duration::from_secs(15)).unwrap().is_none());
        b.offer(chat(16), Duration::from_secs(15)).unwrap();
        assert!(b.next_action(Duration::from_secs(15)).unwrap().is_some());
    }
    #[test]
    fn seen_capacity_fails_closed_until_retention_expires() {
        let mut b = bridge(true);
        select(&mut b, 2, 1, 0);
        for id in 0..256 {
            b.offer(chat(id), Duration::ZERO).unwrap();
            let a = b.next_action(Duration::ZERO).unwrap().unwrap();
            result(&mut b, 3 + id as u64, &a, None, 0);
        }
        assert_eq!(b.offer(chat(256), Duration::ZERO), Err(Rejection::Full));
        b.offer(chat(256), Duration::from_secs(60)).unwrap();
    }
    #[test]
    fn shutdown_and_host_incarnation_failure_are_sticky() {
        let mut b = bridge(true);
        select(&mut b, 2, 1, 0);
        b.offer(chat(1), Duration::ZERO).unwrap();
        b.host(
            envelope(
                3,
                HostMessage::Event {
                    event: HostEvent::AppStopping {
                        app_run_id: Uuid::from_u128(3),
                    },
                },
            ),
            Duration::ZERO,
        )
        .unwrap();
        select(&mut b, 4, 2, 0);
        assert!(b.stopped());
        assert_eq!(b.offer(chat(2), Duration::ZERO), Err(Rejection::Stopped));
        assert!(b.next_action(Duration::ZERO).unwrap().is_none());
        let mut b = bridge(true);
        let mut wrong = envelope(
            2,
            HostMessage::Event {
                event: HostEvent::TargetChanged {
                    target: Some(target(1)),
                },
            },
        );
        wrong.process_generation = 8;
        assert!(b.host(wrong, Duration::ZERO).is_err());
        assert!(b.stopped());
    }
    #[test]
    fn stale_host_frames_cannot_retarget_or_resolve_pending_action() {
        let mut b = bridge(true);
        select(&mut b, 3, 1, 0);
        select(&mut b, 2, 2, 0);
        b.offer(chat(1), Duration::ZERO).unwrap();
        let action = b.next_action(Duration::ZERO).unwrap().unwrap();
        assert_eq!(action.target, target(1));
        result(&mut b, 3, &action, None, 0);
        assert!(b.pending.is_some());
        let mut wrong = action.clone();
        wrong.target = target(2);
        result(&mut b, 4, &wrong, None, 0);
        assert!(b.pending.is_some());
        result(&mut b, 5, &action, None, 0);
        assert!(b.pending.is_none());
    }
}
