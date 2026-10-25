mod agent_controls;
mod appearance_controls;
mod avatar_preview;
mod behavior_controls;
mod behavior_runtime;
mod desktop_behavior;
mod desktop_menu;
mod desktop_settings;
mod environment_controls;
mod expression_controls;
mod extension_check_cli;
mod extension_host;
mod facts_memory_controls;
mod fish_live_controls;
mod fish_ws_controls;
mod gaze_controls;
mod incoming_chat;
mod lipsync_controls;
mod listen_controls;
mod mcp_controls;
mod memory_controls;
mod motion_services;
mod offscreen;
mod orchestration;
mod profile_library;
mod provider_controls;
mod routine_controls;
mod runtime;
mod runtime_metrics;
mod scene_chat_controls;
mod scene_roster_controls;
mod semantic_memory_controls;
mod showcase;
mod spring_physics_controls;
mod studio;
mod task_board_controls;
mod ui;
mod visual_controls;
mod voice_design_controls;
mod voice_hotkeys;
mod voice_library_controls;
mod window_alpha;

use bevy::{
    asset::{AssetMetaCheck, UnapprovedPathMode},
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    window::ExitCondition,
};
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use std::{path::PathBuf, process::ExitCode};
use studio::{Studio, Tab, ViewMode};
use waifu_studio::{
    data::{Store, find_models},
    loading::{AvatarLoadState, LoadStatus, VrmLoaderPlugin},
    shaders::ShaderPreviewPlugin,
};

