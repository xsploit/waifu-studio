//! Native companion stage and recoverable desktop-overlay window management.
//! The renderer remains the same in both views; XR is deliberately a later layer.
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use waifu_studio::settings_kit as kit;

use bevy::{
    asset::AssetPath,
    camera::{RenderTarget, primitives::Aabb, visibility::RenderLayers},
    ecs::schedule::ScheduleLabel,
    input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel},
    prelude::*,
    window::{
        CompositeAlphaMode, CursorOptions, PrimaryWindow, WindowCloseRequested, WindowLevel,
        WindowRef,
    },
    winit::{UpdateMode, WinitSettings},
};
use bevy_egui::{EguiContext, EguiGlobalSettings, EguiSchedule, PrimaryEguiContext, egui};
use bevy_shader_mtoon::{MtoonSun, OutlineMaterial};
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName;

use crate::{
    desktop_settings::DesktopSettings,
    studio::{Studio, ViewMode},
};
use waifu_studio::{
    loading::AvatarLoadState,
    shaders::{ShaderMode, ShaderPreview, ShaderSettings},
};

pub struct StudioRuntimePlugin;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DesktopWindowInput;
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DesktopPerchRequests;

/// Transparency must be requested when the native window is created.
/// Homunculus also selects an explicit compositor alpha mode. The current Bevy
/// example uses premultiplied alpha on Linux and postmultiplied alpha on macOS.
pub fn primary_window() -> Window {
    Window {
        title: "Waifu Studio".into(),
        transparent: true,
        composite_alpha_mode: if cfg!(target_os = "macos") {
            CompositeAlphaMode::PostMultiplied
        } else {
            CompositeAlphaMode::PreMultiplied
        },
        resolution: (1280, 900).into(),
        ..default()
    }
}

impl Plugin for StudioRuntimePlugin {
    fn build(&self, app: &mut App) {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        app.add_systems(Last, reclaim_avatar_load_memory);
        if std::env::var_os("WAIFU_RUNTIME_METRICS").is_some() {
            app.add_systems(Last, log_runtime_metrics);
        }
        app.insert_resource(WinitSettings {
            focused_mode: UpdateMode::reactive_low_power(Duration::from_secs_f64(1.0 / 60.0)),
            unfocused_mode: UpdateMode::reactive_low_power(Duration::from_secs_f64(1.0 / 15.0)),
        })
        .init_resource::<DesktopSettings>()
        .init_resource::<DesktopState>()
        .init_resource::<BackdropState>()
        .add_systems(Startup, setup)
        .add_systems(First, pace_frames.before(bevy::time::TimeSystems))
        .add_systems(
            Update,
            (
                frame_cadence,
                desktop_windows.in_set(DesktopWindowInput),
                desktop_perching.in_set(DesktopPerchRequests),
                stage_settings.before(waifu_studio::environments::EnvironmentSystems),
                tag_avatar,
                camera_controls,
                idle_motion,
            )
                .chain(),
        )
        .add_systems(DesktopControlsPass, desktop_controls)
        .add_systems(
            PostUpdate,
            (relax_arms, frame_avatar).after(bevy::transform::TransformSystems::Propagate),
        );
    }
}

/// glibc can retain freed decoding/upload buffers in its thread arenas. Reclaim
/// unused pages once after a settled avatar change, outside active speech.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn reclaim_avatar_load_memory(
    avatar: Res<AvatarLoadState>,
    studio: Res<Studio>,
    playing: Query<(), With<crate::studio::SpokenAudio>>,
    mut previous: Local<Option<Entity>>,
    mut due: Local<Option<Instant>>,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if *previous != avatar.active {
        *previous = avatar.active;
        *due = Some(Instant::now() + Duration::from_secs(3));
    }
    if due.is_none_or(|due| Instant::now() < due)
        || studio.job.is_some()
        || studio.voice_busy
        || !playing.is_empty()
        || RUNNING.swap(true, Ordering::AcqRel)
    {
        return;
    }
    *due = None;
    if std::thread::Builder::new()
        .name("studio-memory-reclaim".into())
        .spawn(|| {
            // SAFETY: glibc's thread-safe API only releases unused allocator
            // pages; live allocations and their addresses are preserved.
            unsafe { libc::malloc_trim(0) };
            RUNNING.store(false, Ordering::Release);
        })
        .is_err()
    {
        RUNNING.store(false, Ordering::Release);
    }
}

/// Opt-in, content-free measurements for comparing builds at the same actual
/// frame rate and focus state. Disabled during ordinary launches.
fn log_runtime_metrics(
    windows: Query<&Window>,
    images: Res<Assets<Image>>,
    meshes: Res<Assets<Mesh>>,
    mut sample: Local<Option<(Instant, u64)>>,
) {
    let now = Instant::now();
    let Some((start, frames)) = sample.as_mut() else {
        *sample = Some((now, 0));
        return;
    };
    *frames += 1;
    let elapsed = now.duration_since(*start).as_secs_f64();
    if elapsed >= 10.0 {
        let image_bytes: usize = images
            .iter()
            .filter_map(|(_, image)| image.data.as_ref().map(Vec::len))
            .sum();
        info!(
            "Runtime metrics: fps={:.1} focused={} image_cpu_mib={:.1} images={} meshes={}",
            *frames as f64 / elapsed,
            windows.iter().any(|window| window.focused),
            image_bytes as f64 / (1024.0 * 1024.0),
            images.len(),
            meshes.len(),
        );
        *sample = Some((now, 0));
    }
}

