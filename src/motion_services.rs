//! Host-owned motion orchestration. UI controls share handles to this resource;
//! rendering a panel or retaining a UI context is not required to advance it.
#[path = "companion_motion.rs"]
pub mod companion_motion;
#[path = "motion_decision.rs"]
pub mod motion_decision;
#[path = "motion_library.rs"]
pub mod motion_library;

use crate::{
    desktop_behavior::DesktopGrab,
    desktop_settings::DesktopSettings,
    studio::{SpeechPlayback, Studio, ViewMode},
};
use bevy::prelude::*;
use std::sync::{Arc, Mutex, MutexGuard};
use waifu_studio::{
    animation::{AnimationState, MotionLoadError},
    extension_manifest::{ContentKind, ResolvedPack},
    extensions::ExtensionState,
    loading::AvatarLoadState,
};

/// Later extension runtimes can order their output before this host step.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MotionFrameSet;

#[derive(bevy::ecs::system::SystemParam)]
pub struct MotionFrame<'w, 's> {
    services: Res<'w, MotionServices>,
    studio: ResMut<'w, Studio>,
    animation: ResMut<'w, AnimationState>,
    avatar: Res<'w, AvatarLoadState>,
    desktop: Res<'w, DesktopSettings>,
    grab: Option<Res<'w, DesktopGrab>>,
    playing: Query<'w, 's, &'static SpeechPlayback>,
    extensions: ResMut<'w, ExtensionState>,
}

/// Ordinary Update system: polling, revocation and ownership do not require a
/// UI pass, context, window, or renderer. UI code only binds these shared handles.
pub fn host_frame(mut frame: MotionFrame) {
    frame.extensions.poll();
    frame
        .services
        .sync_extensions(&frame.extensions, &mut frame.animation);
    let desktop_pose_owned = matches!(frame.studio.mode, ViewMode::Desktop)
        && (!frame.desktop.automatic_motions
            || frame.desktop.sitting
            || frame.grab.as_ref().is_some_and(|grab| grab.active));
    let avatar = frame
        .studio
        .animation_target(&frame.avatar)
        .map(Entity::to_bits);
    let playing = frame
        .playing
        .iter()
        .any(|source| frame.studio.speech_is_playing(source));
    frame.services.frame(
        &mut frame.studio,
        &mut frame.animation,
        avatar,
        desktop_pose_owned,
        playing,
    );
}

pub type SharedSuggester = Arc<Mutex<motion_decision::MotionSuggester>>;
pub type SharedReplyMotion = Arc<Mutex<motion_decision::ReplyMotion>>;
pub type SharedCompanion = Arc<Mutex<companion_motion::CompanionAnimator>>;
pub type MotionEntries = Arc<Vec<motion_library::MotionEntry>>;

#[derive(Default)]
struct Catalog {
    entries: Option<MotionEntries>,
    extensions: MotionEntries,
    extension_generation: Option<u64>,
}

