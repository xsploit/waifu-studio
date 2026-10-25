//! Frame adapter to the shared extension runtime; no UI-pass dependency.
#[path = "extension_audio_intake.rs"]
mod audio_intake;
use crate::{
    desktop_behavior::DesktopGrab,
    desktop_settings::DesktopSettings,
    incoming_chat::{ChatChunk, IncomingChat},
    studio::{Studio, ViewMode},
};
use audio_intake::AudioIntake;
#[cfg(all(test, target_os = "linux"))]
#[path = "extension_audio_installed_tests.rs"]
mod audio_installed_tests;
use bevy::prelude::*;
use std::collections::BTreeMap;
use uuid::Uuid;
use waifu_studio::{
    animation::AnimationState,
    behavior::BehaviorState,
    extension_actions::{ActionCatalog, ActionControllers, ExternalMessageSink},
    extension_protocol::{ChatMessage, ErrorCode, IntegrationOrigin, Target},
    extension_runtime::ExtensionRuntime,
    extensions::ExtensionState,
    loading::AvatarLoadState,
    memory::MemoryState,
    scene_chat::SceneChat,
};

#[derive(Default)]
struct ChatPickerState {
    pending: Option<PendingSelection>,
}
struct PendingSelection {
    chunk: ChatChunk,
    config: waifu_studio::recipient_decision::DecisionConfig,
    job: Option<waifu_studio::chat_decision::ChatDecisionJob>,
}
impl ChatPickerState {
    fn reconcile(
        &mut self,
        target: Option<Target>,
        allowed: &BTreeMap<String, Uuid>,
        config: &waifu_studio::recipient_decision::DecisionConfig,
        enabled: bool,
        now: std::time::Instant,
    ) {
        if let Some(pending) = &mut self.pending
            && (!enabled
                || &pending.config != config
                || Some(pending.chunk.target()) != target
                || allowed.get(&pending.chunk.origin().integration_id)
                    != Some(&pending.chunk.revision())
                || !pending.chunk.has_live(now))
        {
            self.pending = None;
        }
    }
}

struct StudioExternalMessages<'a> {
    outputs: Option<&'a mut waifu_studio::extension_speech_output::SpeechOutputs>,
    audio: Option<(
        &'a mut AudioIntake,
        &'a waifu_studio::listen::ListeningState,
    )>,
    studio: &'a mut Studio,
    memory: &'a mut MemoryState,
    scene_busy: bool,
    target: Option<Target>,
    incoming: &'a mut IncomingChat,
    picker: &'a mut ChatPickerState,
    allowed: &'a BTreeMap<String, Uuid>,
}
impl ExternalMessageSink for StudioExternalMessages<'_> {
    fn audio_playback_available(
        &mut self,
        target: Target,
        origin: &IntegrationOrigin,
        session: Uuid,
        process_generation: u64,
    ) -> Result<(), (ErrorCode, String)> {
        self.outputs
            .as_deref_mut()
            .ok_or((
                ErrorCode::Unavailable,
                "Speech output is unavailable".into(),
            ))?
            .receive_available(target, origin, session, process_generation)
    }

    fn audio_playback_receipt(
        &mut self,
        target: Target,
        origin: &IntegrationOrigin,
        session: Uuid,
        process_generation: u64,
        receipt: &waifu_studio::extension_protocol::AudioPlaybackReceipt,
    ) -> Result<(), (ErrorCode, String)> {
        let output = self.outputs.as_deref_mut().ok_or((
            ErrorCode::Unavailable,
            "Speech output is unavailable".into(),
        ))?;
        output.receive(
            target,
            origin,
            session,
            process_generation,
            receipt,
            std::time::Instant::now(),
        )
    }

    fn audio_input(
        &mut self,
        target: Target,
        origin: &IntegrationOrigin,
        session: Uuid,
        process_generation: u64,
        input: &waifu_studio::extension_protocol::AudioInput,
    ) -> Result<(), (ErrorCode, String)> {
        let revision = *self.allowed.get(&origin.integration_id).ok_or((
            ErrorCode::PermissionDenied,
            "Audio source is not enabled".into(),
        ))?;
        let Some((audio, listening)) = &mut self.audio else {
            return Err((ErrorCode::Unavailable, "Audio intake is unavailable".into()));
        };
        let route = waifu_studio::extension_audio::AudioRoute {
            session,
            process_generation,
            target,
            origin: origin.clone(),
            revision,
        };
        audio.accept(
            &route,
            input,
            &listening.config,
            &listening.session_key,
            std::time::Instant::now(),
        )
    }
    fn offer_chat(
        &mut self,
        target: Target,
        origin: &IntegrationOrigin,
        message: &ChatMessage,
    ) -> Result<(), (ErrorCode, String)> {
        if self.target != Some(target) {
            return Err((ErrorCode::StaleTarget, "Avatar changed".into()));
        }
        let revision = self.allowed.get(&origin.integration_id).ok_or_else(|| {
            (
                ErrorCode::PermissionDenied,
                "Chat source is not enabled".into(),
            )
        })?;
        self.incoming.offer(
            target,
            origin,
            *revision,
            message,
            std::time::Instant::now(),
        )
    }
    fn deliver(
        &mut self,
        target: Target,
        origin: &IntegrationOrigin,
        text: &str,
    ) -> Result<Uuid, (ErrorCode, String)> {
        if self.target != Some(target) {
            return Err((ErrorCode::StaleTarget, "Avatar changed".into()));
        }
        if self.scene_busy {
            return Err((ErrorCode::Busy, "Scene conversation is busy".into()));
        }
        let message_id = self.studio.deliver_external(target, origin, text)?;
        // Delivery has already committed. Generation failure must not turn its
        // acknowledgement into a failure that could encourage duplicate intake.
        if self.studio.library.scene.reply_to_external_messages
            && self
                .studio
                .respond_to_external(target, message_id, self.memory)
                .is_err()
        {
            self.studio.notice =
                Some("External message delivered, but its response could not start.".into());
        }
        Ok(message_id)
    }
}

