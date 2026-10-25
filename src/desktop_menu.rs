//! Desktop radial actions; right-click, hover and release, or click a wedge.
use crate::voice_hotkeys::{Action as VoiceAction, VoiceHotkeys};
use crate::{
    desktop_behavior::DesktopGrab,
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
    primary: Query<&Window, With<bevy::window::PrimaryWindow>>,
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
        menu.center = Some(center_inside(pointer, ctx.content_rect()));
        menu.voice_page = false;
        menu.opened_at = ctx.input(|i| i.time);
        menu.picker.close();
    }
    if let Some(center) = menu.center {
        let radius = radial_radius(ctx.content_rect());
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
    if menu.picker.ui(ctx, &mut studio) {
        menu.suppress_left = true;
    }
    if listening.is_busy() {
        let response = egui::Area::new("desktop_voice_status".into())
            .anchor(egui::Align2::CENTER_BOTTOM, V2::new(0.0, -8.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width((ctx.content_rect().width() - 24.0).max(140.0));
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
}
