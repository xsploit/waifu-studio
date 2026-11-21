//! Offscreen GPU proof of the production VRM importer and humanoid retargeter.
//! No desktop window, audio device, room geometry, provider or network client.
//!
//! cargo run --example render_desktop_motion -- sitting /tmp/sitting.png --time 0.5
//! Usage: <idle|sitting|grabbed|LOCAL_MOTION_PATH> <output.png>
//!        [--time SECONDS] [--frames 180] [--avatar LOCAL_VRM_PATH] [--sequence FPS]
//!        [--view front|side|rear|other-side|three-quarter] [--rig-overlay]
//!        [--pose-json PATH] [--arm-clearance on|off] [--motion-weight 0..1]
//!        [--continuous --sequence FPS] [--end SECONDS] [--spring-strength 0..1]
//!        [--arm-pose-probe] (diagnostic10° outward swing/−20° twist before live springs)
//!        [--rest-pose] (authored skeleton pose, with live spring physics)
//!        [--blend-from LOCAL_MOTION_PATH] [--blend-from-time SECONDS] [--blend-from-paused]
//! --blend-from records the production half-second crossfade into the requested motion.
//! The first recorded pose is the first animation tick after the switch.
//! Views orbit at the same distance: front +Z, side +X, rear -Z, other-side -X,
//! three-quarter +X/+Z. Rig overlay: left cyan, right magenta, torso yellow;
//! native unlit cylinders at actual avatar joints, with normal depth occlusion.
//! With --sequence, every 1/FPS seconds from --time to the clip end is saved as
//! OUTPUT_0000.png, OUTPUT_0001.png, ... (e.g. for `ffmpeg -i out_%04d.png`).
//! Defaults: front view, arm clearance off. Pose JSON uses the exact requested
//! path for a single capture, or JSON_STEM_0000.json, JSON_STEM_0001.json, ...
//! with --sequence. Quaternions are [x, y, z, w] in model space.
//! --continuous advances the production animation and spring simulation every
//! fixed timestep, capturing each frame without pausing for GPU readback.
use bevy::{
    app::{AppExit, SceneSpawnerSystems, ScheduleRunnerPlugin},
    asset::{AssetMetaCheck, AssetPath, UnapprovedPathMode},
    camera::{RenderTarget, ShadowLodOrigin, visibility::NoFrustumCulling},
    ecs::system::SystemParam,
    light::NotShadowCaster,
    mesh::{
        morph::{MeshMorphWeights, MorphWeights},
        skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    },
    prelude::*,
    render::{
        render_resource::{TextureFormat, TextureUsages},
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    time::TimeUpdateStrategy,
    window::ExitCondition,
    winit::WinitPlugin,
};
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName;
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use waifu_studio::{
    animation::{
        AnimationState, BvhAnimationPlugin, BvhClip, HumanoidRestTransform, parse_bvh, parse_fbx,
        parse_vrma,
    },
    loading::{AvatarLoadState, LoadAvatar, LoadStatus, VrmLoaderPlugin},
    spring_physics::{SpringPhysicsPlugin, SpringPhysicsState},
};

#[path = "support/arm_pose_probe.rs"]
mod arm_pose_probe;

const MAX_MOTION_BYTES: u64 = 32 * 1024 * 1024;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 960;

struct Options {
    batch_dir: Option<PathBuf>,
    follow_hips: bool,
    motion: PathBuf,
    output: PathBuf,
    avatar: PathBuf,
    seconds: f32,
    warmup: u32,
    sequence: Option<f32>,
    view: String,
    pose_json: Option<PathBuf>,
    pose_bones_only: bool,
    arm_clearance: bool,
    arm_pose_probe: bool,
    rig_overlay: bool,
    continuous: bool,
    rest_pose: bool,
    motion_weight: f32,
    end: Option<f32>,
    spring_strength: f32,
    blend_from: Option<PathBuf>,
    blend_from_time: Option<f32>,
    blend_from_paused: bool,
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
            _ => return Err("Usage: render_desktop_motion <idle|sitting|grabbed|LOCAL_MOTION_PATH> <output.png> [--time SECONDS] [--frames 180] [--avatar LOCAL_VRM_PATH] [--sequence FPS] [--view front|side|rear|other-side|three-quarter] [--rig-overlay] [--pose-json PATH] [--arm-clearance on|off]".into()),
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
            batch_dir: None,
            follow_hips: false,
            motion,
            output,
            avatar: root.join("assets/characters/models/4e5c5188c39e-neuro-sama.vrm"),
            seconds: 0.5,
            warmup: 180,
            sequence: None,
            view: "front".into(),
            pose_json: None,
            pose_bones_only: false,
            arm_clearance: false,
            arm_pose_probe: false,
            rig_overlay: false,
            continuous: false,
            rest_pose: false,
            motion_weight: 1.0,
            end: None,
            spring_strength: 0.0,
            blend_from: None,
            blend_from_time: None,
            blend_from_paused: false,
        };
        while let Some(option) = args.next() {
            match option.as_str() {
                "--batch-dir" => {
                    options.batch_dir = Some(PathBuf::from(
                        args.next().ok_or("--batch-dir needs a directory")?,
                    ));
                }
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
                "--sequence" => {
                    options.sequence = Some(
                        args.next()
                            .and_then(|s| s.parse::<f32>().ok())
                            .filter(|fps| (1.0..=60.0).contains(fps))
                            .ok_or("--sequence requires 1..60 frames per second")?,
                    );
                }
                "--view" => {
                    options.view = args
                        .next()
                        .filter(|view| {
                            matches!(
                                view.as_str(),
                                "front" | "side" | "rear" | "other-side" | "three-quarter" | "quad"
                            )
                        })
                        .ok_or("--view requires front, side, rear, other-side or three-quarter")?;
                }
                "--rig-overlay" => options.rig_overlay = true,
                "--pose-bones-only" => options.pose_bones_only = true,
                "--follow-hips" => options.follow_hips = true,
                "--continuous" => options.continuous = true,
                "--rest-pose" => options.rest_pose = true,
                "--motion-weight" => {
                    options.motion_weight = args
                        .next()
                        .and_then(|s| s.parse::<f32>().ok())
                        .filter(|s| (0.0..=1.0).contains(s))
                        .ok_or("--motion-weight requires a number between 0 and 1")?;
                }
                "--arm-pose-probe" => options.arm_pose_probe = true,
                "--end" => {
                    options.end = Some(
                        args.next()
                            .and_then(|s| s.parse::<f32>().ok())
                            .filter(|s| s.is_finite() && *s >= 0.0)
                            .ok_or("--end requires a finite time at or after --time")?,
                    );
                }
                "--blend-from" => {
                    options.blend_from = Some(PathBuf::from(
                        args.next()
                            .ok_or("--blend-from requires a local motion path")?,
                    ));
                }
                "--blend-from-time" => {
                    options.blend_from_time = Some(
                        args.next()
                            .and_then(|value| value.parse::<f32>().ok())
                            .filter(|seconds| seconds.is_finite() && *seconds >= 0.)
                            .ok_or("--blend-from-time requires a finite nonnegative number")?,
                    );
                }
                "--blend-from-paused" => options.blend_from_paused = true,
                "--spring-strength" => {
                    options.spring_strength = args
                        .next()
                        .and_then(|s| s.parse::<f32>().ok())
                        .filter(|s| s.is_finite() && (0.0..=1.0).contains(s))
                        .ok_or("--spring-strength requires 0..1")?;
                }
                "--pose-json" => {
                    options.pose_json = Some(
                        args.next()
                            .filter(|path| !path.is_empty() && !path.starts_with("--"))
                            .map(PathBuf::from)
                            .ok_or("--pose-json requires an output path")?,
                    );
                }
                "--arm-clearance" => {
                    options.arm_clearance = match args.next().as_deref() {
                        Some("on") => true,
                        Some("off") => false,
                        _ => return Err("--arm-clearance requires on or off".into()),
                    };
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
        if options.arm_pose_probe && (options.rest_pose || options.arm_clearance) {
            return Err(
                "--arm-pose-probe cannot be combined with --rest-pose or --arm-clearance on".into(),
            );
        }
        if options.continuous && options.sequence.is_none() {
            return Err("--continuous requires --sequence FPS".into());
        }
        if options.continuous {
            continuous_spring_steps(options.sequence.unwrap())?;
        }
        if options.end.is_some() && !options.continuous {
            return Err("--end requires --continuous".into());
        }
        if let Some(path) = &mut options.blend_from {
            if !options.continuous
                || options.seconds != 0.
                || options.rest_pose
                || options.batch_dir.is_some()
                || options.end.is_some_and(|end| end < 0.5)
            {
                return Err("--blend-from requires --continuous --time 0 and at least 0.5 seconds of capture; it cannot be combined with --rest-pose or --batch-dir".into());
            }
            *path = path
                .canonicalize()
                .map_err(|error| format!("Cannot resolve outgoing motion: {error}"))?;
        } else if options.blend_from_time.is_some() || options.blend_from_paused {
            return Err("--blend-from-time and --blend-from-paused require --blend-from".into());
        }
        if options.end.is_some_and(|end| end < options.seconds) {
            return Err("--end precedes --time".into());
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
        let clip = read_clip(&self.motion)?;
        if self.seconds > clip.duration() {
            return Err(format!(
                "Requested sample {:.3}s exceeds clip duration {:.3}s",
                self.seconds,
                clip.duration()
            ));
        }
        if self.end.is_some_and(|end| end > clip.duration()) {
            return Err("--end exceeds clip duration".into());
        }
        Ok(clip)
    }
}

fn read_clip(path: &Path) -> Result<BvhClip, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Cannot open motion: {e}"))?;
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
    match path
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
    }
}

#[derive(Resource)]
struct Capture {
    remaining: std::collections::VecDeque<PathBuf>,
    batch_root: PathBuf,
    options: Options,
    ready_frames: u32,
    frames: u32,
    next_attempt: u32,
    started: Instant,
    requested: bool,
    target: Handle<Image>,
    duration: f32,
    index: u32,
    pose_written: bool,
    rest_locals: HashMap<Entity, Transform>,
    continuous_started: bool,
    continuous_done: bool,
    completed: Arc<AtomicU32>,
    failed: Arc<AtomicBool>,
    last_pose: f32,
    continuous_start_seconds: f32,
    pending_blend: Option<Arc<BvhClip>>,
}

/// The vendored spring clock uses 1/60 s steps and clamps elapsed to 0.25 s.
/// Restrict this diagnostic so every app frame has an integral spring cadence.
fn continuous_spring_steps(fps: f32) -> Result<u32, String> {
    const RATES: [f32; 9] = [4., 5., 6., 10., 12., 15., 20., 30., 60.];
    if !RATES.contains(&fps) {
        return Err("--continuous requires --sequence 4, 5, 6, 10, 12, 15, 20, 30 or 60 (aligned to the 60 Hz spring clock)".into());
    }
    Ok((60. / fps) as u32)
}

fn check_continuous_clock(position: f32, start: f32, index: u32, fps: f32) -> Result<(), String> {
    let expected = f64::from(start) + f64::from(index) / f64::from(fps);
    if !position.is_finite() || (f64::from(position) - expected).abs() >= 0.5 / f64::from(fps) {
        return Err(format!(
            "Continuous playback clock drift at sample {index}: {position} vs {expected}"
        ));
    }
    Ok(())
}

#[derive(Component)]
struct RigOverlayBone {
    start: HumanBoneName,
    end: HumanBoneName,
}

// Excluding the diagnostic entities also keeps reads of avatar locals disjoint
// from writes to overlay transforms in update_rig_overlay.
type AvatarHierarchy<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        Option<&'static HumanoidRestTransform>,
        Option<&'static ChildOf>,
    ),
    Without<RigOverlayBone>,