impl StudioExternalMessages<'_> {
    fn flush_audio(&mut self, runtime: &ExtensionRuntime) {
        let completed = match &mut self.audio {
            Some((audio, _)) => audio.poll(),
            None => return,
        };
        let Some(completed) = completed else {
            return;
        };
        use waifu_studio::extension_protocol::AudioInputOutcome;
        let outcome = match completed.result {
            Ok(message) => {
                if self
                    .offer_chat(completed.route.target, &completed.route.origin, &message)
                    .is_ok()
                {
                    AudioInputOutcome::Queued
                } else {
                    self.studio.notice =
                        Some("Recognized speech could not enter incoming messages.".into());
                    AudioInputOutcome::Rejected
                }
            }
            Err(outcome) => {
                if outcome != AudioInputOutcome::Cancelled {
                    self.studio.notice = Some(
                        "Participant speech recognition failed or returned unsupported text."
                            .into(),
                    );
                }
                outcome
            }
        };
        runtime.publish_audio_completion(&completed.route, completed.turn_id, outcome);
    }
    fn flush_chat(&mut self, now: std::time::Instant) {
        let selecting = self.studio.library.scene.select_incoming_messages
            && self.studio.library.scene.reply_to_external_messages;
        self.picker.reconcile(
            self.target,
            self.allowed,
            &self.studio.library.decision,
            selecting,
            now,
        );
        if let Some(pending) = &mut self.picker.pending
            && let Some(job) = &pending.job
        {
            match job.poll() {
                None => return,
                Some(Ok(ids)) => {
                    if pending.chunk.select(&ids).is_err() || !pending.chunk.retain_live(now) {
                        self.picker.pending = None;
                        return;
                    }
                    pending.job = None;
                }
                Some(Err(_)) => {
                    self.picker.pending = None;
                    self.studio.notice = Some("Chat selection failed; this chunk was skipped. Check the decision model connection.".into());
                    return;
                }
            }
        }
        if self.scene_busy || self.studio.job.is_some() {
            return;
        }
        if let Some(mut pending) = self.picker.pending.take() {
            if !pending.chunk.retain_live(now) {
                return;
            }
            let text = pending.chunk.encode();
            if let Err((code, _)) =
                self.deliver(pending.chunk.target(), pending.chunk.origin(), &text)
            {
                if code == ErrorCode::Busy {
                    self.picker.pending = Some(pending);
                } else {
                    self.studio.notice =
                        Some("Selected chat could not be saved; discarded without replay.".into());
                }
            }
            return;
        }
        if let Some(chunk) = self.incoming.take_ready(now)
            && self.allowed.get(&chunk.origin().integration_id) == Some(&chunk.revision())
        {
            if selecting {
                let config = self.studio.library.decision.clone();
                match waifu_studio::chat_decision::ChatDecisionJob::start(
                    config.clone(),
                    chunk.originals().cloned().collect(),
                ) {
                    Ok(job) => {
                        self.picker.pending = Some(PendingSelection {
                            chunk,
                            config,
                            job: Some(job),
                        })
                    }
                    Err(_) => {
                        self.studio.notice =
                            Some("Chat selection could not start; this chunk was skipped.".into())
                    }
                }
                return;
            }
            let text = chunk.encode();
            if let Err((code, _)) = self.deliver(chunk.target(), chunk.origin(), &text) {
                if code == ErrorCode::Busy {
                    self.incoming.retry_busy(chunk, now);
                } else {
                    self.studio.notice = Some(
                        "An incoming chat chunk could not be saved; discarded without replay."
                            .into(),
                    );
                }
            }
        }
    }
}

