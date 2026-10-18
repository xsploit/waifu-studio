//! A second render view of the live avatar, docked beside visual settings.
use crate::studio::{Studio, Tab, ViewMode};
use bevy::{
    camera::{RenderTarget, primitives::Aabb},
    prelude::*,
    render::render_resource::TextureFormat,
};
use bevy_egui::{EguiTextureHandle, EguiUserTextures, egui};
use waifu_studio::loading::AvatarLoadState;

pub struct AvatarPreviewPlugin;
impl Plugin for AvatarPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AvatarPreview>()
            .add_systems(Startup, setup)
            .add_systems(Update, update);
    }
}
#[derive(Resource)]
pub struct AvatarPreview {
    texture: Option<egui::TextureId>,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    size: UVec2,
    root: Option<Entity>,
    bounds: Option<(Vec3, Vec3)>,
    framing: Framing,
}
impl Default for AvatarPreview {
    fn default() -> Self {
        Self {
            texture: None,
            yaw: 0.0,
            pitch: 0.0,
            zoom: 1.0,
            size: UVec2::new(600, 900),
            root: None,
            bounds: None,
            framing: Framing::FullBody,
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Framing {
    FullBody,
    UpperBody,
    Face,
}
impl Framing {
    fn bounds(self, min: Vec3, max: Vec3) -> (Vec3, Vec3) {
        let height = max.y - min.y;
        let center = (max + min) * 0.5;
        match self {
            Self::FullBody => (min, max),
            Self::UpperBody => (
                Vec3::new(center.x - height * 0.29, min.y + height * 0.42, min.z),
                Vec3::new(center.x + height * 0.29, max.y, max.z),
            ),
            Self::Face => (
                Vec3::new(center.x - height * 0.17, min.y + height * 0.72, min.z),
                Vec3::new(center.x + height * 0.17, max.y, max.z),
            ),
        }
    }
}
#[derive(Component)]
struct PreviewCamera;
#[derive(Resource)]
struct PreviewImage(Handle<Image>);
fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<EguiUserTextures>,
    mut preview: ResMut<AvatarPreview>,
) {
    let image = images.add(Image::new_target_texture(
        600,
        900,
        TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    preview.texture = Some(textures.add_image(EguiTextureHandle::Strong(image.clone())));
    commands.spawn((
        PreviewCamera,
        waifu_studio::visuals::VisualCamera,
        Camera3d::default(),
        Camera {
            order: -1,
            is_active: false,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.055, 0.065, 0.062)),
            ..default()
        },
        RenderTarget::Image(image.clone().into()),
        Projection::Perspective(PerspectiveProjection {
            fov: 35_f32.to_radians(),
            ..default()
        }),
        Transform::from_xyz(0.0, 0.9, 3.8).looking_at(Vec3::Y * 0.9, Vec3::Y),
    ));
    commands.insert_resource(PreviewImage(image));
}

fn frame(min: Vec3, max: Vec3, aspect: f32, zoom: f32, yaw: f32, pitch: f32) -> Transform {
    let extent = (max - min).max(Vec3::splat(0.01));
    let focus = (min + max) * 0.5;
    let half_fov = 17.5_f32.to_radians();
    let radius = ((extent.y * 0.58 / half_fov.tan())
        .max(extent.x * 0.58 / (half_fov.tan() * aspect.max(0.1)))
        + extent.z * 0.5)
        / zoom;
    let direction = Quat::from_euler(EulerRot::YXZ, yaw, -pitch, 0.0) * Vec3::Z;
    Transform::from_translation(focus + direction * radius).looking_at(focus, Vec3::Y)
}
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update(
    studio: Res<Studio>,
    state: Res<AvatarLoadState>,
    mut preview: ResMut<AvatarPreview>,
    target: Res<PreviewImage>,
    mut images: ResMut<Assets<Image>>,
    parents: Query<&ChildOf>,
    meshes: Query<(Entity, &Aabb, &GlobalTransform), With<Mesh3d>>,
    mut cameras: Query<(&mut Camera, &mut Transform), With<PreviewCamera>>,
) {
    let Ok((mut camera, mut transform)) = cameras.single_mut() else {
        return;
    };
    camera.is_active =
        studio.mode == ViewMode::Studio && matches!(studio.tab, Tab::Characters | Tab::Scene);
    if !camera.is_active {
        return;
    }
    if state.active != preview.root {
        preview.root = state.active;
        preview.bounds = None;
        preview.yaw = 0.0;
        preview.pitch = 0.0;
        preview.zoom = 1.0;
    }
    if preview.bounds.is_none()
        && let Some(root) = state.active
    {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for (entity, aabb, global) in &meshes {
            if entity != root && !parents.iter_ancestors(entity).any(|p| p == root) {
                continue;
            }
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    for z in [-1.0, 1.0] {
                        let point = global.transform_point(
                            Vec3::from(aabb.center)
                                + Vec3::from(aabb.half_extents) * Vec3::new(x, y, z),
                        );
                        min = min.min(point);
                        max = max.max(point);
                    }
                }
            }
        }
        if min.is_finite() && max.is_finite() {
            preview.bounds = Some((min, max));
        }
    }
    let size = preview.size;
    if let Some(mut image) = images.get_mut(&target.0)
        && (image.width() != size.x || image.height() != size.y)
    {
        image.resize(bevy::render::render_resource::Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        });
    }
    if let Some((min, max)) = preview.bounds {
        let (min, max) = preview.framing.bounds(min, max);
        *transform = frame(
            min,
            max,
            size.x as f32 / size.y as f32,
            preview.zoom,
            preview.yaw,
            preview.pitch,
        );
    }
}