/// Winit sleeps between frames instead of polling raw mouse movement elsewhere
/// on the desktop. A separate cap handles egui redraws and input bursts, which
/// otherwise bypass reactive timers on high-refresh displays. No busy spinning.
fn pace_frames(
    windows: Query<&Window>,
    studio: Res<Studio>,
    settings: Res<DesktopSettings>,
    playing: Query<(), With<crate::studio::SpokenAudio>>,
    mut last: Local<Option<Instant>>,
) {
    let focused = windows.iter().any(|window| window.focused);
    let active = studio.job.is_some() || studio.voice_busy || !playing.is_empty();
    let rate = polling_rate(&settings, focused, studio.mode, active) as f64;
    let interval = Duration::from_secs_f64(1.0 / rate);
    if let Some(previous) = *last
        && let Some(remaining) = interval.checked_sub(previous.elapsed())
    {
        std::thread::sleep(remaining);
    }
    *last = Some(Instant::now());
}

/// Keep streamed text, audio startup and lip sync responsive while a turn is
/// active even when the companion is unfocused; idle power caps stay intact.
fn polling_rate(settings: &DesktopSettings, focused: bool, mode: ViewMode, active: bool) -> u32 {
    if active {
        settings.focused_fps.max(60)
    } else if focused {
        settings.focused_fps
    } else if mode == ViewMode::Desktop {
        settings.overlay_fps
    } else {
        settings.background_fps
    }
}

fn frame_cadence(
    studio: Res<Studio>,
    desktop: Res<DesktopSettings>,
    playing: Query<(), With<crate::studio::SpokenAudio>>,
    mut settings: ResMut<WinitSettings>,
) {
    let active = studio.job.is_some() || studio.voice_busy || !playing.is_empty();
    let focused = UpdateMode::reactive_low_power(Duration::from_secs_f64(
        1.0 / polling_rate(&desktop, true, studio.mode, active) as f64,
    ));
    if settings.focused_mode != focused {
        settings.focused_mode = focused;
    }
    // Overlay animation and spring physics keep ticking when clickthrough means
    // the main window cannot receive focus. Network/audio workers remain async.
    let rate = polling_rate(&desktop, false, studio.mode, active) as f64;
    let mode = UpdateMode::reactive_low_power(Duration::from_secs_f64(1.0 / rate));
    if settings.unfocused_mode != mode {
        settings.unfocused_mode = mode;
    }
}

#[derive(Component)]
struct StudioCamera;
#[derive(Component)]
struct Backdrop;
#[derive(Component)]
struct DesktopControlWindow;
#[derive(Component)]
struct DesktopControlCamera;
#[derive(Component)]
struct RelaxedArms;
#[derive(Component)]
struct IdleRoot {
    rest: Transform,
}

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct DesktopControlsPass;

#[derive(Resource, Default)]
struct DesktopState {
    controls: Option<(Entity, Entity)>,
    controls_dismissed: bool,
    original_window: Option<(
        bool,
        WindowLevel,
        bool,
        bevy::window::WindowResolution,
        bevy::window::WindowPosition,
    )>,
}

#[derive(Resource, Default)]
struct BackdropState {
    path: Option<PathBuf>,
    image: Option<Handle<Image>>,
}

#[derive(Component)]
struct Orbit {
    focus: Vec3,
    radius: f32,
    yaw: f32,
    pitch: f32,
    framed: Option<Entity>,
    framing: Option<(Entity, u8)>,
    portrait: bool,
    room_view: bool,
    sitting: bool,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            focus: Vec3::new(0.0, 1.2, 0.0),
            radius: 1.8,
            yaw: 0.0,
            pitch: 0.0,
            framed: None,
            framing: None,
            portrait: true,
            room_view: false,
            sitting: false,
        }
    }
}

fn setup(
    mut commands: Commands,
    mut egui_settings: ResMut<EguiGlobalSettings>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    egui_settings.auto_create_primary_context = false;
    let backdrop_mesh = meshes.add(Rectangle::new(1.0, 1.0));
    let backdrop_material = materials.add(StandardMaterial {
        unlit: true,
        perceptual_roughness: 1.0,
        ..default()
    });
    commands
        .spawn((
            StudioCamera,
            waifu_studio::visuals::VisualCamera,
            Camera3d::default(),
            PrimaryEguiContext,
            Projection::Perspective(PerspectiveProjection {
                fov: 35.0_f32.to_radians(),
                ..default()
            }),
            Transform::from_xyz(0.0, 1.2, 1.8).looking_at(Vec3::new(0.0, 1.2, 0.0), Vec3::Y),
            Orbit::default(),
        ))
        .with_children(|parent| {
            parent.spawn((
                Backdrop,
                waifu_studio::shaders::ExcludeShaderPreview,
                Mesh3d(backdrop_mesh),
                MeshMaterial3d(backdrop_material),
                Transform::from_xyz(0.0, 0.0, -80.0),
                Visibility::Hidden,
            ));
        });
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-2.0, 4.0, 5.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        MtoonSun,
    ));
}

