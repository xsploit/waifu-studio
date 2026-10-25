//! Bounded source lifecycle observations, not transport or avatar-target claims.
//! Integration checkpoint API. Character, reply and voice identities describe
//! their original source; none implies the current selected/loaded avatar.
//!
//! The journal retains at most 255 ordinary records, reserving one record and
//! its inline bytes for a coalesced Gap appended by drain. The 2 MiB budget
//! accounts for occupied inline records and owned text capacity, not allocator
//! overhead. Oversized records are rejected; retention pressure evicts oldest
//! records. Counters survive drains. Gap counts cover losses since the previous
//! drain; lost receipts are counted separately because their missing event count
//! cannot be inferred. Sequence exhaustion rejects new records without wrapping.
//!
//! Speech receipts retain consumption history independently of audio entities.
//! Started means decoder consumption, including silence; stopped means an
//! observed terminal receipt or explicit host cancellation, not device drain.
//! A stopped receipt need never have started. No audio callback, I/O or transport
//! work occurs here. The host must retain/drain these resources and handle gaps.

pub use crate::extension_protocol::{EmotionOwner, ExtensionStopReason, SpeechStopReason};
use crate::extension_protocol::{MotionOutcome, MotionPlaybackSource, Target};
use crate::metered_audio::SpeechPlayback;
use crate::reply_affect::AffectEmotion;
use bevy::prelude::Resource;
use std::{collections::VecDeque, mem::size_of};
use uuid::Uuid;

