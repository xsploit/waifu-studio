//! Render two meshes with one shared morph source and independent weights.
//! Replace and then remove the first mesh while the second keeps deforming.
use bevy::{
    app::{AppExit, ScheduleRunnerPlugin},
    camera::{RenderTarget, ScalingMode},
    core_pipeline::tonemapping::Tonemapping,
    mesh::morph::{MeshMorphWeights, MorphAttributes},
    prelude::*,
    render::{
        render_resource::{TextureFormat, TextureUsages},
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::time::Duration;

#[derive(Resource, Default)]
struct Check {
    target: Handle<Image>,
    first_mesh: Handle<Mesh>,
    first: Option<Entity>,
    phase: u8,
    frame: u32,
    pending: bool,
}

fn main() -> AppExit {
    App::new()
        .init_resource::<Check>()
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
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(16)))
        .add_systems(Startup, setup)
        .add_systems(Update, capture)
        .run()
}

fn setup(
    mut commands: Commands,
    mut check: ResMut<Check>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut image = Image::new_target_texture(640, 320, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    check.target = images.add(image);
    let source = Mesh::from(Rectangle::new(0.4, 0.4)).with_morph_targets(vec![
        MorphAttributes {
            position: Vec3::Y * 0.5,
            ..default()
        };
        4
    ]);
    check.first_mesh = meshes.add(source.clone());
    let second_mesh = meshes.add(source);
    check.first = Some(
        commands
            .spawn((
                Mesh3d(check.first_mesh.clone()),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(1., 0., 0.),
                    unlit: true,
                    ..default()
                })),
                Transform::from_xyz(-1., 0., 0.),
                MeshMorphWeights::Value { weights: vec![0.] },
            ))
            .id(),
    );
    commands.spawn((
        Mesh3d(second_mesh),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0., 1., 0.),
            unlit: true,
            ..default()
        })),
        Transform::from_xyz(1., 0., 0.),
        MeshMorphWeights::Value { weights: vec![1.] },
    ));
    commands.spawn((
        Camera3d::default(),
        Tonemapping::None,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        RenderTarget::Image(check.target.clone().into()),
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

fn capture(mut commands: Commands, mut check: ResMut<Check>, mut exit: MessageWriter<AppExit>) {
    check.frame += 1;
    if check.frame > 700 {
        eprintln!("Morph GPU check timed out in phase {}", check.phase);
        exit.write(AppExit::error());
    } else if check.frame >= 120 && check.frame.is_multiple_of(60) && !check.pending {
        check.pending = true;
        commands
            .spawn(Screenshot::image(check.target.clone()))
            .observe(verify);
    }
}

fn verify(
    event: On<ScreenshotCaptured>,
    mut check: ResMut<Check>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut weights: Query<&mut MeshMorphWeights>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    check.pending = false;
    let Ok(image) = event.image.clone().try_into_dynamic() else {
        exit.write(AppExit::error());
        return;
    };
    let image = image.to_rgba8();
    let red = |x, y| {
        let p = image.get_pixel(x, y).0;
        p[0] > 120 && p[0] > p[1].saturating_add(60)
    };
    let green = |x, y| {
        let p = image.get_pixel(x, y).0;
        p[1] > 120 && p[1] > p[0].saturating_add(60)
    };
    let ok = match check.phase {
        0 => red(160, 160) && green(480, 80),
        1 => red(160, 240) && green(480, 80),
        _ => !red(160, 240) && green(480, 80),
    };
    if !ok {
        return;
    } // Allow pipeline compilation before deciding failure.
    match check.phase {
        0 => {
            meshes
                .get_mut(&check.first_mesh)
                .unwrap()
                .set_morph_targets(vec![
                    MorphAttributes {
                        position: Vec3::NEG_Y * 0.5,
                        ..default()
                    };
                    4
                ]);
            *weights.get_mut(check.first.unwrap()).unwrap() =
                MeshMorphWeights::Value { weights: vec![1.] };
            check.phase = 1;
            check.frame = 0;
        }
        1 => {
            commands.entity(check.first.take().unwrap()).despawn();
            meshes.remove(check.first_mesh.id());
            check.phase = 2;
            check.frame = 0;
        }
        _ => {
            println!(
                "GPU morph sharing: independent weights, owner replacement and owner removal passed"
            );
            exit.write(AppExit::Success);
        }
    }
}