#[derive(Resource)]
struct Capture {
    path: Option<PathBuf>,
    since: Option<f64>,
    target: Option<Handle<Image>>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    if std::env::args_os()
        .skip(1)
        .any(|arg| arg == "--discord-identify-admission")
    {
        if std::env::args_os().len() != 2
            || std::env::args_os()
                .nth(1)
                .is_none_or(|arg| arg != "--discord-identify-admission")
        {
            return Err("Use --discord-identify-admission alone".into());
        }
        let result = waifu_studio::discord_identify_admission().map_err(str::to_owned)?;
        println!("{result}");
        return Ok(());
    }
    let mut check_args = std::env::args_os().skip(1);
    if check_args
        .next()
        .is_some_and(|arg| arg == "--check-extension")
    {
        return extension_check_cli::run(check_args);
    }
    if check_args.any(|arg| arg == "--check-extension") {
        return Err("Use --check-extension FOLDER alone as the first option".into());
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--twitch-extension")
    {
        return waifu_studio::twitch_worker::run();
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--discord-extension")
    {
        return waifu_studio::discord_worker::run();
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--discord-replay-extension")
    {
        return waifu_studio::discord_worker::run_replay();
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--verify-discord-replay")
    {
        if std::env::args_os().count() != 2 {
            return Err("Use --verify-discord-replay alone; it owns a temporary profile".into());
        }
        return extension_host::verify_discord_replay_offline();
    }
    let mut directory = directories::ProjectDirs::from("", "", "waifu-studio")
        .map(|p| p.data_local_dir().to_path_buf())
        .ok_or("Cannot locate application data directory")?;
    let mut animation_path = None;
    let default_directory = directory.clone();
    let mut explicit_directory = false;
    let mut showcase_path = None;
    let mut shader = None;
    let mut imports = vec![];
    let mut capture = None;
    let mut offscreen = false;
    let mut capture_advanced = false;
    let mut show_history = false;
    let mut capture_size = UVec2::new(1920, 1080);
    let mut desktop = None;
    let mut explicit_screen = false;
    let mut environment = None;
    let mut room_view = false;
    let mut tab = Tab::Chat;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => {
                println!(
                    "Waifu Studio — native Rust character companion\n\nUsage: waifu-studio [model.vrm] [--import-folder DIR] [--data-dir DIR]\n       waifu-studio --studio\n\nDefault: draggable desktop companion; right-click for settings.\n--desktop explicitly selects companion mode\n--studio opens the full studio and settings\n\nModels: VRM 0.0 / 1.0, GLB, glTF\nConnections: Vercel AI Gateway, OpenRouter, OpenAI-compatible local/custom\n--check-extension FOLDER validates a manifest without installation or execution (use alone)\n--environment rose-bedroom|violet-studio|cyan-lounge selects a furnished room\n--room-view frames the room\n--show-history opens the conversation history panel\n--capture FILE.png saves the rendered app and exits\n--offscreen-capture FILE.png renders the actual UI without creating a desktop window\n--offscreen-showcase PLAN.json records UI frames (isolated --data-dir required)\n--capture-advanced expands and scrolls to Advanced in offscreen settings captures\n--capture-size WIDTHxHEIGHT sets offscreen dimensions (default 1920x1080)\n--screen chat|scene-chat|characters|scene|connections|voice|tools|memory|tasks|extensions chooses the initial panel\n--animation FILE.bvh|vrma|fbx loads a humanoid motion\n--shader toon|lit|unlit selects appearance\nXR/VR will follow the base app; it is not enabled in this build."
                );
                return Ok(());
            }
            Some("--data-dir") => {
                explicit_directory = true;
                directory = PathBuf::from(args.next().ok_or("--data-dir requires a folder")?)
            }
            Some("--offscreen-showcase") => {
                offscreen = true;
                showcase_path = Some(PathBuf::from(
                    args.next()
                        .ok_or("--offscreen-showcase requires a JSON plan")?,
                ));
            }
            Some("--import-folder") => imports.extend(find_models(
                &PathBuf::from(args.next().ok_or("--import-folder requires a folder")?),
                8,
            )),
            Some("--capture-advanced") => capture_advanced = true,
            Some("--show-history") => show_history = true,
            Some("--capture") => {
                capture = Some(PathBuf::from(
                    args.next().ok_or("--capture requires a path")?,
                ))
            }
            Some("--offscreen-capture") => {
                offscreen = true;
                capture = Some(PathBuf::from(
                    args.next().ok_or("--offscreen-capture requires a path")?,
                ));
            }
            Some("--capture-size") => {
                let size = args
                    .next()
                    .and_then(|v| v.into_string().ok())
                    .ok_or("--capture-size requires WIDTHxHEIGHT")?;
                let (width, height) = size
                    .split_once('x')
                    .ok_or("--capture-size requires WIDTHxHEIGHT")?;
                capture_size = UVec2::new(
                    width.parse().map_err(|_| "Invalid capture width")?,
                    height.parse().map_err(|_| "Invalid capture height")?,
                );
                if capture_size.min_element() < 320 || capture_size.max_element() > 4096 {
                    return Err("Capture dimensions must be between 320 and 4096".into());
                }
            }
            Some("--shader") => {
                shader = Some(
                    match args.next().and_then(|v| v.into_string().ok()).as_deref() {
                        Some("toon") => 0,
                        Some("lit") => 1,
                        Some("unlit") => 2,
                        _ => return Err("Use --shader toon|lit|unlit".into()),
                    },
                )
            }
            Some("--animation") => {
                animation_path = Some(PathBuf::from(
                    args.next()
                        .ok_or("--animation requires a BVH, VRMA or FBX path")?,
                ))
            }
            Some("--desktop") => desktop = Some(true),
            Some("--studio") => desktop = Some(false),
            Some("--room-view") => room_view = true,
            Some("--environment") => {
                use waifu_studio::environments::EnvironmentKind;
                environment = Some(match args.next().and_then(|v|v.into_string().ok()).as_deref() {
                    Some("rose-bedroom") => EnvironmentKind::RoseBedroom,
                    Some("violet-studio") => EnvironmentKind::VioletStudio,
                    Some("cyan-lounge") => EnvironmentKind::CyanLounge,
                    Some("cozy-room") => EnvironmentKind::CozyRoom,
                    Some("moonlit-garden") => EnvironmentKind::MoonlitGarden,
                    Some("neon-rooftop") => EnvironmentKind::NeonRooftop,
                    Some("none") => EnvironmentKind::Disabled,
                    _ => return Err("Use --environment rose-bedroom|violet-studio|cyan-lounge|cozy-room|moonlit-garden|neon-rooftop|none".into()),
                });
            }
            Some("--screen") => {
                explicit_screen = true;
                tab = match args.next().and_then(|v| v.into_string().ok()).as_deref() {
                    Some("chat") => Tab::Chat,
                    Some("scene-chat") => Tab::SceneChat,
                    Some("characters") => Tab::Characters,
                    Some("scene") => Tab::Scene,
                    Some("connections") => Tab::Connections,
                    Some("voice") => Tab::Voice,
                    Some("tools") => Tab::Tools,
                    Some("memory") => Tab::Memory,
                    Some("tasks") => Tab::Tasks,
                    Some("extensions") => Tab::Extensions,
                    _ => return Err("Unknown screen".into()),
                }
            }
            Some(arg) if arg.starts_with('-') => return Err(format!("Unknown option: {arg}")),
            _ => imports.push(PathBuf::from(arg)),
        }
    }
    if capture_advanced && !offscreen {
        return Err("--capture-advanced requires offscreen capture".into());
    }
    if offscreen && desktop == Some(true) {
        return Err("Offscreen UI capture requires Studio mode".into());
    }
    let showcase = if let Some(path) = showcase_path {
        if !explicit_directory
            || capture.is_some()
            || directory.canonicalize().ok() == default_directory.canonicalize().ok()
            || directory.join("credentials.env").exists()
        {
            return Err("Showcase requires a separate explicit --data-dir without credentials and no still capture".into());
        }
        Some(showcase::Showcase::load(&path)?)
    } else {
        None
    };
    // SAFETY: no application worker threads or Bevy plugins have started yet.
    // Private local credentials never enter the library or source package.
    unsafe { waifu_studio::credentials::load_startup(&directory.join("credentials.env")) }
        .map_err(|e| e.to_string())?;
    let desktop_settings =
        desktop_settings::DesktopSettings::load(directory.join("desktop-settings.json"));
    let memory = waifu_studio::memory::MemoryState::load(directory.join("memory"));
    let semantic =
        waifu_studio::semantic_memory::SemanticMemory::load(directory.join("semantic-memory"));
    let scene_chat = waifu_studio::scene_chat::SceneChat::new(directory.clone());
    let scene_roster =
        waifu_studio::scene_roster::SceneRoster::load(directory.join("scene-roster.json"));
    let extensions = waifu_studio::extensions::ExtensionState::new(directory.join("extensions"));
    let (store, mut library) = Store::open(directory.clone())?;
    let bundle = waifu_studio::bundled::manifest_path();
    if bundle.is_file() && waifu_studio::bundled::seed(&mut library, &bundle)? > 0 {
        store.save(&library)?;
    }
    let mut studio = Studio::new(store, library);
    studio.model_discovery_enabled = !offscreen;
    studio.show_history = show_history;
    if !imports.is_empty() {
        studio.import_paths(imports);
    }
    studio.tab = tab;
    if let Some(shader) = shader {
        studio.library.scene.shader = shader;
    }
    if let Some(kind) = environment {
        studio.library.scene.environment = kind;
        studio.library.scene.room_view =
            environment_room_view(kind, studio.library.scene.room_view);
        if matches!(
            kind,
            waifu_studio::environments::EnvironmentKind::RoseBedroom
                | waifu_studio::environments::EnvironmentKind::VioletStudio
                | waifu_studio::environments::EnvironmentKind::CyanLounge
        ) && !studio.library.scene.visuals.enabled
        {
            use waifu_studio::visuals::{VisualPreset, VisualSettings};
            studio.library.scene.visuals = VisualSettings::preset(
                if kind == waifu_studio::environments::EnvironmentKind::RoseBedroom {
                    VisualPreset::SoftAnime
                } else {
                    VisualPreset::Neon
                },
            );
        }
    }
    if room_view {
        studio.library.scene.room_view = true;
    }
    studio.mode = startup_mode(
        desktop,
        offscreen
            || capture.is_some()
            || room_view
            || environment.is_some()
            || explicit_screen
            || show_history,
    );
    let mut animation = waifu_studio::animation::AnimationState::default();
    if let Some(path) = animation_path {
        animation.load_path(path)?;
    }
    let mut listening = waifu_studio::listen::ListeningState::default();
    listening.config = studio.library.listen.clone();
    let hotkeys = voice_hotkeys::VoiceHotkeys::load(directory.join("hotkeys.json"), !offscreen);
    let mut app = App::new();
    let mut plugins = DefaultPlugins
        .set(runtime::task_pool_plugin())
        .set(AssetPlugin {
            file_path: ".".into(),
            meta_check: AssetMetaCheck::Never,
            unapproved_path_mode: UnapprovedPathMode::Allow,
            ..default()
        })
        .set(WindowPlugin {
            primary_window: if offscreen {
                None
            } else {
                Some(runtime::primary_window())
            },
            exit_condition: if offscreen {
                ExitCondition::DontExit
            } else {
                ExitCondition::OnPrimaryClosed
            },
            ..default()
        });
    if offscreen {
        plugins = plugins
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::audio::AudioPlugin>();
        app.insert_resource(offscreen::OffscreenCapture(capture_size, capture_advanced))
            .add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
                std::time::Duration::from_secs_f64(1.0 / 60.0),
            ))
            .add_systems(PostStartup, offscreen::setup);
    }
    app.insert_resource(extensions)
        .insert_resource(hotkeys)
        .insert_resource(listening)
        .insert_resource(memory)
        .insert_resource(semantic)
        .insert_resource(scene_chat)
        .insert_resource(scene_roster)
        .init_resource::<waifu_studio::task_board::TaskBoard>()
        .init_resource::<waifu_studio::fish_live::FishLiveState>()
        .init_resource::<waifu_studio::fish_ws::FishWsState>()
        .init_resource::<waifu_studio::behavior::BehaviorState>()
        .init_resource::<waifu_studio::mcp::McpState>()
        .init_resource::<waifu_studio::agents::AgentState>()
        .init_resource::<orchestration::OrchestrationState>()
        .init_resource::<motion_services::MotionServices>()
        .init_resource::<waifu_studio::voice_design::VoiceDesignState>()
        .init_resource::<waifu_studio::voice_library::VoiceLibraryState>()
        .insert_resource(animation)
        .insert_resource(desktop_settings)
        .insert_resource(studio)
        .insert_resource(Capture {
            path: capture,
            since: None,
            target: None,
        })
        .add_plugins((
            plugins,
            EguiPlugin::default(),
            waifu_studio::animation::BvhAnimationPlugin,
            VrmLoaderPlugin,
            waifu_studio::scene_roster::SceneRosterPlugin,
            waifu_studio::expressions::ExpressionsPlugin,
            waifu_studio::appearance::AppearancePlugin,
            waifu_studio::viseme_audio::VisemePlugin,
            ShaderPreviewPlugin,
            runtime::StudioRuntimePlugin,
            window_alpha::WindowAlphaPlugin,
            desktop_behavior::DesktopBehaviorPlugin,
            waifu_studio::environments::EnvironmentPlugin,
            waifu_studio::visuals::VisualPlugin,
            waifu_studio::desktop_pose::DesktopPosePlugin,
        ))
        .add_plugins(waifu_studio::spring_physics::SpringPhysicsPlugin)
        .add_plugins(voice_hotkeys::VoiceHotkeysPlugin)
        .add_systems(
            Update,
            (
                studio::poll_studio,
                sync_spring_physics,
                sync_scene_members,
                poll_scene_chat,
                studio::sync_viseme_profile,
                studio::poll_voice,
                studio::poll_lifecycle,
                studio::poll_input,
                studio::poll_assistant,
                orchestration::poll_orchestration,
                behavior_runtime::poll_behavior,
                extension_host::frame,
                studio::poll_call_speech,
                motion_services::host_frame.in_set(motion_services::MotionFrameSet),
                capture_frame,
            )
                .chain(),
        )
        .add_plugins((
            avatar_preview::AvatarPreviewPlugin,
            waifu_studio::gaze::GazePlugin,
        ))
        .add_systems(Update, sync_gaze)
        .init_resource::<waifu_studio::model_catalog::SpeechCatalogs>()
        .add_systems(EguiPrimaryContextPass, ui::studio_ui);
    if offscreen {
        app.init_asset::<waifu_studio::metered_audio::MeteredSpeech>()
            .init_resource::<waifu_studio::metered_audio::SpeechMeter>();
    } else {
        app.add_plugins(waifu_studio::metered_audio::MeteredAudioPlugin);
    }
    if let Some(showcase) = showcase {
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / showcase.fps() as f64),
        ))
        .insert_resource(showcase)
        .add_systems(PreUpdate, showcase::record)
        .add_systems(Last, showcase::trace);
    }
    let extension_runtime = waifu_studio::extension_runtime::ExtensionRuntime::default();
    app.insert_resource(extension_runtime.clone())
        .init_resource::<waifu_studio::extension_speech_output::SpeechOutputs>()
        .init_resource::<waifu_studio::speech_pcm_routing::SpeechPcmRouting>();
    app.add_systems(Last, extension_host::quiesce_on_exit);
    let result = app.run();
    if !extension_runtime.shutdown(std::time::Duration::from_secs(2)) {
        warn!("Extension cleanup exceeded the shutdown deadline");
    }
    match result {
        AppExit::Success => Ok(()),
        _ => Err("Application exited with an error".into()),
    }
}