#[derive(Default)]
pub struct TargetTracker {
    identity: Option<(Uuid, u64)>,
    generation: u64,
    exhausted: bool,
}
impl TargetTracker {
    fn observe(&mut self, identity: Option<(Uuid, u64)>) -> Option<Target> {
        if self.exhausted {
            return None;
        }
        if identity != self.identity {
            let Some(generation) = self.generation.checked_add(1) else {
                self.exhausted = true;
                return None;
            };
            self.identity = identity;
            self.generation = generation;
        }
        identity.map(|(uuid, _)| Target {
            uuid,
            generation: self.generation,
        })
    }
}

type ChatSources = BTreeMap<
    String,
    (
        waifu_studio::extension_store::InstalledPack,
        Option<Uuid>,
        u64,
        Uuid,
    ),
>;

#[derive(bevy::ecs::system::SystemParam)]
pub struct ExtensionFrame<'w, 's> {
    audio: Local<'s, AudioIntake>,
    outputs: ResMut<'w, waifu_studio::extension_speech_output::SpeechOutputs>,
    routing: ResMut<'w, waifu_studio::speech_pcm_routing::SpeechPcmRouting>,
    listening: Res<'w, waifu_studio::listen::ListeningState>,
    runtime: Res<'w, ExtensionRuntime>,
    studio: ResMut<'w, Studio>,
    memory: ResMut<'w, MemoryState>,
    scene_chat: Res<'w, SceneChat>,
    animation: ResMut<'w, AnimationState>,
    motion_observations: Option<Res<'w, waifu_studio::animation::MotionObservations>>,
    motion_fallback: Local<'s, waifu_studio::animation::MotionObservations>,
    behavior: ResMut<'w, BehaviorState>,
    facial: Res<'w, waifu_studio::expressions::FacialState>,
    avatar: Res<'w, AvatarLoadState>,
    desktop: Res<'w, DesktopSettings>,
    grab: Option<Res<'w, DesktopGrab>>,
    extensions: ResMut<'w, ExtensionState>,
    installed: Local<'s, waifu_studio::extension_installed::InstalledRuntime>,
    ready: Local<'s, waifu_studio::extension_lifecycle::AppReadySource>,
    target: Local<'s, TargetTracker>,
    incoming: Local<'s, IncomingChat>,
    picker: Local<'s, ChatPickerState>,
    chat_sources: Local<'s, ChatSources>,
}