>;

#[derive(SystemParam)]
struct PoseDiagnostics<'w, 's> {
    bones: Query<'w, 's, (Entity, &'static HumanBoneName, Option<&'static Name>)>,
    hierarchy: AvatarHierarchy<'w, 's>,
    skins: DiagnosticSkins<'w, 's>,
    names: Query<'w, 's, &'static Name>,
    globals: Query<'w, 's, &'static GlobalTransform>,
    bindposes: Res<'w, Assets<SkinnedMeshInverseBindposes>>,
    asset_server: Res<'w, AssetServer>,
    morph_weights: Query<'w, 's, &'static MorphWeights>,
    physics: Res<'w, SpringPhysicsState>,
}

type DiagnosticSkins<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Mesh3d,
        &'static SkinnedMesh,
        Option<&'static Name>,
        Option<&'static MeshMorphWeights>,
    ),
    Without<MeshMaterial3d<bevy_shader_mtoon::OutlineMaterial>>,
>;

fn main() -> AppExit {
    let mut options = match Options::parse() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return AppExit::error();
        }
    };
    let mut remaining = std::collections::VecDeque::new();
    let batch_root = options.output.parent().unwrap_or(Path::new(".")).to_owned();
    if let Some(directory) = &options.batch_dir {
        if !options.continuous
            || options.rest_pose
            || options.seconds != 0.
            || options.end.is_some()
        {
            eprintln!("Batch review needs continuous playback from zero to the full clip end");
            return AppExit::error();
        }
        let mut paths: Vec<_> = std::fs::read_dir(directory)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.extension().is_some_and(|e| e == "bvh"))
            .collect();
        paths.sort();
        remaining = paths.into();
        let Some(first) = remaining.pop_front() else {
            return AppExit::error();
        };
        options.motion = first;
        options.output = batch_root
            .join(options.motion.file_stem().unwrap())
            .join("frame.png");
    }
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
    // Warm up the authored pose through BvhAnimationPlugin before the
    // offscreen capture starts. The optional switch uses the same select_clip
    // entry point as the live app.
    let duration = clip.duration();
    let last_pose = clip.last_sample_time();
    let step = if options.continuous {
        options.sequence.unwrap_or(60.0)
    } else {
        60.0
    };
    let mut physics = SpringPhysicsState::default();
    physics.config.strength = options.spring_strength;
    let mut animation = AnimationState::default();
    let mut pending_blend = None;
    if let Some(path) = &options.blend_from {
        let outgoing = match read_clip(path) {
            Ok(clip) => clip,
            Err(error) => {
                eprintln!("Outgoing motion import failed: {error}");
                return AppExit::error();
            }
        };
        if options.blend_from_time.unwrap_or(0.) > outgoing.last_sample_time() || last_pose < 0.5 {
            eprintln!(
                "Crossfade needs a valid outgoing sample and at least 0.5 seconds of incoming poses"
            );
            return AppExit::error();
        }
        animation.clip = Some(Arc::new(outgoing));
        animation.path = Some(path.clone());
        animation.looping = !options.blend_from_paused;
        pending_blend = Some(Arc::new(clip));
    } else {
        animation.clip = Some(Arc::new(clip));
        animation.path = Some(options.motion.clone());
        animation.looping = false;
    }
    animation.arm_clearance = options.arm_clearance;
    animation.weight = if options.rest_pose {
        0.
    } else {
        options.motion_weight
    };
    animation.play();
    animation.pause();
    let continuous_start_seconds = options.seconds;
    App::new()
        .insert_resource(Capture {
            remaining,
            batch_root,
            options,
            ready_frames: 0,
            frames: 0,
            next_attempt: 0,
            started: Instant::now(),
            requested: false,
            target: default(),
            duration,
            index: 0,
            pose_written: false,
            rest_locals: HashMap::new(),
            continuous_started: false,
            continuous_done: false,
            completed: default(),
            failed: default(),
            last_pose,
            continuous_start_seconds,
            pending_blend,
        })
        .insert_resource(physics)
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
            1. / f64::from(step),
        )))
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
            1. / 60.,
        )))
        .add_plugins((VrmLoaderPlugin, BvhAnimationPlugin, SpringPhysicsPlugin))
        .init_resource::<arm_pose_probe::Probe>()
        .add_systems(First, arm_pose_probe::restore)
        .add_systems(
            PostUpdate,
            arm_pose_probe::apply.before(bevy::transform::TransformSystems::Propagate),
        )
        .add_systems(Startup, setup)
        .add_systems(
            SpawnScene,
            snapshot_diagnostic_rest.after(SceneSpawnerSystems::WorldInstanceSpawn),
        )
        // Sample after all PostUpdate pose writers, including VRM springs.
        // The JSON and overlay therefore describe the same final avatar pose.
        .add_systems(
            Last,
            (follow_review_hips, update_rig_overlay, prepare_and_capture).chain(),
        )
        .run()
}

