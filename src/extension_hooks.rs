//! Permission-filtered snapshots for bounded, one-shot lifecycle hooks.
//! Source identity never supplies a current action target. The runtime owns
//! queue admission, expiry, process budgets and launch-time permission checks.

use crate::{
    extension_lifecycle::{LifecycleEvent, ReplyOutcome},
    extension_process::ProcessSpec,
    extension_protocol::Capability,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, mem::size_of};

pub const MAX_CAPTURED_EVENT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookTrigger {
    #[serde(rename = "motion.playback.started")]
    MotionPlaybackStarted,
    #[serde(rename = "motion.playback.ended")]
    MotionPlaybackEnded,
    #[serde(rename = "extension.stopping")]
    ExtensionStopping,
    #[serde(rename = "extension.stopped")]
    ExtensionStopped,
    #[serde(rename = "extension.started")]
    ExtensionStarted,
    #[serde(rename = "app.ready")]
    AppReady,
    #[serde(rename = "character.selected")]
    CharacterSelected,
    #[serde(rename = "reply.finished")]
    ReplyFinished,
    #[serde(rename = "speech.started")]
    SpeechStarted,
    #[serde(rename = "speech.stopped")]
    SpeechStopped,
    #[serde(rename = "emotion.changed")]
    EmotionChanged,
}

