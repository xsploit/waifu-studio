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
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use waifu_studio::{
    animation::AnimationState,
    environments::EnvironmentKind,
    expressions::AudioEnvelope,
    loading::{AvatarLoadState, LoadStatus},
    metered_audio::{MeteredDecoder, SpeechMeter},
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
        _ => Err("Unknown showcase screen".into()),
    }
}
fn environment(name: &str) -> Result<EnvironmentKind, String> {
    match name {
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
        if !(15..=30).contains(&plan.fps) || plan.shots.is_empty() || plan.shots.len() > 24 {
            return Err("Showcase needs 15–30 fps and 1–24 shots".into());
        }
        let total = plan.shots.iter().try_fold(0usize, |n, s| {
            screen(&s.screen)?;
            environment(&s.environment)?;
            let meta = std::fs::metadata(&s.audio).map_err(|_| "Missing showcase narration")?;
            if s.frames == 0 || !meta.is_file() || meta.len() > 64 * 1024 * 1024 {
                return Err("Invalid showcase shot".to_string());
            }
            let file =
                std::fs::File::open(&s.audio).map_err(|_| "Cannot read showcase narration")?;
            rodio::Decoder::try_from(file).map_err(|_| "Showcase narration does not decode")?;
            Ok(n + s.frames as usize)
        })?;
        if total > plan.fps as usize * 180 {
            return Err("Showcase exceeds 180 seconds".into());
        }
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
        })
    }
    pub(crate) fn fps(&self) -> u32 {
        self.plan.fps
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
    meter: Res<SpeechMeter>,
    envelope: Res<AudioEnvelope>,
    meshes: Res<Assets<Mesh>>,
    images: Res<Assets<Image>>,
    mut exit: MessageWriter<AppExit>,
) {
    if capture.failed.load(Ordering::Relaxed)
        || capture.started.elapsed() > Duration::from_secs(1800)
        || matches!(avatar.status, LoadStatus::Failed(_))
    {
        error!("Showcase capture failed or timed out");
        exit.write(AppExit::error());
        return;
    }
    if capture.shot >= capture.plan.shots.len() {
        envelope.silence();
        meter.stop();
        if capture.completed.load(Ordering::Acquire) == capture.total {
            info!(
                "STUDIO_SHOWCASE {} frames at {} fps",
                capture.total, capture.plan.fps
            );
            exit.write(AppExit::Success);
        }
        return;
    }
    if capture.active != Some(capture.shot) {
        let shot = &capture.plan.shots[capture.shot];
        if let Some(character) = shot.character {
            if !studio.library.characters.iter().any(|c| c.id == character) {
                error!("Showcase character is absent from the isolated profile");
                exit.write(AppExit::error());
                return;
            }
            if studio.library.selected != Some(character) {
                studio.select_character(character);
            }
        }
        studio.tab = screen(&shot.screen).expect("validated screen");
        studio.library.scene.environment = environment(&shot.environment).expect("validated room");
        studio.library.scene.room_view = true;
        studio.frame_requested = true;
        if let Some(motion) = &shot.motion
            && let Err(error) = animation.load_path(motion)
        {
            error!("Showcase motion: {error}");
            exit.write(AppExit::error());
            return;
        }
        *capture.decoder.lock().unwrap() = None;
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
    if capture.frame == 0 {
        // Selection queues the ordinary voice cancellation. Start this test
        // source only after that cancellation and the avatar load have settled,
        // so a stale stop cannot invalidate the new meter generation.
        let Ok(bytes) = std::fs::read(&capture.plan.shots[capture.shot].audio) else {
            exit.write(AppExit::error());
            return;
        };
        *capture.decoder.lock().unwrap() = Some(
            meter
                .buffered(AudioSource {
                    bytes: bytes.into(),
                })
                .decoder(),
        );
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
    if let Some(decoder) = capture.decoder.lock().unwrap().as_mut() {
        let n = u64::from(decoder.sample_rate().get()) * u64::from(decoder.channels().get());
        let frame = u64::from(capture.frame);
        let fps = u64::from(capture.plan.fps);
        for _ in 0..((frame + 1) * n / fps - frame * n / fps) {
            if decoder.next().is_none() {
                break;
            }
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
}
