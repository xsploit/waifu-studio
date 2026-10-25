//! Bounded host-owned audio turns. No threads, provider calls or conversation writes.
use crate::{
    discord_audio::Utterance,
    extension_protocol::{AudioInput, ErrorCode, IntegrationOrigin, Target},
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use uuid::Uuid;

const MAX_TURNS: usize = 4;
const MAX_RECENT: usize = 64;
const TURN_TTL: Duration = Duration::from_secs(35);
const RECENT_TTL: Duration = Duration::from_secs(60);
type Failure = (ErrorCode, String);

/// All fields come from host authority, never the audio payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioRoute {
    pub session: Uuid,
    pub process_generation: u64,
    pub target: Target,
    pub origin: IntegrationOrigin,
    pub revision: Uuid,
}

struct Turn {
    route: AudioRoute,
    id: String,
    participant_id: String,
    participant_name: String,
    started: Instant,
    audio: Utterance,
}

/// Pending worker input; this is not admitted chat or authored-user text.
pub struct ReadyAudio {
    pub route: AudioRoute,
    pub turn_id: String,
    pub participant_id: String,
    pub participant_name: String,
    pub wav: Vec<u8>,
}

#[derive(Default)]
pub struct AudioTurns {
    turns: BTreeMap<Uuid, Turn>,
    recent: Vec<(Uuid, u64, String, Instant)>,
}

fn error(code: ErrorCode, message: &str) -> Failure {
    (code, message.into())
}

impl AudioTurns {
    /// Run every frame, even when no audio record arrives. Removed authority,
    /// including mute/reconfiguration, must be absent from `allowed`.
    pub fn reconcile(&mut self, allowed: &[AudioRoute], now: Instant) {
        self.turns.retain(|_, turn| {
            allowed.contains(&turn.route) && now.saturating_duration_since(turn.started) < TURN_TTL
        });
        self.recent
            .retain(|entry| now.saturating_duration_since(entry.3) < RECENT_TTL);
    }