fn setup(
    mut commands: Commands,
    mut state: ResMut<Capture>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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
    let quad = state.options.view == "quad";
    let views: Vec<&str> = if quad {
        vec!["front", "side", "rear", "other-side"]
    } else {
        vec![state.options.view.as_str()]
    };
    for (index, view) in views.into_iter().enumerate() {
        commands.spawn((
            ReviewCamera,
            Camera3d::default(),
            ShadowLodOrigin,
            Camera {
                clear_color: ClearColorConfig::Custom(Color::NONE),
                order: index as isize,
                viewport: quad.then(|| bevy::camera::Viewport {
                    physical_position: UVec2::new(
                        (index as u32 % 2) * (WIDTH / 2),
                        (index as u32 / 2) * (HEIGHT / 2),
                    ),
                    physical_size: UVec2::new(WIDTH / 2, HEIGHT / 2),
                    ..default()
                }),
                ..default()
            },
            RenderTarget::Image(state.target.clone().into()),
            Projection::Perspective(PerspectiveProjection {
                fov: 45_f32.to_radians(),
                ..default()
            }),
            Transform::from_translation(match view {
                "side" => Vec3::new(3.2, 1., 0.),
                "rear" => Vec3::new(0., 1., -3.2),
                "other-side" => Vec3::new(-3.2, 1., 0.),
                "three-quarter" => {
                    let diagonal = 3.2 * std::f32::consts::FRAC_1_SQRT_2;
                    Vec3::new(diagonal, 1., diagonal)
                }
                _ => Vec3::new(0., 1., 3.2),
            })
            .looking_at(Vec3::new(0., 0.9, 0.), Vec3::Y),
        ));
    }
    if state.options.rig_overlay {
        use HumanBoneName::*;
        // Allocate one mesh, three materials and a fixed set of entities once.
        // Cylinder's unit height is along +Y; its radius is 4 mm in world space.
        let mesh = meshes.add(Cylinder::new(0.004, 1.).mesh().resolution(10));
        for (color, links) in [
            (
                Color::srgb(0., 1., 1.),
                &[
                    (LeftShoulder, LeftUpperArm),
                    (LeftUpperArm, LeftLowerArm),
                    (LeftLowerArm, LeftHand),
                ][..],
            ),
            (
                Color::srgb(1., 0.05, 0.8),
                &[
                    (RightShoulder, RightUpperArm),
                    (RightUpperArm, RightLowerArm),
                    (RightLowerArm, RightHand),
                ][..],
            ),
            (
                Color::srgb(1., 1., 0.1),
                &[(LeftUpperArm, RightUpperArm), (Hips, Spine), (Spine, Chest)][..],
            ),
        ] {
            let material = materials.add(StandardMaterial {
                base_color: color,
                unlit: true,
                ..default()
            });
            for &(start, end) in links {
                commands.spawn((
                    Name::new(format!("Diagnostic {} -> {}", start.as_str(), end.as_str())),
                    RigOverlayBone { start, end },
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::default(),
                    Visibility::Hidden,
                    NotShadowCaster,
                    // Final globals are written after visibility checks. These
                    // nine tiny diagnostics must not use previous-frame bounds.
                    NoFrustumCulling,
                ));
            }
        }
    }
    requests.write(LoadAvatar(
        AssetPath::from_path(&state.options.avatar).into_owned(),
    ));
}

