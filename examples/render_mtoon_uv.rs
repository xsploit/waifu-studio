//! Windowless GPU proof: identity UVs (left) and scaled/offset UVs (right).
//! The procedural emissive texture has red/green/blue/yellow stripes. The capture
//! succeeds only when the corresponding left/right samples are red and blue.
use bevy::{
    app::{AppExit, ScheduleRunnerPlugin},
    asset::RenderAssetUsages,
    camera::{RenderTarget, ScalingMode},
    core_pipeline::tonemapping::Tonemapping,
    math::Affine2,
    prelude::*,
    render::{
        render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use bevy_shader_mtoon::{MtoonMaterial, MtoonPlugin};
use std::{path::PathBuf, time::Duration};

#[derive(Resource)]
struct Capture {
    target: Handle<Image>,
    output: PathBuf,
    frame: u32,
    pending: bool,
}
fn main() -> AppExit {
    let Some(output) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("Usage: render_mtoon_uv OUTPUT.png");
        return AppExit::error();
    };
    App::new()
        .insert_resource(Capture {
            target: default(),
            output,
            frame: 0,
            pending: false,
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
            MtoonPlugin,
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, capture)
        .run()
}
fn setup(
    mut commands: Commands,
    mut state: ResMut<Capture>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<MtoonMaterial>>,
) {
    let mut image = Image::new_target_texture(640, 320, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    state.target = images.add(image);
    let stripes = images.add(Image::new(
        Extent3d {
            width: 4,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
        ],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    ));
    let mesh = meshes.add(Rectangle::new(1.5, 1.5));
    for (x, uv_transform) in [
        (-1., Affine2::IDENTITY),
        (
            1.,
            Affine2::from_scale_angle_translation(Vec2::new(0.5, 1.), 0., Vec2::new(0.5, 0.)),
        ),
    ] {
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(materials.add(MtoonMaterial {
                uv_transform,
                base_color: Color::BLACK,
                shade_factor: Color::BLACK,
                emissive_factor: Color::WHITE,
                emissive_texture: Some(stripes.clone()),
                ..default()
            })),
            Transform::from_xyz(x, 0., 0.),
        ));
    }
    commands.spawn((
        Camera3d::default(),
        Tonemapping::None,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        RenderTarget::Image(state.target.clone().into()),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: 4.,
                height: 2.,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0., 0., 5.).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}
fn capture(mut commands: Commands, mut state: ResMut<Capture>, mut exit: MessageWriter<AppExit>) {
    state.frame += 1;
    if state.frame > 600 {
        eprintln!("MToon UV GPU proof timed out");
        exit.write(AppExit::error());
        return;
    }
    if state.frame >= 120 && state.frame.is_multiple_of(60) && !state.pending {
        state.pending = true;
        commands
            .spawn(Screenshot::image(state.target.clone()))
            .observe(save);
    }
}
fn save(
    event: On<ScreenshotCaptured>,
    mut state: ResMut<Capture>,
    mut exit: MessageWriter<AppExit>,
) {
    state.pending = false;
    let Ok(image) = event.image.clone().try_into_dynamic() else {
        exit.write(AppExit::error());
        return;
    };
    let image = image.to_rgba8();
    let left = image.get_pixel(80, 160).0;
    let right = image.get_pixel(400, 160).0;
    if !(left[0] > 100
        && left[0] > left[2].saturating_add(40)
        && right[2] > 100
        && right[2] > right[0].saturating_add(40))
    {
        return; // Shader compilation may still be in flight; never accept a blank image.
    }
    match image.save(&state.output) {
        Ok(()) => {
            println!(
                "MToon UV GPU proof passed: identity red, transformed blue; {}",
                state.output.display()
            );
            exit.write(AppExit::Success);
        }
        Err(error) => {
            eprintln!("Cannot save UV proof: {error}");
            exit.write(AppExit::error());
        }
    }
}