pub fn frame(mut frame: ExtensionFrame) {
    frame.extensions.poll();
    {
        let ExtensionFrame {
            installed,
            extensions,
            runtime,
            ..
        } = &mut frame;
        installed.reconcile(&extensions.packs, runtime);
        if let Some(error) = installed.errors.values().next() {
            extensions.error = Some(format!("Local integration could not start: {error}"));
        } else if extensions
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("Local integration could not start: "))
        {
            extensions.error = None;
        }
    }
    {
        let ExtensionFrame {
            ready,
            extensions,
            studio,
            ..
        } = &mut frame;
        // Startup metadata is emitted after initial loading and binding, before
        // the source journal is drained. No avatar/provider/account gate here.
        ready.after_reconcile(extensions.initial_load_completed(), &mut studio.lifecycle);
    }
    let identity = frame
        .studio
        .library
        .selected
        .zip(frame.studio.animation_target(&frame.avatar))
        .map(|(id, entity)| (id, entity.to_bits()));
    let target = frame.target.observe(identity);
    {
        let observations = frame
            .motion_observations
            .as_deref()
            .cloned()
            .unwrap_or_else(|| (*frame.motion_fallback).clone());
        observations.attach(&mut frame.animation);
        frame.runtime.collect_motion_observations(&observations);
        let ExtensionFrame {
            runtime, studio, ..
        } = &mut frame;
        runtime.drain_observations(&mut studio.lifecycle);
    }
    let events = frame.studio.lifecycle.drain();
    frame.runtime.observe_app_run(&events);
    let mut allowed = BTreeMap::new();
    {
        let ExtensionFrame {
            extensions,
            chat_sources,
            runtime,
            ..
        } = &mut frame;
        for pack in &extensions.packs {
            if !pack.installed.enabled
                || pack.error.is_some()
                || pack.content.is_none()
                || !pack
                    .installed
                    .grants
                    .contains(&waifu_studio::extension_protocol::Capability::MessageDeliver)
            {
                continue;
            }
            let installed = &pack.installed;
            let Some(generation) = runtime.chat_source_generation(installed.installation_id) else {
                continue;
            };
            let credentials = pack.credentials.revision;
            let key = installed.package_id.clone();
            let entry = chat_sources
                .entry(key.clone())
                .or_insert_with(|| (installed.clone(), credentials, generation, Uuid::new_v4()));
            if &entry.0 != installed || entry.1 != credentials || entry.2 != generation {
                *entry = (installed.clone(), credentials, generation, Uuid::new_v4());
            }
            allowed.insert(key, entry.3);
        }
    }
    frame.chat_sources.retain(|id, _| allowed.contains_key(id));
    let mut audio_routes = Vec::new();
    if let Some(target) = target {
        for pack in &frame.extensions.packs {
            if let Some(revision) = allowed.get(&pack.installed.package_id)
                && let Some((session, process_generation, origin)) = frame
                    .runtime
                    .audio_source_identity(pack.installed.installation_id)
            {
                audio_routes.push(waifu_studio::extension_audio::AudioRoute {
                    session,
                    process_generation,
                    origin,
                    target,
                    revision: *revision,
                });
            }
        }
    }
    {
        let ExtensionFrame {
            audio, listening, ..
        } = &mut frame;
        audio.reconcile(
            audio_routes,
            &listening.config,
            &listening.session_key,
            std::time::Instant::now(),
        );
    }
    frame
        .incoming
        .reconcile(target, &allowed, std::time::Instant::now());
    if events.iter().any(|event| {
        matches!(
            &event.event,
            waifu_studio::extension_lifecycle::LifecycleEvent::ReplyFinished {
                outcome: waifu_studio::extension_lifecycle::ReplyOutcome::Cancelled,
                ..
            }
        )
    }) {
        frame.incoming.clear();
    }
    {
        let ExtensionFrame { picker, studio, .. } = &mut frame;
        picker.reconcile(
            target,
            &allowed,
            &studio.library.decision,
            studio.library.scene.select_incoming_messages
                && studio.library.scene.reply_to_external_messages,
            std::time::Instant::now(),
        );
    }
    {
        let ExtensionFrame {
            outputs,
            routing,
            runtime,
            studio,
            ..
        } = &mut frame;
        routing.tick(outputs, runtime, target, std::time::Instant::now());
        if let Some(notice) = routing.take_notice() {
            studio.notice = Some(notice.into());
        }
        outputs.tick(runtime, target, std::time::Instant::now());
    }
    if frame.runtime.is_empty() {
        return;
    }
    let desktop_owned = matches!(frame.studio.mode, ViewMode::Desktop)
        && (!frame.desktop.automatic_motions
            || frame.desktop.sitting
            || frame.grab.as_ref().is_some_and(|grab| grab.active));
    let catalog = ActionCatalog::from_packs(frame.extensions.active(), &frame.facial.available);
    let ExtensionFrame {
        runtime,
        studio,
        memory,
        scene_chat,
        incoming,
        picker,
        animation,
        behavior,
        audio,
        listening,
        outputs,
        ..
    } = &mut frame;
    let mut external_messages = StudioExternalMessages {
        outputs: Some(outputs),
        audio: Some((audio, listening)),
        studio,
        memory,
        scene_busy: scene_chat.is_busy() || scene_chat.has_history_writer(),
        target,
        incoming,
        picker,
        allowed: &allowed,
    };
    external_messages.flush_audio(runtime);
    external_messages.flush_chat(std::time::Instant::now());
    runtime.frame(
        &events,
        target,
        desktop_owned,
        &catalog,
        ActionControllers {
            animation,
            behavior: &mut behavior.controller,
            external_messages: Some(&mut external_messages),
        },
    );
}