/// Input passthrough is never enabled without an independent recovery surface.
/// The controls use their own native window and egui camera, so the transparent
/// primary window can stop receiving mouse input without trapping the user.
#[allow(clippy::too_many_arguments)]
fn desktop_windows(
    mut commands: Commands,
    mut studio: ResMut<Studio>,
    mut state: ResMut<DesktopState>,
    mut desktop: ResMut<DesktopSettings>,
    mut primary: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    control_windows: Query<(), With<DesktopControlWindow>>,
    control_contexts: Query<(), (With<DesktopControlCamera>, With<EguiContext>)>,
    mut close_requests: MessageReader<WindowCloseRequested>,
    pointer: Option<Res<crate::desktop_behavior::DesktopPointerHit>>,
    grab: Option<Res<crate::desktop_behavior::DesktopGrab>>,
    menu: Option<Res<crate::desktop_menu::DesktopMenu>>,
) {
    let Ok((mut window, mut cursor)) = primary.single_mut() else {
        return;
    };
    if let Some((width, height)) = desktop.resize_requested.take() {
        window.resolution.set(width as f32, height as f32);
    }
    let closing = close_requests.read().any(|event| {
        state
            .controls
            .is_some_and(|(controls, _)| controls == event.window)
    });
    if let Some((controls, camera)) = state.controls
        && (closing || !control_windows.contains(controls))
    {
        // Closing auxiliary controls dismisses only that window. Keep the
        // companion interactive so its right-click menu remains recoverable.
        state.controls = None;
        state.controls_dismissed = true;
        commands.entity(camera).try_despawn();
        commands.entity(controls).try_despawn();
        studio.click_through = false;
        cursor.hit_test = true;
    }
    if studio.mode == ViewMode::Desktop {
        if state.original_window.is_none() {
            state.original_window = Some((
                window.decorations,
                window.window_level,
                window.has_shadow,
                window.resolution.clone(),
                window.position,
            ));
            window.resolution.set(420.0, 640.0);
            studio.frame_requested = true;
        }
        if window.decorations {
            window.decorations = false;
        }
        if window.has_shadow {
            window.has_shadow = false;
        }
        let level = if desktop.always_on_top {
            WindowLevel::AlwaysOnTop
        } else {
            WindowLevel::Normal
        };
        if window.window_level != level {
            window.window_level = level;
        }
        if state.controls.is_none() && !state.controls_dismissed {
            // Begin interactive. Passthrough can only be switched on once the
            // controls' own context is installed on a subsequent frame.
            cursor.hit_test = true;
            let controls = commands
                .spawn((
                    DesktopControlWindow,
                    Window {
                        title: "Waifu Studio · Desktop controls".into(),
                        resolution: (400, 380).into(),
                        resizable: false,
                        window_level: WindowLevel::AlwaysOnTop,
                        ..default()
                    },
                ))
                .id();
            let camera = commands
                .spawn((
                    DesktopControlCamera,
                    Camera2d,
                    Camera {
                        clear_color: ClearColorConfig::Custom(Color::srgb(0.08, 0.09, 0.085)),
                        ..default()
                    },
                    RenderTarget::Window(WindowRef::Entity(controls)),
                    RenderLayers::layer(31),
                    EguiSchedule::new(DesktopControlsPass),
                ))
                .id();
            state.controls = Some((controls, camera));
        } else if let Some((controls, camera)) = state.controls {
            let hit_test = pointer.as_deref().copied().unwrap_or_default().hit_test(
                studio.click_through,
                control_windows.contains(controls) && control_contexts.contains(camera),
                grab.as_ref().is_some_and(|grab| grab.active),
                menu.as_ref().is_some_and(|menu| menu.is_open()),
                Instant::now(),
            );
            if cursor.hit_test != hit_test {
                cursor.hit_test = hit_test;
            }
        } else {
            studio.click_through = false;
            cursor.hit_test = pointer.as_deref().copied().unwrap_or_default().hit_test(
                false,
                false,
                grab.as_ref().is_some_and(|grab| grab.active),
                menu.as_ref().is_some_and(|menu| menu.is_open()),
                Instant::now(),
            );
        }
    } else {
        state.controls_dismissed = false;
        if !cursor.hit_test {
            cursor.hit_test = true;
        }
        if studio.click_through {
            studio.click_through = false;
        }
        if let Some((decorations, level, shadow, resolution, position)) =
            state.original_window.take()
        {
            window.decorations = decorations;
            window.window_level = level;
            window.has_shadow = shadow;
            window.resolution = resolution;
            window.position = position;
            studio.frame_requested = true;
        }
        if let Some((controls, camera)) = state.controls.take() {
            commands.entity(camera).try_despawn();
            commands.entity(controls).try_despawn();
        }
    }
}

/// Explicit positioning request only; no external window enumeration or compositor changes.
fn desktop_perching(
    _main_thread: bevy::ecs::system::NonSendMarker,
    studio: Res<Studio>,
    mut desktop: ResMut<DesktopSettings>,
    mut window: Query<(Entity, &mut Window, Option<&bevy::window::OnMonitor>), With<PrimaryWindow>>,
    monitors: Query<(
        Entity,
        &bevy::window::Monitor,
        Has<bevy::window::PrimaryMonitor>,
    )>,
) {
    if !desktop.perch_requested {
        return;
    }
    desktop.perch_requested = false;
    if studio.mode != ViewMode::Desktop {
        desktop.placement_status = "Switch to Desktop mode to snap the companion.".into();
        return;
    }
    let Ok((entity, mut window, current)) = window.single_mut() else {
        desktop.placement_status = "Snap needs the companion window.".into();
        return;
    };
    let monitor = current
        .and_then(|on| monitors.get(on.0).ok())
        .or_else(|| monitors.iter().find(|(_, _, primary)| *primary));
    let Some((_, monitor, _)) = monitor else {
        desktop.placement_status = "Snap needs a detected monitor.".into();
        return;
    };
    let scale = monitor.scale_factor as f32;
    if !scale.is_finite() || scale <= 0.0 {
        desktop.placement_status = "Could not snap: the monitor scale is unavailable.".into();
        return;
    }
    let viewport = waifu_studio::desktop_perch::ViewportRect {
        origin: [0.0, 0.0],
        size: [
            monitor.physical_width as f32 / scale,
            monitor.physical_height as f32 / scale,
        ],
    };
    if let Some(p) = waifu_studio::desktop_perch::calculate_placement(
        &desktop.perch,
        viewport,
        [window.width(), window.height()],
    ) {
        window.resolution.set(p.avatar_size[0], p.avatar_size[1]);
        let offset = Vec2::from_array(p.window_origin) * scale;
        let native_position_supported = bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
            windows
                .get_window(entity)
                .is_some_and(|window| window.inner_position().is_ok())
        });
        desktop.move_requested = Some(crate::desktop_settings::DesktopMoveRequest {
            logical_origin: monitor.physical_position.as_vec2() / scale
                + Vec2::from_array(p.window_origin),
            physical_origin: monitor.physical_position + offset.as_ivec2(),
            native_position_supported,
        });
    } else {
        desktop.placement_status = "Choose a snap position before requesting placement.".into();
    }
}

