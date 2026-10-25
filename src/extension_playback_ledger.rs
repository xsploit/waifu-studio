//! Host-owned bounded playback accounting. No transport or audio ownership.
//! The caller must cancel the owned worker on a local terminal failure, and must
//! perform the runtime's live route/grant/target check in every send callback.
use crate::{
    extension_protocol::{AudioOutput, AudioPlaybackOutcome, AudioPlaybackReceipt, Target},
    extension_runtime::AudioPlaybackRoute,
};
use std::time::Duration;
use uuid::Uuid;

pub const MAX_PLAYBACK_IN_FLIGHT: u32 = 4;
const STALL: Duration = Duration::from_secs(5);
const LIFETIME: Duration = Duration::from_secs(22 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackLedgerError {
    Identity,
    State,
    Backpressure,
    InvalidFeedback,
    Transport,
    Clock,
    Timeout,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Created,
    Starting,
    Streaming,
    Finishing,
    Cancelling,
    Ended,
}

pub struct PlaybackLedger {
    route: AudioPlaybackRoute,
    id: Uuid,
    phase: Phase,
    cancelled_from: Option<Phase>,
    sent: u32,
    consumed: u32,
    created: Duration,
    progress: Duration,
    clock: Duration,
    outcome: Option<AudioPlaybackOutcome>,
}
impl PlaybackLedger {
    pub fn new(route: AudioPlaybackRoute, now: Duration) -> Self {
        Self {
            route,
            id: Uuid::new_v4(),
            phase: Phase::Created,
            cancelled_from: None,
            sent: 0,
            consumed: 0,
            created: now,
            progress: now,
            clock: now,
            outcome: None,
        }
    }
    pub fn route(&self) -> &AudioPlaybackRoute {
        &self.route
    }
    pub fn playback_id(&self) -> Uuid {
        self.id
    }
    pub fn sent_frames(&self) -> u32 {
        self.sent
    }
    pub fn consumed_frames(&self) -> u32 {
        self.consumed
    }
    pub fn ready_for_frames(&self) -> bool {
        self.phase == Phase::Streaming
    }
    pub fn has_frame_credit(&self) -> bool {
        self.ready_for_frames() && self.sent - self.consumed < MAX_PLAYBACK_IN_FLIGHT
    }
    pub fn outcome(&self) -> Option<AudioPlaybackOutcome> {
        self.outcome
    }
    pub fn tick(&mut self, now: Duration) -> Result<(), PlaybackLedgerError> {
        if self.phase == Phase::Ended {
            return Err(PlaybackLedgerError::State);
        }
        if now < self.clock {
            return self.fail(PlaybackLedgerError::Clock);
        }
        self.clock = now;
        let worker_owes_progress = self.phase != Phase::Streaming || self.sent > self.consumed;
        if now - self.created >= LIFETIME || (worker_owes_progress && now - self.progress >= STALL)
        {
            return self.fail(PlaybackLedgerError::Timeout);
        }
        Ok(())
    }
    fn fail<T>(&mut self, error: PlaybackLedgerError) -> Result<T, PlaybackLedgerError> {
        self.phase = Phase::Ended;
        self.outcome = Some(AudioPlaybackOutcome::Failed);
        Err(error)
    }
    /// This closure must perform checked transport admission exactly once.
    /// Backpressure does not invoke it; uncertain failure is terminal, never retried.
    pub fn send(
        &mut self,
        output: AudioOutput,
        now: Duration,
        publish: impl FnOnce(&AudioPlaybackRoute, AudioOutput) -> bool,
    ) -> Result<(), PlaybackLedgerError> {
        self.tick(now)?;
        if output.validate().is_err() || output.target() != self.route.target {
            return Err(PlaybackLedgerError::Identity);
        }
        let (id, phase, sent) = match &output {
            AudioOutput::Start { playback_id, .. } if self.phase == Phase::Created => {
                (*playback_id, Phase::Starting, self.sent)
            }
            AudioOutput::Frame {
                playback_id,
                frame_sequence,
                ..
            } if self.phase == Phase::Streaming && *frame_sequence == self.sent => {
                if self.sent - self.consumed >= MAX_PLAYBACK_IN_FLIGHT {
                    return Err(PlaybackLedgerError::Backpressure);
                }
                (*playback_id, self.phase, self.sent + 1)
            }
            AudioOutput::Finish {
                playback_id,
                frame_count,
                ..
            } if self.phase == Phase::Streaming && *frame_count == self.sent => {
                (*playback_id, Phase::Finishing, self.sent)
            }
            AudioOutput::Cancel { playback_id, .. }
                if matches!(
                    self.phase,
                    Phase::Starting | Phase::Streaming | Phase::Finishing
                ) =>
            {
                (*playback_id, Phase::Cancelling, self.sent)
            }
            _ => return Err(PlaybackLedgerError::State),
        };
        if id != self.id {
            return Err(PlaybackLedgerError::Identity);
        }
        if !publish(&self.route, output) {
            return self.fail(PlaybackLedgerError::Transport);
        }
        if phase == Phase::Cancelling {
            self.cancelled_from = Some(self.phase);
        }
        self.phase = phase;
        self.sent = sent;
        self.progress = now;
        Ok(())
    }
    /// `source` must be reconstructed from current host-owned action attribution,
    /// never from child fields. The caller also checks the route is still live.
    pub fn receive(
        &mut self,
        source: &AudioPlaybackRoute,
        receipt: &AudioPlaybackReceipt,
        now: Duration,
    ) -> Result<(), PlaybackLedgerError> {
        if source != &self.route || receipt.playback_id() != self.id {
            return Err(PlaybackLedgerError::Identity);
        }
        self.tick(now)?;
        if receipt.validate().is_err() {
            return self.fail(PlaybackLedgerError::InvalidFeedback);
        }
        match receipt {
            // Ready may already be in flight when the host cancels startup.
            // Accept the race without reviving playback or extending the deadline.
            AudioPlaybackReceipt::Ready { .. }
                if self.phase == Phase::Cancelling
                    && self.cancelled_from == Some(Phase::Starting) =>
            {
                self.cancelled_from = Some(Phase::Streaming);
                return Ok(());
            }
            AudioPlaybackReceipt::Ready { .. } if self.phase == Phase::Starting => {
                self.phase = Phase::Streaming
            }
            AudioPlaybackReceipt::Consumed { frame_count, .. }
                if matches!(
                    self.phase,
                    Phase::Streaming | Phase::Finishing | Phase::Cancelling
                ) && *frame_count > self.consumed
                    && *frame_count <= self.sent =>
            {
                self.consumed = *frame_count
            }
            AudioPlaybackReceipt::Finished {
                frame_count,
                outcome,
                ..
            } if self.phase != Phase::Created
                && *frame_count >= self.consumed
                && *frame_count <= self.sent
                && (*outcome != AudioPlaybackOutcome::Completed
                    || ((self.phase == Phase::Finishing
                        || (self.phase == Phase::Cancelling
                            && self.cancelled_from == Some(Phase::Finishing)))
                        && *frame_count == self.sent)) =>
            {
                self.consumed = *frame_count;
                // A worker completion may already be in flight when Cancel is
                // sent. Accept that race without blaming the worker; host
                // cancellation intent still wins the host-facing outcome.
                self.outcome = Some(
                    if self.phase == Phase::Cancelling
                        && *outcome == AudioPlaybackOutcome::Completed
                    {
                        AudioPlaybackOutcome::Cancelled
                    } else {
                        *outcome
                    },
                );
                self.phase = Phase::Ended;
            }
            _ => return self.fail(PlaybackLedgerError::InvalidFeedback),
        }
        self.progress = now;
        Ok(())
    }
    pub fn target(&self) -> Target {
        self.route.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ledger() -> PlaybackLedger {
        PlaybackLedger::new(
            AudioPlaybackRoute {
                installation_id: Uuid::new_v4(),
                session: Uuid::new_v4(),
                process_generation: 1,
                target: Target {
                    uuid: Uuid::new_v4(),
                    generation: 0,
                },
                origin: crate::extension_protocol::IntegrationOrigin {
                    integration_id: "org.example.playback".into(),
                    display_name: "Owned fixture".into(),
                },
            },
            Duration::ZERO,
        )
    }
    fn start(l: &mut PlaybackLedger) {
        l.send(
            AudioOutput::Start {
                playback_id: l.id,
                target: l.target(),
            },
            Duration::ZERO,
            |_, _| true,
        )
        .unwrap();
    }
    fn ready(l: &mut PlaybackLedger) {
        l.receive(
            &l.route.clone(),
            &AudioPlaybackReceipt::Ready { playback_id: l.id },
            Duration::ZERO,
        )
        .unwrap();
    }
    fn frame(l: &PlaybackLedger) -> AudioOutput {
        AudioOutput::Frame {
            playback_id: l.id,
            target: l.target(),
            frame_sequence: l.sent,
            pcm: vec![0; crate::call_pcm::CALL_FRAME_SAMPLES],
        }
    }
    #[test]
    fn ledger_credits_require_ready_and_validated_consumption() {
        let mut l = ledger();
        start(&mut l);
        assert_eq!(
            l.send(frame(&l), Duration::ZERO, |_, _| panic!("not ready")),
            Err(PlaybackLedgerError::State)
        );
        ready(&mut l);
        for _ in 0..4 {
            l.send(frame(&l), Duration::ZERO, |_, _| true).unwrap();
        }
        assert_eq!(
            l.send(frame(&l), Duration::ZERO, |_, _| panic!("full")),
            Err(PlaybackLedgerError::Backpressure)
        );
        l.receive(
            &l.route.clone(),
            &AudioPlaybackReceipt::Consumed {
                playback_id: l.id,
                frame_count: 2,
            },
            Duration::ZERO,
        )
        .unwrap();
        for _ in 0..2 {
            l.send(frame(&l), Duration::ZERO, |_, _| true).unwrap();
        }
        assert_eq!((l.sent_frames(), l.consumed_frames()), (6, 2));
    }
    #[test]
    fn ledger_completion_needs_finish_and_all_admitted_frames() {
        let mut l = ledger();
        start(&mut l);
        ready(&mut l);
        l.send(frame(&l), Duration::ZERO, |_, _| true).unwrap();
        l.send(
            AudioOutput::Finish {
                playback_id: l.id,
                target: l.target(),
                frame_count: 1,
            },
            Duration::ZERO,
            |_, _| true,
        )
        .unwrap();
        assert_eq!(l.outcome(), None);
        l.receive(
            &l.route.clone(),
            &AudioPlaybackReceipt::Finished {
                playback_id: l.id,
                frame_count: 1,
                outcome: AudioPlaybackOutcome::Completed,
            },
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(l.outcome(), Some(AudioPlaybackOutcome::Completed));
        assert_eq!(
            l.send(frame(&l), Duration::ZERO, |_, _| panic!("terminal")),
            Err(PlaybackLedgerError::State)
        );
    }
    #[test]
    fn ledger_foreign_feedback_cannot_poison_or_release_current_credits() {
        let mut l = ledger();
        start(&mut l);
        ready(&mut l);
        let receipt = AudioPlaybackReceipt::Consumed {
            playback_id: l.id,
            frame_count: 1,
        };
        for change in 0..5 {
            let mut source = l.route.clone();
            match change {
                0 => source.installation_id = Uuid::new_v4(),
                1 => source.session = Uuid::new_v4(),
                2 => source.process_generation += 1,
                3 => source.target.generation += 1,
                _ => source.origin.integration_id = "org.example.foreign".into(),
            }
            assert_eq!(
                l.receive(&source, &receipt, Duration::ZERO),
                Err(PlaybackLedgerError::Identity)
            );
        }
        assert_eq!(l.consumed_frames(), 0);
        assert_eq!(l.outcome(), None);
    }
    #[test]
    fn ledger_impossible_or_nonprogress_feedback_is_terminal() {
        for case in 0..3 {
            let mut l = ledger();
            start(&mut l);
            ready(&mut l);
            l.send(frame(&l), Duration::ZERO, |_, _| true).unwrap();
            let receipt = match case {
                0 => AudioPlaybackReceipt::Consumed {
                    playback_id: l.id,
                    frame_count: 2,
                },
                1 => AudioPlaybackReceipt::Ready { playback_id: l.id },
                _ => AudioPlaybackReceipt::Finished {
                    playback_id: l.id,
                    frame_count: 1,
                    outcome: AudioPlaybackOutcome::Completed,
                },
            };
            assert_eq!(
                l.receive(&l.route.clone(), &receipt, Duration::ZERO),
                Err(PlaybackLedgerError::InvalidFeedback)
            );
            assert_eq!(l.outcome(), Some(AudioPlaybackOutcome::Failed));
        }
    }
    #[test]
    fn ledger_uncertain_transport_failure_is_not_retryable() {
        let mut l = ledger();
        start(&mut l);
        ready(&mut l);
        assert_eq!(
            l.send(frame(&l), Duration::ZERO, |_, _| false),
            Err(PlaybackLedgerError::Transport)
        );
        assert_eq!(l.sent_frames(), 0);
        assert_eq!(
            l.send(frame(&l), Duration::ZERO, |_, _| panic!("replayed")),
            Err(PlaybackLedgerError::State)
        );
    }
    #[test]
    fn ledger_startup_cancel_accepts_in_flight_ready_without_reviving() {
        let mut l = ledger();
        start(&mut l);
        l.send(
            AudioOutput::Cancel {
                playback_id: l.id,
                target: l.target(),
            },
            Duration::ZERO,
            |_, _| true,
        )
        .unwrap();
        l.receive(
            &l.route.clone(),
            &AudioPlaybackReceipt::Ready { playback_id: l.id },
            Duration::from_secs(4),
        )
        .unwrap();
        assert_eq!(l.phase, Phase::Cancelling);
        assert_eq!(l.tick(STALL), Err(PlaybackLedgerError::Timeout));
    }
    #[test]
    fn ledger_finish_cancel_race_accepts_completion_but_preserves_host_cancel() {
        let mut l = ledger();
        start(&mut l);
        ready(&mut l);
        l.send(frame(&l), Duration::ZERO, |_, _| true).unwrap();
        l.send(
            AudioOutput::Finish {
                playback_id: l.id,
                target: l.target(),
                frame_count: 1,
            },
            Duration::ZERO,
            |_, _| true,
        )
        .unwrap();
        l.send(
            AudioOutput::Cancel {
                playback_id: l.id,
                target: l.target(),
            },
            Duration::ZERO,
            |_, _| true,
        )
        .unwrap();
        l.receive(
            &l.route.clone(),
            &AudioPlaybackReceipt::Finished {
                playback_id: l.id,
                frame_count: 1,
                outcome: AudioPlaybackOutcome::Completed,
            },
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(l.outcome(), Some(AudioPlaybackOutcome::Cancelled));
    }
    #[test]
    fn ledger_cancellation_does_not_hide_duplicate_ready() {
        for startup in [true, false] {
            let mut l = ledger();
            start(&mut l);
            if !startup {
                ready(&mut l);
            }
            l.send(
                AudioOutput::Cancel {
                    playback_id: l.id,
                    target: l.target(),
                },
                Duration::ZERO,
                |_, _| true,
            )
            .unwrap();
            let receipt = AudioPlaybackReceipt::Ready { playback_id: l.id };
            if startup {
                l.receive(&l.route.clone(), &receipt, Duration::ZERO)
                    .unwrap();
            }
            assert_eq!(
                l.receive(&l.route.clone(), &receipt, Duration::ZERO),
                Err(PlaybackLedgerError::InvalidFeedback)
            );
        }
    }
    #[test]
    fn ledger_cancelled_tail_cannot_become_completed() {
        let mut l = ledger();
        start(&mut l);
        ready(&mut l);
        l.send(frame(&l), Duration::ZERO, |_, _| true).unwrap();
        l.send(
            AudioOutput::Cancel {
                playback_id: l.id,
                target: l.target(),
            },
            Duration::ZERO,
            |_, _| true,
        )
        .unwrap();
        assert_eq!(
            l.receive(
                &l.route.clone(),
                &AudioPlaybackReceipt::Finished {
                    playback_id: l.id,
                    frame_count: 1,
                    outcome: AudioPlaybackOutcome::Completed
                },
                Duration::ZERO
            ),
            Err(PlaybackLedgerError::InvalidFeedback)
        );
    }
    #[test]
    fn ledger_idle_source_does_not_blame_worker_and_next_frame_restarts_deadline() {
        let mut l = ledger();
        start(&mut l);
        ready(&mut l);
        l.send(frame(&l), Duration::ZERO, |_, _| true).unwrap();
        l.receive(
            &l.route.clone(),
            &AudioPlaybackReceipt::Consumed {
                playback_id: l.id,
                frame_count: 1,
            },
            Duration::ZERO,
        )
        .unwrap();
        l.tick(Duration::from_secs(6)).unwrap();
        assert_eq!(l.outcome(), None);
        l.send(frame(&l), Duration::from_secs(6), |_, _| true)
            .unwrap();
        l.tick(Duration::from_secs(10)).unwrap();
        assert_eq!(
            l.tick(Duration::from_secs(11)),
            Err(PlaybackLedgerError::Timeout)
        );
    }
    #[test]
    fn ledger_stall_clock_and_absolute_deadlines_are_bounded() {
        let mut l = ledger();
        start(&mut l);
        assert_eq!(l.tick(STALL), Err(PlaybackLedgerError::Timeout));
        let mut l = ledger();
        l.tick(Duration::from_secs(1)).unwrap();
        assert_eq!(l.tick(Duration::ZERO), Err(PlaybackLedgerError::Clock));
        let mut l = ledger();
        start(&mut l);
        ready(&mut l);
        for seconds in (4..22 * 60).step_by(4) {
            let now = Duration::from_secs(seconds);
            l.send(frame(&l), now, |_, _| true).unwrap();
            l.receive(
                &l.route.clone(),
                &AudioPlaybackReceipt::Consumed {
                    playback_id: l.id,
                    frame_count: l.sent,
                },
                now,
            )
            .unwrap();
        }
        assert_eq!(l.tick(LIFETIME), Err(PlaybackLedgerError::Timeout));
    }
}