impl HookTrigger {
    pub fn of(event: &LifecycleEvent) -> Option<Self> {
        match event {
            LifecycleEvent::ExtensionStarted { .. } => Some(Self::ExtensionStarted),
            LifecycleEvent::MotionPlaybackStarted { .. } => Some(Self::MotionPlaybackStarted),
            LifecycleEvent::MotionPlaybackEnded { .. } => Some(Self::MotionPlaybackEnded),
            LifecycleEvent::ExtensionStopping { .. } => Some(Self::ExtensionStopping),
            LifecycleEvent::ExtensionStopped { .. } => Some(Self::ExtensionStopped),
            LifecycleEvent::AppReady { .. } => Some(Self::AppReady),
            LifecycleEvent::CharacterSelected { .. } => Some(Self::CharacterSelected),
            LifecycleEvent::ReplyFinished { .. } => Some(Self::ReplyFinished),
            LifecycleEvent::SpeechStarted { .. } => Some(Self::SpeechStarted),
            LifecycleEvent::SpeechStopped { .. } => Some(Self::SpeechStopped),
            LifecycleEvent::EmotionChanged { .. } => Some(Self::EmotionChanged),
            LifecycleEvent::Gap { .. } => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HookProcess {
    pub trigger: HookTrigger,
    pub process: ProcessSpec,
}

#[derive(Clone, Debug)]
pub struct CapturedHookEvent {
    event: LifecycleEvent,
    grants: BTreeSet<Capability>,
}

impl CapturedHookEvent {
    /// Capture only authorized data, checking retained text length before any
    /// payload copy. Provider errors are never copied, including with ReplyRead.
    /// An observed source gap is explicit loss, not an ordinary hook trigger.
    pub fn capture(
        source: &LifecycleEvent,
        grants: &BTreeSet<Capability>,
    ) -> Result<Option<Self>, String> {
        if !grants.contains(&Capability::LifecycleObserve) {
            return Ok(None);
        }
        if let LifecycleEvent::Gap {
            dropped_records,
            dropped_receipts,
        } = source
        {
            return Err(format!(
                "Hook lifecycle source lost {dropped_records} records and {dropped_receipts} receipts"
            ));
        }
        if HookTrigger::of(source).is_none() {
            return Ok(None);
        }
        let text = match source {
            LifecycleEvent::ReplyFinished {
                outcome: ReplyOutcome::Completed { text },
                ..
            } if grants.contains(&Capability::ReplyRead) => text.as_str(),
            _ => "",
        };
        if text
            .len()
            .saturating_add(crate::extension_lifecycle::motion_metadata_bytes(source))
            > MAX_CAPTURED_EVENT_BYTES.saturating_sub(size_of::<Self>())
        {
            return Err(format!(
                "Hook lifecycle event exceeds {MAX_CAPTURED_EVENT_BYTES} captured bytes"
            ));
        }
        let event = match source {
            LifecycleEvent::ReplyFinished {
                reply_id,
                character,
                outcome,
            } => LifecycleEvent::ReplyFinished {
                reply_id: *reply_id,
                character: *character,
                outcome: match outcome {
                    ReplyOutcome::Completed { .. } => ReplyOutcome::Completed {
                        // Compact allocation: do not inherit source capacity.
                        text: Box::<str>::from(text).into_string(),
                    },
                    ReplyOutcome::Failed { .. } => ReplyOutcome::Failed {
                        error: String::new(),
                    },
                    ReplyOutcome::Cancelled => ReplyOutcome::Cancelled,
                },
            },
            LifecycleEvent::ExtensionStarted { .. }
            | LifecycleEvent::MotionPlaybackStarted { .. }
            | LifecycleEvent::MotionPlaybackEnded { .. }
            | LifecycleEvent::ExtensionStopping { .. }
            | LifecycleEvent::ExtensionStopped { .. }
            | LifecycleEvent::AppReady { .. }
            | LifecycleEvent::CharacterSelected { .. }
            | LifecycleEvent::SpeechStarted { .. }
            | LifecycleEvent::SpeechStopped { .. }
            | LifecycleEvent::EmotionChanged { .. } => source.clone(),
            LifecycleEvent::Gap { .. } => unreachable!("source gap rejected before capture"),
        };
        Ok(Some(Self {
            event,
            // Startup/selection hooks observe metadata only, even when the installation
            // has controller, reply-read or event-emission permissions.
            grants: if matches!(
                source,
                LifecycleEvent::ExtensionStarted { .. }
                    | LifecycleEvent::MotionPlaybackStarted { .. }
                    | LifecycleEvent::MotionPlaybackEnded { .. }
                    | LifecycleEvent::ExtensionStopping { .. }
                    | LifecycleEvent::ExtensionStopped { .. }
                    | LifecycleEvent::AppReady { .. }
                    | LifecycleEvent::CharacterSelected { .. }
            ) {
                BTreeSet::from([Capability::LifecycleObserve])
            } else {
                grants.clone()
            },
        }))
    }

    pub fn event(&self) -> &LifecycleEvent {
        &self.event
    }

    pub fn selection_revision(&self) -> Option<u64> {
        match &self.event {
            LifecycleEvent::CharacterSelected {
                selection_revision, ..
            } => Some(*selection_revision),
            _ => None,
        }
    }

    pub fn metadata_only(&self) -> bool {
        matches!(
            self.event,
            LifecycleEvent::ExtensionStarted { .. }
                | LifecycleEvent::MotionPlaybackStarted { .. }
                | LifecycleEvent::MotionPlaybackEnded { .. }
                | LifecycleEvent::ExtensionStopping { .. }
                | LifecycleEvent::ExtensionStopped { .. }
                | LifecycleEvent::AppReady { .. }
                | LifecycleEvent::CharacterSelected { .. }
        )
    }

    pub fn trigger(&self) -> HookTrigger {
        HookTrigger::of(&self.event).expect("captured events have a hook trigger")
    }

    /// Queue accounting: inline snapshot size plus retained compact text
    /// capacity. This is not a measurement of allocator or grant-tree overhead.
    pub fn bytes(&self) -> usize {
        size_of::<Self>()
            + crate::extension_lifecycle::motion_metadata_bytes(&self.event)
            + match &self.event {
                LifecycleEvent::ReplyFinished {
                    outcome: ReplyOutcome::Completed { text },
                    ..
                } => text.capacity(),
                _ => 0,
            }
    }

    /// Apply every grant change, including revoke/regrant between frames.
    /// Permissions can only decrease for this snapshot; discarded text cannot
    /// return. False means discard this event because observation was revoked.
    pub fn restrict(&mut self, current: &BTreeSet<Capability>) -> bool {
        self.grants.retain(|grant| current.contains(grant));
        let observing = self.grants.contains(&Capability::LifecycleObserve);
        if (!observing || !self.grants.contains(&Capability::ReplyRead))
            && let LifecycleEvent::ReplyFinished {
                outcome: ReplyOutcome::Completed { text },
                ..
            } = &mut self.event
        {
            *text = String::new();
        }
        observing
    }

    pub fn grants(&self) -> &BTreeSet<Capability> {
        &self.grants
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn motion_end_capture_strips_all_actions_messages_and_events() {
        let event = LifecycleEvent::MotionPlaybackEnded {
            playback_id: uuid::Uuid::new_v4(),
            source_target: Some(crate::extension_protocol::Target {
                uuid: uuid::Uuid::new_v4(),
                generation: 7,
            }),
            motion_id: Some("org.example.pack/wave".into()),
            source: crate::extension_protocol::MotionPlaybackSource::Host,
            outcome: crate::extension_protocol::MotionOutcome::Completed,
        };
        let grants = BTreeSet::from([
            Capability::LifecycleObserve,
            Capability::MotionPlay,
            Capability::ExpressionApply,
            Capability::MessageDeliver,
            Capability::EventEmit,
            Capability::ReplyRead,
            Capability::CatalogGet,
        ]);
        let mut captured = CapturedHookEvent::capture(&event, &grants)
            .unwrap()
            .unwrap();
        assert_eq!(captured.trigger(), HookTrigger::MotionPlaybackEnded);
        assert!(captured.metadata_only());
        assert_eq!(captured.selection_revision(), None);
        assert_eq!(captured.event(), &event);
        assert_eq!(
            captured.grants(),
            &BTreeSet::from([Capability::LifecycleObserve])
        );
        assert!(captured.restrict(&grants));
        assert!(!captured.restrict(&BTreeSet::new()));
    }

    #[test]
    fn motion_start_capture_strips_all_actions_messages_and_events() {
        let event = LifecycleEvent::MotionPlaybackStarted {
            playback_id: uuid::Uuid::new_v4(),
            source_target: Some(crate::extension_protocol::Target {
                uuid: uuid::Uuid::new_v4(),
                generation: 7,
            }),
            motion_id: Some("org.example.pack/wave".into()),
            source: crate::extension_protocol::MotionPlaybackSource::Host,
        };
        let grants = BTreeSet::from([
            Capability::LifecycleObserve,
            Capability::MotionPlay,
            Capability::ExpressionApply,
            Capability::MessageDeliver,
            Capability::EventEmit,
            Capability::ReplyRead,
            Capability::CatalogGet,
        ]);
        let mut captured = CapturedHookEvent::capture(&event, &grants)
            .unwrap()
            .unwrap();
        assert_eq!(captured.trigger(), HookTrigger::MotionPlaybackStarted);
        assert!(captured.metadata_only());
        assert_eq!(captured.selection_revision(), None);
        assert_eq!(captured.event(), &event);
        assert_eq!(
            captured.grants(),
            &BTreeSet::from([Capability::LifecycleObserve])
        );
        assert!(captured.restrict(&grants));
        assert!(!captured.restrict(&BTreeSet::new()));
    }

    #[test]
    fn stop_hooks_are_metadata_only_and_revoke_without_action_upgrade() {
        use crate::extension_lifecycle::ExtensionStopReason;
        let installation_id = uuid::Uuid::new_v4();
        let session_id = uuid::Uuid::new_v4();
        for event in [
            LifecycleEvent::ExtensionStopping {
                installation_id,
                session_id,
                process_generation: 1,
                reason: ExtensionStopReason::Disabled,
            },
            LifecycleEvent::ExtensionStopped {
                installation_id,
                session_id,
                process_generation: 1,
                exit_code: Some(7),
                requested_reason: None,
            },
        ] {
            let grants = BTreeSet::from([
                Capability::LifecycleObserve,
                Capability::ExpressionApply,
                Capability::ReplyRead,
            ]);
            let mut captured = CapturedHookEvent::capture(&event, &grants)
                .unwrap()
                .unwrap();
            assert!(captured.metadata_only());
            assert_eq!(captured.selection_revision(), None);
            assert_eq!(
                captured.grants(),
                &BTreeSet::from([Capability::LifecycleObserve])
            );
            assert_eq!(captured.event(), &event);
            assert!(captured.restrict(&grants));
            assert_eq!(captured.grants().len(), 1);
            assert!(!captured.restrict(&BTreeSet::new()));
            assert!(
                CapturedHookEvent::capture(&event, &BTreeSet::new())
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn extension_started_capture_has_no_action_authority_or_selection_binding() {
        let source = LifecycleEvent::ExtensionStarted {
            installation_id: uuid::Uuid::new_v4(),
            session_id: uuid::Uuid::new_v4(),
            process_generation: 2,
        };
        let grants = BTreeSet::from([
            Capability::LifecycleObserve,
            Capability::ReplyRead,
            Capability::MotionPlay,
            Capability::ExpressionApply,
            Capability::MessageDeliver,
            Capability::EventEmit,
        ]);
        let mut captured = CapturedHookEvent::capture(&source, &grants)
            .unwrap()
            .unwrap();
        assert_eq!(captured.event(), &source);
        assert_eq!(captured.trigger(), HookTrigger::ExtensionStarted);
        assert!(captured.metadata_only());
        assert_eq!(captured.selection_revision(), None);
        assert_eq!(
            captured.grants(),
            &BTreeSet::from([Capability::LifecycleObserve])
        );
        assert!(captured.restrict(&grants));
        assert_eq!(captured.grants().len(), 1);
        assert!(!captured.restrict(&BTreeSet::new()));
        assert!(
            CapturedHookEvent::capture(&source, &BTreeSet::new())
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn app_ready_capture_has_only_observation_grant_and_no_selection_revision() {
        let source = LifecycleEvent::AppReady {
            app_run_id: uuid::Uuid::new_v4(),
        };
        let grants = BTreeSet::from([
            Capability::LifecycleObserve,
            Capability::ReplyRead,
            Capability::ExpressionApply,
            Capability::MotionPlay,
            Capability::MessageDeliver,
            Capability::EventEmit,
        ]);
        let mut captured = CapturedHookEvent::capture(&source, &grants)
            .unwrap()
            .unwrap();
        assert_eq!(captured.event(), &source);
        assert_eq!(captured.trigger(), HookTrigger::AppReady);
        assert!(captured.metadata_only());
        assert_eq!(captured.selection_revision(), None);
        assert_eq!(
            captured.grants(),
            &BTreeSet::from([Capability::LifecycleObserve])
        );
        assert!(captured.restrict(&grants));
        assert_eq!(captured.grants().len(), 1);
        assert!(!captured.restrict(&BTreeSet::new()));
        assert!(
            CapturedHookEvent::capture(&source, &BTreeSet::new())
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn selection_capture_never_carries_action_or_private_text_grants() {
        let source = LifecycleEvent::CharacterSelected {
            previous: None,
            new: Some(uuid::Uuid::from_u128(1)),
            selection_revision: 3,
        };
        let grants = BTreeSet::from([
            Capability::LifecycleObserve,
            Capability::ReplyRead,
            Capability::ExpressionApply,
            Capability::MotionPlay,
            Capability::MessageDeliver,
            Capability::EventEmit,
        ]);
        let mut captured = CapturedHookEvent::capture(&source, &grants)
            .unwrap()
            .unwrap();
        assert_eq!(captured.trigger(), HookTrigger::CharacterSelected);
        assert_eq!(captured.selection_revision(), Some(3));
        assert_eq!(
            captured.grants(),
            &BTreeSet::from([Capability::LifecycleObserve])
        );
        assert!(captured.restrict(&grants));
        assert_eq!(captured.grants().len(), 1);
        assert!(!captured.restrict(&BTreeSet::new()));
        assert!(
            CapturedHookEvent::capture(&source, &BTreeSet::new())
                .unwrap()
                .is_none()
        );
    }
    use crate::{
        extension_protocol::{EmotionOwner, SpeechStopReason},
        reply_affect::AffectEmotion,
    };
    use uuid::Uuid;

    fn grants(read: bool) -> BTreeSet<Capability> {
        let mut grants = BTreeSet::from([Capability::LifecycleObserve]);
        if read {
            grants.insert(Capability::ReplyRead);
        }
        grants
    }

    #[test]
    fn emotion_hooks_snapshot_source_owner_and_optional_reply_without_private_text() {
        for owner in [EmotionOwner::Studio, EmotionOwner::SceneChat] {
            for reply_id in [None, Some(Uuid::from_u128(10))] {
                for read in [false, true] {
                    let mut source = LifecycleEvent::EmotionChanged {
                        character: Uuid::from_u128(9),
                        owner,
                        previous: AffectEmotion::Happy,
                        emotion: AffectEmotion::Neutral,
                        reply_id,
                    };
                    let expected = source.clone();
                    let snapshot = captured(&source, &grants(read));
                    assert_eq!(HookTrigger::of(&source), Some(HookTrigger::EmotionChanged));
                    assert_eq!(snapshot.trigger(), HookTrigger::EmotionChanged);
                    assert_eq!(snapshot.event(), &expected);
                    assert_eq!(snapshot.grants(), &grants(read));
                    assert_eq!(snapshot.bytes(), size_of::<CapturedHookEvent>());
                    assert!(snapshot.bytes() <= MAX_CAPTURED_EVENT_BYTES);

                    // A later source change cannot change queued ownership or
                    // infer a new reply identity. There is no retained text.
                    source = LifecycleEvent::EmotionChanged {
                        character: Uuid::from_u128(99),
                        owner: match owner {
                            EmotionOwner::Studio => EmotionOwner::SceneChat,
                            EmotionOwner::SceneChat => EmotionOwner::Studio,
                        },
                        previous: AffectEmotion::Neutral,
                        emotion: AffectEmotion::Sad,
                        reply_id: Some(Uuid::from_u128(100)),
                    };
                    assert_ne!(snapshot.event(), &source);
                    assert_eq!(snapshot.event(), &expected);
                }
            }
        }
    }

    #[test]
    fn emotion_hooks_require_observation_and_permanently_restrict_permissions() {
        let source = LifecycleEvent::EmotionChanged {
            character: Uuid::from_u128(9),
            owner: EmotionOwner::SceneChat,
            previous: AffectEmotion::Neutral,
            emotion: AffectEmotion::Happy,
            reply_id: None,
        };
        for permissions in [BTreeSet::new(), BTreeSet::from([Capability::ReplyRead])] {
            assert!(
                CapturedHookEvent::capture(&source, &permissions)
                    .unwrap()
                    .is_none()
            );
        }

        let mut permissions = grants(true);
        permissions.insert(Capability::MotionPlay);
        let mut snapshot = captured(&source, &permissions);
        assert!(snapshot.restrict(&grants(false)));
        assert_eq!(snapshot.event(), &source);
        assert_eq!(snapshot.grants(), &grants(false));
        assert_eq!(snapshot.bytes(), size_of::<CapturedHookEvent>());
        assert!(snapshot.restrict(&permissions));
        assert_eq!(
            snapshot.grants(),
            &grants(false),
            "regrant cannot expand a captured snapshot"
        );
        assert_eq!(snapshot.event(), &source);
        assert!(!snapshot.restrict(&BTreeSet::from([Capability::ReplyRead])));
        assert!(
            !snapshot.restrict(&permissions),
            "observation revocation permanently invalidates the snapshot"
        );
        assert!(snapshot.grants().is_empty());
        assert_eq!(snapshot.bytes(), size_of::<CapturedHookEvent>());
    }

    fn reply(text: String) -> LifecycleEvent {
        LifecycleEvent::ReplyFinished {
            reply_id: Uuid::new_v4(),
            character: Some(Uuid::new_v4()),
            outcome: ReplyOutcome::Completed { text },
        }
    }

    fn captured(source: &LifecycleEvent, grants: &BTreeSet<Capability>) -> CapturedHookEvent {
        CapturedHookEvent::capture(source, grants).unwrap().unwrap()
    }

    fn text(event: &CapturedHookEvent) -> &str {
        match event.event() {
            LifecycleEvent::ReplyFinished {
                outcome: ReplyOutcome::Completed { text },
                ..
            } => text,
            _ => panic!("expected a completed reply"),
        }
    }

    #[test]
    fn captures_independent_compact_source_and_permission_snapshots() {
        let mut source_text = String::with_capacity(MAX_CAPTURED_EVENT_BYTES * 2);
        source_text.push_str("Hello 世界 🌸");
        let mut source = reply(source_text);
        let expected = source.clone();
        let mut permissions = grants(true);
        permissions.insert(Capability::MotionPlay);
        let snapshot = captured(&source, &permissions);
        let mut clone = snapshot.clone();
        permissions.clear();
        if let LifecycleEvent::ReplyFinished {
            reply_id,
            character,
            outcome: ReplyOutcome::Completed { text },
        } = &mut source
        {
            *reply_id = Uuid::new_v4();
            *character = None;
            text.clear();
        }
        assert_eq!(snapshot.event(), &expected);
        assert_eq!(snapshot.trigger(), HookTrigger::ReplyFinished);
        assert!(snapshot.grants().contains(&Capability::MotionPlay));
        assert_eq!(
            snapshot.bytes(),
            size_of::<CapturedHookEvent>() + text(&snapshot).len()
        );
        assert!(clone.restrict(&grants(false)));
        assert!(text(&clone).is_empty());
        assert!(!clone.grants().contains(&Capability::MotionPlay));
        assert!(clone.restrict(snapshot.grants()));
        assert!(!clone.grants().contains(&Capability::MotionPlay));
        assert_eq!(snapshot.event(), &expected);
    }

    #[test]
    fn speech_and_cancelled_reply_preserve_all_source_identities() {
        let playback_id = Uuid::new_v4();
        let character = Some(Uuid::new_v4());
        let reply_id = Uuid::new_v4();
        let cases = [
            (
                LifecycleEvent::SpeechStarted {
                    playback_id,
                    character,
                    voice_generation: 91,
                    reply_id: Some(reply_id),
                },
                HookTrigger::SpeechStarted,
            ),
            (
                LifecycleEvent::SpeechStopped {
                    playback_id,
                    character: None,
                    voice_generation: 92,
                    reply_id: None,
                    reason: SpeechStopReason::Cancelled,
                },
                HookTrigger::SpeechStopped,
            ),
            (
                LifecycleEvent::ReplyFinished {
                    reply_id,
                    character,
                    outcome: ReplyOutcome::Cancelled,
                },
                HookTrigger::ReplyFinished,
            ),
        ];
        for (source, trigger) in cases {
            let snapshot = captured(&source, &grants(false));
            assert_eq!(snapshot.event(), &source);
            assert_eq!(snapshot.trigger(), trigger);
            assert_eq!(snapshot.bytes(), size_of::<CapturedHookEvent>());
        }
    }

    #[test]
    fn revocation_and_regrant_never_restore_text_or_permissions() {
        let source = reply("private completed reply".into());
        let mut snapshot = captured(&source, &grants(true));
        assert!(snapshot.restrict(&grants(false)));
        assert!(text(&snapshot).is_empty());
        assert_eq!(snapshot.bytes(), size_of::<CapturedHookEvent>());
        assert!(snapshot.restrict(&grants(true)));
        assert!(!snapshot.grants().contains(&Capability::ReplyRead));
        assert!(text(&snapshot).is_empty());
        assert!(!snapshot.restrict(&BTreeSet::from([Capability::ReplyRead])));
        assert!(!snapshot.restrict(&grants(true)));
        assert!(snapshot.grants().is_empty());

        let mut snapshot = captured(&source, &grants(true));
        assert!(!snapshot.restrict(&BTreeSet::from([Capability::ReplyRead])));
        assert!(text(&snapshot).is_empty());
        assert_eq!(snapshot.bytes(), size_of::<CapturedHookEvent>());
        assert!(!snapshot.restrict(&grants(true)));
        assert!(text(&snapshot).is_empty());
    }

    #[test]
    fn ungranted_text_is_never_copied_or_recovered_and_provider_errors_are_stripped() {
        let source = reply("x".repeat(MAX_CAPTURED_EVENT_BYTES + 1));
        for permissions in [BTreeSet::new(), BTreeSet::from([Capability::ReplyRead])] {
            assert!(
                CapturedHookEvent::capture(&source, &permissions)
                    .unwrap()
                    .is_none()
            );
        }
        // An oversized private source still permits its small authorized metadata.
        let mut snapshot = captured(&source, &grants(false));
        assert!(text(&snapshot).is_empty());
        assert_eq!(snapshot.bytes(), size_of::<CapturedHookEvent>());
        assert!(snapshot.restrict(&grants(true)));
        assert!(text(&snapshot).is_empty());
        assert!(!snapshot.grants().contains(&Capability::ReplyRead));

        let reply_id = Uuid::new_v4();
        let provider_error = "secret provider error";
        let source = LifecycleEvent::ReplyFinished {
            reply_id,
            character: None,
            outcome: ReplyOutcome::Failed {
                error: provider_error.repeat(MAX_CAPTURED_EVENT_BYTES / provider_error.len() + 1),
            },
        };
        for permissions in [grants(false), grants(true)] {
            let snapshot = captured(&source, &permissions);
            assert_eq!(
                snapshot.event(),
                &LifecycleEvent::ReplyFinished {
                    reply_id,
                    character: None,
                    outcome: ReplyOutcome::Failed {
                        error: String::new()
                    },
                }
            );
            assert_eq!(snapshot.bytes(), size_of::<CapturedHookEvent>());
        }
    }

    #[test]
    fn observing_source_gaps_are_explicit_loss_without_exposing_a_trigger() {
        let gap = LifecycleEvent::Gap {
            dropped_records: 3,
            dropped_receipts: 7,
        };
        assert_eq!(HookTrigger::of(&gap), None);
        let error = CapturedHookEvent::capture(&gap, &grants(false)).unwrap_err();
        assert!(error.contains("3 records"));
        assert!(error.contains("7 receipts"));
        assert!(CapturedHookEvent::capture(&gap, &grants(true)).is_err());
        assert!(
            CapturedHookEvent::capture(&gap, &BTreeSet::new())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn enforces_exact_retained_byte_budget_before_copying_utf8_text() {
        let budget = MAX_CAPTURED_EVENT_BYTES - size_of::<CapturedHookEvent>();
        let source = reply("x".repeat(budget));
        assert_eq!(
            captured(&source, &grants(true)).bytes(),
            MAX_CAPTURED_EVENT_BYTES
        );
        let source = reply("x".repeat(budget + 1));
        assert!(CapturedHookEvent::capture(&source, &grants(true)).is_err());
        let source = reply("🌸".repeat(budget / 4 + 1));
        assert!(CapturedHookEvent::capture(&source, &grants(true)).is_err());
    }

    #[test]
    fn trigger_wire_names_are_exact() {
        for (trigger, name) in [
            (HookTrigger::ReplyFinished, "reply.finished"),
            (HookTrigger::SpeechStarted, "speech.started"),
            (HookTrigger::SpeechStopped, "speech.stopped"),
            (HookTrigger::EmotionChanged, "emotion.changed"),
        ] {
            assert_eq!(serde_json::to_value(trigger).unwrap(), name);
            assert_eq!(
                serde_json::from_value::<HookTrigger>(serde_json::json!(name)).unwrap(),
                trigger
            );
        }
        assert!(serde_json::from_str::<HookTrigger>("\"reply_finished\"").is_err());
        assert!(serde_json::from_str::<HookTrigger>("\"emotion_changed\"").is_err());
        assert!(serde_json::from_str::<HookTrigger>("\"motion.finished\"").is_err());
    }
}