fn sync_spring_physics(
    studio: Res<Studio>,
    avatar: Res<AvatarLoadState>,
    mut physics: ResMut<waifu_studio::spring_physics::SpringPhysicsState>,
) {
    let config = &studio.library.scene.spring_physics;
    let available = studio.animation_target(&avatar).is_some();
    if physics.config != *config || !available {
        let mut desired = config.clone();
        desired.enabled &= available;
        if physics.config != desired {
            physics.config = desired;
        }
    }
}

fn sync_scene_members(
    mut commands: Commands,
    studio: Res<Studio>,
    chat: Res<waifu_studio::scene_chat::SceneChat>,
    focused: Res<AvatarLoadState>,
    mut roster: ResMut<waifu_studio::scene_roster::SceneRoster>,
    attached: Query<&waifu_studio::expressions::SceneMemberFacial>,
) {
    roster.poll(&studio.library);
    let removed: Vec<_> = roster
        .slots
        .iter()
        .filter(|slot| {
            Some(slot.character) == studio.library.selected
                || !studio
                    .library
                    .characters
                    .iter()
                    .any(|c| c.id == slot.character)
        })
        .map(|slot| slot.character)
        .collect();
    for character in removed {
        roster.remove(character);
    }
    let members = roster
        .slots
        .iter()
        .filter_map(|slot| slot.load.active.map(|root| (slot.character, root)))
        .chain(studio.library.selected.zip(focused.active));
    for (id, entity) in members {
        let Some(character) = studio.library.characters.iter().find(|c| c.id == id) else {
            continue;
        };
        let config = if studio.library.selected == Some(id) {
            studio.library.scene.facial.clone()
        } else {
            character
                .scene
                .as_ref()
                .map(|s| s.facial.clone())
                .unwrap_or_default()
        };
        let scene = if studio.library.selected == Some(id) {
            Some(&studio.library.scene)
        } else {
            character.scene.as_ref()
        };
        let emotion = scene
            .filter(|scene| {
                scene.behavior.enabled && scene.behavior.validate().is_ok() && scene.affect.enabled
            })
            .and_then(|scene| {
                chat.affect
                    .entries
                    .get(&id)
                    .filter(|mood| mood.config == scene.affect)
                    .map(|mood| waifu_studio::expressions::SceneMemberEmotion {
                        emotion: mood.emotion,
                        strength: mood.mood.magnitude().min(scene.behavior.emotion_max),
                        smoothing_seconds: scene.behavior.smoothing_seconds,
                    })
            });
        if let Some(emotion) = emotion {
            commands.entity(entity).insert(emotion);
        } else {
            commands
                .entity(entity)
                .remove::<waifu_studio::expressions::SceneMemberEmotion>();
        }
        if attached
            .get(entity)
            .is_ok_and(|old| old.character == id && old.config == config)
        {
            continue;
        }
        commands
            .entity(entity)
            .insert(waifu_studio::expressions::SceneMemberFacial {
                character: id,
                config,
            });
    }
}

