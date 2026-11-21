//! Desktop interaction host; native compositor dragging keeps working on Wayland.
//! Window-edge placement is applied only when the backend supplies a global position.
use crate::{
    desktop_menu::DesktopMenu,
    desktop_settings::{DesktopMoveRequest, DesktopSettings},
    studio::{Studio, ViewMode},
};
use bevy::{
    prelude::*,
    window::{PrimaryWindow, WindowPosition},
};
use bevy_egui::PrimaryEguiContext;
use serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use waifu_studio::{
    animation::{AnimationState, BvhClip, parse_vrma},
    desktop_interaction::SurfaceId,
    desktop_interaction::{
        DesktopInteraction, InteractionFrame, InteractionInput, InteractionState,
    },
    desktop_occlusion::{DesktopOcclusion, DesktopOcclusionPlugin},
    desktop_platform::{DesktopBridge, DesktopSnapshot},
    loading::AvatarLoadState,
};

pub struct DesktopBehaviorPlugin;
impl Plugin for DesktopBehaviorPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(DesktopOcclusionPlugin)
            .init_resource::<DesktopInteraction>()
            .init_resource::<DesktopGrab>()
            .init_resource::<DesktopMenu>()
            .init_resource::<DesktopMotions>()
            .init_resource::<DesktopPlatform>()
            .add_systems(
                PostUpdate,
                grab_companion.after(bevy::transform::TransformSystems::Propagate),
            )
            .add_systems(PostUpdate, sync_occlusion.after(grab_companion))
            .add_systems(Update, (sync_desktop_platform, play_desktop_motion))
            .add_systems(
                Update,
                consume_desktop_move_request
                    .after(crate::runtime::DesktopPerchRequests)
                    .after(sync_desktop_platform),
            )
            .add_systems(
                Update,
                track_desktop_pointer.before(crate::runtime::DesktopWindowInput),
            )
            .add_systems(bevy_egui::EguiPrimaryContextPass, crate::desktop_menu::ui);
    }
}

fn route_move_request(
    request: DesktopMoveRequest,
    window: &mut Window,
    bridge_move: Option<&dyn Fn(Vec2) -> Result<(), String>>,
) -> String {
    let mut failure = None;
    if let Some(send) = bridge_move {
        match send(request.logical_origin) {
            Ok(()) => {
                return "Snap requested through the desktop bridge; waiting for the compositor position.".into();
            }
            Err(error) => failure = Some(error),
        }
    }
    if request.native_position_supported {
        window.position = WindowPosition::At(request.physical_origin);
        "Snap requested on the current monitor.".into()
    } else {
        format!("Could not snap: {}", failure.unwrap_or_else(|| "Wayland needs connected window detection to move the companion. Enable it and try again.".into()))
    }
}

fn consume_desktop_move_request(
    platform: Res<DesktopPlatform>,
    mut settings: ResMut<DesktopSettings>,
    mut primary: Query<&mut Window, With<PrimaryWindow>>,
) {
    let Some(mut request) = settings.move_requested.take() else {
        return;
    };
    let Ok(mut window) = primary.single_mut() else {
        settings.placement_status = "Could not snap: the companion window is unavailable.".into();
        return;
    };
    settings.placement_status = if let Some(bridge) = &platform.bridge {
        let placement = bridge.snapshot().ok().and_then(|snapshot| {
            verified_snap_placement(
                &settings,
                &snapshot,
                request.logical_origin,
                window.resolution.size(),
            )
        });
        let Some(placement) = placement else {
            settings.placement_status =
                "Could not snap: waiting for verified desktop bounds.".into();
            return;
        };
        request.logical_origin = Vec2::from_array(placement.window_origin);
        request.native_position_supported = false;
        window
            .resolution
            .set(placement.avatar_size[0], placement.avatar_size[1]);
        route_move_request(
            request,
            &mut window,
            Some(&|origin| bridge.request_move(origin)),
        )
    } else {
        route_move_request(request, &mut window, None)
    };
}

fn verified_snap_placement(
    settings: &DesktopSettings,
    snapshot: &DesktopSnapshot,
    target: Vec2,
    size: Vec2,
) -> Option<waifu_studio::desktop_perch::PerchPlacement> {
    let area = snapshot
        .surfaces
        .iter()
        .filter(|surface| matches!(surface.id, SurfaceId::Monitor(_)))
        .min_by(|a, b| {
            let distance = |rect: waifu_studio::desktop_interaction::ScreenRect| {
                target.distance_squared(target.max(rect.origin).min(rect.origin + rect.size))
            };
            distance(a.rect).total_cmp(&distance(b.rect))
        })?;
    waifu_studio::desktop_perch::calculate_placement(
        &settings.perch,
        waifu_studio::desktop_perch::ViewportRect {
            origin: area.rect.origin.to_array(),
            size: area.rect.size.to_array(),
        },
        size.to_array(),
    )
}

/// Native input stays enabled unless the user explicitly chose full
/// passthrough and its separate recovery window is ready.
pub(crate) fn desktop_accepts_input(manual_pass: bool, recovery_ready: bool) -> bool {
    !manual_pass || !recovery_ready
}

fn trusted_viewport_cursor(snapshot: &DesktopSnapshot, now: Instant) -> Option<Vec2> {
    let age = now.checked_duration_since(snapshot.received_at)?;
    if age > Duration::from_millis(500) {
        return None;
    }
    let cursor = snapshot.cursor - snapshot.own_window_rect?.origin;
    cursor.is_finite().then_some(cursor)
}