/// Revoke while the real controllers still exist. Transport grace/waits remain
/// in main's post-run retained-runtime shutdown path, never this frame system.
pub fn quiesce_on_exit(
    mut exits: MessageReader<AppExit>,
    runtime: Res<ExtensionRuntime>,
    mut animation: ResMut<AnimationState>,
    mut behavior: ResMut<BehaviorState>,
) {
    if exits.read().next().is_some() {
        runtime.begin_shutdown(&mut animation, &mut behavior.controller);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_exit_message_quiesces_runtime_in_last_without_gpu_or_processes() {
        let runtime = ExtensionRuntime::default();
        let mut app = App::new();
        app.insert_resource(runtime.clone())
            .init_resource::<AnimationState>()
            .init_resource::<BehaviorState>()
            .add_message::<AppExit>()
            .add_systems(Update, |mut exit: MessageWriter<AppExit>| {
                exit.write(AppExit::Success);
            })
            .add_systems(Last, quiesce_on_exit);
        app.update();
        // Hook registration itself launches nothing, even if this assertion
        // fails. The API rejection proves the Last system latched quiescence.
        let result = runtime.configure_hooks(
            Uuid::new_v4(),
            IntegrationOrigin {
                integration_id: "org.example.exit-test".into(),
                display_name: "Exit test".into(),
            },
            [waifu_studio::extension_protocol::Capability::LifecycleObserve],
            Default::default(),
            vec![waifu_studio::extension_hooks::HookProcess {
                trigger: waifu_studio::extension_hooks::HookTrigger::AppReady,
                process: waifu_studio::extension_process::ProcessSpec {
                    program: "/not/executed".into(),
                    args: vec![],
                    working_directory: std::env::temp_dir(),
                    data_directory: std::env::temp_dir(),
                    lifetime: None,
                },
            }],
        );
        assert!(result.unwrap_err().contains("shutting down"));
        assert!(runtime.is_empty());
        assert!(runtime.shutdown(std::time::Duration::ZERO));
    }
    #[test]
    fn staggered_expiry_keeps_offered_ids_until_selection_then_retains_live_original() {
        let t = std::time::Instant::now();
        let target = Target {
            uuid: Uuid::new_v4(),
            generation: 1,
        };
        let origin = IntegrationOrigin {
            integration_id: "fixture".into(),
            display_name: "Fixture".into(),
        };
        let revision = Uuid::new_v4();
        let allowed = BTreeMap::from([("fixture".into(), revision)]);
        let config = waifu_studio::recipient_decision::DecisionConfig::default();
        let mut incoming = IncomingChat::default();
        for (id, offset) in [("old", 0), ("live", 2)] {
            incoming
                .offer(
                    target,
                    &origin,
                    revision,
                    &ChatMessage {
                        source_id: id.into(),
                        author_id: "viewer".into(),
                        author_name: "Viewer".into(),
                        text: format!("Exact {id}"),
                    },
                    t + std::time::Duration::from_secs(offset),
                )
                .unwrap();
        }
        let chunk = incoming
            .take_ready(t + std::time::Duration::from_secs(4))
            .unwrap();
        let mut picker = ChatPickerState {
            pending: Some(PendingSelection {
                chunk,
                config: config.clone(),
                job: None,
            }),
        };
        let later = t + std::time::Duration::from_secs(15);
        picker.reconcile(Some(target), &allowed, &config, true, later);
        let pending = picker.pending.as_mut().unwrap();
        assert_eq!(
            pending.chunk.originals().len(),
            2,
            "validation must use original offered IDs"
        );
        pending
            .chunk
            .select(&["old".into(), "live".into()])
            .unwrap();
        assert!(pending.chunk.retain_live(later));
        assert_eq!(pending.chunk.originals().next().unwrap().source_id, "live");
        assert_eq!(pending.chunk.originals().len(), 1);
        assert!(
            !pending
                .chunk
                .retain_live(t + std::time::Duration::from_secs(17))
        );
    }
    #[test]
    fn pending_selection_is_invalidated_by_policy_config_target_source_and_expiry() {
        let t = std::time::Instant::now();
        let target = Target {
            uuid: Uuid::new_v4(),
            generation: 1,
        };
        let origin = IntegrationOrigin {
            integration_id: "fixture".into(),
            display_name: "Fixture".into(),
        };
        let revision = Uuid::new_v4();
        let allowed = BTreeMap::from([("fixture".into(), revision)]);
        let config = waifu_studio::recipient_decision::DecisionConfig::default();
        for case in 0..6 {
            let mut incoming = IncomingChat::default();
            incoming
                .offer(
                    target,
                    &origin,
                    revision,
                    &ChatMessage {
                        source_id: "id".into(),
                        author_id: "viewer".into(),
                        author_name: "Viewer".into(),
                        text: "Exact original".into(),
                    },
                    t,
                )
                .unwrap();
            let chunk = incoming
                .take_ready(t + std::time::Duration::from_secs(4))
                .unwrap();
            let mut picker = ChatPickerState {
                pending: Some(PendingSelection {
                    chunk,
                    config: config.clone(),
                    job: None,
                }),
            };
            let mut current_config = config.clone();
            if case == 1 {
                current_config.model = "changed".into();
            }
            if case == 5 {
                incoming.clear();
            }
            let empty_allowed = BTreeMap::new();
            picker.reconcile(
                if case == 2 { None } else { Some(target) },
                if case == 3 { &empty_allowed } else { &allowed },
                &current_config,
                case != 0,
                if case == 4 {
                    t + std::time::Duration::from_secs(15)
                } else {
                    t
                },
            );
            assert!(picker.pending.is_none(), "case {case}");
        }
    }
    #[test]
    fn external_sink_rejects_scene_work_and_stale_generation_while_focused_idle() {
        let root = std::env::temp_dir().join(format!("external-sink-{}", Uuid::new_v4()));
        let (store, mut library) = waifu_studio::data::Store::open(root.clone()).unwrap();
        let character = Uuid::new_v4();
        library.characters.push(
            serde_json::from_value(serde_json::json!({
                "id": character, "name": "Fixture", "model": "fixture.vrm",
                "persona": "Fixture", "format": "VRM 1.0"
            }))
            .unwrap(),
        );
        library.selected = Some(character);
        let mut studio = Studio::new(store, library);
        assert!(studio.job.is_none());
        let target = Target {
            uuid: character,
            generation: 3,
        };
        let origin = IntegrationOrigin {
            integration_id: "org.example.fixture".into(),
            display_name: "Fixture".into(),
        };
        let mut memory = MemoryState::default();
        let mut incoming = IncomingChat::default();
        let mut picker = ChatPickerState::default();
        let allowed = [(origin.integration_id.clone(), Uuid::new_v4())].into();
        let mut sink = StudioExternalMessages {
            outputs: None,
            audio: None,
            studio: &mut studio,
            memory: &mut memory,
            scene_busy: true,
            target: Some(target),
            incoming: &mut incoming,
            picker: &mut picker,
            allowed: &allowed,
        };
        assert_eq!(
            sink.deliver(target, &origin, "Context").unwrap_err().0,
            ErrorCode::Busy
        );
        assert!(sink.studio.turns.is_empty());
        sink.scene_busy = false;
        assert_eq!(
            sink.deliver(
                Target {
                    generation: 2,
                    ..target
                },
                &origin,
                "Context"
            )
            .unwrap_err()
            .0,
            ErrorCode::StaleTarget
        );
        assert!(sink.studio.turns.is_empty());
        assert!(!sink.studio.library.scene.reply_to_external_messages);
        let message_id = sink.deliver(target, &origin, "Context").unwrap();
        assert!(
            sink.studio.job.is_none(),
            "default-off delivery must not generate"
        );
        assert_eq!(
            sink.studio.turns[0].external.as_ref().unwrap().message_id,
            message_id
        );
        assert_eq!(
            sink.studio.store.history(character).unwrap(),
            sink.studio.turns
        );
        sink.studio.library.scene.reply_to_external_messages = true;
        let provider = sink.studio.library.provider;
        sink.studio.library.providers[provider].model.clear();
        let second = sink
            .deliver(
                target,
                &origin,
                "Context retained even when a reply cannot start",
            )
            .unwrap();
        assert_ne!(second, message_id);
        assert!(sink.studio.job.is_none());
        assert_eq!(sink.studio.turns.len(), 2);
        assert_eq!(
            sink.studio.turns[1].external.as_ref().unwrap().message_id,
            second
        );
        assert!(sink.studio.notice.as_deref().unwrap().contains("delivered"));
        assert_eq!(
            sink.studio.store.history(character).unwrap(),
            sink.studio.turns
        );
        drop(studio);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn offered_chat_waits_for_chunk_window_and_saves_one_attributed_turn() {
        let root = std::env::temp_dir().join(format!("chat-chunk-sink-{}", Uuid::new_v4()));
        let (store, mut library) = waifu_studio::data::Store::open(root.clone()).unwrap();
        let character = Uuid::new_v4();
        library.characters.push(serde_json::from_value(serde_json::json!({
            "id": character, "name":"Fixture", "model":"fixture.vrm", "persona":"Fixture", "format":"VRM 1.0"
        })).unwrap());
        library.selected = Some(character);
        let mut studio = Studio::new(store, library);
        let target = Target {
            uuid: character,
            generation: 1,
        };
        let origin = IntegrationOrigin {
            integration_id: "org.example.chunk".into(),
            display_name: "Chunk fixture".into(),
        };
        let allowed = [(origin.integration_id.clone(), Uuid::new_v4())].into();
        let mut memory = MemoryState::default();
        let mut incoming = IncomingChat::default();
        let mut picker = ChatPickerState::default();
        let mut sink = StudioExternalMessages {
            outputs: None,
            audio: None,
            studio: &mut studio,
            memory: &mut memory,
            scene_busy: false,
            target: Some(target),
            incoming: &mut incoming,
            picker: &mut picker,
            allowed: &allowed,
        };
        let messages = [
            ChatMessage {
                source_id: "a".into(),
                author_id: "viewer-a".into(),
                author_name: "Alice".into(),
                text: "Hello 🍵".into(),
            },
            ChatMessage {
                source_id: "b".into(),
                author_id: "viewer-b".into(),
                author_name: "Bob".into(),
                text: "Quoted \"data\"\nsecond line".into(),
            },
        ];
        for message in &messages {
            sink.offer_chat(target, &origin, message).unwrap();
        }
        assert!(
            sink.studio.turns.is_empty(),
            "offer acceptance is not delivery"
        );
        sink.flush_chat(std::time::Instant::now());
        assert!(sink.studio.turns.is_empty());
        let ready = std::time::Instant::now() + std::time::Duration::from_secs(5);
        sink.scene_busy = true;
        sink.flush_chat(ready);
        assert!(
            sink.studio.turns.is_empty(),
            "scene writer must retain ownership"
        );
        sink.scene_busy = false;
        sink.flush_chat(ready);
        assert_eq!(sink.studio.turns.len(), 1);
        let turn = &sink.studio.turns[0];
        assert_eq!(turn.role, "external");
        assert_eq!(turn.external.as_ref().unwrap().origin, origin);
        let chunk: serde_json::Value = serde_json::from_str(&turn.content).unwrap();
        assert_eq!(chunk["kind"], "chat_chunk");
        let originals: Vec<ChatMessage> =
            serde_json::from_value(chunk["messages"].clone()).unwrap();
        assert_eq!(originals, messages);
        assert_eq!(
            sink.studio.store.history(character).unwrap(),
            sink.studio.turns
        );
        sink.flush_chat(ready);
        assert_eq!(sink.studio.turns.len(), 1, "chunk must not replay");
        assert!(
            sink.studio.job.is_none(),
            "default-off replies must stay off"
        );
        // A ready selection saves only its exact originals and never replays.
        sink.studio.library.scene.reply_to_external_messages = true;
        sink.studio.library.scene.select_incoming_messages = true;
        sink.studio.library.providers[sink.studio.library.provider]
            .model
            .clear();
        let mut selected_message = messages[1].clone();
        selected_message.source_id = "selected".into();
        let mut ignored_message = messages[0].clone();
        ignored_message.source_id = "ignored".into();
        sink.incoming
            .offer(
                target,
                &origin,
                allowed[&origin.integration_id],
                &ignored_message,
                ready,
            )
            .unwrap();
        sink.incoming
            .offer(
                target,
                &origin,
                allowed[&origin.integration_id],
                &selected_message,
                ready + std::time::Duration::from_secs(2),
            )
            .unwrap();
        let mut chunk = sink
            .incoming
            .take_ready(ready + std::time::Duration::from_secs(4))
            .unwrap();
        chunk
            .select(&["ignored".into(), "selected".into()])
            .unwrap();
        sink.picker.pending = Some(PendingSelection {
            chunk,
            config: sink.studio.library.decision.clone(),
            job: None,
        });
        sink.flush_chat(ready + std::time::Duration::from_secs(15));
        assert_eq!(sink.studio.turns.len(), 2);
        let selected: serde_json::Value =
            serde_json::from_str(&sink.studio.turns[1].content).unwrap();
        assert_eq!(selected["messages"].as_array().unwrap().len(), 1);
        assert_eq!(selected["messages"][0]["text"], selected_message.text);
        assert_eq!(
            selected["messages"][0]["author_id"],
            selected_message.author_id
        );
        assert_eq!(selected["messages"][0]["source_id"], "selected");
        sink.flush_chat(ready + std::time::Duration::from_secs(15));
        assert_eq!(sink.studio.turns.len(), 2);
        assert!(
            sink.studio.job.is_none(),
            "empty model must prevent network generation"
        );
        drop(studio);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_character_and_avatar_incarnation_change_target_generation() {
        let mut tracker = TargetTracker::default();
        let character = Uuid::new_v4();
        let first = tracker.observe(Some((character, 10))).unwrap();
        for _ in 0..4 {
            assert_eq!(tracker.observe(Some((character, 10))), Some(first));
        }
        let replacement = tracker.observe(Some((character, 11))).unwrap();
        assert!(replacement.generation > first.generation);
        assert_eq!(tracker.observe(None), None);
        assert!(
            tracker.observe(Some((character, 11))).unwrap().generation > replacement.generation
        );
        assert_eq!(
            tracker
                .observe(Some((Uuid::new_v4(), 11)))
                .unwrap()
                .generation,
            5
        );
    }
}

#[cfg(any(target_os = "linux", windows))]
#[path = "discord_replay_check.rs"]
mod discord_replay_check;

pub(crate) fn verify_discord_replay_offline() -> Result<(), String> {
    #[cfg(any(target_os = "linux", windows))]
    {
        discord_replay_check::run()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    Err("Offline installed Discord replay verification requires Windows or Linux".into())
}