fn poll_scene_chat(
    mut chat: ResMut<waifu_studio::scene_chat::SceneChat>,
    roster: Res<waifu_studio::scene_roster::SceneRoster>,
    mut studio: ResMut<Studio>,
    time: Res<Time>,
    mut cancellation_revision: Local<u64>,
) {
    chat.poll(&studio.library, &roster);
    for event in chat.drain_lifecycle() {
        studio.lifecycle.record(event.event);
    }
    chat.tick_affect(&studio.library, &roster, time.delta_secs_f64());
    if *cancellation_revision != chat.cancellation_revision {
        studio.cancel_scene_speech();
        *cancellation_revision = chat.cancellation_revision;
    }
    let allowed: Vec<_> = studio
        .library
        .characters
        .iter()
        .filter(|character| {
            Some(character.id) == studio.library.selected || roster.contains(character.id)
        })
        .map(|character| character.id)
        .collect();
    studio.retain_scene_speech(&allowed);
    while let Some((reply_id, character, config, text)) = chat.take_speech_with_reply() {
        if let Err(error) = studio.queue_reply_speech(reply_id, character, config, text) {
            studio.error = Some(error);
        }
    }
}

fn capture_frame(
    mut commands: Commands,
    time: Res<Time>,
    state: Res<AvatarLoadState>,
    studio: Res<Studio>,
    mut capture: ResMut<Capture>,
    mut exit: MessageWriter<AppExit>,
) {
    if capture.path.is_none() {
        return;
    }
    if matches!(state.status, LoadStatus::Failed(_)) {
        error!("Model failed to load: {:?}", state.status);
        exit.write(AppExit::error());
        return;
    }
    if state.status != LoadStatus::Ready && studio.library.selected.is_some() {
        if time.elapsed_secs_f64() > 60.0 {
            error!("Capture timed out");
            exit.write(AppExit::error());
        }
        return;
    }
    let since = *capture.since.get_or_insert(time.elapsed_secs_f64());
    if time.elapsed_secs_f64() - since < 5.0 {
        return;
    }
    let path = capture.path.take().unwrap();
    let screenshot = capture
        .target
        .as_ref()
        .map_or_else(Screenshot::primary_window, |target| {
            Screenshot::image(target.clone())
        });
    commands.spawn(screenshot).observe(
        move |event: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
            let result = event
                .image
                .clone()
                .try_into_dynamic()
                .map_err(|e| e.to_string())
                .and_then(|image| image.save(&path).map_err(|e| e.to_string()));
            match result {
                Ok(()) => {
                    info!("STUDIO_CAPTURE {}", path.display());
                    exit.write(AppExit::Success);
                }
                Err(error) => {
                    error!("Capture failed: {error}");
                    exit.write(AppExit::error());
                }
            }
        },
    );
}

