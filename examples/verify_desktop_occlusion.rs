//! Headless GPU regression: back geometry disappears, front geometry survives,
//! transparent pixels stay transparent. Uses the production depth-mask plugin.
//! cargo run --offline --example verify_desktop_occlusion -- [1|4] [/tmp/proof.png]
use bevy::{
    app::{AppExit, ScheduleRunnerPlugin},
    camera::{RenderTarget, ScalingMode},
    core_pipeline::tonemapping::Tonemapping,
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
    desktop_interaction::{DesktopSurface, ScreenRect, SurfaceId},
    desktop_occlusion::{DesktopOcclusion, DesktopOcclusionPlugin},
    desktop_platform::DesktopSnapshot,
};

#[derive(Resource)]
struct Proof {
    target: Handle<Image>,
    frames: u32,
    requested: bool,
    msaa: Msaa,
    output: PathBuf,
    camera: Option<Entity>,
    mask: DesktopOcclusion,
    stage: u8,
}

fn main() -> AppExit {
    let mut args = std::env::args_os().skip(1);
    let msaa = match args.next().and_then(|s| s.into_string().ok()).as_deref() {
        None | Some("1") => Msaa::Off,
        Some("4") => Msaa::Sample4,
        _ => panic!("Use 1 or 4 samples"),
    };
    let output = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| "/tmp/desktop-occlusion-proof.png".into());
    App::new()
        .insert_resource(Proof {
            target: default(),
            frames: 0,
            requested: false,
            msaa,
            output,
            camera: None,
            mask: DesktopOcclusion::default(),
            stage: 0,
        })
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                })
                .disable::<WinitPlugin>()
                .disable::<bevy::audio::AudioPlugin>(),
        )
        .add_plugins((
            ScheduleRunnerPlugin::run_loop(Duration::from_millis(16)),
            DesktopOcclusionPlugin,
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, capture)
        .run()
}

fn setup(
    mut commands: Commands,
    mut proof: ResMut<Proof>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut image = Image::new_target_texture(256, 128, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    proof.target = images.add(image);
    let now = Instant::now();
    let snapshot = DesktopSnapshot {
        received_at: now,
        surfaces: vec![DesktopSurface {
            id: SurfaceId::Window(1),
            rect: ScreenRect {
                origin: Vec2::new(128., 0.),
                size: Vec2::new(128., 128.),
            },
            z_order: 0,
        }],
        own_window_ids: vec![],
        own_window_rect: Some(ScreenRect {
            origin: Vec2::ZERO,
            size: Vec2::new(256., 128.),
        }),
        own_interactive_move: None,
        own_above_panels: None,
        cursor: Vec2::ZERO,
    };
    proof.mask = DesktopOcclusion::for_seat(&snapshot, SurfaceId::Window(1), 0.5, now);
    proof.camera = Some(
        commands
            .spawn((
                Camera3d::default(),
                RenderTarget::Image(proof.target.clone().into()),
                Camera {
                    clear_color: ClearColorConfig::Custom(Color::NONE),
                    ..default()
                },
                Projection::Orthographic(OrthographicProjection {
                    near: 0.,
                    far: 10.,
                    scaling_mode: ScalingMode::Fixed {
                        width: 4.,
                        height: 2.,
                    },
                    ..OrthographicProjection::default_3d()
                }),
                Transform::from_xyz(0., 0., 5.).looking_at(Vec3::ZERO, Vec3::Y),
                proof.msaa,
                Tonemapping::None,
                proof.mask,
            ))
            .id(),
    );
    for (position, size, color) in [
        (
            Vec3::new(0., 0., -1.),
            Vec2::new(4., 2.),
            Color::srgb(1., 0., 0.),
        ),
        (
            Vec3::new(1., 0., 1.),
            Vec2::splat(0.5),
            Color::srgb(0., 1., 0.),
        ),
    ] {
        commands.spawn((
            Mesh3d(meshes.add(Rectangle::from_size(size))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color,
                unlit: true,
                ..default()
            })),
            Transform::from_translation(position),
        ));
    }
}

fn capture(mut commands: Commands, mut proof: ResMut<Proof>, mut exit: MessageWriter<AppExit>) {
    proof.frames += 1;
    if proof.frames > 600 {
        exit.write(AppExit::error());
    }
    if proof.frames < if proof.stage == 0 { 90 } else { 30 } || proof.requested {
        return;
    }
    proof.requested = true;
    let output = proof.output.clone();
    commands
        .spawn(Screenshot::image(proof.target.clone()))
        .observe(
            move |event: On<ScreenshotCaptured>,
                  mut exit: MessageWriter<AppExit>,
                  mut proof: ResMut<Proof>,
                  mut commands: Commands| {
                let image = event
                    .image
                    .clone()
                    .try_into_dynamic()
                    .expect("GPU readback")
                    .to_rgba8();
                let baseline = image.get_pixel(64, 64).0;
                let hidden = image.get_pixel(144, 32).0;
                let foreground = image.get_pixel(192, 64).0;
                image.save(&output).expect("save GPU proof");
                assert!(
                    baseline[0] > 220 && baseline[3] > 220,
                    "unmasked red baseline: {baseline:?}"
                );
                if proof.stage == 1 {
                    assert!(
                        hidden[0] > 220 && hidden[3] > 220,
                        "removed mask must restore red geometry: {hidden:?}"
                    );
                } else {
                    assert!(
                        hidden[3] < 5,
                        "masked geometry must leave transparent pixels: {hidden:?}"
                    );
                }
                assert!(
                    foreground[1] > 220 && foreground[0] < 20 && foreground[3] > 220,
                    "front green geometry: {foreground:?}"
                );
                let camera = proof.camera.expect("proof camera");
                match proof.stage {
                    0 => {
                        commands.entity(camera).remove::<DesktopOcclusion>();
                    }
                    1 => {
                        commands.entity(camera).insert(proof.mask);
                    }
                    _ => {
                        println!(
                            "PASS: active, removed and restored masks; {}",
                            output.display()
                        );
                        exit.write(AppExit::Success);
                        return;
                    }
                }
                proof.stage += 1;
                proof.frames = 0;
                proof.requested = false;
            },
        );
}
