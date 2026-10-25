//! Per-connection, bounded relay of source-owned lifecycle observations.
//! No avatar selection, subscription registry, provider calls or application effects.
//! Call set_grants (or reconcile_grants immediately after an external grant
//! change) so a revoke/regrant between frames cannot revive queued private text.
use crate::{
    extension_connection::ExtensionConnection,
    extension_lifecycle::{LifecycleEvent, ReplyOutcome as SourceReplyOutcome},
    extension_protocol::{Capability, HostEvent, ReplyOutcome, reply_events},
};
use std::{collections::VecDeque, mem::size_of};
use uuid::Uuid;

pub const MAX_RELAY_EVENTS: usize = 32;
pub const MAX_RELAY_BYTES: usize = 1024 * 1024;
pub const MAX_WIRE_EVENTS_PER_PUMP: usize = 8;

enum Pending {
    Reply {
        reply_id: Uuid,
        character: Option<Uuid>,
        outcome: ReplyOutcome,
        text: Option<Box<str>>,
        offset: usize,
        finished: bool,
    },
    Metadata(HostEvent),
}
impl Pending {
    fn bytes(&self) -> usize {
        size_of::<Self>()
            + match self {
                Self::Reply {
                    text: Some(text), ..
                } => text.len(),
                Self::Metadata(HostEvent::MotionPlaybackStarted {
                    motion_id, source, ..
                })
                | Self::Metadata(HostEvent::MotionPlaybackEnded {
                    motion_id, source, ..
                }) => {
                    motion_id.as_ref().map_or(0, String::capacity)
                        + match source {
                            crate::extension_protocol::MotionPlaybackSource::Host => 0,
                            crate::extension_protocol::MotionPlaybackSource::Extension {
                                integration_id,
                                action_id,
                                ..
                            } => integration_id.capacity() + action_id.capacity(),
                        }
                }
                _ => 0,
            }
    }
    fn next_wire(&self) -> Option<HostEvent> {
        match self {
            Self::Reply {
                reply_id,
                character,
                outcome,
                text,
                offset,
                finished,
            } => {
                if let Some(text) = text.as_ref().filter(|text| *offset < text.len()) {
                    // The protocol helper owns UTF-8 chunk sizing. Shift its
                    // suffix-relative offset back to the original reply offset.
                    let mut event =
                        reply_events(*reply_id, *character, &text[*offset..], *outcome).next()?;
                    if let HostEvent::ReplyText {
                        offset: wire_offset,
                        ..
                    } = &mut event
                    {
                        *wire_offset += *offset as u64;
                    }
                    Some(event)
                } else if *finished {
                    Some(HostEvent::ReplyFinished {
                        reply_id: *reply_id,
                        character: *character,
                        outcome: *outcome,
                    })
                } else {
                    None
                }
            }
            Self::Metadata(event) => Some(event.clone()),
        }
    }
    fn sent(&mut self, event: &HostEvent) -> bool {
        match self {
            Self::Reply {
                text,
                offset,
                finished,
                ..
            } => {
                match event {
                    HostEvent::ReplyText { text: chunk, .. } => {
                        *offset += chunk.len();
                        if text.as_ref().is_some_and(|text| *offset == text.len()) {
                            *text = None;
                        }
                    }
                    HostEvent::ReplyFinished { .. } => *finished = false,
                    _ => unreachable!("reply queue produces only reply events"),
                }
                text.is_none() && !*finished
            }
            Self::Metadata(_) => true,
        }
    }
}