    pub fn accept(
        &mut self,
        route: &AudioRoute,
        input: &AudioInput,
        now: Instant,
    ) -> Result<Option<ReadyAudio>, Failure> {
        input
            .validate()
            .map_err(|_| error(ErrorCode::InvalidRequest, "Invalid audio record"))?;
        self.turns
            .retain(|_, turn| now.saturating_duration_since(turn.started) < TURN_TTL);
        self.recent
            .retain(|entry| now.saturating_duration_since(entry.3) < RECENT_TTL);
        if let AudioInput::Start {
            turn_id,
            participant_id,
            participant_name,
        } = input
        {
            if self.recent.iter().any(|entry| {
                entry.0 == route.session
                    && entry.1 == route.process_generation
                    && entry.2 == *turn_id
            }) {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "Audio turn was already started",
                ));
            }
            if self.turns.contains_key(&route.session)
                || self.turns.len() >= MAX_TURNS
                || self.recent.len() >= MAX_RECENT
            {
                return Err(error(ErrorCode::Busy, "Audio turn budget is full"));
            }
            self.recent.push((
                route.session,
                route.process_generation,
                turn_id.clone(),
                now,
            ));
            self.turns.insert(
                route.session,
                Turn {
                    route: route.clone(),
                    id: turn_id.clone(),
                    participant_id: participant_id.clone(),
                    participant_name: participant_name.clone(),
                    started: now,
                    // The containing turn owns the participant identity; frames
                    // cannot supply or replace it. Zero is its private collector token.
                    audio: Utterance::new(route.target.generation, 0),
                },
            );
            return Ok(None);
        }
        let id = match input {
            AudioInput::Frame { turn_id, .. }
            | AudioInput::Finish { turn_id }
            | AudioInput::Cancel { turn_id } => turn_id,
            AudioInput::Start { .. } => unreachable!(),
        };
        let turn = self
            .turns
            .get(&route.session)
            .ok_or_else(|| error(ErrorCode::Unavailable, "Audio turn is not active"))?;
        if turn.route != *route || turn.id != *id {
            return Err(error(ErrorCode::StaleTarget, "Audio route or turn changed"));
        }
        match input {
            AudioInput::Frame {
                frame_sequence,
                pcm,
                ..
            } => {
                let turn = self.turns.get_mut(&route.session).unwrap();
                if turn
                    .audio
                    .push(route.target.generation, 0, *frame_sequence, pcm)
                    .is_err()
                {
                    self.turns.remove(&route.session);
                    return Err(error(
                        ErrorCode::InvalidRequest,
                        "Audio sequence failed; turn discarded",
                    ));
                }
                Ok(None)
            }
            AudioInput::Finish { .. } => {
                let turn = self.turns.remove(&route.session).unwrap();
                let wav = turn.audio.finish(route.target.generation).map_err(|_| {
                    error(
                        ErrorCode::InvalidRequest,
                        "Audio turn is empty or cancelled",
                    )
                })?;
                Ok(Some(ReadyAudio {
                    route: turn.route,
                    turn_id: turn.id,
                    participant_id: turn.participant_id,
                    participant_name: turn.participant_name,
                    wav,
                }))
            }
            AudioInput::Cancel { .. } => {
                self.turns.remove(&route.session);
                Ok(None)
            }
            AudioInput::Start { .. } => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn route() -> AudioRoute {
        AudioRoute {
            session: Uuid::new_v4(),
            process_generation: 1,
            target: Target {
                uuid: Uuid::new_v4(),
                generation: 1,
            },
            origin: IntegrationOrigin {
                integration_id: "fixture".into(),
                display_name: "Fixture".into(),
            },
            revision: Uuid::new_v4(),
        }
    }
    fn start(id: &str) -> AudioInput {
        AudioInput::Start {
            turn_id: id.into(),
            participant_id: "participant".into(),
            participant_name: "Speaker".into(),
        }
    }
    fn frame(id: &str, seq: u64) -> AudioInput {
        AudioInput::Frame {
            turn_id: id.into(),
            frame_sequence: seq,
            pcm: vec![1; 320],
        }
    }
    fn finish(id: &str) -> AudioInput {
        AudioInput::Finish { turn_id: id.into() }
    }

    #[test]
    fn completed_audio_retains_host_route_and_participant() {
        let mut turns = AudioTurns::default();
        let route = route();
        let now = Instant::now();
        turns.accept(&route, &start("turn"), now).unwrap();
        turns.accept(&route, &frame("turn", 0), now).unwrap();
        let ready = turns.accept(&route, &finish("turn"), now).unwrap().unwrap();
        assert_eq!(ready.route, route);
        assert_eq!(ready.participant_id, "participant");
        assert_eq!(ready.participant_name, "Speaker");
        assert_eq!(ready.wav.len(), 684);
        assert!(turns.accept(&route, &start("turn"), now).is_err());
        assert!(turns.accept(&route, &finish("turn"), now).is_err());
    }
    #[test]
    fn sequence_failure_discards_whole_turn() {
        let mut turns = AudioTurns::default();
        let route = route();
        let now = Instant::now();
        turns.accept(&route, &start("turn"), now).unwrap();
        turns.accept(&route, &frame("turn", 0), now).unwrap();
        assert!(turns.accept(&route, &frame("turn", 2), now).is_err());
        assert!(turns.accept(&route, &finish("turn"), now).is_err());
    }
    #[test]
    fn revocation_route_change_and_timeout_remove_authority() {
        for case in 0..4 {
            let mut turns = AudioTurns::default();
            let route = route();
            let now = Instant::now();
            turns.accept(&route, &start("turn"), now).unwrap();
            let mut changed = route.clone();
            match case {
                0 => changed.revision = Uuid::new_v4(),
                1 => changed.target.generation += 1,
                2 => changed.process_generation += 1,
                _ => {}
            }
            let later = if case == 3 { now + TURN_TTL } else { now };
            turns.reconcile(&[changed], later);
            assert!(turns.accept(&route, &finish("turn"), later).is_err());
        }
    }
    #[test]
    fn global_and_source_budgets_do_not_evict_active_turns() {
        let mut turns = AudioTurns::default();
        let now = Instant::now();
        let first = route();
        turns.accept(&first, &start("first"), now).unwrap();
        assert_eq!(
            turns.accept(&first, &start("second"), now).err().unwrap().0,
            ErrorCode::Busy
        );
        for _ in 1..MAX_TURNS {
            turns.accept(&route(), &start("turn"), now).unwrap();
        }
        assert_eq!(
            turns.accept(&route(), &start("turn"), now).err().unwrap().0,
            ErrorCode::Busy
        );
        turns.accept(&first, &frame("first", 0), now).unwrap();
        assert!(
            turns
                .accept(&first, &finish("first"), now)
                .unwrap()
                .is_some()
        );
        turns.reconcile(&[], now);
        assert!(turns.turns.is_empty());
    }

    #[test]
    fn cancelled_turns_cannot_overflow_or_evict_replay_budget() {
        let mut turns = AudioTurns::default();
        let route = route();
        let now = Instant::now();
        for index in 0..MAX_RECENT {
            let id = format!("turn-{index}");
            turns.accept(&route, &start(&id), now).unwrap();
            turns
                .accept(&route, &AudioInput::Cancel { turn_id: id }, now)
                .unwrap();
        }
        assert_eq!(turns.recent.len(), MAX_RECENT);
        assert_eq!(
            turns.accept(&route, &start("new"), now).err().unwrap().0,
            ErrorCode::Busy
        );
        assert_eq!(
            turns.accept(&route, &start("turn-0"), now).err().unwrap().0,
            ErrorCode::InvalidRequest
        );
        turns
            .accept(&route, &start("new"), now + RECENT_TTL)
            .unwrap();
        assert_eq!(turns.recent.len(), 1);
    }
}