/// One state owner for production and its UI adapters. Clones share the same
/// controllers/catalog; always lock the suggester before reply-motion state.
#[derive(Resource, Clone, Default)]
pub struct MotionServices {
    pub suggester: SharedSuggester,
    pub reply: SharedReplyMotion,
    pub companion: SharedCompanion,
    catalog: Arc<Mutex<Catalog>>,
}

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl MotionServices {
    /// Lazy scan: reply motion set to Off does not cause a filesystem scan.
    pub fn entries(&self, rescan: bool) -> MotionEntries {
        let mut catalog = lock(&self.catalog);
        if !rescan && let Some(entries) = &catalog.entries {
            return entries.clone();
        }
        let mut entries = motion_library::scan(&motion_library::roots());
        entries.extend(catalog.extensions.iter().cloned());
        let entries = Arc::new(entries);
        catalog.entries = Some(entries.clone());
        entries
    }

    pub fn cached_entries(&self) -> Option<MotionEntries> {
        lock(&self.catalog).entries.clone()
    }

    /// Also permits UI test fixtures to supply a catalog without a disk scan.
    #[cfg(test)]
    pub fn replace_entries(&self, entries: MotionEntries) {
        lock(&self.catalog).entries = Some(entries);
    }

    #[cfg(test)]
    pub fn extension_entries(&self) -> MotionEntries {
        lock(&self.catalog).extensions.clone()
    }

    /// Called after the extension worker is polled, before any UI early return.
    /// Retains manual ownership while removing a disabled pack's contributions.
    pub fn sync_extensions(&self, extensions: &ExtensionState, animation: &mut AnimationState) {
        let revoked = {
            let mut catalog = lock(&self.catalog);
            if catalog.extension_generation == Some(extensions.generation) {
                return;
            }
            let next: Vec<_> = extensions.active().flat_map(pack_motions).collect();
            let revoked: Vec<_> = catalog
                .extensions
                .iter()
                .filter(|old| !next.iter().any(|new| new.path == old.path))
                .map(|entry| entry.path.clone())
                .collect();
            catalog.extensions = Arc::new(next);
            catalog.extension_generation = Some(extensions.generation);
            catalog.entries = None;
            revoked
        };
        if !revoked.is_empty() {
            animation.revoke_paths(&revoked);
            {
                let mut suggester = lock(&self.suggester);
                lock(&self.reply).manual_override(&mut suggester);
            }
            let mut companion = lock(&self.companion);
            companion.suspend();
            companion.loading_path = None;
            companion.cache.retain(|(path, _)| !revoked.contains(path));
        }
    }

    /// Every-frame production step, independent of whether a UI context exists.
    /// Returns whether a suggestion worker is pending (for UI repaint hints).
    pub fn frame(
        &self,
        studio: &mut Studio,
        animation: &mut AnimationState,
        avatar: Option<u64>,
        desktop_pose_owned: bool,
        playing: bool,
    ) -> bool {
        let pending = reply_motion_frame(self, studio, animation, avatar, desktop_pose_owned);
        if desktop_pose_owned {
            lock(&self.companion).suspend();
        } else {
            companion_motion_frame(self, studio, animation, avatar, playing);
        }
        pending
    }
}

fn pack_motions(pack: &ResolvedPack) -> impl Iterator<Item = motion_library::MotionEntry> + '_ {
    pack.manifest.content.motions.iter().filter_map(|motion| {
        let path = pack
            .content
            .iter()
            .find(|content| content.kind == ContentKind::Motion && content.local_id == motion.id)?
            .path
            .clone()?;
        let format = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "bvh" => "BVH",
            "vrma" => "VRMA",
            "fbx" => "FBX",
            _ => return None,
        };
        Some(motion_library::MotionEntry {
            metadata: Some(motion.clone()),
            group: pack.manifest.name.clone(),
            label: motion.name.clone(),
            path,
            format,
        })
    })
}

/// Include named reactions which do not match a coarse category, too. Walking
/// and other locomotion remain manual so a reply cannot move the companion away.
pub const SUGGESTED_CATEGORIES: [motion_library::MotionCategory; 7] = [
    motion_library::MotionCategory::Greeting,
    motion_library::MotionCategory::Talking,
    motion_library::MotionCategory::Thinking,
    motion_library::MotionCategory::Celebration,
    motion_library::MotionCategory::Idle,
    motion_library::MotionCategory::Dance,
    motion_library::MotionCategory::Unknown,
];

/// Any manual motion action cancels or discards an automatic pick.
pub fn manual_motion(services: &MotionServices, animation: &mut AnimationState) {
    animation.take_manual_control();
    lock(&services.companion).manual_override();
    let mut suggester = lock(&services.suggester);
    lock(&services.reply).manual_override(&mut suggester);
}

/// Advance local playlists using actual audio ownership and pending speech.
fn settle_companion_import(
    controller: &mut companion_motion::CompanionAnimator,
    animation: &AnimationState,
    now: std::time::Instant,
) {
    if !animation.loading
        && let Some(path) = controller.loading_path.take()
    {
        if animation.error.is_some() {
            controller.failed(path, now);
        } else if animation.path.as_ref() == Some(&path)
            && let Some(clip) = &animation.clip
        {
            if controller.cache.len() >= 8 {
                controller.cache.pop_front();
            }
            controller.cache.push_back((path, clip.clone()));
        }
    }
}

