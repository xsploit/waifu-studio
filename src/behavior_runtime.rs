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
    extension_manifest::Emotion,
    extensions::ExtensionState,
    loading::AvatarLoadState,
    reply_affect::{AffectContribution, AffectEmotion, AffectExpressionMapping, AffectLibrary},
};

// Keep the wire manifest enum independent of affect's persisted representation.
fn affect_emotion(emotion: Emotion) -> AffectEmotion {
    match emotion {
        Emotion::Neutral => AffectEmotion::Neutral,
        Emotion::Happy => AffectEmotion::Happy,
        Emotion::Sad => AffectEmotion::Sad,
        Emotion::Angry => AffectEmotion::Angry,
        Emotion::Relaxed => AffectEmotion::Relaxed,
        Emotion::Surprised => AffectEmotion::Surprised,
        Emotion::Amused => AffectEmotion::Amused,
        Emotion::Excited => AffectEmotion::Excited,
        Emotion::Curious => AffectEmotion::Curious,
        Emotion::Confused => AffectEmotion::Confused,
        Emotion::Thinking => AffectEmotion::Thinking,
        Emotion::Annoyed => AffectEmotion::Annoyed,
        Emotion::Embarrassed => AffectEmotion::Embarrassed,
        Emotion::Grateful => AffectEmotion::Grateful,
        Emotion::Optimistic => AffectEmotion::Optimistic,
        Emotion::Proud => AffectEmotion::Proud,
        Emotion::Nervous => AffectEmotion::Nervous,
        Emotion::Caring => AffectEmotion::Caring,
        Emotion::Fear => AffectEmotion::Fear,
        Emotion::Disgust => AffectEmotion::Disgust,
    }
}

fn expression_mappings(extensions: Option<&ExtensionState>) -> Vec<AffectExpressionMapping<'_>> {
    let mut entries: Vec<_> = extensions
        .into_iter()
        .flat_map(ExtensionState::active)
        .flat_map(|pack| {
            pack.manifest
                .content
                .expressions
                .iter()
                .map(move |entry| (pack.manifest.id.as_str(), entry))
        })
        .collect();
    // Installation order and manifest array order do not define precedence.
    // The first usable mapping by package ID, then local content ID wins.
    entries.sort_by(|(pack_a, a), (pack_b, b)| (pack_a, &a.id).cmp(&(pack_b, &b.id)));
    entries
        .into_iter()
        .map(|(_, entry)| AffectExpressionMapping {
            emotion: affect_emotion(entry.emotion),
            expression: &entry.expression,
            strength: entry.strength,
        })
        .collect()
}

#[derive(Clone, Copy, Default)]
struct Snapshot {
    character: Option<Uuid>,
    avatar: Option<Entity>,
    job: Option<Uuid>,
    completed: Option<(Uuid, bool)>,
    audio: Option<Entity>,
    speech_pending: bool,
}
#[derive(Default)]
pub struct BehaviorBridge {
    initialized: bool,
    character: Option<Uuid>,
    avatar: Option<Entity>,
    config: Option<BehaviorConfig>,
    available: Vec<String>,
    job: Option<Uuid>,
    completed: Option<Uuid>,
    audio: Option<Entity>,
    owned: Option<BehaviorToken>,
    superseded_by_ui: bool,
    extension_generation: Option<u64>,
    pack_contribution: Option<AffectContribution>,
}
impl BehaviorBridge {
    fn revoke_pack(&mut self, controller: &mut BehaviorController) {
        if let Some(previous) = self.pack_contribution.take() {
            controller.cancel_emotion(&previous.contribution);
        }
    }

    fn update_affect(
        &mut self,
        affect: &mut AffectLibrary,
        id: Uuid,
        controller: &mut BehaviorController,
        available: &[String],
        extensions: Option<&ExtensionState>,
    ) -> Result<(), String> {
        let generation = extensions.map(|extensions| extensions.generation);
        let changed_extensions = self.extension_generation != generation;
        self.extension_generation = generation;
        let pending = affect.take_pending(id);
        if !pending {
            if !changed_extensions {
                return Ok(());
            }
            let Some(previous) = self.pack_contribution.take() else {
                return Ok(());
            };
            let selected =
                affect.expression_with_mappings(id, available, expression_mappings(extensions));
            if selected
                .as_ref()
                .is_ok_and(|selection| selection.as_ref() == Some(&previous.selection))
            {
                self.pack_contribution = Some(previous);
                return Ok(());
            }
            let may_recompute = self.owned == controller.token()
                && self.owned.is_some()
                && controller.is_current_emotion(&previous.contribution);
            let revoked = controller.cancel_emotion(&previous.contribution);
            selected?;
            if !may_recompute || !revoked {
                return Ok(());
            }
        } else {
            self.revoke_pack(controller);
        }
        if let Some(token) = self.owned {
            self.pack_contribution = affect
                .drive_with_mappings_tracked(
                    id,
                    controller,
                    token,
                    available,
                    expression_mappings(extensions),
                )?
                .filter(|driven| driven.selection.mapped);
        }
        Ok(())
    }

