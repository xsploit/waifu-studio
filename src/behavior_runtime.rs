//! Lifecycle bridge for opt-in typed reply metadata or Jev character-reaction appraisal.
use crate::studio::{SpokenAudio, Studio};
use bevy::prelude::*;
use uuid::Uuid;
use waifu_studio::{
    behavior::{
        BehaviorConfig, BehaviorController, BehaviorEvent, BehaviorPhase, BehaviorState,
        BehaviorToken,
    },
    expressions::FacialState,
};

#[derive(Clone, Copy, Default)]
struct Snapshot {
    character: Option<Uuid>,
    job: Option<Uuid>,
    completed: Option<(Uuid, bool)>,
    audio: Option<Entity>,
    speech_pending: bool,
}
#[derive(Default)]
pub struct BehaviorBridge {
    initialized: bool,
    character: Option<Uuid>,
    config: Option<BehaviorConfig>,
    available: Vec<String>,
    job: Option<Uuid>,
    completed: Option<Uuid>,
    audio: Option<Entity>,
    owned: Option<BehaviorToken>,
    superseded_by_ui: bool,
}
impl BehaviorBridge {
    fn configure(
        &mut self,
        state: &mut BehaviorState,
        config: &BehaviorConfig,
        available: &[String],
        frame: Snapshot,
    ) {
        let changed_character = !self.initialized || self.character != frame.character;
        let was_enabled = state.controller.enabled();
        let newly_enabled = self
            .config
            .as_ref()
            .is_some_and(|old| !old.enabled && config.enabled);
        if changed_character || self.config.as_ref() != Some(config) || self.available != available
        {
            state.error = state
                .controller
                .select(frame.character, config, available)
                .err();
            self.config = Some(config.clone());
            self.available = available.to_vec();
            self.character = frame.character;
        }
        let recovered = !was_enabled && state.controller.enabled();
        if changed_character || newly_enabled || recovered {
            self.owned = None;
            self.superseded_by_ui = false;
            self.job = None;
            // A previous character's completed response remains in Studio until
            // another send. Never replay it as a new turn for this character.
            self.completed = frame.completed.map(|(id, _)| id);
            self.audio = if changed_character { frame.audio } else { None };
        }
        self.initialized = true;
    }
    fn begin(&mut self, controller: &mut BehaviorController, character: Option<Uuid>) {
        self.owned = character.and_then(|id| controller.begin_turn(id).ok());
    }
    fn interrupt(&mut self, controller: &mut BehaviorController) {
        if let Some(token) = self.owned.take() {
            let _ = controller.event(token, BehaviorEvent::Interrupted);
        }
    }
    fn finish_thinking(&self, controller: &mut BehaviorController) {
        if controller.phase() == BehaviorPhase::Thinking
            && let Some(token) = self.owned
        {
            let _ = controller.event(token, BehaviorEvent::ThinkingFinished);
        }
    }
    fn observe(&mut self, controller: &mut BehaviorController, frame: Snapshot) {
        // A UI reset/interrupt advances the epoch. Never adopt its fresh token
        // to continue a job this bridge no longer owns.
        if self.owned.is_some() && controller.token() != self.owned {
            self.owned = None;
            self.superseded_by_ui = true;
        }
        let old_job = self.job;
        if self.job != frame.job {
            if let Some(previous) = self.job {
                if frame.completed == Some((previous, true)) {
                    self.finish_thinking(controller);
                } else {
                    self.interrupt(controller);
                }
            }
            self.job = frame.job;
            if frame.job.is_some() {
                self.superseded_by_ui = false;
                self.begin(controller, frame.character);
            }
        }
        if let Some((id, success)) = frame.completed
            && self.completed != Some(id)
        {
            self.completed = Some(id);
            // A response can start and finish between bridge frames. Only adopt
            // that unseen completion if no newer generation currently exists.
            if frame.job.is_none() {
                if self.owned.is_none() && old_job != Some(id) && !self.superseded_by_ui {
                    self.begin(controller, frame.character);
                }
                if success {
                    self.finish_thinking(controller);
                } else {
                    self.interrupt(controller);
                }
            }
        }
        if self.audio != frame.audio {
            if frame.audio.is_some() {
                if self.owned.is_none() && !self.superseded_by_ui {
                    self.begin(controller, frame.character);
                }
                if let Some(token) = self.owned {
                    let _ = controller.event(token, BehaviorEvent::SpeechStarted);
                }
            }
            self.audio = frame.audio;
        }
        // Brief gaps while text/audio is still arriving belong to the same
        // reply. Ending the epoch here would discard its expression decision.
        if frame.audio.is_none()
            && !frame.speech_pending
            && controller.phase() == BehaviorPhase::Speaking
            && let Some(token) = self.owned.take()
        {
            let _ = controller.event(token, BehaviorEvent::SpeechFinished);
        }
    }
}