fn desktop_controls(
    mut contexts: Query<&mut EguiContext, With<DesktopControlCamera>>,
    mut studio: ResMut<Studio>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    mut desktop: ResMut<DesktopSettings>,
) {
    let Ok(mut context) = contexts.single_mut() else {
        return;
    };
    let mut root_ui = egui::Ui::new(
        context.get_mut().clone(),
        egui::Id::new("desktop-controls"),
        egui::UiBuilder::new(),
    );
    egui::CentralPanel::default().show(&mut root_ui, |ui| {
        kit::section(ui, "Desktop companion");
        ui.add_space(7.0);
        ui.label("This window always stays interactive.");
        kit::note(ui, "Right-click the companion for its radial menu. Grab it with the left button to move it.");
        if kit::toggle_row(ui, "Automatic desktop animations", "", &mut desktop.automatic_motions).changed() {
            desktop.mark_dirty();
        }
        kit::toggle_row(ui, "Pass clicks through the character window", "", &mut studio.click_through);
        let mut sitting = desktop.sit_requested.unwrap_or(desktop.sitting);
        if kit::toggle_row(ui, "Sit on a detected edge", "", &mut sitting)
            .changed()
        {
            desktop.request_sitting(sitting);
        }
        if !desktop.sitting_status.is_empty() { ui.small(&desktop.sitting_status); }
        if ui.button("Snap to bottom right").clicked() {
            desktop.perch.mode = waifu_studio::desktop_perch::PerchMode::ScreenRight;
            desktop.mark_dirty();
            desktop.perch_requested = true;
        }
        if !desktop.placement_status.is_empty() { ui.small(&desktop.placement_status); }
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            if ui.button("Move companion").clicked() {
                studio.click_through = false;
                if let Ok(mut window) = window.single_mut() {
                    window.start_drag_move();
                }
            }
            if ui.button("Reframe").clicked() {
                studio.frame_requested = true;
            }
        });
        ui.add_space(7.0);
        if ui.button("Return to studio").clicked() {
            studio.mode = ViewMode::Studio;
            studio.click_through = false;
        }
        ui.small("Close these controls to leave just the companion on your desktop.");
    });
}

#[allow(clippy::too_many_arguments)]
fn stage_settings(
    mut appearance: ResMut<waifu_studio::appearance::AppearanceState>,
    mut facial: ResMut<waifu_studio::expressions::FacialState>,
    mut environment: ResMut<waifu_studio::environments::EnvironmentSettings>,
    mut visuals: ResMut<waifu_studio::visuals::VisualState>,
    mut pose: ResMut<waifu_studio::desktop_pose::DesktopPoseSettings>,
    studio: Res<Studio>,
    desktop: Res<DesktopSettings>,
    mut lights: Query<&mut DirectionalLight, With<MtoonSun>>,
    mut shaders: ResMut<ShaderSettings>,
    mut clear: ResMut<ClearColor>,
    mut backdrop: ResMut<BackdropState>,
    server: Res<AssetServer>,
    images: Res<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    camera: Query<&Projection, With<StudioCamera>>,
    mut backgrounds: Query<
        (
            &MeshMaterial3d<StandardMaterial>,
            &mut Visibility,
            &mut Transform,
        ),
        With<Backdrop>,
    >,
) {
    if appearance.config != studio.library.scene.appearance {
        appearance.config = studio.library.scene.appearance.clone();
    }
    if facial.config != studio.library.scene.facial {
        facial.config = studio.library.scene.facial.clone();
    }
    if visuals.settings != studio.library.scene.visuals {
        visuals.settings = studio.library.scene.visuals.clone();
    }
    visuals.suppressed = studio.mode == ViewMode::Desktop;
    pose.sitting = desktop.sitting && studio.mode == ViewMode::Desktop;
    environment.kind = studio.library.scene.environment;
    environment.suppressed = studio.mode == ViewMode::Desktop;
    // MToon uses its own direct-light shading without sampling shadow maps.
    // A plain/unlit backdrop cannot receive shadows either. Keep shadow passes
    // only when a room or the lit avatar appearance can actually use them.
    let shadows = environment.is_active() || studio.library.scene.shader == 1;
    for mut light in &mut lights {
        let room_factor = if environment.suppressed {
            1.0
        } else {
            match environment.kind {
                waifu_studio::environments::EnvironmentKind::RoseBedroom => 0.65,
                waifu_studio::environments::EnvironmentKind::VioletStudio => 0.12,
                waifu_studio::environments::EnvironmentKind::CyanLounge => 0.20,
                _ => 1.0,
            }
        };
        let illuminance = 10_000.0 * desktop.light_intensity * room_factor;
        if light.illuminance != illuminance {
            light.illuminance = illuminance;
        }
        if light.shadow_maps_enabled != shadows {
            light.shadow_maps_enabled = shadows;
        }
    }
    let settings = &studio.library.scene;
    let mode = match settings.shader {
        1 => ShaderMode::Lit,
        2 => ShaderMode::Unlit,
        _ => ShaderMode::Toon,
    };
    if shaders.mode != mode {
        shaders.mode = mode;
    }
    if shaders.outlines != settings.outlines {
        shaders.outlines = settings.outlines;
    }
    let clear_color = if studio.mode == ViewMode::Desktop {
        Color::NONE
    } else {
        Color::srgb(
            settings.background[0],
            settings.background[1],
            settings.background[2],
        )
    };
    if clear.0 != clear_color {
        clear.0 = clear_color;
    }
    // A photo hidden behind a 3D room or the transparent desktop should not
    // retain its decoded pixels and GPU texture. The saved choice is untouched.
    let image_path = if studio.mode == ViewMode::Studio && !environment.is_active() {
        settings.image.as_ref()
    } else {
        None
    };
    if backdrop.path.as_ref() != image_path {
        backdrop.path = image_path.cloned();
        backdrop.image = backdrop
            .path
            .as_ref()
            .map(|path| server.load(AssetPath::from_path(path).into_owned()));
    }
    for (handle, mut visibility, mut transform) in &mut backgrounds {
        let image = backdrop
            .image
            .as_ref()
            .and_then(|handle| images.get(handle));
        let desired_visibility =
            if studio.mode == ViewMode::Desktop || environment.is_active() || image.is_none() {
                Visibility::Hidden
            } else {
                Visibility::Visible
            };
        if *visibility != desired_visibility {
            *visibility = desired_visibility;
        }
        // Avoid emitting AssetEvent::Modified (and uploading the material) every
        // frame; dropping this texture handle releases the previous backdrop.
        if materials
            .get(&handle.0)
            .is_some_and(|material| material.base_color_texture != backdrop.image)
            && let Some(mut material) = materials.get_mut(&handle.0)
        {
            material.base_color_texture = backdrop.image.clone();
        }
        if let Ok(Projection::Perspective(projection)) = camera.single() {
            let height = 2.0 * 80.0 * (projection.fov * 0.5).tan();
            let width = height * projection.aspect_ratio;
            let aspect = image
                .map(|image| image.width() as f32 / image.height().max(1) as f32)
                .unwrap_or(projection.aspect_ratio);
            // Cover the viewport without stretching the selected photograph.
            let scale = if width / height > aspect {
                Vec3::new(width, width / aspect, 1.0)
            } else {
                Vec3::new(height * aspect, height, 1.0)
            };
            if transform.scale != scale {
                transform.scale = scale;
            }
        }
    }
}