#[derive(Component)]
struct ReviewCamera;

// Diagnostic framing only: preserve the complete locomotion pose by following
// horizontal hips translation. The avatar and all animation tracks are unchanged.
// Write the camera global as well because this runs after transform propagation.
fn follow_review_hips(
    state: Res<Capture>,
    hips: Query<(&HumanBoneName, &GlobalTransform), Without<ReviewCamera>>,
    mut cameras: Query<(&Camera, &mut Transform, &mut GlobalTransform), With<ReviewCamera>>,
) {
    if !state.options.follow_hips {
        return;
    }
    let Some((_, hips)) = hips.iter().find(|(bone, _)| **bone == HumanBoneName::Hips) else {
        return;
    };
    let position = hips.translation();
    let center = Vec3::new(position.x, 0., position.z);
    for (camera, mut local, mut global) in &mut cameras {
        let view = if state.options.view == "quad" {
            ["front", "side", "rear", "other-side"][camera.order as usize]
        } else {
            state.options.view.as_str()
        };
        let offset = match view {
            "side" => Vec3::new(3.2, 1., 0.),
            "rear" => Vec3::new(0., 1., -3.2),
            "other-side" => Vec3::new(-3.2, 1., 0.),
            "three-quarter" => Vec3::new(
                3.2 * std::f32::consts::FRAC_1_SQRT_2,
                1.,
                3.2 * std::f32::consts::FRAC_1_SQRT_2,
            ),
            _ => Vec3::new(0., 1., 3.2),
        };
        *local = Transform::from_translation(center + offset)
            .looking_at(center + Vec3::new(0., 0.9, 0.), Vec3::Y);
        *global = GlobalTransform::from(*local);
    }
}

/// The same root-relative FK chain is used by both JSON and overlay endpoints.
/// Keep imported helper nodes and scales; exclude the application root.
fn diagnostic_joint_chain(
    root: Entity,
    entity: Entity,
    hierarchy: &AvatarHierarchy,
) -> Result<Vec<Entity>, String> {
    let mut chain = Vec::new();
    let mut ancestor = entity;
    while ancestor != root {
        if chain.len() >= 128 || chain.contains(&ancestor) {
            return Err("Avatar diagnostic hierarchy is cyclic or exceeds 128 nodes".into());
        }
        let Ok((_, _, Some(parent))) = hierarchy.get(ancestor) else {
            break;
        };
        chain.push(ancestor);
        ancestor = parent.parent();
    }
    if ancestor != root {
        chain.clear();
    }
    Ok(chain)
}