/// Root schedules this after poll_studio/poll_voice/orchestration in Update;
/// ExpressionsPlugin consumes its named contributions later in PostUpdate.
pub fn poll_behavior(
    mut studio: ResMut<Studio>,
    time: Res<Time>,
    facial: Res<FacialState>,
    mut state: ResMut<BehaviorState>,
    playing: Query<Entity, With<SpokenAudio>>,
    mut bridge: Local<BehaviorBridge>,
) {
    let frame = Snapshot {
        character: studio.library.selected,
        job: studio.job.as_ref().map(|job| job.id),
        completed: studio
            .completed_chat
            .as_ref()
            .map(|(id, result)| (*id, result.is_ok())),
        audio: playing.iter().next().filter(|_| {
            studio.speech_character.is_none() || studio.speech_character == studio.library.selected
        }),
        speech_pending: (studio.voice_busy || studio.job.is_some())
            && (studio.speech_character.is_none()
                || studio.speech_character == studio.library.selected),
    };
    bridge.configure(
        &mut state,
        &studio.library.scene.behavior,
        &facial.available,
        frame,
    );
    bridge.observe(&mut state.controller, frame);
    if let Some(id) = studio.library.selected {
        if studio
            .affect
            .entries
            .get(&id)
            .is_none_or(|entry| entry.config != studio.library.scene.affect)
        {
            let config = studio.library.scene.affect.clone();
            studio.affect.entries.entry(id).or_default().config = config;
        }
        if studio.affect.take_pending(id)
            && let Some(token) = bridge.owned
            && let Err(error) =
                studio
                    .affect
                    .drive(id, &mut state.controller, token, &facial.available)
        {
            state.error = Some(error);
        }
    }
    studio.affect.tick(time.delta_secs_f64());
    state.controller.tick(time.delta_secs());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn companion_audio_does_not_drive_focused_behavior() {
        use waifu_studio::data::{Character, Store};
        let directory = std::env::temp_dir().join(format!("scene-behavior-{}", Uuid::new_v4()));
        let (store, mut library) = Store::open(directory.clone()).unwrap();
        let focused = Uuid::new_v4();
        library.characters.push(Character {
            delegation: Default::default(),
            id: focused,
            name: "Focused".into(),
            model: "unused.vrm".into(),
            persona: String::new(),
            format: "VRM".into(),
            greeting: String::new(),
            scene: None,
            provider: None,
            voice: None,
        });
        library.selected = Some(focused);
        library.scene.behavior.enabled = true;
        let mut studio = Studio::new(store, library);
        studio.speech_character = Some(Uuid::new_v4());
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .insert_resource(studio)
            .init_resource::<FacialState>()
            .init_resource::<BehaviorState>()
            .add_systems(Update, poll_behavior);
        app.world_mut().spawn(SpokenAudio);
        app.update();
        assert_eq!(
            app.world().resource::<BehaviorState>().controller.phase(),
            BehaviorPhase::Idle
        );
        app.world_mut().resource_mut::<Studio>().speech_character = Some(focused);
        app.update();
        assert_eq!(
            app.world().resource::<BehaviorState>().controller.phase(),
            BehaviorPhase::Speaking
        );
        app.world_mut().resource_mut::<Studio>().speech_character = Some(Uuid::new_v4());
        app.update();
        assert_eq!(
            app.world().resource::<BehaviorState>().controller.phase(),
            BehaviorPhase::Idle
        );
        drop(app);
        std::fs::remove_dir_all(directory).unwrap();
    }
    fn setup() -> (BehaviorState, BehaviorBridge, BehaviorConfig, Snapshot) {
        let mut state = BehaviorState::default();
        let mut bridge = BehaviorBridge::default();
        let config = BehaviorConfig {
            enabled: true,
            ..Default::default()
        };
        let frame = Snapshot {
            character: Some(Uuid::new_v4()),
            ..Default::default()
        };
        bridge.configure(&mut state, &config, &["happy".into()], frame);
        (state, bridge, config, frame)
    }
    #[test]
    fn audio_gaps_preserve_reply_epoch_and_emotion_until_playback_finishes() {
        let (mut state, mut bridge, _, mut frame) = setup();
        frame.job = Some(Uuid::new_v4());
        frame.speech_pending = true;
        bridge.observe(&mut state.controller, frame);
        let token = state.controller.token().unwrap();
        state.controller.emotion(token, "happy", 0.8).unwrap();
        frame.audio = Some(Entity::PLACEHOLDER);
        bridge.observe(&mut state.controller, frame);
        frame.audio = None;
        bridge.observe(&mut state.controller, frame);
        assert_eq!(state.controller.token(), Some(token));
        assert_eq!(state.controller.phase(), BehaviorPhase::Speaking);
        frame.audio = Some(Entity::PLACEHOLDER);
        bridge.observe(&mut state.controller, frame);
        assert_eq!(state.controller.token(), Some(token));
        let job = frame.job.take().unwrap();
        frame.completed = Some((job, true));
        frame.audio = None;
        frame.speech_pending = false;
        bridge.observe(&mut state.controller, frame);
        assert_eq!(state.controller.phase(), BehaviorPhase::Idle);
        assert_ne!(state.controller.token(), Some(token));
    }
    #[test]
    fn generation_completion_actual_audio_and_end_follow_owned_epoch() {
        let (mut state, mut bridge, _, mut frame) = setup();
        let job = Uuid::new_v4();
        frame.job = Some(job);
        bridge.observe(&mut state.controller, frame);
        assert_eq!(state.controller.phase(), BehaviorPhase::Thinking);
        let token = bridge.owned.unwrap();
        frame.job = None;
        frame.completed = Some((job, true));
        bridge.observe(&mut state.controller, frame);
        assert_eq!(state.controller.phase(), BehaviorPhase::Idle);
        frame.audio = Some(Entity::PLACEHOLDER);
        bridge.observe(&mut state.controller, frame);
        assert_eq!(state.controller.phase(), BehaviorPhase::Speaking);
        assert_eq!(bridge.owned, Some(token));
        frame.audio = None;
        bridge.observe(&mut state.controller, frame);
        assert_eq!(state.controller.phase(), BehaviorPhase::Idle);
        assert!(
            state
                .controller
                .event(token, BehaviorEvent::SpeechStarted)
                .is_err()
        );
    }
    #[test]
    fn cancellation_failure_and_new_job_invalidate_old_owned_work() {
        let (mut state, mut bridge, _, mut frame) = setup();
        let first = Uuid::new_v4();
        frame.job = Some(first);
        bridge.observe(&mut state.controller, frame);
        let old = bridge.owned.unwrap();
        frame.job = None;
        bridge.observe(&mut state.controller, frame);
        assert_eq!(state.controller.phase(), BehaviorPhase::Idle);
        assert!(state.controller.emotion(old, "happy", 1.0).is_err());
        let second = Uuid::new_v4();
        frame.job = Some(second);
        bridge.observe(&mut state.controller, frame);
        let token = bridge.owned.unwrap();
        frame.job = None;
        frame.completed = Some((second, false));
        bridge.observe(&mut state.controller, frame);
        assert!(state.controller.emotion(token, "happy", 1.0).is_err());
        assert_eq!(state.controller.phase(), BehaviorPhase::Idle);
    }
    #[test]
    fn character_swap_drops_old_completion_and_manual_reset_is_not_reclaimed() {
        let (mut state, mut bridge, config, mut frame) = setup();
        frame.job = Some(Uuid::new_v4());
        bridge.observe(&mut state.controller, frame);
        let old = bridge.owned.unwrap();
        state.controller.reset();
        bridge.observe(&mut state.controller, frame);
        assert!(bridge.owned.is_none());
        assert_eq!(state.controller.phase(), BehaviorPhase::Idle);
        frame.completed = Some((frame.job.unwrap(), true));
        frame.job = None;
        bridge.observe(&mut state.controller, frame);
        assert!(
            bridge.owned.is_none(),
            "reset generation must not adopt a fresh epoch on completion"
        );
        frame.audio = Some(Entity::PLACEHOLDER);
        bridge.observe(&mut state.controller, frame);
        assert!(
            bridge.owned.is_none(),
            "late speech from reset work must not claim the UI's fresh epoch"
        );
        assert_eq!(state.controller.phase(), BehaviorPhase::Idle);
        frame.character = Some(Uuid::new_v4());
        bridge.configure(&mut state, &config, &["happy".into()], frame);
        bridge.observe(&mut state.controller, frame);
        assert!(bridge.owned.is_none());
        assert!(
            state
                .controller
                .event(old, BehaviorEvent::SpeechStarted)
                .is_err()
        );
    }
    #[test]
    fn streaming_audio_before_generation_completion_remains_speaking() {
        let (mut state, mut bridge, _, mut frame) = setup();
        let job = Uuid::new_v4();
        frame.job = Some(job);
        bridge.observe(&mut state.controller, frame);
        frame.audio = Some(Entity::PLACEHOLDER);
        bridge.observe(&mut state.controller, frame);
        assert_eq!(state.controller.phase(), BehaviorPhase::Speaking);
        frame.job = None;
        frame.completed = Some((job, true));
        bridge.observe(&mut state.controller, frame);
        assert_eq!(state.controller.phase(), BehaviorPhase::Speaking);
    }
}