fn tag_avatar(
    mut commands: Commands,
    state: Res<AvatarLoadState>,
    roots: Query<&Transform, Without<IdleRoot>>,
) {
    if let Some(root) = state.active
        && let Ok(transform) = roots.get(root)
    {
        commands
            .entity(root)
            .insert((ShaderPreview, IdleRoot { rest: *transform }));
    }
}

#[allow(clippy::too_many_arguments)]
fn camera_controls(
    desktop: Res<DesktopSettings>,
    menu: Res<crate::desktop_menu::DesktopMenu>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
    mut camera: Query<(&mut Orbit, &mut Transform, &mut Projection), With<StudioCamera>>,
    mut contexts: Query<&mut EguiContext, With<PrimaryEguiContext>>,
    primary: Query<(Entity, &Window), With<PrimaryWindow>>,
    mut studio: ResMut<Studio>,
) {
    let Ok((primary_entity, window)) = primary.single() else {
        return;
    };
    let (pointer_captured, keyboard_captured) = contexts
        .single_mut()
        .map(|mut context| {
            let ctx = context.get_mut();
            (
                ctx.egui_wants_pointer_input(),
                ctx.egui_wants_keyboard_input(),
            )
        })
        .unwrap_or((false, false));
    let scroll: f32 = wheel
        .read()
        .filter(|event| event.window == primary_entity)
        .map(|event| {
            event.y
                * if event.unit == MouseScrollUnit::Pixel {
                    0.01
                } else {
                    1.0
                }
        })
        .sum();
    if window.focused && !keyboard_captured && keys.just_pressed(KeyCode::KeyF) {
        studio.frame_requested = true;
    }
    if window.focused
        && !keyboard_captured
        && keys.just_pressed(KeyCode::Escape)
        && studio.mode == ViewMode::Desktop
        && !menu.is_open()
    {
        studio.mode = ViewMode::Studio;
        studio.click_through = false;
    }
    let frame_requested = studio.frame_requested;
    if frame_requested {
        studio.frame_requested = false;
    }
    for (mut orbit, mut transform, mut projection) in &mut camera {
        let room_view = studio.library.scene.room_view && studio.mode != ViewMode::Desktop;
        let sitting = desktop.sitting && studio.mode == ViewMode::Desktop;
        let portrait =
            studio.library.scene.portrait && !sitting && studio.mode != ViewMode::Desktop;
        if frame_requested
            || orbit.room_view != room_view
            || orbit.portrait != portrait
            || orbit.sitting != sitting
        {
            orbit.framed = None;
            orbit.framing = None;
            orbit.portrait = portrait;
            orbit.sitting = sitting;
            orbit.room_view = room_view;
            if let Projection::Perspective(p) = projection.as_mut() {
                p.fov = if room_view { 45.0_f32 } else { 35.0_f32 }.to_radians();
            }
            if room_view {
                orbit.focus = Vec3::new(0.0, 1.45, -1.2);
                orbit.radius = 6.9;
                orbit.yaw = 0.0;
                orbit.pitch = 0.04;
            }
        }
        if window.focused && !pointer_captured && !studio.click_through {
            if studio.mode == ViewMode::Studio && buttons.pressed(MouseButton::Left) {
                orbit.yaw -= motion.delta.x * 0.006;
                orbit.pitch = (orbit.pitch + motion.delta.y * 0.006).clamp(-1.35, 1.35);
            }
            if studio.mode == ViewMode::Studio
                && (buttons.pressed(MouseButton::Right) || buttons.pressed(MouseButton::Middle))
            {
                let pan = (-transform.right() * motion.delta.x + transform.up() * motion.delta.y)
                    * orbit.radius
                    * 0.0012;
                orbit.focus += pan;
            }
            orbit.radius = (orbit.radius * (-scroll * 0.10).exp()).clamp(0.05, 1000.0);
        }
        let mut next = *transform;
        position_camera(&orbit, &desktop, &mut next);
        if *transform != next {
            *transform = next;
        }
    }
}

