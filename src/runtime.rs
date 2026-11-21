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

/// Bound frame-worker overhead on high-core hosts. I/O, async computation and
/// CPU affinity retain Bevy's policies; smaller hosts still use fewer workers.
/// A metrics-only override retains the original policy as an experimental arm.
pub fn task_pool_plugin() -> bevy::app::TaskPoolPlugin {
    let requested = std::env::var("WAIFU_RUNTIME_COMPUTE_THREADS").ok();
    bevy::app::TaskPoolPlugin {
        task_pool_options: task_pool_policy(
            std::env::var_os("WAIFU_RUNTIME_METRICS").is_some(),
            requested.as_deref(),
        ),
    }
}

fn task_pool_policy(metrics: bool, requested: Option<&str>) -> bevy::app::TaskPoolOptions {
    let defaults = bevy::app::TaskPoolOptions::default();
    bevy::app::TaskPoolOptions {
        compute: bevy::app::TaskPoolThreadAssignmentPolicy {
            max_threads: compute_thread_probe(metrics, requested).unwrap_or(4),
            ..defaults.compute
        },
        ..defaults
    }
}

fn compute_thread_probe(metrics: bool, requested: Option<&str>) -> Option<usize> {
    metrics
        .then_some(requested)
        .flatten()
        .and_then(|value| value.parse().ok())
        .filter(|threads| (1..=64).contains(threads))
}

/// Diagnostic-only surface queue-latency request; ordinary launches retain None.
fn frame_latency_probe(metrics: bool, requested: Option<&str>) -> Option<std::num::NonZeroU32> {
    metrics
        .then_some(requested)
        .flatten()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| (1..=3).contains(value))
        .and_then(std::num::NonZeroU32::new)
}

/// Prefer Mailbox with a renderer-enforced FIFO fallback on unsupported surfaces.
/// Explicit diagnostic candidates take precedence over the legacy no-vsync probe.
fn presentation_policy(
    metrics: bool,
    requested: Option<&str>,
    legacy_no_vsync: bool,
) -> bevy::window::PresentMode {
    use bevy::window::PresentMode;
    if !metrics {
        return PresentMode::Mailbox;
    }
    match requested {
        Some("fifo") => PresentMode::Fifo,
        Some("mailbox") => PresentMode::Mailbox,
        _ if legacy_no_vsync => PresentMode::AutoNoVsync,
        _ => PresentMode::Mailbox,
    }
}

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
        desired_maximum_frame_latency: frame_latency_probe(
            std::env::var_os("WAIFU_RUNTIME_METRICS").is_some(),
            std::env::var("WAIFU_RUNTIME_FRAME_LATENCY").ok().as_deref(),
        ),
        present_mode: presentation_policy(
            std::env::var_os("WAIFU_RUNTIME_METRICS").is_some(),
            std::env::var("WAIFU_RUNTIME_PRESENT_MODE").ok().as_deref(),
            runtime_probe("WAIFU_RUNTIME_NO_VSYNC"),
        ),
        ..default()
    }
}