fn track_desktop_pointer(
    studio: Res<Studio>,
    platform: Res<DesktopPlatform>,
    state: Res<AvatarLoadState>,
    mut gaze_pointer: ResMut<waifu_studio::gaze::GazePointer>,
) {
    let now = Instant::now();
    gaze_pointer.update(None, now);
    if studio.mode != ViewMode::Desktop || state.active.is_none() {
        return;
    }
    if let Some(snapshot) = platform
        .bridge
        .as_ref()
        .and_then(|bridge| bridge.snapshot().ok())
    {
        gaze_pointer.update(
            trusted_viewport_cursor(&snapshot, now),
            snapshot.received_at,
        );
    }
}

#[derive(Resource, Default)]
pub(crate) struct DesktopGrab {
    pub active: bool,
    bridge_drag: bool,
    compositor_seen: bool,
    started: Option<std::time::Instant>,
    avatar: Option<Entity>,
}

#[derive(Resource, Default)]
pub(crate) struct DesktopPlatform {
    bridge: Option<Arc<DesktopBridge>>,
    pending: Option<Mutex<mpsc::Receiver<Result<DesktopBridge, String>>>>,
    attempted: bool,
    above_panels: Option<bool>,
}
impl DesktopPlatform {
    /// Current compositor geometry, including Wayland's otherwise unavailable
    /// client origin. The bridge rejects expired samples itself.
    pub(crate) fn snapshot(&self) -> Option<DesktopSnapshot> {
        self.bridge
            .as_ref()
            .and_then(|bridge| bridge.snapshot().ok())
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_occlusion(
    mut commands: Commands,
    studio: Res<Studio>,
    settings: Res<DesktopSettings>,
    grab: Res<DesktopGrab>,
    interaction: Res<DesktopInteraction>,
    platform: Res<DesktopPlatform>,
    state: Res<AvatarLoadState>,
    cameras: Query<
        (Entity, &Camera, &GlobalTransform, Option<&DesktopOcclusion>),
        With<crate::runtime::StudioCamera>,
    >,
    bones: Query<(Entity, &HumanBoneName, &GlobalTransform)>,
    parents: Query<&ChildOf>,
) {
    for (entity, camera, camera_transform, previous) in &cameras {
        let mask = (|| {
            if studio.mode != ViewMode::Desktop
                || !settings.window_occlusion
                || !settings.sitting
                || grab.active
            {
                return None;
            }
            let seat = interaction.seated_surface()?;
            let snapshot = platform.bridge.as_ref()?.snapshot().ok()?;
            let root = state.active?;
            let (_, _, hips) = bones.iter().find(|(entity, bone, _)| {
                **bone == HumanBoneName::Hips
                    && (*entity == root
                        || parents
                            .iter_ancestors(*entity)
                            .any(|ancestor| ancestor == root))
            })?;
            // A small relative depth clearance keeps the contact surface just
            // behind the pelvis, independent of avatar/world scale.
            let depth = camera.world_to_ndc(camera_transform, hips.translation())?.z * 0.99;
            Some(DesktopOcclusion::for_seat(
                &snapshot,
                seat,
                depth,
                Instant::now(),
            ))
        })()
        .unwrap_or_default();
        sync_camera_mask(&mut commands, entity, previous, mask);
    }
}

pub(crate) fn sync_camera_mask(
    commands: &mut Commands,
    entity: Entity,
    previous: Option<&DesktopOcclusion>,
    mask: DesktopOcclusion,
) {
    // Bevy's SyncComponentPlugin removes the render-world counterpart when
    // the main-world component is removed, so no stale ledge remains.
    if mask.is_empty() {
        if previous.is_some() {
            commands.entity(entity).remove::<DesktopOcclusion>();
        }
    } else if previous != Some(&mask) {
        commands.entity(entity).insert(mask);
    }
}

fn sync_desktop_platform(
    studio: Res<Studio>,
    mut settings: ResMut<DesktopSettings>,
    mut platform: ResMut<DesktopPlatform>,
    #[cfg(windows)] primary: Query<&bevy::window::RawHandleWrapper, With<PrimaryWindow>>,
) {
    if studio.mode != ViewMode::Desktop || !settings.kwin_window_surfaces {
        platform.pending = None;
        platform.attempted = false;
        platform.above_panels = None;
        if let Some(bridge) = platform.bridge.take() {
            // Unloading a compositor script may block on D-Bus, never the frame loop.
            std::thread::spawn(move || drop(bridge));
        }
        if !settings.surface_status.is_empty() {
            settings.surface_status.clear();
        }
        return;
    }
    if !platform.attempted {
        #[cfg(windows)]
        let native_window = {
            let Ok(handle) = primary.single() else {
                return;
            };
            let wgpu::rwh::RawWindowHandle::Win32(handle) = handle.get_window_handle() else {
                return;
            };
            handle.hwnd.get() as usize
        };
        platform.attempted = true;
        settings.surface_status = "Starting temporary desktop window tracking…".into();
        let (sender, receiver) = mpsc::sync_channel(1);
        platform.pending = Some(Mutex::new(receiver));
        std::thread::spawn(move || {
            #[cfg(windows)]
            let _ = sender.send(DesktopBridge::start_windows(native_window));
            #[cfg(not(windows))]
            let _ = sender.send(DesktopBridge::start("Waifu Studio"));
        });
    }
    let result = platform.pending.as_ref().and_then(|pending| {
        pending
            .lock()
            .ok()
            .and_then(|receiver| receiver.try_recv().ok())
    });
    if let Some(result) = result {
        platform.pending = None;
        match result {
            Ok(bridge) => platform.bridge = Some(Arc::new(bridge)),
            Err(error) => settings.surface_status = error,
        }
    }
    if let Some(bridge) = &platform.bridge {
        let stacking_error = bridge.request_keep_above(settings.always_on_top).err();
        let snapshot = bridge.snapshot();
        let status: std::borrow::Cow<'_, str> = match snapshot {
            Ok(snapshot) if snapshot.own_window_rect.is_some() => {
                if platform.above_panels != snapshot.own_above_panels {
                    platform.above_panels = snapshot.own_above_panels;
                    info!(
                        "Desktop companion above panels: {:?}",
                        snapshot.own_above_panels
                    );
                }
                if let Some(error) = stacking_error {
                    error.into()
                } else if settings.always_on_top && snapshot.own_above_panels == Some(false) {
                    "Desktop connected; waiting for the compositor to place the companion above panels.".into()
                } else {
                    "Desktop geometry connected; drag above a window edge or taskbar to sit.".into()
                }
            }
            Ok(_) => "Desktop bridge connected; waiting for the companion window.".into(),
            Err(error) => error.into(),
        };
        if settings.surface_status != status {
            settings.surface_status = status.into_owned();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Motion {
    Idle,
    Grabbed,
    Sitting,
}
#[derive(Resource, Default)]
struct DesktopMotions {
    clips: Vec<(Motion, Arc<BvhClip>, PathBuf)>,
    initialized: bool,
    previous: Option<AnimationState>,
    selected: Option<Motion>,
}
impl DesktopMotions {
    fn initialize(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        for (motion, name, bytes) in [
            (
                Motion::Idle,
                "idle-maid.vrma",
                include_bytes!("../assets/animations/desktop/idle-maid.vrma").as_slice(),
            ),
            (
                Motion::Grabbed,
                "grabbed.vrma",
                include_bytes!("../assets/animations/desktop/grabbed.vrma").as_slice(),
            ),
            (
                Motion::Sitting,
                "idle-sitting.vrma",
                include_bytes!("../assets/animations/desktop/idle-sitting.vrma").as_slice(),
            ),
        ] {
            match parse_vrma(bytes) {
                Ok(clip) => self.clips.push((
                    motion,
                    Arc::new(clip),
                    PathBuf::from("assets/animations/desktop").join(name),
                )),
                Err(error) => error!("Bundled desktop motion {name}: {error}"),
            }
        }
    }
}

fn play_desktop_motion(
    studio: Res<Studio>,
    settings: Res<DesktopSettings>,
    grab: Res<DesktopGrab>,
    interaction: Res<DesktopInteraction>,
    mut motions: ResMut<DesktopMotions>,
    mut animation: ResMut<AnimationState>,
) {
    if studio.mode != ViewMode::Desktop || !settings.automatic_motions {
        if let Some(previous) = motions.previous.take() {
            animation.restore_parked(previous);
        }
        motions.selected = None;
        return;
    }
    motions.initialize();
    let next = if grab.active {
        Motion::Grabbed
    } else if interaction.state == InteractionState::Sitting && settings.sitting {
        Motion::Sitting
    } else {
        Motion::Idle
    };
    if motions.selected == Some(next) {
        return;
    }
    let Some((_, clip, path)) = motions.clips.iter().find(|(motion, _, _)| *motion == next) else {
        return;
    };
    let (clip, path) = (clip.clone(), path.clone());
    if motions.previous.is_none() {
        motions.previous = Some(animation.park_for_desktop());
    }
    animation.select_clip(clip, Some(path));
    motions.selected = Some(next);
}

#[allow(clippy::too_many_arguments)]
fn grab_companion(
    (_main_thread, time): (bevy::ecs::system::NonSendMarker, Res<Time>),
    studio: Res<Studio>,
    menu: Res<DesktopMenu>,
    platform: Res<DesktopPlatform>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut mouse_events: MessageReader<bevy::input::mouse::MouseButtonInput>,
    state: Res<AvatarLoadState>,
    mut grab: ResMut<DesktopGrab>,
    mut interaction: ResMut<DesktopInteraction>,
    mut settings: ResMut<DesktopSettings>,
    mut primary: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<PrimaryEguiContext>>,
    (bones, globals): (
        Query<(Entity, &HumanBoneName, &GlobalTransform)>,
        Query<&GlobalTransform>,
    ),
    parents: Query<&ChildOf>,
) {
    // App-wide ButtonInput also sees auxiliary windows. Only a real press
    // delivered to the companion window may start moving it.
    let pressed_window = mouse_events
        .read()
        .filter(|event| event.button == MouseButton::Left && event.state.is_pressed())
        .map(|event| event.window)
        .last();
    if studio.mode != ViewMode::Desktop {
        *grab = DesktopGrab::default();
        *interaction = DesktopInteraction::default();
        settings.sitting = false;
        if settings.sit_requested.take() == Some(true) {
            settings.sitting_status = "Switch to Desktop mode to sit on a detected surface.".into();
        }
        return;
    }
    let (Ok((window_entity, mut window)), Ok((camera, camera_transform)), Some(root)) =
        (primary.single_mut(), cameras.single(), state.active)
    else {
        fail_sit_request(
            &mut settings,
            &mut interaction,
            "Sitting needs a loaded avatar and its desktop viewport.",
        );
        return;
    };
    if grab.avatar != Some(root) {
        *grab = DesktopGrab {
            avatar: Some(root),
            ..default()
        };
        *interaction = DesktopInteraction::default();
        settings.sitting = false;
    }
    let cursor = window.cursor_position();
    let snapshot = platform
        .bridge
        .as_ref()
        .and_then(|bridge| bridge.snapshot().ok());
    let native_origin = bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
        windows
            .get_window(window_entity)
            .and_then(|window| window.inner_position().ok())
            .map(|position| IVec2::new(position.x, position.y))
    });
    let descendant = |entity: Entity| {
        entity == root
            || parents
                .iter_ancestors(entity)
                .any(|ancestor| ancestor == root)
    };
    let logical_origin = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.own_window_rect.map(|rect| rect.origin))
        .or_else(|| native_origin.map(|origin| origin.as_vec2() / window.scale_factor()));
    if !studio.click_through
        && !menu.blocks_drag()
        && pressed_window == Some(window_entity)
        && let Some(cursor) = cursor
        && cursor.is_finite()
    {
        grab.active = true;
        grab.compositor_seen = false;
        grab.started = Some(std::time::Instant::now());
        settings.sitting = false;
        settings.sit_requested = None;
        // Hyprland cannot report native drag completion. Keep Wayland's normal
        // implicit pointer grab and move through its bridge instead, so release
        // is delivered to us. KWin can observe a compositor-owned native grab.
        grab.bridge_drag = snapshot.as_ref().is_some_and(|sample| {
            sample.own_window_rect.is_some() && sample.own_interactive_move.is_none()
        });
        if !grab.bridge_drag {
            window.start_drag_move();
        }
        if let Some(origin) = logical_origin {
            let frame = InteractionFrame {
                avatar_origin: origin,
                avatar_size: window.resolution.size(),
                hips_offset: Vec2::ZERO,
                root_offset: Vec2::ZERO,
                feet_offset: None,
                pixels_per_unit: 1.0,
                surfaces: &[],
                own_window_ids: &[],
            };
            interaction.handle(
                InteractionInput::Start {
                    cursor: frame.avatar_origin + cursor,
                },
                &frame,
            );
        }
    }
    let compositor_drag = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.own_interactive_move);
    grab.compositor_seen |= grab.active && compositor_drag == Some(true);
    let released = native_drag_released(
        &grab,
        if grab.bridge_drag {
            None
        } else {
            compositor_drag
        },
        snapshot
            .as_ref()
            .filter(|snapshot| snapshot.own_window_rect.is_some())
            .map(|snapshot| snapshot.received_at),
        buttons.pressed(MouseButton::Left),
        window.focused,
        std::time::Instant::now(),
    );
    if released {
        grab.active = false;
    }
    if grab.active {
        if settings.sit_requested.take() == Some(true) {
            settings.sitting_status =
                "Finish moving the companion before requesting a seat.".into();
        }
        settings.sitting = false;
        if grab.bridge_drag
            && let (Some(sample), Some(bridge)) = (&snapshot, &platform.bridge)
            && let Some(rect) = sample.own_window_rect
        {
            let frame = InteractionFrame {
                avatar_origin: rect.origin,
                avatar_size: window.resolution.size(),
                hips_offset: Vec2::ZERO,
                root_offset: Vec2::ZERO,
                feet_offset: None,
                pixels_per_unit: 1.,
                surfaces: &[],
                own_window_ids: &[],
            };
            let output = interaction.handle(
                InteractionInput::Move {
                    cursor: sample.cursor,
                },
                &frame,
            );
            if let Err(error) = bridge.request_move(output.window_origin) {
                settings.sitting_status = format!("Could not move the companion: {error}");
                grab.active = false;
                *interaction = DesktopInteraction::default();
            }
        }
        return;
    }
    // Native Wayland intentionally does not disclose global surface positions.
    // Do not reuse a guessed/stale monitor origin to pretend it can track windows.
    let Some(origin) = logical_origin else {
        fail_sit_request(
            &mut settings,
            &mut interaction,
            "Sitting needs trusted window coordinates. Enable window detection and try again.",
        );
        return;
    };
    let Some(hip_transform) = bones
        .iter()
        .find(|(entity, name, _)| **name == HumanBoneName::Hips && descendant(*entity))
        .map(|(_, _, transform)| transform)
    else {
        fail_sit_request(
            &mut settings,
            &mut interaction,
            "Sitting needs the avatar's projected hips.",
        );
        return;
    };
    let Ok(hips) = camera.world_to_viewport(camera_transform, hip_transform.translation()) else {
        fail_sit_request(
            &mut settings,
            &mut interaction,
            "The avatar's hips are outside the desktop viewport.",
        );
        return;
    };
    let Some(root_offset) = globals.get(root).ok().and_then(|root| {
        camera
            .world_to_viewport(camera_transform, root.translation())
            .ok()
    }) else {
        fail_sit_request(
            &mut settings,
            &mut interaction,
            "Sitting needs the avatar's projected root.",
        );
        return;
    };
    let feet = bones
        .iter()
        .filter(|(entity, name, _)| {
            descendant(*entity)
                && matches!(
                    **name,
                    HumanBoneName::LeftFoot
                        | HumanBoneName::RightFoot
                        | HumanBoneName::LeftToes
                        | HumanBoneName::RightToes
                )
        })
        .filter_map(|(_, _, transform)| {
            camera
                .world_to_viewport(camera_transform, transform.translation())
                .ok()
        })
        .max_by(|a, b| a.y.total_cmp(&b.y));
    let pixels_per_unit = camera
        .world_to_viewport(camera_transform, hip_transform.translation() + Vec3::Y)
        .map_or(window.height() / 2.0, |above| above.distance(hips));
    let scale = window.scale_factor();
    // A monitor bottom is not the taskbar top. Without a compositor snapshot,
    // leave placement manual instead of snapping the avatar under an unknown bar.
    let surfaces = snapshot
        .as_ref()
        .map_or(&[][..], |snapshot| snapshot.surfaces.as_slice());
    let own_ids = snapshot
        .as_ref()
        .map_or(&[][..], |snapshot| snapshot.own_window_ids.as_slice());
    let frame = InteractionFrame {
        avatar_origin: origin,
        avatar_size: window.resolution.size(),
        hips_offset: hips,
        root_offset,
        feet_offset: feet,
        pixels_per_unit,
        surfaces,
        own_window_ids: own_ids,
    };
    let requested = settings.sit_requested.take();
    let output = if let Some(sitting) = requested {
        let (output, status) = resolve_sit_request(&mut interaction, &frame, sitting);
        settings.sitting_status = status.into();
        output
    } else if released {
        let cursor = snapshot.as_ref().map_or_else(
            || frame.avatar_origin + cursor.unwrap_or(hips),
            |snapshot| snapshot.cursor,
        );
        finish_native_drop(&mut interaction, &frame, cursor)
    } else if interaction.state == InteractionState::Sitting {
        interaction.tick(&frame, time.delta_secs())
    } else {
        settings.sitting = false;
        return;
    };
    // Standing up moves the window so the standing feet stay on the seat edge.
    let stood_up = requested == Some(false) && output.window_origin != frame.avatar_origin;
    if output.state == InteractionState::Sitting || stood_up {
        let placement = if snapshot.is_some() {
            platform
                .bridge
                .as_ref()
                .ok_or_else(|| "Window detection is disconnected.".to_string())
                .and_then(|bridge| bridge.request_move(output.window_origin))
        } else if native_origin.is_some() {
            window.position = WindowPosition::At((output.window_origin * scale).as_ivec2());
            Ok(())
        } else {
            Err("The compositor does not support positioning this window.".into())
        };
        match placement {
            Err(error) if stood_up => {
                // The seat is already released; the companion stays where it was.
                settings.sitting_status =
                    format!("Stood up, but could not move onto the edge: {error}");
            }
            Err(error) => {
                // A detected surface is not enough if the host rejected placement.
                // Drop the seated pose instead of leaving it floating in place.
                settings.sitting_status = format!("Could not sit on the surface: {error}");
                settings.sitting = false;
                *interaction = DesktopInteraction::default();
                return;
            }
            Ok(()) => {}
        }
    }
    settings.sitting = output.state == InteractionState::Sitting;
}