    fn configure(
        &mut self,
        state: &mut BehaviorState,
        config: &BehaviorConfig,
        available: &[String],
        frame: Snapshot,
    ) {
        let changed_character = !self.initialized || self.character != frame.character;
        if changed_character || self.avatar != frame.avatar || self.available != available {
            // Do not replay a previous avatar's emotion, even if the replacement
            // happens to expose the same expression names and character ID.
            self.revoke_pack(&mut state.controller);
        }
        self.avatar = frame.avatar;
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

#[derive(bevy::ecs::system::SystemParam)]
pub struct BehaviorInputs<'w> {
    extensions: Option<Res<'w, ExtensionState>>,
    avatar: Option<Res<'w, AvatarLoadState>>,
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
    inputs: BehaviorInputs,
) {
    let frame = Snapshot {
        character: studio.library.selected,
        avatar: inputs.avatar.as_ref().and_then(|avatar| avatar.active),
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
        if let Err(error) = bridge.update_affect(
            &mut studio.affect,
            id,
            &mut state.controller,
            &facial.available,
            inputs.extensions.as_deref(),
        ) {
            state.error = Some(error);
        }
    }
    studio.affect.tick(time.delta_secs_f64());
    state.controller.tick(time.delta_secs());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached_expression_pack(
        package_id: &str,
        entries: &[(&str, &str)],
    ) -> waifu_studio::extensions::PackView {
        use waifu_studio::{
            extension_manifest::{Content, ExpressionContent, Manifest, Provenance, ResolvedPack},
            extension_store::InstalledPack,
            extensions::PackView,
        };
        PackView {
            credentials: Default::default(),
            data_directory: None,
            installed: InstalledPack {
                installation_id: Uuid::new_v4(),
                revision: Uuid::new_v4(),
                package_id: package_id.into(),
                name: package_id.into(),
                version: "1.0.0".into(),
                enabled: true,
                executable: false,
                grants: vec![],
                settings: Default::default(),
            },
            content: Some(ResolvedPack {
                manifest: Manifest {
                    test_process: None,
                    credentials: vec![],
                    settings: vec![],
                    api_version: 1,
                    id: package_id.into(),
                    version: "1.0.0".into(),
                    name: package_id.into(),
                    description: String::new(),
                    provenance: Provenance {
                        author: "Test".into(),
                        license: "MIT".into(),
                        source: None,
                    },
                    content: Content {
                        expressions: entries
                            .iter()
                            .map(|(id, expression)| ExpressionContent {
                                id: (*id).into(),
                                name: (*id).into(),
                                emotion: Emotion::Happy,
                                expression: (*expression).into(),
                                strength: 0.5,
                            })
                            .collect(),
                        ..Default::default()
                    },
                    process: None,
                    hooks: vec![],
                    capabilities: vec![],
                },
                // No assets are loaded by behavior; these are cached manifest fixtures.
                root: "unused-cached-pack".into(),
                content: vec![],
            }),
            error: None,
        }
    }

    fn pack_affect_app() -> (App, Uuid, Vec<String>, std::path::PathBuf) {
        use waifu_studio::data::{Character, Store};
        let directory = std::env::temp_dir().join(format!("pack-affect-{}", Uuid::new_v4()));
        let (store, mut library) = Store::open(directory.clone()).unwrap();
        let id = Uuid::new_v4();
        library.characters.push(Character {
            delegation: Default::default(),
            id,
            name: "Focused".into(),
            model: "unused.vrm".into(),
            persona: String::new(),
            format: "VRM".into(),
            greeting: String::new(),
            scene: None,
            provider: None,
            voice: None,
        });
        library.selected = Some(id);
        library.scene.behavior.enabled = true;
        library.scene.affect.enabled = true;
        let studio = Studio::new(store, library);
        let mut extensions = ExtensionState::new(directory.join("extensions"));
        // Let the constructor finish its local catalog read before substituting
        // cached fixtures, so cleanup cannot race the worker's initialization.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while extensions.busy && std::time::Instant::now() < deadline {
            extensions.poll();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!extensions.busy);
        assert!(extensions.error.is_none());
        let mut disabled = cached_expression_pack("org.aaa", &[("smile", "Disabled")]);
        disabled.installed.enabled = false;
        let mut errored = cached_expression_pack("org.aab", &[("smile", "Errored")]);
        errored.error = Some("Invalid pack".into());
        let mut unloaded = cached_expression_pack("org.aac", &[("smile", "Unloaded")]);
        unloaded.content = None;
        extensions.packs = vec![
            cached_expression_pack("org.z", &[("smile", "Late")]),
            disabled,
            errored,
            unloaded,
            cached_expression_pack(
                "org.b",
                &[
                    ("z-last", "Later"),
                    ("b-first", "Smile"),
                    ("a-unavailable", "Missing"),
                ],
            ),
        ];
        let available = [
            "happy", "Smile", "Later", "Late", "Disabled", "Errored", "Unloaded",
        ]
        .map(String::from)
        .to_vec();
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .insert_resource(studio)
            .insert_resource(extensions)
            .init_resource::<FacialState>()
            .init_resource::<BehaviorState>()
            .add_systems(Update, poll_behavior);
        app.world_mut().resource_mut::<FacialState>().available = available.clone();
        // The bridge ignores audio already playing when a character is selected.
        app.update();
        app.world_mut().spawn(SpokenAudio);
        (app, id, available, directory)
    }