impl Plugin for StudioRuntimePlugin {
    fn build(&self, app: &mut App) {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        app.add_systems(Last, reclaim_avatar_load_memory);
        if std::env::var_os("WAIFU_RUNTIME_METRICS").is_some() {
            app.init_resource::<crate::runtime_metrics::FramePhases>()
                .init_resource::<RuntimeMetricsSnapshot>()
                .add_systems(Last, log_runtime_metrics);
            crate::runtime_metrics::install_render_probes(app);
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
#[derive(Default)]
struct RuntimeSample {
    start: Option<Instant>,
    last: Option<Instant>,
    frames: u64,
    frame_ms: Vec<f64>,
    focus: Option<bool>,
    mixed_focus: bool,
}

/// Latest opt-in diagnostics window for the local control endpoint.
#[derive(Resource, Default)]
pub(crate) struct RuntimeMetricsSnapshot(pub serde_json::Value);

fn frame_percentiles(samples: &mut [f64]) -> (f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    samples.sort_unstable_by(f64::total_cmp);
    let at = |percentile: f64| samples[((samples.len() - 1) as f64 * percentile).ceil() as usize];
    (at(0.5), at(0.99))
}

fn log_runtime_metrics(
    windows: Query<&Window>,
    images: Res<Assets<Image>>,
    meshes: Res<Assets<Mesh>>,
    desktop: Res<DesktopSettings>,
    winit: Res<WinitSettings>,
    mut sample: Local<RuntimeSample>,
    mut snapshot: ResMut<RuntimeMetricsSnapshot>,
    phase_data: (
        ResMut<crate::runtime_metrics::FramePhases>,
        Option<Res<crate::runtime_metrics::RenderPhases>>,
    ),
) {
    let (mut phases, render_phases) = phase_data;
    let now = Instant::now();
    phases.end(now);
    let focused = windows.iter().any(|window| window.focused);
    let Some(start) = sample.start else {
        sample.start = Some(now);
        sample.last = Some(now);
        sample.focus = Some(focused);
        phases.clear_samples();
        return;
    };
    sample.frames += 1;
    if sample.focus != Some(focused) {
        sample.mixed_focus = true;
    }
    if let Some(last) = sample.last
        && sample.frame_ms.len() < 4096
    {
        sample
            .frame_ms
            .push(now.duration_since(last).as_secs_f64() * 1000.0);
    }
    sample.last = Some(now);
    let elapsed = now.duration_since(start).as_secs_f64();
    if elapsed >= 10.0 {
        let image_bytes: usize = images
            .iter()
            .filter_map(|(_, image)| image.data.as_ref().map(Vec::len))
            .sum();
        let (p50, p99) = frame_percentiles(&mut sample.frame_ms);
        let (morph_logical, morph_unique) = waifu_studio::loading::morph_cpu_bytes(&meshes);
        info!(
            "Runtime metrics: fps={:.1} focused={} image_cpu_mib={:.1} images={} meshes={} frame_ms_p50={:.2} frame_ms_p99={:.2} mixed_focus={} morph_logical_mib={:.2} morph_unique_mib={:.2} focused_cap={} loop={:?} present={:?} compute_threads={}",
            sample.frames as f64 / elapsed,
            focused,
            image_bytes as f64 / (1024.0 * 1024.0),
            images.len(),
            meshes.len(),
            p50,
            p99,
            sample.mixed_focus,
            morph_logical as f64 / (1024.0 * 1024.0),
            morph_unique as f64 / (1024.0 * 1024.0),
            desktop.focused_fps,
            winit.update_mode(focused),
            windows.iter().next().map(|window| window.present_mode),
            bevy::tasks::ComputeTaskPool::get().thread_num(),
        );
        let phases = phases.take_window();
        let render_installed = render_phases.is_some();
        let render = render_phases
            .as_ref()
            .map(|p| p.take_window())
            .unwrap_or_default();
        let (width, height) = windows.iter().next().map_or((0, 0), |window| {
            (
                window.resolution.physical_width(),
                window.resolution.physical_height(),
            )
        });
        snapshot.0 = serde_json::json!({
            "fps": sample.frames as f64 / elapsed,
            "frame_p50_ms": p50, "frame_p99_ms": p99,
            "focused": focused, "mixed_focus": sample.mixed_focus,
            "compute_threads": bevy::tasks::ComputeTaskPool::get().thread_num(),
            "pacing_sleep_ms": phases.sleep_ms,
            "main_work_ms": phases.work_ms,
            "between_main_ms": phases.between_ms,
            "prepare_windows_ms": render.prepare_ms,
            "render_system_ms": render.render_ms,
            "mesh_slab_bytes": render.mesh_slab_bytes,
            "mesh_allocations": render.mesh_allocations,
            "main_samples": phases.work_samples,
            "render_samples": render.render_samples,
            "render_probe_installed": render_installed,
            "physical_size": [width, height]
        });
        info!(
            "Runtime phase metrics: pacing_sleep_ms_p50={:.3} pacing_sleep_ms_p99={:.3} sampled_main_work_ms_p50={:.3} sampled_main_work_ms_p99={:.3} between_main_ms_p50={:.3} between_main_ms_p99={:.3} sleep_samples={} work_samples={} between_samples={} windows={} sampled_window_physical={}x{} present_is_requested=true prepare_windows_bracket_ms_p50={:.3} prepare_windows_bracket_ms_p99={:.3} render_system_bracket_ms_p50={:.3} render_system_bracket_ms_p99={:.3} prepare_samples={} render_samples={} render_probe_installed={} desired_frame_latency_requested={:?}",
            phases.sleep_ms.0,
            phases.sleep_ms.1,
            phases.work_ms.0,
            phases.work_ms.1,
            phases.between_ms.0,
            phases.between_ms.1,
            phases.sleep_samples,
            phases.work_samples,
            phases.between_samples,
            windows.iter().count(),
            width,
            height,
            render.prepare_ms.0,
            render.prepare_ms.1,
            render.render_ms.0,
            render.render_ms.1,
            render.prepare_samples,
            render.render_samples,
            render_installed,
            windows.iter().next().and_then(|window| window
                .desired_maximum_frame_latency
                .map(std::num::NonZeroU32::get))
        );
        sample.start = Some(now);
        sample.frames = 0;
        sample.frame_ms.clear();
        sample.focus = Some(focused);
        sample.mixed_focus = false;
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
    mut phases: Option<ResMut<crate::runtime_metrics::FramePhases>>,
) {
    if let Some(phases) = phases.as_mut() {
        phases.begin(Instant::now());
    }
    let focused = windows.iter().any(|window| window.focused);
    let active = studio.job.is_some() || studio.voice_busy || !playing.is_empty();
    let rate = polling_rate(&settings, focused, studio.mode, active) as f64;
    let interval = Duration::from_secs_f64(1.0 / rate);
    if let Some(previous) = *last
        && let Some(remaining) = interval.checked_sub(previous.elapsed())
    {
        let before = phases.as_ref().map(|_| Instant::now());
        std::thread::sleep(remaining);
        if let Some(before) = before
            && let Some(phases) = phases.as_mut()
        {
            phases.slept(before.elapsed());
        }
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

fn cadence_mode(rate: u32, diagnostic_continuous: bool) -> UpdateMode {
    if diagnostic_continuous {
        // pace_frames still sleeps to the selected cap. This opt-in probe only
        // separates event-loop timer wakeups from rendering/animation cost.
        UpdateMode::Continuous
    } else {
        UpdateMode::reactive_low_power(Duration::from_secs_f64(1.0 / rate as f64))
    }
}

fn runtime_probe(name: &str) -> bool {
    std::env::var_os("WAIFU_RUNTIME_METRICS").is_some()
        && std::env::var(name).is_ok_and(|value| value == "1")
}

fn frame_cadence(
    studio: Res<Studio>,
    desktop: Res<DesktopSettings>,
    playing: Query<(), With<crate::studio::SpokenAudio>>,
    mut settings: ResMut<WinitSettings>,
    mut probe: Local<Option<bool>>,
) {
    let active = studio.job.is_some() || studio.voice_busy || !playing.is_empty();
    let diagnostic = *probe.get_or_insert_with(|| runtime_probe("WAIFU_RUNTIME_CONTINUOUS"));
    let focused = cadence_mode(
        polling_rate(&desktop, true, studio.mode, active),
        diagnostic,
    );
    if settings.focused_mode != focused {
        settings.focused_mode = focused;
    }
    // Overlay animation and spring physics keep ticking when clickthrough means
    // the main window cannot receive focus. Network/audio workers remain async.
    let mode = cadence_mode(
        polling_rate(&desktop, false, studio.mode, active),
        diagnostic,
    );
    if settings.unfocused_mode != mode {
        settings.unfocused_mode = mode;
    }
}

#[derive(Component)]
pub(crate) struct StudioCamera;
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
    companion_size: Option<Vec2>,
    pending_studio_resolution: Option<bevy::window::WindowResolution>,
    original_window: Option<(
        bool,
        WindowLevel,
        bool,
        bevy::window::WindowResolution,
        bevy::window::WindowPosition,
        bool,
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
    desktop: bool,
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
            desktop: false,
        }
    }
}

/// Local developer control changes only this app's camera, without pointer input.
pub(crate) fn control_camera_view(world: &mut World, view: &str) -> Result<(), String> {
    let yaw = match view {
        "front" => 0.,
        "side" => std::f32::consts::FRAC_PI_2,
        "rear" => std::f32::consts::PI,
        "other-side" => -std::f32::consts::FRAC_PI_2,
        _ => return Err("Unknown camera view".into()),
    };
    let mut cameras = world.query_filtered::<&mut Orbit, With<StudioCamera>>();
    for mut orbit in cameras.iter_mut(world) {
        orbit.yaw = yaw;
        orbit.pitch = 0.;
    }
    Ok(())
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
    mut primary: Query<
        (
            Entity,
            &mut Window,
            &mut CursorOptions,
            Option<&bevy::window::OnMonitor>,
        ),
        With<PrimaryWindow>,
    >,
    monitors: Query<(
        Entity,
        &bevy::window::Monitor,
        Has<bevy::window::PrimaryMonitor>,
    )>,
    control_windows: Query<(), With<DesktopControlWindow>>,
    control_contexts: Query<(), (With<DesktopControlCamera>, With<EguiContext>)>,
    mut close_requests: MessageReader<WindowCloseRequested>,
    _main_thread: bevy::ecs::system::NonSendMarker,
) {
    let Ok((entity, mut window, mut cursor, current_monitor)) = primary.single_mut() else {
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
        desktop.controls_requested = false;
        commands.entity(camera).try_despawn();
        commands.entity(controls).try_despawn();
        studio.click_through = false;
        cursor.hit_test = true;
    }
    if studio.mode == ViewMode::Desktop {
        if state.original_window.is_none() {
            state.pending_studio_resolution = None;
            state.original_window = Some((
                window.decorations,
                window.window_level,
                window.has_shadow,
                window.resolution.clone(),
                window.position,
                bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
                    windows
                        .get_window(entity)
                        .is_some_and(|window| window.is_maximized())
                }),
            ));
            // A maximized transparent HWND can keep its old presentation
            // surface after the borderless resize, leaving only egui visible.
            // Leave maximized mode explicitly before using companion geometry.
            window.set_maximized(false);
            studio.frame_requested = true;
        }
        let monitor = current_monitor
            .and_then(|current| monitors.get(current.0).ok())
            .or_else(|| monitors.iter().find(|(_, _, primary)| *primary));
        let limit = monitor.map(|(_, monitor, _)| {
            Vec2::new(
                // An exact monitor-sized transparent window can trigger the
                // Windows fullscreen presentation path and lose desktop alpha.
                monitor
                    .physical_width
                    .saturating_sub(u32::from(cfg!(windows))) as f32,
                monitor
                    .physical_height
                    .saturating_sub(u32::from(cfg!(windows))) as f32,
            ) / monitor.scale_factor as f32
        });
        let size = companion_window_size(desktop.avatar_zoom, limit);
        if state.companion_size != Some(size) || window.resolution.size() != size {
            // Grow the render surface with the avatar instead of pushing her
            // head out of a fixed rectangle by moving the lens closer.
            window.resolution.set(size.x, size.y);
            state.companion_size = Some(size);
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
        if state.controls.is_none() && desktop.controls_requested {
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
            let hit_test = crate::desktop_behavior::desktop_accepts_input(
                studio.click_through,
                control_windows.contains(controls) && control_contexts.contains(camera),
            );
            if cursor.hit_test != hit_test {
                cursor.hit_test = hit_test;
            }
        } else {
            studio.click_through = false;
            cursor.hit_test = true;
        }
    } else {
        state.companion_size = None;
        desktop.controls_requested = false;
        if !cursor.hit_test {
            cursor.hit_test = true;
        }
        if studio.click_through {
            studio.click_through = false;
        }
        if let Some((decorations, level, shadow, resolution, position, maximized)) =
            state.original_window.take()
        {
            window.decorations = decorations;
            window.window_level = level;
            window.has_shadow = shadow;
            // Winit applies a requested inner size before restoring decorations.
            // On Windows, adding the frame afterwards shrinks the client area.
            // Reapply the saved size on the next update, after that transition.
            window.resolution = resolution.clone();
            // Maximized windows are restored by the window manager, not by a
            // subsequent client-size request that could undo maximization.
            state.pending_studio_resolution = (cfg!(windows) && !maximized).then_some(resolution);
            window.position = position;
            if maximized {
                window.set_maximized(true);
            }
            studio.frame_requested = true;
        } else if let Some(resolution) = state.pending_studio_resolution.take() {
            window.resolution = resolution;
        }
        if let Some((controls, camera)) = state.controls.take() {
            commands.entity(camera).try_despawn();
            commands.entity(controls).try_despawn();
        }
    }
}

fn companion_window_size(zoom: f32, monitor: Option<Vec2>) -> Vec2 {
    let zoom = if zoom.is_finite() {
        zoom.clamp(0.5, 4.0)
    } else {
        1.0
    };
    let size = Vec2::new(420.0, 640.0) * zoom;
    let fit = monitor
        .filter(|size| size.is_finite() && size.min_element() > 32.0)
        .map_or(1.0, |limit| {
            ((limit - Vec2::splat(16.0)) / size).min_element().min(1.0)
        });
    (size * fit).round()
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
    crate::ui::ensure_style(context.get_mut());
    let mut root_ui = egui::Ui::new(
        context.get_mut().clone(),
        egui::Id::new("desktop-controls"),
        egui::UiBuilder::new(),
    );
    egui::CentralPanel::default().show(&mut root_ui, |ui| {
        kit::section(ui, "Desktop companion");
        ui.add_space(7.0);
        ui.label("This window always stays interactive.");
        kit::note(ui, "Right-click for the companion menu. Left-drag to move.");
        if kit::toggle_row(
            ui,
            "Automatic desktop animations",
            "",
            &mut desktop.automatic_motions,
        )
        .changed()
        {
            desktop.mark_dirty();
        }
        kit::toggle_row(
            ui,
            "Pass clicks through the character window",
            "",
            &mut studio.click_through,
        );
        let mut sitting = desktop.sit_requested.unwrap_or(desktop.sitting);
        if kit::toggle_row(ui, "Sit on a detected edge", "", &mut sitting).changed() {
            desktop.request_sitting(sitting);
        }
        if !desktop.sitting_status.is_empty() {
            ui.small(&desktop.sitting_status);
        }
        if ui.button("Snap to bottom right").clicked() {
            desktop.perch.mode = waifu_studio::desktop_perch::PerchMode::ScreenRight;
            desktop.mark_dirty();
            desktop.perch_requested = true;
        }
        if !desktop.placement_status.is_empty() {
            ui.small(&desktop.placement_status);
        }
        ui.add_space(5.0);
        ui.horizontal_wrapped(|ui| {
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
    mut desktop: ResMut<DesktopSettings>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
    mut camera: Query<(&mut Orbit, &mut Transform, &mut Projection), With<StudioCamera>>,
    mut contexts: Query<&mut EguiContext, With<PrimaryEguiContext>>,
    primary: Query<(Entity, &Window), With<PrimaryWindow>>,
    mut studio: ResMut<Studio>,
) {
    // Rendering to an image still needs configuration/recenter updates. Only
    // actual desktop input depends on the presence of a focused native window.
    let primary = primary.single().ok();
    let focused = primary.is_some_and(|(_, window)| window.focused);
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
        .filter(|event| primary.is_some_and(|(entity, _)| event.window == entity))
        .map(|event| {
            event.y
                * if event.unit == MouseScrollUnit::Pixel {
                    0.01
                } else {
                    1.0
                }
        })
        .sum();
    let scene_visible = studio.mode == ViewMode::Desktop || studio.tab == crate::studio::Tab::Chat;
    // The preview/settings pages own their input, including focus shortcuts.
    if scene_visible && focused && !keyboard_captured && keys.just_pressed(KeyCode::KeyF) {
        studio.frame_requested = true;
    }
    let frame_requested = studio.frame_requested;
    if frame_requested {
        studio.frame_requested = false;
    }
    for (mut orbit, mut transform, mut projection) in &mut camera {
        let desktop_mode = studio.mode == ViewMode::Desktop;
        let room_view = studio.library.scene.room_view && studio.mode != ViewMode::Desktop;
        let portrait = studio.library.scene.portrait && studio.mode != ViewMode::Desktop;
        // Sitting changes the pose, not the lens or apparent avatar size. A
        // refit here captures transient animation bounds and can zoom/crop the
        // head while the seated crossfade is still moving through that fit.
        if frame_requested
            || orbit.room_view != room_view
            || orbit.portrait != portrait
            || orbit.desktop != desktop_mode
        {
            orbit.framed = None;
            orbit.framing = None;
            orbit.portrait = portrait;
            orbit.room_view = room_view;
            orbit.desktop = desktop_mode;
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
        // Settings pages draw on egui's background layer, which egui does not
        // report as pointer-captured; scrolling them must not zoom the hidden
        // scene camera. Only the conversation view steers the camera.
        if focused && !pointer_captured && !studio.click_through && scene_visible {
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
            if desktop_mode {
                if scroll != 0.0 {
                    desktop.avatar_zoom =
                        (desktop.avatar_zoom * (scroll * 0.10).exp()).clamp(0.5, 4.0);
                    desktop.mark_dirty();
                }
            } else {
                orbit.radius = (orbit.radius * (-scroll * 0.10).exp()).clamp(0.05, 1000.0);
            }
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
    let radius = if orbit.desktop {
        orbit.radius
    } else {
        orbit.radius / settings.avatar_zoom
    };
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
    fn presentation_policy_prefers_vsync_mailbox_and_gates_diagnostic_overrides() {
        use bevy::window::PresentMode;
        for requested in [
            None,
            Some("fifo"),
            Some("mailbox"),
            Some("immediate"),
            Some(""),
        ] {
            assert_eq!(
                presentation_policy(false, requested, true),
                PresentMode::Mailbox
            );
        }
        assert_eq!(
            presentation_policy(true, Some("mailbox"), false),
            PresentMode::Mailbox
        );
        assert_eq!(
            presentation_policy(true, Some("fifo"), true),
            PresentMode::Fifo
        );
        assert_eq!(
            presentation_policy(true, None, true),
            PresentMode::AutoNoVsync
        );
        assert_eq!(
            presentation_policy(true, Some("invalid"), false),
            PresentMode::Mailbox
        );
    }
    #[test]
    fn frame_latency_override_requires_metrics_and_valid_bounded_nonzero_input() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("4"),
            Some("-1"),
            Some("NaN"),
            Some("2x"),
        ] {
            assert!(frame_latency_probe(true, value).is_none());
        }
        for value in ["1", "2", "3"] {
            assert!(frame_latency_probe(false, Some(value)).is_none());
            assert_eq!(
                frame_latency_probe(true, Some(value)).unwrap().get(),
                value.parse::<u32>().unwrap()
            );
        }
    }
    #[test]
    fn compute_probe_requires_metrics_and_rejects_invalid_limits() {
        assert_eq!(compute_thread_probe(false, Some("4")), None);
        assert_eq!(compute_thread_probe(true, None), None);
        for invalid in ["", "0", "65", "-1", "auto", "184467440737095516160"] {
            assert_eq!(compute_thread_probe(true, Some(invalid)), None);
        }
        for (value, expected) in [("1", 1), ("4", 4), ("64", 64)] {
            assert_eq!(compute_thread_probe(true, Some(value)), Some(expected));
        }
        let defaults = bevy::app::TaskPoolOptions::default();
        for (metrics, value, expected) in [
            (false, None, 4),
            (false, Some("12"), 4),
            (true, None, 4),
            (true, Some("0"), 4),
            (true, Some("12"), 12),
        ] {
            let policy = task_pool_policy(metrics, value);
            assert_eq!(policy.compute.max_threads, expected);
            assert_eq!(policy.compute.min_threads, defaults.compute.min_threads);
            assert_eq!(policy.compute.percent, defaults.compute.percent);
            assert_eq!(policy.io.min_threads, defaults.io.min_threads);
            assert_eq!(policy.io.max_threads, defaults.io.max_threads);
            assert_eq!(policy.io.percent, defaults.io.percent);
            assert_eq!(
                policy.async_compute.min_threads,
                defaults.async_compute.min_threads
            );
            assert_eq!(
                policy.async_compute.max_threads,
                defaults.async_compute.max_threads
            );
            assert_eq!(policy.async_compute.percent, defaults.async_compute.percent);
        }
    }

    #[test]
    fn frame_percentiles_include_slow_tail_and_handle_short_samples() {
        assert_eq!(frame_percentiles(&mut []), (0.0, 0.0));
        assert_eq!(frame_percentiles(&mut [16.7]), (16.7, 16.7));
        let mut samples = vec![16.7; 99];
        samples.insert(0, 80.0);
        assert_eq!(frame_percentiles(&mut samples), (16.7, 80.0));
    }

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
    fn cadence_probe_changes_only_loop_policy_and_default_stays_reactive() {
        assert_eq!(
            cadence_mode(60, false),
            UpdateMode::reactive_low_power(Duration::from_secs_f64(1.0 / 60.0))
        );
        assert_eq!(
            cadence_mode(15, false),
            UpdateMode::reactive_low_power(Duration::from_secs_f64(1.0 / 15.0))
        );
        assert_eq!(cadence_mode(60, true), UpdateMode::Continuous);
        assert_eq!(cadence_mode(15, true), UpdateMode::Continuous);
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

    struct CameraFixture {
        app: App,
        camera: Entity,
        primary: Entity,
        directory: PathBuf,
    }

    impl Drop for CameraFixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.directory).unwrap();
        }
    }

    fn camera_fixture() -> CameraFixture {
        let directory =
            std::env::temp_dir().join(format!("studio-camera-{}", uuid::Uuid::new_v4()));
        let (store, library) = waifu_studio::data::Store::open(directory.clone()).unwrap();
        let mut studio = Studio::new(store, library);
        studio.library.scene.room_view = false;
        studio.library.scene.portrait = true;
        let mut app = App::new();
        app.insert_resource(studio)
            .init_resource::<DesktopSettings>()
            .init_resource::<crate::desktop_menu::DesktopMenu>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<AccumulatedMouseMotion>()
            .init_resource::<AvatarLoadState>()
            .init_resource::<Assets<Image>>()
            .init_resource::<bevy_egui::EguiUserTextures>()
            .add_message::<MouseWheel>()
            .add_plugins(crate::avatar_preview::AvatarPreviewPlugin)
            .add_systems(Update, camera_controls)
            .add_systems(PostUpdate, frame_avatar);
        let primary = app
            .world_mut()
            .spawn((
                PrimaryWindow,
                Window {
                    focused: true,
                    resolution: (1280, 900).into(),
                    ..default()
                },
            ))
            .id();
        let root = app.world_mut().spawn_empty().id();
        app.world_mut().spawn((
            ChildOf(root),
            Mesh3d::default(),
            Aabb::from_min_max(Vec3::new(-0.4, 0.0, -0.2), Vec3::new(0.4, 1.8, 0.2)),
            GlobalTransform::IDENTITY,
        ));
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        let camera = app
            .world_mut()
            .spawn((
                StudioCamera,
                Orbit::default(),
                Transform::default(),
                Projection::Perspective(PerspectiveProjection {
                    fov: 35_f32.to_radians(),
                    aspect_ratio: 1280.0 / 900.0,
                    ..default()
                }),
            ))
            .id();
        app.update();
        // A user has zoomed, panned and orbited after the initial automatic fit.
        {
            let mut orbit = app.world_mut().get_mut::<Orbit>(camera).unwrap();
            assert_eq!(orbit.framed, Some(root));
            orbit.focus = Vec3::new(0.31, 1.37, -0.16);
            orbit.radius = 1.23;
            orbit.yaw = 0.48;
            orbit.pitch = -0.19;
        }
        app.update();
        CameraFixture {
            app,
            camera,
            primary,
            directory,
        }
    }

    fn camera_pose(app: &App, camera: Entity) -> (Vec3, f32, f32, f32, Transform) {
        let orbit = app.world().get::<Orbit>(camera).unwrap();
        (
            orbit.focus,
            orbit.radius,
            orbit.yaw,
            orbit.pitch,
            *app.world().get::<Transform>(camera).unwrap(),
        )
    }

    #[test]
    fn desktop_scroll_grows_render_surface_without_cropping_the_avatar() {
        use bevy::camera::CameraProjection;
        let mut fixture = camera_fixture();
        fixture
            .app
            .init_resource::<DesktopState>()
            .add_message::<WindowCloseRequested>()
            .add_systems(Update, desktop_windows.before(camera_controls));
        fixture
            .app
            .world_mut()
            .entity_mut(fixture.primary)
            .insert(CursorOptions::default());
        fixture.app.world_mut().resource_mut::<Studio>().mode = ViewMode::Desktop;
        if let Projection::Perspective(projection) = fixture
            .app
            .world_mut()
            .get_mut::<Projection>(fixture.camera)
            .unwrap()
            .as_mut()
        {
            projection.update(420.0, 640.0);
        }
        fixture.app.update();
        fixture.app.update();
        let camera = camera_pose(&fixture.app, fixture.camera);
        let height = fixture
            .app
            .world()
            .get::<Window>(fixture.primary)
            .unwrap()
            .height();
        fixture.app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 2.0_f32.ln() / 0.1,
            phase: bevy::input::touch::TouchPhase::Moved,
            window: fixture.primary,
        });
        fixture.app.update();
        fixture.app.update();
        assert_eq!(camera_pose(&fixture.app, fixture.camera), camera);
        let window = fixture.app.world().get::<Window>(fixture.primary).unwrap();
        assert_eq!(window.height(), height * 2.0);
        assert_eq!(window.width(), 840.0);
        let projection = fixture
            .app
            .world()
            .get::<Projection>(fixture.camera)
            .unwrap();
        let clip = projection.get_clip_from_view() * camera.4.to_matrix().inverse();
        let top = clip.project_point3(Vec3::new(0.0, 1.8, 0.08));
        let bottom = clip.project_point3(Vec3::new(0.0, 1.58, 0.08));
        assert!(top.y.abs() < 0.95, "head stays inside the growing surface");
        let old_head_pixels = (top.y - bottom.y) * height * 0.5;
        let new_head_pixels = (top.y - bottom.y) * window.height() * 0.5;
        assert!((new_head_pixels / old_head_pixels - 2.0).abs() < 1e-5);
    }

    #[test]
    fn large_companion_fits_current_monitor_without_losing_aspect() {
        for monitor in [Vec2::new(1920.0, 1080.0), Vec2::new(720.0, 1280.0)] {
            let size = companion_window_size(4.0, Some(monitor));
            assert!(size.cmple(monitor - Vec2::splat(16.0)).all());
            assert!((size.x / size.y - 420.0 / 640.0).abs() < 0.002);
        }
    }

    #[test]
    fn sitting_transition_preserves_scale_and_headroom_in_companion_window() {
        use bevy::camera::CameraProjection;

        let mut fixture = camera_fixture();
        fixture.app.world_mut().resource_mut::<Studio>().mode = ViewMode::Desktop;
        fixture
            .app
            .world_mut()
            .get_mut::<Window>(fixture.primary)
            .unwrap()
            .resolution
            .set(420.0, 640.0);
        if let Projection::Perspective(projection) = fixture
            .app
            .world_mut()
            .get_mut::<Projection>(fixture.camera)
            .unwrap()
            .as_mut()
        {
            projection.update(420.0, 640.0);
        }
        fixture.app.update();
        fixture.app.update();
        let standing = camera_pose(&fixture.app, fixture.camera);
        let project = |app: &App, point: Vec3| {
            let projection = app.world().get::<Projection>(fixture.camera).unwrap();
            let transform = app.world().get::<Transform>(fixture.camera).unwrap();
            (projection.get_clip_from_view() * transform.to_matrix().inverse())
                .project_point3(point)
        };
        let head_top = Vec3::new(0.0, 1.8, 0.08);
        let head_bottom = Vec3::new(0.0, 1.58, 0.08);
        let head_size = project(&fixture.app, head_top).y - project(&fixture.app, head_bottom).y;
        let mesh = fixture
            .app
            .world_mut()
            .query_filtered::<Entity, With<Mesh3d>>()
            .single(fixture.app.world())
            .unwrap();
        for sitting in [true, false, true, false] {
            fixture
                .app
                .world_mut()
                .resource_mut::<DesktopSettings>()
                .sitting = sitting;
            // A transient, shorter pose must not become a new camera fit while
            // the head/torso continue to move through the animation crossfade.
            *fixture.app.world_mut().get_mut::<Aabb>(mesh).unwrap() = if sitting {
                Aabb::from_min_max(Vec3::new(-0.4, 0.7, -0.2), Vec3::new(0.4, 1.55, 0.4))
            } else {
                Aabb::from_min_max(Vec3::new(-0.4, 0.0, -0.2), Vec3::new(0.4, 1.8, 0.2))
            };
            for _ in 0..60 {
                fixture.app.update();
                assert_eq!(camera_pose(&fixture.app, fixture.camera), standing);
                for bob in [-0.03, 0.0, 0.03] {
                    let top = project(&fixture.app, head_top + Vec3::Y * bob);
                    assert!(top.x.abs() < 0.95 && top.y.abs() < 0.95, "{top:?}");
                    let bottom = project(&fixture.app, head_bottom + Vec3::Y * bob);
                    assert!(((top.y - bottom.y) / head_size - 1.0).abs() < 1e-5);
                }
            }
        }
    }

    #[test]
    fn sitting_and_standing_preserve_user_camera_adjustments() {
        let mut fixture = camera_fixture();
        fixture.app.world_mut().resource_mut::<Studio>().mode = ViewMode::Desktop;
        fixture.app.update();
        fixture.app.update();
        {
            let mut orbit = fixture
                .app
                .world_mut()
                .get_mut::<Orbit>(fixture.camera)
                .unwrap();
            orbit.focus = Vec3::new(0.2, 1.1, -0.1);
            orbit.radius = 2.7;
            orbit.yaw = 0.15;
            orbit.pitch = -0.05;
        }
        {
            let mut desktop = fixture.app.world_mut().resource_mut::<DesktopSettings>();
            desktop.avatar_zoom = 1.15;
            desktop.avatar_x = 0.1;
            desktop.avatar_y = -0.1;
        }
        fixture.app.update();
        let before = camera_pose(&fixture.app, fixture.camera);
        for sitting in [true, false] {
            fixture
                .app
                .world_mut()
                .resource_mut::<DesktopSettings>()
                .sitting = sitting;
            fixture.app.update();
            fixture.app.update();
            assert_eq!(camera_pose(&fixture.app, fixture.camera), before);
        }
    }

    #[test]
    fn settings_tabs_preserve_main_orbit_and_drain_hidden_scene_input() {
        use crate::studio::Tab;
        let mut fixture = camera_fixture();
        let before = camera_pose(&fixture.app, fixture.camera);
        let auxiliary = fixture.app.world_mut().spawn(Window::default()).id();
        for tab in [
            Tab::Scene,
            Tab::Characters,
            Tab::Connections,
            Tab::Voice,
            Tab::Memory,
            Tab::Tools,
            Tab::Tasks,
            Tab::SceneChat,
        ] {
            fixture.app.world_mut().resource_mut::<Studio>().tab = tab;
            {
                let mut buttons = fixture
                    .app
                    .world_mut()
                    .resource_mut::<ButtonInput<MouseButton>>();
                buttons.press(MouseButton::Left);
                buttons.press(MouseButton::Right);
            }
            fixture
                .app
                .world_mut()
                .resource_mut::<AccumulatedMouseMotion>()
                .delta = Vec2::new(30.0, -20.0);
            for (unit, y) in [
                (MouseScrollUnit::Line, -8.0),
                (MouseScrollUnit::Pixel, -150.0),
            ] {
                fixture.app.world_mut().write_message(MouseWheel {
                    unit,
                    x: 0.0,
                    y,
                    phase: bevy::input::touch::TouchPhase::Moved,
                    window: fixture.primary,
                });
            }
            // No egui capture is supplied: reproduce background-layer settings.
            fixture.app.update();
            assert_eq!(camera_pose(&fixture.app, fixture.camera), before, "{tab:?}");
        }
        fixture
            .app
            .world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .reset_all();
        fixture
            .app
            .world_mut()
            .resource_mut::<AccumulatedMouseMotion>()
            .delta = Vec2::ZERO;
        fixture.app.world_mut().resource_mut::<Studio>().tab = Tab::Chat;
        fixture.app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: -10.0,
            phase: bevy::input::touch::TouchPhase::Moved,
            window: auxiliary,
        });
        fixture.app.update();
        assert_eq!(camera_pose(&fixture.app, fixture.camera), before);
        fixture.app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: -1.0,
            phase: bevy::input::touch::TouchPhase::Moved,
            window: fixture.primary,
        });
        fixture.app.update();
        let after = camera_pose(&fixture.app, fixture.camera);
        assert!((after.1 - before.1 * 0.1_f32.exp()).abs() < 1e-6);
        assert_eq!((after.0, after.2, after.3), (before.0, before.2, before.3));
    }

