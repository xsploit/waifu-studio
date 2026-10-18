//! Genuine offscreen GPU room preview; never creates a desktop window.
//! Run from the crate directory: render_rooms <rose|violet|cyan> <output.png> [--frames 180] [--avatar PATH] [--sitting]
use bevy::{
    app::{AppExit, ScheduleRunnerPlugin},
    asset::{AssetPath, UnapprovedPathMode},
    camera::{RenderTarget, ShadowLodOrigin},
    prelude::*,
    render::{
        render_resource::{TextureFormat, TextureUsages},
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use waifu_studio::{
    desktop_pose::{DesktopPosePlugin, DesktopPoseSettings},
    loading::{AvatarLoadState, LoadAvatar, LoadStatus, VrmLoaderPlugin},
    rooms::{RoomTextures, RoomTheme, build_room_textured},
    visuals::{VisualCamera, VisualPlugin, VisualPreset, VisualSettings, VisualState},
};

#[derive(Resource)]
struct Capture {
    theme: RoomTheme,
    avatar: Option<PathBuf>,
    avatar_ready_frames: u32,
    output: PathBuf,
    warmup: u32,
    frames: u32,
    started: Instant,
    requested: bool,
    target: Handle<Image>,
    art: Handle<Image>,
}
fn main() -> AppExit {
    let mut args = std::env::args().skip(1);
    let theme = match args.next().as_deref() {
        Some("rose") => RoomTheme::Rose,
        Some("violet") => RoomTheme::Violet,
        Some("cyan") => RoomTheme::Cyan,
        _ => {
            eprintln!(
                "Usage: render_rooms <rose|violet|cyan> <output.png> [--frames 180] [--avatar PATH] [--sitting]"
            );
            return AppExit::error();
        }
    };
    let Some(output) = args.next().map(PathBuf::from) else {
        eprintln!("Missing PNG output path");
        return AppExit::error();
    };
    if output.extension().and_then(|s| s.to_str()) != Some("png") {
        eprintln!("Output must have .png extension");
        return AppExit::error();
    }
    let mut warmup = 180;
    let mut avatar = None;
    let mut sitting = false;
    while let Some(option) = args.next() {
        match option.as_str() {
            "--frames" => {
                warmup = match args.next().and_then(|s| s.parse::<u32>().ok()) {
                    Some(n) if (30..=600).contains(&n) => n,
                    _ => {
                        eprintln!("--frames requires 30..600");
                        return AppExit::error();
                    }
                };
            }
            "--avatar" => {
                avatar = match args
                    .next()
                    .and_then(|path| std::fs::canonicalize(path).ok())
                    .filter(|p| p.is_file())
                {
                    Some(path) => Some(path),
                    None => {
                        eprintln!("--avatar requires an existing model file path");
                        return AppExit::error();
                    }
                };
            }
            "--sitting" => sitting = true,
            _ => {
                eprintln!("Unexpected argument: {option}");
                return AppExit::error();
            }
        }
    }
    if sitting && avatar.is_none() {
        eprintln!("--sitting requires --avatar PATH");
        return AppExit::error();
    }
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("Cannot create output directory: {e}");
        return AppExit::error();
    }
    App::new()
        .insert_resource(Capture {
            theme,
            avatar,
            avatar_ready_frames: 0,
            output,
            warmup,
            frames: 0,
            started: Instant::now(),
            requested: false,
            target: default(),
            art: default(),
        })
        .insert_resource(ClearColor(Color::srgb(0.025, 0.025, 0.04)))
        .insert_resource(GlobalAmbientLight {
            color: Color::srgb(0.85, 0.88, 1.0),
            brightness: 180.0,
            ..default()
        })
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: format!("{}/assets", env!("CARGO_MANIFEST_DIR")),
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
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .add_plugins((VrmLoaderPlugin, DesktopPosePlugin, VisualPlugin))
        .insert_resource(VisualState {
            settings: VisualSettings::preset(if matches!(theme, RoomTheme::Rose) {
                VisualPreset::SoftAnime
            } else {
                VisualPreset::Neon
            }),
            suppressed: false,
        })
        .insert_resource(DesktopPoseSettings { sitting })
        .add_systems(Startup, (setup, request_avatar).chain())
        .add_systems(Update, capture)
        .run()
}
fn setup(
    mut commands: Commands,
    mut state: ResMut<Capture>,
    assets: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut image = Image::new_target_texture(1280, 960, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    state.target = images.add(image);
    state.art = assets.load("rooms/wave-art.png");
    let layout = build_room_textured(
        state.theme,
        &mut meshes,
        &mut materials,
        &RoomTextures {
            wall_art: Some(state.art.clone()),
        },
    );
    for part in layout.parts {
        commands.spawn((
            Mesh3d(part.mesh),
            MeshMaterial3d(part.material),
            part.transform,
        ));
    }
    for (light, transform) in layout.lights {
        commands.spawn((light, transform));
    }
    commands.spawn((
        bevy_shader_mtoon::MtoonSun,
        DirectionalLight {
            illuminance: 1400.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(3.0, 6.0, 4.0).looking_at(Vec3::new(0.0, 1.0, -1.5), Vec3::Y),
    ));
    commands.spawn((
        Camera3d::default(),
        ShadowLodOrigin,
        RenderTarget::Image(state.target.clone().into()),
        VisualCamera,
        Projection::Perspective(PerspectiveProjection {
            fov: 45.0_f32.to_radians(),
            ..default()
        }),
        Transform::from_xyz(0.0, 1.9, 5.7).looking_at(Vec3::new(0.0, 1.55, -1.4), Vec3::Y),
    ));
}
fn request_avatar(state: Res<Capture>, mut requests: MessageWriter<LoadAvatar>) {
    if let Some(path) = &state.avatar {
        requests.write(LoadAvatar(AssetPath::from_path(path).into_owned()));
    }
}
fn capture(
    mut commands: Commands,
    mut state: ResMut<Capture>,
    assets: Res<AssetServer>,
    avatar: Res<AvatarLoadState>,
    mut exit: MessageWriter<AppExit>,
) {
    state.frames += 1;
    if state.started.elapsed() > Duration::from_secs(120) || state.frames > state.warmup + 1800 {
        eprintln!("Room render timed out after {} frames", state.frames);
        exit.write(AppExit::error());
        return;
    }
    if state.avatar.is_some() {
        match &avatar.status {
            LoadStatus::Failed(error) => {
                eprintln!("Avatar load failed: {error}");
                exit.write(AppExit::error());
                return;
            }
            LoadStatus::Ready if avatar.active.is_some() => state.avatar_ready_frames += 1,
            _ => {
                state.avatar_ready_frames = 0;
                return;
            }
        }
        if state.avatar_ready_frames < 30 {
            return;
        }
    }
    if state.requested
        || state.frames < state.warmup
        || !assets.is_loaded_with_dependencies(state.art.id())
    {
        return;
    }
    state.requested = true;
    commands
        .spawn(Screenshot::image(state.target.clone()))
        .observe(
            |event: On<ScreenshotCaptured>,
             mut state: ResMut<Capture>,
             mut exit: MessageWriter<AppExit>| {
                let image = match event.image.clone().try_into_dynamic() {
                    Ok(image) => image.to_rgb8(),
                    Err(error) => {
                        eprintln!("Cannot convert room capture: {error}");
                        exit.write(AppExit::error());
                        return;
                    }
                };
                // A frame-count warmup alone cannot establish GPU pipeline readiness.
                // Retry an empty clear frame rather than saving it as a successful preview.
                let first = image.get_pixel(0, 0).0;
                let visible = image
                    .pixels()
                    .step_by(1024)
                    .any(|pixel| pixel.0.iter().zip(first).any(|(a, b)| a.abs_diff(b) > 16));
                if !visible {
                    state.requested = false;
                    state.warmup = state.frames + 60;
                    return;
                }
                let result = image
                    .save_with_format(&state.output, image::ImageFormat::Png)
                    .map_err(|error| error.to_string());
                match result {
                    Ok(()) => {
                        println!(
                            "GPU room preview saved: {} ({} frames)",
                            state.output.display(),
                            state.frames
                        );
                        exit.write(AppExit::Success);
                    }
                    Err(e) => {
                        eprintln!("Cannot save room capture: {e}");
                        exit.write(AppExit::error());
                    }
                }
            },
        );
}