#[derive(Default)]
pub struct EventRelay {
    identity: Option<(Uuid, u64)>,
    queue: VecDeque<Pending>,
    bytes: usize,
    dropped: u64,
    status: String,
    failed: bool,
}
impl EventRelay {
    /// Counts source records lost to failure/session replacement, and pending
    /// records whose text or metadata was removed by a permission revocation.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn queued_events(&self) -> usize {
        self.queue.len()
    }
    /// Occupied inline pending records plus owned text bytes (not allocator overhead).
    pub fn queued_bytes(&self) -> usize {
        self.bytes
    }

    fn recount(&mut self) {
        self.bytes = self.queue.iter().map(Pending::bytes).sum();
    }
    fn discard(&mut self) {
        self.dropped = self.dropped.saturating_add(self.queue.len() as u64);
        self.queue.clear();
        self.bytes = 0;
    }
    fn bind(&mut self, connection: &ExtensionConnection) {
        let identity = (connection.session_id(), connection.process_generation());
        if self.identity != Some(identity) {
            self.discard();
            self.identity = Some(identity);
            self.failed = false;
            self.status = "Ready".into();
        }
    }
    fn fail(
        &mut self,
        connection: &mut ExtensionConnection,
        reason: &str,
        additional: u64,
    ) -> String {
        self.discard();
        self.dropped = self.dropped.saturating_add(additional);
        self.failed = true;
        self.status = reason.into();
        connection.stop(reason);
        self.status.clone()
    }
    /// Use for every grant change, including changes followed by an immediate
    /// regrant. Already-published transport bytes cannot be recalled.
    pub fn set_grants(
        &mut self,
        connection: &mut ExtensionConnection,
        grants: impl IntoIterator<Item = Capability>,
    ) {
        connection.set_grants(grants);
        self.reconcile_grants(connection);
    }
    /// Purges revoked queue parts permanently; new grants never add parts that
    /// were absent at capture time. No wire messages are sent by this method.
    pub fn reconcile_grants(&mut self, connection: &mut ExtensionConnection) {
        self.bind(connection);
        let read = connection.grants().contains(&Capability::ReplyRead);
        let observe = connection.grants().contains(&Capability::LifecycleObserve);
        self.queue.retain_mut(|pending| {
            let (keep, lost) = match pending {
                Pending::Reply { text, finished, .. } => {
                    let mut lost = false;
                    if !read {
                        lost |= text.take().is_some();
                    }
                    if !observe {
                        lost |= *finished;
                        *finished = false;
                    }
                    (text.is_some() || *finished, lost)
                }
                Pending::Metadata(_) => (observe, !observe),
            };
            if lost {
                self.dropped = self.dropped.saturating_add(1);
            }
            keep
        });
        self.recount();
    }
    /// True means retained; false means no part was authorized at capture time.
    /// Capacity/source-gap failures stop the connection and clear all pending work.
    pub fn enqueue(
        &mut self,
        event: &LifecycleEvent,
        connection: &mut ExtensionConnection,
    ) -> Result<bool, String> {
        self.reconcile_grants(connection);
        if self.failed {
            return Err(self.status.clone());
        }
        if !connection.is_active() {
            return Err(self.fail(connection, "Lifecycle relay connection stopped", 1));
        }
        let read = connection.grants().contains(&Capability::ReplyRead);
        let observe = connection.grants().contains(&Capability::LifecycleObserve);
        let text = match event {
            LifecycleEvent::ReplyFinished {
                outcome: SourceReplyOutcome::Completed { text },
                ..
            } if read => Some(text.as_str()),
            _ => None,
        };
        let authorized = match event {
            LifecycleEvent::ReplyFinished { .. } => {
                observe || text.is_some_and(|text| !text.is_empty())
            }
            LifecycleEvent::ExtensionStarted { .. }
            | LifecycleEvent::MotionPlaybackStarted { .. }
            | LifecycleEvent::MotionPlaybackEnded { .. }
            | LifecycleEvent::ExtensionStopping { .. }
            | LifecycleEvent::ExtensionStopped { .. }
            | LifecycleEvent::AppReady { .. }
            | LifecycleEvent::CharacterSelected { .. }
            | LifecycleEvent::SpeechStarted { .. }
            | LifecycleEvent::SpeechStopped { .. }
            | LifecycleEvent::EmotionChanged { .. } => observe,
            LifecycleEvent::Gap {
                dropped_records,
                dropped_receipts,
            } => {
                if (read || observe) && (*dropped_records != 0 || *dropped_receipts != 0) {
                    let lost = dropped_records.saturating_add(*dropped_receipts);
                    return Err(self.fail(connection, "Lifecycle relay source gap", lost));
                }
                return Ok(false);
            }
        };
        if !authorized {
            return Ok(false);
        }
        let charge = size_of::<Pending>()
            .saturating_add(text.map_or(0, str::len))
            .saturating_add(crate::extension_lifecycle::motion_metadata_bytes(event));
        if self.queue.len() >= MAX_RELAY_EVENTS
            || charge > MAX_RELAY_BYTES.saturating_sub(self.bytes)
        {
            return Err(self.fail(connection, "Lifecycle relay queue capacity exceeded", 1));
        }
        let pending = match event {
            LifecycleEvent::MotionPlaybackStarted {
                playback_id,
                source_target,
                motion_id,
                source,
            } => Pending::Metadata(HostEvent::MotionPlaybackStarted {
                playback_id: *playback_id,
                source_target: *source_target,
                motion_id: motion_id.clone(),
                source: source.clone(),
            }),
            LifecycleEvent::MotionPlaybackEnded {
                playback_id,
                source_target,
                motion_id,
                source,
                outcome,
            } => Pending::Metadata(HostEvent::MotionPlaybackEnded {
                playback_id: *playback_id,
                source_target: *source_target,
                motion_id: motion_id.clone(),
                source: source.clone(),
                outcome: *outcome,
            }),
            LifecycleEvent::ExtensionStopping {
                installation_id,
                session_id,
                process_generation,
                reason,
            } => Pending::Metadata(HostEvent::ExtensionStopping {
                installation_id: *installation_id,
                session_id: *session_id,
                process_generation: *process_generation,
                reason: *reason,
            }),
            LifecycleEvent::ExtensionStopped {
                installation_id,
                session_id,
                process_generation,
                exit_code,
                requested_reason,
            } => Pending::Metadata(HostEvent::ExtensionStopped {
                installation_id: *installation_id,
                session_id: *session_id,
                process_generation: *process_generation,
                exit_code: *exit_code,
                requested_reason: *requested_reason,
            }),
            LifecycleEvent::ExtensionStarted {
                installation_id,
                session_id,
                process_generation,
            } => Pending::Metadata(HostEvent::ExtensionStarted {
                installation_id: *installation_id,
                session_id: *session_id,
                process_generation: *process_generation,
            }),
            LifecycleEvent::AppReady { app_run_id } => Pending::Metadata(HostEvent::AppReady {
                app_run_id: *app_run_id,
            }),
            LifecycleEvent::CharacterSelected {
                previous,
                new,
                selection_revision,
            } => Pending::Metadata(HostEvent::CharacterSelected {
                previous: *previous,
                new: *new,
                selection_revision: *selection_revision,
            }),
            LifecycleEvent::ReplyFinished {
                reply_id,
                character,
                outcome,
            } => Pending::Reply {
                reply_id: *reply_id,
                character: *character,
                outcome: match outcome {
                    SourceReplyOutcome::Completed { .. } => ReplyOutcome::Completed,
                    SourceReplyOutcome::Failed { .. } => ReplyOutcome::Failed,
                    SourceReplyOutcome::Cancelled => ReplyOutcome::Cancelled,
                },
                // Failed.error is deliberately never retained or emitted.
                text: text.filter(|text| !text.is_empty()).map(Box::<str>::from),
                offset: 0,
                finished: observe,
            },
            LifecycleEvent::SpeechStarted {
                playback_id,
                character,
                voice_generation,
                reply_id,
            } => Pending::Metadata(HostEvent::SpeechStarted {
                playback_id: *playback_id,
                character: *character,
                voice_generation: *voice_generation,
                reply_id: *reply_id,
            }),
            LifecycleEvent::SpeechStopped {
                playback_id,
                character,
                voice_generation,
                reply_id,
                reason,
            } => Pending::Metadata(HostEvent::SpeechStopped {
                playback_id: *playback_id,
                character: *character,
                voice_generation: *voice_generation,
                reply_id: *reply_id,
                reason: *reason,
            }),
            LifecycleEvent::EmotionChanged {
                character,
                owner,
                previous,
                emotion,
                reply_id,
            } => Pending::Metadata(HostEvent::EmotionChanged {
                character: *character,
                owner: *owner,
                previous: *previous,
                emotion: *emotion,
                reply_id: *reply_id,
            }),
            LifecycleEvent::Gap { .. } => unreachable!("gap handled above"),
        };
        self.queue.push_back(pending);
        self.recount();
        Ok(true)
    }
    /// Send at most eight wire events, checking live grants before each send.
    /// A long reply stays one source record; only its next UTF-8 chunk is allocated.
    pub fn pump(&mut self, connection: &mut ExtensionConnection) -> Result<usize, String> {
        self.reconcile_grants(connection);
        if self.failed {
            return Err(self.status.clone());
        }
        if !connection.is_active() {
            return Err(self.fail(connection, "Lifecycle relay connection stopped", 0));
        }
        let mut sent = 0;
        while sent < MAX_WIRE_EVENTS_PER_PUMP {
            self.reconcile_grants(connection);
            let Some(pending) = self.queue.front() else {
                break;
            };
            let Some(event) = pending.next_wire() else {
                self.queue.pop_front();
                self.recount();
                continue;
            };
            if let Err(error) = connection.publish_lifecycle(event.clone()) {
                let reason = format!("Lifecycle relay send failed: {error}");
                return Err(self.fail(connection, &reason, 0));
            }
            sent += 1;
            if self
                .queue
                .front_mut()
                .expect("front retained until publication")
                .sent(&event)
            {
                self.queue.pop_front();
            }
            self.recount();
        }
        Ok(sent)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{
        extension_process::ProcessSpec,
        extension_protocol::{IntegrationOrigin, MAX_REPLY_CHUNK_BYTES, SpeechStopReason},
    };
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };

    const BOTH: [Capability; 2] = [Capability::ReplyRead, Capability::LifecycleObserve];

    // A real subprocess consumes host envelopes, validates session/sequence
    // framing, and flushes events to a private fixture file. No action executor
    // or loaded avatar is needed to observe source-owned lifecycle events.
    struct Fixture {
        connection: ExtensionConnection,
        root: PathBuf,
    }
    impl Fixture {
        fn new(grants: impl IntoIterator<Item = Capability>) -> Self {
            let root = std::env::temp_dir().join(format!("waifu-relay-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let script = r#"
import json, pathlib, sys
h = json.loads(sys.stdin.readline())
assert h['message']['type'] == 'handshake' and h['sequence'] == 1
pathlib.Path('ready').write_text('ready')
sequence = 1
with open('events.jsonl', 'w', encoding='utf-8') as output:
    for line in sys.stdin:
        message = json.loads(line)
        sequence += 1
        assert message['sequence'] == sequence
        assert message['session_id'] == h['session_id']
        assert message['process_generation'] == h['process_generation']
        assert message['message']['type'] == 'event'
        output.write(json.dumps(message['message']['event'], ensure_ascii=False) + '\n')
        output.flush()
"#;
            let connection = ExtensionConnection::spawn(
                ProcessSpec {
                    program: "/usr/bin/python3".into(),
                    args: vec!["-u".into(), "-c".into(), script.into()],
                    working_directory: root.clone(),
                    data_directory: root.join("data"),
                    lifetime: Some(Duration::from_secs(10)),
                },
                IntegrationOrigin {
                    integration_id: "org.example.relay".into(),
                    display_name: "Relay fixture".into(),
                },
                grants,
                7,
            )
            .unwrap();
            let mut fixture = Self { connection, root };
            fixture.until(|fixture| fixture.root.join("ready").exists());
            fixture
        }
        fn until(&mut self, mut ready: impl FnMut(&Self) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                assert!(self.connection.drain_target(None).is_empty());
                if ready(self) {
                    return;
                }
                assert!(
                    self.connection.is_active() && Instant::now() < deadline,
                    "relay fixture timed out or stopped: {} ({:?})",
                    self.connection.status(),
                    self.connection.logs(),
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        fn events(&self) -> Vec<HostEvent> {
            std::fs::read_to_string(self.root.join("events.jsonl"))
                .unwrap_or_default()
                .lines()
                // A concurrent append may leave the last line incomplete.
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        }
        fn wait_events(&mut self, count: usize) -> Vec<HostEvent> {
            self.until(|fixture| fixture.events().len() >= count);
            self.events()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Test-only join; frame pumping and relay failure never wait.
            self.connection.shutdown(Duration::from_secs(1));
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn reply(text: &str) -> LifecycleEvent {
        LifecycleEvent::ReplyFinished {
            reply_id: Uuid::from_u128(11),
            character: Some(Uuid::from_u128(9)),
            outcome: SourceReplyOutcome::Completed { text: text.into() },
        }
    }
    fn speech() -> LifecycleEvent {
        LifecycleEvent::SpeechStopped {
            playback_id: Uuid::from_u128(17),
            character: None,
            voice_generation: 8,
            reply_id: None,
            reason: SpeechStopReason::Cancelled,
        }
    }

    #[test]
    fn real_child_motion_activation_retains_source_and_revoke_drops_queued_activation() {
        let mut fixture = Fixture::new(BOTH);
        let mut relay = EventRelay::default();
        let playback_id = Uuid::new_v4();
        let source = crate::extension_protocol::MotionPlaybackSource::Extension {
            integration_id: "org.example.sender".into(),
            session_id: Uuid::new_v4(),
            process_generation: 9,
            action_id: "wave-1".into(),
        };
        let event = LifecycleEvent::MotionPlaybackStarted {
            playback_id,
            source_target: None,
            motion_id: Some("org.example.pack/wave".into()),
            source: source.clone(),
        };
        assert!(relay.enqueue(&event, &mut fixture.connection).unwrap());
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 1);
        assert_eq!(
            fixture.wait_events(1),
            vec![HostEvent::MotionPlaybackStarted {
                playback_id,
                source_target: None,
                motion_id: Some("org.example.pack/wave".into()),
                source
            }]
        );
        assert!(relay.enqueue(&event, &mut fixture.connection).unwrap());
        relay.set_grants(&mut fixture.connection, [Capability::ReplyRead]);
        relay.set_grants(&mut fixture.connection, BOTH);
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 0);
        assert_eq!(fixture.events().len(), 1);
    }

    #[test]
    fn real_child_extension_started_keeps_source_session_distinct_from_receiver() {
        let mut fixture = Fixture::new(BOTH);
        let installation_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        assert_ne!(session_id, fixture.connection.session_id());
        let mut relay = EventRelay::default();
        assert!(
            relay
                .enqueue(
                    &LifecycleEvent::ExtensionStarted {
                        installation_id,
                        session_id,
                        process_generation: 2,
                    },
                    &mut fixture.connection
                )
                .unwrap()
        );
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 1);
        assert_eq!(
            fixture.wait_events(1),
            vec![HostEvent::ExtensionStarted {
                installation_id,
                session_id,
                process_generation: 2,
            }]
        );
    }

    #[test]
    fn real_child_app_ready_is_metadata_and_revocation_discards_queued_startup() {
        let mut fixture = Fixture::new(BOTH);
        let mut relay = EventRelay::default();
        let app_run_id = Uuid::new_v4();
        let event = LifecycleEvent::AppReady { app_run_id };
        assert!(relay.enqueue(&event, &mut fixture.connection).unwrap());
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 1);
        assert_eq!(
            fixture.wait_events(1),
            vec![HostEvent::AppReady { app_run_id }]
        );
        assert!(relay.enqueue(&event, &mut fixture.connection).unwrap());
        relay.set_grants(&mut fixture.connection, [Capability::ReplyRead]);
        assert_eq!(relay.queued_events(), 0);
        relay.set_grants(&mut fixture.connection, BOTH);
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 0);
        assert_eq!(fixture.events().len(), 1);
        let mut text_only = Fixture::new([Capability::ReplyRead]);
        assert!(
            !EventRelay::default()
                .enqueue(&event, &mut text_only.connection)
                .unwrap()
        );
    }

    #[test]
    fn real_child_emotion_metadata_requires_observation_and_regrant_cannot_restore_queue() {
        use crate::{extension_protocol::EmotionOwner, reply_affect::AffectEmotion};
        let mut fixture = Fixture::new([Capability::ReplyRead]);
        let mut relay = EventRelay::default();
        let character = Uuid::from_u128(9);
        let discarded = LifecycleEvent::EmotionChanged {
            character,
            owner: EmotionOwner::Studio,
            previous: AffectEmotion::Neutral,
            emotion: AffectEmotion::Happy,
            reply_id: Some(Uuid::from_u128(11)),
        };
        assert!(!relay.enqueue(&discarded, &mut fixture.connection).unwrap());
        assert_eq!(relay.queued_events(), 0);
        relay.set_grants(&mut fixture.connection, [Capability::LifecycleObserve]);
        assert!(relay.enqueue(&discarded, &mut fixture.connection).unwrap());
        assert_eq!(relay.queued_bytes(), size_of::<Pending>());
        relay.set_grants(&mut fixture.connection, [Capability::ReplyRead]);
        assert_eq!(relay.queued_events(), 0);
        assert_eq!(relay.queued_bytes(), 0);
        assert_eq!(relay.dropped(), 1);
        relay.set_grants(&mut fixture.connection, [Capability::LifecycleObserve]);
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 0);
        assert!(fixture.events().is_empty());

        for (owner, previous, emotion, reply_id) in [
            (
                EmotionOwner::Studio,
                AffectEmotion::Happy,
                AffectEmotion::Curious,
                None,
            ),
            (
                EmotionOwner::SceneChat,
                AffectEmotion::Neutral,
                AffectEmotion::Caring,
                Some(Uuid::from_u128(12)),
            ),
        ] {
            assert!(
                relay
                    .enqueue(
                        &LifecycleEvent::EmotionChanged {
                            character,
                            owner,
                            previous,
                            emotion,
                            reply_id,
                        },
                        &mut fixture.connection
                    )
                    .unwrap()
            );
        }
        assert_eq!(relay.queued_events(), 2);
        assert_eq!(relay.queued_bytes(), 2 * size_of::<Pending>());
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 2);
        // Fixture always drains with target None; ordered delivery also proves
        // the earlier unauthorized/revoked Happy observation was not revived.
        assert_eq!(
            fixture.wait_events(2),
            vec![
                HostEvent::EmotionChanged {
                    character,
                    owner: EmotionOwner::Studio,
                    previous: AffectEmotion::Happy,
                    emotion: AffectEmotion::Curious,
                    reply_id: None,
                },
                HostEvent::EmotionChanged {
                    character,
                    owner: EmotionOwner::SceneChat,
                    previous: AffectEmotion::Neutral,
                    emotion: AffectEmotion::Caring,
                    reply_id: Some(Uuid::from_u128(12)),
                },
            ]
        );
        assert_eq!(relay.queued_events(), 0);
        assert_eq!(relay.queued_bytes(), 0);
    }

    #[test]
    fn real_child_reconstructs_ten_kib_utf8_and_source_owner_without_avatar() {
        let mut fixture = Fixture::new(BOTH);
        let mut relay = EventRelay::default();
        let text = "雪🦊\n\"a".repeat(1100);
        assert!(text.len() >= 10 * 1024);
        assert!(
            relay
                .enqueue(&reply(&text), &mut fixture.connection)
                .unwrap()
        );
        assert_eq!(relay.queued_events(), 1);
        let count = relay.pump(&mut fixture.connection).unwrap();
        let events = fixture.wait_events(count);
        let mut reconstructed = String::new();
        for event in &events[..events.len() - 1] {
            let HostEvent::ReplyText {
                reply_id,
                character,
                offset,
                text,
            } = event
            else {
                panic!("completion must follow all chunks");
            };
            assert_eq!(*reply_id, Uuid::from_u128(11));
            assert_eq!(*character, Some(Uuid::from_u128(9)));
            assert_eq!(*offset, reconstructed.len() as u64);
            assert!(text.len() <= MAX_REPLY_CHUNK_BYTES);
            reconstructed.push_str(text);
        }
        assert_eq!(reconstructed, text);
        assert_eq!(
            events.last(),
            Some(&HostEvent::ReplyFinished {
                reply_id: Uuid::from_u128(11),
                character: Some(Uuid::from_u128(9)),
                outcome: ReplyOutcome::Completed,
            })
        );
        assert_eq!(relay.queued_events(), 0);
        assert_eq!(relay.queued_bytes(), 0);
        assert_eq!(relay.dropped(), 0);
    }

    #[test]
    fn real_child_mid_reply_revocation_permanently_discards_unsent_text() {
        let mut fixture = Fixture::new(BOTH);
        let mut relay = EventRelay::default();
        let text = "🦊雪a".repeat(6000);
        relay
            .enqueue(&reply(&text), &mut fixture.connection)
            .unwrap();
        assert_eq!(
            relay.pump(&mut fixture.connection).unwrap(),
            MAX_WIRE_EVENTS_PER_PUMP
        );
        fixture.wait_events(MAX_WIRE_EVENTS_PER_PUMP);
        assert_eq!(relay.queued_events(), 1);

        relay.set_grants(&mut fixture.connection, [Capability::LifecycleObserve]);
        assert!(relay.queued_bytes() <= size_of::<Pending>());
        relay.set_grants(&mut fixture.connection, BOTH);
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 1);
        let events = fixture.wait_events(MAX_WIRE_EVENTS_PER_PUMP + 1);
        let mut prefix = String::new();
        for event in &events[..MAX_WIRE_EVENTS_PER_PUMP] {
            let HostEvent::ReplyText { offset, text, .. } = event else {
                panic!("expected text");
            };
            assert_eq!(*offset, prefix.len() as u64);
            prefix.push_str(text);
        }
        assert!(prefix.len() < text.len() && text.starts_with(&prefix));
        assert!(matches!(
            events.last(),
            Some(HostEvent::ReplyFinished { .. })
        ));
        assert_eq!(relay.queued_events(), 0);
        assert_eq!(relay.dropped(), 1);
    }

    #[test]
    fn real_child_capture_permissions_never_backfill_text_or_metadata() {
        let mut fixture = Fixture::new([Capability::LifecycleObserve]);
        let mut relay = EventRelay::default();
        relay
            .enqueue(
                &reply("capture-time private reply"),
                &mut fixture.connection,
            )
            .unwrap();
        assert_eq!(relay.queued_bytes(), size_of::<Pending>());
        relay.set_grants(&mut fixture.connection, BOTH);
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 1);

        relay.set_grants(&mut fixture.connection, [Capability::ReplyRead]);
        assert!(!relay.enqueue(&speech(), &mut fixture.connection).unwrap());
        relay
            .enqueue(&reply("only text was authorized"), &mut fixture.connection)
            .unwrap();
        relay.set_grants(&mut fixture.connection, BOTH);
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 1);
        // Ordered sentinel proves there are no delayed unauthorized messages.
        relay.enqueue(&speech(), &mut fixture.connection).unwrap();
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 1);
        let events = fixture.wait_events(3);
        assert!(matches!(events[0], HostEvent::ReplyFinished { .. }));
        assert!(
            matches!(&events[1], HostEvent::ReplyText { text, .. } if text == "only text was authorized")
        );
        assert!(matches!(events[2], HostEvent::SpeechStopped { .. }));
    }

    #[test]
    fn real_child_revoking_observation_preserves_text_and_redacts_failure_details() {
        let mut fixture = Fixture::new(BOTH);
        let mut relay = EventRelay::default();
        relay
            .enqueue(&reply("allowed text"), &mut fixture.connection)
            .unwrap();
        relay.enqueue(&speech(), &mut fixture.connection).unwrap();
        relay.set_grants(&mut fixture.connection, [Capability::ReplyRead]);
        relay.set_grants(&mut fixture.connection, BOTH);
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 1);

        relay.set_grants(&mut fixture.connection, [Capability::LifecycleObserve]);
        relay
            .enqueue(
                &LifecycleEvent::ReplyFinished {
                    reply_id: Uuid::from_u128(12),
                    character: None,
                    outcome: SourceReplyOutcome::Failed {
                        error: "private provider failure".into(),
                    },
                },
                &mut fixture.connection,
            )
            .unwrap();
        assert_eq!(relay.queued_bytes(), size_of::<Pending>());
        assert_eq!(relay.pump(&mut fixture.connection).unwrap(), 1);
        let events = fixture.wait_events(2);
        assert!(matches!(&events[0], HostEvent::ReplyText { text, .. } if text == "allowed text"));
        assert_eq!(
            events[1],
            HostEvent::ReplyFinished {
                reply_id: Uuid::from_u128(12),
                character: None,
                outcome: ReplyOutcome::Failed,
            }
        );
        assert!(
            !std::fs::read_to_string(fixture.root.join("events.jsonl"))
                .unwrap()
                .contains("private provider failure")
        );
    }

    #[test]
    fn real_connection_queue_overflow_stops_and_accounts_for_every_record() {
        let mut fixture = Fixture::new(BOTH);
        let mut relay = EventRelay::default();
        for _ in 0..MAX_RELAY_EVENTS {
            relay.enqueue(&speech(), &mut fixture.connection).unwrap();
        }
        assert!(relay.enqueue(&speech(), &mut fixture.connection).is_err());
        assert!(!fixture.connection.is_active());
        assert_eq!(relay.dropped(), (MAX_RELAY_EVENTS + 1) as u64);
        assert_eq!(relay.queued_events(), 0);
        assert_eq!(relay.queued_bytes(), 0);
        assert!(relay.status().contains("capacity"));
        assert!(fixture.connection.status().contains("capacity"));
        assert!(relay.pump(&mut fixture.connection).is_err());
    }

    #[test]
    fn real_connection_combined_text_budget_fails_before_event_limit() {
        let mut fixture = Fixture::new(BOTH);
        let mut relay = EventRelay::default();
        let event = reply(&"x".repeat(MAX_RELAY_BYTES / 2));
        relay.enqueue(&event, &mut fixture.connection).unwrap();
        assert!(relay.queued_bytes() <= MAX_RELAY_BYTES);
        assert!(relay.enqueue(&event, &mut fixture.connection).is_err());
        assert_eq!(relay.dropped(), 2);
        assert_eq!(relay.queued_bytes(), 0);
        assert!(!fixture.connection.is_active());
        assert!(fixture.connection.status().contains("capacity"));
    }

    #[test]
    fn real_connection_oversize_and_source_gap_fail_closed() {
        let mut fixture = Fixture::new(BOTH);
        let mut relay = EventRelay::default();
        assert!(
            relay
                .enqueue(
                    &reply(&"x".repeat(MAX_RELAY_BYTES)),
                    &mut fixture.connection
                )
                .is_err()
        );
        assert_eq!(relay.dropped(), 1);
        assert_eq!(relay.queued_bytes(), 0);
        assert!(!fixture.connection.is_active());

        let mut replacement = Fixture::new(BOTH);
        relay
            .enqueue(&speech(), &mut replacement.connection)
            .unwrap();
        assert!(
            relay
                .enqueue(
                    &LifecycleEvent::Gap {
                        dropped_records: 3,
                        dropped_receipts: 2,
                    },
                    &mut replacement.connection
                )
                .is_err()
        );
        assert_eq!(relay.dropped(), 7);
        assert!(relay.status().contains("source gap"));
        assert!(!replacement.connection.is_active());
    }

    #[test]
    fn real_child_restart_never_replays_old_session_backlog() {
        let mut original = Fixture::new(BOTH);
        let mut relay = EventRelay::default();
        relay
            .enqueue(&reply("old session secret"), &mut original.connection)
            .unwrap();
        original.connection.stop("restart");
        let mut replacement = Fixture::new(BOTH);
        assert_eq!(relay.pump(&mut replacement.connection).unwrap(), 0);
        assert_eq!(relay.dropped(), 1);
        relay
            .enqueue(&speech(), &mut replacement.connection)
            .unwrap();
        assert_eq!(relay.pump(&mut replacement.connection).unwrap(), 1);
        assert!(matches!(
            replacement.wait_events(1)[0],
            HostEvent::SpeechStopped { .. }
        ));
    }
}