pub const MAX_JOURNAL_RECORDS: usize = 256;
pub const MAX_JOURNAL_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SPEECH_RECEIPTS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplyOutcome {
    Completed { text: String },
    Failed { error: String },
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleEvent {
    MotionPlaybackStarted {
        playback_id: Uuid,
        source_target: Option<Target>,
        motion_id: Option<String>,
        source: MotionPlaybackSource,
    },
    MotionPlaybackEnded {
        playback_id: Uuid,
        source_target: Option<Target>,
        motion_id: Option<String>,
        source: MotionPlaybackSource,
        outcome: MotionOutcome,
    },

    ExtensionStopping {
        installation_id: Uuid,
        session_id: Uuid,
        process_generation: u64,
        reason: ExtensionStopReason,
    },
    ExtensionStopped {
        installation_id: Uuid,
        session_id: Uuid,
        process_generation: u64,
        exit_code: Option<i32>,
        requested_reason: Option<ExtensionStopReason>,
    },

    /// Original persistent incarnation; not the receiving observer's session.
    ExtensionStarted {
        installation_id: Uuid,
        session_id: Uuid,
        process_generation: u64,
    },
    /// Local host initialization completed, not provider/account/avatar readiness.
    AppReady { app_run_id: Uuid },
    /// Selection source metadata, never loaded-avatar action authority.
    CharacterSelected {
        previous: Option<Uuid>,
        new: Option<Uuid>,
        selection_revision: u64,
    },
    ReplyFinished {
        reply_id: Uuid,
        character: Option<Uuid>,
        outcome: ReplyOutcome,
    },
    SpeechStarted {
        playback_id: Uuid,
        character: Option<Uuid>,
        voice_generation: u64,
        reply_id: Option<Uuid>,
    },
    SpeechStopped {
        playback_id: Uuid,
        character: Option<Uuid>,
        voice_generation: u64,
        reply_id: Option<Uuid>,
        reason: SpeechStopReason,
    },
    /// Original affect ownership, not a current avatar/action target.
    EmotionChanged {
        character: Uuid,
        owner: EmotionOwner,
        previous: AffectEmotion,
        emotion: AffectEmotion,
        reply_id: Option<Uuid>,
    },
    /// Loss summary appended on drain; it is not an inferred source event.
    Gap {
        dropped_records: u64,
        dropped_receipts: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequencedEvent {
    pub sequence: u64,
    pub event: LifecycleEvent,
}

/// Main-host latch, retained for the application run, including empty runtimes.
/// Call only after ExtensionState::poll and installed integration reconciliation.
pub struct AppReadySource {
    app_run_id: Uuid,
    emitted: bool,
}
impl Default for AppReadySource {
    fn default() -> Self {
        Self {
            app_run_id: Uuid::new_v4(),
            emitted: false,
        }
    }
}
impl AppReadySource {
    pub fn after_reconcile(
        &mut self,
        initial_load_completed: bool,
        journal: &mut LifecycleJournal,
    ) {
        if !initial_load_completed || self.emitted {
            return;
        }
        // A source/retention gap is explicit loss, never a reason to replay
        // startup to an integration installed, enabled or restarted later.
        self.emitted = true;
        journal.record(LifecycleEvent::AppReady {
            app_run_id: self.app_run_id,
        });
    }
}

#[derive(Resource)]
pub struct LifecycleJournal {
    records: VecDeque<SequencedEvent>,
    bytes: usize,
    last_sequence: u64,
    selection_revision: u64,
    dropped: u64,
    pending_record_loss: u64,
    pending_receipt_loss: u64,
}

impl Default for LifecycleJournal {
    fn default() -> Self {
        Self {
            records: VecDeque::with_capacity(MAX_JOURNAL_RECORDS - 1),
            bytes: 0,
            last_sequence: 0,
            selection_revision: 0,
            dropped: 0,
            pending_record_loss: 0,
            pending_receipt_loss: 0,
        }
    }
}

impl LifecycleJournal {
    /// Upstream bounded-source loss joins the same sequenced coalesced gap as
    /// journal retention loss. The source never allocates its own event sequence.
    pub(crate) fn note_source_loss(&mut self, count: u64) {
        self.dropped = self.dropped.saturating_add(count);
        self.pending_record_loss = self.pending_record_loss.saturating_add(count);
    }

    /// Call at a committed host selection mutation, after old-source cancellation.
    /// Revisions survive drains; exhaustion reports a gap rather than wrapping.
    pub fn record_selection(&mut self, previous: Option<Uuid>, new: Option<Uuid>) {
        if previous == new {
            return;
        }
        let Some(selection_revision) = self.selection_revision.checked_add(1) else {
            self.note_record_loss();
            return;
        };
        self.selection_revision = selection_revision;
        self.record(LifecycleEvent::CharacterSelected {
            previous,
            new,
            selection_revision,
        });
    }

    /// Returns the assigned sequence if retained now, or None if oversized or
    /// sequence space is exhausted. A later retention overflow may evict it.
    pub fn record(&mut self, mut event: LifecycleEvent) -> Option<u64> {
        let budget = MAX_JOURNAL_BYTES - size_of::<SequencedEvent>();
        let text_len = event_text(&event).map_or(0, String::len);
        if text_len > budget.saturating_sub(size_of::<SequencedEvent>()) {
            self.note_record_loss();
            return None;
        }
        // Do not retain a huge caller allocation with only a tiny used prefix.
        if let Some(text) = event_text_mut(&mut event) {
            *text = std::mem::take(text).into_boxed_str().into_string();
        }
        let charge = event_bytes(&event);
        let Some(sequence) = self.last_sequence.checked_add(1) else {
            self.note_record_loss();
            return None;
        };
        while self.records.len() >= MAX_JOURNAL_RECORDS - 1
            || self.bytes.saturating_add(charge) > budget
        {
            if let Some(old) = self.records.pop_front() {
                self.bytes -= event_bytes(&old.event);
                self.note_record_loss();
            } else {
                self.note_record_loss();
                return None;
            }
        }
        self.last_sequence = sequence;
        self.bytes += charge;
        self.records.push_back(SequencedEvent { sequence, event });
        Some(sequence)
    }

    /// Returns retained events in sequence order, followed by one coalesced Gap
    /// if losses occurred. The returned batch respects the record/byte limits.
    /// At u64 sequence exhaustion, loss counters remain visible but a new gap
    /// cannot receive a sequence; the host must replace the exhausted journal.
    pub fn drain(&mut self) -> Vec<SequencedEvent> {
        let mut records: Vec<_> = self.records.drain(..).collect();
        self.bytes = 0;
        if (self.pending_record_loss != 0 || self.pending_receipt_loss != 0)
            && let Some(sequence) = self.last_sequence.checked_add(1)
        {
            self.last_sequence = sequence;
            records.push(SequencedEvent {
                sequence,
                event: LifecycleEvent::Gap {
                    dropped_records: std::mem::take(&mut self.pending_record_loss),
                    dropped_receipts: std::mem::take(&mut self.pending_receipt_loss),
                },
            });
        }
        records
    }

    /// Lifetime count of rejected/evicted journal records, not lost receipts.
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    fn note_record_loss(&mut self) {
        self.dropped = self.dropped.saturating_add(1);
        self.pending_record_loss = self.pending_record_loss.saturating_add(1);
    }

    pub(crate) fn note_receipt_loss(&mut self, count: u64) {
        self.pending_receipt_loss = self.pending_receipt_loss.saturating_add(count);
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn app_ready_waits_for_initial_load_emits_once_and_has_per_run_identity() {
        let mut journal = LifecycleJournal::default();
        let mut startup = AppReadySource::default();
        startup.after_reconcile(false, &mut journal);
        assert!(journal.drain().is_empty());
        // The host calls this after installed reconciliation, before its drain.
        startup.after_reconcile(true, &mut journal);
        let events = journal.drain();
        assert_eq!(events.len(), 1);
        let LifecycleEvent::AppReady { app_run_id } = events[0].event else {
            panic!("expected local startup metadata");
        };
        assert!(!app_run_id.is_nil());
        for busy in [false, true, false] {
            startup.after_reconcile(busy, &mut journal);
            assert!(
                journal.drain().is_empty(),
                "later install/refresh replayed startup"
            );
        }
        let mut another_run = AppReadySource::default();
        another_run.after_reconcile(true, &mut journal);
        assert!(matches!(journal.drain()[0].event,
            LifecycleEvent::AppReady { app_run_id: next } if next != app_run_id));
    }

    #[test]
    fn selection_revisions_survive_drains_noops_and_report_exhaustion() {
        let mut journal = LifecycleJournal::default();
        let a = Some(Uuid::from_u128(1));
        let b = Some(Uuid::from_u128(2));
        journal.record_selection(None, a);
        journal.record_selection(a, a);
        journal.record_selection(a, b);
        journal.record_selection(b, a);
        let events = journal.drain();
        assert_eq!(events.len(), 3);
        for (event, (previous, new, revision)) in
            events.iter().zip([(None, a, 1), (a, b, 2), (b, a, 3)])
        {
            assert_eq!(
                event.event,
                LifecycleEvent::CharacterSelected {
                    previous,
                    new,
                    selection_revision: revision,
                }
            );
        }
        journal.record_selection(a, None);
        assert!(matches!(
            journal.drain()[0].event,
            LifecycleEvent::CharacterSelected {
                new: None,
                selection_revision: 4,
                ..
            }
        ));
        journal.selection_revision = u64::MAX;
        journal.record_selection(None, a);
        assert!(matches!(
            journal.drain()[0].event,
            LifecycleEvent::Gap {
                dropped_records: 1,
                ..
            }
        ));
    }
}

fn event_text(event: &LifecycleEvent) -> Option<&String> {
    match event {
        LifecycleEvent::ReplyFinished {
            outcome: ReplyOutcome::Completed { text },
            ..
        } => Some(text),
        LifecycleEvent::ReplyFinished {
            outcome: ReplyOutcome::Failed { error },
            ..
        } => Some(error),
        _ => None,
    }
}

fn event_text_mut(event: &mut LifecycleEvent) -> Option<&mut String> {
    match event {
        LifecycleEvent::ReplyFinished {
            outcome: ReplyOutcome::Completed { text },
            ..
        } => Some(text),
        LifecycleEvent::ReplyFinished {
            outcome: ReplyOutcome::Failed { error },
            ..
        } => Some(error),
        _ => None,
    }
}

fn event_bytes(event: &LifecycleEvent) -> usize {
    size_of::<SequencedEvent>()
        + event_text(event).map_or(0, String::capacity)
        + motion_metadata_bytes(event)
}

struct TrackedSpeech {
    playback_id: Uuid,
    character: Option<Uuid>,
    voice_generation: u64,
    reply_id: Option<Uuid>,
    receipt: SpeechPlayback,
    started_emitted: bool,
    cancelled: bool,
}

#[derive(Resource)]
pub struct SpeechReceipts {
    receipts: VecDeque<TrackedSpeech>,
    dropped: u64,
    pending_dropped: u64,
}

impl Default for SpeechReceipts {
    fn default() -> Self {
        Self {
            receipts: VecDeque::with_capacity(MAX_SPEECH_RECEIPTS),
            dropped: 0,
            pending_dropped: 0,
        }
    }
}

impl SpeechReceipts {
    /// Register exactly once per source. Retains its receipt, not its audio
    /// asset/entity. Overflow discards oldest tracking and reports a gap on poll.
    pub fn register(
        &mut self,
        character: Option<Uuid>,
        voice_generation: u64,
        reply_id: Option<Uuid>,
        receipt: SpeechPlayback,
    ) -> Uuid {
        if self.receipts.len() == MAX_SPEECH_RECEIPTS {
            self.receipts.pop_front();
            self.dropped = self.dropped.saturating_add(1);
            self.pending_dropped = self.pending_dropped.saturating_add(1);
        }
        let playback_id = Uuid::new_v4();
        self.receipts.push_back(TrackedSpeech {
            playback_id,
            character,
            voice_generation,
            reply_id,
            receipt,
            started_emitted: false,
            cancelled: false,
        });
        playback_id
    }

    /// Mark the old incarnation before its host fields are cleared. This does
    /// not stop the audio device; the host still owns cancellation. A later poll
    /// preserves any observed consumption and reports Cancelled once per receipt.
    pub fn cancel_generation(&mut self, generation: u64) {
        for tracked in &mut self.receipts {
            if tracked.voice_generation == generation && !tracked.receipt.is_terminal() {
                tracked.cancelled = true;
            }
        }
    }

    pub fn poll(&mut self, journal: &mut LifecycleJournal) {
        journal.note_receipt_loss(std::mem::take(&mut self.pending_dropped));
        self.receipts.retain_mut(|tracked| {
            // Read terminal first: EOF publishes the earlier started latch.
            // Both can become true between frame polls without losing ordering.
            // Cancellation intent is not audio invalidation. Retain the receipt
            // until EOF/drop or meter invalidation so consumption occurring
            // before deferred device cancellation cannot disappear between polls.
            let terminal = tracked.receipt.is_terminal();
            let started = tracked.receipt.has_started();
            if started && !tracked.started_emitted {
                let _ = journal.record(LifecycleEvent::SpeechStarted {
                    playback_id: tracked.playback_id,
                    character: tracked.character,
                    voice_generation: tracked.voice_generation,
                    reply_id: tracked.reply_id,
                });
                // Journal pressure is represented by a gap, not later replay.
                tracked.started_emitted = true;
            }
            if terminal {
                let _ = journal.record(LifecycleEvent::SpeechStopped {
                    playback_id: tracked.playback_id,
                    character: tracked.character,
                    voice_generation: tracked.voice_generation,
                    reply_id: tracked.reply_id,
                    reason: if tracked.cancelled {
                        SpeechStopReason::Cancelled
                    } else {
                        SpeechStopReason::ObservedTerminal
                    },
                });
            }
            !terminal
        });
    }

    /// Lifetime count of discarded receipt registrations, not inferred events.
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metered_audio::SpeechMeter;
    use bevy::{audio::Decodable, prelude::AudioSource};

    fn audio(samples: u32) -> AudioSource {
        // Tiny PCM16 mono WAV: exercise the real metered decoder without output.
        let data_size = samples * 2;
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((36 + data_size).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(8_000u32.to_le_bytes());
        bytes.extend(16_000u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(data_size.to_le_bytes());
        for _ in 0..samples {
            bytes.extend(0i16.to_le_bytes());
        }
        AudioSource {
            bytes: bytes.into(),
        }
    }

    fn reply(outcome: ReplyOutcome) -> LifecycleEvent {
        LifecycleEvent::ReplyFinished {
            reply_id: Uuid::from_u128(1),
            character: Some(Uuid::from_u128(2)),
            outcome,
        }
    }

    #[test]
    fn reply_outcomes_keep_source_identity_and_sequences_across_drains() {
        let mut journal = LifecycleJournal::default();
        for (i, outcome) in [
            ReplyOutcome::Completed {
                text: "reply".into(),
            },
            ReplyOutcome::Failed {
                error: "failure".into(),
            },
            ReplyOutcome::Cancelled,
        ]
        .into_iter()
        .enumerate()
        {
            let event = reply(outcome);
            let sequence = i as u64 + 1;
            assert_eq!(journal.record(event.clone()), Some(sequence));
            assert_eq!(journal.drain(), vec![SequencedEvent { sequence, event }]);
        }
        assert!(journal.drain().is_empty());
        assert_eq!(journal.dropped(), 0);
    }

    #[test]
    fn emotion_observation_retains_original_owner_and_optional_reply() {
        let mut journal = LifecycleJournal::default();
        for (owner, reply_id) in [
            (EmotionOwner::Studio, None),
            (EmotionOwner::SceneChat, Some(Uuid::from_u128(4))),
        ] {
            let event = LifecycleEvent::EmotionChanged {
                character: Uuid::from_u128(3),
                owner,
                previous: AffectEmotion::Neutral,
                emotion: AffectEmotion::Curious,
                reply_id,
            };
            assert_eq!(event_bytes(&event), size_of::<SequencedEvent>());
            let sequence = journal.record(event.clone()).unwrap();
            assert_eq!(journal.drain(), vec![SequencedEvent { sequence, event }]);
        }
        assert_eq!(journal.dropped(), 0);
    }

    #[test]
    fn record_pressure_keeps_newest_with_one_explicit_gap() {
        let mut journal = LifecycleJournal::default();
        let total = MAX_JOURNAL_RECORDS + 5;
        for _ in 0..total {
            assert!(journal.record(reply(ReplyOutcome::Cancelled)).is_some());
        }
        let lost = (total - (MAX_JOURNAL_RECORDS - 1)) as u64;
        assert_eq!(journal.dropped(), lost);
        let records = journal.drain();
        assert_eq!(records.len(), MAX_JOURNAL_RECORDS);
        assert_eq!(records[0].sequence, lost + 1);
        assert!(
            records
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
        );
        assert_eq!(
            records.last().unwrap(),
            &SequencedEvent {
                sequence: total as u64 + 1,
                event: LifecycleEvent::Gap {
                    dropped_records: lost,
                    dropped_receipts: 0
                },
            }
        );
        assert!(journal.drain().is_empty());
        assert_eq!(journal.dropped(), lost);
    }

    #[test]
    fn byte_pressure_evicts_oldest_and_oversized_reply_preserves_retained_reply() {
        let mut journal = LifecycleJournal::default();
        let large = reply(ReplyOutcome::Completed {
            text: "x".repeat(MAX_JOURNAL_BYTES / 2),
        });
        assert_eq!(journal.record(large.clone()), Some(1));
        assert_eq!(journal.record(large.clone()), Some(2));
        assert_eq!(journal.records.len(), 1);
        assert_eq!(journal.dropped(), 1);
        assert_eq!(
            journal.record(reply(ReplyOutcome::Failed {
                error: "x".repeat(MAX_JOURNAL_BYTES),
            })),
            None
        );
        assert_eq!(journal.records.len(), 1);
        assert_eq!(journal.dropped(), 2);
        let records = journal.drain();
        assert_eq!(records[0].event, large);
        assert_eq!(
            records[1].event,
            LifecycleEvent::Gap {
                dropped_records: 2,
                dropped_receipts: 0,
            }
        );
        assert!(records.iter().map(|r| event_bytes(&r.event)).sum::<usize>() <= MAX_JOURNAL_BYTES);
    }

    #[test]
    fn unused_text_capacity_is_not_retained() {
        let mut text = String::with_capacity(MAX_JOURNAL_BYTES * 2);
        text.push_str("short");
        let mut journal = LifecycleJournal::default();
        assert_eq!(
            journal.record(reply(ReplyOutcome::Completed { text })),
            Some(1)
        );
        assert!(journal.bytes < MAX_JOURNAL_BYTES / 2);
        assert_eq!(journal.dropped(), 0);
    }

    #[test]
    fn sequence_exhaustion_does_not_wrap() {
        let mut journal = LifecycleJournal {
            last_sequence: u64::MAX - 1,
            ..Default::default()
        };
        assert_eq!(
            journal.record(reply(ReplyOutcome::Cancelled)),
            Some(u64::MAX)
        );
        assert_eq!(journal.record(reply(ReplyOutcome::Cancelled)), None);
        let records = journal.drain();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].sequence, u64::MAX);
        assert_eq!(journal.dropped(), 1);
        assert!(journal.drain().is_empty());
    }

    #[test]
    fn short_eof_before_poll_emits_start_then_stop_after_source_destruction_once() {
        let meter = SpeechMeter::default();
        let source = meter.buffered(audio(1));
        let mut receipts = SpeechReceipts::default();
        let character = Some(Uuid::from_u128(3));
        let reply_id = Some(Uuid::from_u128(4));
        let playback_id = receipts.register(character, 7, reply_id, source.playback());
        let mut decoder = source.decoder();
        assert_eq!(decoder.next(), Some(0.0));
        assert_eq!(decoder.next(), None);
        drop(decoder);
        drop(source);
        let mut journal = LifecycleJournal::default();
        receipts.poll(&mut journal);
        assert_eq!(
            journal.drain(),
            vec![
                SequencedEvent {
                    sequence: 1,
                    event: LifecycleEvent::SpeechStarted {
                        playback_id,
                        character,
                        voice_generation: 7,
                        reply_id,
                    }
                },
                SequencedEvent {
                    sequence: 2,
                    event: LifecycleEvent::SpeechStopped {
                        playback_id,
                        character,
                        voice_generation: 7,
                        reply_id,
                        reason: SpeechStopReason::ObservedTerminal,
                    }
                },
            ]
        );
        receipts.poll(&mut journal);
        assert!(journal.drain().is_empty());
        assert!(receipts.receipts.is_empty());
    }

    #[test]
    fn dropped_unconsumed_decoder_stops_without_false_start() {
        let meter = SpeechMeter::default();
        let source = meter.buffered(audio(1));
        let mut receipts = SpeechReceipts::default();
        let playback_id = receipts.register(None, 1, None, source.playback());
        drop(source.decoder());
        drop(source);
        let mut journal = LifecycleJournal::default();
        receipts.poll(&mut journal);
        assert_eq!(
            journal.drain()[0].event,
            LifecycleEvent::SpeechStopped {
                playback_id,
                character: None,
                voice_generation: 1,
                reply_id: None,
                reason: SpeechStopReason::ObservedTerminal,
            }
        );
        assert_eq!(journal.last_sequence, 1);
    }

    #[test]
    fn cancellation_is_generation_scoped_and_unconsumed_sources_never_start() {
        let meter = SpeechMeter::default();
        let other_meter = SpeechMeter::default();
        let source = meter.buffered(audio(1));
        let other_source = other_meter.buffered(audio(1));
        let mut receipts = SpeechReceipts::default();
        let cancelled = receipts.register(None, 1, None, source.playback());
        let other = receipts.register(None, 2, None, other_source.playback());
        receipts.cancel_generation(1);
        meter.stop();
        let mut journal = LifecycleJournal::default();
        receipts.poll(&mut journal);
        assert_eq!(
            journal.drain(),
            vec![SequencedEvent {
                sequence: 1,
                event: LifecycleEvent::SpeechStopped {
                    playback_id: cancelled,
                    character: None,
                    voice_generation: 1,
                    reply_id: None,
                    reason: SpeechStopReason::Cancelled,
                },
            }]
        );
        assert_eq!(receipts.receipts.len(), 1);
        assert_eq!(receipts.receipts[0].playback_id, other);
        receipts.poll(&mut journal);
        assert!(journal.drain().is_empty());
        receipts.cancel_generation(2);
        receipts.poll(&mut journal);
        assert!(
            journal.drain().is_empty(),
            "intent alone is not invalidation"
        );
        other_meter.stop();
        receipts.poll(&mut journal);
        assert!(matches!(journal.drain()[0].event,
            LifecycleEvent::SpeechStopped { playback_id, reason: SpeechStopReason::Cancelled, .. }
            if playback_id == other));
    }

    #[test]
    fn cancellation_preserves_start_and_does_not_reemit_previous_start() {
        for poll_before_cancel in [false, true] {
            let meter = SpeechMeter::default();
            let source = meter.buffered(audio(2));
            let mut receipts = SpeechReceipts::default();
            let playback_id = receipts.register(None, 9, None, source.playback());
            let mut decoder = source.decoder();
            assert_eq!(decoder.next(), Some(0.0));
            let mut journal = LifecycleJournal::default();
            if poll_before_cancel {
                receipts.poll(&mut journal);
            }
            receipts.cancel_generation(9);
            meter.stop();
            receipts.poll(&mut journal);
            let records = journal.drain();
            assert_eq!(records.len(), 2);
            assert!(matches!(records[0].event, LifecycleEvent::SpeechStarted {
                playback_id: id, ..
            } if id == playback_id));
            assert!(matches!(records[1].event, LifecycleEvent::SpeechStopped {
                playback_id: id, reason: SpeechStopReason::Cancelled, ..
            } if id == playback_id));
            receipts.poll(&mut journal);
            assert!(journal.drain().is_empty());
        }
    }

    #[test]
    fn deferred_cancellation_preserves_consumption_until_actual_invalidation() {
        let meter = SpeechMeter::default();
        let source = meter.buffered(audio(2));
        let mut receipts = SpeechReceipts::default();
        receipts.register(None, 1, None, source.playback());
        let mut journal = LifecycleJournal::default();
        receipts.cancel_generation(1);
        receipts.poll(&mut journal);
        assert!(journal.drain().is_empty());
        let mut decoder = source.decoder();
        assert_eq!(decoder.next(), Some(0.0));
        meter.stop();
        receipts.poll(&mut journal);
        let events = journal.drain();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0].event,
            LifecycleEvent::SpeechStarted { .. }
        ));
        assert!(matches!(
            events[1].event,
            LifecycleEvent::SpeechStopped {
                reason: SpeechStopReason::Cancelled,
                ..
            }
        ));
    }

    #[test]
    fn cancelling_after_observed_eof_preserves_terminal_reason() {
        let meter = SpeechMeter::default();
        let source = meter.buffered(audio(1));
        let mut receipts = SpeechReceipts::default();
        receipts.register(None, 1, None, source.playback());
        let mut decoder = source.decoder();
        while decoder.next().is_some() {}
        receipts.cancel_generation(1);
        let mut journal = LifecycleJournal::default();
        receipts.poll(&mut journal);
        assert!(matches!(
            journal.drain()[1].event,
            LifecycleEvent::SpeechStopped {
                reason: SpeechStopReason::ObservedTerminal,
                ..
            }
        ));
    }

    #[test]
    fn independent_receipts_overflow_reports_gap_and_keeps_each_retained_identity() {
        let mut receipts = SpeechReceipts::default();
        let mut playback_ids = Vec::new();
        for _ in 0..MAX_SPEECH_RECEIPTS + 3 {
            // Independent meters avoid invalidating earlier registrations.
            let meter = SpeechMeter::default();
            let source = meter.buffered(audio(1));
            playback_ids.push(receipts.register(None, 5, None, source.playback()));
            let mut decoder = source.decoder();
            assert_eq!(decoder.next(), Some(0.0));
            assert_eq!(decoder.next(), None);
        }
        assert_eq!(receipts.receipts.len(), MAX_SPEECH_RECEIPTS);
        assert_eq!(receipts.dropped(), 3);
        let mut journal = LifecycleJournal::default();
        receipts.poll(&mut journal);
        let records = journal.drain();
        assert_eq!(records.len(), 2 * MAX_SPEECH_RECEIPTS + 1);
        for (pair, id) in records[..2 * MAX_SPEECH_RECEIPTS]
            .as_chunks::<2>()
            .0
            .iter()
            .zip(&playback_ids[3..])
        {
            assert!(matches!(pair[0].event, LifecycleEvent::SpeechStarted {
                playback_id, ..
            } if playback_id == *id));
            assert!(matches!(pair[1].event, LifecycleEvent::SpeechStopped {
                playback_id, reason: SpeechStopReason::ObservedTerminal, ..
            } if playback_id == *id));
        }
        assert_eq!(
            records.last().unwrap().event,
            LifecycleEvent::Gap {
                dropped_records: 0,
                dropped_receipts: 3,
            }
        );
        assert_eq!(journal.dropped(), 0);
        receipts.poll(&mut journal);
        assert!(journal.drain().is_empty());
        assert_eq!(receipts.dropped(), 3);
    }

    #[test]
    fn receipt_loss_survives_journal_pressure_until_drain() {
        let mut journal = LifecycleJournal::default();
        journal.note_receipt_loss(3);
        for _ in 0..MAX_JOURNAL_RECORDS + 2 {
            assert!(journal.record(reply(ReplyOutcome::Cancelled)).is_some());
        }
        let records = journal.drain();
        assert_eq!(records.len(), MAX_JOURNAL_RECORDS);
        assert_eq!(
            records.last().unwrap().event,
            LifecycleEvent::Gap {
                dropped_records: 3,
                dropped_receipts: 3,
            }
        );
        assert!(journal.drain().is_empty());
    }
}

pub(crate) fn motion_metadata_bytes(event: &LifecycleEvent) -> usize {
    match event {
        LifecycleEvent::MotionPlaybackStarted {
            motion_id, source, ..
        }
        | LifecycleEvent::MotionPlaybackEnded {
            motion_id, source, ..
        } => {
            motion_id.as_ref().map_or(0, String::capacity)
                + match source {
                    MotionPlaybackSource::Host => 0,
                    MotionPlaybackSource::Extension {
                        integration_id,
                        action_id,
                        ..
                    } => integration_id.capacity() + action_id.capacity(),
                }
        }
        _ => 0,
    }
}