fn position_camera(orbit: &Orbit, settings: &DesktopSettings, transform: &mut Transform) {
    let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, -orbit.pitch, 0.0);
    let radius = orbit.radius / settings.avatar_zoom;
    let focus = orbit.focus
        + Vec3::new(
            -settings.avatar_x * radius * 0.25,
            -settings.avatar_y * radius * 0.25,
            0.0,
        );
    transform.translation = focus + rotation * Vec3::new(0.0, 0.0, radius);
    transform.look_at(focus, Vec3::Y);
}

fn idle_motion(
    studio: Res<Studio>,
    time: Res<Time>,
    mut roots: Query<(&IdleRoot, &mut Transform)>,
) {
    for (idle, mut transform) in &mut roots {
        let mut next = idle.rest;
        if studio.library.scene.idle_motion {
            let t = time.elapsed_secs();
            next.rotation = idle.rest.rotation
                * Quat::from_euler(
                    EulerRot::YXZ,
                    (t * 0.43).sin() * 0.018,
                    0.0,
                    (t * 0.67).sin() * 0.003,
                );
        }
        if *transform != next {
            *transform = next;
        }
    }
}

/// Only correct nearly horizontal humanoid upper arms. Work in world space and
/// convert back through the actual parent rotation, retaining each rig's rest
/// orientation instead of assuming identical local bone axes across exporters.
#[allow(clippy::too_many_arguments)]
fn relax_arms(
    animation: Res<waifu_studio::animation::AnimationState>,
    mut commands: Commands,
    state: Res<AvatarLoadState>,
    roster: Option<Res<waifu_studio::scene_roster::SceneRoster>>,
    parents: Query<&ChildOf>,
    globals: Query<&GlobalTransform>,
    bones: Query<(Entity, &HumanBoneName, &GlobalTransform)>,
    rest: Query<&waifu_studio::animation::HumanoidRestTransform>,
    mut transforms: Query<&mut Transform>,
    done: Query<(), With<RelaxedArms>>,
) {
    let extra_roots = roster
        .as_ref()
        .into_iter()
        .flat_map(|r| r.slots.iter().filter_map(|slot| slot.load.active));
    for root in state.active.into_iter().chain(extra_roots) {
        if (Some(root) == state.active && animation.is_posing()) || done.contains(root) {
            continue;
        }
        // Wait for instantiated bones rather than permanently marking an empty root.
        if !bones
            .iter()
            .any(|(entity, _, _)| parents.iter_ancestors(entity).any(|parent| parent == root))
        {
            continue;
        }

        // Scene entities can arrive after Update's rest capture. Never lower
        // either arm until both original bind transforms have been recorded.
        if [HumanBoneName::LeftUpperArm, HumanBoneName::RightUpperArm]
            .iter()
            .any(|arm| {
                !bones.iter().any(|(entity, name, _)| {
                    name == arm
                        && parents.iter_ancestors(entity).any(|parent| parent == root)
                        && rest.contains(entity)
                })
            })
        {
            continue;
        }

        for (upper_name, lower_name) in [
            (HumanBoneName::LeftUpperArm, HumanBoneName::LeftLowerArm),
            (HumanBoneName::RightUpperArm, HumanBoneName::RightLowerArm),
        ] {
            let upper = bones.iter().find(|(entity, name, _)| {
                **name == upper_name && parents.iter_ancestors(*entity).any(|parent| parent == root)
            });
            let lower = bones.iter().find(|(entity, name, _)| {
                **name == lower_name && parents.iter_ancestors(*entity).any(|parent| parent == root)
            });
            let (Some((entity, _, upper)), Some((_, _, lower))) = (upper, lower) else {
                continue;
            };
            let direction = (lower.translation() - upper.translation()).normalize_or_zero();
            if direction.length_squared() < 0.5 || direction.y.abs() > 0.45 {
                continue;
            }
            let desired = Vec3::new(direction.x.signum() * 0.32, -0.94, 0.06).normalize();
            let delta = Quat::from_rotation_arc(direction, desired);
            let parent_rotation = parents
                .get(entity)
                .ok()
                .and_then(|parent| globals.get(parent.parent()).ok())
                .map(|global| global.to_scale_rotation_translation().1)
                .unwrap_or(Quat::IDENTITY);
            if let Ok(mut transform) = transforms.get_mut(entity) {
                transform.rotation =
                    parent_rotation.inverse() * delta * parent_rotation * transform.rotation;
            }
        }
        commands.entity(root).insert(RelaxedArms);
    }
}