/// Apply an explicit sit/stand request against the current trusted frame.
/// Standing leaves a live seat with the projected root on its edge.
fn resolve_sit_request(
    interaction: &mut DesktopInteraction,
    frame: &InteractionFrame<'_>,
    sitting: bool,
) -> (
    waifu_studio::desktop_interaction::InteractionOutput,
    &'static str,
) {
    if sitting {
        let output = interaction.sit_on_surface_below(frame);
        let status = if output.state == InteractionState::Sitting {
            "Sitting on a detected surface."
        } else {
            "Drag the companion onto a window edge or taskbar, then release to sit."
        };
        (output, status)
    } else {
        let was_seated = interaction.state == InteractionState::Sitting;
        let output = interaction.stand_up(frame);
        (output, if was_seated { "Standing up." } else { "" })
    }
}

fn fail_sit_request(
    settings: &mut DesktopSettings,
    interaction: &mut DesktopInteraction,
    notice: &str,
) {
    if settings.sit_requested.take() == Some(true) || settings.sitting {
        settings.sitting_status = notice.into();
    }
    settings.sitting = false;
    *interaction = DesktopInteraction::default();
}

fn native_drag_released(
    grab: &DesktopGrab,
    compositor_drag: Option<bool>,
    sampled_at: Option<std::time::Instant>,
    left_pressed: bool,
    focused: bool,
    now: std::time::Instant,
) -> bool {
    const START_GRACE: std::time::Duration = std::time::Duration::from_millis(150);
    if !grab.active || compositor_drag == Some(true) {
        return false;
    }
    let waiting = compositor_drag.is_some()
        && sampled_at.is_some()
        && !grab.compositor_seen
        && grab
            .started
            .is_some_and(|start| now.saturating_duration_since(start) < START_GRACE);
    // KWin can start and finish a short drag between samples, consuming the
    // button release. Only a distinct sample received after the startup grace
    // can confirm this case; repeatedly cloning a cached premove frame cannot.
    let fresh_native_end = compositor_drag == Some(false)
        && grab
            .started
            .zip(sampled_at)
            .is_some_and(|(start, at)| at.saturating_duration_since(start) >= START_GRACE);
    !waiting && (grab.compositor_seen || fresh_native_end || !left_pressed || !focused)
}