    #[test]
    fn escape_in_companion_mode_does_not_open_the_studio() {
        let mut fixture = camera_fixture();
        fixture.app.world_mut().resource_mut::<Studio>().mode = ViewMode::Desktop;
        fixture
            .app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        fixture.app.update();
        assert_eq!(
            fixture.app.world().resource::<Studio>().mode,
            ViewMode::Desktop
        );
    }

    #[test]
    fn settings_focus_shortcut_does_not_reframe_main_camera() {
        use crate::studio::Tab;
        let mut fixture = camera_fixture();
        let before = camera_pose(&fixture.app, fixture.camera);
        for tab in [Tab::Scene, Tab::Characters, Tab::Connections, Tab::Voice] {
            fixture.app.world_mut().resource_mut::<Studio>().tab = tab;
            fixture
                .app
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::KeyF);
            fixture.app.update();
            assert_eq!(camera_pose(&fixture.app, fixture.camera), before, "{tab:?}");
            fixture
                .app
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .reset_all();
        }
        fixture.app.world_mut().resource_mut::<Studio>().tab = Tab::Chat;
        fixture.app.update();
        assert_eq!(camera_pose(&fixture.app, fixture.camera), before);
        fixture
            .app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyF);
        fixture.app.update();
        fixture.app.update();
        let after = camera_pose(&fixture.app, fixture.camera);
        assert_ne!(after, before);
        assert_eq!((after.2, after.3), (0.0, 0.0));
    }

    #[test]
    fn explicit_settings_recenter_and_framing_changes_still_apply() {
        let mut fixture = camera_fixture();
        let before = camera_pose(&fixture.app, fixture.camera);
        {
            let mut studio = fixture.app.world_mut().resource_mut::<Studio>();
            studio.tab = crate::studio::Tab::Scene;
            studio.frame_requested = true;
        }
        fixture.app.update();
        fixture.app.update();
        let portrait = camera_pose(&fixture.app, fixture.camera);
        assert_ne!(portrait, before);
        assert_eq!((portrait.2, portrait.3), (0.0, 0.0));
        fixture
            .app
            .world_mut()
            .resource_mut::<Studio>()
            .library
            .scene
            .portrait = false;
        fixture.app.update();
        fixture.app.update();
        let full_body = camera_pose(&fixture.app, fixture.camera);
        assert_ne!(full_body, portrait);
        fixture
            .app
            .world_mut()
            .resource_mut::<Studio>()
            .library
            .scene
            .room_view = true;
        fixture.app.update();
        let room = camera_pose(&fixture.app, fixture.camera);
        assert_eq!((room.0, room.1), (Vec3::new(0.0, 1.45, -1.2), 6.9));
        let Projection::Perspective(projection) = fixture
            .app
            .world()
            .get::<Projection>(fixture.camera)
            .unwrap()
        else {
            panic!("expected perspective camera");
        };
        assert_eq!(projection.fov, 45_f32.to_radians());
    }

    #[test]
    fn windowless_capture_applies_camera_settings_without_accepting_desktop_input() {
        let mut fixture = camera_fixture();
        fixture.app.world_mut().despawn(fixture.primary);
        {
            let mut studio = fixture.app.world_mut().resource_mut::<Studio>();
            studio.library.scene.portrait = false;
            studio.frame_requested = true;
        }
        fixture
            .app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyF);
        fixture
            .app
            .world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        fixture
            .app
            .world_mut()
            .resource_mut::<AccumulatedMouseMotion>()
            .delta = Vec2::splat(50.0);
        fixture.app.update();
        fixture.app.update();
        let orbit = fixture.app.world().get::<Orbit>(fixture.camera).unwrap();
        assert!(
            !orbit.portrait,
            "offscreen output must honor the requested full-body framing"
        );
        assert_eq!((orbit.yaw, orbit.pitch), (0.0, 0.0));
        assert!(!fixture.app.world().resource::<Studio>().frame_requested);
        let before = camera_pose(&fixture.app, fixture.camera);
        for tab in [
            crate::studio::Tab::Scene,
            crate::studio::Tab::Characters,
            crate::studio::Tab::Chat,
        ] {
            fixture.app.world_mut().resource_mut::<Studio>().tab = tab;
            fixture.app.update();
            assert_eq!(camera_pose(&fixture.app, fixture.camera), before);
        }
    }

    #[test]
    fn resize_during_settings_preserves_orbit_with_updated_projection() {
        use bevy::camera::CameraProjection;
        let mut fixture = camera_fixture();
        let before = camera_pose(&fixture.app, fixture.camera);
        fixture.app.world_mut().resource_mut::<Studio>().tab = crate::studio::Tab::Scene;
        for (width, height) in [(480.0, 960.0), (1920.0, 720.0), (1280.0, 900.0)] {
            fixture
                .app
                .world_mut()
                .get_mut::<Window>(fixture.primary)
                .unwrap()
                .resolution
                .set(width, height);
            // Exercise Bevy's real projection resize callback without a renderer.
            if let Projection::Perspective(projection) = fixture
                .app
                .world_mut()
                .get_mut::<Projection>(fixture.camera)
                .unwrap()
                .as_mut()
            {
                projection.update(width, height);
                assert_eq!(projection.aspect_ratio, width / height);
                assert_eq!(projection.fov, 35_f32.to_radians());
            }
            fixture.app.update();
            assert_eq!(camera_pose(&fixture.app, fixture.camera), before);
        }
        fixture.app.world_mut().resource_mut::<Studio>().tab = crate::studio::Tab::Chat;
        fixture.app.update();
        assert_eq!(camera_pose(&fixture.app, fixture.camera), before);
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
        assert_eq!(
            app.world_mut()
                .get_mut::<Window>(primary)
                .unwrap()
                .internal
                .take_maximize_request(),
            Some(false)
        );
        // Native un-maximizing may first report the restored Studio size.
        app.world_mut()
            .get_mut::<Window>(primary)
            .unwrap()
            .resolution
            .set(1280.0, 900.0);
        app.update();
        assert_eq!(
            app.world()
                .get::<Window>(primary)
                .unwrap()
                .resolution
                .size(),
            Vec2::new(420.0, 640.0)
        );
        assert!(
            app.world().resource::<DesktopState>().controls.is_none(),
            "Entering companion mode must not open settings or recovery controls"
        );
        assert!(app.world().get::<CursorOptions>(primary).unwrap().hit_test);
        assert!(!app.world().resource::<Studio>().click_through);
        app.update();
        assert!(app.world().resource::<DesktopState>().controls.is_none());
        // Only the explicit right-click 'Pass clicks' action requests its
        // recovery surface. Ordinary startup never reaches this path.
        app.world_mut()
            .resource_mut::<DesktopSettings>()
            .controls_requested = true;
        app.world_mut().resource_mut::<Studio>().click_through = true;
        app.update();
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
        #[cfg(windows)]
        {
            // Simulate the native decoration change reporting a smaller client area.
            app.world_mut()
                .get_mut::<Window>(primary)
                .unwrap()
                .resolution
                .set(1000., 700.);
            app.update();
            assert_eq!(
                app.world()
                    .get::<Window>(primary)
                    .unwrap()
                    .resolution
                    .size(),
                original_size
            );
            // The correction is one-shot; later user resizes must not be undone.
            app.world_mut()
                .get_mut::<Window>(primary)
                .unwrap()
                .resolution
                .set(1000., 700.);
            app.update();
            assert_eq!(
                app.world()
                    .get::<Window>(primary)
                    .unwrap()
                    .resolution
                    .size(),
                Vec2::new(1000., 700.)
            );
        }
        app.world_mut().resource_mut::<Studio>().mode = ViewMode::Desktop;
        app.update();
        assert!(
            app.world().resource::<DesktopState>().controls.is_none(),
            "Returning from explicit settings must not pop controls back open"
        );
        assert!(app.world().get::<CursorOptions>(primary).unwrap().hit_test);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
