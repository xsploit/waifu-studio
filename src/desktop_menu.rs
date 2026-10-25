//! Desktop radial actions; right-click, hover and release, or click a wedge.
use crate::voice_hotkeys::{Action as VoiceAction, VoiceHotkeys};
use crate::{
    desktop_behavior::{DesktopGrab, DesktopPlatform},
    desktop_settings::DesktopSettings,
    studio::{Studio, Tab, ViewMode},
};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use egui::{Pos2, Vec2 as V2};
mod paint;
mod switcher;
use std::f32::consts::{FRAC_PI_2, TAU};
const RADIUS: f32 = 142.0;
const INNER: f32 = 49.0;
#[derive(Resource, Default)]
pub(crate) struct DesktopMenu {
    center: Option<Pos2>,
    picker: switcher::Switcher,
    voice_page: bool,
    opened_at: f64,
    suppress_left: bool,
    voice_hover: bool,
}
impl DesktopMenu {
    pub fn is_open(&self) -> bool {
        self.center.is_some() || self.picker.open
    }
    pub fn blocks_drag(&self) -> bool {
        self.is_open() || self.suppress_left || self.voice_hover
    }
}
#[derive(Clone, Copy)]
enum Action {
    Settings,
    Character,
    Personality,
    Voice,
    Sitting,
    Passthrough,
    Studio,
    Reframe,
    Close,
    Microphone,
    ToggleVoice,
    StopVoice,
    VoiceSettings,
}
fn label(action: Action, sitting: bool) -> &'static str {
    match action {
        Action::Settings => "Settings",
        Action::Character => "Profiles",
        Action::Personality => "Personality",
        Action::Voice => "Voice",
        Action::Sitting if sitting => "Stand up",
        Action::Sitting => "Sit down",
        Action::Passthrough => "Pass clicks",
        Action::Studio => "Studio",
        Action::Reframe => "Reframe",
        Action::Close => "Close",
        Action::Microphone => "Microphone",
        Action::ToggleVoice => "Voice replies",
        Action::StopVoice => "Stop voice",
        Action::VoiceSettings => "Hotkeys",
    }
}
fn hovered(delta: V2, radius: f32, count: usize) -> Option<usize> {
    if !(radius * INNER / RADIUS..=radius).contains(&delta.length()) {
        return None;
    }
    let angle = (delta.y.atan2(delta.x) + FRAC_PI_2 + TAU / (2.0 * count as f32)).rem_euclid(TAU);
    Some(((angle / (TAU / count as f32)) as usize).min(count - 1))
}
fn radial_radius(screen: egui::Rect) -> f32 {
    RADIUS.min(((screen.width().min(screen.height()) - 24.0) * 0.5).max(1.0))
}
fn center_inside(pointer: Pos2, screen: egui::Rect) -> Pos2 {
    let padding = radial_radius(screen) + 4.0;
    Pos2::new(
        pointer.x.clamp(
            screen.left() + padding,
            (screen.right() - padding).max(screen.left() + padding),
        ),
        pointer.y.clamp(
            screen.top() + padding,
            (screen.bottom() - padding).max(screen.top() + padding),
        ),
    )
}
/// A seated companion can extend below the monitor so its feet hang over the
/// taskbar. Fit menus to the visible part, not the full backing window. Geometry
/// may be physical native pixels or logical compositor pixels; convert it to
/// egui points using the client-size ratio, so DPI and UI scaling stay separate.
fn visible_screen(
    screen: egui::Rect,
    window: egui::Rect,
    monitors: impl Iterator<Item = egui::Rect>,
    pointer: Option<Pos2>,
) -> egui::Rect {
    if !window.is_finite() || window.width() <= 0.0 || window.height() <= 0.0 {
        return screen;
    }
    let scale = screen.size() / window.size();
    monitors
        .filter(|monitor| monitor.is_finite())
        .filter_map(|monitor| {
            let visible = window.intersect(monitor);
            if !visible.is_positive() {
                return None;
            }
            Some(egui::Rect::from_min_max(
                screen.min + (visible.min - window.min) * scale,
                screen.min + (visible.max - window.min) * scale,
            ))
        })
        .max_by(|a, b| {
            let under_pointer = |rect: &egui::Rect| pointer.is_some_and(|p| rect.contains(p));
            under_pointer(a)
                .cmp(&under_pointer(b))
                .then_with(|| a.area().total_cmp(&b.area()))
        })
        .unwrap_or(screen)
}
fn menu_screen(
    ctx: &egui::Context,
    window: Option<&Window>,
    platform: &DesktopPlatform,
    monitors: &Query<&bevy::window::Monitor>,
) -> egui::Rect {
    let screen = ctx.content_rect();
    let pointer = ctx.pointer_latest_pos();
    let rect = |origin: Vec2, size: Vec2| {
        egui::Rect::from_min_size(Pos2::new(origin.x, origin.y), V2::new(size.x, size.y))
    };
    if let Some(snapshot) = platform.snapshot()
        && let Some(client) = snapshot.own_window_rect
    {
        return visible_screen(
            screen,
            rect(client.origin, client.size),
            snapshot.surfaces.iter().filter_map(|surface| {
                matches!(
                    surface.id,
                    waifu_studio::desktop_interaction::SurfaceId::Monitor(_)
                )
                .then(|| rect(surface.rect.origin, surface.rect.size))
            }),
            pointer,
        );
    }
    if let Some(window) = window
        && let bevy::window::WindowPosition::At(origin) = window.position
    {
        return visible_screen(
            screen,
            rect(
                origin.as_vec2(),
                window.resolution.physical_size().as_vec2(),
            ),
            monitors.iter().map(|monitor| {
                rect(
                    monitor.physical_position.as_vec2(),
                    Vec2::new(
                        monitor.physical_width as f32,
                        monitor.physical_height as f32,
                    ),
                )
            }),
            pointer,
        );
    }
    screen
}
const ACTIONS: [Action; 9] = [
    Action::Settings,
    Action::Character,
    Action::Voice,
    Action::Personality,
    Action::Sitting,
    Action::Passthrough,
    Action::Studio,
    Action::Reframe,
    Action::Close,
];
const VOICE_ACTIONS: [Action; 5] = [
    Action::Microphone,
    Action::ToggleVoice,
    Action::StopVoice,
    Action::VoiceSettings,
    Action::Close,
];
#[allow(clippy::too_many_arguments)]
pub(crate) fn ui(
    mut contexts: EguiContexts,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    grab: Res<DesktopGrab>,
    platform: Res<DesktopPlatform>,
    primary: Query<&Window, With<bevy::window::PrimaryWindow>>,
    monitors: Query<&bevy::window::Monitor>,
    mut menu: ResMut<DesktopMenu>,
    mut studio: ResMut<Studio>,
    mut settings: ResMut<DesktopSettings>,
    mut hotkeys: ResMut<VoiceHotkeys>,
    listening: Res<waifu_studio::listen::ListeningState>,
) {
    menu.voice_hover = false;
    if !buttons.pressed(MouseButton::Left) {
        menu.suppress_left = false;
    }
    if studio.mode != ViewMode::Desktop || grab.active {
        menu.center = None;
        menu.picker.close();
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    crate::ui::ensure_style(ctx);
    let screen = menu_screen(ctx, primary.single().ok(), &platform, &monitors);
    if keys.just_pressed(KeyCode::Escape) {
        menu.center = None;
        menu.picker.close();
    }
    if buttons.just_pressed(MouseButton::Right)
        && !studio.click_through
        && primary
            .single()
            .ok()
            .and_then(Window::cursor_position)
            .is_some()
        && let Some(pointer) = ctx.pointer_latest_pos()
    {
        menu.center = Some(center_inside(pointer, screen));
        menu.voice_page = false;
        menu.opened_at = ctx.input(|i| i.time);
        menu.picker.close();
    }
    if let Some(center) = menu.center {
        let center = center_inside(center, screen);
        menu.center = Some(center);
        let radius = radial_radius(screen);
        let items: &[Action] = if menu.voice_page {
            &VOICE_ACTIONS
        } else {
            &ACTIONS
        };
        let origin = center - V2::splat(radius + 4.0);
        let mut action = None;
        egui::Area::new("desktop_radial".into())
            .order(egui::Order::Foreground)
            .fixed_pos(origin)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(
                    V2::splat((radius + 4.0) * 2.0),
                    egui::Sense::click_and_drag(),
                );
                let center = rect.center();
                let active = ui
                    .input(|input| input.pointer.hover_pos())
                    .and_then(|pointer| hovered(pointer - center, radius, items.len()));
                paint::wheel(
                    ui.painter(),
                    &paint::Wheel {
                        center,
                        radius,
                        items,
                        active,
                        sitting: settings.sitting,
                        recording: listening.is_recording(),
                        voice_enabled: studio.library.voice.enabled,
                        voice_page: menu.voice_page,
                        name: studio
                            .library
                            .selected_character()
                            .map_or("Your companion", |c| c.name.as_str()),
                        opened_at: menu.opened_at,
                    },
                );
                if let Some(index) = active {
                    if buttons.just_released(MouseButton::Right)
                        || buttons.just_pressed(MouseButton::Left)
                    {
                        action = Some(items[index]);
                    }
                } else if buttons.just_pressed(MouseButton::Left) {
                    action = Some(Action::Close);
                }
            });
        if let Some(action) = action {
            menu.suppress_left |= buttons.pressed(MouseButton::Left);
            menu.center = None;
            match action {
                Action::Settings => {
                    studio.tab = Tab::Scene;
                    studio.mode = ViewMode::Studio;
                }
                Action::Character => {
                    menu.picker.show(switcher::Page::Profiles);
                }
                Action::Personality => menu.picker.show(switcher::Page::Personality),
                Action::Voice => {
                    menu.voice_page = true;
                    menu.opened_at = ctx.input(|i| i.time);
                    menu.center = Some(center);
                }
                Action::VoiceSettings => hotkeys.queue(VoiceAction::Settings),
                Action::Sitting => {
                    let sitting = !settings.sitting;
                    settings.request_sitting(sitting);
                }
                Action::Passthrough => {
                    // Full passthrough needs an explicit recovery surface;
                    // ordinary companion startup never creates these controls.
                    settings.controls_requested = true;
                    studio.click_through = true;
                }
                Action::Studio => {
                    studio.mode = ViewMode::Studio;
                    studio.tab = Tab::Chat;
                }
                Action::Reframe => studio.frame_requested = true,
                Action::Close => {}
                Action::Microphone => hotkeys.queue(VoiceAction::Microphone),
                Action::ToggleVoice => hotkeys.queue(VoiceAction::Voice),
                Action::StopVoice => hotkeys.queue(VoiceAction::Stop),
            }
        }
    }
    if menu.picker.ui(ctx, &mut studio, screen) {
        menu.suppress_left = true;
    }
    if listening.is_busy() {
        let response = egui::Area::new("desktop_voice_status".into())
            .fixed_pos(screen.left_bottom() + V2::new(8.0, -80.0))
            .constrain_to(screen)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width((screen.width() - 24.0).max(140.0));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(if listening.is_recording() {
                            "● Recording"
                        } else {
                            "Transcribing…"
                        });
                        if listening.is_recording() && ui.small_button("Stop").clicked() {
                            hotkeys.queue(VoiceAction::Microphone);
                            menu.suppress_left = true;
                        }
                        if ui.small_button("Cancel").clicked() {
                            hotkeys.queue(VoiceAction::Stop);
                            menu.suppress_left = true;
                        }
                    });
                });
            });
        menu.voice_hover = response.response.contains_pointer();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn radial_wedges_have_a_dead_zone_and_match_compass_directions() {
        assert_eq!(hovered(V2::ZERO, RADIUS, 8), None);
        for (point, index) in [
            (V2::new(0.0, -85.0), 0),
            (V2::new(85.0, 0.0), 2),
            (V2::new(0.0, 85.0), 4),
            (V2::new(-85.0, 0.0), 6),
        ] {
            assert_eq!(hovered(point, RADIUS, 8), Some(index));
        }
        assert_eq!(hovered(V2::splat(200.0), RADIUS, 8), None);
        for items in [&ACTIONS[..], &VOICE_ACTIONS[..]] {
            for index in 0..items.len() {
                let angle = index as f32 * TAU / items.len() as f32 - FRAC_PI_2;
                assert_eq!(
                    hovered(V2::angled(angle) * 85.0, RADIUS, items.len()),
                    Some(index)
                );
            }
        }
    }
    #[test]
    fn sitting_wedge_offers_stand_up_only_while_seated() {
        assert_eq!(label(Action::Sitting, false), "Sit down");
        assert_eq!(label(Action::Sitting, true), "Stand up");
    }
    #[test]
    fn radial_stays_inside_small_and_normal_companion_windows() {
        for size in [
            V2::new(170.0, 256.0),
            V2::new(210.0, 320.0),
            V2::new(420.0, 640.0),
        ] {
            let screen = egui::Rect::from_min_size(Pos2::ZERO, size);
            let radius = radial_radius(screen) + 4.0;
            for pointer in [screen.min, screen.max] {
                let center = center_inside(pointer, screen);
                assert!(screen.contains(center - V2::splat(radius)));
                assert!(screen.contains(center + V2::splat(radius)));
            }
        }
    }
    #[test]
    fn seated_menu_fits_the_visible_monitor_at_both_dpi_scales() {
        let screen = egui::Rect::from_min_size(Pos2::ZERO, V2::new(420.0, 640.0));
        for dpi in [1.0, 2.0] {
            let window = egui::Rect::from_min_size(
                Pos2::new(1500.0 * dpi, 715.0 * dpi),
                screen.size() * dpi,
            );
            let monitor = egui::Rect::from_min_size(Pos2::ZERO, V2::new(1920.0, 1080.0) * dpi);
            let visible = visible_screen(screen, window, [monitor].into_iter(), None);
            assert_eq!(visible.size(), V2::new(420.0, 365.0));
            for pointer in [screen.min, screen.center(), screen.max] {
                let center = center_inside(pointer, visible);
                let padding = V2::splat(radial_radius(visible) + 4.0);
                assert!(visible.contains(center - padding));
                assert!(visible.contains(center + padding));
            }
        }
    }
    #[test]
    fn visible_menu_region_handles_negative_outputs_and_pointer_monitor() {
        let screen = egui::Rect::from_min_size(Pos2::ZERO, V2::new(420.0, 640.0));
        let window = egui::Rect::from_min_size(Pos2::new(-210.0, 900.0), screen.size());
        let left = egui::Rect::from_min_size(Pos2::new(-1920.0, 0.0), V2::new(1920.0, 1080.0));
        let right = egui::Rect::from_min_size(Pos2::ZERO, V2::new(1920.0, 1080.0));
        let visible = visible_screen(
            screen,
            window,
            [left, right].into_iter(),
            Some(Pos2::new(315.0, 80.0)),
        );
        assert_eq!(visible.min, Pos2::new(210.0, 0.0));
        assert_eq!(visible.size(), V2::new(210.0, 180.0));
        let center = center_inside(screen.max, visible);
        let padding = V2::splat(radial_radius(visible) + 4.0);
        assert!(visible.contains(center - padding));
        assert!(visible.contains(center + padding));
    }
    #[test]
    fn unknown_monitor_geometry_keeps_the_existing_viewport() {
        let screen = egui::Rect::from_min_size(Pos2::ZERO, V2::new(420.0, 640.0));
        assert_eq!(visible_screen(screen, screen, [].into_iter(), None), screen);
        assert_eq!(
            visible_screen(screen, egui::Rect::NOTHING, [screen].into_iter(), None),
            screen
        );
    }
}
