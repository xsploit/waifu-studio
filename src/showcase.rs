//! Opt-in, windowless recording of the real renderer. No desktop input or speakers.
//! Narration is consumed through the production audio meter at the video frame rate.
use crate::studio::{Studio, Tab};
use bevy::{
    audio::Decodable,
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
};
use rodio::Source;
use serde::Deserialize;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use waifu_studio::{
    animation::{AnimationState, BvhClip, parse_bvh, parse_fbx, parse_vrma},
    environments::EnvironmentKind,
    expressions::AudioEnvelope,
    extension_runtime::ExtensionRuntime,
    loading::{AvatarLoadState, LoadStatus},
    metered_audio::{MeteredDecoder, MeteredSpeech, SpeechMeter},
    viseme_audio::VisemeState,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    fps: u32,
    frames_dir: PathBuf,
    shots: Vec<Shot>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Shot {
    frames: u32,
    screen: String,
    audio: PathBuf,
    environment: String,
    motion: Option<PathBuf>,
    /// Optional existing character in the isolated capture profile. Uses the
    /// ordinary selection/load path, including voice cancellation and cleanup.
    character: Option<uuid::Uuid>,
    /// Diagnostic timeline using the production preview and playlist paths.
    /// All shots must use probes when this mode is selected.
    probe: Option<MotionProbe>,
    /// Explicit saved-reaction invocation using the isolated profile's existing
    /// installation, enabled state, grants and settings. No controller bypass.
    extension_test: Option<ExtensionTest>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtensionTest {
    installation_id: uuid::Uuid,
    /// Zero-based captured frame, after loading and settling.
    at_frame: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MotionProbe {
    idle: PathBuf,
    talking: PathBuf,
    reaction: PathBuf,
    speech_delay_frames: u32,
    reaction_delay_frames: u32,
    stop_speech_frame: Option<u32>,
    settings_frames: Option<[u32; 2]>,
}
fn probe_clip(path: &Path) -> Result<Arc<BvhClip>, String> {
    const LIMIT: u64 = 32 * 1024 * 1024;
    let mut bytes = Vec::new();
    let file = std::fs::File::open(path).map_err(|_| "Cannot read probe motion")?;
    if !file
        .metadata()
        .map_err(|_| "Cannot inspect probe motion")?
        .is_file()
    {
        return Err("Probe motion must be a regular file".into());
    }
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read probe motion")?;
    if bytes.len() as u64 > LIMIT {
        return Err("Probe motion exceeds 32 MiB".into());
    }
    match path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("bvh") => {
            parse_bvh(std::str::from_utf8(&bytes).map_err(|_| "Probe BVH must be UTF-8")?)
        }
        Some("vrma") => parse_vrma(&bytes),
        Some("fbx") => parse_fbx(&bytes),
        _ => Err("Probe motion must be BVH, VRMA or FBX".into()),
    }
    .map(Arc::new)
}
fn screen(name: &str) -> Result<Tab, String> {
    match name {
        "chat" => Ok(Tab::Chat),
        "characters" => Ok(Tab::Characters),
        "scene" => Ok(Tab::Scene),
        "connections" => Ok(Tab::Connections),
        "voice" => Ok(Tab::Voice),
        "memory" => Ok(Tab::Memory),
        "tools" => Ok(Tab::Tools),
        "tasks" => Ok(Tab::Tasks),
        "extensions" => Ok(Tab::Extensions),
        _ => Err("Unknown showcase screen".into()),
    }
}
fn environment(name: &str) -> Result<EnvironmentKind, String> {
    match name {
        "none" => Ok(EnvironmentKind::Disabled),
        "rose-bedroom" => Ok(EnvironmentKind::RoseBedroom),
        "violet-studio" => Ok(EnvironmentKind::VioletStudio),
        "cyan-lounge" => Ok(EnvironmentKind::CyanLounge),
        _ => Err("Unknown showcase room".into()),
    }
}
#[derive(Resource)]
pub(crate) struct Showcase {
    plan: Plan,
    shot: usize,
    active: Option<usize>,
    frame: u32,
    total: usize,
    requested: usize,
    settle: u32,
    decoder: Mutex<Option<MeteredDecoder>>,
    completed: Arc<AtomicUsize>,
    failed: Arc<AtomicBool>,
    started: Instant,
    peak_audio: f32,
    probe_reactions: Vec<Option<Arc<BvhClip>>>,
    speech_entity: Option<Entity>,
    audio_frame: u32,
    trace_pending: Option<(usize, usize, u32)>,
    trace: Vec<serde_json::Value>,
    trace_enabled: bool,
    extension_test_invocation: Option<uuid::Uuid>,
    extension_scene_restore: Option<(bool, bool, bool)>,
}
impl Showcase {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let metadata = std::fs::metadata(path).map_err(|_| "Cannot read showcase plan")?;
        if !metadata.is_file() || metadata.len() > 65536 {
            return Err("Showcase plan too large".into());
        }
        let plan: Plan =
            serde_json::from_slice(&std::fs::read(path).map_err(|_| "Cannot read showcase plan")?)
                .map_err(|_| "Invalid showcase plan")?;
        if !(5..=30).contains(&plan.fps) || plan.shots.is_empty() || plan.shots.len() > 24 {
            return Err("Showcase needs 5–30 fps and 1–24 shots".into());
        }
        let probes = plan.shots.iter().filter(|s| s.probe.is_some()).count();
        if probes > 0 && probes != plan.shots.len() {
            return Err("Do not mix manual showcase shots with motion probes".into());
        }
        let total = plan.shots.iter().try_fold(0usize, |n, s| {
            screen(&s.screen)?;
            environment(&s.environment)?;
            if let Some(test) = &s.extension_test
                && (test.installation_id.is_nil()
                    || test.at_frame >= s.frames
                    || s.motion.is_some()
                    || s.probe.is_some())
            {
                return Err(
                    "Invalid extension test identity, timeline or mixed motion/probe".into(),
                );
            }
            let meta = std::fs::metadata(&s.audio).map_err(|_| "Missing showcase narration")?;
            if s.frames == 0 || !meta.is_file() || meta.len() > 64 * 1024 * 1024 {
                return Err("Invalid showcase shot".to_string());
            }
            if let Some(probe) = &s.probe {
                if s.motion.is_some()
                    || probe.speech_delay_frames >= s.frames
                    || probe.reaction_delay_frames >= s.frames
                    || probe
                        .stop_speech_frame
                        .is_some_and(|stop| stop <= probe.speech_delay_frames || stop >= s.frames)
                    || probe
                        .settings_frames
                        .is_some_and(|[open, close]| open >= close || close >= s.frames)
                    || [&probe.idle, &probe.talking, &probe.reaction]
                        .iter()
                        .any(|p| !p.is_absolute())
                {
                    return Err("Invalid motion probe paths or timeline".into());
                }
                probe_clip(&probe.idle)?;
                probe_clip(&probe.talking)?;
            }
            let file =
                std::fs::File::open(&s.audio).map_err(|_| "Cannot read showcase narration")?;
            rodio::Decoder::try_from(file).map_err(|_| "Showcase narration does not decode")?;
            Ok(n + s.frames as usize)
        })?;
        if total > plan.fps as usize * 180 {
            return Err("Showcase exceeds 180 seconds".into());
        }
        let probe_reactions = plan
            .shots
            .iter()
            .map(|s| {
                s.probe
                    .as_ref()
                    .map(|p| probe_clip(&p.reaction))
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let trace_enabled = probes > 0 || plan.shots.iter().any(|s| s.extension_test.is_some());
        std::fs::create_dir_all(&plan.frames_dir).map_err(|_| "Cannot create frame directory")?;
        if std::fs::read_dir(&plan.frames_dir)
            .map_err(|_| "Cannot read frame directory")?
            .next()
            .is_some()
        {
            return Err("Showcase frame directory must be empty".into());
        }
        Ok(Self {
            plan,
            shot: 0,
            active: None,
            frame: 0,
            total,
            requested: 0,
            settle: 60,
            decoder: Mutex::new(None),
            completed: Arc::default(),
            failed: Arc::default(),
            started: Instant::now(),
            peak_audio: 0.0,
            probe_reactions,
            speech_entity: None,
            audio_frame: 0,
            trace_pending: None,
            trace: Vec::new(),
            trace_enabled,
            extension_test_invocation: None,
            extension_scene_restore: None,
        })
    }
    pub(crate) fn fps(&self) -> u32 {
        self.plan.fps
    }

    fn write_trace(&self, failed: bool) -> Result<(), String> {
        if !self.trace_enabled {
            return Ok(());
        }
        let trace = serde_json::json!({
            "failed": failed,
            "scope": if failed {
                "FAILED/incomplete offscreen capture; retained frame observations only, not success or playback proof; screenshot completion counts are separate from trace frames; no private protocol payloads"
            } else if self.plan.shots.iter().any(|s| s.extension_test.is_some()) {
                "offscreen app renderer and explicit installed test process; current saved settings/grants; recorded narration through production preview/meter/visemes; process completion is not playback proof; no automatic install, enabling, grants, credential changes or network/provider setup; no speakers or desktop input"
            } else {
                "offscreen app renderer; recorded audio consumed through production preview/meter/viseme paths; no provider, speakers or physical desktop input"
            },
            "fps": self.plan.fps,
            "expected_frames": self.total,
            "trace_frames": self.trace.len(),
            "requested_screenshots": self.requested,
            "completed_screenshot_callbacks": self.completed.load(Ordering::Acquire),
            "frames": self.trace,
        });
        std::fs::write(self.plan.frames_dir.join("trace.json"), trace.to_string())
            .map_err(|_| "Cannot save showcase motion trace".into())
    }

    fn fail(&self, exit: &mut MessageWriter<AppExit>) {
        self.failed.store(true, Ordering::Release);
        if let Err(error) = self.write_trace(true) {
            error!("{error}");
        }
        exit.write(AppExit::error());
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record(
    mut commands: Commands,
    mut capture: ResMut<Showcase>,
    target: Res<crate::Capture>,
    avatar: Res<AvatarLoadState>,
    mut studio: ResMut<Studio>,
    mut animation: ResMut<AnimationState>,
    host: (
        Res<ExtensionRuntime>,
        Res<crate::motion_services::MotionServices>,
    ),
    meter: Res<SpeechMeter>,
    envelope: Res<AudioEnvelope>,
    meshes: Res<Assets<Mesh>>,
    images: Res<Assets<Image>>,
    mut audio: ResMut<Assets<MeteredSpeech>>,
    mut visemes: ResMut<VisemeState>,
    mut speech_target: ResMut<waifu_studio::expressions::SpeechTarget>,
    playing: Query<Entity, With<crate::studio::SpokenAudio>>,
    mut exit: MessageWriter<AppExit>,
) {
    let (runtime, services) = host;
    if capture.failed.load(Ordering::Relaxed)
        || capture.started.elapsed() > Duration::from_secs(1800)
        || matches!(avatar.status, LoadStatus::Failed(_))
    {
        error!("Showcase capture failed or timed out");
        capture.fail(&mut exit);
        return;
    }
    if capture.shot >= capture.plan.shots.len() {
        envelope.silence();
        meter.stop();
        if capture.completed.load(Ordering::Acquire) == capture.total {
            if capture.trace_enabled && capture.trace.len() != capture.total {
                error!("Incomplete showcase motion trace");
                capture.fail(&mut exit);
                return;
            }
            if let Err(error) = capture.write_trace(false) {
                error!("{error}");
                capture.fail(&mut exit);
                return;
            }
            info!(
                "STUDIO_SHOWCASE {} frames at {} fps",
                capture.total, capture.plan.fps
            );
            exit.write(AppExit::Success);
        }
        return;
    }
    if capture.active != Some(capture.shot) {
        if let Some((portrait, companion_enabled, companion_paused)) =
            capture.extension_scene_restore.take()
        {
            studio.library.scene.portrait = portrait;
            studio.library.scene.companion_motion.enabled = companion_enabled;
            if companion_paused {
                crate::motion_services::lock(&services.companion).manual_override();
            }
        }
        capture.extension_test_invocation = None;
        let shot = &capture.plan.shots[capture.shot];
        if let Some(character) = shot.character {
            if !studio.library.characters.iter().any(|c| c.id == character) {
                error!("Showcase character is absent from the isolated profile");
                capture.fail(&mut exit);
                return;
            }
            if studio.library.selected != Some(character) {
                studio.select_character(character);
            }
        }
        studio.tab = screen(&shot.screen).expect("validated screen");
        studio.library.scene.environment = environment(&shot.environment).expect("validated room");
        studio.library.scene.room_view = true;
        if let Some(probe) = &shot.probe {
            studio.stop_voice();
            studio.library.scene.room_view = false;
            studio.library.scene.portrait = false;
            studio.library.scene.companion_motion.enabled = true;
            studio.library.scene.companion_motion.idle_paths = vec![probe.idle.clone()];
            studio.library.scene.companion_motion.talking_paths = vec![probe.talking.clone()];
            animation.resume_automatic_control();
        }
        let extension_scene_restore = if shot.extension_test.is_some() {
            studio.stop_voice();
            let previous = (
                studio.library.scene.portrait,
                studio.library.scene.companion_motion.enabled,
                crate::motion_services::lock(&services.companion).paused,
            );
            studio.library.scene.room_view = false;
            studio.library.scene.portrait = true;
            studio.library.scene.companion_motion.enabled = true;
            crate::motion_services::lock(&services.companion).resume();
            animation.resume_automatic_control();
            Some(previous)
        } else {
            None
        };
        studio.frame_requested = true;
        if let Some(motion) = &shot.motion
            && let Err(error) = animation.load_path(motion)
        {
            error!("Showcase motion: {error}");
            capture.fail(&mut exit);
            return;
        }
        capture.extension_scene_restore = extension_scene_restore;
        *capture.decoder.lock().unwrap() = None;
        if let Some(entity) = capture.speech_entity.take() {
            commands.entity(entity).try_despawn();
        }
        capture.audio_frame = 0;
        meter.stop();
        capture.active = Some(capture.shot);
        capture.frame = 0;
        capture.peak_audio = 0.0;
        capture.settle = if capture.shot == 0 { 60 } else { 12 };
    }
    if avatar.status != LoadStatus::Ready || studio.requested_model.is_some() {
        return;
    }
    if capture.plan.shots[capture.shot].character.is_some()
        && !studio
            .library
            .selected_character()
            .is_some_and(|character| {
                avatar
                    .requested_path
                    .as_deref()
                    .is_some_and(|path| Path::new(path) == character.model)
            })
    {
        // The old avatar remains visible while a replacement loads. Never
        // mistake its Ready state for completion of the requested switch.
        return;
    }
    if capture.settle > 0 {
        capture.settle -= 1;
        return;
    }
    let Some(target) = &target.target else {
        return;
    };
    if let Some(test) = &capture.plan.shots[capture.shot].extension_test
        && capture.frame == test.at_frame
    {
        // Settling returns through ordinary Update before frame zero; the shared
        // runtime has observed this loaded target before start_test pins it.
        if studio.animation_target(&avatar).is_none() {
            error!("Showcase extension test needs the loaded animation target");
            capture.fail(&mut exit);
            return;
        }
        match runtime.start_test(test.installation_id) {
            Ok(invocation) => {
                info!(
                    "STUDIO_SHOWCASE_EXTENSION_TEST admitted installation={} invocation={}; process completion is not playback proof",
                    test.installation_id, invocation
                );
                capture.extension_test_invocation = Some(invocation);
            }
            Err(error) => {
                error!("Showcase extension test admission failed: {error}");
                capture.fail(&mut exit);
                return;
            }
        }
    }
    let probe = capture.plan.shots[capture.shot].probe.as_ref();
    let start_audio = probe.map_or(capture.frame == 0, |p| {
        capture.frame == p.speech_delay_frames
    });
    if let Some(probe) = probe {
        if capture.frame == probe.reaction_delay_frames {
            animation.select_one_shot(
                capture.probe_reactions[capture.shot]
                    .as_ref()
                    .unwrap()
                    .clone(),
                Some(probe.reaction.clone()),
            );
        }
        if let Some([open, close]) = probe.settings_frames {
            if capture.frame == open {
                studio.tab = Tab::Scene;
            }
            if capture.frame == close {
                studio.tab = Tab::Chat;
            }
        }
        if probe.stop_speech_frame == Some(capture.frame) {
            studio.stop_voice();
            *capture.decoder.lock().unwrap() = None;
            capture.speech_entity = None;
            meter.stop();
        }
    }
    if start_audio {
        // Selection queues the ordinary voice cancellation. Start this test
        // source only after that cancellation and the avatar load have settled,
        // so a stale stop cannot invalidate the new meter generation.
        let Ok(bytes) = std::fs::read(&capture.plan.shots[capture.shot].audio) else {
            capture.fail(&mut exit);
            return;
        };
        let source = AudioSource {
            bytes: bytes.into(),
        };
        let source = if capture.plan.shots[capture.shot].probe.is_some()
            || capture.plan.shots[capture.shot].extension_test.is_some()
        {
            let (entity, source) = crate::studio::play_preview_audio(
                &mut commands,
                &mut studio,
                &mut audio,
                &meter,
                &mut visemes,
                playing.iter(),
                source,
            );
            speech_target.0 = studio.speaking_character();
            capture.speech_entity = Some(entity);
            source
        } else {
            meter.buffered(source)
        };
        *capture.decoder.lock().unwrap() = Some(source.decoder());
        capture.audio_frame = 0;
        let (logical, unique) = waifu_studio::loading::morph_cpu_bytes(&meshes);
        info!(
            "STUDIO_SHOWCASE_SHOT {} avatar={:?} meshes={} images={} morph_logical_mib={:.2} morph_unique_mib={:.2}",
            capture.shot,
            avatar.active,
            meshes.len(),
            images.len(),
            logical as f64 / (1024.0 * 1024.0),
            unique as f64 / (1024.0 * 1024.0),
        );
    }
    // Exact interleaved sample count per frame avoids clock drift at arbitrary rates.
    let mut ended = false;
    if let Some(decoder) = capture.decoder.lock().unwrap().as_mut() {
        let n = u64::from(decoder.sample_rate().get()) * u64::from(decoder.channels().get());
        let frame = u64::from(capture.audio_frame);
        let fps = u64::from(capture.plan.fps);
        for _ in 0..((frame + 1) * n / fps - frame * n / fps) {
            if decoder.next().is_none() {
                ended = true;
                break;
            }
        }
    }
    capture.audio_frame += 1;
    if ended {
        *capture.decoder.lock().unwrap() = None;
        if let Some(entity) = capture.speech_entity.take() {
            commands.entity(entity).try_despawn();
        }
    }
    let level = meter.level();
    capture.peak_audio = capture.peak_audio.max(level);
    envelope.set(level);
    let path = capture
        .plan
        .frames_dir
        .join(format!("frame-{:06}.jpg", capture.requested));
    let completed = capture.completed.clone();
    let failed = capture.failed.clone();
    commands.spawn(Screenshot::image(target.clone())).observe(
        move |event: On<ScreenshotCaptured>| {
            let result = event
                .image
                .clone()
                .try_into_dynamic()
                .map_err(|e| e.to_string())
                .and_then(|image| {
                    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
                    image::codecs::jpeg::JpegEncoder::new_with_quality(
                        std::io::BufWriter::new(file),
                        90,
                    )
                    .encode_image(&image::DynamicImage::ImageRgb8(image.to_rgb8()))
                    .map_err(|e| e.to_string())
                });
            if result.is_err() {
                failed.store(true, Ordering::Release);
            }
            completed.fetch_add(1, Ordering::Release);
        },
    );
    capture.requested += 1;
    if capture.trace_enabled {
        capture.trace_pending = Some((capture.requested - 1, capture.shot, capture.frame));
    }
    capture.frame += 1;
    if capture.frame >= capture.plan.shots[capture.shot].frames {
        info!(
            "STUDIO_SHOWCASE_SHOT_END {} peak_audio={:.4}",
            capture.shot, capture.peak_audio,
        );
        capture.shot += 1;
        capture.active = None;
    }
}

/// Observe final app state after the production animator and facial systems.
#[allow(clippy::too_many_arguments)] // Bevy injects each independent resource/query.
pub(crate) fn trace(
    mut capture: ResMut<Showcase>,
    studio: Res<Studio>,
    avatar: Res<AvatarLoadState>,
    animation: Res<AnimationState>,
    facial: Res<waifu_studio::expressions::FacialState>,
    physics: Res<waifu_studio::spring_physics::SpringPhysicsState>,
    bones: Query<&Transform>,
    playing: Query<&crate::studio::SpeechPlayback>,
    camera: Query<&Transform, With<crate::runtime::StudioCamera>>,
    runtime: Res<ExtensionRuntime>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some((index, shot, frame)) = capture.trace_pending.take() else {
        return;
    };
    let extension_test_installation = capture.plan.shots[shot]
        .extension_test
        .as_ref()
        .map(|test| test.installation_id);
    let extension_test_active = extension_test_installation.map(|id| runtime.test_active(id));
    let extension_test_status = extension_test_installation.and_then(|id| runtime.test_status(id));
    let extension_test_invocation = capture.extension_test_invocation;
    if extension_test_installation.is_some()
        && frame + 1 == capture.plan.shots[shot].frames
        && (capture.extension_test_invocation.is_none()
            || extension_test_active != Some(false)
            || extension_test_status.as_deref() != Some("Test process completed"))
    {
        error!(
            "Showcase extension test did not complete by shot end; active={extension_test_active:?} status={extension_test_status:?}; this check proves process completion only"
        );
        capture.failed.store(true, Ordering::Release);
    }
    let camera = camera.single().ok().map(|t| {
        serde_json::json!({
            "translation": t.translation.to_array(), "rotation": t.rotation.to_array(),
        })
    });
    let springs: Vec<_> = physics.available_joints.iter().filter(|joint| joint.selected).filter_map(|joint| {
        let transform = bones.get(joint.bone).ok()?;
        Some(serde_json::json!({
            "name": joint.name,
            "rotation": transform.rotation.to_array(),
            "finite": transform.translation.is_finite() && transform.rotation.is_finite() && transform.scale.is_finite(),
        }))
    }).collect();
    capture.trace.push(serde_json::json!({
        "index": index, "shot": shot, "frame": frame,
        "character": studio.library.selected,
        "avatar": avatar.active.map(Entity::to_bits),
        "screen": format!("{:?}", studio.tab),
        "motion": animation.path,
        "position": animation.position,
        "gesture": animation.gesture_active(),
        "motion_loading": animation.loading,
        "manual_motion": animation.manual_controlled(),
        "speech_playing": playing.iter().any(|source| studio.speech_is_playing(source)),
        "speech_character": studio.speaking_character(),
        "speech_generation": studio.speech_generation(),
        "speech_level": facial.speech_level,
        "mouth_weight": facial.mouth_weight,
        "physics_strength": physics.config.strength,
        "physics_enabled": physics.config.enabled,
        "physics_status": physics.status,
        "physics_available": physics.available_joints.len(),
        "physics_tuned": physics.available_joints.iter().filter(|joint| joint.tuned).count(),
        "chest_springs": springs,
        "camera": camera,
        "extension_test_installation": extension_test_installation,
        "extension_test_invocation": extension_test_invocation,
        "extension_test_active": extension_test_active,
        "extension_test_status": extension_test_status,
    }));
    // Retain this final observation before emitting AppExit: the next PreUpdate
    // may never run, so failure persistence cannot rely on record being called.
    if capture.failed.load(Ordering::Acquire) {
        capture.fail(&mut exit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(frames: u32) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("studio-showcase-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let audio = root.join("clip.wav");
        let mut wav = b"RIFF".to_vec();
        wav.extend(52u32.to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(16000u32.to_le_bytes());
        wav.extend(32000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(16u32.to_le_bytes());
        wav.extend([0u8; 16]);
        std::fs::write(&audio, wav).unwrap();
        let output = root.join("frames");
        let plan = root.join("plan.json");
        std::fs::write(&plan,serde_json::to_vec(&serde_json::json!({"fps":15,"frames_dir":output,
            "shots":[{"frames":frames,"screen":"chat","audio":audio,"environment":"rose-bedroom"}]})).unwrap()).unwrap();
        (root, plan, output)
    }
    #[test]
    fn existing_capture_files_are_never_overwritten() {
        let (root, plan, output) = fixture(15);
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("keep.txt"), b"keep").unwrap();
        assert!(Showcase::load(&plan).err().unwrap().contains("empty"));
        assert_eq!(std::fs::read(output.join("keep.txt")).unwrap(), b"keep");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_audio_and_excess_duration_fail_before_creating_frames() {
        let (root, plan, output) = fixture(2701);
        assert!(Showcase::load(&plan).err().unwrap().contains("180 seconds"));
        assert!(!output.exists());
        std::fs::write(root.join("clip.wav"), b"invalid").unwrap();
        assert!(Showcase::load(&plan).err().unwrap().contains("decode"));
        assert!(!output.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn diagnostic_cadence_rejects_four_fps_and_accepts_five() {
        let (root, plan, output) = fixture(15);
        let mut data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&plan).unwrap()).unwrap();
        data["fps"] = 4.into();
        std::fs::write(&plan, data.to_string()).unwrap();
        assert_eq!(
            Showcase::load(&plan).err().unwrap(),
            "Showcase needs 5–30 fps and 1–24 shots"
        );
        assert!(!output.exists());
        data["fps"] = 5.into();
        std::fs::write(&plan, data.to_string()).unwrap();
        assert_eq!(Showcase::load(&plan).unwrap().fps(), 5);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failure_exit_preserves_partial_trace_with_explicit_failed_scope_and_cadence() {
        let (root, plan, output) = fixture(30);
        let mut data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&plan).unwrap()).unwrap();
        data["fps"] = 5.into();
        data["shots"][0]["extension_test"] = serde_json::json!({
            "installation_id": uuid::Uuid::new_v4(), "at_frame": 0,
        });
        std::fs::write(&plan, data.to_string()).unwrap();
        let mut showcase = Showcase::load(&plan).unwrap();
        showcase.requested = 1;
        showcase.trace.push(serde_json::json!({
            "index": 0, "shot": 0, "frame": 0,
            "extension_test_status": "Test process failed",
        }));
        let mut app = App::new();
        app.insert_resource(showcase)
            .add_message::<AppExit>()
            .add_systems(
                Update,
                |capture: Res<Showcase>, mut exit: MessageWriter<AppExit>| {
                    capture.fail(&mut exit);
                },
            );
        app.update();
        let trace: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.join("trace.json")).unwrap()).unwrap();
        assert_eq!(trace["failed"], true);
        assert!(
            trace["scope"]
                .as_str()
                .unwrap()
                .starts_with("FAILED/incomplete")
        );
        assert_eq!(trace["fps"], 5);
        assert_eq!(trace["expected_frames"], 30);
        assert_eq!(trace["trace_frames"], 1);
        assert_eq!(trace["requested_screenshots"], 1);
        assert_eq!(trace["completed_screenshot_callbacks"], 0);
        assert_eq!(trace["frames"].as_array().unwrap().len(), 1);
        assert_eq!(
            trace["frames"][0]["extension_test_status"],
            "Test process failed"
        );
        assert!(!app.world().resource::<Messages<AppExit>>().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extension_tests_reject_invalid_identity_timeline_and_controller_bypasses_upfront() {
        let (root, plan, output) = fixture(30);
        let mut data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&plan).unwrap()).unwrap();
        data["shots"][0]["extension_test"] = serde_json::json!({
            "installation_id": uuid::Uuid::new_v4(), "at_frame": 10,
        });
        let valid = data.clone();
        for invalid in 0..8 {
            let shot = &mut data["shots"][0];
            match invalid {
                0 => {
                    shot["extension_test"]["installation_id"] = uuid::Uuid::nil().to_string().into()
                }
                1 => shot["extension_test"]["installation_id"] = "invalid-id".into(),
                2 => shot["extension_test"]["at_frame"] = 30.into(),
                3 => shot["extension_test"]["at_frame"] = (-1).into(),
                4 => shot["extension_test"]["at_frame"] = u64::MAX.into(),
                5 => shot["extension_test"]["grant"] = "motion.play".into(),
                6 => shot["motion"] = "must-not-load.bvh".into(),
                _ => {
                    shot["probe"] = serde_json::json!({
                        "idle":"missing.bvh", "talking":"missing.bvh", "reaction":"missing.bvh",
                        "speech_delay_frames": 1, "reaction_delay_frames": 2,
                    })
                }
            }
            std::fs::write(&plan, data.to_string()).unwrap();
            assert!(Showcase::load(&plan).is_err());
            assert!(!output.exists());
            data = valid.clone();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extension_test_frame_bounds_and_mixed_shots_enable_complete_trace_without_starting_code() {
        for at_frame in [0, 29] {
            let (root, plan, _) = fixture(30);
            let mut data: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&plan).unwrap()).unwrap();
            let manual = data["shots"][0].clone();
            let installation = uuid::Uuid::new_v4();
            data["shots"][0]["screen"] = "extensions".into();
            data["shots"][0]["extension_test"] = serde_json::json!({
                "installation_id": installation, "at_frame": at_frame,
            });
            data["shots"].as_array_mut().unwrap().push(manual);
            std::fs::write(&plan, data.to_string()).unwrap();
            let showcase = Showcase::load(&plan).unwrap();
            assert!(showcase.trace_enabled);
            assert_eq!(showcase.total, 60);
            let test = showcase.plan.shots[0].extension_test.as_ref().unwrap();
            assert_eq!(test.installation_id, installation);
            assert_eq!(test.at_frame, at_frame);
            assert!(showcase.plan.shots[1].extension_test.is_none());
            assert!(showcase.extension_test_invocation.is_none());
            assert!(showcase.extension_scene_restore.is_none());
            assert!(showcase.probe_reactions.iter().all(Option::is_none));
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn probes_validate_motion_and_timeline_before_creating_output() {
        let (root, plan, output) = fixture(30);
        let motion = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/animations/companion/talking-nod.bvh");
        let mut data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&plan).unwrap()).unwrap();
        data["shots"][0]["probe"] = serde_json::json!({
            "idle":motion,"talking":motion,"reaction":motion,
            "speech_delay_frames":6,"reaction_delay_frames":3,
        });
        let valid = data.clone();
        for invalid in 0..5 {
            let shot = &mut data["shots"][0];
            match invalid {
                0 => shot["motion"] = motion.to_string_lossy().as_ref().into(),
                1 => shot["probe"]["speech_delay_frames"] = 30.into(),
                2 => shot["probe"]["stop_speech_frame"] = 5.into(),
                3 => shot["probe"]["settings_frames"] = serde_json::json!([20, 10]),
                _ => shot["probe"]["reaction"] = "missing.bvh".into(),
            }
            std::fs::write(&plan, data.to_string()).unwrap();
            assert!(Showcase::load(&plan).is_err());
            assert!(!output.exists());
            data = valid.clone();
        }
        data["shots"][0]["probe"]["reaction"] =
            root.join("broken.bvh").to_string_lossy().as_ref().into();
        std::fs::write(root.join("broken.bvh"), "invalid").unwrap();
        std::fs::write(&plan, data.to_string()).unwrap();
        assert!(Showcase::load(&plan).is_err());
        assert!(!output.exists());
        std::fs::write(&plan, valid.to_string()).unwrap();
        let showcase = Showcase::load(&plan).unwrap();
        assert!(showcase.probe_reactions[0].is_some());
        std::fs::remove_dir_all(root).unwrap();
    }
}