#[allow(clippy::type_complexity)]
fn frame_avatar(
    state: Res<AvatarLoadState>,
    roster: Option<Res<waifu_studio::scene_roster::SceneRoster>>,
    mut roster_revision: Local<u64>,
    parents: Query<&ChildOf>,
    meshes: Query<
        (Entity, &Aabb, &GlobalTransform),
        (
            With<Mesh3d>,
            Without<Backdrop>,
            Without<MeshMaterial3d<OutlineMaterial>>,
        ),
    >,
    mut cameras: Query<(&mut Orbit, &Projection), With<StudioCamera>>,
) {
    let Some(root) = state.active else { return };
    let revision = roster.as_ref().map_or(0, |r| r.revision);
    let scene_changed = *roster_revision != revision;
    *roster_revision = revision;
    for (mut orbit, projection) in &mut cameras {
        if orbit.room_view {
            continue;
        }
        if scene_changed {
            orbit.framed = None;
            orbit.framing = None;
        }
        if orbit.framed == Some(root) {
            continue;
        }
        let attempts = match orbit.framing {
            Some((attempted, attempts)) if attempted == root => attempts,
            _ => 0,
        };
        if attempts >= 30 {
            orbit.framed = Some(root);
            warn!(
                "Avatar has no usable mesh bounds; keeping the current camera. Press F to retry."
            );
            continue;
        }
        orbit.framing = Some((root, attempts + 1));
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for (entity, aabb, transform) in &meshes {
            if !parents.iter_ancestors(entity).any(|ancestor| {
                ancestor == root
                    || roster.as_ref().is_some_and(|r| {
                        r.slots
                            .iter()
                            .any(|slot| slot.load.active == Some(ancestor))
                    })
            }) {
                continue;
            }
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    for z in [-1.0, 1.0] {
                        let point = transform.transform_point(
                            Vec3::from(aabb.center)
                                + Vec3::from(aabb.half_extents) * Vec3::new(x, y, z),
                        );
                        min = min.min(point);
                        max = max.max(point);
                    }
                }
            }
        }
        if !min.is_finite() || !max.is_finite() {
            continue;
        }
        let extent = max - min;
        let height = extent.y.max(0.1);
        orbit.focus = (min + max) * 0.5;
        let (half_fov, aspect) = match projection {
            Projection::Perspective(projection) => (projection.fov * 0.5, projection.aspect_ratio),
            _ => (0.35, 1.0),
        };
        if orbit.portrait
            && !roster
                .as_ref()
                .is_some_and(|r| r.slots.iter().any(|slot| slot.load.active.is_some()))
        {
            orbit.focus.y = min.y + height * 0.80;
            let vertical = height * 0.28 / half_fov.tan();
            let horizontal = height * 0.23 / (half_fov.tan() * aspect.max(0.2));
            orbit.radius = vertical.max(horizontal);
            // Keep the character at 60% viewport width beside the left rail/title.
            orbit.focus.x -= orbit.radius * half_fov.tan() * aspect * 0.20;
        } else {
            orbit.radius = (height * 0.58 / half_fov.tan())
                .max(extent.x * 0.58 / (half_fov.tan() * aspect.max(0.2)))
                + extent.z * 0.5;
        }
        orbit.yaw = 0.0;
        orbit.pitch = 0.0;
        orbit.framed = Some(root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_turns_keep_fast_polling_and_idle_restores_power_caps() {
        let mut settings = DesktopSettings::default();
        settings.focused_fps = 30;
        settings.overlay_fps = 20;
        settings.background_fps = 5;
        for mode in [ViewMode::Studio, ViewMode::Desktop] {
            assert_eq!(polling_rate(&settings, false, mode, true), 60);
            assert_eq!(polling_rate(&settings, true, mode, true), 60);
        }
        assert_eq!(polling_rate(&settings, false, ViewMode::Studio, false), 5);
        assert_eq!(polling_rate(&settings, false, ViewMode::Desktop, false), 20);
        assert_eq!(polling_rate(&settings, true, ViewMode::Studio, false), 30);
        settings.focused_fps = 144;
        assert_eq!(polling_rate(&settings, false, ViewMode::Desktop, true), 144);
    }
    #[test]
    fn idle_arm_relaxation_waits_for_both_original_bind_snapshots() {
        use waifu_studio::animation::{AnimationState, HumanoidRestTransform};
        let mut app = App::new();
        app.init_resource::<AnimationState>()
            .init_resource::<AvatarLoadState>()
            .add_systems(Update, relax_arms);
        let root = app
            .world_mut()
            .spawn((Transform::default(), GlobalTransform::default()))
            .id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        let mut arms = Vec::new();
        for (upper, lower, sign) in [
            (
                HumanBoneName::LeftUpperArm,
                HumanBoneName::LeftLowerArm,
                -1.0,
            ),
            (
                HumanBoneName::RightUpperArm,
                HumanBoneName::RightLowerArm,
                1.0,
            ),
        ] {
            let entity = app
                .world_mut()
                .spawn((
                    upper,
                    Transform::default(),
                    GlobalTransform::from_translation(Vec3::new(sign * 0.1, 1.0, 0.0)),
                    ChildOf(root),
                ))
                .id();
            app.world_mut().spawn((
                lower,
                Transform::default(),
                GlobalTransform::from_translation(Vec3::new(sign * 0.5, 1.0, 0.0)),
                ChildOf(entity),
            ));
            arms.push(entity);
        }
        app.update();
        assert!(app.world().get::<RelaxedArms>(root).is_none());
        app.world_mut()
            .entity_mut(arms[0])
            .insert(HumanoidRestTransform(Transform::default()));
        app.update();
        assert!(app.world().get::<RelaxedArms>(root).is_none());
        assert_eq!(
            app.world().get::<Transform>(arms[0]).unwrap().rotation,
            Quat::IDENTITY
        );
        app.world_mut()
            .entity_mut(arms[1])
            .insert(HumanoidRestTransform(Transform::default()));
        app.update();
        assert!(app.world().get::<RelaxedArms>(root).is_some());
        for arm in arms {
            assert!(
                !app.world()
                    .get::<Transform>(arm)
                    .unwrap()
                    .rotation
                    .abs_diff_eq(Quat::IDENTITY, 0.01)
            );
            assert_eq!(
                app.world()
                    .get::<HumanoidRestTransform>(arm)
                    .unwrap()
                    .0
                    .rotation,
                Quat::IDENTITY
            );
        }
    }

    #[test]
    fn snap_is_explicit_uses_current_monitor_scale_and_does_not_repeat() {
        let directory = std::env::temp_dir().join(format!("studio-snap-{}", uuid::Uuid::new_v4()));
        let (store, library) = waifu_studio::data::Store::open(directory.clone()).unwrap();
        let mut studio = Studio::new(store, library);
        studio.mode = ViewMode::Desktop;
        let mut app = App::new();
        app.insert_resource(studio)
            .init_resource::<DesktopSettings>()
            .add_systems(Update, desktop_perching);
        let monitor = app
            .world_mut()
            .spawn(bevy::window::Monitor {
                name: None,
                physical_width: 3840,
                physical_height: 2160,
                physical_position: IVec2::new(-3840, 0),
                refresh_rate_millihertz: None,
                scale_factor: 2.0,
                video_modes: Vec::new(),
            })
            .id();
        let primary = app
            .world_mut()
            .spawn((
                Window {
                    resolution: (640, 720).into(),
                    ..default()
                },
                PrimaryWindow,
                bevy::window::OnMonitor(monitor),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Window>(primary).unwrap().position,
            bevy::window::WindowPosition::Automatic
        );
        assert!(
            app.world()
                .resource::<DesktopSettings>()
                .move_requested
                .is_none()
        );
        {
            let mut settings = app.world_mut().resource_mut::<DesktopSettings>();
            settings.perch.mode = waifu_studio::desktop_perch::PerchMode::ScreenRight;
            settings.perch_requested = true;
        }
        app.update();
        // Placement queues both coordinate spaces for the platform consumer.
        // A headless fixture has no supported native window to move directly.
        let request = app
            .world_mut()
            .resource_mut::<DesktopSettings>()
            .move_requested
            .take()
            .expect("an explicit snap should queue one movement request");
        assert_eq!(request.logical_origin, Vec2::new(-652.0, 348.0));
        assert_eq!(request.physical_origin, IVec2::new(-1304, 696));
        assert!(!request.native_position_supported);
        assert_eq!(
            app.world().get::<Window>(primary).unwrap().position,
            bevy::window::WindowPosition::Automatic
        );
        app.world_mut().get_mut::<Window>(primary).unwrap().position =
            bevy::window::WindowPosition::At(IVec2::ZERO);
        app.update();
        assert_eq!(
            app.world().get::<Window>(primary).unwrap().position,
            bevy::window::WindowPosition::At(IVec2::ZERO)
        );
        assert!(
            app.world()
                .resource::<DesktopSettings>()
                .move_requested
                .is_none()
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn empty_avatar_framing_stops_retrying_and_new_avatar_gets_a_fresh_budget() {
        let mut app = App::new();
        app.init_resource::<AvatarLoadState>()
            .add_systems(Update, frame_avatar);
        let root = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        let camera = app
            .world_mut()
            .spawn((StudioCamera, Orbit::default(), Projection::default()))
            .id();
        for _ in 0..40 {
            app.update();
        }
        let orbit = app.world().get::<Orbit>(camera).unwrap();
        assert_eq!(orbit.framed, Some(root));
        assert_eq!(orbit.framing, Some((root, 30)));
        let replacement = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(replacement);
        app.update();
        assert_eq!(
            app.world().get::<Orbit>(camera).unwrap().framing,
            Some((replacement, 1))
        );
    }
    #[test]
    fn closing_controls_keeps_desktop_interactive_until_explicit_studio_return() {
        let directory =
            std::env::temp_dir().join(format!("studio-desktop-{}", uuid::Uuid::new_v4()));
        let (store, library) = waifu_studio::data::Store::open(directory.clone()).unwrap();
        let mut studio = Studio::new(store, library);
        studio.mode = ViewMode::Desktop;
        studio.click_through = true;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(studio)
            .init_resource::<DesktopState>()
            .init_resource::<DesktopSettings>()
            .add_message::<WindowCloseRequested>()
            .add_systems(Update, desktop_windows);
        let primary = app
            .world_mut()
            .spawn((primary_window(), PrimaryWindow, CursorOptions::default()))
            .id();
        let window = app.world().get::<Window>(primary).unwrap();
        assert!(window.transparent);
        assert_ne!(window.composite_alpha_mode, CompositeAlphaMode::Auto);
        let original_decorations = window.decorations;
        let original_shadow = window.has_shadow;
        let original_size = window.resolution.size();
        let original_position = window.position;
        app.update();
        let window = app.world().get::<Window>(primary).unwrap();
        assert!(!window.decorations);
        assert!(!window.has_shadow);
        assert_eq!(window.resolution.size(), Vec2::new(420.0, 640.0));
        let (controls, camera) = app.world().resource::<DesktopState>().controls.unwrap();
        app.world_mut().entity_mut(camera).remove::<EguiContext>();
        assert!(app.world().get::<CursorOptions>(primary).unwrap().hit_test);
        app.update();
        assert!(app.world().get::<CursorOptions>(primary).unwrap().hit_test);
        app.world_mut()
            .entity_mut(camera)
            .insert(EguiContext::default());
        app.update();
        assert!(!app.world().get::<CursorOptions>(primary).unwrap().hit_test);
        app.world_mut()
            .write_message(WindowCloseRequested { window: controls });
        app.update();
        assert_eq!(app.world().resource::<Studio>().mode, ViewMode::Desktop);
        assert!(app.world().get::<CursorOptions>(primary).unwrap().hit_test);
        assert!(!app.world().resource::<Studio>().click_through);
        app.update();
        assert!(app.world().resource::<DesktopState>().controls.is_none());
        assert_eq!(
            app.world()
                .get::<Window>(primary)
                .unwrap()
                .resolution
                .size(),
            Vec2::new(420., 640.)
        );
        app.world_mut().resource_mut::<Studio>().mode = ViewMode::Studio;
        app.update();
        let window = app.world().get::<Window>(primary).unwrap();
        assert_eq!(window.decorations, original_decorations);
        assert_eq!(window.has_shadow, original_shadow);
        assert_eq!(window.resolution.size(), original_size);
        assert_eq!(window.position, original_position);
        assert!(app.world().resource::<DesktopState>().controls.is_none());
        assert!(app.world().get_entity(controls).is_err());
        assert!(app.world().get_entity(camera).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