fn update_rig_overlay(
    state: Res<Capture>,
    avatar: Res<AvatarLoadState>,
    bones: Query<(Entity, &HumanBoneName)>,
    hierarchy: AvatarHierarchy,
    globals: Query<&GlobalTransform, Without<RigOverlayBone>>,
    mut overlay: Query<(
        &RigOverlayBone,
        &mut Transform,
        &mut GlobalTransform,
        &mut Visibility,
        &mut ViewVisibility,
    )>,
) {
    if !state.options.rig_overlay {
        return;
    }
    let mut positions = HashMap::new();
    if let Some(root) = avatar.active
        && let Ok(root_global) = globals.get(root)
    {
        for (entity, bone) in &bones {
            if !overlay
                .iter()
                .any(|(link, ..)| link.start == *bone || link.end == *bone)
            {
                continue;
            }
            let Ok(chain) = diagnostic_joint_chain(root, entity, &hierarchy) else {
                continue;
            };
            if chain.is_empty() {
                continue;
            }
            let Some(model) =
                chain
                    .into_iter()
                    .rev()
                    .try_fold(GlobalTransform::IDENTITY, |model, entity| {
                        let (local, _, _) = hierarchy.get(entity).ok()?;
                        Some(model.mul_transform(*local))
                    })
            else {
                continue;
            };
            // Convert the JSON's model coordinates back through the application
            // root for rendering, without nudging joints toward the camera.
            let world = root_global.transform_point(model.translation());
            if world.is_finite() {
                positions.insert(*bone, world);
            }
        }
    }
    for (link, mut local, mut global, mut visibility, mut view_visibility) in &mut overlay {
        let endpoints = positions.get(&link.start).zip(positions.get(&link.end));
        if let Some((&start, &end)) = endpoints {
            let delta = end - start;
            let length = delta.length();
            if length.is_finite() && length > 1e-6 {
                *local = Transform {
                    translation: (start + end) * 0.5,
                    rotation: Quat::from_rotation_arc(Vec3::Y, delta / length),
                    scale: Vec3::new(1., length, 1.),
                };
                // Overlay entities have no parent. Write the final global now,
                // since ordinary propagation has already finished this frame.
                *global = GlobalTransform::from(*local);
                *visibility = Visibility::Visible;
                continue;
            }
        }
        // Optional/missing joints never get invented endpoints or stale links.
        *visibility = Visibility::Hidden;
        *view_visibility = ViewVisibility::HIDDEN;
    }
}

fn snapshot_diagnostic_rest(
    mut state: ResMut<Capture>,
    transforms: Query<(Entity, &Transform), Added<Transform>>,
) {
    if state.options.pose_json.is_none() || state.pose_written {
        return;
    }
    // Scene instantiation precedes this snapshot, and posing starts in the next
    // Update. Preserve unmapped helper locals before spring/procedural writers.
    for (entity, transform) in &transforms {
        state.rest_locals.entry(entity).or_insert(*transform);
    }
}

fn skeleton_entry(
    bone: Option<&str>,
    name: &str,
    current: (Vec3, Quat),
    rest: (Vec3, Quat),
) -> Result<serde_json::Value, String> {
    if !current.0.is_finite()
        || !rest.0.is_finite()
        || !current.1.is_finite()
        || !rest.1.is_finite()
        || current.1.length_squared() < 1e-12
        || rest.1.length_squared() < 1e-12
    {
        return Err(format!("Nonfinite or invalid diagnostic pose for {name}"));
    }
    Ok(serde_json::json!({
        "bone": bone,
        "name": name,
        "current_position": current.0.to_array(),
        "rest_position": rest.0.to_array(),
        "current_world_rotation": current.1.normalize().to_array(),
        "rest_world_rotation": rest.1.normalize().to_array(),
    }))
}