/// Reserve a visible dock before creating the controls' independent scroll area.
/// Narrow windows keep a short, full-width live preview above the controls.
pub fn dock(ui: &mut egui::Ui, preview: &mut AvatarPreview) {
    let available = ui.available_size();
    if available.x >= 860.0 {
        egui::Panel::right("live_avatar_preview")
            .exact_size((available.x * 0.31).clamp(280.0, 520.0))
            .resizable(true)
            .show(ui, |ui| widget(ui, preview, ui.available_height()));
    } else {
        widget(ui, preview, (available.y * 0.32).clamp(140.0, 280.0));
        ui.separator();
    }
}
fn widget(ui: &mut egui::Ui, preview: &mut AvatarPreview, height: f32) {
    let top = ui.cursor().top();
    ui.heading("Live preview");
    egui::ComboBox::from_id_salt("preview_framing")
        .width(ui.available_width())
        .selected_text(match preview.framing {
            Framing::FullBody => "Full body",
            Framing::UpperBody => "Upper body",
            Framing::Face => "Face & hair",
        })
        .show_ui(ui, |ui| {
            for (value, label) in [
                (Framing::FullBody, "Full body"),
                (Framing::UpperBody, "Upper body"),
                (Framing::Face, "Face & hair"),
            ] {
                if ui
                    .selectable_value(&mut preview.framing, value, label)
                    .changed()
                {
                    preview.zoom = 1.0;
                }
            }
        });
    ui.label("Drag to rotate · scroll to zoom");
    let size = egui::vec2(
        ui.available_width(),
        (height - (ui.cursor().top() - top) - 44.0).max(60.0),
    );
    if let Some(texture) = preview.texture {
        let response = ui.add(egui::Image::new((texture, size)).sense(egui::Sense::drag()));
        let pixels = ui.ctx().pixels_per_point();
        preview.size = UVec2::new(
            (size.x * pixels).round().clamp(128.0, 1536.0) as u32,
            (size.y * pixels).round().clamp(128.0, 2048.0) as u32,
        );
        if response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            preview.yaw -= delta.x * 0.008;
            preview.pitch = (preview.pitch + delta.y * 0.008).clamp(-1.2, 1.2);
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            preview.zoom = (preview.zoom * (scroll * 0.003).exp()).clamp(0.4, 4.0);
        }
    } else {
        ui.allocate_space(size);
    }
    if ui.button("Reset view").clicked() {
        preview.yaw = 0.0;
        preview.pitch = 0.0;
        preview.zoom = 1.0;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_contains_full_avatar_at_portrait_and_landscape_aspects() {
        let min = Vec3::new(-0.5, 0.0, -0.2);
        let max = Vec3::new(0.5, 1.8, 0.2);
        for aspect in [0.35, 0.67, 1.0, 2.5] {
            let pose = frame(min, max, aspect, 1.0, 0.0, 0.0);
            for x in [min.x, max.x] {
                for y in [min.y, max.y] {
                    for z in [min.z, max.z] {
                        let p = pose
                            .to_matrix()
                            .inverse()
                            .transform_point3(Vec3::new(x, y, z));
                        let half = (-p.z) * 17.5_f32.to_radians().tan();
                        assert!(p.z < 0.0 && p.y.abs() < half && p.x.abs() < half * aspect);
                    }
                }
            }
        }
    }
}
