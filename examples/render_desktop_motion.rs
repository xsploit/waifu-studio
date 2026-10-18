//! Offscreen GPU proof of the production VRM importer and humanoid retargeter.
//! No desktop window, audio device, room geometry, provider or network client.
//!
//! cargo run --example render_desktop_motion -- sitting /tmp/sitting.png --time 0.5
//! Usage: <idle|sitting|grabbed|LOCAL_MOTION_PATH> <output.png>
//!        [--time SECONDS] [--frames 180] [--avatar LOCAL_VRM_PATH]
use bevy::{
    app::{AppExit, ScheduleRunnerPlugin},
    asset::{AssetMetaCheck, AssetPath, UnapprovedPathMode},
    camera::{RenderTarget, ShadowLodOrigin},
    prelude::*,
    render::{
        render_resource::{TextureFormat, TextureUsages},
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    time::TimeUpdateStrategy,
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use waifu_studio::{
    animation::{AnimationState, BvhAnimationPlugin, BvhClip, parse_bvh, parse_fbx, parse_vrma},
    loading::{AvatarLoadState, LoadAvatar, LoadStatus, VrmLoaderPlugin},
};

const MAX_MOTION_BYTES: u64 = 32 * 1024 * 1024;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 960;

struct Options {
    motion: PathBuf,
    output: PathBuf,
    avatar: PathBuf,
    seconds: f32,
    warmup: u32,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut args = std::env::args().skip(1);
        let motion = match args.next().as_deref() {
            Some("idle") => root.join("assets/animations/desktop/idle-maid.vrma"),
            Some("sitting") => root.join("assets/animations/desktop/idle-sitting.vrma"),
            Some("grabbed" | "drag") => root.join("assets/animations/desktop/grabbed.vrma"),
            Some(path) if !path.starts_with('-') => PathBuf::from(path),
            _ => return Err("Usage: render_desktop_motion <idle|sitting|grabbed|LOCAL_MOTION_PATH> <output.png> [--time SECONDS] [--frames 180] [--avatar LOCAL_VRM_PATH]".into()),
        };
        let output = args
            .next()
            .map(PathBuf::from)
            .ok_or("Missing PNG output path")?;
        if !output
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("png"))
        {
            return Err("Output must have a .png extension".into());
        }
        let mut options = Self {
            motion,
            output,
            avatar: root.join("assets/characters/models/4e5c5188c39e-neuro-sama.vrm"),
            seconds: 0.5,
            warmup: 180,
        };
        while let Some(option) = args.next() {
            match option.as_str() {
                "--time" => {
                    options.seconds = args
                        .next()
                        .and_then(|s| s.parse::<f32>().ok())
                        .filter(|s| s.is_finite() && *s >= 0.)
                        .ok_or("--time requires a finite nonnegative number")?;
                }
                "--frames" => {
                    options.warmup = args
                        .next()
                        .and_then(|s| s.parse::<u32>().ok())
                        .filter(|n| (60..=600).contains(n))
                        .ok_or("--frames requires 60..600")?;
                }
                "--avatar" => {
                    options.avatar = args
                        .next()
                        .map(PathBuf::from)
                        .ok_or("--avatar requires a local VRM path")?
                }
                _ => return Err(format!("Unexpected argument: {option}")),
            }
        }
        options.motion = options
            .motion
            .canonicalize()
            .map_err(|e| format!("Cannot resolve motion: {e}"))?;
        options.avatar = options
            .avatar
            .canonicalize()
            .map_err(|e| format!("Cannot resolve avatar: {e}"))?;
        if !options.avatar.is_file()
            || !options
                .avatar
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "vrm" | "glb"))
        {
            return Err("Avatar must be a local .vrm or .glb file".into());
        }
        Ok(options)
    }

    fn clip(&self) -> Result<BvhClip, String> {
        let file =
            std::fs::File::open(&self.motion).map_err(|e| format!("Cannot open motion: {e}"))?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > MAX_MOTION_BYTES {
            return Err("Motion must be a regular local file at most 32 MiB".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_MOTION_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_MOTION_BYTES {
            return Err("Motion exceeds 32 MiB".into());
        }
        let clip = match self
            .motion
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "vrma" => parse_vrma(&bytes),
            "fbx" => parse_fbx(&bytes),
            "bvh" => parse_bvh(std::str::from_utf8(&bytes).map_err(|_| "BVH must be UTF-8 text")?),
            _ => return Err("Motion must be a local .vrma, .fbx or .bvh file".into()),
        }?;
        if self.seconds > clip.duration() {
            return Err(format!(
                "Requested sample {:.3}s exceeds clip duration {:.3}s",
                self.seconds,
                clip.duration()
            ));
        }
        Ok(clip)
    }
}

#[derive(Resource)]
struct Capture {
    options: Options,
    ready_frames: u32,
    frames: u32,
    next_attempt: u32,
    started: Instant,
    requested: bool,
    target: Handle<Image>,
}