fn write_pose_json(
    state: &Capture,
    root: Entity,
    clip: &BvhClip,
    motion_weight: f32,
    diagnostics: &PoseDiagnostics,
) -> Result<(), String> {
    let Some(path) = &state.options.pose_json else {
        return Ok(());
    };
    let batch_path;
    let path = if state.options.batch_dir.is_some() {
        batch_path = state
            .options
            .output
            .parent()
            .ok_or("Missing output directory")?
            .join(path.file_name().ok_or("Missing pose filename")?);
        &batch_path
    } else {
        path
    };
    let path = if state.options.sequence.is_some() {
        let mut stem = path.with_extension("").into_os_string();
        stem.push(format!("_{:04}.json", state.index));
        PathBuf::from(stem)
    } else {
        path.clone()
    };
    let mut avatar = Vec::new();
    let hierarchy = &diagnostics.hierarchy;
    for (entity, bone, name) in &diagnostics.bones {
        let chain = diagnostic_joint_chain(root, entity, hierarchy)?;
        if chain.is_empty() {
            continue;
        }
        let mut current = GlobalTransform::IDENTITY;
        let mut rest = GlobalTransform::IDENTITY;
        for entity in chain.into_iter().rev() {
            let (local, humanoid_rest, _) = hierarchy
                .get(entity)
                .map_err(|error| format!("Missing diagnostic transform: {error}"))?;
            let rest_local = humanoid_rest
                .map(|rest| &rest.0)
                .or_else(|| state.rest_locals.get(&entity))
                .ok_or_else(|| format!("Missing imported rest transform for {entity:?}"))?;
            current = current.mul_transform(*local);
            rest = rest.mul_transform(*rest_local);
        }
        // Both FK chains start below the application root, retaining all helpers
        // and their scales while excluding desktop placement/facing/scale.
        let (_, current_rotation, current_position) = current.to_scale_rotation_translation();
        let (_, rest_rotation, rest_position) = rest.to_scale_rotation_translation();
        avatar.push(skeleton_entry(
            Some(bone.as_str()),
            name.map_or(bone.as_str(), Name::as_str),
            (current_position, current_rotation),
            (rest_position, rest_rotation),
        )?);
    }
    if avatar.is_empty() {
        return Err("Active avatar has no diagnostic humanoid bones".into());
    }
    if state.options.pose_bones_only {
        avatar.sort_by(|a, b| a["bone"].as_str().cmp(&b["bone"].as_str()));
        let data = serde_json::json!({"seconds":state.options.seconds,"arm_clearance":state.options.arm_clearance,"avatar":avatar});
        std::fs::write(path, serde_json::to_vec(&data).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    // Use the actual globals consumed by render skinning, including spring
    // helpers that have no HumanBoneName. Keep full affine matrices: decomposing
    // them to position/quaternion loses scale and shear on imported rigs.
    let root_inverse = diagnostics
        .globals
        .get(root)
        .map_err(|error| error.to_string())?
        .to_matrix()
        .inverse();
    if !root_inverse.is_finite() {
        return Err("Invalid avatar root transform".into());
    }
    let mut skins = Vec::new();
    for (entity, mesh, skin, name, morph) in &diagnostics.skins {
        if diagnostic_joint_chain(root, entity, hierarchy)?.is_empty() {
            continue;
        }
        let inverse_bind = diagnostics
            .bindposes
            .get(&skin.inverse_bindposes)
            .ok_or("Missing diagnostic inverse bind poses")?;
        if skin.joints.len() != inverse_bind.len() {
            return Err("Diagnostic skin joint/bind count mismatch".into());
        }
        let mut joints = Vec::new();
        for (index, (&joint, &bind)) in skin.joints.iter().zip(inverse_bind.iter()).enumerate() {
            let chain = diagnostic_joint_chain(root, joint, hierarchy)?;
            if chain.is_empty() {
                return Err("Diagnostic skin joint is outside active avatar".into());
            }
            let mut fk = GlobalTransform::IDENTITY;
            let mut rest = GlobalTransform::IDENTITY;
            for ancestor in chain.into_iter().rev() {
                let (local, humanoid_rest, _) =
                    hierarchy.get(ancestor).map_err(|error| error.to_string())?;
                let rest_local = humanoid_rest
                    .map(|rest| &rest.0)
                    .or_else(|| state.rest_locals.get(&ancestor))
                    .ok_or("Missing diagnostic joint rest local")?;
                fk = fk.mul_transform(*local);
                rest = rest.mul_transform(*rest_local);
            }
            let model = root_inverse
                * diagnostics
                    .globals
                    .get(joint)
                    .map_err(|error| error.to_string())?
                    .to_matrix();
            let rest = rest.to_matrix();
            let fk = fk.to_matrix();
            if ![model, rest, fk, bind].into_iter().all(|m| m.is_finite()) {
                return Err("Nonfinite diagnostic skin matrix".into());
            }
            joints.push(serde_json::json!({
                "index": index,
                "name": diagnostics.names.get(joint).ok().map(Name::as_str),
                "current_model_matrix": model.to_cols_array(),
                "current_fk_model_matrix": fk.to_cols_array(),
                "rest_model_matrix": rest.to_cols_array(),
                "inverse_bind_matrix": bind.to_cols_array(),
                "current_skin_matrix": (model * bind).to_cols_array(),
            }));
        }
        skins.push(serde_json::json!({
            "entity": entity.to_bits(),
            "mesh_name": name.map(Name::as_str),
            "mesh_asset": diagnostics.asset_server.get_path(mesh.0.id()).map(|p| p.to_string()),
            "morph_weights": rendered_morph_weights(morph, &diagnostics.morph_weights)?,
            "joints": joints,
        }));
    }
    avatar.sort_by(|a, b| a["bone"].as_str().cmp(&b["bone"].as_str()));
    let sampled = clip.sampled_skeleton(state.options.seconds, false);
    let rest = clip.rest_skeleton();
    if sampled.len() != clip.joints.len() || rest.len() != clip.joints.len() {
        return Err("Source diagnostic skeleton size does not match joints".into());
    }
    let source = clip
        .joints
        .iter()
        .zip(sampled)
        .zip(rest)
        .map(|((joint, current), rest)| {
            skeleton_entry(
                joint.bone.map(|bone| bone.as_str()),
                &joint.name,
                current,
                rest,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let diagnostics = serde_json::json!({
        "avatar_path": state.options.avatar,
        "motion_path": state.options.motion,
        "source_format": clip.format(),
        "seconds": state.options.seconds,
        "view": state.options.view,
        "motion_weight": motion_weight,
        "arm_clearance": state.options.arm_clearance,
        "diagnostic_arm_pose_probe": if state.options.arm_pose_probe { serde_json::json!({"outward_swing_degrees":10,"axial_twist_degrees":-20,"stage":"before transform propagation and spring simulation; restored before each Update"}) } else { serde_json::Value::Null },
        "pose_control": if state.options.arm_pose_probe { "production animation plus diagnostic arm pose probe; live springs" } else if state.options.rest_pose { "authored rest skeleton; live springs; zero animation weight" } else { "production animation" },
        "rig_overlay": state.options.rig_overlay,
        "position_space": "model space; avatar application root excluded; source units unchanged",
        "rotation_space": "model world; avatar application root excluded",
        "quaternion_order": "xyzw",
        "skin_matrix_order": "column major; model space; avatar application root excluded",
        "skin_pose_scope": "final render globals after springs; actual primitive morph weights exported before skinning",
        "shader_outline_passes_excluded": true,
        "continuous_playback": state.options.continuous,
        "crossfade": state.options.blend_from.as_ref().map(|path| serde_json::json!({
            "from": path,
            "from_seconds": state.options.blend_from_time.unwrap_or(0.),
            "from_playing": !state.options.blend_from_paused,
        })),
        "app_step_seconds": if state.options.continuous { 1.0 / state.options.sequence.unwrap_or(60.0) } else { 1.0 / 60.0 },
        "spring_fixed_step_seconds": 1.0_f64 / 60.0,
        "expected_spring_steps_per_app_frame": if state.options.continuous { Some(continuous_spring_steps(state.options.sequence.unwrap())?) } else { None },
        "spring_step_count_instrumented": false,
        "physics_strength": diagnostics.physics.config.strength,
        "physics_tuned": diagnostics.physics.available_joints.iter().filter(|j| j.tuned).count(),
        "physics_status": diagnostics.physics.status,
        "app_frame": state.frames,
        "skins": skins,
        "avatar": avatar,
        "source": source,
    });
    let bytes = serde_json::to_vec_pretty(&diagnostics).map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
    println!("Pose diagnostics saved: {}", path.display());
    Ok(())
}

fn rendered_morph_weights(
    component: Option<&MeshMorphWeights>,
    sources: &Query<&MorphWeights>,
) -> Result<Vec<f32>, String> {
    let weights = match component {
        Some(MeshMorphWeights::Value { weights }) => weights.clone(),
        Some(MeshMorphWeights::Reference(entity)) => sources
            .get(*entity)
            .map_err(|_| "Missing rendered morph-weight reference")?
            .weights()
            .to_vec(),
        None => Vec::new(),
    };
    if !weights.iter().all(|w| w.is_finite()) {
        return Err("Nonfinite rendered morph weights".into());
    }
    Ok(weights)
}

/// Readback completion never drives the simulation clock or sample selection.
fn capture_continuous(
    commands: &mut Commands,
    state: &mut Capture,
    animation: &mut AnimationState,
    root: Entity,
    diagnostics: &PoseDiagnostics,
) -> Result<(), String> {
    let fps = state
        .options
        .sequence
        .ok_or("Missing continuous capture rate")?;
    if !state.continuous_started {
        state.continuous_started = true;
        if let Some(incoming) = state.pending_blend.take() {
            if !state.options.blend_from_paused {
                animation.play();
            }
            animation.select_clip(incoming, Some(state.options.motion.clone()));
            state.continuous_start_seconds = 1. / fps;
            return Ok(());
        }
    }
    // The animator has already sampled this position in Update, followed by
    // the actual spring simulation. Never pin a later position onto its label.
    state.options.seconds = animation.position;
    check_continuous_clock(
        animation.position,
        state.continuous_start_seconds,
        state.index,
        fps,
    )?;
    let clip = animation.clip.as_deref().ok_or("Missing continuous clip")?;
    write_pose_json(state, root, clip, animation.weight, diagnostics)?;
    let stem = state.options.output.with_extension("");
    let path = PathBuf::from(format!("{}_{:04}.png", stem.display(), state.index));
    let completed = state.completed.clone();
    let failed = state.failed.clone();
    commands
        .spawn(Screenshot::image(state.target.clone()))
        .observe(move |event: On<ScreenshotCaptured>| {
            let result = event
                .image
                .clone()
                .try_into_dynamic()
                .map_err(|e| e.to_string())
                .and_then(|image| {
                    let image = image.to_rgba8();
                    if image.pixels().filter(|p| p.0[3] > 16).count() < 1024 {
                        return Err("Continuous capture has no visible avatar".into());
                    }
                    if [
                        (0, 0),
                        (WIDTH - 1, 0),
                        (0, HEIGHT - 1),
                        (WIDTH - 1, HEIGHT - 1),
                    ]
                    .iter()
                    .any(|&(x, y)| image.get_pixel(x, y).0[3] > 2)
                    {
                        return Err("Continuous capture has opaque corners".into());
                    }
                    image
                        .save_with_format(&path, image::ImageFormat::Png)
                        .map_err(|e| e.to_string())
                });
            if let Err(error) = result {
                eprintln!("Continuous capture failed: {error}");
                failed.store(true, Ordering::Release);
            }
            completed.fetch_add(1, Ordering::Release);
        });
    state.index += 1;
    let end = state
        .options
        .end
        .unwrap_or(state.last_pose)
        .min(state.last_pose);
    if animation.position >= end - 1e-5 {
        state.continuous_done = true;
        animation.pause();
    } else {
        // A paused sampler is resumed only at the start; GPU callbacks never
        // pause, seek or restart the animator between these fixed steps.
        animation.play();
    }
    if state.index > ((state.duration * fps).ceil() as u32 + 3) {
        return Err("Continuous animation did not reach its end".into());
    }
    Ok(())
}

fn prepare_and_capture(
    mut commands: Commands,
    mut state: ResMut<Capture>,
    avatar: Res<AvatarLoadState>,
    mut animation: ResMut<AnimationState>,
    diagnostics: PoseDiagnostics,
    probe: Res<arm_pose_probe::Probe>,
    mut exit: MessageWriter<AppExit>,
) {
    state.frames += 1;
    if state.options.continuous && state.failed.load(Ordering::Acquire) {
        exit.write(AppExit::error());
        return;
    }
    if state.continuous_done {
        if state.completed.load(Ordering::Acquire) == state.index {
            println!(
                "Saved {} continuous frames with active spring simulation",
                state.index
            );
            if state.options.batch_dir.is_some() {
                let receipt = serde_json::json!({"clip":state.options.motion.file_name().map(|n|n.to_string_lossy()),"captured_frames":state.index,"fps":state.options.sequence,"source_frames":animation.clip.as_ref().map(|c|c.frame_count),"view":state.options.view,"avatar":state.options.avatar,"arm_clearance":state.options.arm_clearance,"camera_follows_horizontal_hips":state.options.follow_hips,"visual_status":"needs_review"});
                let _ = std::fs::write(
                    state.options.output.parent().unwrap().join("receipt.json"),
                    receipt.to_string(),
                );
                if let Some(path) = state.remaining.pop_front() {
                    state.options.motion = path;
                    state.options.seconds = 0.;
                    let next_output = state
                        .batch_root
                        .join(state.options.motion.file_stem().unwrap())
                        .join("frame.png");
                    state.options.output = next_output;
                    if let Err(error) =
                        std::fs::create_dir_all(state.options.output.parent().unwrap())
                    {
                        eprintln!("{error}");
                        exit.write(AppExit::error());
                        return;
                    }
                    let clip = match state.options.clip() {
                        Ok(clip) => clip,
                        Err(error) => {
                            eprintln!("{error}");
                            exit.write(AppExit::error());
                            return;
                        }
                    };
                    state.duration = clip.duration();
                    state.last_pose = clip.last_sample_time();
                    animation.clip = Some(Arc::new(clip));
                    animation.path = Some(state.options.motion.clone());
                    animation.position = 0.;
                    animation.play();
                    animation.pause();
                    state.options.seconds = 0.;
                    state.ready_frames = 0;
                    state.frames = 0;
                    state.index = 0;
                    state.started = Instant::now();
                    state.continuous_started = false;
                    state.continuous_done = false;
                    state.continuous_start_seconds = 0.;
                    state.completed = default();
                    state.failed = default();
                    println!(
                        "BATCH_NEXT {} remaining={}",
                        state.options.motion.display(),
                        state.remaining.len()
                    );
                    return;
                }
            }
            exit.write(AppExit::Success);
        } else if state.started.elapsed() > Duration::from_secs(600) {
            eprintln!("Continuous capture readback timed out");
            exit.write(AppExit::error());
        }
        return;
    }
    let budget = state
        .options
        .sequence
        .map_or(0, |fps| (state.duration * fps) as u32 * 4);
    if state.started.elapsed() > Duration::from_secs(120 + u64::from(budget / 8))
        || state.frames > state.options.warmup + 2400 + budget
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
    if !state.continuous_started {
        animation.position = if state.pending_blend.is_some() {
            state.options.blend_from_time.unwrap_or(0.)
        } else {
            state.options.seconds
        };
        animation.pause();
    }
    if state.requested
        || state.ready_frames < state.options.warmup
        || state.frames < state.next_attempt
    {
        return;
    }
    if state.options.arm_pose_probe && !probe.applied {
        eprintln!("Arm pose probe was not applied to both active avatar arms");
        exit.write(AppExit::error());
        return;
    }
    if state.options.continuous {
        let Some(root) = avatar.active else { return };
        if let Err(error) = capture_continuous(
            &mut commands,
            &mut state,
            &mut animation,
            root,
            &diagnostics,
        ) {
            eprintln!("Cannot record continuous motion: {error}");
            exit.write(AppExit::error());
        }
        return;
    }
    if state.options.pose_json.is_some() && !state.pose_written {
        let Some(root) = avatar.active else { return };
        let Some(clip) = animation.clip.as_deref() else {
            return;
        };
        if let Err(error) = write_pose_json(&state, root, clip, animation.weight, &diagnostics) {
            eprintln!("Cannot save pose diagnostics: {error}");
            exit.write(AppExit::error());
            return;
        }
        state.pose_written = true;
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
    if let Some(fps) = state.options.sequence {
        let stem = state.options.output.with_extension("");
        let path = PathBuf::from(format!("{}_{:04}.png", stem.display(), state.index));
        if let Err(error) = image.save_with_format(&path, image::ImageFormat::Png) {
            eprintln!("Cannot save GPU capture: {error}");
            exit.write(AppExit::error());
            return;
        }
        state.index += 1;
        let next = state.options.seconds + 1.0 / fps;
        if next > state.duration {
            println!(
                "Saved {} frames to {}_NNNN.png",
                state.index,
                stem.display()
            );
            exit.write(AppExit::Success);
            return;
        }
        // The next Update applies the new pose; capture on the frame after it.
        state.options.seconds = next;
        state.requested = false;
        state.pose_written = false;
        state.next_attempt = state.frames + 2;
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::SystemState;

    #[test]
    fn continuous_capture_rejects_spring_clamping_and_fractional_step_cadences() {
        assert_eq!(continuous_spring_steps(30.).unwrap(), 2);
        assert_eq!(continuous_spring_steps(60.).unwrap(), 1);
        assert_eq!(continuous_spring_steps(4.).unwrap(), 15);
        for fps in [1., 3., 24., 25., 50., 59.94, f32::NAN] {
            assert!(continuous_spring_steps(fps).is_err(), "fps {fps}");
        }
    }

    #[test]
    fn continuous_clock_check_detects_drift_without_seeking_the_animation() {
        assert!(check_continuous_clock(5.966661, 0., 179, 30.).is_ok());
        assert!(check_continuous_clock(4.5, 2., 150, 60.).is_ok());
        assert!(check_continuous_clock(4.4, 2., 150, 60.).is_err());
        assert!(check_continuous_clock(f32::NAN, 0., 0, 30.).is_err());
    }

    #[test]
    fn morph_diagnostics_follow_live_render_references_and_reject_missing_sources() {
        let mut world = World::new();
        let source = world
            .spawn(MorphWeights::new(vec![0.1, 0.7], None).unwrap())
            .id();
        let stale = world.spawn_empty().id();
        let mut state: SystemState<Query<&MorphWeights>> = SystemState::new(&mut world);
        {
            let query = state.get(&world).unwrap();
            assert_eq!(
                rendered_morph_weights(Some(&MeshMorphWeights::Reference(source)), &query).unwrap(),
                vec![0.1, 0.7]
            );
            assert_eq!(
                rendered_morph_weights(
                    Some(&MeshMorphWeights::Value { weights: vec![0.4] }),
                    &query
                )
                .unwrap(),
                vec![0.4]
            );
            assert!(
                rendered_morph_weights(Some(&MeshMorphWeights::Reference(stale)), &query).is_err()
            );
            assert!(rendered_morph_weights(None, &query).unwrap().is_empty());
        }
        world.get_mut::<MorphWeights>(source).unwrap().weights_mut()[1] = 0.9;
        let query = state.get(&world).unwrap();
        assert_eq!(
            rendered_morph_weights(Some(&MeshMorphWeights::Reference(source)), &query).unwrap(),
            vec![0.1, 0.9]
        );
    }

    #[test]
    fn nonfinite_render_morph_values_fail_instead_of_exporting_a_false_clear_pose() {
        let mut world = World::new();
        let source = world
            .spawn(MorphWeights::new(vec![f32::NAN], None).unwrap())
            .id();
        let mut state: SystemState<Query<&MorphWeights>> = SystemState::new(&mut world);
        let query = state.get(&world).unwrap();
        assert!(
            rendered_morph_weights(Some(&MeshMorphWeights::Reference(source)), &query).is_err()
        );
        assert!(
            rendered_morph_weights(
                Some(&MeshMorphWeights::Value {
                    weights: vec![f32::INFINITY]
                }),
                &query
            )
            .is_err()
        );
    }
}