fn finish_native_drop(
    interaction: &mut DesktopInteraction,
    frame: &InteractionFrame<'_>,
    cursor: Vec2,
) -> waifu_studio::desktop_interaction::InteractionOutput {
    // The actual mesh hit started this native drag. Geometry may have arrived
    // after that hit, so establish the portable drag state before releasing it.
    if interaction.state != InteractionState::Dragging {
        interaction.handle(InteractionInput::Start { cursor }, frame);
    }
    interaction.handle(InteractionInput::NativeEnd { cursor }, frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use waifu_studio::desktop_interaction::{DesktopSurface, ScreenRect, SurfaceId};

    #[test]
    fn camera_mask_is_created_updated_and_removed_without_idle_reinsertion() {
        #[derive(Resource)]
        struct Desired(DesktopOcclusion);
        #[derive(Component)]
        struct TestCamera;
        fn sync(
            mut commands: Commands,
            desired: Res<Desired>,
            cameras: Query<(Entity, Option<&DesktopOcclusion>), With<TestCamera>>,
        ) {
            for (entity, old) in &cameras {
                sync_camera_mask(&mut commands, entity, old, desired.0);
            }
        }
        let mut app = App::new();
        app.insert_resource(Desired(DesktopOcclusion::default()))
            .add_systems(Update, sync);
        let entity = app.world_mut().spawn(TestCamera).id();
        app.update();
        assert!(app.world().get::<DesktopOcclusion>(entity).is_none());
        let rect = ScreenRect {
            origin: Vec2::ZERO,
            size: Vec2::splat(100.),
        };
        let snap = DesktopSnapshot {
            received_at: Instant::now(),
            surfaces: vec![DesktopSurface {
                id: SurfaceId::Window(1),
                rect,
                z_order: 1,
            }],
            own_window_ids: Vec::new(),
            own_window_rect: Some(rect),
            own_interactive_move: None,
            own_above_panels: None,
            cursor: Vec2::ZERO,
        };
        let mask = |depth| {
            DesktopOcclusion::for_seat(&snap, SurfaceId::Window(1), depth, snap.received_at)
        };
        app.world_mut().resource_mut::<Desired>().0 = mask(0.3);
        app.update();
        assert!(
            app.world()
                .get::<DesktopOcclusion>(entity)
                .unwrap()
                .hides(Vec2::splat(0.5), 0.2)
        );
        let before = app
            .world()
            .entity(entity)
            .get_ref::<DesktopOcclusion>()
            .unwrap()
            .last_changed();
        app.update();
        let after = app
            .world()
            .entity(entity)
            .get_ref::<DesktopOcclusion>()
            .unwrap()
            .last_changed();
        assert_eq!(before, after, "unchanged masks must not be reinserted");
        app.world_mut().resource_mut::<Desired>().0 = mask(0.6);
        app.update();
        assert!(
            app.world()
                .get::<DesktopOcclusion>(entity)
                .unwrap()
                .hides(Vec2::splat(0.5), 0.5)
        );
        app.world_mut().resource_mut::<Desired>().0 = DesktopOcclusion::default();
        app.update();
        assert!(
            app.world().get::<DesktopOcclusion>(entity).is_none(),
            "stale ledge must be removed"
        );
    }

    #[test]
    fn verified_snap_uses_usable_bounds_above_the_panel() {
        let mut snapshot = DesktopSnapshot {
            received_at: Instant::now(),
            surfaces: Vec::new(),
            own_window_ids: Vec::new(),
            own_window_rect: None,
            own_interactive_move: None,
            own_above_panels: None,
            cursor: Vec2::ZERO,
        };
        snapshot.surfaces = vec![DesktopSurface {
            id: SurfaceId::Monitor(1),
            rect: ScreenRect {
                origin: Vec2::new(-1920., 0.),
                size: Vec2::new(1920., 1040.),
            },
            z_order: i32::MAX,
        }];
        let mut settings = DesktopSettings::default();
        settings.perch.mode = waifu_studio::desktop_perch::PerchMode::ScreenRight;
        let p = verified_snap_placement(
            &settings,
            &snapshot,
            Vec2::new(-500., 500.),
            Vec2::new(420., 640.),
        )
        .unwrap();
        assert!(p.window_origin[1] + p.avatar_size[1] <= 1040.);
        assert!(p.window_origin[0] + p.avatar_size[0] <= 0.);
        snapshot.surfaces.clear();
        assert!(
            verified_snap_placement(&settings, &snapshot, Vec2::ZERO, Vec2::new(420., 640.))
                .is_none()
        );
    }

    #[test]
    fn snap_routes_logical_coordinates_through_bridge_and_only_uses_supported_native_fallback() {
        let request = DesktopMoveRequest {
            logical_origin: Vec2::new(-300., 480.),
            physical_origin: IVec2::new(-600, 960),
            native_position_supported: false,
        };
        let mut window = Window::default();
        let original = window.position;
        let sent = std::cell::Cell::new(None);
        let status = route_move_request(
            request,
            &mut window,
            Some(&|origin| {
                sent.set(Some(origin));
                Ok(())
            }),
        );
        assert_eq!(sent.get(), Some(request.logical_origin));
        assert_eq!(window.position, original);
        assert!(status.contains("waiting for the compositor"));
        let status = route_move_request(request, &mut window, None);
        assert_eq!(window.position, original);
        assert!(status.starts_with("Could not snap:"));
        let failed = route_move_request(
            request,
            &mut window,
            Some(&|_| Err("Current geometry expired".into())),
        );
        assert!(failed.contains("geometry expired"));
        assert_eq!(window.position, original);
        let status = route_move_request(
            DesktopMoveRequest {
                native_position_supported: true,
                ..request
            },
            &mut window,
            None,
        );
        assert_eq!(window.position, WindowPosition::At(request.physical_origin));
        assert!(!status.starts_with("Could not"));
    }

    #[test]
    fn ordinary_companion_always_receives_clicks_and_passthrough_requires_explicit_recovery() {
        for recovery_ready in [false, true] {
            assert!(
                desktop_accepts_input(false, recovery_ready),
                "A failed or stale mesh/hover test must never disable native input"
            );
        }
        assert!(
            desktop_accepts_input(true, false),
            "Never disable the only recovery surface"
        );
        assert!(
            !desktop_accepts_input(true, true),
            "Explicit full passthrough remains available"
        );
    }

    #[test]
    fn trusted_global_pointer_recovers_after_native_leave_and_rejects_cached_or_missing_geometry() {
        let now = Instant::now();
        let mut snapshot = DesktopSnapshot {
            received_at: now,
            surfaces: Vec::new(),
            own_window_ids: Vec::new(),
            own_window_rect: Some(ScreenRect {
                origin: Vec2::new(-500., 200.),
                size: Vec2::new(420., 640.),
            }),
            own_interactive_move: Some(false),
            own_above_panels: None,
            cursor: Vec2::new(-350., 350.),
        };
        assert_eq!(
            trusted_viewport_cursor(&snapshot, now),
            Some(Vec2::new(150., 150.))
        );
        assert!(trusted_viewport_cursor(&snapshot, now + Duration::from_millis(501)).is_none());
        snapshot.own_window_rect = None;
        assert!(trusted_viewport_cursor(&snapshot, now).is_none());
    }

    #[test]
    fn failed_sit_request_clears_pose_and_explains_why() {
        let mut settings = DesktopSettings::default();
        settings.sitting = true;
        settings.request_sitting(true);
        let mut interaction = DesktopInteraction::default();
        interaction.state = InteractionState::Sitting;
        fail_sit_request(
            &mut settings,
            &mut interaction,
            "No trusted surface is available.",
        );
        assert!(!settings.sitting && settings.sit_requested.is_none());
        assert_eq!(interaction.state, InteractionState::Idle);
        assert_eq!(settings.sitting_status, "No trusted surface is available.");
    }
    #[test]
    fn stand_request_moves_feet_onto_the_seat_and_only_reports_a_real_seat() {
        let surfaces = [DesktopSurface {
            id: SurfaceId::Window(4),
            rect: ScreenRect {
                origin: Vec2::new(0., 700.),
                size: Vec2::new(1200., 300.),
            },
            z_order: 0,
        }];
        let mut frame = InteractionFrame {
            avatar_origin: Vec2::new(200., 100.),
            avatar_size: Vec2::new(400., 480.),
            hips_offset: Vec2::new(150., 200.),
            root_offset: Vec2::new(150., 420.),
            feet_offset: Some(Vec2::new(150., 420.)),
            pixels_per_unit: 200.,
            surfaces: &surfaces,
            own_window_ids: &[],
        };
        let mut interaction = DesktopInteraction::default();
        frame.avatar_origin.y = 700. - frame.hips_offset.y - 10.;
        let (seated, status) = resolve_sit_request(&mut interaction, &frame, true);
        assert_eq!(seated.state, InteractionState::Sitting);
        assert_eq!(status, "Sitting on a detected surface.");
        frame.avatar_origin = seated.window_origin;
        let (stood, status) = resolve_sit_request(&mut interaction, &frame, false);
        assert_eq!(stood.state, InteractionState::Idle);
        assert_eq!(status, "Standing up.");
        assert_eq!(stood.window_origin, Vec2::new(seated.window_origin.x, 280.));
        // Not seated: no motion and no misleading status.
        frame.avatar_origin = stood.window_origin;
        let (again, status) = resolve_sit_request(&mut interaction, &frame, false);
        assert_eq!(again.window_origin, frame.avatar_origin);
        assert!(status.is_empty());
    }
    #[test]
    fn short_native_drag_ends_from_a_fresh_sample_when_the_button_release_was_consumed() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let grab = DesktopGrab {
            active: true,
            started: Some(start),
            ..default()
        };
        let now = start + Duration::from_millis(200);
        assert!(native_drag_released(
            &grab,
            Some(false),
            Some(start + Duration::from_millis(160)),
            true,
            true,
            now
        ));
        assert!(
            !native_drag_released(
                &grab,
                Some(false),
                Some(start - Duration::from_millis(20)),
                true,
                true,
                now
            ),
            "A cached premove snapshot must not finish an active native drag"
        );
        assert!(
            !native_drag_released(
                &grab,
                Some(false),
                Some(start + Duration::from_millis(100)),
                true,
                true,
                now
            ),
            "A sample captured during startup grace is still insufficient"
        );
        assert!(
            !native_drag_released(
                &grab,
                Some(true),
                Some(start + Duration::from_millis(160)),
                true,
                true,
                now
            ),
            "An active compositor drag must keep the grabbed state"
        );
    }

    #[test]
    fn unsupported_native_drag_status_never_means_released_while_held() {
        let start = Instant::now();
        let grab = DesktopGrab {
            active: true,
            started: Some(start),
            ..default()
        };
        let now = start + Duration::from_secs(3);
        assert!(!native_drag_released(
            &grab,
            None,
            Some(now),
            true,
            true,
            now
        ));
        assert!(!native_drag_released(&grab, None, None, true, true, now));
        assert!(native_drag_released(
            &grab,
            None,
            Some(now),
            false,
            true,
            now
        ));
        assert!(native_drag_released(
            &grab,
            None,
            Some(now),
            true,
            false,
            now
        ));
        // Ordinary bridge drags must also finish on a quick release, without
        // waiting out the grace used only for observable native grabs.
        let quick = start + Duration::from_millis(20);
        assert!(native_drag_released(
            &grab,
            None,
            Some(quick),
            false,
            true,
            quick
        ));
    }

    #[test]
    fn mesh_confirmed_native_drop_can_seat_when_geometry_arrives_after_the_drag_started() {
        let surfaces = [DesktopSurface {
            id: SurfaceId::WorkArea(7),
            rect: ScreenRect {
                origin: Vec2::ZERO,
                size: Vec2::new(1920., 1032.),
            },
            z_order: i32::MAX,
        }];
        let frame = InteractionFrame {
            avatar_origin: Vec2::new(200., 600.),
            avatar_size: Vec2::new(400., 480.),
            hips_offset: Vec2::new(150., 200.),
            root_offset: Vec2::new(150., 200.),
            feet_offset: Some(Vec2::new(150., 420.)),
            pixels_per_unit: 200.,
            surfaces: &surfaces,
            own_window_ids: &[],
        };
        let out = finish_native_drop(
            &mut DesktopInteraction::default(),
            &frame,
            Vec2::new(350., 720.),
        );
        assert_eq!(out.state, InteractionState::Sitting);
        assert_eq!(out.hips_anchor, Some(Vec2::new(350., 1032.)));
        assert_eq!(out.window_origin, Vec2::new(200., 832.));
    }

    #[test]
    fn actual_homunculus_motions_load_and_keep_humanoid_tracks() {
        let mut motions = DesktopMotions::default();
        motions.initialize();
        assert_eq!(motions.clips.len(), 3);
        for (_, clip, _) in &motions.clips {
            assert!(clip.mapped_bones() >= 10);
            assert!(clip.duration() > 0.0);
            assert_eq!(clip.format(), "VRMA");
        }
    }
    #[test]
    fn desktop_motions_restore_the_complete_user_motion_on_exit_or_disable() {
        for disable in [false, true] {
            let directory =
                std::env::temp_dir().join(format!("studio-motion-{}", uuid::Uuid::new_v4()));
            let (store, library) = waifu_studio::data::Store::open(directory.clone()).unwrap();
            let mut studio = Studio::new(store, library);
            studio.mode = ViewMode::Desktop;
            let original = Arc::new(
                parse_vrma(include_bytes!(
                    "../assets/animations/desktop/idle-maid.vrma"
                ))
                .unwrap(),
            );
            let mut animation = AnimationState::default();
            animation.select_clip(original.clone(), Some("my-custom-motion.vrma".into()));
            animation.position = 1.25;
            animation.speed = 0.75;
            animation.weight = 0.4;
            animation.looping = false;
            animation.pause();
            let mut app = App::new();
            app.insert_resource(studio)
                .insert_resource(animation)
                .init_resource::<DesktopSettings>()
                .init_resource::<DesktopGrab>()
                .init_resource::<DesktopMotions>()
                .init_resource::<DesktopInteraction>()
                .add_systems(Update, play_desktop_motion);
            // A stale/manual flag cannot activate a pose without a real seat.
            app.world_mut().resource_mut::<DesktopSettings>().sitting = true;
            app.update();
            assert!(!Arc::ptr_eq(
                app.world()
                    .resource::<AnimationState>()
                    .clip
                    .as_ref()
                    .unwrap(),
                &original
            ));
            assert!(
                app.world()
                    .resource::<AnimationState>()
                    .path
                    .as_ref()
                    .unwrap()
                    .ends_with("idle-maid.vrma")
            );
            assert_eq!(app.world().resource::<AnimationState>().weight, 0.4);
            app.world_mut().resource_mut::<DesktopInteraction>().state = InteractionState::Sitting;
            app.update();
            assert!(
                app.world()
                    .resource::<AnimationState>()
                    .path
                    .as_ref()
                    .unwrap()
                    .ends_with("idle-sitting.vrma")
            );
            assert_eq!(app.world().resource::<AnimationState>().weight, 0.4);
            app.world_mut().resource_mut::<DesktopGrab>().active = true;
            app.update();
            assert!(
                app.world()
                    .resource::<AnimationState>()
                    .path
                    .as_ref()
                    .unwrap()
                    .ends_with("grabbed.vrma")
            );
            if disable {
                app.world_mut()
                    .resource_mut::<DesktopSettings>()
                    .automatic_motions = false;
            } else {
                app.world_mut().resource_mut::<Studio>().mode = ViewMode::Studio;
            }
            app.update();
            let restored = app.world().resource::<AnimationState>();
            assert!(Arc::ptr_eq(restored.clip.as_ref().unwrap(), &original));
            assert_eq!(
                restored.path.as_deref(),
                Some(std::path::Path::new("my-custom-motion.vrma"))
            );
            assert_eq!(
                (restored.position, restored.speed, restored.weight),
                (1.25, 0.75, 0.4)
            );
            assert!(!restored.playing && !restored.looping);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}
