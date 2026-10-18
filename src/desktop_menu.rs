//! Mate-inspired desktop radial: right-click, hover, release; center dead zone.
use crate::{
    desktop_behavior::{DesktopGrab, DesktopPointerHit},
    desktop_settings::DesktopSettings,
    studio::{Studio, Tab, ViewMode},
};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use egui::{Color32, Pos2, Vec2 as V2};
use std::f32::consts::{FRAC_PI_2, TAU};
const RADIUS: f32 = 122.0;
const INNER: f32 = 40.0;
#[derive(Resource, Default)]
pub(crate) struct DesktopMenu {
    center: Option<Pos2>,
    picker: bool,
    search: String,
    suppress_left: bool,
}
impl DesktopMenu {
    pub fn is_open(&self) -> bool {
        self.center.is_some() || self.picker
    }
    pub fn blocks_drag(&self) -> bool {
        self.is_open() || self.suppress_left
    }
}
#[derive(Clone, Copy)]
enum Action {
    Settings,
    Character,
    Voice,
    Sitting,
    Passthrough,
    Studio,
    Reframe,
    Close,
}
const ACTIONS: [Action; 8] = [
    Action::Settings,
    Action::Character,
    Action::Voice,
    Action::Sitting,
    Action::Passthrough,
    Action::Studio,
    Action::Reframe,
    Action::Close,
];
fn label(action: Action, sitting: bool) -> &'static str {
    match action {
        Action::Settings => "Settings",
        Action::Character => "Character",
        Action::Voice => "Voice",
        Action::Sitting if sitting => "Stand up",
        Action::Sitting => "Sit down",
        Action::Passthrough => "Pass clicks",
        Action::Studio => "Studio",
        Action::Reframe => "Reframe",
        Action::Close => "Close",
    }
}
fn hovered(delta: V2) -> Option<usize> {
    if !(INNER..=RADIUS).contains(&delta.length()) {
        return None;
    }
    let angle = (delta.y.atan2(delta.x) + FRAC_PI_2 + TAU / 16.0).rem_euclid(TAU);
    Some(((angle / (TAU / 8.0)) as usize).min(7))
}
fn center_inside(pointer: Pos2, screen: egui::Rect) -> Pos2 {
    let padding = RADIUS + 9.0;
    Pos2::new(
        if screen.width() >= padding * 2.0 {
            pointer
                .x
                .clamp(screen.left() + padding, screen.right() - padding)
        } else {
            screen.center().x
        },
        if screen.height() >= padding * 2.0 {
            pointer
                .y
                .clamp(screen.top() + padding, screen.bottom() - padding)
        } else {
            screen.center().y
        },
    )
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn ui(
    mut contexts: EguiContexts,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    grab: Res<DesktopGrab>,
    hit: Res<DesktopPointerHit>,
    mut menu: ResMut<DesktopMenu>,
    mut studio: ResMut<Studio>,
    mut settings: ResMut<DesktopSettings>,
) {
    if !buttons.pressed(MouseButton::Left) {
        menu.suppress_left = false;
    }
    if studio.mode != ViewMode::Desktop || grab.active {
        menu.center = None;
        menu.picker = false;
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    if keys.just_pressed(KeyCode::Escape) {
        menu.center = None;
        menu.picker = false;
    }
    if buttons.just_pressed(MouseButton::Right)
        && !studio.click_through
        && hit.avatar_hit
        && let Some(pointer) = ctx.pointer_latest_pos()
    {
        menu.center = Some(center_inside(pointer, ctx.content_rect()));
        menu.picker = false;
    }
    if let Some(center) = menu.center {
        let origin = center - V2::splat(RADIUS + 4.0);
        let mut action = None;
        egui::Area::new("desktop_radial".into())
            .order(egui::Order::Foreground)
            .fixed_pos(origin)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(
                    V2::splat((RADIUS + 4.0) * 2.0),
                    egui::Sense::click_and_drag(),
                );
                let center = rect.center();
                let active = ui
                    .input(|input| input.pointer.hover_pos())
                    .and_then(|pointer| hovered(pointer - center));
                let painter = ui.painter();
                painter.circle_filled(
                    center,
                    RADIUS + 4.0,
                    Color32::from_rgba_unmultiplied(12, 15, 19, 245),
                );
                for (index, item) in ACTIONS.iter().enumerate() {
                    let middle = index as f32 * TAU / 8.0 - FRAC_PI_2;
                    let start = middle - TAU / 16.0 + 0.015;
                    let end = middle + TAU / 16.0 - 0.015;
                    let color = if active == Some(index) {
                        Color32::from_rgb(108, 65, 86)
                    } else {
                        Color32::from_rgb(37, 41, 48)
                    };
                    let mut mesh = egui::Mesh::default();
                    for step in 0..=16 {
                        let direction = V2::angled(start + (end - start) * step as f32 / 16.0);
                        mesh.colored_vertex(center + direction * INNER, color);
                        mesh.colored_vertex(center + direction * RADIUS, color);
                        if step > 0 {
                            let end = step * 2;
                            mesh.add_triangle(end - 2, end - 1, end);
                            mesh.add_triangle(end - 1, end + 1, end);
                        }
                    }
                    painter.add(egui::Shape::mesh(mesh));
                    painter.text(
                        center + V2::angled(middle) * 82.0,
                        egui::Align2::CENTER_CENTER,
                        label(*item, settings.sitting),
                        egui::FontId::proportional(12.0),
                        Color32::from_rgb(238, 237, 241),
                    );
                }
                painter.circle_filled(center, INNER - 3.0, Color32::from_rgb(26, 29, 35));
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    "MENU",
                    egui::FontId::proportional(11.0),
                    Color32::from_rgb(178, 182, 194),
                );
                if let Some(index) = active {
                    if buttons.just_released(MouseButton::Right)
                        || buttons.just_pressed(MouseButton::Left)
                    {
                        action = Some(ACTIONS[index]);
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
                    menu.picker = true;
                    menu.search.clear();
                }
                Action::Voice => {
                    studio.tab = Tab::Voice;
                    studio.mode = ViewMode::Studio;
                }
                Action::Sitting => {
                    let sitting = !settings.sitting;
                    settings.request_sitting(sitting);
                }
                Action::Passthrough => studio.click_through = true,
                Action::Studio => {
                    studio.mode = ViewMode::Studio;
                    studio.tab = Tab::Chat;
                }
                Action::Reframe => studio.frame_requested = true,
                Action::Close => {}
            }
        }
    }
    if menu.picker {
        let mut open = true;
        let mut selected = None;
        egui::Window::new("Change companion")
            .open(&mut open)
            .default_width(390.0)
            .constrain_to(ctx.content_rect())
            .show(ctx, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut menu.search)
                        .hint_text("Search your characters…")
                        .desired_width(f32::INFINITY),
                );
                let search = menu.search.to_lowercase();
                egui::ScrollArea::vertical()
                    .max_height(380.0)
                    .show(ui, |ui| {
                        for character in &studio.library.characters {
                            if character.name.to_lowercase().contains(&search)
                                && ui
                                    .add_sized(
                                        [ui.available_width(), 36.0],
                                        egui::Button::new(&character.name).selected(
                                            studio.library.selected == Some(character.id),
                                        ),
                                    )
                                    .clicked()
                            {
                                selected = Some(character.id);
                            }
                        }
                    });
                ui.separator();
                if ui.button("Import another VRM…").clicked() {
                    studio.open_models();
                }
            });
        if let Some(id) = selected {
            menu.suppress_left = true;
            studio.select_character(id);
            menu.picker = false;
        }
        if !open {
            menu.picker = false;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn radial_wedges_have_a_dead_zone_and_match_compass_directions() {
        assert_eq!(hovered(V2::ZERO), None);
        assert_eq!(hovered(V2::new(0.0, -85.0)), Some(0));
        assert_eq!(hovered(V2::new(85.0, 0.0)), Some(2));
        assert_eq!(hovered(V2::new(0.0, 85.0)), Some(4));
        assert_eq!(hovered(V2::new(-85.0, 0.0)), Some(6));
        assert_eq!(hovered(V2::splat(200.0)), None);
    }
    #[test]
    fn radial_stays_inside_the_viewport_at_screen_edges() {
        let screen = egui::Rect::from_min_size(Pos2::ZERO, V2::new(420.0, 640.0));
        for pointer in [screen.min, screen.max] {
            let center = center_inside(pointer, screen);
            assert!(screen.contains(center - V2::splat(RADIUS)));
            assert!(screen.contains(center + V2::splat(RADIUS)));
        }
    }
}