pub fn companion_motion_frame(
    services: &MotionServices,
    studio: &Studio,
    animation: &mut AnimationState,
    avatar: Option<u64>,
    playing: bool,
) {
    let config = &studio.library.scene.companion_motion;
    if !config.enabled {
        return;
    }
    // A queued gesture owns the next admission; keep visible ambient playback
    // running but don't let it acquire the import permit again first.
    let playback_waiting = lock(&services.reply).playback_waiting();
    let mut controller = lock(&services.companion);
    let now = std::time::Instant::now();
    settle_companion_import(&mut controller, animation, now);
    let selected_speech = studio.speaking_character().is_some()
        && studio.speaking_character() == studio.library.selected;
    let owner = studio.library.selected.zip(avatar);
    let talking = controller.speech_frame(
        now,
        owner,
        selected_speech.then_some(companion_motion::SpeechActivity {
            generation: studio.speech_generation(),
            playing,
            pending: studio.voice_busy,
        }),
    );
    if animation.manual_controlled() && !controller.paused {
        controller.manual_override();
    }
    if playback_waiting {
        return;
    }
    let entries = services.entries(false);
    if let Some(path) = controller.frame(
        now,
        config,
        owner,
        talking,
        animation.gesture_active(),
        animation.loading,
        animation
            .playing
            .then_some(animation.path.as_deref())
            .flatten(),
        &entries,
    ) {
        let cached = controller
            .cache
            .iter()
            .find(|(key, _)| key == &path)
            .map(|(_, clip)| clip.clone());
        if let Some(clip) = cached {
            animation.select_clip(clip, Some(path));
        } else {
            match animation.load_playlist(&path) {
                Ok(()) => controller.loading_path = Some(path),
                Err(error) => controller.retry_import(now, error),
            }
        }
    }
}
/// Advance reply motions; Off consumes reply IDs without scanning or requesting.
pub fn reply_motion_frame(
    services: &MotionServices,
    studio: &mut Studio,
    animation: &mut AnimationState,
    avatar: Option<u64>,
    desktop_pose_owned: bool,
) -> bool {
    use waifu_studio::data::ReplyMotionMode;
    let mut config = studio.library.scene.reply_motion.clone();
    let manual_owned = {
        let mut controller = lock(&services.companion);
        if controller.select_owner(studio.library.selected.zip(avatar)) {
            animation.cancel_automatic_motion();
        }
        controller.paused || animation.manual_controlled()
    };
    if desktop_pose_owned || manual_owned {
        // Still consume the reply ID, but do not request a motion or replay it
        // later after the user releases manual control, dragging or seating.
        config.mode = ReplyMotionMode::Off;
    }
    let mut suggester = lock(&services.suggester);
    let mut auto = lock(&services.reply);
    if let Some(job) = &studio.job {
        auto.supersede_playback(job.id);
    }
    let cached = services.cached_entries();
    let entries = match cached {
        Some(entries) => entries,
        None if config.mode != ReplyMotionMode::Off => services.entries(false),
        None => Default::default(),
    };
    let selected = studio.library.selected;
    // A catalog implies a cached scan; never poll against a missing one.
    if !entries.is_empty() {
        suggester.poll(selected, &entries);
    }
    let character = studio.library.selected_character();
    let reaction_context = studio
        .appraised_affect
        .map(|(_, _, emotion)| {
            format!(
                "The character feels {} in response to the user's message: {}",
                emotion.as_str(),
                studio
                    .turns
                    .iter()
                    .rev()
                    .find(|turn| turn.role == "user")
                    .map_or("", |turn| motion_decision::bounded_context(&turn.content))
            )
        })
        .unwrap_or_default();
    let reply = match (&studio.completed_chat, character) {
        (Some((id, Ok(text))), Some(character)) => Some(motion_decision::CompletedReply {
            id: *id,
            character: character.id,
            character_name: &character.name,
            avatar,
            text,
            affect: motion_decision::job_affect(studio.completed_affect, *id, character.id),
        }),
        // Appraisal arrives while the reply is still streaming: start the
        // emotion motion now so it plays before the talking loop takes over.
        // The same job ID is then ignored when the full reply completes.
        _ if matches!(
            config.mode,
            ReplyMotionMode::EmotionMapped | ReplyMotionMode::AutoPlay
        ) =>
        {
            match (studio.appraised_affect, character) {
                (Some((id, owner, emotion)), Some(character))
                    if owner == character.id
                        && studio.job.as_ref().is_some_and(|job| job.id == id) =>
                {
                    Some(motion_decision::CompletedReply {
                        id,
                        character: character.id,
                        character_name: &character.name,
                        avatar,
                        text: &reaction_context,
                        affect: Some(emotion),
                    })
                }
                _ => None,
            }
        }
        _ => None,
    };
    let decision = studio.library.decision.clone();
    let action = auto.frame(
        std::time::Instant::now(),
        &config,
        selected,
        avatar,
        reply,
        &entries,
        &mut suggester,
        |entries| {
            motion_library::MotionCatalog::offer(
                entries,
                &SUGGESTED_CATEGORIES,
                motion_library::MAX_OFFERED,
            )
        },
        |suggester, request| suggester.start(&decision, request),
    );
    if let Some(motion_decision::AutoAction::Play(path)) = action {
        if desktop_pose_owned {
            auto.note =
                Some("Skipped reply motion while desktop drag or seating owns the body".into());
        } else if animation.loading {
            auto.note = Some("Gesture waiting for the current motion import".into());
        } else {
            settle_companion_import(
                &mut lock(&services.companion),
                animation,
                std::time::Instant::now(),
            );
            match animation.try_load_one_shot(path) {
                Ok(()) => {
                    auto.playback_admitted(std::time::Instant::now());
                }
                Err(MotionLoadError::Busy) => {
                    auto.note = Some("Gesture waiting for the motion importer".into());
                }
                Err(MotionLoadError::Failed(error)) => auto.playback_failed(error),
            }
        }
    }
    suggester.busy()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };
    use waifu_studio::{
        animation::parse_bvh,
        data::{Character, EmotionMotionBinding, ReplyMotionMode, Store},
        loading::LoadStatus,
        reply_affect::AffectEmotion,
    };

    /// Real ordinary-Update host with no UI plugin, context, window or renderer.
    /// Shared with the UI adapter regression so it starts from a host install.
    pub(crate) struct Headless {
        pub(crate) app: App,
        root: PathBuf,
        character: uuid::Uuid,
    }

    impl Headless {
        pub(crate) fn new() -> Self {
            let root = std::env::temp_dir().join(format!("motion-host-{}", uuid::Uuid::new_v4()));
            let (store, mut library) = Store::open(root.join("library")).unwrap();
            let character = uuid::Uuid::new_v4();
            library.characters.push(Character {
                delegation: Default::default(),
                id: character,
                name: "Fixture".into(),
                model: "fixture.vrm".into(),
                persona: "Offline fixture".into(),
                format: "VRM".into(),
                greeting: "Hello".into(),
                scene: None,
                provider: None,
                voice: None,
            });
            library.selected = Some(character);
            library.scene.reply_motion.mode = ReplyMotionMode::Off;
            library.scene.companion_motion.enabled = false;
            let mut studio = Studio::new(store, library);
            studio.requested_model = None;
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .insert_resource(studio)
                .insert_resource(ExtensionState::new(root.join("extensions")))
                .init_resource::<MotionServices>()
                .init_resource::<AnimationState>()
                .init_resource::<AvatarLoadState>()
                .init_resource::<DesktopSettings>()
                .add_systems(Update, host_frame.in_set(MotionFrameSet));
            let avatar = app.world_mut().spawn_empty().id();
            {
                let mut load = app.world_mut().resource_mut::<AvatarLoadState>();
                load.active = Some(avatar);
                load.status = LoadStatus::Ready;
                load.requested_path = Some("fixture.vrm".into());
            }
            let mut fixture = Self {
                app,
                root,
                character,
            };
            fixture.settle_extensions();
            fixture
        }

        pub(crate) fn settle_extensions(&mut self) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                self.app.update();
                let state = self.app.world().resource::<ExtensionState>();
                if !state.busy {
                    assert!(state.error.is_none(), "{:?}", state.error);
                    break;
                }
                assert!(Instant::now() < deadline, "extension worker timed out");
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        pub(crate) fn install_starter(&mut self) {
            self.app
                .world_mut()
                .resource_mut::<ExtensionState>()
                .install(
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("assets/extensions/companion-starter"),
                );
            self.settle_extensions();
        }

        fn reply(&mut self) -> uuid::Uuid {
            let id = uuid::Uuid::new_v4();
            let mut studio = self.app.world_mut().resource_mut::<Studio>();
            studio.completed_chat = Some((id, Ok("Offline reply".into())));
            studio.completed_affect = Some((id, self.character, AffectEmotion::Happy));
            id
        }
    }

    impl Drop for Headless {
        fn drop(&mut self) {
            self.app = App::new();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn update_preserves_manual_hold_and_consumes_replies_during_manual_and_desktop_ownership() {
        let mut fixture = Headless::new();
        let services = fixture.app.world().resource::<MotionServices>().clone();
        assert!(
            services.cached_entries().is_none(),
            "Off with playlists disabled must not scan"
        );
        let path = PathBuf::from("joy.bvh");
        let entries = Arc::new(vec![motion_library::MotionEntry {
            metadata: None,
            group: "fixture".into(),
            label: "Joy".into(),
            path: path.clone(),
            format: "BVH",
        }]);
        services.replace_entries(entries.clone());
        {
            let mut studio = fixture.app.world_mut().resource_mut::<Studio>();
            studio.library.scene.reply_motion.mode = ReplyMotionMode::EmotionMapped;
            studio
                .library
                .scene
                .reply_motion
                .emotion_bindings
                .push(EmotionMotionBinding {
                    emotion: AffectEmotion::Happy,
                    path: path.clone(),
                });
        }
        let clip = Arc::new(
            parse_bvh(include_str!(
                "../assets/animations/companion/idle-look-down.bvh"
            ))
            .unwrap(),
        );
        {
            let mut animation = fixture.app.world_mut().resource_mut::<AnimationState>();
            animation.select_clip(clip, Some("manual.bvh".into()));
            manual_motion(&services, &mut animation);
            animation.pause();
        }
        fixture.reply();
        fixture.app.update();
        let next_avatar = fixture.app.world_mut().spawn_empty().id();
        fixture
            .app
            .world_mut()
            .resource_mut::<AvatarLoadState>()
            .active = Some(next_avatar);
        fixture.app.update();
        {
            let mut animation = fixture.app.world_mut().resource_mut::<AnimationState>();
            assert!(animation.manual_controlled() && animation.is_posing());
            assert!(!animation.playing && !animation.loading);
            animation.resume_automatic_control();
        }
        lock(&services.companion).resume();
        fixture.app.update();
        assert!(
            !fixture.app.world().resource::<AnimationState>().loading,
            "Resume must not replay the consumed reply"
        );
        fixture.app.world_mut().resource_mut::<Studio>().mode = ViewMode::Desktop;
        fixture
            .app
            .world_mut()
            .resource_mut::<DesktopSettings>()
            .sitting = true;
        fixture.reply();
        fixture.app.update();
        fixture
            .app
            .world_mut()
            .resource_mut::<DesktopSettings>()
            .sitting = false;
        fixture.app.update();
        assert!(
            !fixture.app.world().resource::<AnimationState>().loading,
            "Releasing desktop ownership must not replay its reply"
        );
        // Prove the current identity/catalog is eligible for a subsequent reply,
        // using the real selector without starting a file or provider worker.
        let studio = fixture.app.world().resource::<Studio>();
        let load = fixture.app.world().resource::<AvatarLoadState>();
        assert_eq!(studio.animation_target(load), Some(next_avatar));
        let mut suggester = lock(&services.suggester);
        let action = lock(&services.reply).frame(
            Instant::now(),
            &studio.library.scene.reply_motion,
            Some(fixture.character),
            Some(next_avatar.to_bits()),
            Some(motion_decision::CompletedReply {
                id: uuid::Uuid::new_v4(),
                character: fixture.character,
                character_name: "Fixture",
                avatar: Some(next_avatar.to_bits()),
                text: "Next reply",
                affect: Some(AffectEmotion::Happy),
            }),
            &entries,
            &mut suggester,
            |_| panic!("Emotion mapping requires no decision catalog"),
            |_, _| panic!("Emotion mapping requires no provider"),
        );
        assert!(
            matches!(action, Some(motion_decision::AutoAction::Play(chosen)) if chosen == path)
        );
    }

    #[test]
    fn headless_update_installs_and_revokes_catalog_and_pose_through_shared_handles() {
        let mut fixture = Headless::new();
        fixture.install_starter();
        let services = fixture.app.world().resource::<MotionServices>().clone();
        let handles = services.clone();
        let entries = services.extension_entries();
        assert_eq!(entries.len(), 3);
        // Supply the installed catalog without scanning unrelated local folders.
        services.replace_entries(entries.clone());
        let path = entries[0].path.clone();
        let clip = Arc::new(parse_bvh(&std::fs::read_to_string(&path).unwrap()).unwrap());
        {
            let mut animation = fixture.app.world_mut().resource_mut::<AnimationState>();
            animation.select_clip(clip.clone(), Some(path.clone()));
            manual_motion(&handles, &mut animation);
        }
        {
            let mut companion = lock(&handles.companion);
            companion.cache.push_back((path.clone(), clip));
            companion.loading_path = Some(path.clone());
        }
        let id = fixture.app.world().resource::<ExtensionState>().packs[0]
            .installed
            .installation_id;
        fixture
            .app
            .world_mut()
            .resource_mut::<ExtensionState>()
            .set_enabled(id, false);
        fixture.settle_extensions();
        assert!(services.extension_entries().is_empty());
        assert!(handles.cached_entries().is_none());
        let animation = fixture.app.world().resource::<AnimationState>();
        assert!(animation.manual_controlled() && !animation.is_posing());
        let companion = lock(&handles.companion);
        assert!(companion.paused);
        assert!(companion.cache.is_empty() && companion.loading_path.is_none());
        assert!(path.is_file(), "Revocation retains imported source files");
    }

    #[test]
    fn headless_emotion_waits_for_import_admission_then_plays_once() {
        let mut fixture = Headless::new();
        fixture
            .app
            .add_plugins(waifu_studio::animation::BvhAnimationPlugin);
        let services = fixture.app.world().resource::<MotionServices>().clone();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets/animations/companion/idle-look-down.bvh");
        services.replace_entries(Arc::new(vec![motion_library::MotionEntry {
            metadata: None,
            group: "fixture".into(),
            label: "joy".into(),
            path: path.clone(),
            format: "BVH",
        }]));
        {
            let mut studio = fixture.app.world_mut().resource_mut::<Studio>();
            studio.library.scene.reply_motion.mode = ReplyMotionMode::EmotionMapped;
            studio
                .library
                .scene
                .reply_motion
                .emotion_bindings
                .push(EmotionMotionBinding {
                    emotion: AffectEmotion::Happy,
                    path: path.clone(),
                });
        }
        fixture
            .app
            .world_mut()
            .resource_mut::<AnimationState>()
            .loading = true;
        fixture.reply();
        fixture.app.update();
        fixture.app.update();
        assert!(lock(&services.reply).playback_waiting());
        fixture
            .app
            .world_mut()
            .resource_mut::<AnimationState>()
            .loading = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            fixture.app.update();
            let state = fixture.app.world().resource::<AnimationState>();
            if !state.loading && state.path.as_ref() == Some(&path) {
                assert!(state.gesture_active() && !state.looping);
                break;
            }
            assert!(state.error.is_none(), "{:?}", state.error);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!lock(&services.reply).playback_waiting());
        fixture
            .app
            .world_mut()
            .resource_mut::<AnimationState>()
            .stop();
        fixture.app.update();
        let state = fixture.app.world().resource::<AnimationState>();
        assert!(
            !state.loading && !state.playing,
            "same reply cannot play twice"
        );
    }

    #[test]
    fn queued_gesture_settles_real_playlist_success_or_failure_before_admission() {
        for broken in [false, true] {
            let mut fixture = Headless::new();
            let services = fixture.app.world().resource::<MotionServices>().clone();
            let path = fixture.root.join("playlist.bvh");
            std::fs::write(
                &path,
                if broken {
                    "invalid motion"
                } else {
                    include_str!("../assets/animations/companion/idle-look-down.bvh")
                },
            )
            .unwrap();
            let gesture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets/animations/companion/talking-nod.bvh");
            services.replace_entries(Arc::new(vec![motion_library::MotionEntry {
                metadata: None,
                group: "fixture".into(),
                label: "joy".into(),
                path: gesture.clone(),
                format: "BVH",
            }]));
            {
                let mut studio = fixture.app.world_mut().resource_mut::<Studio>();
                studio.library.scene.reply_motion.mode = ReplyMotionMode::EmotionMapped;
                studio
                    .library
                    .scene
                    .reply_motion
                    .emotion_bindings
                    .push(EmotionMotionBinding {
                        emotion: AffectEmotion::Happy,
                        path: gesture.clone(),
                    });
            }
            fixture
                .app
                .world_mut()
                .resource_mut::<AnimationState>()
                .load_playlist(&path)
                .unwrap();
            lock(&services.companion).loading_path = Some(path.clone());
            fixture.reply();
            fixture.app.update();
            assert!(lock(&services.reply).playback_waiting());
            fixture
                .app
                .add_plugins(waifu_studio::animation::BvhAnimationPlugin);
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                fixture.app.update();
                let state = fixture.app.world().resource::<AnimationState>();
                if !state.loading && state.path.as_ref() == Some(&gesture) {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            let controller = lock(&services.companion);
            assert!(controller.loading_path.is_none());
            if broken {
                assert!(controller.cache.iter().all(|(cached, _)| cached != &path));
                assert!(
                    controller
                        .note
                        .as_deref()
                        .is_some_and(|note| note.contains("could not be loaded"))
                );
            } else {
                assert!(controller.cache.iter().any(|(cached, _)| cached == &path));
            }
        }
    }
}