    fn apply_happy(app: &mut App, id: Uuid) {
        let mut studio = app.world_mut().resource_mut::<Studio>();
        let config = studio.library.scene.affect.clone();
        studio.affect.entries.entry(id).or_default().config = config;
        studio
            .affect
            .apply_reply(
                id,
                waifu_studio::reply_affect::ReplyAffect {
                    emotion: AffectEmotion::Happy,
                    valence: 0.8,
                    arousal: 0.4,
                    dominance: 0.0,
                },
            )
            .unwrap();
    }

    fn weights(app: &App, id: Uuid) -> std::collections::BTreeMap<String, f32> {
        let available = &app.world().resource::<FacialState>().available;
        app.world()
            .resource::<BehaviorState>()
            .controller
            .weights_for(Some(id), available, &Default::default(), "")
    }

    #[test]
    fn poll_consumes_only_active_cached_mappings_and_disable_restores_default_on_next_affect() {
        let (mut app, id, available, directory) = pack_affect_app();

        for (disable, expected) in [
            (None, "Smile"),
            (Some("org.b"), "Late"),
            (Some("org.z"), "happy"),
        ] {
            if let Some(package_id) = disable {
                let mut extensions = app.world_mut().resource_mut::<ExtensionState>();
                extensions
                    .packs
                    .iter_mut()
                    .find(|pack| pack.installed.package_id == package_id)
                    .unwrap()
                    .installed
                    .enabled = false;
                extensions.generation += 1;
            }
            apply_happy(&mut app, id);
            app.update();
            let mut state = app.world_mut().resource_mut::<BehaviorState>();
            assert!(state.error.is_none(), "{:?}", state.error);
            state.controller.tick(0.2);
            let weights =
                state
                    .controller
                    .weights_for(Some(id), &available, &Default::default(), "");
            assert_eq!(weights.len(), 1, "{weights:?}");
            assert!(weights[expected] > 0.0);
            assert!(
                state
                    .controller
                    .weights_for(
                        Some(id),
                        &available,
                        &std::collections::BTreeMap::from([("Smile".into(), 0.7)]),
                        "",
                    )
                    .is_empty()
            );
            // The next assertion observes a new one-shot decision without the
            // previous channel's smoothing tail; the bridge retains its epoch.
            state.controller.tick(60.0);
        }
        drop(app);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn disabling_active_mapping_revokes_immediately_without_pending_affect() {
        for fallback in ["alias", "neutral", "missing"] {
            let (mut app, id, _, directory) = pack_affect_app();
            apply_happy(&mut app, id);
            if fallback != "alias" {
                let (affect_emotion, manifest_emotion) = if fallback == "neutral" {
                    (AffectEmotion::Neutral, Emotion::Neutral)
                } else {
                    (AffectEmotion::Fear, Emotion::Fear)
                };
                app.world_mut()
                    .resource_mut::<Studio>()
                    .affect
                    .entries
                    .get_mut(&id)
                    .unwrap()
                    .emotion = affect_emotion;
                for pack in &mut app.world_mut().resource_mut::<ExtensionState>().packs {
                    if let Some(content) = &mut pack.content {
                        for entry in &mut content.manifest.content.expressions {
                            entry.emotion = manifest_emotion;
                        }
                    }
                }
            }
            app.update();
            app.world_mut()
                .resource_mut::<BehaviorState>()
                .controller
                .tick(0.2);
            assert!(weights(&app, id)["Smile"] > 0.0);
            assert!(
                !app.world_mut()
                    .resource_mut::<Studio>()
                    .affect
                    .take_pending(id)
            );
            let token = app.world().resource::<BehaviorState>().controller.token();
            {
                let mut extensions = app.world_mut().resource_mut::<ExtensionState>();
                for pack in &mut extensions.packs {
                    pack.installed.enabled = false;
                }
                extensions.generation += 1;
            }
            app.update();
            assert!(
                !weights(&app, id).contains_key("Smile"),
                "revocation must remove the smoothing tail in this frame"
            );
            assert_eq!(
                app.world().resource::<BehaviorState>().controller.token(),
                token
            );
            app.world_mut()
                .resource_mut::<BehaviorState>()
                .controller
                .tick(0.2);
            if fallback == "alias" {
                assert!(weights(&app, id)["happy"] > 0.0);
            } else {
                assert!(weights(&app, id).is_empty());
            }
            drop(app);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn unchanged_refresh_does_not_restart_drive_but_mapping_update_replaces_it() {
        let (mut app, id, _, directory) = pack_affect_app();
        apply_happy(&mut app, id);
        app.update();
        let mut reference = BehaviorController::default();
        {
            let studio = app.world().resource::<Studio>();
            let available = &app.world().resource::<FacialState>().available;
            reference
                .select(Some(id), &studio.library.scene.behavior, available)
                .unwrap();
            let token = reference.begin_turn(id).unwrap();
            studio
                .affect
                .drive_with_mappings(
                    id,
                    &mut reference,
                    token,
                    available,
                    expression_mappings(Some(app.world().resource::<ExtensionState>())),
                )
                .unwrap();
        }
        reference.tick(0.2);
        app.world_mut()
            .resource_mut::<BehaviorState>()
            .controller
            .tick(0.2);
        let before = weights(&app, id);
        // Decaying mood must not enter the mapping identity comparison.
        app.world_mut().resource_mut::<Studio>().affect.tick(30.0);
        app.world_mut().resource_mut::<ExtensionState>().generation += 1;
        app.update();
        assert_eq!(weights(&app, id), before);
        reference.tick(0.2);
        app.world_mut()
            .resource_mut::<BehaviorState>()
            .controller
            .tick(0.2);
        assert_eq!(
            weights(&app, id),
            reference.weights_for(
                Some(id),
                &app.world().resource::<FacialState>().available,
                &Default::default(),
                ""
            )
        );
        {
            let mut extensions = app.world_mut().resource_mut::<ExtensionState>();
            let entry = extensions
                .packs
                .iter_mut()
                .find(|pack| pack.installed.package_id == "org.b")
                .unwrap()
                .content
                .as_mut()
                .unwrap()
                .manifest
                .content
                .expressions
                .iter_mut()
                .find(|entry| entry.id == "b-first")
                .unwrap();
            entry.expression = "Later".into();
            extensions.generation += 1;
        }
        app.update();
        assert!(!weights(&app, id).contains_key("Smile"));
        app.world_mut()
            .resource_mut::<BehaviorState>()
            .controller
            .tick(0.2);
        assert!(weights(&app, id)["Later"] > 0.0);
        drop(app);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn disabling_pack_does_not_replace_a_later_manual_preview() {
        let (mut app, id, _, directory) = pack_affect_app();
        apply_happy(&mut app, id);
        app.update();
        {
            let mut state = app.world_mut().resource_mut::<BehaviorState>();
            state.controller.tick(0.2);
            let token = state.controller.token().unwrap();
            state.controller.emotion(token, "Smile", 0.9).unwrap();
            state.controller.tick(0.2);
        }
        let before = weights(&app, id);
        {
            let mut extensions = app.world_mut().resource_mut::<ExtensionState>();
            for pack in &mut extensions.packs {
                pack.installed.enabled = false;
            }
            extensions.generation += 1;
        }
        app.update();
        assert_eq!(weights(&app, id), before);
        app.world_mut()
            .resource_mut::<BehaviorState>()
            .controller
            .tick(0.2);
        assert!(!weights(&app, id).contains_key("happy"));
        drop(app);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn changed_avatar_or_catalog_does_not_replay_stale_pack_affect() {
        for change_avatar in [false, true] {
            let (mut app, id, _, directory) = pack_affect_app();
            app.init_resource::<AvatarLoadState>();
            apply_happy(&mut app, id);
            app.update();
            app.world_mut()
                .resource_mut::<BehaviorState>()
                .controller
                .tick(0.2);
            assert!(weights(&app, id)["Smile"] > 0.0);
            if change_avatar {
                let replacement = app.world_mut().spawn_empty().id();
                app.world_mut().resource_mut::<AvatarLoadState>().active = Some(replacement);
            } else {
                app.world_mut()
                    .resource_mut::<FacialState>()
                    .available
                    .push("NewAvatarExpression".into());
            }
            app.world_mut().resource_mut::<ExtensionState>().generation += 1;
            app.update();
            assert!(weights(&app, id).is_empty());
            app.world_mut().resource_mut::<ExtensionState>().generation += 1;
            app.update();
            app.world_mut()
                .resource_mut::<BehaviorState>()
                .controller
                .tick(0.2);
            assert!(weights(&app, id).is_empty());
            drop(app);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

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