fn main() -> AppExit {
    let options = match Options::parse() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return AppExit::error();
        }
    };
    let clip = match options.clip() {
        Ok(clip) => clip,
        Err(error) => {
            eprintln!("Motion import failed: {error}");
            return AppExit::error();
        }
    };
    if let Some(parent) = options
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("Cannot create output directory: {error}");
        return AppExit::error();
    }
    println!(
        "Production {} retarget proof: {} -> {} at {:.3}s ({} mapped humanoid bones)",
        clip.format(),
        options.motion.display(),
        options.avatar.display(),
        options.seconds,
        clip.mapped_bones()
    );
    // Activate the real retargeter without desktop behavior's entry crossfade.
    // PostUpdate pins the requested sample after the loader has selected a root;
    // the next Update applies it through BvhAnimationPlugin's actual rig code.
    let mut animation = AnimationState::default();
    animation.clip = Some(Arc::new(clip));
    animation.path = Some(options.motion.clone());
    animation.looping = false;
    animation.play();
    animation.pause();
    App::new()
        .insert_resource(Capture {
            options,
            ready_frames: 0,
            frames: 0,
            next_attempt: 0,
            started: Instant::now(),
            requested: false,
            target: default(),
        })
        .insert_resource(animation)
        .insert_resource(ClearColor(Color::NONE))
        .insert_resource(GlobalAmbientLight {
            color: Color::srgb(0.85, 0.88, 1.0),
            brightness: 400.,
            ..default()
        })
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: format!("{}/assets", env!("CARGO_MANIFEST_DIR")),
                    meta_check: AssetMetaCheck::Never,
                    unapproved_path_mode: UnapprovedPathMode::Allow,
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                })
                .disable::<WinitPlugin>()
                .disable::<bevy::audio::AudioPlugin>(),
        )
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1. / 60.,
        )))
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
            1. / 60.,
        )))
        .add_plugins((VrmLoaderPlugin, BvhAnimationPlugin))
        .add_systems(Startup, setup)
        .add_systems(PostUpdate, prepare_and_capture)
        .run()
}

fn setup(
    mut commands: Commands,
    mut state: ResMut<Capture>,
    mut images: ResMut<Assets<Image>>,
    mut requests: MessageWriter<LoadAvatar>,
) {
    let mut image = Image::new_target_texture(WIDTH, HEIGHT, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    state.target = images.add(image);
    commands.spawn((
        bevy_shader_mtoon::MtoonSun,
        DirectionalLight {
            illuminance: 10000.,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(3., 6., 4.).looking_at(Vec3::new(0., 1., 0.), Vec3::Y),
    ));
    commands.spawn((
        Camera3d::default(),
        ShadowLodOrigin,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
        RenderTarget::Image(state.target.clone().into()),
        Projection::Perspective(PerspectiveProjection {
            fov: 45_f32.to_radians(),
            ..default()
        }),
        Transform::from_xyz(0., 1., 3.2).looking_at(Vec3::new(0., 0.9, 0.), Vec3::Y),
    ));
    requests.write(LoadAvatar(
        AssetPath::from_path(&state.options.avatar).into_owned(),
    ));
}

fn prepare_and_capture(
    mut commands: Commands,
    mut state: ResMut<Capture>,
    avatar: Res<AvatarLoadState>,
    mut animation: ResMut<AnimationState>,
    mut exit: MessageWriter<AppExit>,
) {
    state.frames += 1;
    if state.started.elapsed() > Duration::from_secs(120)
        || state.frames > state.options.warmup + 2400
    {
        eprintln!(
            "Offscreen motion render timed out after {} frames",
            state.frames
        );
        exit.write(AppExit::error());
        return;
    }
    match &avatar.status {
        LoadStatus::Failed(error) => {
            eprintln!("Avatar load failed: {error}");
            exit.write(AppExit::error());
            return;
        }
        LoadStatus::Ready if avatar.active.is_some() => state.ready_frames += 1,
        _ => {
            state.ready_frames = 0;
            return;
        }
    }
    if let Some(error) = &animation.error {
        eprintln!("Production retargeter failed: {error}");
        exit.write(AppExit::error());
        return;
    }
    animation.position = state.options.seconds;
    animation.pause();
    if state.requested
        || state.ready_frames < state.options.warmup
        || state.frames < state.next_attempt
    {
        return;
    }
    state.requested = true;
    commands
        .spawn(Screenshot::image(state.target.clone()))
        .observe(save_capture);
}

fn save_capture(
    event: On<ScreenshotCaptured>,
    mut state: ResMut<Capture>,
    mut exit: MessageWriter<AppExit>,
) {
    let image = match event.image.clone().try_into_dynamic() {
        Ok(image) => image.to_rgba8(),
        Err(error) => {
            eprintln!("Cannot convert RGBA GPU capture: {error}");
            exit.write(AppExit::error());
            return;
        }
    };
    let visible = image.pixels().filter(|pixel| pixel.0[3] > 16).count();
    if visible < 1024 {
        // Shader compilation can outlast a frame warmup. A transparent clear
        // frame is not evidence of an avatar render; wait for a real GPU image.
        state.requested = false;
        state.next_attempt = state.frames + 60;
        return;
    }
    let corners = [
        (0, 0),
        (WIDTH - 1, 0),
        (0, HEIGHT - 1),
        (WIDTH - 1, HEIGHT - 1),
    ];
    if corners.iter().any(|&(x, y)| image.get_pixel(x, y).0[3] > 2) {
        eprintln!(
            "Capture failed transparency check: expected transparent corners, got an opaque background"
        );
        exit.write(AppExit::error());
        return;
    }
    match image.save_with_format(&state.options.output, image::ImageFormat::Png) {
        Ok(()) => {
            println!(
                "RGBA GPU motion proof saved: {} at {:.3}s ({} visible pixels, {} ready frames)",
                state.options.output.display(),
                state.options.seconds,
                visible,
                state.ready_frames
            );
            exit.write(AppExit::Success);
        }
        Err(error) => {
            eprintln!("Cannot save GPU capture: {error}");
            exit.write(AppExit::error());
        }
    }
}