fn sync_gaze(studio: Res<Studio>, mut gaze: ResMut<waifu_studio::gaze::GazeState>) {
    if gaze.config != studio.library.scene.gaze {
        gaze.config = studio.library.scene.gaze.clone();
    }
}

fn environment_room_view(kind: waifu_studio::environments::EnvironmentKind, saved: bool) -> bool {
    use waifu_studio::environments::EnvironmentKind;
    match kind {
        EnvironmentKind::Disabled => saved,
        EnvironmentKind::RoseBedroom
        | EnvironmentKind::VioletStudio
        | EnvironmentKind::CyanLounge => true,
        _ => false,
    }
}

fn startup_mode(desktop: Option<bool>, studio_workflow: bool) -> ViewMode {
    match desktop {
        Some(true) => ViewMode::Desktop,
        Some(false) => ViewMode::Studio,
        None if studio_workflow => ViewMode::Studio,
        None => ViewMode::Desktop,
    }
}
#[cfg(test)]
mod command_line_tests {
    use super::*;
    #[test]
    fn ordinary_launch_is_a_draggable_companion_and_studio_is_explicit() {
        assert_eq!(startup_mode(None, false), ViewMode::Desktop);
        assert_eq!(startup_mode(None, true), ViewMode::Studio);
        for workflow in [false, true] {
            assert_eq!(startup_mode(Some(true), workflow), ViewMode::Desktop);
            assert_eq!(startup_mode(Some(false), workflow), ViewMode::Studio);
        }
    }
    #[test]
    fn environment_override_preserves_none_and_only_widens_furnished_rooms() {
        use waifu_studio::environments::EnvironmentKind::*;
        for saved in [false, true] {
            assert_eq!(environment_room_view(Disabled, saved), saved);
            for kind in [CozyRoom, MoonlitGarden, NeonRooftop] {
                assert!(!environment_room_view(kind, saved));
            }
            for kind in [RoseBedroom, VioletStudio, CyanLounge] {
                assert!(environment_room_view(kind, saved));
            }
        }
    }
}
