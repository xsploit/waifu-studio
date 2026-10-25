//! Bounded retained motion activation/terminals, independent of replaceable state.
use super::PlaybackOutcome;
use crate::{
    extension_lifecycle::LifecycleEvent,
    extension_protocol::{HostEvent, MotionOutcome, MotionPlaybackSource, Target},
};
use bevy::prelude::Resource;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Instant,
};
use uuid::Uuid;

pub const MAX_MOTION_RECEIPTS: usize = 64;

/// Immutable host-authenticated source snapshot. Host calls without a proven
/// protocol target use Default; filesystem paths are never catalog identities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MotionSourceSnapshot {
    pub source_target: Option<Target>,
    pub motion_id: Option<String>,
    pub source: MotionPlaybackSource,
}
impl Default for MotionSourceSnapshot {
    fn default() -> Self {
        Self {
            source_target: None,
            motion_id: None,
            source: MotionPlaybackSource::Host,
        }
    }
}
impl MotionSourceSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        HostEvent::MotionPlaybackEnded {
            playback_id: Uuid::new_v4(),
            source_target: self.source_target,
            motion_id: self.motion_id.clone(),
            source: self.source.clone(),
            outcome: MotionOutcome::Cancelled,
        }
        .validate()
        .map_err(|error| error.to_string())
    }
}
#[derive(Clone, Debug)]
pub(super) struct MotionReceipt(Arc<RetainedMotion>);
#[derive(Debug)]
struct RetainedMotion {
    id: Uuid,
    snapshot: MotionSourceSnapshot,
    progress: Mutex<MotionProgress>,
}
#[derive(Debug, Default)]
struct MotionProgress {
    started_at: Option<Instant>,
    start_taken: bool,
    terminal: Option<(MotionOutcome, Instant)>,
}
impl MotionReceipt {
    pub(super) fn start(&self) {
        let mut progress = self
            .0
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if progress.terminal.is_none() && progress.started_at.is_none() && !progress.start_taken {
            progress.started_at = Some(Instant::now());
        }
    }
    #[cfg(test)]
    pub(super) fn id_for_test(&self) -> Uuid {
        self.0.id
    }

    pub(super) fn finish(&self, outcome: PlaybackOutcome) {
        let outcome = match outcome {
            PlaybackOutcome::Pending => return,
            PlaybackOutcome::Completed => MotionOutcome::Completed,
            PlaybackOutcome::Cancelled => MotionOutcome::Cancelled,
            PlaybackOutcome::Failed => MotionOutcome::Failed,
        };
        let mut progress = self
            .0
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if progress.terminal.is_none() {
            progress.terminal = Some((outcome, Instant::now()));
        }
    }
}
#[derive(Default, Debug)]
struct Retained {
    receipts: VecDeque<MotionReceipt>,
    lost: u64,
}
/// Keep this resource alive across animation replacement/parking/drop. Admission
/// evicts oldest receipts at a fixed bound; lost evidence becomes a source gap.
#[derive(Clone, Default, Resource, Debug)]
pub struct MotionObservations(Arc<Mutex<Retained>>);
impl MotionObservations {
    pub(super) fn admit(&self, mut snapshot: MotionSourceSnapshot) -> MotionReceipt {
        snapshot.motion_id = snapshot
            .motion_id
            .map(|id| id.into_boxed_str().into_string());
        if let MotionPlaybackSource::Extension {
            integration_id,
            action_id,
            ..
        } = &mut snapshot.source
        {
            *integration_id = std::mem::take(integration_id)
                .into_boxed_str()
                .into_string();
            *action_id = std::mem::take(action_id).into_boxed_str().into_string();
        }
        let receipt = MotionReceipt(Arc::new(RetainedMotion {
            id: Uuid::new_v4(),
            snapshot,
            progress: Mutex::new(MotionProgress::default()),
        }));
        let mut retained = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if retained.receipts.len() == MAX_MOTION_RECEIPTS {
            retained.receipts.pop_front();
            retained.lost = retained.lost.saturating_add(1);
        }
        retained.receipts.push_back(receipt.clone());
        receipt
    }
    /// Bind a newly replaced state without losing either state's admitted work.
    pub fn attach(&self, state: &mut super::AnimationState) {
        if Arc::ptr_eq(&self.0, &state.motion_observations.0) {
            return;
        }
        let (receipts, lost) = {
            let mut old = state
                .motion_observations
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                std::mem::take(&mut old.receipts),
                std::mem::take(&mut old.lost),
            )
        };
        let mut retained = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        retained.lost = retained.lost.saturating_add(lost);
        for receipt in receipts {
            if retained.receipts.len() == MAX_MOTION_RECEIPTS {
                retained.receipts.pop_front();
                retained.lost = retained.lost.saturating_add(1);
            }
            retained.receipts.push_back(receipt);
        }
        state.motion_observations = self.clone();
    }
    /// Retained activation/terminal mutations, with immutable admission provenance.
    /// Start means animator clip activation, not a rendered pose. A failed/cancelled
    /// import may terminate without activation. Each transition drains at most once.
    pub(crate) fn take_events(&self) -> (Vec<(LifecycleEvent, Instant)>, u64) {
        let mut retained = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut events = Vec::new();
        retained.receipts.retain(|receipt| {
            let mut progress = receipt
                .0
                .progress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let snapshot = &receipt.0.snapshot;
            if !progress.start_taken
                && let Some(at) = progress.started_at
            {
                events.push((
                    LifecycleEvent::MotionPlaybackStarted {
                        playback_id: receipt.0.id,
                        source_target: snapshot.source_target,
                        motion_id: snapshot.motion_id.clone(),
                        source: snapshot.source.clone(),
                    },
                    at,
                ));
                progress.start_taken = true;
            }
            if let Some((outcome, at)) = progress.terminal {
                events.push((
                    LifecycleEvent::MotionPlaybackEnded {
                        playback_id: receipt.0.id,
                        source_target: snapshot.source_target,
                        motion_id: snapshot.motion_id.clone(),
                        source: snapshot.source.clone(),
                        outcome,
                    },
                    at,
                ));
                false
            } else {
                true
            }
        });
        events.sort_by_key(|(_, at)| *at);
        (events, std::mem::take(&mut retained.lost))
    }
    #[cfg(test)]
    pub(crate) fn take_terminals(&self) -> (Vec<(LifecycleEvent, Instant)>, u64) {
        let (events, lost) = self.take_events();
        (
            events
                .into_iter()
                .filter(|(e, _)| matches!(e, LifecycleEvent::MotionPlaybackEnded { .. }))
                .collect(),
            lost,
        )
    }
}
